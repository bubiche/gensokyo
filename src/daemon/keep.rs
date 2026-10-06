//! `keep`: a ritual run that has sat finished longer than its keep is asked to leave, and its
//! record retired, so the session is still in recall. So is a helper whose lead has gone.

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
        let s = shrine.clone();
        spawn(shrine, id.clone(), async move { take(&s, &id, launched).await });
    }
}

/// One due to go, taken in a task of its own; `reaping` keeps the ticks meanwhile off it.
fn spawn(shrine: &Shared, id: String, f: impl Future<Output = ()> + 'static) {
    let shrine = shrine.clone();
    tokio::task::spawn_local(async move {
        f.await;
        shrine.borrow_mut().rites.reaping.remove(&id);
    });
}

/// Out of the shrine and into recall, once the exit it was asked for (if it was) is recorded.
async fn retire(shrine: &Shared, id: &str) -> bool {
    let t = Instant::now();
    let departed = || shrine.borrow().entries.iter().any(|e| e.rec.id == id && e.handle.is_none());
    while !departed() && t.elapsed() < Duration::from_secs(2) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    shrine::close(shrine, id).await.is_ok()
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
    if !retire(shrine, id).await {
        return;
    }
    let how = if live.is_some() { "since the run finished" } else { "after it left" };
    Dir::of(&slug).note(now(), "closed", &format!("closed {name}, idle {keep_s} {how} (keep)"));
    log(json!({"ev": "ritual", "slug": slug, "closed": name}));
}

/// How long a helper whose lead has departed sits idle before it is closed, in seconds.
const ORPHAN: i64 = 2 * 3600;

/// Helpers whose lead has departed, however it went, and which have been idle (finished,
/// resting or departed themselves) for `ORPHAN` since: closed, so they are still in recall. The
/// lead's recall stops the clock, as does work.
pub(super) fn orphans(shrine: &Shared, now: i64) {
    let mut due = Vec::new();
    {
        let mut sh = shrine.borrow_mut();
        for i in 0..sh.entries.len() {
            let e = &sh.entries[i];
            let left = e.rec.owner.as_ref().is_some_and(|l| !live(&sh, l)) && idle(e);
            let e = &mut sh.entries[i];
            e.orphan_since = left.then(|| e.orphan_since.unwrap_or(now));
            let id = e.rec.id.clone();
            if orphaned(&sh, &id, now) && sh.rites.reaping.insert(id.clone()) {
                due.push(id);
            }
        }
    }
    for id in due {
        let s = shrine.clone();
        spawn(shrine, id.clone(), async move { orphan(&s, &id).await });
    }
}

fn live(sh: &shrine::Shrine, id: &str) -> bool {
    sh.entries.iter().any(|e| e.rec.id == id && e.handle.is_some())
}

fn idle(e: &shrine::Entry) -> bool {
    e.handle.is_none() || e.aware.idle()
}

/// Still a helper left behind `ORPHAN` ago, idle, and on nobody's focused screen. Asked again
/// before every /exit: its lead may be back, or the user at it.
fn orphaned(sh: &shrine::Shrine, id: &str, now: i64) -> bool {
    let Some(e) = sh.entries.iter().find(|e| e.rec.id == id) else { return false };
    let lead_gone = e.rec.owner.as_ref().is_some_and(|l| !live(sh, l));
    let due = e.orphan_since.is_some_and(|since| now - since >= ORPHAN);
    !sh.quitting && lead_gone && due && idle(e) && !sh_views(&sh.views)(id)
}

/// One orphan asked to /exit, then out of the shrine once its exit is recorded. One that will
/// not go starts its clock again, rather than being asked every tick.
async fn orphan(shrine: &Shared, id: &str) {
    if !orphaned(&shrine.borrow(), id, now()) {
        return;
    }
    if live(&shrine.borrow(), id) && shrine::close(shrine, id).await.is_err() {
        let mut sh = shrine.borrow_mut();
        if let Some(e) = sh.entries.iter_mut().find(|e| e.rec.id == id) {
            e.orphan_since = Some(now());
        }
        return;
    }
    let who = shrine.borrow().entries.iter().find(|e| e.rec.id == id).map(|e| e.rec.name.clone());
    if retire(shrine, id).await {
        log(json!({"ev": "orphan", "id": id, "closed": who}));
    }
}
