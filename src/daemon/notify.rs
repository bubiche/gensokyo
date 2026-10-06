//! Who hears that a resident needs the user, and when a finished turn has been seen.

use super::log::log;
use super::shrine::{Shrine, touch};
use crate::proto::{Reply, State};
use serde_json::json;

/// Watchers hear of the change, and of a resident that has just come to need the user. A
/// turn that finishes while someone watches it is seen at once.
pub(super) fn after(sh: &mut Shrine, i: usize, before: State) {
    let id = &sh.entries[i].rec.id;
    let watched = sh.views.values().any(|(v, f)| *f && v.as_ref() == Some(id));
    if watched {
        sh.entries[i].aware.seen();
    }
    let mut now = sh.entries[i].aware.state();
    if now.needs_you() && now != before && (quiet(sh, i) || super::lead::hush(sh, i)) {
        now = sh.entries[i].aware.state();
    }
    // A lead done being busy: what it held of its helpers and did not collect rings now.
    if before == State::Busy && now != State::Busy {
        let lead = sh.entries[i].rec.id.clone();
        super::lead::release(sh, &lead);
    }
    let e = &sh.entries[i];
    if now.needs_you() && now != before {
        log(json!({"ev": "notify", "id": e.rec.id, "state": now, "watched": watched}));
        let (who, name, text) = (e.rec.id.clone(), e.rec.name.clone(), e.aware.notice(&e.rec.name));
        let _ = sh.notices.send(Reply::Notify { who, name, state: now, text, watched });
    }
    touch(sh);
}

/// A quiet ritual run's turn has just finished: seen at once, so it neither rings nor turns
/// gold. Its dialogs still ring: a run stuck at one does nothing more.
fn quiet(sh: &mut Shrine, i: usize) -> bool {
    let e = &mut sh.entries[i];
    if !e.rec.quiet || e.aware.pending != Some(super::aware::Pending::Stopped) {
        return false;
    }
    e.aware.seen();
    log(json!({"ev": "quiet", "id": e.rec.id, "ritual": e.rec.ritual}));
    true
}

/// Connection `me` changed what it views or its focus: a resident it now shows in a focused
/// terminal has been seen.
pub(super) fn looked(sh: &mut Shrine, me: u64) {
    let Some((Some(who), true)) = sh.views.get(&me).cloned() else { return };
    let e = sh.entries.iter_mut().find(|e| e.rec.id == who && e.handle.is_some());
    if e.is_some_and(|e| e.aware.seen()) {
        touch(sh);
    }
}
