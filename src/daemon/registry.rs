//! Claude Code's session registry, `claude agents --json`: an array of `{pid, cwd, kind,
//! sessionId, name, status, startedAt}` (2.1.260), status `idle`, `busy`, `waiting`, or null for
//! a headless run. The format is undocumented, and this is the one place that reads it. Asked
//! every few seconds while anyone is here, with the spool replayed on the same beat.

use super::aware::Registry;
use super::ingest::replay;
use super::launch;
use super::notify::after;
use super::pty;
use super::server::log;
use super::shrine::{Shared, taken, valid_name};
use crate::hooks;
use serde::Deserialize;
use serde_json::json;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

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

pub fn parse(json: &[u8]) -> Option<Vec<Session>> {
    serde_json::from_slice(json).ok()
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

/// Every `POLL`: the spool, then the registry while anyone is here.
pub(super) async fn poll(shrine: Shared) {
    loop {
        tokio::time::sleep(POLL).await;
        replay(&shrine);
        let (claude, env) = {
            let sh = shrine.borrow();
            if sh.entries.iter().all(|e| e.handle.is_none()) {
                continue;
            }
            let bin = sh.exe.parent().unwrap_or(Path::new("/")).to_path_buf();
            let path = std::env::var_os("PATH");
            let Some(claude) = launch::claude(path.as_deref()) else { continue };
            (claude, launch::env(std::env::vars_os(), "", &bin, &sh.socket))
        };
        // When the snapshot began: a hook that lands during the call is newer than it.
        let at = hooks::now_ms();
        if let Some(list) = fetch(&claude, &env).await {
            seen(&shrine, &list, at);
        }
    }
}

/// A registry snapshot: each resident's status, and the name it was renamed to inside.
fn seen(shrine: &Shared, list: &[Session], at: i64) {
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
