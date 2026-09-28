//! Headless ritual runs: `claude -p` with no pane, its files in the ritual's `runs/`, a time
//! limit, and the journal line and notice when it ends.

use super::rituals::{notice, now, repush};
use super::shrine::Shared;
use crate::ritual::{self, Dir, Ritual, when};
use crate::tele;
use jiff::tz::TimeZone;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Headless run logs kept per ritual.
const RUNS_KEPT: usize = 50;

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
/// the ritual's notes half written for nothing). One still going after `HEADLESS_LIMIT` is.
pub(super) fn start(shrine: &Shared, r: &Ritual, d: &Dir, label: &str) -> Result<(), String> {
    let (claude, env) = shrine.borrow().claude(&r.slug).ok_or("claude not found on PATH")?;
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
    let args = ritual::args(r, &d.path);
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
    let mut child =
        super::pty::locked(|| cmd.spawn()).map_err(|e| format!("could not start claude: {e}"))?;
    *shrine.borrow_mut().rites.headless.entry(r.slug.clone()).or_default() += 1;
    let (shrine, slug) = (shrine.clone(), r.slug.clone());
    tokio::task::spawn_local(async move {
        let t = Instant::now();
        let limit = headless_limit();
        let (journal, told, tail) = match tokio::time::timeout(limit, child.wait()).await {
            Ok(status) => {
                let took = tele::age(t.elapsed().as_secs());
                report(&stem, status.ok().and_then(|s| s.code()), &took)
            }
            Err(_) => {
                stop_group(&mut child).await;
                let limit = tele::age(limit.as_secs());
                let journal =
                    format!("failed (headless, {limit}): still running, so it was stopped");
                let told =
                    format!("the headless run was still going after {limit}, and was stopped");
                (journal, told, format!("--- stopped: still running after {limit}\n"))
            }
        };
        let _ = logf.write_all(tail.as_bytes());
        let d = Dir::of(&slug);
        let ok = journal.starts_with("done");
        d.note(now(), if ok { "done" } else { "failed" }, &journal);
        d.trim_runs(RUNS_KEPT);
        let mut sh = shrine.borrow_mut();
        if let Some(n) = sh.rites.headless.get_mut(&slug) {
            *n = n.saturating_sub(1);
        }
        notice(&sh, &slug, &told);
        drop(sh);
        repush(&shrine);
    });
    Ok(())
}

/// TERM to the run's process group, and KILL to what is left of it a few seconds later.
async fn stop_group(child: &mut tokio::process::Child) {
    let Some(pid) = child.id() else { return };
    // SAFETY: plain libc calls; the group is the child's own (`process_group(0)`).
    unsafe { libc::killpg(pid as i32, libc::SIGTERM) };
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    // Whether or not the leader went: a tool or MCP server under it may ignore TERM.
    unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
    let _ = child.wait().await;
}

/// `<stem>.<ext>`: the stem ends in a random part, which `with_extension` would replace.
fn part(stem: &Path, ext: &str) -> PathBuf {
    PathBuf::from(format!("{}.{ext}", stem.display()))
}

/// From a finished headless run's files: the journal line, the notice, and what the log ends
/// with. A run that fails can still print its object (`is_error`, the message as `result`).
fn report(stem: &Path, code: Option<i32>, took: &str) -> (String, String, String) {
    let raw = std::fs::read(part(stem, "json")).unwrap_or_default();
    let v: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let err = std::fs::read_to_string(part(stem, "err")).unwrap_or_default();
    let result = v["result"].as_str().unwrap_or("").to_string();
    let first = tele::clean(&result, 120);
    let failed = code != Some(0) || v["is_error"] == true;
    if failed {
        let code = code.map_or("with a signal".into(), |c| format!("{c}"));
        let said: Vec<&str> = err.lines().take(20).collect();
        let mut tail = format!("--- claude exited {code}, and said:\n");
        if !result.is_empty() {
            tail += &format!("{result}\n");
        }
        tail += &format!("{}\n---\nfailed after {took}\n", said.join("\n"));
        let why = if first.is_empty() { tele::clean(&err, 120) } else { first };
        let journal = format!("failed (headless, {took}): claude exited {code}: {why}");
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
            "gensokyo could not read the result as JSON; it is here as claude printed it.\n\n{}\n",
            String::from_utf8_lossy(&raw)
        )
    } else {
        format!("{result}\n")
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
