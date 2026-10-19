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

/// Starting or working, a session is looked at every `BUSY` seconds (two ticks) for its first
/// `BUSY_FOR`, then every `BUSY_LATER`; with nothing new in its log for `QUIET` (a long command),
/// every `IDLE` until a day after its newest event, then hourly. Idle, it is not looked at: each
/// look costs a courier, and only words said to it start another turn, from here (looked at
/// then) or on the web (looked at once the user has it on screen). A failing look backs off as
/// the watch does, until one gets through.
const BUSY: i64 = 40;
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
    /// The newest event of any kind its log has shown, epoch µs.
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
    fn every(&self, now: i64) -> Option<i64> {
        if self.asked.is_none() && self.phase == Phase::Idle && self.fails == 0 {
            return None;
        }
        let quiet = self.newest.map(|t| now - t / 1_000_000).filter(|q| *q >= QUIET);
        let pace = match quiet.filter(|_| self.asked.is_none()) {
            None if now - self.since < BUSY_FOR => BUSY,
            None => BUSY_LATER,
            Some(q) if q < DAY => IDLE,
            Some(_) => HOURLY,
        };
        Some(match self.fails {
            0 => pace,
            n => pace.max((BUSY_LATER << n.min(6)).min(HOURLY)),
        })
    }

    /// The user said something to it, its log's newest words `words`: looked at again soon, and
    /// often.
    pub(super) fn asked(&mut self, now: i64, words: Option<i64>) {
        (self.asked, self.since, self.again) = (Some((now, words)), now, true);
    }
}

/// The user has session `id` on screen: looked at on the next tick, unless it was within
/// `BUSY_LATER`.
pub(super) fn shown(sh: &mut Shrine, id: &str, now: i64) {
    let Some(e) = sh.entries.iter_mut().find(|e| e.rec.id == id && cloud_here(e)) else { return };
    let f = &mut e.follow;
    f.again |= !f.busy && f.looked.is_some_and(|t| now - t >= BUSY_LATER);
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
            let due = |t| f.every(now).is_some_and(|p| catch || now - t >= p);
            cloud_here(e) && !f.busy && (f.again || f.looked.is_none_or(due))
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
            // A 404 that says the session is not there: any other (a route moved, another
            // account) is a look that failed.
            let not_there =
                |b: &str| cloud::json(b).is_ok_and(|v| v["error"]["type"] == "not_found_error");
            let failed = match r {
                Ok((200, body)) => return seen(&shrine, &mut sh, i, &body, now),
                Ok((404, body)) if not_there(&body) => return gone(&mut sh, i),
                Ok((code, body)) => format!("HTTP {code}: {}", cloud::error_of(&body)),
                Err(e) => e,
            };
            let f = &mut sh.entries[i].follow;
            f.fails += 1;
            log(json!({"ev": "follow", "id": id, "failed": failed, "fails": f.fails}));
        });
    }
}

/// A page of its log read: its phase, and the turns that ended since the last counted.
fn seen(shrine: &Shared, sh: &mut Shrine, i: usize, body: &str, now: i64) {
    let page = cloud::session(body);
    let e = &mut sh.entries[i];
    let Some(c) = e.rec.cloud.as_mut() else { return };
    let f = &mut e.follow;
    (f.fails, f.newest) = (0, page.newest);
    if f.asked.is_some_and(|(t, w)| page.words > w || now - t >= HOURLY) {
        f.asked = None;
    }
    if page.phase != f.phase {
        (f.phase, f.since) = (page.phase, now);
    }
    // Kept: words said after a restart wait for newer ones than these. Its newest only, as a
    // long turn pushes them off the page.
    let said = page.words > c.words;
    c.words = c.words.max(page.words);
    let new: Vec<&cloud::Turn> =
        page.turns.iter().filter(|t| c.ended.is_none_or(|x| t.at > x)).collect();
    if c.branch.is_none() && !f.seeking && (!new.is_empty() || !f.sought) {
        find_branch(shrine, e);
    }
    let Some(last) = new.last() else {
        if said {
            let r = e.rec.clone();
            let _ = sh.store.save(&r);
        }
        return touch(sh);
    };
    let Some(c) = e.rec.cloud.as_mut() else { return };
    c.ended = Some(last.at);
    e.rec.turns += new.len() as u64;
    let ended = if last.failed { Ended::Failed } else { Ended::Stop };
    let (r, failed) = (e.rec.clone(), last.failed);
    super::lead::keep_answer(&sh.store, &r, ended, Some(&last.answer));
    let _ = sh.store.save(&r);
    log(json!({"ev": "follow", "id": r.id, "turns": r.turns, "failed": failed}));
    // A helper's, held while its lead will collect it.
    if !super::lead::hush(sh, i) {
        answered(sh, i, failed);
    }
    touch(sh);
}

/// The user told its turn has ended, with its link.
pub(super) fn answered(sh: &mut Shrine, i: usize, failed: bool) {
    let r = &sh.entries[i].rec;
    let url = r.cloud.as_ref().map_or("", |c| c.url.as_str());
    let how = if failed { "stopped on an error" } else { "answered" };
    let text = format!("☁ {} {how}: {url}", r.name);
    tell(sh, &text);
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
