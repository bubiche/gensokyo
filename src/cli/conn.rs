//! The socket: connecting, starting the daemon when nobody answers, one request, and replacing
//! the daemon that is running with this binary's.

use crate::daemon::store::{Record, Store};
use crate::paths;
use crate::proto::{self, Envelope, Reply, Request};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Why a request got no answer.
#[derive(Debug)]
pub enum Error {
    NotRunning,
    /// The daemon turned our hello away: another build's, speaking `proto`.
    Refused {
        proto: Option<u32>,
        pid: Option<i32>,
    },
    /// The daemon's own answer: no such resident, and the like.
    Daemon(String),
    Io(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::NotRunning => f.write_str("the daemon is not running"),
            Error::Refused { proto, pid } => {
                f.write_str("the running daemon is ")?;
                match proto {
                    Some(p) => write!(f, "protocol {p}")?,
                    None => f.write_str("from another build")?,
                }
                if let Some(pid) = pid {
                    write!(f, " (pid {pid})")?;
                }
                write!(f, " and this is protocol {}; `gensokyo restart` replaces it", proto::PROTO)
            }
            Error::Daemon(e) | Error::Io(e) => f.write_str(e),
        }
    }
}

impl From<Error> for String {
    fn from(e: Error) -> String {
        e.to_string()
    }
}

/// The daemon's refusal of a hello (`protocol 5, want 4`), with who sent it.
pub fn refusal(error: &str, pid: Option<i32>) -> Option<Error> {
    let want = error.strip_prefix("protocol ")?.split_once(", want ")?.1;
    Some(Error::Refused { proto: want.trim().parse().ok(), pid })
}

/// The process at the other end of a unix socket.
pub fn peer_pid(s: &UnixStream) -> Option<i32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: a pid_t-sized out parameter and its length, on a descriptor we hold.
    let r = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &mut len,
        )
    };
    (r == 0 && pid > 0).then_some(pid)
}

/// tmux's algorithm: connect; failing that, take the client lock, connect again (another client
/// may have started the daemon meanwhile), and only then spawn it, holding the lock until the
/// socket answers so the clients queued behind it connect instead of spawning.
pub fn connect_or_start() -> Result<UnixStream, String> {
    let sock = paths::socket_path();
    if let Ok(s) = UnixStream::connect(&sock) {
        return Ok(s);
    }
    let run = paths::state_dir().join("run");
    std::fs::create_dir_all(&run).map_err(|e| format!("{}: {e}", run.display()))?;
    let lock = std::fs::File::create(run.join("client.lock")).map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| format!("client lock: {e}"))?;
    if let Ok(s) = UnixStream::connect(&sock) {
        return Ok(s);
    }
    let log_from = std::fs::metadata(paths::state_dir().join("daemon.log")).map_or(0, |m| m.len());
    // With the login agent running this binary, launchd starts the daemon, and starts it again
    // if it crashes; a plist written but not loaded leaves it to us.
    if super::login::supervises_me() && super::login::kickstart() {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            if let Ok(s) = UnixStream::connect(&sock) {
                return Ok(s);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        return Err(format!(
            "launchd started the daemon, and it did not answer in 5 s{}",
            why(log_from)
        ));
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
        // Exit 0 is losing the lock to a daemon started some other way, which binds soon.
        if let Ok(Some(status)) = child.try_wait().map(|s| s.filter(|s| !s.success())) {
            return Err(format!("the daemon stopped as it started ({status}){}", why(log_from)));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(format!("the daemon did not come up in 5 s{}", why(log_from)))
}

/// The lines of `daemon.log` written since `from` that say why it stopped, and where the rest
/// is: older ones are some earlier daemon's. A log now shorter than `from` moved to
/// `daemon.log.1` meanwhile: the rest of that, then all of the new one.
fn why(from: u64) -> String {
    let path = paths::state_dir().join("daemon.log");
    let from = from as usize;
    let mut all = std::fs::read(&path).unwrap_or_default();
    if all.len() < from {
        let old = std::fs::read(path.with_extension("log.1")).unwrap_or_default();
        all = [old.get(from..).unwrap_or_default(), &all].concat();
    } else {
        all.drain(..from);
    }
    let text = String::from_utf8_lossy(&all);
    // A JSON line that is an exit or a panic, or anything written to stderr as it is.
    let telling = |l: &&str| match serde_json::from_str::<serde_json::Value>(l) {
        Ok(v) => matches!(v["ev"].as_str(), Some("exit" | "panic" | "error")),
        Err(_) => !l.trim().is_empty(),
    };
    let lines: Vec<&str> = text.lines().rev().filter(telling).take(5).collect();
    let mut s: String = lines.iter().rev().map(|l| format!("\n  {l}")).collect();
    s.push_str(&format!(
        "\n  (the daemon's log: {})",
        crate::paths::short(&path.to_string_lossy())
    ));
    s
}

/// hello/welcome, then one request and its reply.
pub fn request(req: Request, start: bool) -> Result<Reply, Error> {
    let s = if start {
        connect_or_start().map_err(Error::Io)?
    } else {
        UnixStream::connect(paths::socket_path()).map_err(|_| Error::NotRunning)?
    };
    let pid = peer_pid(&s);
    let io = |e: std::io::Error| Error::Io(e.to_string());
    let mut w = s.try_clone().map_err(io)?;
    let hello = Envelope { id: 0, req: proto::hello("cli") };
    let line = |e: &Envelope| serde_json::to_string(e).unwrap_or_default();
    writeln!(w, "{}\n{}", line(&hello), line(&Envelope { id: 1, req })).map_err(io)?;
    let mut lines = BufReader::new(s).lines();
    let mut next = || -> Result<Reply, Error> {
        let l = lines.next().ok_or(Error::Io("the daemon hung up".into()))?.map_err(io)?;
        serde_json::from_str(&l).map_err(|e| Error::Io(format!("{e}: {l}")))
    };
    match next()? {
        Reply::Welcome { .. } => next(),
        other => Ok(other),
    }
    .and_then(|r| match r {
        // The hello is id 0: its refusal is the daemon's build, not the request.
        Reply::Error { id: 0, error } => Err(refusal(&error, pid).unwrap_or(Error::Daemon(error))),
        Reply::Error { error, .. } => Err(Error::Daemon(error)),
        r => Ok(r),
    })
}

/// How long the daemon may take to see everyone out: two `/exit`s each, then the sweep.
const STOP_WAIT: Duration = Duration::from_secs(45);

/// The daemon that is running, whatever its build, replaced by this binary's. Who it had is
/// read off disk and left in `run/comeback` for the new daemon, which recalls them as it starts,
/// each into its own conversation; the old one is stopped as SIGTERM stops it (each asked to
/// `/exit`), and killed if it has not gone in `STOP_WAIT`. With no daemon running, the records
/// a crashed one left are the ones brought back.
pub fn restart() -> Result<(), String> {
    // SIGTERM asks every resident to leave, and this process with the one it runs in.
    if std::env::var_os("GENSOKYO_RESIDENT").is_some() {
        return Err("restart stops every resident, this one too; run it from a terminal of \
                    your own, outside gensokyo"
            .into());
    }
    let old = match UnixStream::connect(paths::socket_path()).map(|s| peer_pid(&s)) {
        Err(_) => None,
        Ok(None) => return Err("could not tell which process the daemon is".into()),
        Ok(pid) => pid,
    };
    let store = Store::new(paths::state_dir());
    let mut live = Vec::new();
    for r in store.load() {
        match r {
            Ok(r) if r.departed.is_none() => live.push(r),
            Ok(_) => {}
            Err(e) => eprintln!("gensokyo: a record that cannot be read stays behind: {e}"),
        }
    }
    live.sort_by_key(|r: &Record| (r.slot.unwrap_or(u8::MAX), r.launched));
    let ids: String = live.iter().map(|r| format!("{}\n", r.id)).collect();
    let _ = std::fs::create_dir_all(paths::state_dir().join("run"));
    crate::daemon::store::write_atomic(&paths::comeback_path(), ids.as_bytes())
        .map_err(|e| format!("{}: {e}", paths::comeback_path().display()))?;
    let n = live.len();
    match old {
        None => {}
        Some(pid) => {
            let who = match n {
                0 => String::new(),
                1 => "; its one resident is asked to /exit first".into(),
                _ => format!("; its {n} residents are asked to /exit first"),
            };
            println!("stopping the daemon (pid {pid}){who}");
            stop(pid)?;
        }
    }
    let s = connect_or_start()?;
    let new = peer_pid(&s).map_or(String::new(), |p| format!(" (pid {p})"));
    drop(s);
    match old {
        None => println!("no daemon was running; started one{new}"),
        Some(_) => println!("the new daemon is up{new}"),
    }
    if let super::login::Agent::Ours(p) = super::login::agent()
        && !super::login::supervises_me()
    {
        let p = paths::short(&p.to_string_lossy());
        eprintln!("gensokyo: the login agent runs {p}, not this binary, so launchd does not");
        eprintln!("  watch this daemon; `gensokyo login setup` from here points it at this one");
    }
    // The recalls are done by the time it answers: they come before its first request.
    let back: Vec<proto::Resident> = match request(Request::List { all: false }, false) {
        Ok(Reply::List { residents, .. }) => residents,
        Ok(other) => return Err(format!("unexpected reply {other:?}")),
        Err(e) => return Err(e.to_string()),
    };
    let mut lost = 0;
    for r in &live {
        match back.iter().find(|b| b.id == r.id && b.departed.is_none()) {
            Some(b) => {
                let slot = b.slot.map_or(String::new(), |s| format!(" (slot {s})"));
                println!("recalled {}{slot} in {}", b.name, b.cwd);
            }
            None => {
                lost += 1;
                eprintln!("gensokyo: {} did not come back; the daemon's log says why", r.name);
            }
        }
    }
    match lost {
        0 => Ok(()),
        _ => Err(format!("{lost} of {n} did not come back; `gensokyo resume <name>` tries again")),
    }
}

/// SIGTERM, and SIGKILL once `STOP_WAIT` has gone by.
fn stop(pid: i32) -> Result<(), String> {
    // SAFETY: plain libc calls; signal 0 only asks whether it is there.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(format!("could not stop pid {pid}: {}", std::io::Error::last_os_error()));
    }
    let gone = |wait: Duration| {
        let t = Instant::now();
        while unsafe { libc::kill(pid, 0) } == 0 {
            if t.elapsed() > wait {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        true
    };
    if gone(STOP_WAIT) {
        return Ok(());
    }
    eprintln!("gensokyo: the daemon is still stopping after {} s; killing it", STOP_WAIT.as_secs());
    unsafe { libc::kill(pid, libc::SIGKILL) };
    match gone(Duration::from_secs(5)) {
        true => Ok(()),
        false => Err(format!("pid {pid} would not die")),
    }
}
