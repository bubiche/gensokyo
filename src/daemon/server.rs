//! The daemon process: one per state dir (a flock held for life), NDJSON over a unix socket,
//! and the shrine of residents, all on one thread.

use super::launch;
use super::pty;
use super::resident::{self, Handle};
use super::store::{self, Record, Store};
use crate::proto::{self, Envelope, Reply, Request, Summon};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

/// From HUP to TERM: how long a leader gets to leave on its own. Claude Code on haiku took
/// 0.74-1.35 s over 20 banishes, idle and mid-turn; this is about twice the slowest.
pub const BANISH_GRACE: Duration = Duration::from_secs(3);
/// How long one resident gets to act on one `/exit`; one that runs out is asked once more.
const EXIT_WAIT: Duration = Duration::from_secs(6);
/// `quit` waits this long for everyone's `/exit`, then banishes whoever is left.
const QUIT_WAIT: Duration = Duration::from_secs(20);
/// Until a client says how big it is.
const SIZE: (u16, u16) = (80, 24);

static LOG: Mutex<Option<File>> = Mutex::new(None);

/// One JSON line in `daemon.log`.
pub fn log(mut v: Value) {
    v["ts"] = json!(store::now());
    if let Some(f) = LOG.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = writeln!(f, "{v}");
    }
}

struct Entry {
    rec: Record,
    handle: Option<Rc<Handle>>,
}

struct Shrine {
    entries: Vec<Entry>,
    store: Store,
    exe: PathBuf,
    share: Option<PathBuf>,
    socket: PathBuf,
    quitting: bool,
}

type Shared = Rc<RefCell<Shrine>>;

pub fn main() -> std::process::ExitCode {
    let root = proto::state_dir();
    let run = root.join("run");
    for d in [&run, &root.join("residents"), &root.join("departed")] {
        if let Err(e) = std::fs::create_dir_all(d) {
            eprintln!("gensokyo daemon: {}: {e}", d.display());
            return std::process::ExitCode::FAILURE;
        }
    }
    let _ = std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700));
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) =
        OpenOptions::new().create(true).append(true).open(root.join("daemon.log")).ok();
    let Ok(lock) =
        OpenOptions::new().create(true).truncate(false).write(true).open(run.join("daemon.lock"))
    else {
        return std::process::ExitCode::FAILURE;
    };
    // The lock comes before the socket: whoever holds it owns the socket path, and a socket file
    // already there is stale. Losing it is not an error (launchd's KeepAlive would retry).
    if lock.try_lock().is_err() {
        log(json!({"ev": "exit", "why": "another daemon holds run/daemon.lock"}));
        return std::process::ExitCode::SUCCESS;
    }
    let _ = std::env::set_current_dir("/");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build();
    let Ok(rt) = rt else { return std::process::ExitCode::FAILURE };
    let code = tokio::task::LocalSet::new().block_on(&rt, serve(Store { root }));
    drop(lock);
    code
}

async fn serve(store: Store) -> std::process::ExitCode {
    let path = proto::socket_path();
    // A socket that answers belongs to a daemon for another state dir (one started from inside a
    // resident, which inherits `GENSOKYO_SOCKET`): never take it over.
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        log(json!({"ev": "exit", "why": format!("{} is in use", path.display())}));
        return std::process::ExitCode::FAILURE;
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            log(json!({"ev": "exit", "why": format!("bind {}: {e}", path.display())}));
            return std::process::ExitCode::FAILURE;
        }
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    let ino = std::fs::metadata(&path).map(|m| m.ino()).unwrap_or(0);
    let exe = std::env::current_exe().unwrap_or_else(|_| "gensokyo".into());
    // Nobody from before this start is still running: their masters closed with that daemon.
    for r in store.load() {
        match r {
            Ok(mut r) => {
                r.departed.get_or_insert(store::now());
                let _ = store.retire(&r);
            }
            Err(e) => log(json!({"ev": "record", "error": e})),
        }
    }
    let shrine = Rc::new(RefCell::new(Shrine {
        entries: Vec::new(),
        store,
        share: share_dir(&exe),
        socket: std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()),
        exe,
        quitting: false,
    }));
    log(
        json!({"ev": "started", "pid": std::process::id(), "socket": path, "ppid": unsafe { libc::getppid() }}),
    );
    let quit = Rc::new(Notify::new());
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
        signal(SignalKind::hangup()),
    ) else {
        return std::process::ExitCode::FAILURE;
    };
    loop {
        tokio::select! {
            a = listener.accept() => if let Ok((s, _)) = a {
                tokio::task::spawn_local(conn(shrine.clone(), quit.clone(), s));
            },
            _ = quit.notified() => break,
            _ = term.recv() => { leave_all(&shrine).await; break }
            _ = int.recv() => { leave_all(&shrine).await; break }
            _ = hup.recv() => {}
        }
    }
    // Only our own socket: a newer daemon may already have bound a new one at this path.
    if std::fs::metadata(&path).is_ok_and(|m| m.ino() == ino) {
        let _ = std::fs::remove_file(&path);
    }
    log(json!({"ev": "exit", "why": "quit"}));
    std::process::ExitCode::SUCCESS
}

/// `share/` beside the binary or up to three levels above it (a build tree), or
/// `$GENSOKYO_SHARE`.
fn share_dir(exe: &Path) -> Option<PathBuf> {
    if let Some(s) = std::env::var_os("GENSOKYO_SHARE") {
        return Some(s.into());
    }
    exe.ancestors().skip(1).take(4).map(|d| d.join("share")).find(|s| s.join("names.txt").is_file())
}

async fn conn(shrine: Shared, quit: Rc<Notify>, s: UnixStream) {
    // SAFETY: getuid cannot fail.
    if s.peer_cred().map(|c| c.uid()).ok() != Some(unsafe { libc::getuid() }) {
        return;
    }
    let (r, mut w) = s.into_split();
    let mut lines = BufReader::new(r).lines();
    let mut greeted = false;
    while let Ok(Some(line)) = lines.next_line().await {
        let (reply, stop) = match serde_json::from_str::<Envelope>(&line) {
            Err(e) => (Reply::Error { id: 0, error: format!("bad request: {e}") }, !greeted),
            Ok(Envelope { req: Request::Hello { proto, .. }, id }) => {
                greeted = proto == proto::PROTO;
                let reply = if greeted {
                    Reply::Welcome { proto: proto::PROTO, pid: std::process::id() }
                } else {
                    Reply::Error { id, error: format!("protocol {proto}, want {}", proto::PROTO) }
                };
                (reply, !greeted)
            }
            Ok(Envelope { id, .. }) if !greeted => {
                (Reply::Error { id, error: "say hello first".into() }, true)
            }
            Ok(Envelope { id, req: Request::Quit }) => {
                leave_all(&shrine).await;
                (Reply::Done { id, message: "the shrine is empty; the daemon stops".into() }, true)
            }
            Ok(Envelope { id, req }) => (handle(&shrine, id, req).await, false),
        };
        let mut out = serde_json::to_vec(&reply).unwrap_or_default();
        out.push(b'\n');
        let sent = w.write_all(&out).await.is_ok();
        // After a quit the daemon stops whether or not the asker is still there to hear it.
        if stop && matches!(reply, Reply::Done { .. }) {
            quit.notify_one();
        }
        if stop || !sent {
            return;
        }
    }
}

async fn handle(shrine: &Shared, id: u64, req: Request) -> Reply {
    let r = match req {
        Request::List => Ok(Reply::List { id, residents: list(shrine) }),
        Request::Summon(s) => summon(shrine, s).map(|resident| Reply::Summoned { id, resident }),
        Request::Banish { who } => {
            banish(shrine, &who).await.map(|message| Reply::Done { id, message })
        }
        Request::Close { who } => {
            close(shrine, &who).await.map(|message| Reply::Done { id, message })
        }
        Request::Hello { .. } | Request::Quit => Err("unexpected".into()),
    };
    r.unwrap_or_else(|error| Reply::Error { id, error })
}

fn info(e: &Entry) -> proto::Resident {
    let r = &e.rec;
    proto::Resident {
        id: r.id.clone(),
        name: r.name.clone(),
        slot: r.slot,
        cwd: r.cwd.clone(),
        pid: e.handle.as_ref().map(|h| h.pid),
        departed: r.departed,
        exit: r.exit,
        signal: r.signal,
    }
}

fn list(shrine: &Shared) -> Vec<proto::Resident> {
    shrine.borrow().entries.iter().map(info).collect()
}

/// By name (any case), slot or id.
fn find(shrine: &Shrine, who: &str) -> Option<usize> {
    let slot = who.parse::<u8>().ok();
    shrine.entries.iter().position(|e| {
        e.rec.name.eq_ignore_ascii_case(who)
            || e.rec.id == who
            || (slot.is_some() && e.rec.slot == slot)
    })
}

fn valid_name(n: &str) -> bool {
    // A letter first, so a name can never be mistaken for a slot.
    n.starts_with(|c: char| c.is_ascii_alphabetic())
        && n.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

fn summon(shrine: &Shared, s: Summon) -> Result<proto::Resident, String> {
    let mut sh = shrine.borrow_mut();
    if sh.quitting {
        return Err("the daemon is stopping".into());
    }
    let cwd = std::fs::canonicalize(&s.cwd)
        .ok()
        .filter(|p| p.is_dir())
        .ok_or_else(|| format!("no such directory: {}", s.cwd))?;
    if s.prompt.as_deref().is_some_and(|p| p.contains('\n')) {
        return Err("a prompt is a single line".into());
    }
    let share = sh.share.clone().ok_or("no share/ directory beside the binary")?;
    let name = match s.name {
        Some(n) if !valid_name(&n) => {
            return Err("a name starts with a letter and uses letters, digits, _ . - only".into());
        }
        Some(n) if taken(&sh, &n) => return Err(format!("{n} is already here")),
        Some(n) => n,
        None => pick_name(&sh, &share),
    };
    let path = std::env::var_os("PATH");
    let program = launch::claude(path.as_deref()).ok_or("claude not found on PATH")?;
    let slot = (1..=9).find(|n| sh.entries.iter().all(|e| e.rec.slot != Some(*n)));
    let id = store::uuid();
    let paths = launch::Paths { exe: &sh.exe, share: &share, socket: &sh.socket };
    let opts = launch::Options {
        id: &id,
        name: &name,
        model: s.model.as_deref(),
        effort: s.effort.as_deref(),
        mode: s.mode.as_deref(),
        prompt: s.prompt.as_deref(),
        resume: false,
    };
    let argv = launch::argv(&paths, &opts);
    let bin = sh.exe.parent().unwrap_or(Path::new("/")).to_path_buf();
    let env = launch::env(std::env::vars_os(), &id, &bin, &sh.socket);
    let (cols, rows) = SIZE;
    let spawn = pty::Spawn { program: &program, args: &argv, env: &env, cwd: &cwd, cols, rows };
    let handle = resident::start(spawn)
        .map_err(|e| format!("could not start {}: {e}", program.display()))?;
    let rec = Record {
        id: id.clone(),
        session: id.clone(),
        name,
        slot,
        cwd: cwd.to_string_lossy().into_owned(),
        program: program.to_string_lossy().into_owned(),
        argv: argv.iter().map(|a| a.to_string_lossy().into_owned()).collect(),
        prompt: s.prompt,
        ritual: None,
        launched: store::now(),
        departed: None,
        exit: None,
        signal: None,
    };
    if let Err(e) = sh.store.save(&rec) {
        log(json!({"ev": "record", "id": id, "error": e.to_string()}));
    }
    log(json!({"ev": "summoned", "id": id, "name": rec.name, "pid": handle.pid}));
    let handle = Rc::new(handle);
    tokio::task::spawn_local(watch_exit(shrine.clone(), id, handle.clone()));
    sh.entries.push(Entry { rec, handle: Some(handle) });
    Ok(info(sh.entries.last().expect("just pushed")))
}

fn taken(sh: &Shrine, n: &str) -> bool {
    sh.entries.iter().any(|e| e.rec.name.eq_ignore_ascii_case(n))
}

fn pick_name(sh: &Shrine, share: &Path) -> String {
    let names = std::fs::read_to_string(share.join("names.txt")).unwrap_or_default();
    let free: Vec<&str> =
        names.lines().map(str::trim).filter(|n| valid_name(n) && !taken(sh, n)).collect();
    match free.len() {
        0 => (1..).map(|n| format!("Resident{n}")).find(|n| !taken(sh, n)).expect("unbounded"),
        n => free[store::random(n)].to_string(),
    }
}

/// Departed now, with the status `wait()` gave if it gave one.
fn depart(r: &mut Record, x: Option<resident::Exit>) {
    r.departed = Some(store::now());
    r.exit = x.and_then(|x| x.code);
    r.signal = x.and_then(|x| x.signal);
}

/// The record follows the exit: departed, with the status `wait()` gave.
async fn watch_exit(shrine: Shared, id: String, handle: Rc<Handle>) {
    let exit = handle.exited().await;
    let mut sh = shrine.borrow_mut();
    let Some(e) = sh.entries.iter_mut().find(|e| e.rec.id == id) else { return };
    e.handle = None;
    depart(&mut e.rec, Some(exit));
    let rec = e.rec.clone();
    let _ = sh.store.save(&rec);
    log(
        json!({"ev": "departed", "id": id, "name": rec.name, "exit": exit.code, "signal": exit.signal}),
    );
}

fn live(shrine: &Shared, who: &str) -> Result<(String, Rc<Handle>), String> {
    let sh = shrine.borrow();
    let i = find(&sh, who).ok_or_else(|| format!("no resident {who}"))?;
    let e = &sh.entries[i];
    match &e.handle {
        Some(h) if h.exit().is_none() => Ok((e.rec.name.clone(), h.clone())),
        _ => Err(format!("{} has already departed", e.rec.name)),
    }
}

async fn banish(shrine: &Shared, who: &str) -> Result<String, String> {
    let (name, h) = live(shrine, who)?;
    let t = Instant::now();
    let (_, sweep) = pty::sweep(h.pid, BANISH_GRACE).await;
    let exit = tokio::time::timeout(Duration::from_secs(5), h.exited()).await;
    let ms = |e: &resident::Exit| e.at.saturating_duration_since(t).as_millis() as u64;
    log(json!({"ev": "banished", "name": name, "pid": h.pid,
               "hup_to_exit_ms": exit.as_ref().ok().map(ms),
               "stages": sweep.iter().map(|(s, p)| json!({"sig": s, "pids": p})).collect::<Vec<_>>()}));
    match exit {
        Ok(_) => Ok(format!("banished {name}")),
        Err(_) => Err(format!("{name} survived the sweep")),
    }
}

/// Ctrl-C (clears a half-typed prompt or interrupts the turn), a pause for that to land, then
/// `/exit` and Enter. True once the resident has left.
async fn ask_leave(h: &Handle, wait: Duration) -> bool {
    h.input(b"\x03").await;
    h.settle(Duration::from_millis(200), Duration::from_secs(2)).await;
    h.input(b"/exit").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.input(b"\r").await;
    tokio::time::timeout(wait, h.exited()).await.is_ok()
}

/// A keystroke that went missing would leave the resident sitting there, so the gesture is
/// repeated once when nothing comes of it.
async fn ask_twice(h: &Handle) -> bool {
    ask_leave(h, EXIT_WAIT).await || ask_leave(h, EXIT_WAIT).await
}

async fn close(shrine: &Shared, who: &str) -> Result<String, String> {
    let (name, h) = {
        let mut sh = shrine.borrow_mut();
        let i = find(&sh, who).ok_or_else(|| format!("no resident {who}"))?;
        match sh.entries[i].handle.clone() {
            Some(h) => (sh.entries[i].rec.name.clone(), h),
            None => {
                let e = sh.entries.remove(i);
                let _ = sh.store.retire(&e.rec);
                return Ok(format!("closed {}", e.rec.name));
            }
        }
    };
    // Bounded as a whole: a resident that stopped reading its tty blocks the keystrokes too.
    let most = 2 * (EXIT_WAIT + Duration::from_secs(3));
    if tokio::time::timeout(most, ask_twice(&h)).await.unwrap_or(false) {
        Ok(format!("{name} has left (/exit)"))
    } else {
        Err(format!("{name} did not answer /exit in {}s, twice", EXIT_WAIT.as_secs()))
    }
}

/// Everyone gets `/exit` at once; whoever is still here after `QUIT_WAIT` is banished. Then
/// every record leaves the shrine for `departed/`.
async fn leave_all(shrine: &Shared) {
    shrine.borrow_mut().quitting = true;
    let live: Vec<Rc<Handle>> =
        shrine.borrow().entries.iter().filter_map(|e| e.handle.clone()).collect();
    let deadline = tokio::time::Instant::now() + QUIT_WAIT;
    let asks: Vec<_> = live
        .iter()
        .map(|h| {
            let h = h.clone();
            tokio::task::spawn_local(async move {
                tokio::time::timeout_at(deadline, ask_twice(&h)).await
            })
        })
        .collect();
    for a in asks {
        let _ = a.await;
    }
    let sweeps: Vec<_> = live
        .iter()
        .filter(|h| h.exit().is_none())
        .map(|h| {
            log(json!({"ev": "quit-banish", "pid": h.pid}));
            let h = h.clone();
            tokio::task::spawn_local(async move {
                pty::sweep(h.pid, BANISH_GRACE).await;
                let _ = tokio::time::timeout(Duration::from_secs(2), h.exited()).await;
            })
        })
        .collect();
    for s in sweeps {
        let _ = s.await;
    }
    let mut sh = shrine.borrow_mut();
    for mut e in std::mem::take(&mut sh.entries) {
        if let Some(h) = &e.handle {
            depart(&mut e.rec, h.exit());
        }
        let _ = sh.store.retire(&e.rec);
    }
}
