//! What goes down a connection: its writer, the `watch` events, and a resident's screen as a
//! whole frame and then the rows that changed.

use super::resident::Handle;
use super::rituals;
use super::shrine::{Shared, list};
use crate::proto::Reply;
use crate::vt::{Frame, Modes};
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{broadcast, mpsc, oneshot};

/// A viewer gets at most one screen per this, about 60 a second.
const FRAME_GAP: Duration = Duration::from_millis(16);

/// How long a child's synchronized-output block may hold its screen back.
const SYNC_HOLD: Duration = Duration::from_millis(150);

/// One line for the writer, and a word back once it is written.
type Line = (Vec<u8>, Option<oneshot::Sender<()>>);

pub(super) type Out = mpsc::Sender<Line>;

/// A connection's writer, and where its lines go.
pub(super) fn writer(w: OwnedWriteHalf) -> Out {
    let (out, rx) = mpsc::channel(64);
    tokio::task::spawn_local(write_lines(w, rx));
    out
}

fn encode(r: &Reply) -> Vec<u8> {
    let mut b = serde_json::to_vec(r).expect("a reply serializes");
    b.push(b'\n');
    b
}

pub(super) async fn send(out: &Out, r: &Reply) -> bool {
    out.send((encode(r), None)).await.is_ok()
}

/// Returns once the line is written, so a caller that waits on it never has two in flight.
pub(super) async fn send_written(out: &Out, r: &Reply) -> bool {
    let (tx, rx) = oneshot::channel();
    out.send((encode(r), Some(tx))).await.is_ok() && rx.await.is_ok()
}

async fn write_lines(mut w: OwnedWriteHalf, mut rx: mpsc::Receiver<Line>) {
    while let Some((b, done)) = rx.recv().await {
        if w.write_all(&b).await.is_err() {
            return;
        }
        if let Some(d) = done {
            let _ = d.send(());
        }
    }
}

/// A `residents` event now and whenever the shrine changes after, the timetable now and
/// whenever it changes, a crash's notice nobody has had yet, and every `notify` and `notice`.
pub(super) async fn watch(shrine: Shared, out: Out) {
    let (mut rx, mut notices) = {
        let sh = shrine.borrow();
        (sh.changed.subscribe(), sh.notices.subscribe())
    };
    if !send(&out, &rituals::listing(&shrine, 0)).await {
        return;
    }
    if let Some(text) = super::crash::unseen()
        && !send(&out, &Reply::Notice { text }).await
    {
        return;
    }
    loop {
        rx.borrow_and_update();
        if !send(&out, &Reply::Residents { residents: list(&shrine, false) }).await {
            return;
        }
        loop {
            tokio::select! {
                c = rx.changed() => match c {
                    Ok(()) => break,
                    Err(_) => return,
                },
                n = notices.recv() => match n {
                    Ok(r) => if !send(&out, &r).await { return },
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => return,
                },
            }
        }
    }
}

/// What a viewer was last sent, and its number.
struct Sent {
    frame: Frame,
    modes: Modes,
    rev: u64,
}

/// The screen now against what the viewer has: a whole frame for a first screen or a new size,
/// the rows that changed otherwise, nothing when nothing did.
fn next(who: &str, sent: Option<&Sent>, frame: &Frame, modes: Modes) -> Option<Reply> {
    let rev = sent.map_or(0, |s| s.rev);
    let who = who.to_string();
    match sent {
        Some(s) if s.frame.cols == frame.cols && s.frame.rows.len() == frame.rows.len() => {
            let rows = frame.damage(&s.frame);
            let moved = s.frame.cursor != frame.cursor || s.frame.back != frame.back;
            (!rows.is_empty() || moved || s.modes != modes).then(|| {
                let rows = rows.into_iter().map(|y| (y as u16, frame.rows[y].clone()));
                Reply::Damage {
                    who,
                    base: rev,
                    rev: rev + 1,
                    rows: rows.collect(),
                    cursor: frame.cursor,
                    modes,
                    back: frame.back,
                    history: frame.history,
                }
            })
        }
        _ => Some(Reply::Frame { who, rev: rev + 1, frame: frame.clone(), modes }),
    }
}

/// Streams one resident's screen to connection `me`, its own selection drawn. Each is computed
/// when the last has been written, against what this client was last sent, so a slow client
/// skips screens rather than queueing them.
pub(super) async fn view(
    shrine: Shared,
    h: Rc<Handle>,
    who: String,
    out: Out,
    me: u64,
    mut nudge: bool,
) {
    let mut changes = h.changes();
    let mut sent: Option<Sent> = None;
    loop {
        // A child inside its own synchronized-output block is mid-draw.
        let held = Instant::now();
        while h.in_sync() && held.elapsed() < SYNC_HOLD {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        changes.borrow_and_update();
        let (frame, modes) = (h.frame_for(me), h.modes());
        if let Some(r) = next(&who, sent.as_ref(), &frame, modes) {
            if !send_written(&out, &r).await {
                return;
            }
            let rev = sent.as_ref().map_or(0, |s| s.rev) + 1;
            sent = Some(Sent { frame, modes, rev });
        }
        // A client coming back gets the screen it missed, then a SIGWINCH, which makes Claude
        // Code draw everything again. On its own task: a view dropped midway must not leave the
        // resident a row short.
        let (cols, rows) = h.size();
        if std::mem::take(&mut nudge) && rows > 1 {
            let (shrine, h) = (shrine.clone(), h.clone());
            tokio::task::spawn_local(async move {
                h.resize(cols, rows - 1);
                tokio::time::sleep(Duration::from_millis(30)).await;
                let (cols, rows) = shrine.borrow().size;
                h.resize(cols, rows);
            });
        }
        tokio::time::sleep(FRAME_GAP).await;
        if changes.changed().await.is_err() {
            return;
        }
    }
}
