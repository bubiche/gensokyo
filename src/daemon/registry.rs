//! Claude Code's session registry, `claude agents --json`: an array of `{pid, cwd, kind,
//! sessionId, name, status, startedAt}` (2.1.260), status `idle`, `busy`, `waiting`, or null for
//! a headless run. The format is undocumented, and this is the one place that reads it.

use super::aware::Registry;
use super::pty;
use serde::Deserialize;
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
