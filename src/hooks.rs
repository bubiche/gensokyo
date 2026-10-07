//! The two commands Claude Code runs inside every resident: `_hook` for each hook event, and
//! `_statusline` for the line at the bottom of its screen. Both always exit 0, never start a
//! daemon, and wait on one no longer than twice `DELIVER` (the write, then the answer):
//! `UserPromptSubmit` holds up the prompt until its hook ends.

use crate::paths;
use crate::proto::{self, Envelope, Hook, Request};
use crate::tele;
use serde_json::Value;
use std::io::{BufRead, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// How long a hook may spend handing its line to the daemon.
const DELIVER: Duration = Duration::from_millis(300);

/// The most `spool.later.jsonl` holds, in bytes.
const SPOOL_LATER_MAX: u64 = 256 * 1024;

/// The size, in bytes, past which `spool.jsonl` moves to `spool.0.jsonl`.
pub const SPOOL_MOST: u64 = 2 << 20;

/// How long a spool renamed aside waits before the poll reads it, in ms.
pub const SPOOL_SETTLE: i64 = 1000;

/// The longest answer kept, in bytes.
pub const ANSWER_MOST: usize = 64 * 1024;

fn stdin() -> Vec<u8> {
    let mut b = Vec::new();
    // Whole: a payload cut short is no JSON, and a Stop lost with it is a turn never counted.
    let _ = std::io::stdin().take(64 << 20).read_to_end(&mut b);
    b
}

/// `gensokyo _hook`: the payload on stdin, reduced, for the resident `$GENSOKYO_RESIDENT`. A
/// daemon that cannot be reached gets it later from the spool. Nothing goes to stdout, which
/// Claude Code would feed to the model.
pub fn hook_main() {
    let payload = stdin();
    let (Ok(j), Some(resident)) =
        (serde_json::from_slice::<Value>(&payload), std::env::var("GENSOKYO_RESIDENT").ok())
    else {
        return;
    };
    let req = Request::Hook { resident, hook: reduce(&j, now_ms()) };
    if !deliver(&req) {
        spool(&paths::state_dir(), &req);
    }
}

/// What the shrine keeps of a hook payload (2.1.260: `hook_event_name`, `session_id`, and per
/// event `notification_type` + `message`, `source`, `permission_mode` on UserPromptSubmit and
/// Stop, `tool_name` + `tool_input`, `last_assistant_message`; `error` on StopFailure, 2.1.290;
/// `cwd`, which follows a `cd`, 2.1.291). Never the prompt.
pub fn reduce(j: &Value, at: i64) -> Hook {
    let s = |p: &str| j.pointer(p).and_then(Value::as_str);
    let event = s("/hook_event_name").unwrap_or_default();
    let answer = ["Stop", "StopFailure"]
        .contains(&event)
        .then(|| s("/last_assistant_message"))
        .flatten()
        .map(|a| match cap(a, ANSWER_MOST) {
            c if c.len() < a.len() => format!("{c}\n[cut at {} KB]", ANSWER_MOST / 1024),
            c => c.to_string(),
        });
    let paths =
        ["/error", "/tool_input/questions/0/question", "/message", "/last_assistant_message"];
    let text =
        paths.iter().find_map(|p| s(p)).map(|t| tele::clean(t, 80)).filter(|t| !t.is_empty());
    Hook {
        event: event.into(),
        session: s("/session_id").map(String::from),
        at,
        kind: s("/notification_type").or(s("/source")).map(String::from),
        mode: s("/permission_mode").map(String::from),
        tool: s("/tool_name").map(String::from),
        text,
        answer,
        cwd: s("/cwd").filter(|c| tele::is_dir_path(c)).map(String::from),
    }
}

/// At most `n` bytes of `s`, cut at a character.
pub fn cap(s: &str, n: usize) -> &str {
    let mut end = s.len().min(n);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// hello, then the request, then the hello's answer, within `DELIVER`. A daemon that refused
/// the hello (one from before an update, speaking another protocol) has not taken the request,
/// which goes to the spool for the next daemon. No answer in time is a daemon that is there and
/// busy: it has the request, and spooling it too would replay it twice.
fn deliver(req: &Request) -> bool {
    let Ok(mut s) = UnixStream::connect(paths::socket_path()) else { return false };
    let hello = Request::Hello { proto: proto::PROTO, who: "hook".into(), resident: None };
    let mut b = Vec::new();
    for req in [hello, req.clone()] {
        let _ = serde_json::to_writer(&mut b, &Envelope { id: 0, req });
        b.push(b'\n');
    }
    if s.set_write_timeout(Some(DELIVER)).is_err() || s.write_all(&b).is_err() {
        return false;
    }
    let _ = s.set_read_timeout(Some(DELIVER));
    let mut line = Vec::new();
    match std::io::BufReader::new(&s).read_until(b'\n', &mut line) {
        // A welcome in another protocol is a daemon that takes hooks from any build, but may
        // not read this one: the spool has it too, for a daemon of this build.
        Ok(_) => serde_json::from_slice::<Value>(&line)
            .is_ok_and(|v| v["t"] == "welcome" && v["proto"] == proto::PROTO),
        Err(e) => matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut),
    }
}

/// One line appended to `spool.jsonl`, for the daemon to replay. Past `SPOOL_MOST` it moves to
/// `spool.0.jsonl`, over the one before: with no daemon to read it, the newest hooks are kept.
pub fn spool(root: &Path, req: &Request) {
    let Ok(mut line) = serde_json::to_vec(req) else { return };
    line.push(b'\n');
    let path = root.join("spool.jsonl");
    let f = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&path);
    // One write: appends from several hooks at once do not interleave.
    let _ = f.and_then(|mut f| f.write_all(&line));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > SPOOL_MOST) {
        let _ = std::fs::rename(&path, root.join("spool.0.jsonl"));
    }
}

/// Every spooled hook, oldest first, and the spool emptied. It is renamed aside, so a hook
/// spooled meanwhile starts a new one, and read once `settle` ms old: a hook that opened it
/// just before the rename has written by then. At start, with nobody yet able to hook in,
/// it is read at once. A crash before the delete replays those again, which the timestamps
/// make harmless.
pub fn take_spool(root: &Path, settle: i64) -> Vec<(String, Hook)> {
    let now = now_ms();
    let _ = std::fs::rename(root.join("spool.jsonl"), root.join(format!("spool.{now}.jsonl")));
    // Lines an older build could not read, tried again by each daemon as it starts.
    let later = root.join("spool.later.jsonl");
    if settle == 0 {
        let _ = std::fs::rename(&later, root.join(format!("spool.{}.jsonl", now - 1)));
    }
    let mut unread = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap_or_default().to_string_lossy();
            let at = n.strip_prefix("spool.").and_then(|n| n.strip_suffix(".jsonl"));
            at.and_then(|a| a.parse::<i64>().ok()).is_some_and(|a| a <= now - settle)
        })
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        for l in std::fs::read_to_string(&f).unwrap_or_default().lines() {
            match serde_json::from_str(l) {
                Ok(Request::Hook { resident, hook }) => out.push((resident, hook)),
                Ok(_) => {}
                Err(_) => unread.extend_from_slice(format!("{l}\n").as_bytes()),
            }
        }
        let _ = std::fs::remove_file(&f);
    }
    // Kept only while small: past that it is noise no build reads.
    let size = std::fs::metadata(&later).map_or(0, |m| m.len());
    if !unread.is_empty() && size + (unread.len() as u64) < SPOOL_LATER_MAX {
        let f = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&later);
        let _ = f.and_then(|mut f| f.write_all(&unread));
    }
    out.sort_by_key(|(_, h)| h.at);
    out
}

/// `gensokyo _statusline <id>`: the report goes to the daemon, then the line is printed from
/// the same JSON. With `STATUSLINE=user` in the config, the user's own statusLine command gets
/// the JSON instead, and its output goes out untouched.
pub fn statusline_main(id: &str) {
    let payload = stdin();
    let Ok(j) = serde_json::from_slice::<Value>(&payload) else { return };
    // The settings chain is the project's, wherever Claude has gone inside it.
    let cwd = ["/workspace/project_dir", "/workspace/current_dir", "/cwd"]
        .iter()
        .find_map(|p| j.pointer(p).and_then(Value::as_str))
        .map_or_else(|| Path::new(".").to_path_buf(), Into::into);
    let mut t = tele::from_statusline(&j);
    t.advisor =
        tele::setting(&cwd, "/advisorModel").and_then(|v| v.as_str().map(|a| tele::clean(a, 20)));
    deliver(&Request::Statusline { resident: id.into(), telemetry: t.clone() });
    let user = (paths::config("STATUSLINE").as_deref() == Some("user"))
        .then(|| tele::setting(&cwd, "/statusLine/command"))
        .flatten();
    if let Some(cmd) = user.as_ref().and_then(Value::as_str) {
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", cmd])
            .stdin(std::process::Stdio::piped())
            .spawn();
        if let Ok(mut c) = child {
            if let Some(mut i) = c.stdin.take() {
                let _ = i.write_all(&payload);
            }
            let _ = c.wait();
        }
        return;
    }
    println!("{}", tele::own_line(&t));
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
