//! The daemon process: one per state dir (a flock held for life), NDJSON over a unix socket,
//! and the shrine of residents, all on one thread.

use super::aware::Aware;
use super::launch;
use super::pty;
use super::registry::{self, Session};
use super::resident::{self, Handle};
use super::store::{self, Record, Store};
use crate::hooks;
use crate::proto::{self, Envelope, Hook, Reply, Request, State, Summon, Telemetry};
use crate::tele;
use crate::vt::{Frame, KeyEvent, Modes};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, broadcast, mpsc, oneshot, watch};
use tokio::task::AbortHandle;

/// From HUP to TERM: how long a leader gets to leave on its own. Claude Code on haiku took
/// 0.74-1.35 s over 20 banishes, idle and mid-turn; this is about twice the slowest.
pub const BANISH_GRACE: Duration = Duration::from_secs(3);
/// How long one resident gets to act on one `/exit`; one that runs out is asked once more.
const EXIT_WAIT: Duration = Duration::from_secs(6);
/// `quit` waits this long for everyone's `/exit`, then banishes whoever is left.
const QUIT_WAIT: Duration = Duration::from_secs(20);
/// Until a client says how big it is.
const SIZE: (u16, u16) = (80, 24);
/// A viewer gets at most one screen per this, about 60 a second.
const FRAME_GAP: Duration = Duration::from_millis(16);
/// How long a child's synchronized-output block may hold its screen back.
const SYNC_HOLD: Duration = Duration::from_millis(150);
/// How often the spool is replayed and the registry asked (a call costs about 0.12 s).
const POLL: Duration = Duration::from_secs(3);

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
    aware: Aware,
    tele: Option<Telemetry>,
}

impl Entry {
    fn new(rec: Record, handle: Rc<Handle>) -> Entry {
        Entry { rec, handle: Some(handle), aware: Aware::default(), tele: None }
    }
}

struct Shrine {
    entries: Vec<Entry>,
    store: Store,
    exe: PathBuf,
    share: Option<PathBuf>,
    socket: PathBuf,
    quitting: bool,
    /// Every resident's size: the last client's grid.
    size: (u16, u16),
    /// Bumped whenever the shrine changes, for `watch`.
    changed: watch::Sender<u64>,
    /// `notify` events, for every `watch`.
    notices: broadcast::Sender<Reply>,
    views: HashMap<u64, View>,
    conns: u64,
}

type Shared = Rc<RefCell<Shrine>>;
/// Per connection: the resident it views, and whether its terminal has focus.
type View = (Option<String>, bool);

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
        share: proto::share_dir(&exe),
        socket: std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()),
        exe,
        quitting: false,
        size: SIZE,
        changed: watch::channel(0).0,
        notices: broadcast::channel(16).0,
        views: HashMap::new(),
        conns: 0,
    }));
    log(
        json!({"ev": "started", "pid": std::process::id(), "socket": path, "ppid": unsafe { libc::getppid() }}),
    );
    // Hooks spooled while no daemon answered: a /clear before the last one stopped, say.
    replay(&shrine);
    tokio::task::spawn_local(poll(shrine.clone()));
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

/// One line for the writer, and a word back once it is written.
type Line = (Vec<u8>, Option<oneshot::Sender<()>>);
type Out = mpsc::Sender<Line>;

fn encode(r: &Reply) -> Vec<u8> {
    let mut b = serde_json::to_vec(r).expect("a reply serializes");
    b.push(b'\n');
    b
}

async fn send(out: &Out, r: &Reply) -> bool {
    out.send((encode(r), None)).await.is_ok()
}

/// Returns once the line is written, so a caller that waits on it never has two in flight.
async fn send_written(out: &Out, r: &Reply) -> bool {
    let (tx, rx) = oneshot::channel();
    out.send((encode(r), Some(tx))).await.is_ok() && rx.await.is_ok()
}

async fn write_lines(mut w: OwnedWriteHalf, mut rx: mpsc::Receiver<Line>) {
    while let Some((b, done)) = rx.recv().await {
        if w.write_all(&b).await.is_err() {
            return;
        }
        if let Some(d) = done {
            let _ = d.send(());
        }
    }
}

/// Requests are read in order. Banish, close and quit run on their own, so a long one holds up
/// neither the rest nor the screen; their replies come when they finish.
async fn conn(shrine: Shared, quit: Rc<Notify>, s: UnixStream) {
    if !same_user(&s) {
        return;
    }
    let me = {
        let mut sh = shrine.borrow_mut();
        sh.conns += 1;
        let me = sh.conns;
        sh.views.insert(me, (None, false));
        me
    };
    // A resident brought on screen in a focused terminal has been seen.
    let set_view = |f: &dyn Fn(&mut View)| {
        let mut sh = shrine.borrow_mut();
        let Some(v) = sh.views.get_mut(&me) else { return };
        f(v);
        let (Some(who), true) = v.clone() else { return };
        let at = hooks::now_ms();
        let e = sh.entries.iter_mut().find(|e| e.rec.id == who && e.handle.is_some());
        if e.is_some_and(|e| e.aware.seen(at)) {
            touch(&sh);
        }
    };
    let (r, w) = s.into_split();
    let (out, rx) = mpsc::channel(64);
    tokio::task::spawn_local(write_lines(w, rx));
    let mut lines = BufReader::new(r).lines();
    let mut greeted = false;
    let (mut watching, mut viewing): (Option<AbortHandle>, Option<AbortHandle>) = (None, None);
    let mut viewed = false;
    while let Ok(Some(line)) = lines.next_line().await {
        let Envelope { id, req } = match serde_json::from_str::<Envelope>(&line) {
            Ok(e) => e,
            Err(e) => {
                send(&out, &Reply::Error { id: 0, error: format!("bad request: {e}") }).await;
                match greeted {
                    true => continue,
                    false => break,
                }
            }
        };
        let fail = move |error: String| Reply::Error { id, error };
        let reply = match req {
            Request::Hello { proto, .. } => {
                greeted = proto == proto::PROTO;
                let reply = match greeted {
                    true => Reply::Welcome { proto: proto::PROTO, pid: std::process::id() },
                    false => fail(format!("protocol {proto}, want {}", proto::PROTO)),
                };
                send(&out, &reply).await;
                match greeted {
                    true => continue,
                    false => break,
                }
            }
            _ if !greeted => {
                send(&out, &fail("say hello first".into())).await;
                break;
            }
            Request::List { all } => Some(Reply::List { id, residents: list(&shrine, all) }),
            Request::Summon(s) => Some(
                summon(&shrine, s).map_or_else(fail, |resident| Reply::Summoned { id, resident }),
            ),
            Request::Recall { who } => Some(
                recall(&shrine, &who)
                    .map_or_else(fail, |resident| Reply::Summoned { id, resident }),
            ),
            Request::Banish { .. } | Request::Close { .. } => {
                let (shrine, out) = (shrine.clone(), out.clone());
                tokio::task::spawn_local(async move {
                    let r = match req {
                        Request::Banish { who } => banish(&shrine, &who).await,
                        Request::Close { who } => close(&shrine, &who).await,
                        _ => unreachable!("matched above"),
                    };
                    send(&out, &r.map_or_else(fail, |message| Reply::Done { id, message })).await;
                });
                None
            }
            Request::Quit => {
                let (shrine, out, quit) = (shrine.clone(), out.clone(), quit.clone());
                tokio::task::spawn_local(async move {
                    leave_all(&shrine).await;
                    let message = "the shrine is empty; the daemon stops".into();
                    send_written(&out, &Reply::Done { id, message }).await;
                    // Whether or not the asker is still there to hear it.
                    quit.notify_one();
                });
                None
            }
            Request::Watch => {
                if watching.is_none() {
                    let task = tokio::task::spawn_local(watch(shrine.clone(), out.clone()));
                    watching = Some(task.abort_handle());
                }
                None
            }
            Request::View { who } => match live(&shrine, &who) {
                Ok((rid, _, h)) => {
                    viewing.take().inspect(AbortHandle::abort);
                    let task = tokio::task::spawn_local(view(
                        shrine.clone(),
                        h,
                        rid.clone(),
                        out.clone(),
                        !viewed,
                    ));
                    viewing = Some(task.abort_handle());
                    viewed = true;
                    set_view(&|v| v.0 = Some(rid.clone()));
                    None
                }
                Err(e) => Some(fail(e)),
            },
            Request::Unview => {
                viewing.take().inspect(AbortHandle::abort);
                set_view(&|v| v.0 = None);
                None
            }
            Request::Focus { on } => {
                set_view(&|v| v.1 = on);
                None
            }
            Request::Hook { resident, hook: h } => {
                hook(&shrine, &resident, h);
                None
            }
            Request::Statusline { resident, telemetry } => {
                statusline(&shrine, &resident, telemetry);
                None
            }
            Request::Input { who, bytes, key } => {
                input(&shrine, &who, bytes, key).await.err().map(fail)
            }
            Request::Resize { cols, rows } => {
                resize(&shrine, cols, rows);
                None
            }
        };
        if let Some(r) = reply {
            send(&out, &r).await;
        }
    }
    for t in [watching, viewing].into_iter().flatten() {
        t.abort();
    }
    shrine.borrow_mut().views.remove(&me);
}

/// The peer runs as us. `getpeereid` alone: tokio's `peer_cred` also asks for the pid, which
/// fails once the peer has hung up, and a hook writes its line and hangs up at once.
fn same_user(s: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: getpeereid writes two ids into the locals; getuid cannot fail.
    unsafe { libc::getpeereid(s.as_raw_fd(), &mut uid, &mut gid) == 0 && uid == libc::getuid() }
}

/// A `residents` event now and whenever the shrine changes after, and every `notify`.
async fn watch(shrine: Shared, out: Out) {
    let (mut rx, mut notices) = {
        let sh = shrine.borrow();
        (sh.changed.subscribe(), sh.notices.subscribe())
    };
    loop {
        rx.borrow_and_update();
        if !send(&out, &Reply::Residents { residents: list(&shrine, false) }).await {
            return;
        }
        loop {
            tokio::select! {
                c = rx.changed() => match c {
                    Ok(()) => break,
                    Err(_) => return,
                },
                n = notices.recv() => match n {
                    Ok(r) => if !send(&out, &r).await { return },
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => return,
                },
            }
        }
    }
}

/// Streams one resident's screen: a whole frame, then the rows that changed. Each is computed
/// when the last has been written, against what this client was last sent, so a slow client
/// skips screens rather than queueing them.
async fn view(shrine: Shared, h: Rc<Handle>, who: String, out: Out, mut nudge: bool) {
    let mut changes = h.changes();
    let mut sent: Option<(Frame, Modes)> = None;
    let mut rev = 0;
    loop {
        // A child inside its own synchronized-output block is mid-draw.
        let held = Instant::now();
        while h.in_sync() && held.elapsed() < SYNC_HOLD {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        changes.borrow_and_update();
        let (frame, modes) = (h.frame(), h.modes());
        let reply = match &sent {
            Some((prev, m)) if prev.cols == frame.cols && prev.rows.len() == frame.rows.len() => {
                let rows = frame.damage(prev);
                (!rows.is_empty() || prev.cursor != frame.cursor || *m != modes).then(|| {
                    let rows = rows.into_iter().map(|y| (y as u16, frame.rows[y].clone()));
                    Reply::Damage {
                        who: who.clone(),
                        base: rev,
                        rev: rev + 1,
                        rows: rows.collect(),
                        cursor: frame.cursor,
                        modes,
                    }
                })
            }
            _ => Some(Reply::Frame { who: who.clone(), rev: rev + 1, frame: frame.clone(), modes }),
        };
        if let Some(r) = reply {
            if !send_written(&out, &r).await {
                return;
            }
            rev += 1;
            sent = Some((frame, modes));
        }
        // A client coming back gets the screen it missed, then a SIGWINCH, which makes Claude
        // Code draw everything again. On its own task: a view dropped midway must not leave the
        // resident a row short.
        let (cols, rows) = h.size();
        if std::mem::take(&mut nudge) && rows > 1 {
            let (shrine, h) = (shrine.clone(), h.clone());
            tokio::task::spawn_local(async move {
                h.resize(cols, rows - 1);
                tokio::time::sleep(Duration::from_millis(30)).await;
                let (cols, rows) = shrine.borrow().size;
                h.resize(cols, rows);
            });
        }
        tokio::time::sleep(FRAME_GAP).await;
        if changes.changed().await.is_err() {
            return;
        }
    }
}

async fn input(
    shrine: &Shared,
    who: &str,
    mut bytes: Vec<u8>,
    key: Option<proto::Key>,
) -> Result<(), String> {
    let (_, name, h) = live(shrine, who)?;
    if let Some(k) = key {
        let ev = KeyEvent::from_kitty(k.code, k.mods, k.event).ok_or("no such key")?;
        bytes.extend(h.encode(ev));
    }
    if !bytes.is_empty() && !h.input(&bytes).await {
        return Err(format!("{name} is not reading"));
    }
    Ok(())
}

fn resize(shrine: &Shared, cols: u16, rows: u16) {
    let mut sh = shrine.borrow_mut();
    sh.size = (cols.max(1), rows.max(1));
    for h in sh.entries.iter().filter_map(|e| e.handle.as_ref()) {
        h.resize(cols, rows);
    }
}

fn info(r: &Record, pid: Option<i32>) -> proto::Resident {
    proto::Resident {
        id: r.id.clone(),
        name: r.name.clone(),
        slot: r.slot,
        cwd: r.cwd.clone(),
        pid,
        departed: r.departed,
        exit: r.exit,
        signal: r.signal,
        state: if r.departed.is_some() { State::Departed } else { State::Resting },
        detail: None,
        mode: flag(&r.argv, "--permission-mode"),
        branch: None,
        telemetry: None,
    }
}

/// Everything known about one in the shrine: what it is doing, and its last report.
fn entry_info(e: &Entry) -> proto::Resident {
    let mut r = info(&e.rec, e.handle.as_ref().map(|h| h.pid));
    if e.handle.is_some() {
        (r.state, r.detail) = (e.aware.state(), e.aware.detail.clone());
    }
    r.mode = e.aware.mode.clone().or(r.mode);
    r.branch = tele::git_branch(Path::new(&e.rec.cwd));
    r.telemetry = e.tele.clone();
    r
}

/// A flag's value in a launch argv, before any `--`.
fn flag(argv: &[String], f: &str) -> Option<String> {
    argv.iter().take_while(|a| *a != "--").skip_while(|a| *a != f).nth(1).cloned()
}

/// The shrine in order; with `all`, then everyone in `departed/`, newest first and slotless.
fn list(shrine: &Shared, all: bool) -> Vec<proto::Resident> {
    let sh = shrine.borrow();
    let mut v: Vec<_> = sh.entries.iter().map(entry_info).collect();
    if all {
        let mut gone = sh.store.load_departed();
        gone.retain(|r| !sh.entries.iter().any(|e| e.rec.id == r.id));
        gone.sort_by_key(|r| std::cmp::Reverse(r.departed));
        v.extend(gone.iter().map(|r| proto::Resident { slot: None, ..info(r, None) }));
    }
    v
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

/// Starts `claude` for a resident: the argv, the environment and the PTY at the shrine's size,
/// with the record following its exit. Gives the program and argv for the record.
fn launch(
    shrine: &Shared,
    sh: &Shrine,
    o: &launch::Options,
    cwd: &Path,
) -> Result<(String, Vec<String>, Rc<Handle>), String> {
    let share = sh.share.clone().ok_or("no share/ directory beside the binary")?;
    let path = std::env::var_os("PATH");
    let program = launch::claude(path.as_deref()).ok_or("claude not found on PATH")?;
    let paths = launch::Paths { exe: &sh.exe, share: &share, socket: &sh.socket };
    let argv = launch::argv(&paths, o, cwd);
    let bin = sh.exe.parent().unwrap_or(Path::new("/")).to_path_buf();
    let env = launch::env(std::env::vars_os(), o.id, &bin, &sh.socket);
    let (cols, rows) = sh.size;
    let spawn = pty::Spawn { program: &program, args: &argv, env: &env, cwd, cols, rows };
    let handle = resident::start(spawn)
        .map_err(|e| format!("could not start {}: {e}", program.display()))?;
    let handle = Rc::new(handle);
    tokio::task::spawn_local(watch_exit(shrine.clone(), o.id.to_string(), handle.clone()));
    let argv = argv.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    Ok((program.to_string_lossy().into_owned(), argv, handle))
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
    let slot = (1..=9).find(|n| sh.entries.iter().all(|e| e.rec.slot != Some(*n)));
    let id = store::uuid();
    let opts = launch::Options {
        id: &id,
        session: &id,
        name: &name,
        model: s.model.as_deref(),
        effort: s.effort.as_deref(),
        mode: s.mode.as_deref(),
        prompt: s.prompt.as_deref(),
        resume: false,
    };
    let (program, argv, handle) = launch(shrine, &sh, &opts, &cwd)?;
    let rec = Record {
        id: id.clone(),
        session: id.clone(),
        session_at: 0,
        name,
        slot,
        cwd: cwd.to_string_lossy().into_owned(),
        program,
        argv,
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
    let r = info(&rec, Some(handle.pid));
    sh.entries.push(Entry::new(rec, handle));
    touch(&sh);
    Ok(r)
}

/// A departed resident back in the shrine: one of this run's, or the newest in `departed/`
/// by that name or id. Its session resumes with the flags it was summoned with.
fn recall(shrine: &Shared, who: &str) -> Result<proto::Resident, String> {
    let mut sh = shrine.borrow_mut();
    if sh.quitting {
        return Err("the daemon is stopping".into());
    }
    let (mut rec, at) = match find(&sh, who) {
        Some(i) if sh.entries[i].handle.is_some() => {
            return Err(format!("{} is still here", sh.entries[i].rec.name));
        }
        Some(i) => (sh.entries[i].rec.clone(), Some(i)),
        None => {
            let mut gone = sh.store.load_departed();
            gone.retain(|r| r.name.eq_ignore_ascii_case(who) || r.id == who);
            let r = gone.into_iter().max_by_key(|r| r.departed);
            let r = r.ok_or_else(|| format!("no resident {who}"))?;
            if taken(&sh, &r.name) {
                return Err(format!("{} is already here", r.name));
            }
            (r, None)
        }
    };
    let cwd = PathBuf::from(&rec.cwd);
    if !cwd.is_dir() {
        return Err(format!("{} is gone", rec.cwd));
    }
    let flag = |f: &str| flag(&rec.argv, f);
    let (model, effort, mode) = (flag("--model"), flag("--effort"), flag("--permission-mode"));
    // The session its hooks last named, which /clear moves on. One that never got a prompt has
    // nothing to resume: it starts afresh on that session, with the same name.
    let resume = launch::has_conversation(&rec.session);
    let opts = launch::Options {
        id: &rec.id,
        session: &rec.session,
        name: &rec.name,
        model: model.as_deref(),
        effort: effort.as_deref(),
        mode: mode.as_deref(),
        prompt: None,
        resume,
    };
    let (program, argv, handle) = launch(shrine, &sh, &opts, &cwd)?;
    let free =
        |n: u8| sh.entries.iter().enumerate().all(|(i, e)| Some(i) == at || e.rec.slot != Some(n));
    rec.slot = rec.slot.filter(|&n| free(n)).or_else(|| (1..=9).find(|&n| free(n)));
    (rec.program, rec.argv) = (program, argv);
    (rec.launched, rec.departed, rec.exit, rec.signal) = (store::now(), None, None, None);
    if let Err(e) = sh.store.restore(&rec) {
        log(json!({"ev": "record", "id": rec.id, "error": e.to_string()}));
    }
    log(
        json!({"ev": "recalled", "id": rec.id, "name": rec.name, "pid": handle.pid, "resumed": resume}),
    );
    let r = info(&rec, Some(handle.pid));
    let entry = Entry::new(rec, handle);
    match at {
        Some(i) => sh.entries[i] = entry,
        None => sh.entries.push(entry),
    }
    touch(&sh);
    Ok(r)
}

/// One hook from inside a resident. A SessionStart moves its record on to the new session,
/// even once it has departed: a /clear the daemon hears of only from the spool.
fn hook(shrine: &Shared, resident: &str, h: Hook) {
    let mut sh = shrine.borrow_mut();
    let sh = &mut *sh;
    let at = sh.entries.iter().position(|e| e.rec.id == resident);
    if let (true, Some(s)) = (h.event == "SessionStart", h.session.as_deref()) {
        rotate(sh, at, resident, s, h.at);
    }
    let Some(i) = at.filter(|&i| sh.entries[i].handle.is_some()) else { return };
    let before = sh.entries[i].aware.state();
    if sh.entries[i].aware.hook(&h) {
        let state = sh.entries[i].aware.state();
        log(
            json!({"ev": "hook", "id": resident, "event": h.event, "kind": h.kind, "state": state}),
        );
        after(sh, i, before);
    }
}

fn rotate(sh: &mut Shrine, at: Option<usize>, resident: &str, session: &str, when: i64) {
    let fresh = |r: &Record| when >= r.session_at && r.session != session;
    let saved = match at {
        Some(i) => {
            let r = &mut sh.entries[i].rec;
            if !fresh(r) {
                return;
            }
            (r.session, r.session_at) = (session.into(), when);
            let r = r.clone();
            sh.store.save(&r)
        }
        None => match sh.store.load_departed_id(resident).filter(|r| fresh(r)) {
            Some(mut r) => {
                (r.session, r.session_at) = (session.into(), when);
                sh.store.save_departed(&r)
            }
            None => return,
        },
    };
    log(
        json!({"ev": "session", "id": resident, "session": session, "error": saved.err().map(|e| e.to_string())}),
    );
}

fn statusline(shrine: &Shared, resident: &str, mut t: Telemetry) {
    let mut sh = shrine.borrow_mut();
    let Some(e) = sh.entries.iter_mut().find(|e| e.rec.id == resident && e.handle.is_some()) else {
        return;
    };
    t.at = store::now();
    e.tele = Some(t);
    touch(&sh);
}

/// Watchers hear of the change, and of a resident that has just come to need the user. A
/// turn that finishes while someone watches it is seen at once.
fn after(sh: &mut Shrine, i: usize, before: State) {
    let id = &sh.entries[i].rec.id;
    let watched = sh.views.values().any(|(v, f)| *f && v.as_ref() == Some(id));
    if watched {
        sh.entries[i].aware.seen(hooks::now_ms());
    }
    let e = &sh.entries[i];
    let now = e.aware.state();
    if now.needs_you() && now != before {
        log(json!({"ev": "notify", "id": e.rec.id, "state": now, "watched": watched}));
        let (who, name, text) = (e.rec.id.clone(), e.rec.name.clone(), e.aware.notice(&e.rec.name));
        let _ = sh.notices.send(Reply::Notify { who, name, state: now, text, watched });
    }
    touch(sh);
}

fn replay(shrine: &Shared) {
    let root = shrine.borrow().store.root.clone();
    for (who, h) in hooks::take_spool(&root) {
        hook(shrine, &who, h);
    }
}

/// Every `POLL`: the spool, then the registry while anyone is here.
async fn poll(shrine: Shared) {
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
        if let Some(list) = registry::fetch(&claude, &env).await {
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

/// Tells every `watch`er that the shrine changed.
fn touch(sh: &Shrine) {
    sh.changed.send_modify(|v| *v += 1);
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
    touch(&sh);
    log(
        json!({"ev": "departed", "id": id, "name": rec.name, "exit": exit.code, "signal": exit.signal}),
    );
}

/// Id, name and handle of a resident still running.
fn live(shrine: &Shared, who: &str) -> Result<(String, String, Rc<Handle>), String> {
    let sh = shrine.borrow();
    let i = find(&sh, who).ok_or_else(|| format!("no resident {who}"))?;
    let e = &sh.entries[i];
    match &e.handle {
        Some(h) if h.exit().is_none() => Ok((e.rec.id.clone(), e.rec.name.clone(), h.clone())),
        _ => Err(format!("{} has already departed", e.rec.name)),
    }
}

async fn banish(shrine: &Shared, who: &str) -> Result<String, String> {
    let (_, name, h) = live(shrine, who)?;
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
/// `/exit` and Enter. True once the resident has left. Plain bytes rather than encoded keys:
/// Claude Code takes them in kitty mode too.
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
                touch(&sh);
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
    touch(&sh);
}
