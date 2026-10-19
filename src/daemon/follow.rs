//! Cloud sessions in the sidebar, followed on the ritual clock by couriers reading their logs:
//! where each is, and each turn that ends, counted and its answer kept as a resident's are, so
//! `wait` and `read` work on one. Whatever the routine watch is set to: the user started these.
//! One gone from claude.ai departs.

use super::log::log;
use super::shrine::{Entry, Shared, Shrine, cloud_here, depart, touch};
use crate::cloud::{self, Phase};
use crate::proto::Ended;
use serde_json::json;
use std::path::Path;

/// Couriers out at once, the watch's and the user's counted too: a session due a look while
/// that many are out waits for the next tick.
const COURIERS_MOST: usize = 3;

/// Starting or working, a session is looked at every `BUSY` seconds for its first `BUSY_FOR`, then
/// every `BUSY_LATER`; idle, or with nothing new in its log for `QUIET`, every `IDLE` for a day,
/// then hourly, as the user may go on with it on the web. A failing look backs off as the watch
/// does.
const BUSY: i64 = 30;
const BUSY_FOR: i64 = 300;
const BUSY_LATER: i64 = 60;
const QUIET: i64 = 1800;
const IDLE: i64 = 600;
const DAY: i64 = 86400;
const HOURLY: i64 = 3600;

#[derive(Default)]
pub(super) struct Follow {
    pub(super) phase: Phase,
    /// Since when it has been in that phase, or was said to, on the ritual clock.
    since: i64,
    /// When the last courier went for it: a reply finds the look it answers by this.
    looked: Option<i64>,
    /// To be looked at on the next tick, whenever it was last.
    again: bool,
    /// A courier is out for it.
    busy: bool,
    /// Looks in a row that failed.
    fails: u32,
    /// The newest words to it and the newest event of any kind its log has shown, epoch µs.
    words: Option<i64>,
    newest: Option<i64>,
    /// When the user said something to it, and the newest words its log had shown then: until
    /// newer ones show, the words wait in a queue, and it is looked at as a working one is.
    asked: Option<(i64, Option<i64>)>,
    /// Branch tips on origin looked at for its trailer; whether they were since the daemon
    /// started, and whether a look is under way.
    checked: Vec<String>,
    sought: bool,
    seeking: bool,
}

impl Follow {
    fn every(&self, now: i64) -> i64 {
        let long = now - self.since;
        let quiet = self.newest.is_some_and(|t| now - t / 1_000_000 >= QUIET);
        let idle = self.asked.is_none() && (self.phase == Phase::Idle || quiet);
        let pace = match idle {
            false if long < BUSY_FOR => BUSY,
            false => BUSY_LATER,
            true if long < DAY => IDLE,
            true => HOURLY,
        };
        match self.fails {
            0 => pace,
            n => pace.max((BUSY_LATER << n.min(6)).min(HOURLY)),
        }
    }

    /// The user said something to it: looked at again soon, and often.
    pub(super) fn asked(&mut self, now: i64) {
        (self.asked, self.since, self.again) = (Some((now, self.words)), now, true);
    }
}

/// Every tick: a courier for each session due a look, the longest waiting first, as many as
/// there is room for.
pub(super) fn tick(shrine: &Shared, now: i64, catch: bool) {
    loop {
        let mut sh = shrine.borrow_mut();
        if sh.quitting || sh.couriers >= COURIERS_MOST {
            return;
        }
        let due = |e: &&mut Entry| {
            let f = &e.follow;
            let due = f.again || f.looked.is_none_or(|t| now - t >= f.every(now));
            cloud_here(e) && !f.busy && (catch || due)
        };
        let Some(e) = sh.entries.iter_mut().filter(due).min_by_key(|e| e.follow.looked) else {
            return;
        };
        if e.follow.looked.is_none() && e.follow.since == 0 {
            e.follow.since = now;
        }
        (e.follow.busy, e.follow.looked, e.follow.again) = (true, Some(now), false);
        let id = e.rec.id.clone();
        let session = e.rec.cloud.as_ref().map(|c| c.session.clone()).unwrap_or_default();
        sh.couriers += 1;
        drop(sh);
        let shrine = shrine.clone();
        tokio::task::spawn_local(async move {
            let call = json!({"action": "get_run_log", "session_id": session});
            let r = super::cloud::courier(&shrine, call).await;
            let mut sh = shrine.borrow_mut();
            sh.couriers -= 1;
            // Closed meanwhile, or recalled: this look is no longer anyone's.
            let mine = |e: &Entry| e.rec.id == id && cloud_here(e) && e.follow.looked == Some(now);
            let Some(i) = sh.entries.iter().position(mine) else { return };
            sh.entries[i].follow.busy = false;
            let failed = match r {
                Ok((200, body)) => return seen(&shrine, &mut sh, i, &body, now),
                Ok((404, body)) if gone_from(&body) => return gone(&mut sh, i),
                Ok((code, body)) => format!("HTTP {code}: {}", cloud::error_of(&body)),
                Err(e) => e,
            };
            let f = &mut sh.entries[i].follow;
            f.fails += 1;
            log(json!({"ev": "follow", "id": id, "failed": failed, "fails": f.fails}));
        });
    }
}

/// A 404 that says the session is not there: any other (a route moved, another account) is a
/// look that failed.
fn gone_from(body: &str) -> bool {
    cloud::json(body).is_ok_and(|v| v["error"]["type"] == "not_found_error")
}

/// A page of its log read: its phase, and the turns that ended since the last counted.
fn seen(shrine: &Shared, sh: &mut Shrine, i: usize, body: &str, now: i64) {
    let page = cloud::session(body);
    let e = &mut sh.entries[i];
    let Some(c) = e.rec.cloud.as_mut() else { return };
    let f = &mut e.follow;
    (f.fails, f.words, f.newest) = (0, page.words, page.newest);
    if f.asked.is_some_and(|(t, w)| page.words > w || now - t >= HOURLY) {
        f.asked = None;
    }
    if page.phase != f.phase {
        (f.phase, f.since) = (page.phase, now);
    }
    let new: Vec<&cloud::Turn> =
        page.turns.iter().filter(|t| c.ended.is_none_or(|x| t.at > x)).collect();
    let seek = c.branch.is_none() && !f.seeking && (!new.is_empty() || !f.sought);
    let Some(last) = new.last() else {
        if seek {
            find_branch(shrine, e);
        }
        return touch(sh);
    };
    c.ended = Some(last.at);
    e.rec.turns += new.len() as u64;
    let ended = if last.failed { Ended::Failed } else { Ended::Stop };
    if seek {
        find_branch(shrine, e);
    }
    let r = e.rec.clone();
    super::lead::keep_answer(&sh.store, &r, ended, Some(&last.answer));
    let _ = sh.store.save(&r);
    let url = &r.cloud.as_ref().map_or("", |c| c.url.as_str());
    let how = if last.failed { "stopped on an error" } else { "answered" };
    log(json!({"ev": "follow", "id": r.id, "turns": r.turns, "failed": last.failed}));
    tell(sh, &format!("☁ {} {how}: {url}", r.name));
    touch(sh);
}

/// Gone from claude.ai, archived or deleted there: departed here, and a recall follows it again.
fn gone(sh: &mut Shrine, i: usize) {
    let e = &mut sh.entries[i];
    depart(&mut e.rec, None);
    let r = e.rec.clone();
    let _ = sh.store.save(&r);
    super::lead::release(sh, &r.id);
    tell(sh, &format!("☁ {} is gone from claude.ai: archived or deleted there", r.name));
    touch(sh);
}

fn tell(sh: &mut Shrine, text: &str) {
    log(json!({"ev": "follow", "notice": text}));
    sh.notice(text);
}

/// The branch it pushed to, looked for in the background among tips not looked at yet, kept
/// once found.
fn find_branch(shrine: &Shared, e: &mut Entry) {
    let Some(c) = &e.rec.cloud else { return };
    (e.follow.seeking, e.follow.sought) = (true, true);
    let (shrine, id, dir, url) =
        (shrine.clone(), e.rec.id.clone(), e.rec.cwd.clone(), c.url.clone());
    let checked = e.follow.checked.clone();
    tokio::task::spawn_local(async move {
        let found = super::worktree::claude_branch(Path::new(&dir), &url, &checked).await;
        log(json!({"ev": "follow", "id": id, "branch": found.as_ref().map(|f| &f.0)}));
        let mut sh = shrine.borrow_mut();
        let Some(e) = sh.entries.iter_mut().find(|e| e.rec.id == id) else { return };
        e.follow.seeking = false;
        let Ok((b, tips)) = found else { return };
        e.follow.checked.extend(tips);
        let (Some(b), Some(c)) = (b, e.rec.cloud.as_mut()) else { return };
        c.branch = Some(b);
        let r = e.rec.clone();
        let _ = sh.store.save(&r);
        touch(&sh);
    });
}
