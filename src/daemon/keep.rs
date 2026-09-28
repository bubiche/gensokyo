//! `keep`: a ritual run that has sat finished longer than its keep is asked to leave, and its
//! record retired, so the session is still in recall.

use super::log::log;
use super::resident::Handle;
use super::rituals::now;
use super::shrine::{self, Shared, ask_once};
use crate::ritual::Dir;
use crate::tele;
use serde_json::json;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Ritual runs that have sat finished (or departed) longer than their `keep`: asked to leave,
/// and their records retired, so the session is still in recall.
pub(super) fn reap(shrine: &Shared, now: i64) {
    let mut due = Vec::new();
    {
        let mut sh = shrine.borrow_mut();
        let sh = &mut *sh;
        for e in sh.entries.iter_mut().filter(|e| e.rec.ritual.is_some()) {
            let idle = e.handle.is_none() || e.aware.finished();
            if !idle {
                e.idle_since = None;
                continue;
            }
            e.idle_since.get_or_insert(now);
            if expired(sh_views(&sh.views), e, now) && sh.rites.reaping.insert(e.rec.id.clone()) {
                due.push((e.rec.id.clone(), e.rec.launched));
            }
        }
    }
    for (id, launched) in due {
        let shrine = shrine.clone();
        tokio::task::spawn_local(async move {
            take(&shrine, &id, launched).await;
            shrine.borrow_mut().rites.reaping.remove(&id);
        });
    }
}

fn sh_views(v: &HashMap<u64, shrine::View>) -> impl Fn(&str) -> bool + '_ {
    move |id| v.values().any(|(who, focused)| *focused && who.as_deref() == Some(id))
}

/// Idle past its keep, and not on anyone's focused screen: someone looking at it is using it.
fn expired(watched: impl Fn(&str) -> bool, e: &shrine::Entry, now: i64) -> bool {
    match (e.rec.keep, e.idle_since) {
        (Some(keep), Some(since)) => now - since >= keep as i64 && !watched(&e.rec.id),
        _ => false,
    }
}

/// Still the run `reap` chose (not recalled since, still idle past its keep): its name, keep,
/// ritual, and its handle while it is live. Asked again before every /exit, since the user may
/// have come back to it in between.
type Still = (String, u64, String, Option<Rc<Handle>>);

fn still(shrine: &Shared, id: &str, launched: i64) -> Option<Still> {
    let sh = shrine.borrow();
    if sh.quitting {
        return None;
    }
    let e = sh.entries.iter().find(|e| e.rec.id == id && e.rec.launched == launched)?;
    let idle = e.handle.is_none() || e.aware.finished();
    (idle && expired(sh_views(&sh.views), e, now())).then(|| {
        let (keep, slug) = (e.rec.keep.unwrap_or(0), e.rec.ritual.clone().unwrap_or_default());
        (e.rec.name.clone(), keep, slug, e.handle.clone())
    })
}

async fn take(shrine: &Shared, id: &str, launched: i64) {
    let Some((name, keep, slug, live)) = still(shrine, id, launched) else { return };
    let keep_s = tele::age(keep);
    if let Some(mut h) = live.clone() {
        let mut left = false;
        for _ in 0..2 {
            if ask_once(&h).await {
                left = true;
                break;
            }
            match still(shrine, id, launched) {
                Some((.., Some(again))) => h = again,
                _ => return,
            }
        }
        if !left {
            // Said once: the keep goes, so the clock does not come back with another /exit.
            let mut sh = shrine.borrow_mut();
            if let Some(e) = sh.entries.iter_mut().find(|e| e.rec.id == id) {
                e.rec.keep = None;
                let rec = e.rec.clone();
                let _ = sh.store.save(&rec);
            }
            let text = format!("{name} did not answer /exit, so it stays (keep {keep_s})");
            Dir::of(&slug).note(now(), "kept", &text);
            return;
        }
    }
    // Out of the shrine, and into recall, once its exit has been recorded.
    let t = Instant::now();
    let departed = || shrine.borrow().entries.iter().any(|e| e.rec.id == id && e.handle.is_none());
    while !departed() && t.elapsed() < Duration::from_secs(2) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    if shrine::close(shrine, id).await.is_err() {
        return;
    }
    let how = if live.is_some() { "since the run finished" } else { "after it left" };
    Dir::of(&slug).note(now(), "closed", &format!("closed {name}, idle {keep_s} {how} (keep)"));
    log(json!({"ev": "ritual", "slug": slug, "closed": name}));
}
