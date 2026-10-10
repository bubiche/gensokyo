//! Claude Code's session registry, `claude agents --json`: an array of `{pid, cwd, kind,
//! sessionId, name, status, startedAt}` (2.1.260), status `idle`, `busy`, `waiting`, or null for
//! a headless run. The format is undocumented, and this is the one place that reads it. Asked
//! every few seconds while anyone is here and may change without a hook, with the spool
//! replayed on the same beat. On that beat too, which version of `claude` is installed.

use super::aware::Registry;
use super::ingest::replay;
use super::log::log;
use super::notify::after;
use super::pty;
use super::shrine::{Shared, taken, touch};
use crate::hooks;
use crate::proto::{State, valid_name};
use serde::Deserialize;
use serde_json::json;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub session_id: String,
    #[serde(default)]
    pub pid: Option<i32>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

impl Session {
    pub fn status(&self) -> Option<Registry> {
        match self.status.as_deref()? {
            "idle" => Some(Registry::Idle),
            "busy" => Some(Registry::Busy),
            "waiting" => Some(Registry::Waiting),
            _ => None,
        }
    }
}

/// Every session on the machine is listed, the user's own too: one entry this cannot read
/// is skipped, not the snapshot.
pub fn parse(json: &[u8]) -> Option<Vec<Session>> {
    let all: Vec<serde_json::Value> = serde_json::from_slice(json).ok()?;
    Some(all.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

/// One call, at most 5 s; `None` when it fails.
pub async fn fetch(claude: &Path, env: &[(OsString, OsString)]) -> Option<Vec<Session>> {
    let mut cmd = tokio::process::Command::new(claude);
    cmd.args(["agents", "--json"]).env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    cmd.current_dir("/").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let child = pty::locked(|| cmd.kill_on_drop(true).spawn()).ok()?;
    let out =
        tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await.ok()?.ok()?;
    out.status.success().then(|| parse(&out.stdout)).flatten()
}

/// How often the spool is replayed and the registry asked (a call costs about 0.12 s).
const POLL: Duration = Duration::from_secs(3);

/// How long the registry goes unasked while everyone here rests, quietly: nobody types, nothing
/// changes and no resident writes a thing. A turn the user did not start (a peer's message, a
/// background task) may fire no hook, but it draws; any of these brings the next ask back to
/// `POLL`.
const POLL_RESTING: Duration = Duration::from_secs(30);

/// Every `POLL`: the spool, then the registry while anyone is here, less often while all rest.
pub(super) async fn poll(shrine: Shared) {
    let (mut asked, mut rev) = (None::<Instant>, None);
    loop {
        tokio::time::sleep(POLL).await;
        replay(&shrine, hooks::SPOOL_SETTLE);
        let (claude, env, ask) = {
            let mut sh = shrine.borrow_mut();
            let live = || sh.entries.iter().filter(|e| e.handle.is_some());
            if live().next().is_none() {
                continue;
            }
            let resting =
                live().all(|e| e.aware.state() == State::Resting && e.aware.dialog().is_none());
            let drew = live().any(|e| {
                let h = e.handle.as_ref().expect("live");
                asked.is_none_or(|t| h.last_output() > t)
            });
            let still = !sh.typed && !drew && rev == Some(*sh.changed.borrow());
            let ask = !(resting && still && asked.is_some_and(|t| t.elapsed() < POLL_RESTING));
            let Some((claude, env)) = sh.claude("") else { continue };
            if ask {
                sh.typed = false;
            }
            (claude, env, ask)
        };
        installed(&shrine, &claude, &env).await;
        if !ask {
            continue;
        }
        // When the snapshot began: a hook that lands during the call is newer than it.
        let at = hooks::now_ms();
        asked = Some(Instant::now());
        if let Some(list) = fetch(&claude, &env).await {
            seen(&shrine, &list, at);
        }
        rev = Some(*shrine.borrow().changed.borrow());
    }
}

/// The `claude` on PATH as it was last asked its version: the file it resolves to, that file's
/// time and size, what it said (none when it could not say), and when it was asked.
pub(crate) struct Installed {
    file: (PathBuf, SystemTime, u64),
    pub(super) version: Option<String>,
    at: Instant,
}

/// How long a `claude` that could not say its version goes unasked: the first run of a new
/// binary may be held up past the 5 s by the system checking it.
const VERSION_AGAIN: Duration = Duration::from_secs(60);

/// The installed version, asked again only when the file `claude` resolves to has changed (an
/// update moves the native installer's link to another version, or writes npm's file over), or
/// a while after it could not say.
async fn installed(shrine: &Shared, claude: &Path, env: &[(OsString, OsString)]) {
    let file = std::fs::canonicalize(claude).ok().and_then(|f| {
        let m = std::fs::metadata(&f).ok()?;
        Some((f, m.modified().ok()?, m.len()))
    });
    let Some(file) = file else { return };
    let known = |i: &Installed| i.version.is_some() || i.at.elapsed() < VERSION_AGAIN;
    if shrine.borrow().installed.as_ref().is_some_and(|i| i.file == file && known(i)) {
        return;
    }
    let version = version(claude, env).await;
    log(json!({"ev": "claude", "path": file.0, "version": version}));
    let mut sh = shrine.borrow_mut();
    sh.installed = Some(Installed { file, version, at: Instant::now() });
    touch(&sh);
}

/// `claude --version`'s first word, `2.1.294 (Claude Code)`; at most 5 s.
async fn version(claude: &Path, env: &[(OsString, OsString)]) -> Option<String> {
    let mut cmd = tokio::process::Command::new(claude);
    cmd.arg("--version").env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    cmd.current_dir("/").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let child = pty::locked(|| cmd.kill_on_drop(true).spawn()).ok()?;
    let out =
        tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let word = text.split_whitespace().next()?;
    (out.status.success() && word.starts_with(|c: char| c.is_ascii_digit()))
        .then(|| crate::tele::clean(word, 40))
}

/// A registry snapshot: each resident's status, and the name it was renamed to inside.
pub(super) fn seen(shrine: &Shared, list: &[Session], at: i64) {
    let mut sh = shrine.borrow_mut();
    let sh = &mut *sh;
    for i in 0..sh.entries.len() {
        let e = &sh.entries[i];
        let Some(pid) = e.handle.as_ref().map(|h| h.pid) else { continue };
        let s = list.iter().find(|s| s.session_id == e.rec.session);
        let s = s.or_else(|| list.iter().find(|s| s.pid == Some(pid)));
        let renamed = s
            .and_then(|s| s.name.clone())
            .filter(|n| !n.eq_ignore_ascii_case(&e.rec.name) && valid_name(n) && !taken(sh, n));
        let before = e.aware.state();
        let e = &mut sh.entries[i];
        e.aware.registry(s.and_then(Session::status), at);
        if let Some(n) = &renamed {
            log(json!({"ev": "renamed", "id": e.rec.id, "from": e.rec.name, "to": n}));
            e.rec.name = n.clone();
            let _ = sh.store.save(&sh.entries[i].rec);
        }
        super::ingest::tally(sh, i, None);
        if renamed.is_some() || sh.entries[i].aware.state() != before {
            after(sh, i, before);
        }
    }
}
