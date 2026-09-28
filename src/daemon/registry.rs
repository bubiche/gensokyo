//! Claude Code's session registry, `claude agents --json`: an array of `{pid, cwd, kind,
//! sessionId, name, status, startedAt}` (2.1.260), status `idle`, `busy`, `waiting`, or null for
//! a headless run. The format is undocumented, and this is the one place that reads it. Asked
//! every few seconds while anyone is here and may change without a hook, with the spool
//! replayed on the same beat.

use super::aware::Registry;
use super::ingest::replay;
use super::log::log;
use super::notify::after;
use super::pty;
use super::shrine::{Shared, taken, valid_name};
use crate::hooks;
use crate::proto::State;
use serde::Deserialize;
use serde_json::json;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

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

/// How long the registry goes unasked while everyone here rests and nobody types or changes
/// anything: nothing but the user can move a resting session, and the user's typing and every
/// hook bring the next ask back to `POLL`.
const POLL_RESTING: Duration = Duration::from_secs(30);

/// Every `POLL`: the spool, then the registry while anyone is here, less often while all rest.
pub(super) async fn poll(shrine: Shared) {
    let (mut asked, mut rev) = (None::<Instant>, None);
    loop {
        tokio::time::sleep(POLL).await;
        replay(&shrine, hooks::SPOOL_SETTLE);
        let (claude, env) = {
            let mut sh = shrine.borrow_mut();
            let live = || sh.entries.iter().filter(|e| e.handle.is_some());
            if live().next().is_none() {
                continue;
            }
            let resting =
                live().all(|e| e.aware.state() == State::Resting && e.aware.blocked().is_none());
            let still = !sh.typed && rev == Some(*sh.changed.borrow());
            if resting && still && asked.is_some_and(|t| t.elapsed() < POLL_RESTING) {
                continue;
            }
            sh.typed = false;
            let Some(found) = sh.claude("") else { continue };
            found
        };
        // When the snapshot began: a hook that lands during the call is newer than it.
        let at = hooks::now_ms();
        asked = Some(Instant::now());
        if let Some(list) = fetch(&claude, &env).await {
            seen(&shrine, &list, at);
        }
        rev = Some(*shrine.borrow().changed.borrow());
    }
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
        if renamed.is_some() || sh.entries[i].aware.state() != before {
            after(sh, i, before);
        }
    }
}
