//! The plumbing verbs: one request over the socket, starting the daemon when nobody answers.

use crate::proto::{self, Envelope, Reply, Request, Summon};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

/// tmux's algorithm: connect; failing that, take the client lock, connect again (another client
/// may have started the daemon meanwhile), and only then spawn it, holding the lock until the
/// socket answers so the clients queued behind it connect instead of spawning.
pub fn connect_or_start() -> Result<UnixStream, String> {
    let sock = proto::socket_path();
    if let Ok(s) = UnixStream::connect(&sock) {
        return Ok(s);
    }
    let run = proto::state_dir().join("run");
    std::fs::create_dir_all(&run).map_err(|e| format!("{}: {e}", run.display()))?;
    let lock = std::fs::File::create(run.join("client.lock")).map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| format!("client lock: {e}"))?;
    if let Ok(s) = UnixStream::connect(&sock) {
        return Ok(s);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            (libc::setsid() != -1).then_some(()).ok_or_else(std::io::Error::last_os_error)
        })
    };
    let mut child = cmd.spawn().map_err(|e| format!("start the daemon: {e}"))?;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(5) {
        if let Ok(s) = UnixStream::connect(&sock) {
            // It stays running on its own; reap it if it ever exits while we run.
            std::thread::spawn(move || child.wait());
            return Ok(s);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err("the daemon did not come up in 5 s".into())
}

/// hello/welcome, then one request and its reply.
pub fn request(req: Request, start: bool) -> Result<Reply, String> {
    let s = if start {
        connect_or_start()?
    } else {
        UnixStream::connect(proto::socket_path()).map_err(|_| "the daemon is not running")?
    };
    let mut w = s.try_clone().map_err(|e| e.to_string())?;
    let hello = Envelope { id: 0, req: Request::Hello { proto: proto::PROTO, who: "cli".into() } };
    let line = |e: &Envelope| serde_json::to_string(e).unwrap_or_default();
    writeln!(w, "{}\n{}", line(&hello), line(&Envelope { id: 1, req }))
        .map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(s).lines();
    let mut next = || -> Result<Reply, String> {
        let l = lines.next().ok_or("the daemon hung up")?.map_err(|e| e.to_string())?;
        serde_json::from_str(&l).map_err(|e| format!("{e}: {l}"))
    };
    match next()? {
        Reply::Welcome { .. } => next(),
        other => Ok(other),
    }
    .and_then(|r| match r {
        Reply::Error { error, .. } => Err(error),
        r => Ok(r),
    })
}

pub fn main(args: &[String]) -> ExitCode {
    let verb = args.first().map(String::as_str).unwrap_or("");
    let rest = &args[1.min(args.len())..];
    let r = match verb {
        "list" => list(rest),
        "new" => new(rest),
        "resume" => match rest {
            [who] => resume(who),
            _ => Err("usage: gensokyo resume <name|id>".into()),
        },
        "banish" | "close" => match rest {
            [who] if verb == "banish" => say(request(Request::Banish { who: who.clone() }, false)),
            [who] => say(request(Request::Close { who: who.clone() }, false)),
            _ => Err(format!("usage: gensokyo {verb} <name|slot>")),
        },
        "quit" => match request(Request::Quit, false) {
            Err(e) if e == "the daemon is not running" => Ok(()),
            r => say(r),
        },
        _ => Err(format!("unknown command {verb:?}")),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gensokyo: {e}");
            ExitCode::FAILURE
        }
    }
}

fn say(r: Result<Reply, String>) -> Result<(), String> {
    match r? {
        Reply::Done { message, .. } => {
            println!("{message}");
            Ok(())
        }
        other => Err(format!("unexpected reply {other:?}")),
    }
}

fn list(args: &[String]) -> Result<(), String> {
    let json = args.iter().any(|a| a == "--json");
    let all = args.iter().any(|a| a == "--all");
    let residents = match request(Request::List { all }, true)? {
        Reply::List { residents, .. } => residents,
        other => return Err(format!("unexpected reply {other:?}")),
    };
    if json {
        println!("{}", serde_json::to_string(&residents).unwrap_or_default());
        return Ok(());
    }
    let now = crate::daemon::store::now();
    for r in &residents {
        let slot = r.slot.map_or("-".into(), |s| s.to_string());
        let state = serde_json::to_value(r.state).ok();
        let state = state.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        let t = r.telemetry.as_ref();
        let mut fields = crate::tele::fields(t, r.mode.as_deref(), r.branch.as_deref(), true);
        if let Some(at) = t.map(|t| t.at).filter(|at| *at > 0) {
            fields = format!("{fields} · {} ago", crate::tele::age(now.saturating_sub(at) as u64));
        }
        println!("{slot} {} {:<12} {state:<8} {}  {fields}", r.state.glyph(), r.name, r.cwd);
    }
    let newest = residents.iter().filter_map(|r| r.telemetry.as_ref()).max_by_key(|t| t.at);
    let usage: Vec<String> = newest
        .into_iter()
        .flat_map(|t| [("5h", &t.five_hour), ("wk", &t.seven_day)])
        .filter_map(|(label, l)| l.as_ref().map(|l| crate::tele::usage(label, l, now, 10)))
        .collect();
    if !usage.is_empty() {
        println!("usage {}", usage.join("   "));
    }
    Ok(())
}

fn resume(who: &str) -> Result<(), String> {
    match request(Request::Recall { who: who.into() }, true)? {
        Reply::Summoned { resident: r, .. } => {
            let slot = r.slot.map_or(String::new(), |s| format!(" (slot {s})"));
            println!("recalled {}{slot} in {}", r.name, r.cwd);
            Ok(())
        }
        other => Err(format!("unexpected reply {other:?}")),
    }
}

fn new(args: &[String]) -> Result<(), String> {
    let mut s = Summon::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("new: {a} needs a value"));
        match a.as_str() {
            "-n" | "--name" => s.name = Some(val()?),
            "-m" | "--model" => s.model = Some(val()?),
            "-e" | "--effort" => s.effort = Some(val()?),
            "-p" | "--permission-mode" => s.mode = Some(val()?),
            "--prompt" => s.prompt = Some(val()?),
            _ if a.starts_with('-') => return Err(format!("new: unknown option {a}")),
            _ if !s.cwd.is_empty() => return Err("new: one directory only".into()),
            _ => s.cwd = a.clone(),
        }
    }
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    s.cwd = match s.cwd.as_str() {
        "" => here.to_string_lossy().into_owned(),
        c => here.join(c).to_string_lossy().into_owned(),
    };
    match request(Request::Summon(s), true)? {
        Reply::Summoned { resident: r, .. } => {
            let slot = r.slot.map_or(String::new(), |s| format!(" (slot {s})"));
            println!("summoned {}{slot} in {}", r.name, r.cwd);
            Ok(())
        }
        other => Err(format!("unexpected reply {other:?}")),
    }
}
