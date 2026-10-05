//! `gensokyo doctor`: which binary, which `claude`, and whether what gensokyo leans on is there.
//! It only reads: it never starts the daemon or loads anything.

use super::{Error, login, request};
use crate::paths;
use crate::proto::{Reply, Request};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn main() -> Result<(), String> {
    let line = |k: &str, v: String| println!("  {k:<11}{v}");
    let short = |p: &Path| paths::short(&p.to_string_lossy());
    println!("gensokyo {}", env!("CARGO_PKG_VERSION"));
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).map_err(|e| e.to_string())?;
    let how = match paths::release_root(&exe) {
        Some(root) => format!("the release in {}", short(&root)),
        None => "a build from source".into(),
    };
    line("binary", format!("{} ({how})", short(&exe)));
    let path = std::env::var_os("PATH");
    let on_path = std::env::split_paths(path.as_deref().unwrap_or_default())
        .map(|d| d.join("gensokyo"))
        .find(|p| p.is_file());
    line(
        "on PATH",
        match on_path {
            None => "no: the install links ~/.local/bin/gensokyo; put that dir on PATH".into(),
            Some(p) if std::fs::canonicalize(&p).is_ok_and(|c| c == exe) => {
                format!("{} -> this binary", short(&p))
            }
            Some(p) => format!("{} is another copy; residents get this one first", short(&p)),
        },
    );
    line(
        "share",
        match paths::share_dir(&exe) {
            Some(s) => short(&s),
            None => "MISSING: no share/ beside the binary; residents cannot start".into(),
        },
    );
    let claude = crate::daemon::launch::claude(path.as_deref());
    line(
        "claude",
        match &claude {
            None => "MISSING: install Claude Code (https://code.claude.com)".into(),
            Some(c) => {
                let v = run(c, &["--version"], Duration::from_secs(10));
                let v = v.map(|o| o.lines().next().unwrap_or("").replace(" (Claude Code)", ""));
                format!("{}  {}", v.as_deref().unwrap_or("?"), short(c))
            }
        },
    );
    // The registry is undocumented: a change in it shows here first.
    if let Some(c) = &claude {
        let out = run(c, &["agents", "--json"], Duration::from_secs(5));
        let parsed = out.as_deref().and_then(|o| {
            let all: Vec<serde_json::Value> = serde_json::from_str(o).ok()?;
            let read = crate::daemon::registry::parse(o.as_bytes())?.len();
            Some((all.len(), read))
        });
        line(
            "registry",
            match parsed {
                Some((n, r)) if n == r => format!("`claude agents --json` lists {n} sessions"),
                Some((n, r)) => format!("{} of {n} sessions unreadable: its format moved", n - r),
                None => "`claude agents --json` gave nothing readable: residents' states come \
                     from their hooks alone"
                    .into(),
            },
        );
    }
    line("daemon", daemon());
    line("at login", login::status());
    // A daemon launchd starts has the agent's environment, not this shell's.
    if let login::Agent::Ours(_) = login::agent() {
        let proxy = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"];
        let mut unseen: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
        unseen
            .retain(|k| k.starts_with("ANTHROPIC_") || proxy.contains(&k.to_uppercase().as_str()));
        unseen.sort();
        if !unseen.is_empty() {
            line("", format!("{} set here never reach residents launchd starts", unseen.join(" ")));
        }
    }
    let state = paths::state_dir();
    line("state", short(&state));
    let config = paths::config_dir();
    let defaults = if config.join("config").is_file() { "" } else { " (defaults)" };
    line("config", format!("{}{defaults}", short(&config)));
    let sock = paths::socket_path();
    let real = std::fs::canonicalize(&sock).unwrap_or(sock);
    line("socket", short(&real));
    line(
        "terminal",
        match std::env::var("TERM_PROGRAM").as_deref() {
            Ok("iTerm.app") => {
                "iTerm2: banners while a client is attached (NOTIFY_DESKTOP=off in the \
                            config stops them)"
                    .into()
            }
            Ok("Apple_Terminal") => "Terminal.app: no banners; turn on \"Use Option as Meta key\" \
                                 for Alt keys"
                .into(),
            Ok(t) => format!("{t}: banners if it shows OSC 9 notifications"),
            Err(_) => "unknown (TERM_PROGRAM unset)".into(),
        },
    );
    let mut claude_env: Vec<String> =
        std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("CLAUDE")).collect();
    claude_env.retain(|k| k != "CLAUDE_CONFIG_DIR");
    if !claude_env.is_empty() {
        let n = claude_env.len();
        line("shell env", format!("{n} CLAUDE* variables here, all kept from residents"));
    }
    Ok(())
}

/// The daemon as it is, asked without starting one.
fn daemon() -> String {
    let pid = UnixStream::connect(paths::socket_path()).ok().and_then(|s| super::peer_pid(&s));
    let pid = pid.map_or(String::new(), |p| format!(" (pid {p})"));
    match request(Request::List { all: false }, false) {
        Err(Error::NotRunning) => "not running (`gensokyo` starts it)".into(),
        Err(e @ Error::Refused { .. }) => e.to_string(),
        Err(e) => format!("not answering: {e}"),
        Ok(Reply::List { residents, .. }) => {
            let n = residents.len();
            format!("running{pid}, {n} resident{}", if n == 1 { "" } else { "s" })
        }
        Ok(other) => format!("unexpected reply {other:?}"),
    }
}

/// A command's stdout, if it succeeds within `most`. Read as it comes, so a long answer cannot
/// fill the pipe and hold the child up.
fn run(cmd: &Path, args: &[&str], most: Duration) -> Option<String> {
    let mut c = Command::new(cmd);
    c.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = c.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        std::io::Read::read_to_string(&mut stdout, &mut out).map(|_| out).ok()
    });
    let t = Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.success(),
            Ok(None) if t.elapsed() <= most => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let out = reader.join().ok().flatten();
    out.filter(|_| ok)
}
