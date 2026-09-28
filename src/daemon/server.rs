//! The daemon process: one per state dir (a flock held for life), NDJSON over a unix socket,
//! and each connection's requests and screen stream, all on one thread. The residents
//! themselves are the shrine's (`shrine.rs`).

use super::cards;
use super::ingest::{hook, replay, statusline};
use super::log::log;
use super::notify::looked;
use super::registry::poll;
use super::rituals;
use super::shrine::{
    SIZE, Shared, Shrine, View, banish, close, leave_all, list, live, recall, summon,
};
use super::store::{self, Store};
use super::stream::{self, send, send_written, view, writer};
use crate::paths;
use crate::proto::{self, Envelope, Reply, Request};
use crate::vt::KeyEvent;
use serde_json::json;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, broadcast, watch};
use tokio::task::AbortHandle;

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
        typed: false,
    }));
    log(
        json!({"ev": "started", "pid": std::process::id(), "socket": path, "ppid": unsafe { libc::getppid() }}),
    );
    // Hooks spooled while no daemon answered: a /clear before the last one stopped, say. Before
    // the first request, so a resume that started this daemon resumes the session it moved to.
    replay(&shrine, 0);
    comeback(&shrine);
    super::headless::adopt(&shrine);
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
    let out = writer(w);
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
                    let task = tokio::task::spawn_local(stream::watch(shrine.clone(), out.clone()));
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
            Request::Scroll { .. } => Some(fail("no scrollback in this daemon".into())),
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
    shrine.borrow_mut().typed = true;
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
            // On a task of its own: one that panics still ends in the daemon stopping, rather
            // than in a daemon that refuses everything as it stops and never does.
            let everyone = shrine.clone();
            if let Err(e) =
                tokio::task::spawn_local(async move { leave_all(&everyone).await }).await
            {
                log(json!({"ev": "stopping", "error": e.to_string()}));
            }
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
