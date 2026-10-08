//! Headless ritual runs: `claude -p` with no pane, its files in the ritual's `runs/`, a time
//! limit, and the journal line and notice when it ends. A run outlives the daemon that started
//! it, and the next daemon sees it out.

use super::log::log;
use super::rituals::{notice, now, repush};
use super::shrine::Shared;
use crate::ritual::{self, Dir, Ritual, when};
use crate::tele;
use jiff::tz::TimeZone;
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Headless run logs kept per ritual.
const RUNS_KEPT: usize = 50;

/// The most of a run's stderr kept once it ends, and of its answer copied into its log, in bytes.
const RUN_MOST: u64 = 1 << 20;

/// `RUN_MOST`, or `$GENSOKYO_RUN_BYTES` for tests, read once.
fn run_most() -> u64 {
    static MOST: OnceLock<u64> = OnceLock::new();
    *MOST.get_or_init(|| {
        std::env::var("GENSOKYO_RUN_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(RUN_MOST)
    })
}

/// A headless run still going after this is stopped, its whole process group: a `claude -p`
/// that hangs would otherwise hold its ritual up until the daemon restarts.
const HEADLESS_LIMIT: Duration = Duration::from_secs(3600);

/// `HEADLESS_LIMIT`, or `$GENSOKYO_HEADLESS_MS` for tests, read once.
fn headless_limit() -> Duration {
    static LIMIT: OnceLock<Duration> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        let ms = std::env::var("GENSOKYO_HEADLESS_MS").ok().and_then(|v| v.parse().ok());
        ms.map_or(HEADLESS_LIMIT, Duration::from_millis)
    })
}

/// `claude -p` with no pane. Its answer goes to a file rather than a pipe, so a run the daemon
/// stops under still leaves it (the child is not killed: cutting a turn off halfway would leave
/// the ritual's notes half written for nothing), and its `.pid` file lets the next daemon see it
/// out. One still going after `HEADLESS_LIMIT` is stopped, by whichever daemon is there.
pub(super) fn start(shrine: &Shared, r: &Ritual, d: &Dir, label: &str) -> Result<(), String> {
    let (claude, env) = shrine.borrow().claude(&r.slug).ok_or("claude not found on PATH")?;
    // Found before any of the run's files is made, so a role gone leaves none behind.
    let mut args = ritual::args(r, &d.path);
    if let Some(role) = &r.role {
        let role = crate::role::find(role, shrine.borrow().share.as_deref())?;
        // Before the variadic `--allowedTools`, which must end the list.
        let at = args.iter().position(|a| a == "--allowedTools").unwrap_or(args.len());
        args.splice(at..at, ["--append-system-prompt-file".into(), role.to_string_lossy().into()]);
    }
    let cwd = r.cwd.clone().unwrap_or_default();
    let tz = TimeZone::system();
    let started = now();
    let stamp = jiff::Timestamp::from_second(started)
        .map(|t| t.to_zoned(tz.clone()).strftime("%Y%m%d-%H%M%S").to_string())
        .unwrap_or_else(|_| started.to_string());
    let stem = d.runs().join(format!("{stamp}.{:06x}", super::store::random(1 << 24)));
    std::fs::create_dir_all(d.runs()).map_err(|e| e.to_string())?;
    let file = |ext: &str| std::fs::File::create(part(&stem, ext)).map_err(|e| e.to_string());
    let (out, err, mut logf) = (file("json")?, file("err")?, file("log")?);
    let prompt = ritual::prompt_text(r, &d.memory());
    let _ = write!(
        logf,
        "ritual   {}\nstarted  {}  ({label})\nin       {cwd}\nflags    {}\nnotes    {}\n\n\
         No pane and nobody to answer a prompt: this run finishes on its own, and goes on\n\
         doing so if the daemon stops under it.\n\n",
        r.slug,
        when(started, &tz),
        args.join(" "),
        d.memory().display(),
    );
    let mut cmd = tokio::process::Command::new(&claude);
    cmd.args(["-p", "--output-format", "json", "--disallowed-tools"])
        .args(["CronCreate", "CronList", "CronDelete"])
        .args(&args)
        .arg("--")
        .arg(&prompt)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        // Its own group: whatever stops the daemon's group does not cut the run off.
        .process_group(0);
    let child =
        super::pty::locked(|| cmd.spawn()).map_err(|e| format!("could not start claude: {e}"))?;
    let pid = child.id().map_or(0, |p| p as i32);
    let run = Run {
        stem,
        slug: r.slug.clone(),
        pid,
        ident: super::pty::start_id(pid).unwrap_or(0),
        started: super::store::now(),
    };
    let line = format!("{} {} {}\n", run.pid, run.ident, run.started);
    if let Err(e) = std::fs::write(part(&run.stem, "pid"), line) {
        log(json!({"ev": "ritual", "slug": r.slug, "pid_file": e.to_string()}));
    }
    see_out(shrine, run, Some(child));
    Ok(())
}

/// A headless run in flight, as its `.pid` file keeps it: the pid, what the pid was when it
/// started (so a pid used again by something else is never taken for it), and when, in wall
/// seconds.
struct Run {
    stem: PathBuf,
    slug: String,
    pid: i32,
    ident: u64,
    started: i64,
}

impl Run {
    /// Still the process that was started.
    fn here(&self) -> bool {
        !super::pty::gone(self.pid) && super::pty::start_id(self.pid) == Some(self.ident)
    }
}

/// How a run ended, as far as this daemon saw.
#[derive(Clone, Copy)]
enum Ended {
    Exit(i32),
    Signal,
    /// It was a daemon before this one's child: its exit status went with that daemon.
    Unseen,
}

/// The headless runs an earlier daemon started, each seen out as if this one had: timed from
/// its start, and journaled when it ends. One that ended while no daemon was there is journaled
/// now. Before the clock's first tick, so its ritual counts as running.
pub(super) fn adopt(shrine: &Shared) {
    let rituals = crate::paths::state_dir().join("rituals");
    let pid_files = std::fs::read_dir(&rituals)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|d| std::fs::read_dir(d.path().join("runs")).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "pid"));
    for p in pid_files.collect::<Vec<_>>() {
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        let mut f = text.split_whitespace().map(|w| w.parse::<i64>().ok());
        let (Some(Some(pid)), Some(Some(ident)), Some(Some(started))) =
            (f.next(), f.next(), f.next())
        else {
            let _ = std::fs::remove_file(&p);
            continue;
        };
        // `<rituals>/<slug>/runs/<stem>.pid`
        let slug = p.parent().and_then(Path::parent).and_then(Path::file_name);
        let slug = slug.map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let run =
            Run { stem: p.with_extension(""), slug, pid: pid as i32, ident: ident as u64, started };
        log(json!({"ev": "ritual", "slug": run.slug, "adopted": run.pid, "here": run.here()}));
        see_out(shrine, run, None);
    }
}

/// Counted as running until it ends or is stopped at its limit; then its journal line, its
/// notice, and the log's last lines.
fn see_out(shrine: &Shared, run: Run, child: Option<tokio::process::Child>) {
    *shrine.borrow_mut().rites.headless.entry(run.slug.clone()).or_default() += 1;
    let shrine = shrine.clone();
    tokio::task::spawn_local(async move {
        let limit = headless_limit();
        let age =
            |run: &Run| Duration::from_secs((super::store::now() - run.started).max(0) as u64);
        let left = limit.saturating_sub(age(&run));
        let ended = match child {
            Some(mut c) => match tokio::time::timeout(left, c.wait()).await {
                Ok(st) => {
                    // What it started and left behind (a tool, an MCP server) goes with it. The
                    // group outlives its leader only while such a member does, so the id is
                    // still the run's.
                    // SAFETY: plain killpg on the run's own group, just after its leader ended.
                    unsafe { libc::killpg(run.pid, libc::SIGTERM) };
                    Some(match st.ok().and_then(|s| s.code()) {
                        Some(code) => Ended::Exit(code),
                        None => Ended::Signal,
                    })
                }
                Err(_) => {
                    stop_group(run.pid, Some(&mut c)).await;
                    None
                }
            },
            None => {
                let gone = async {
                    while run.here() {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                };
                match tokio::time::timeout(left, gone).await {
                    Ok(()) => Some(Ended::Unseen),
                    Err(_) => {
                        stop_group(run.pid, None).await;
                        None
                    }
                }
            }
        };
        // One that ended while no daemon was there took until its last write, not until now.
        let took = match ended {
            Some(Ended::Unseen) => last_write(&run.stem)
                .map_or(age(&run).as_secs(), |t| (t - run.started).max(0) as u64),
            _ => age(&run).as_secs(),
        };
        // After `took`: the cut is a write.
        super::log::cut(&part(&run.stem, "err"), run_most(), run_most());
        let (journal, told, tail) = match ended {
            Some(e) => report(&run.stem, e, &tele::age(took)),
            None => {
                let limit = tele::age(limit.as_secs());
                let journal =
                    format!("failed (headless, {limit}): still running, so it was stopped");
                let told =
                    format!("the headless run was still going after {limit}, and was stopped");
                (journal, told, format!("--- stopped: still running after {limit}\n"))
            }
        };
        let logf = std::fs::OpenOptions::new().append(true).open(part(&run.stem, "log"));
        if let Ok(mut f) = logf {
            let _ = f.write_all(tail.as_bytes());
        }
        let _ = std::fs::remove_file(part(&run.stem, "pid"));
        let d = Dir::of(&run.slug);
        let ok = journal.starts_with("done");
        d.note(now(), if ok { "done" } else { "failed" }, &journal);
        d.trim_runs(RUNS_KEPT);
        let mut sh = shrine.borrow_mut();
        if let Some(n) = sh.rites.headless.get_mut(&run.slug) {
            *n = n.saturating_sub(1);
        }
        notice(&mut sh, &run.slug, &told);
        drop(sh);
        repush(&shrine);
    });
}

/// TERM to the run's process group, and KILL to what is left of it a few seconds later.
async fn stop_group(pid: i32, child: Option<&mut tokio::process::Child>) {
    if pid <= 0 {
        return;
    }
    // SAFETY: plain libc calls; the group is the run's own (`process_group(0)`), and one that
    // is not our child was checked to be the run by its start time just now.
    unsafe { libc::killpg(pid, libc::SIGTERM) };
    let settled = Duration::from_secs(5);
    match child {
        Some(c) => {
            let _ = tokio::time::timeout(settled, c.wait()).await;
            // Whether or not the leader went: a tool or MCP server under it may ignore TERM.
            unsafe { libc::killpg(pid, libc::SIGKILL) };
            let _ = c.wait().await;
        }
        None => {
            let t = Instant::now();
            while !super::pty::gone(pid) && t.elapsed() < settled {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            unsafe { libc::killpg(pid, libc::SIGKILL) };
        }
    }
}

/// When a run last wrote its answer or its errors, epoch seconds.
fn last_write(stem: &Path) -> Option<i64> {
    let at = |ext| std::fs::metadata(part(stem, ext)).and_then(|m| m.modified()).ok();
    let t = ["json", "err"].into_iter().filter_map(at).max()?;
    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

/// `<stem>.<ext>`: the stem ends in a random part, which `with_extension` would replace.
fn part(stem: &Path, ext: &str) -> PathBuf {
    PathBuf::from(format!("{}.{ext}", stem.display()))
}

/// A run's answer as its log copies it: past `run_most()`, the rest is only in its `.json`.
fn shown(s: &str, stem: &Path) -> String {
    match crate::hooks::cap(s, run_most() as usize) {
        c if c.len() < s.len() => {
            let json = part(stem, "json");
            format!("{c}\n--- cut at {} KB: all of it is in {}\n", run_most() >> 10, json.display())
        }
        c => format!("{c}\n"),
    }
}

/// From a finished headless run's files: the journal line, the notice, and what the log ends
/// with. A run that fails can still print its object (`is_error`, the message as `result`).
fn report(stem: &Path, ended: Ended, took: &str) -> (String, String, String) {
    let raw = std::fs::read(part(stem, "json")).unwrap_or_default();
    let v: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let err = std::fs::read_to_string(part(stem, "err")).unwrap_or_default();
    let result = v["result"].as_str().unwrap_or("").to_string();
    let first = tele::clean(&result, 120);
    let failed = match ended {
        Ended::Exit(0) => v["is_error"] == true,
        // Its result is all there is to go by.
        Ended::Unseen => v.is_null() || v["is_error"] == true,
        _ => true,
    };
    if failed {
        let code = match ended {
            Ended::Exit(c) => format!("exited {c}"),
            Ended::Signal => "exited with a signal".into(),
            Ended::Unseen => "ended while no daemon was watching".into(),
        };
        let said: Vec<&str> = err.lines().take(20).collect();
        let mut tail = format!("--- claude {code}, and said:\n");
        if !result.is_empty() {
            tail += &shown(&result, stem);
        }
        tail += &format!("{}\n---\nfailed after {took}\n", said.join("\n"));
        let why = if first.is_empty() { tele::clean(&err, 120) } else { first };
        let journal = format!("failed (headless, {took}): claude {code}: {why}");
        let told = "the headless run failed; gensokyo ritual log says what it said".to_string();
        return (journal, told, tail);
    }
    let cost = v["total_cost_usd"].as_f64().map(tele::cost);
    let mut denied: Vec<&str> = v["permission_denials"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| d["tool_name"].as_str().unwrap_or("a tool"))
        .collect();
    denied.sort();
    denied.dedup();
    let denied = denied.join(", ");
    let mut tail = if v.is_null() {
        format!(
            "gensokyo could not read the result as JSON; it is here as claude printed it.\n\n{}",
            shown(&String::from_utf8_lossy(&raw), stem)
        )
    } else {
        shown(&result, stem)
    };
    tail += &format!("\n---\ndone in {took}");
    if let Some(c) = &cost {
        tail += &format!(", {c}");
    }
    if let Some(n) = v["num_turns"].as_u64() {
        tail += &format!(", {n} turns");
    }
    tail.push('\n');
    if !denied.is_empty() {
        tail += &format!(
            "refused  {denied} - a run with nobody to ask needs it in the ritual's allowed_tools\n"
        );
    }
    if let Some(s) = v["session_id"].as_str() {
        tail += &format!("to read the whole transcript: claude --resume {s}\n");
    }
    let cost = cost.map(|c| format!(", {c}")).unwrap_or_default();
    let mut journal = format!("done (headless, {took}{cost})");
    let mut told = "done".to_string();
    if !first.is_empty() {
        journal += &format!(": {first}");
        told += &format!(": {}", tele::clean(&first, 90));
    }
    if !denied.is_empty() {
        told += &format!(" (it needed {denied} and had nobody to ask)");
    }
    (journal, told, tail)
}
