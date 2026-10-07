//! A daemon that went down without stopping (a crash, a SIGKILL) leaves its residents' records
//! live. The next one makes sure their claudes are gone, then brings each back into its own
//! conversation and says their turns were cut off. A crash again within `WINDOW` of that leaves
//! them departed, for the user to recall.

use super::launch::has_conversation;
use super::log::log;
use super::pty;
use super::shrine::{Shared, recall};
use super::store::{self, Record, Store};
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A crash this soon after the last recovery is left to the user: whoever was brought back may
/// be what brings the daemon down.
const WINDOW: Duration = Duration::from_secs(600);

/// From HUP to TERM for a claude a dead daemon left running: it had one HUP already, when its
/// PTY closed.
const GRACE: Duration = Duration::from_secs(1);

fn run(f: &str) -> PathBuf {
    crate::paths::state_dir().join("run").join(f)
}

/// Written as a stop begins: whatever it leaves live, cut off partway (launchd waits only so long
/// at logout), was not left by a crash.
pub(super) fn stopping() {
    let _ = store::write_atomic(&run("stopping"), b"");
}

/// What the daemon before this one left behind.
pub(super) struct Left {
    /// Its live records, in slot order.
    records: Vec<Record>,
    /// Those whose claude is still running: none of them is resumed, which would have two
    /// processes writing one session.
    pub(super) running: HashSet<String>,
    /// After a crash: whether it came too soon after the last recovery to recover again.
    crash: Option<bool>,
}

/// At start, before anyone is recalled: the claude of every live record ended, if it is still
/// the process the record names, and every record moved to `departed/`.
pub(super) async fn leftovers(store: &Store) -> Left {
    let clean = std::fs::remove_file(run("stopping")).is_ok();
    let mut all = Vec::new();
    for r in store.load() {
        match r {
            Ok(r) => all.push(r),
            Err(e) => log(json!({"ev": "record", "error": e})),
        }
    }
    let mut records: Vec<Record> = all.iter().filter(|r| r.departed.is_none()).cloned().collect();
    records.sort_by_key(|r| (r.slot.unwrap_or(u8::MAX), r.launched));
    let mut crash = None;
    // Neither a stop nor `restart` saw them out. The time goes down first, so a crash while
    // they are brought back counts as one soon after.
    if !records.is_empty() && !clean && !crate::paths::comeback_path().exists() {
        let window = super::rituals::env_ms("GENSOKYO_CRASH_WINDOW_MS", WINDOW).as_secs() as i64;
        let now = store::now();
        let last = std::fs::read_to_string(run("crash")).ok();
        let last = last.and_then(|s| s.lines().next()?.trim().parse::<i64>().ok());
        let again = last.is_some_and(|t| now - t < window);
        if !again {
            let _ = store::write_atomic(&run("crash"), format!("{now}\n").as_bytes());
        }
        log(json!({"ev": "crash", "left": records.len(), "again": again}));
        crash = Some(again);
    }
    // Only a process that started when the record says: the pid may be someone else's by now.
    let still = |(pid, at): (i32, u64)| pty::start_id(pid) == Some(at);
    let ends: Vec<_> = records
        .iter()
        .filter_map(|r| r.pid.filter(|p| still(*p)).map(|p| (r, p)))
        .map(|(r, p)| {
            log(json!({"ev": "leftover", "id": r.id, "name": r.name, "pid": p.0}));
            (r.id.clone(), p, tokio::task::spawn_local(pty::sweep(p.0, GRACE)))
        })
        .collect();
    let mut running = HashSet::new();
    for (id, p, sweep) in ends {
        let _ = sweep.await;
        // The KILL is not waited on, and launchd reaps it, not us.
        let t = Instant::now();
        while still(p) && t.elapsed() < Duration::from_secs(2) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if still(p) {
            log(json!({"ev": "leftover", "id": id, "pid": p.0, "error": "still running"}));
            running.insert(id);
        }
    }
    // Nobody from before this start is running now.
    let now = store::now();
    for mut r in all {
        r.departed.get_or_insert(now);
        let _ = store.retire(&r);
    }
    Left { records, running, crash }
}

/// After a crash: everyone it cut off back in their conversations, unless it came too soon
/// after the last recovery, and a notice kept for the clients to come.
pub(super) fn recover(shrine: &Shared, left: &Left) {
    let Some(again) = left.crash else { return };
    let text = if again {
        let all: Vec<String> = left.records.iter().map(|r| r.name.clone()).collect();
        format!(
            "the daemon crashed again soon after bringing its residents back, so it left them \
             departed: recall them by hand ({})",
            all.join(", ")
        )
    } else {
        let (mut back, mut not) = (Vec::new(), Vec::new());
        for r in &left.records {
            // The session a spooled hook may have moved it on to since.
            let gone = shrine.borrow().store.load_departed_id(&r.id);
            let session = gone.map_or(r.session.clone(), |g| g.session);
            let why = if left.running.contains(&r.id) {
                Err("its claude would not stop".to_string())
            } else if !has_conversation(&session) {
                Err("no conversation to resume".to_string())
            } else {
                recall(shrine, &r.id, None).map(|_| ())
            };
            log(json!({"ev": "crash", "id": r.id, "name": r.name, "error": why.as_ref().err()}));
            match why {
                Ok(()) => back.push(r.name.clone()),
                Err(e) => not.push(format!("{} ({e})", r.name)),
            }
        }
        let mut t = "the daemon crashed, cutting its residents' turns off".to_string();
        if !back.is_empty() {
            t += &format!("; back in their conversations: {}", back.join(", "));
        }
        if !not.is_empty() {
            t += &format!("; left departed: {}", not.join(", "));
        }
        t
    };
    log(json!({"ev": "crash", "notice": text}));
    // Kept as missed: no client can be watching yet.
    shrine.borrow_mut().notice(&text);
}
