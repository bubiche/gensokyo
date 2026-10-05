//! `gensokyo login`: the launchd agent that starts the daemon when you log in, so rituals fire on
//! a day nobody opened a terminal, and starts it again if it crashes. With the agent in place a
//! client asks launchd to start the daemon rather than starting one itself, so the one running is
//! always the supervised one.
//!
//! One plist of gensokyo's own under ~/Library/LaunchAgents is all this writes; a plist at the
//! same label that does not run a gensokyo is somebody else's and is never loaded or removed.

use crate::paths;
use clap::Subcommand;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Subcommand)]
pub enum Cmd {
    /// Start the daemon at every login (and now), with this binary and this shell's PATH
    Setup,
    /// Stop starting it at login; a daemon already running keeps running
    Remove,
}

/// One label for every copy on the Mac: two agents would race for the same state dir at login,
/// so a setup from a second copy replaces the first. `$GENSOKYO_LAUNCH_LABEL` is for tests.
pub fn label() -> String {
    std::env::var("GENSOKYO_LAUNCH_LABEL").unwrap_or_else(|_| "io.github.bubiche.gensokyo".into())
}

pub fn plist() -> PathBuf {
    let dir = std::env::var_os("GENSOKYO_LAUNCH_DIR")
        .map_or_else(|| PathBuf::from(paths::home()).join("Library/LaunchAgents"), PathBuf::from);
    dir.join(format!("{}.plist", label()))
}

/// Tests hand this a stand-in that writes its arguments down: a suite that ran the real one would
/// leave an agent behind on the machine it ran on.
fn launchctl() -> Command {
    let mut c = Command::new(std::env::var_os("GENSOKYO_LAUNCHCTL").unwrap_or("launchctl".into()));
    c.stdin(Stdio::null());
    c
}

fn domain() -> String {
    // SAFETY: getuid cannot fail.
    format!("gui/{}", unsafe { libc::getuid() })
}

fn target() -> String {
    format!("{}/{}", domain(), label())
}

/// What is at the label: nothing, somebody else's plist, or ours and the binary it runs.
pub enum Agent {
    None,
    Foreign,
    Ours(PathBuf),
}

pub fn agent() -> Agent {
    let p = plist();
    if !p.exists() {
        return Agent::None;
    }
    let out = Command::new("plutil")
        .args(["-extract", "ProgramArguments.0", "raw", "-o", "-"])
        .arg(&p)
        .stderr(Stdio::null())
        .output();
    let prog = out.ok().filter(|o| o.status.success()).map(|o| o.stdout);
    let prog = prog.map(|s| String::from_utf8_lossy(&s).trim_end().to_string());
    match prog {
        Some(p) if p.ends_with("/gensokyo") => Agent::Ours(p.into()),
        _ => Agent::Foreign,
    }
}

/// What launchd says of the job: `None` when the domain does not hold it (`print` answers 113),
/// else whether its process is running.
fn job() -> Option<bool> {
    let out = launchctl().arg("print").arg(target()).stderr(Stdio::null()).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    out.status.success().then(|| text.lines().any(|l| l.trim() == "state = running"))
}

/// `bootout` stops the job's process with SIGTERM, which asks every resident to /exit: never as
/// a side effect of setting up or removing the agent.
fn refuse_running(what: &str) -> Result<(), String> {
    match job() {
        Some(true) => Err(format!(
            "the daemon launchd started is running, and {what} would stop it: `gensokyo quit` \
             first (each resident is asked to /exit), then this again"
        )),
        _ => Ok(()),
    }
}

/// The agent's binary, when it is this one: then launchd is who starts the daemon.
pub fn supervises_me() -> bool {
    let me = std::env::current_exe().and_then(std::fs::canonicalize);
    match (agent(), me) {
        (Agent::Ours(p), Ok(me)) => std::fs::canonicalize(p).is_ok_and(|p| p == me),
        _ => false,
    }
}

/// Ask launchd to start the job if it is not running. False when launchd does not hold it (a
/// plist written but not loaded): the caller starts the daemon itself.
pub fn kickstart() -> bool {
    launchctl()
        .arg("kickstart")
        .arg(target())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// XML has five characters that cannot be written as themselves, and a path may hold three.
fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The agent as installed. KeepAlive only on a failure: `gensokyo quit` exits 0 and stays down
/// until the next login or client, a crash comes back. PATH is baked in because launchd gives an
/// agent /usr/bin:/bin:/usr/sbin:/sbin, and `claude` lives under Homebrew or ~/.local. The
/// GENSOKYO_* places carry over only when set, so an install run against a state dir of its own
/// keeps it.
pub fn render(exe: &Path, env: &[(String, String)]) -> String {
    let kv = |k: &str, v: &str| format!("    <key>{}</key><string>{}</string>\n", xml(k), xml(v));
    let state = paths::state_dir();
    let log = state.join("login.log").to_string_lossy().into_owned();
    let mut s = String::from(concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
        "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
        "<plist version=\"1.0\">\n<dict>\n",
    ));
    s += &kv("Label", &label());
    s += "    <key>ProgramArguments</key>\n    <array>\n";
    for a in [&*exe.to_string_lossy(), "daemon", "--foreground"] {
        s += &format!("      <string>{}</string>\n", xml(a));
    }
    s += "    </array>\n";
    s += "    <key>RunAtLoad</key><true/>\n";
    s += "    <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n";
    // Residents are interactive: launchd must not throttle them as background work.
    s += &kv("ProcessType", "Interactive");
    s += &kv("WorkingDirectory", &paths::home());
    // The daemon logs to daemon.log itself; this catches what happens before it can.
    s += &kv("StandardOutPath", &log);
    s += &kv("StandardErrorPath", &log);
    s += "    <key>EnvironmentVariables</key>\n    <dict>\n";
    for (k, v) in env {
        s += &format!("    {}", kv(k, v));
    }
    s += "    </dict>\n</dict>\n</plist>\n";
    s
}

/// What the agent is given of this shell's environment.
fn baked() -> Vec<(String, String)> {
    const CARRIED: [&str; 7] = [
        "PATH",
        "GENSOKYO_STATE_DIR",
        "GENSOKYO_CONFIG_DIR",
        "GENSOKYO_SOCKET",
        "GENSOKYO_SHARE",
        "GENSOKYO_CLAUDE",
        "CLAUDE_CONFIG_DIR",
    ];
    CARRIED
        .iter()
        .filter_map(|k| Some((k.to_string(), std::env::var(k).ok().filter(|v| !v.is_empty())?)))
        .collect()
}

pub fn main(cmd: Option<Cmd>) -> Result<(), String> {
    match cmd {
        None => {
            println!("at login: {}", status());
            tail();
            Ok(())
        }
        Some(Cmd::Setup) => setup(),
        Some(Cmd::Remove) => remove(),
    }
}

/// The one line `login` and `doctor` both say. A plist left pointing at a binary that has gone is
/// the case that fails silently: it loads at every login and dies before it can log a thing.
pub fn status() -> String {
    let p = paths::short(&plist().to_string_lossy());
    match agent() {
        Agent::None => "off: `gensokyo login setup` starts the daemon at login, so rituals fire \
                        on a day you never open a terminal"
            .into(),
        Agent::Foreign => format!("{p} holds an agent gensokyo did not write: left alone"),
        Agent::Ours(prog) if !prog.exists() => format!(
            "STALE: it runs {}, which is not there any more (`gensokyo login setup` repoints it)",
            paths::short(&prog.to_string_lossy())
        ),
        Agent::Ours(prog) => {
            let prog = paths::short(&prog.to_string_lossy());
            match job().is_some() {
                true => format!("on: launchd runs {prog} at login"),
                false => format!(
                    "written, not loaded: {prog} (it loads at your next login; `gensokyo login \
                     setup` loads it now)"
                ),
            }
        }
    }
}

/// What launchd caught the last time, if anything: a working agent writes nothing there.
fn tail() {
    let f = paths::state_dir().join("login.log");
    let text = std::fs::read_to_string(&f).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return;
    }
    println!("  it last said, in {}:", paths::short(&f.to_string_lossy()));
    for l in &lines[lines.len().saturating_sub(5)..] {
        println!("    {l}");
    }
}

fn setup() -> Result<(), String> {
    let dest = plist();
    let short = paths::short(&dest.to_string_lossy());
    if let Agent::Foreign = agent() {
        return Err(format!("{short} was not written by gensokyo; move it aside first"));
    }
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).map_err(|e| e.to_string())?;
    let env = baked();
    let path = env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.as_str());
    if crate::daemon::launch::claude(path.map(std::ffi::OsStr::new)).is_none() {
        return Err("no `claude` on this shell's PATH, which is the PATH the agent gets: \
                    run this from a shell where `claude` works"
            .into());
    }
    // launchd opens the log before the daemon runs, and fails the job if its dir is missing.
    let state = paths::state_dir();
    std::fs::create_dir_all(&state).map_err(|e| format!("{}: {e}", state.display()))?;
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let text = render(&exe, &env);
    if std::fs::read_to_string(&dest).is_ok_and(|t| t == text) && job().is_some() {
        println!("already on: launchd runs {} at login", paths::short(&exe.to_string_lossy()));
        return Ok(());
    }
    refuse_running("loading the new agent")?;
    let tmp = dest.with_extension("plist.new");
    std::fs::write(&tmp, &text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let lint = Command::new("plutil").arg("-lint").arg(&tmp).stdout(Stdio::null()).status();
    if !lint.is_ok_and(|s| s.success()) {
        let _ = std::fs::remove_file(&tmp);
        return Err("the agent's plist does not lint (plutil -lint); nothing was changed".into());
    }
    std::fs::rename(&tmp, &dest).map_err(|e| format!("{short}: {e}"))?;
    // `bootstrap` refuses a label the domain already holds, so the old one comes out first; with
    // nothing to boot out that is a no-op.
    let _ = launchctl()
        .arg("bootout")
        .arg(target())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let boot = launchctl().arg("bootstrap").arg(domain()).arg(&dest).stderr(Stdio::null()).status();
    println!("wrote {short}: launchd runs {} at login.", paths::short(&exe.to_string_lossy()));
    if boot.is_ok_and(|s| s.success()) {
        println!("It is loaded, and starts the daemon now unless one is running already; the next");
        println!("time it is needed, launchd starts it.");
    } else {
        eprintln!("gensokyo: launchd would not load it now; it loads at your next login");
    }
    Ok(())
}

fn remove() -> Result<(), String> {
    let dest = plist();
    let short = paths::short(&dest.to_string_lossy());
    match agent() {
        Agent::None => {
            println!("nothing to remove: {short} is not there");
            return Ok(());
        }
        Agent::Foreign => return Err(format!("{short} was not written by gensokyo: left alone")),
        Agent::Ours(_) => {}
    }
    refuse_running("removing the agent")?;
    let _ = launchctl()
        .arg("bootout")
        .arg(target())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    std::fs::remove_file(&dest).map_err(|e| format!("{short}: {e}"))?;
    println!("removed {short}: the daemon no longer starts at login.");
    Ok(())
}
