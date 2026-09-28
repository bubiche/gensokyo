//! What residents report from inside: their hooks, which may come late through the spool,
//! and their status line.

use super::log::log;
use super::notify::after;
use super::shrine::{Shared, Shrine, touch};
use super::store::{self, Record};
use crate::hooks;
use crate::proto::{Hook, Telemetry};
use serde_json::json;

/// One hook from inside a resident. A SessionStart moves its record on to the new session,
/// even once it has departed: a /clear the daemon hears of only from the spool.
pub(super) fn hook(shrine: &Shared, resident: &str, h: Hook) {
    let mut sh = shrine.borrow_mut();
    let sh = &mut *sh;
    let at = sh.entries.iter().position(|e| e.rec.id == resident);
    if let (true, Some(s)) = (h.event == "SessionStart", h.session.as_deref()) {
        rotate(sh, at, resident, s, h.at);
    }
    let Some(i) = at.filter(|&i| sh.entries[i].handle.is_some()) else { return };
    let before = sh.entries[i].aware.state();
    if sh.entries[i].aware.hook(&h) {
        // Typed into: a ritual run's `keep` starts again from its next finish.
        if h.event == "UserPromptSubmit" {
            sh.entries[i].idle_since = None;
        }
        let state = sh.entries[i].aware.state();
        log(
            json!({"ev": "hook", "id": resident, "event": h.event, "kind": h.kind, "state": state}),
        );
        after(sh, i, before);
    }
}

fn rotate(sh: &mut Shrine, at: Option<usize>, resident: &str, session: &str, when: i64) {
    let fresh = |r: &Record| when >= r.session_at && r.session != session;
    let saved = match at {
        Some(i) => {
            let r = &mut sh.entries[i].rec;
            if !fresh(r) {
                return;
            }
            (r.session, r.session_at) = (session.into(), when);
            let r = r.clone();
            sh.store.save(&r)
        }
        None => match sh.store.load_departed_id(resident).filter(|r| fresh(r)) {
            Some(mut r) => {
                (r.session, r.session_at) = (session.into(), when);
                sh.store.save_departed(&r)
            }
            None => return,
        },
    };
    log(
        json!({"ev": "session", "id": resident, "session": session, "error": saved.err().map(|e| e.to_string())}),
    );
}

pub(super) fn statusline(shrine: &Shared, resident: &str, mut t: Telemetry) {
    let mut sh = shrine.borrow_mut();
    let Some(e) = sh.entries.iter_mut().find(|e| e.rec.id == resident && e.handle.is_some()) else {
        return;
    };
    // Claude Code redraws its status line several times a second; watchers hear of a change.
    t.at = store::now();
    let changed = e.tele.as_ref().is_none_or(|o| Telemetry { at: t.at, ..o.clone() } != t);
    e.tele = Some(t);
    if changed {
        touch(&sh);
    }
}

pub(super) fn replay(shrine: &Shared, settle: i64) {
    let root = shrine.borrow().store.root.clone();
    for (who, h) in hooks::take_spool(&root, settle) {
        hook(shrine, &who, h);
    }
}
