//! One resident's actor: it owns the PTY master and the emulator, answers the child's terminal
//! queries, and reports the exit that `wait()` returns.

use super::pty::{self, OwnedReadPty, OwnedWritePty, Size};
use crate::vt::Vt;
use std::cell::Cell;
use std::os::unix::process::ExitStatusExt;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

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
    resize: mpsc::Sender<(u16, u16)>,
    exit: watch::Receiver<Option<Exit>>,
    last_output: Rc<Cell<Instant>>,
}

impl Handle {
    pub async fn input(&self, bytes: &[u8]) -> bool {
        self.out.send(Out::Bytes(bytes.to_vec())).await.is_ok()
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.resize.try_send((cols, rows));
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
    let (resize, resize_rx) = mpsc::channel(4);
    let (exit_tx, exit) = watch::channel(None);
    let last_output = Rc::new(Cell::new(Instant::now()));
    tokio::task::spawn_local(writer(w, out_rx));
    let actor = Actor { vt: Vt::new(cols, rows), out: out.clone(), last: last_output.clone() };
    tokio::task::spawn_local(actor.run(r, child, resize_rx, exit_tx));
    Ok(Handle { pid, out, resize, exit, last_output })
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
    vt: Vt,
    out: mpsc::Sender<Out>,
    last: Rc<Cell<Instant>>,
}

impl Actor {
    async fn run(
        mut self,
        mut r: OwnedReadPty,
        mut child: tokio::process::Child,
        mut resize: mpsc::Receiver<(u16, u16)>,
        exit: watch::Sender<Option<Exit>>,
    ) {
        let mut buf = vec![0u8; 65536];
        let mut eof = false;
        // Read until wait() returns: a leader cannot finish exiting while its output is unread.
        let status = loop {
            tokio::select! {
                n = r.read(&mut buf), if !eof => match n {
                    Ok(n) if n > 0 => self.feed(&buf[..n]),
                    // Every slave fd is closed; the exit follows.
                    _ => eof = true,
                },
                st = child.wait() => break st,
                // Both sizes change or neither: a resize dropped on a full queue is dropped here too.
                Some((cols, rows)) = resize.recv() => {
                    if self.out.try_send(Out::Resize(Size::new(rows.max(1), cols.max(1)))).is_ok() {
                        self.vt.resize(cols, rows);
                    }
                }
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
        self.vt.feed(bytes);
        let replies = self.vt.take_replies();
        if !replies.is_empty() {
            let _ = self.out.try_send(Out::Bytes(replies));
        }
    }
}
