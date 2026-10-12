//! The cloud watch: the user's routines on claude.ai and their runs, read by couriers
//! (`crate::cloud`) on the ritual clock while `CLOUD_WATCH=on`. It reads status only: a run
//! reports through its own Slack posts and PR comments, and gensokyo says when a turn of it ends,
//! with its link. Nothing a run wrote is shown or typed anywhere.

use super::log::log;
use super::shrine::{Shared, Shrine, touch};
use super::store::write_atomic;
use crate::cloud::{self, Pace, Routine, Run};
use crate::paths;
use crate::proto::{CloudWatch, RoutineInfo};
use crate::ritual::{Dir, when};
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

/// A fire not seen this long after its time is no longer waited for: the hourly list finds it.
const OVERDUE_MOST: i64 = 3600;

/// Failed rounds in a row before the watch says it is broken: the network is often down for the
/// first tick after a wake.
const FAILS_TOLD: u32 = 3;

/// A courier still going after this is stopped. One call takes a few seconds.
const COURIER_LIMIT: Duration = Duration::from_secs(120);

/// Whether the user has the watch on: it spends their plan's usage, on an API Claude Code does
/// not document, so it is off until they say.
pub(super) fn on() -> bool {
    paths::config("CLOUD_WATCH").as_deref() == Some("on")
}

/// What the daemon keeps about the watch. `seen` is on disk too: a daemon started again tells
/// nothing twice.
#[derive(Default)]
pub(super) struct Watch {
    /// A round of couriers under way.
    busy: bool,
    pace: Pace,
    /// The routines as last listed.
    pub(super) routines: Vec<Routine>,
    seen: Option<Seen>,
    /// Why the last round failed.
    failed: Option<String>,
}

/// What has been seen of each routine and its runs.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Seen {
    /// Per routine, its last fire and its next run as last listed: a change is a fire to look
    /// for.
    routines: HashMap<String, (Option<i64>, Option<i64>)>,
    /// Each routine's newest runs, by id.
    runs: HashMap<String, Watched>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Watched {
    pub(super) routine: String,
    /// Its turn has ended: the worker is `idle`, or the session no longer `active`. Anything
    /// else (running, starting, a state not seen yet) is watched until it says so.
    pub(super) ended: bool,
    pub(super) created: i64,
    /// The last event of the turn whose end was told, or was found ended at first sight.
    told: Option<i64>,
    pub(super) url: String,
}

fn dir() -> PathBuf {
    paths::state_dir().join("cloud")
}

fn load() -> Seen {
    let text = std::fs::read(dir().join("seen.json")).unwrap_or_default();
    serde_json::from_slice(&text).unwrap_or_default()
}

/// A routine's journal: `<state>/cloud/<id>/journal.jsonl`.
fn journal(id: &str) -> Dir {
    Dir { path: dir().join(id) }
}

/// Every tick, after the rituals': a round of couriers when one is due and none is under way.
pub(super) fn tick(shrine: &Shared, now: i64, catch: bool) {
    let mut sh = shrine.borrow_mut();
    if !on() {
        if !sh.cloud.busy && sh.cloud.pace.listed.is_some() {
            sh.cloud = Watch::default();
            drop(sh);
            super::rituals::repush(shrine);
        }
        return;
    }
    if sh.cloud.busy || !sh.cloud.pace.due(&sh.cloud.routines, now, catch) {
        return;
    }
    sh.cloud.busy = true;
    if sh.cloud.seen.is_none() {
        sh.cloud.seen = Some(load());
    }
    drop(sh);
    let shrine = shrine.clone();
    tokio::task::spawn_local(async move {
        let r = round(&shrine, now).await;
        let mut sh = shrine.borrow_mut();
        sh.cloud.busy = false;
        sh.cloud.pace.listed = Some(now);
        sh.cloud.failed = r.as_ref().err().cloned();
        let p = &mut sh.cloud.pace;
        match r {
            Ok(()) => {
                if p.fails >= FAILS_TOLD {
                    tell(&mut sh, "cloud watch", "it works again");
                }
                sh.cloud.pace.fails = 0;
            }
            Err(e) => {
                p.fails += 1;
                log(json!({"ev": "cloud", "failed": e, "fails": p.fails}));
                if p.fails == FAILS_TOLD {
                    tell(
                        &mut sh,
                        "cloud watch",
                        &format!("{e} (gensokyo cloud watch off stops it)"),
                    );
                }
            }
        }
        touch(&sh);
        drop(sh);
        super::rituals::repush(&shrine);
    });
}

/// The watch, for the timetable: none while it is off.
pub(super) fn info(sh: &Shrine) -> Option<CloudWatch> {
    if !on() {
        return None;
    }
    let (tz, w) = (TimeZone::system(), &sh.cloud);
    let runs = w.seen.as_ref().map(|s| &s.runs);
    let newest = |id: &str| {
        let mine = runs.into_iter().flatten().filter(|(_, r)| r.routine == id);
        mine.max_by_key(|(_, r)| r.created).map(|(_, r)| r)
    };
    let routines = w
        .routines
        .iter()
        .map(|r| RoutineInfo {
            id: r.id.clone(),
            name: r.name.clone(),
            enabled: r.enabled,
            cron: r.cron.clone(),
            next_run_local: r.next_run.map(|t| when(t, &tz)),
            next_run: r.next_run,
            url: newest(&r.id).map_or_else(|| cloud::routine_link(&r.id), |n| n.url.clone()),
            ran: newest(&r.id).map(|n| n.created),
            going: newest(&r.id).is_some_and(|n| !n.ended),
        })
        .collect();
    Some(CloudWatch { listed: w.pace.listed, failing: w.failed.clone(), routines })
}

fn tell(sh: &mut Shrine, name: &str, text: &str) {
    log(json!({"ev": "cloud", "notice": format!("{name}: {text}")}));
    sh.notice(&format!("☁ {name}: {text}"));
}

/// The routines listed, then the runs of each that fired or has one going. What is seen is
/// kept after each routine, so a round that fails part way tells nothing twice; and one
/// routine's failure (a run's title can lead its courier astray) holds up none of the others.
async fn round(shrine: &Shared, now: i64) -> Result<(), String> {
    let (code, body) = ask(shrine, json!({"action": "list"})).await?;
    if code != 200 {
        return Err(format!("list: HTTP {code}: {}", cloud::error_of(&body)));
    }
    let (routines, more) = cloud::routines(&body)?;
    let mut seen = shrine.borrow().cloud.seen.clone().unwrap_or_default();
    // Only a whole list says what is gone: one on a later page would come back as new, and
    // what had ended by then would never be told.
    if more {
        log(json!({"ev": "cloud", "more": "the list has more pages than its first"}));
    } else {
        seen.routines.retain(|id, _| routines.iter().any(|r| &r.id == id));
        seen.runs.retain(|_, w| routines.iter().any(|r| r.id == w.routine));
    }
    shrine.borrow_mut().cloud.routines = routines.clone();
    let (mut failed, mut awaited) = (None, false);
    for r in &routines {
        let key = (r.last_fired, r.next_run);
        let first = !seen.routines.contains_key(&r.id);
        let fired = seen.routines.get(&r.id).is_some_and(|k| *k != key);
        let going = seen.runs.values().any(|w| w.routine == r.id && !w.ended);
        if first || fired || going {
            match runs_of(shrine, r).await {
                Ok(Some(runs)) => {
                    see(shrine, &mut seen, r, &runs, first, now);
                    // A fire is listed a moment before its run: until that is seen, the fire
                    // is looked for again, for as long as a late fire is.
                    let since = r.last_fired.map(|t| t - cloud::MARGIN);
                    let shown = runs.iter().any(|x| since.is_none_or(|t| x.created >= t));
                    let old = r.last_fired.is_none_or(|t| now - t >= OVERDUE_MOST);
                    if fired && !shown && !old {
                        awaited = true;
                        continue;
                    }
                }
                // Deleted since the list: the next one leaves it out.
                Ok(None) => {}
                Err(e) => {
                    log(json!({"ev": "cloud", "routine": r.id, "failed": e}));
                    failed.get_or_insert(format!("{}: {e}", r.name));
                    continue;
                }
            }
        }
        seen.routines.insert(r.id.clone(), key);
        keep(shrine, &seen);
    }
    keep(shrine, &seen);
    let going = seen.runs.values().any(|w| !w.ended);
    let late = |r: &Routine| r.next_run.is_some_and(|t| t <= now && now - t < OVERDUE_MOST);
    let overdue = routines.iter().any(|r| r.enabled && late(r));
    let p = &mut shrine.borrow_mut().cloud.pace;
    p.pending = (going || overdue || awaited).then(|| p.pending.unwrap_or(now));
    failed.map_or(Ok(()), Err)
}

/// A routine's newest runs; none when it has been deleted.
async fn runs_of(shrine: &Shared, r: &Routine) -> Result<Option<Vec<Run>>, String> {
    let (code, body) = ask(shrine, json!({"action": "list_runs", "trigger_id": r.id})).await?;
    match code {
        200 => cloud::runs(&body).map(Some),
        404 => Ok(None),
        _ => Err(format!("list_runs: HTTP {code}: {}", cloud::error_of(&body))),
    }
}

fn keep(shrine: &Shared, seen: &Seen) {
    shrine.borrow_mut().cloud.seen = Some(seen.clone());
    let text = serde_json::to_vec(seen).expect("seen serializes");
    let _ = std::fs::create_dir_all(dir());
    if let Err(e) = write_atomic(&dir().join("seen.json"), &text) {
        log(json!({"ev": "cloud", "seen": e.to_string()}));
    }
}

/// A routine's newest runs: each turn that has ended since it was last seen is told once. At a
/// routine's first sight what has ended is only recorded.
fn see(shrine: &Shared, seen: &mut Seen, r: &Routine, runs: &[Run], first: bool, now: i64) {
    let d = journal(&r.id);
    // Gone from its newest page, or deleted.
    seen.runs.retain(|id, w| w.routine != r.id || runs.iter().any(|x| &x.id == id));
    for run in runs {
        let w = seen.runs.entry(run.id.clone()).or_insert_with(|| {
            if !first {
                d.note(now, "fired", &format!("fired: {}", run.url));
            }
            Watched { routine: r.id.clone(), created: run.created, ..Default::default() }
        });
        // `idle` is all a turn's end has been seen to say; a session that is no longer
        // `active` has ended too, however.
        let ended = run.worker == "idle" || run.status != "active";
        (w.ended, w.url) = (ended, run.url.clone());
        if !ended || w.told == Some(run.last_event) {
            continue;
        }
        w.told = Some(run.last_event);
        if first {
            continue;
        }
        let how = match run.status.as_str() {
            "active" => String::new(),
            s => format!(" ({s})"),
        };
        d.note(now, "finished", &format!("finished{how}: {}", run.url));
        tell(&mut shrine.borrow_mut(), &r.name, &format!("finished{how} {}", run.url));
    }
}

/// `cloud_log`: a page of a run's log. A routine's newest run is the one the watch last saw:
/// nothing is listed to find it, so with the watch off, only a run's id or link will do.
pub(super) async fn run_log(shrine: &Shared, who: &str) -> Result<String, String> {
    let id = match cloud::run_id(who) {
        Some(id) => id,
        None => newest(&shrine.borrow(), who)?,
    };
    let (code, body) = ask(shrine, json!({"action": "get_run_log", "session_id": id})).await?;
    match code {
        200 => Ok(cloud::run_log(&body, &id)),
        404 => Err(format!("{id} is gone from claude.ai")),
        _ => Err(format!("get_run_log: HTTP {code}: {}", cloud::error_of(&body))),
    }
}

/// The newest run the watch has seen of a routine, by name or id.
fn newest(sh: &Shrine, who: &str) -> Result<String, String> {
    if !on() {
        return Err(format!(
            "the cloud watch is off, so no run of {who} is known: give a run's link, or turn it \
             on (gensokyo cloud watch on)"
        ));
    }
    let r = sh.cloud.routines.iter().find(|r| r.id == who || r.name == who);
    let r =
        r.ok_or_else(|| format!("no routine {who} on claude.ai (gensokyo cloud lists them)"))?;
    let runs = sh.cloud.seen.as_ref().map(|s| &s.runs).into_iter().flatten();
    let mine = runs.filter(|(_, w)| w.routine == r.id).max_by_key(|(_, w)| w.created);
    mine.map(|(id, _)| id.clone()).ok_or_else(|| format!("{} has no run yet", r.name))
}

/// One courier, for one call: the API's reply to it, or why there is none.
async fn ask(shrine: &Shared, call: Value) -> Result<(u16, String), String> {
    let (claude, env, exe) = {
        let sh = shrine.borrow();
        let (claude, env) = sh.claude("").ok_or("claude not found on PATH")?;
        (claude, env, sh.exe.to_string_lossy().into_owned())
    };
    // Empty, and outside any repository: nothing there for Claude Code to load.
    let cwd = dir().join("courier");
    std::fs::create_dir_all(&cwd).map_err(|e| format!("{}: {e}", cwd.display()))?;
    let mut cmd = tokio::process::Command::new(&claude);
    cmd.args(cloud::args(&call, &exe))
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let child =
        super::pty::locked(|| cmd.spawn()).map_err(|e| format!("could not start claude: {e}"))?;
    let pid = child.id().map_or(0, |p| p as i32);
    let out = tokio::time::timeout(COURIER_LIMIT, child.wait_with_output()).await;
    // What it started and left behind goes with it, as a headless run's does.
    if pid > 0 {
        // SAFETY: plain killpg on the courier's own group (`process_group(0)`).
        unsafe { libc::killpg(pid, libc::SIGKILL) };
    }
    let out = match out {
        Ok(o) => o.map_err(|e| format!("the courier: {e}"))?,
        Err(_) => return Err(format!("a courier took over {}s", COURIER_LIMIT.as_secs())),
    };
    let stream = String::from_utf8_lossy(&out.stdout);
    let r = cloud::reply(&stream, &call);
    log(json!({"ev": "cloud", "call": call, "exit": out.status.code(), "ok": r.is_ok()}));
    r.map_err(|e| match stream.trim().is_empty() {
        true => format!(
            "claude said: {}",
            crate::tele::clean(&String::from_utf8_lossy(&out.stderr), 200)
        ),
        false => e,
    })
}
