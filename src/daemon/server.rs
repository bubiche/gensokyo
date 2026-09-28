//! The daemon process: one per state dir (a flock held for life), NDJSON over a unix socket,
//! and each connection's requests and screen stream, all on one thread. The residents
//! themselves are the shrine's (`shrine.rs`).

use super::cards;
use super::ingest::{hook, replay, statusline};
use super::log::log;
use super::notify::looked;
use super::registry::poll;
use super::resident::Handle;
use super::rituals;
use super::shrine::{
    SIZE, Shared, Shrine, View, banish, close, leave_all, list, live, recall, summon,
};
use super::store::{self, Store};
use crate::paths;
use crate::proto::{self, Envelope, Reply, Request};
use crate::vt::{Frame, KeyEvent, Modes};
use serde_json::json;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, broadcast, mpsc, oneshot, watch};
use tokio::task::AbortHandle;

/// A viewer gets at most one screen per this, about 60 a second.
const FRAME_GAP: Duration = Duration::from_millis(16);

/// How long a child's synchronized-output block may hold its screen back.
const SYNC_HOLD: Duration = Duration::from_millis(150);

/// How long an input may wait for room in a resident's queue: one that has stopped reading its
/// tty would otherwise hold up every request after it on the connection.
const INPUT_WAIT: Duration = Duration::from_millis(500);

pub fn main() -> std::process::ExitCode {
    // Absolute: the daemon moves to / below, and residents inherit it with their own cwd.
    let root = std::path::absolute(paths::state_dir()).unwrap_or_else(|_| paths::state_dir());
    // SAFETY: one thread still; the runtime starts below.
    unsafe { std::env::set_var("GENSOKYO_STATE_DIR", &root) };
    let run = root.join("run");
    for d in [&run, &root.join("residents"), &root.join("departed")] {
        if let Err(e) = std::fs::create_dir_all(d) {
            eprintln!("gensokyo daemon: {}: {e}", d.display());
            return std::process::ExitCode::FAILURE;
        }
    }
    // Records hold prompts and argv: the user's alone, like the spool.
    for d in [&root, &run] {
        let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700));
    }
    super::log::open(&root.join("daemon.log"));
    let lock =
        OpenOptions::new().create(true).truncate(false).write(true).open(run.join("daemon.lock"));
    let lock = match lock {
        Ok(l) => l,
        Err(e) => {
            log(json!({"ev": "exit", "why": format!("run/daemon.lock: {e}")}));
            return std::process::ExitCode::FAILURE;
        }
    };
    // The lock comes before the socket: whoever holds it owns the socket path, and a socket file
    // already there is stale. Losing it is not an error (launchd's KeepAlive would retry).
    if lock.try_lock().is_err() {
        log(json!({"ev": "exit", "why": "another daemon holds run/daemon.lock"}));
        return std::process::ExitCode::SUCCESS;
    }
    let _ = std::env::set_current_dir("/");
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            log(json!({"ev": "exit", "why": format!("runtime: {e}")}));
            return std::process::ExitCode::FAILURE;
        }
    };
    let code = tokio::task::LocalSet::new().block_on(&rt, serve(Store::new(root)));
    drop(lock);
    code
}

async fn serve(store: Store) -> std::process::ExitCode {
    let path = paths::socket_path();
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
        share: paths::share_dir(&exe),
        socket: std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()),
        exe,
        quitting: false,
        size: SIZE,
        changed: watch::channel(0).0,
        notices: broadcast::channel(16).0,
        views: HashMap::new(),
        conns: 0,
        rites: Default::default(),
        refused: HashSet::new(),
    }));
    log(
        json!({"ev": "started", "pid": std::process::id(), "socket": path, "ppid": unsafe { libc::getppid() }}),
    );
    // Hooks spooled while no daemon answered: a /clear before the last one stopped, say. Before
    // the first request, so a resume that started this daemon resumes the session it moved to.
    replay(&shrine, 0);
    comeback(&shrine);
    tokio::task::spawn_local(supervise("registry poll", shrine.clone(), poll));
    tokio::task::spawn_local(supervise("ritual clock", shrine.clone(), rituals::clock));
    let quit = Rc::new(Stop { quit: Notify::new(), left: watch::channel(false).0 });
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
        signal(SignalKind::hangup()),
    ) else {
        log(json!({"ev": "exit", "why": "could not take its signals"}));
        return std::process::ExitCode::FAILURE;
    };
    loop {
        tokio::select! {
            a = listener.accept() => match a {
                Ok((s, _)) => { tokio::task::spawn_local(conn(shrine.clone(), quit.clone(), s)); }
                // Out of fds, say: not a spin.
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            },
            _ = quit.quit.notified() => break,
            // As `quit` does: hooks and the CLI are still heard while everyone leaves.
            _ = term.recv() => { stop(&shrine, &quit); }
            _ = int.recv() => { stop(&shrine, &quit); }
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

/// Whoever `gensokyo restart` found in the last daemon, recalled in its order: before the
/// clock's first tick, which would otherwise start a missed ritual run in a kept session's place.
/// The file goes first, so a recall that brings the daemon down is not tried again.
fn comeback(shrine: &Shared) {
    let path = paths::comeback_path();
    let Ok(ids) = std::fs::read_to_string(&path) else { return };
    let _ = std::fs::remove_file(&path);
    for id in ids.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match recall(shrine, id) {
            Ok(r) => log(json!({"ev": "comeback", "id": id, "name": r.name})),
            Err(e) => log(json!({"ev": "comeback", "id": id, "error": e})),
        }
    }
}

/// A task the daemon cannot do without (the registry poll, the ritual clock), started again
/// when it panics: tokio would otherwise end it for good while everything else carried on. One
/// that keeps panicking stays down, and the log says so.
async fn supervise<F, T>(what: &'static str, shrine: Shared, task: F)
where
    F: Fn(Shared) -> T,
    T: Future<Output = ()> + 'static,
{
    let mut quick = 0;
    loop {
        let t = Instant::now();
        match tokio::task::spawn_local(task(shrine.clone())).await {
            Err(e) if e.is_panic() => {
                quick = if t.elapsed() < Duration::from_secs(60) { quick + 1 } else { 1 };
                let again = quick <= 3;
                log(json!({"ev": "task", "task": what, "panicked": true, "restarted": again}));
                if !again {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            _ => return,
        }
    }
}

/// How the daemon stops: everyone asked to leave once, however many ask (SIGTERM, SIGINT,
/// each `quit`), then the loop ends once each `quit` has had its answer.
struct Stop {
    quit: Notify,
    /// True once the shrine is empty. Each `quit` holds a receiver until its answer is written.
    left: watch::Sender<bool>,
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

/// Requests are read in order. Banish, close, cast and quit run on their own, so a long one holds up
/// neither the rest nor the screen; their replies come when they finish.
async fn conn(shrine: Shared, quit: Rc<Stop>, s: UnixStream) {
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
    let set_view = |f: &dyn Fn(&mut View)| {
        let mut sh = shrine.borrow_mut();
        if let Some(v) = sh.views.get_mut(&me) {
            f(v);
            looked(&mut sh, me);
        }
    };
    let (r, w) = s.into_split();
    let (out, rx) = mpsc::channel(64);
    tokio::task::spawn_local(write_lines(w, rx));
    let mut lines = BufReader::new(r).lines();
    let mut greeted = false;
    // Greeted in another protocol by a hook: a binary updated under a running resident still
    // reports, and Hook and Statusline are all it may send.
    let mut hooks_only = false;
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
            Request::Hello { proto, who } => {
                greeted = proto == proto::PROTO;
                hooks_only = !greeted && who == "hook";
                if !greeted {
                    refused(&mut shrine.borrow_mut(), &who, proto, hooks_only);
                }
                let reply = match greeted || hooks_only {
                    true => Reply::Welcome { proto: proto::PROTO, pid: std::process::id() },
                    false => fail(format!("protocol {proto}, want {}", proto::PROTO)),
                };
                send(&out, &reply).await;
                match greeted || hooks_only {
                    true => continue,
                    false => break,
                }
            }
            ref r
                if !greeted
                    && !(hooks_only
                        && matches!(r, Request::Hook { .. } | Request::Statusline { .. })) =>
            {
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
            Request::Rituals => Some(rituals::listing(&shrine, id)),
            Request::Ritual { verb, name } => Some(
                rituals::verb(&shrine, verb, &name)
                    .map_or_else(fail, |message| Reply::Done { id, message }),
            ),
            Request::Banish { .. } | Request::Close { .. } | Request::Cast(_) => {
                let (shrine, out) = (shrine.clone(), out.clone());
                tokio::task::spawn_local(async move {
                    let r = match req {
                        Request::Banish { who } => banish(&shrine, &who).await,
                        Request::Close { who } => close(&shrine, &who).await,
                        Request::Cast(c) => cards::cast(&shrine, c).await,
                        _ => unreachable!("matched above"),
                    };
                    send(&out, &r.map_or_else(fail, |message| Reply::Done { id, message })).await;
                });
                None
            }
            Request::Quit => {
                let (mut left, out) = (stop(&shrine, &quit), out.clone());
                tokio::task::spawn_local(async move {
                    let _ = left.wait_for(|l| *l).await;
                    let message = "the shrine is empty; the daemon stops".into();
                    send_written(&out, &Reply::Done { id, message }).await;
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
            Request::Cards => {
                let (cards, unusable) = cards::cards(&shrine.borrow());
                Some(Reply::Cards { id, cards, unusable })
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

/// A `residents` event now and whenever the shrine changes after, the timetable now and
/// whenever it changes, and every `notify` and `notice`.
async fn watch(shrine: Shared, out: Out) {
    let (mut rx, mut notices) = {
        let sh = shrine.borrow();
        (sh.changed.subscribe(), sh.notices.subscribe())
    };
    if !send(&out, &rituals::listing(&shrine, 0)).await {
        return;
    }
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
    if bytes.is_empty() {
        return Ok(());
    }
    match tokio::time::timeout(INPUT_WAIT, h.input(&bytes)).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("{name} has already departed")),
        Err(_) => Err(format!("{name} is not reading its input")),
    }
}

/// Logged once per asker and protocol: a hook from an updated binary says hello on every event.
fn refused(sh: &mut Shrine, who: &str, proto: u32, hooks_only: bool) {
    let who: String = who.chars().take(40).collect();
    if sh.refused.insert((who.clone(), proto)) {
        let took = if hooks_only { "its hooks and status lines" } else { "nothing" };
        log(
            json!({"ev": "refused", "who": who, "proto": proto, "want": proto::PROTO, "took": took}),
        );
    }
}

/// Starts everyone leaving, once; gives what turns true when the shrine is empty.
fn stop(shrine: &Shared, s: &Rc<Stop>) -> watch::Receiver<bool> {
    let left = s.left.subscribe();
    if !std::mem::replace(&mut shrine.borrow_mut().quitting, true) {
        log(json!({"ev": "stopping"}));
        let (shrine, s) = (shrine.clone(), s.clone());
        tokio::task::spawn_local(async move {
            leave_all(&shrine).await;
            s.left.send_replace(true);
            // Each `quit` gets its answer written first, whether or not its asker is still there.
            let _ = tokio::time::timeout(Duration::from_secs(2), s.left.closed()).await;
            s.quit.notify_one();
        });
    }
    left
}

/// Bounded: an emulator is allocated at this size for every resident, and every summon after.
fn resize(shrine: &Shared, cols: u16, rows: u16) {
    let mut sh = shrine.borrow_mut();
    let (cols, rows) = (cols.clamp(1, 1000), rows.clamp(1, 500));
    sh.size = (cols, rows);
    for h in sh.entries.iter().filter_map(|e| e.handle.as_ref()) {
        h.resize(cols, rows);
    }
}
