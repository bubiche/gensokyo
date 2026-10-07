//! One resident's actor: it owns the PTY master and feeds the emulator, answers the child's
//! terminal queries, and reports the exit that `wait()` returns. The emulator is shared with
//! whoever shows the screen, on the same thread.

use super::pty::{self, OwnedReadPty, OwnedWritePty, Size};
use crate::vt::{Frame, KeyEvent, Modes, Pointer, Vt};
use std::cell::{Cell, RefCell};
use std::os::unix::process::ExitStatusExt;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, mpsc, watch};

/// Writes queued for the child; a child that stops reading its tty fills this, and whatever
/// is sent to it then waits (input) or is dropped (query replies, which it is not reading).
const QUEUE: usize = 64;

#[derive(Debug, Clone, Copy)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub at: Instant,
}

enum Out {
    Bytes(Vec<u8>),
    Resize(Size),
}

pub struct Handle {
    pub pid: i32,
    out: mpsc::Sender<Out>,
    /// The newest size wins: a burst of them never drops the last, nor does a full queue.
    resize: watch::Sender<(u16, u16)>,
    exit: watch::Receiver<Option<Exit>>,
    last_output: Rc<Cell<Instant>>,
    vt: Rc<RefCell<Vt>>,
    rev: watch::Receiver<u64>,
    /// Asks the actor to count a change the output did not make: a scroll.
    poke: Rc<Notify>,
}

impl Handle {
    pub async fn input(&self, bytes: &[u8]) -> bool {
        self.out.send(Out::Bytes(bytes.to_vec())).await.is_ok()
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        self.resize.send_replace((cols, rows));
    }

    pub fn exit(&self) -> Option<Exit> {
        *self.exit.borrow()
    }

    pub async fn exited(&self) -> Exit {
        let mut rx = self.exit.clone();
        match rx.wait_for(Option::is_some).await {
            Ok(e) => e.expect("waited for Some"),
            // The actor is gone without a word, which it never is; report a bare exit.
            Err(_) => Exit { code: None, signal: None, at: Instant::now() },
        }
    }

    /// When the child last wrote anything.
    pub fn last_output(&self) -> Instant {
        self.last_output.get()
    }

    /// The screen as `viewer` sees it, its own selection drawn.
    pub fn frame_for(&self, viewer: u64) -> Frame {
        self.vt.borrow_mut().frame_for(viewer)
    }

    /// `viewer`'s pointer in the screen (`Vt::select`); a release gives the selected text.
    pub fn select(&self, viewer: u64, how: Pointer, x: u16, y: u16) -> Option<String> {
        let text = self.vt.borrow_mut().select(viewer, how, x, y);
        self.poke.notify_one();
        text
    }

    /// `viewer`'s selection gone.
    pub fn unselect(&self, viewer: u64) {
        if self.vt.borrow_mut().unselect(viewer) {
            self.poke.notify_one();
        }
    }

    /// The live screen as text, however far back the view is: what a check on typed text reads.
    pub fn live_text(&self) -> Vec<String> {
        self.vt.borrow_mut().live_text()
    }

    /// Moves the view through the scrollback (`Vt::scroll`). The view is the resident's, not
    /// a client's: every client showing it scrolls with it.
    pub fn scroll(&self, rows: Option<i32>) {
        self.vt.borrow_mut().scroll(rows);
        self.poke.notify_one();
    }

    /// Looks for `needle` (`Vt::find`); like a scroll, every client showing it sees the view
    /// move and what was found.
    pub fn find(&self, needle: &str, back: bool) -> bool {
        let found = self.vt.borrow_mut().find(needle, back);
        self.poke.notify_one();
        found
    }

    /// Back to the live screen, before anything is typed: nobody types into history, and what
    /// a search found goes.
    pub fn to_live(&self) {
        let vt = self.vt.borrow();
        if vt.scrolled().0 > 0 || vt.finding() {
            drop(vt);
            self.scroll(None);
        }
    }

    pub fn modes(&self) -> Modes {
        self.vt.borrow().modes()
    }

    /// Inside the child's own synchronized-output block (2026): the screen is half drawn.
    pub fn in_sync(&self) -> bool {
        self.vt.borrow().mode(2026)
    }

    pub fn size(&self) -> (u16, u16) {
        self.vt.borrow().size()
    }

    /// The key as the child asked keys to be sent.
    pub fn encode(&self, ev: KeyEvent) -> Vec<u8> {
        self.vt.borrow_mut().encode(ev)
    }

    /// Changes whenever the screen may have: output, a resize. Closed once the child is gone.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.rev.clone()
    }

    /// Waits until the child has written nothing for `quiet`, or `max` has passed.
    pub async fn settle(&self, quiet: Duration, max: Duration) {
        let until = Instant::now() + max;
        while Instant::now() < until && self.last_output.get().elapsed() < quiet {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// Spawns the child on a fresh PTY and starts its actor on the current `LocalSet`.
pub fn start(spawn: pty::Spawn) -> std::io::Result<Handle> {
    let (cols, rows) = (spawn.cols, spawn.rows);
    let (pty, child) = pty::spawn(spawn).map_err(std::io::Error::other)?;
    let pid = child.id().expect("a child just spawned has a pid") as i32;
    let (r, w) = pty.into_split();
    let (out, out_rx) = mpsc::channel(QUEUE);
    let (resize, resize_rx) = watch::channel((cols, rows));
    let (exit_tx, exit) = watch::channel(None);
    let last_output = Rc::new(Cell::new(Instant::now()));
    let vt = Rc::new(RefCell::new(Vt::new(cols, rows)));
    let (rev_tx, rev) = watch::channel(0);
    let poke = Rc::new(Notify::new());
    tokio::task::spawn_local(writer(w, out_rx));
    let actor = Actor {
        vt: vt.clone(),
        rev: rev_tx,
        out: out.clone(),
        last: last_output.clone(),
        poke: poke.clone(),
    };
    tokio::task::spawn_local(actor.run(r, child, resize_rx, exit_tx));
    Ok(Handle { pid, out, resize, exit, last_output, vt, rev, poke })
}

/// Resizes go through the write half, in order with the bytes around them.
async fn writer(mut w: OwnedWritePty, mut rx: mpsc::Receiver<Out>) {
    while let Some(o) = rx.recv().await {
        let ok = match o {
            Out::Bytes(b) => w.write_all(&b).await.is_ok(),
            Out::Resize(s) => w.resize(s).is_ok(),
        };
        if !ok {
            break;
        }
    }
}

struct Actor {
    vt: Rc<RefCell<Vt>>,
    rev: watch::Sender<u64>,
    out: mpsc::Sender<Out>,
    last: Rc<Cell<Instant>>,
    poke: Rc<Notify>,
}

impl Actor {
    async fn run(
        mut self,
        mut r: OwnedReadPty,
        mut child: tokio::process::Child,
        mut resize: watch::Receiver<(u16, u16)>,
        exit: watch::Sender<Option<Exit>>,
    ) {
        let mut buf = vec![0u8; 65536];
        let mut eof = false;
        // A size the write queue had no room for yet: tried again until it goes.
        let mut pending: Option<(u16, u16)> = None;
        // Read until wait() returns: a leader cannot finish exiting while its output is unread.
        let status = loop {
            tokio::select! {
                n = r.read(&mut buf), if !eof => match n {
                    Ok(n) if n > 0 => self.feed(&buf[..n]),
                    // Every slave fd is closed; the exit follows.
                    _ => eof = true,
                },
                st = child.wait() => break st,
                Ok(()) = resize.changed() => pending = Some(*resize.borrow_and_update()),
                () = tokio::time::sleep(Duration::from_millis(50)), if pending.is_some() => {}
                () = self.poke.notified() => self.rev.send_modify(|r| *r += 1),
            }
            // Both sizes change or neither: the emulator's follows the child's.
            if let Some((cols, rows)) = pending
                && self.out.try_send(Out::Resize(Size::new(rows.max(1), cols.max(1)))).is_ok()
            {
                pending = None;
                self.vt.borrow_mut().resize(cols, rows);
                self.rev.send_modify(|r| *r += 1);
            }
        };
        // Take what is already there, but no longer than that: the read end is not ours to wait on.
        if !eof {
            let _ = tokio::time::timeout(Duration::from_millis(100), async {
                while let Ok(n @ 1..) = r.read(&mut buf).await {
                    self.feed(&buf[..n]);
                }
            })
            .await;
        }
        let (code, signal) = match status {
            Ok(s) => (s.code(), s.signal()),
            Err(_) => (None, None),
        };
        let _ = exit.send(Some(Exit { code, signal, at: Instant::now() }));
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.last.set(Instant::now());
        let replies = {
            let mut vt = self.vt.borrow_mut();
            vt.feed(bytes);
            vt.take_replies()
        };
        self.rev.send_modify(|r| *r += 1);
        if !replies.is_empty() {
            let _ = self.out.try_send(Out::Bytes(replies));
        }
    }
}
