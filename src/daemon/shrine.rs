//! The shrine: every resident's record, PTY handle and state, and what is done to them:
//! summon, recall, banish, close, quit.

use super::aware::Aware;
use super::launch;
use super::pty;
use super::resident::{self, Handle};
use super::server::log;
use super::store::{self, Record, Store};
use crate::proto::{self, Reply, State, Summon, Telemetry};
use crate::tele;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};

/// From HUP to TERM: how long a leader gets to leave on its own. Claude Code on haiku took
/// 0.74-1.35 s over 20 banishes, idle and mid-turn; this is about twice the slowest.
const BANISH_GRACE: Duration = Duration::from_secs(3);

/// How long one resident gets to act on one `/exit`; one that runs out is asked once more.
const EXIT_WAIT: Duration = Duration::from_secs(6);

/// `quit` waits this long for everyone's `/exit`, then banishes whoever is left.
const QUIT_WAIT: Duration = Duration::from_secs(20);

/// Until a client says how big it is.
pub(super) const SIZE: (u16, u16) = (80, 24);

pub(super) struct Entry {
    pub(super) rec: Record,
    pub(super) handle: Option<Rc<Handle>>,
    pub(super) aware: Aware,
    pub(super) tele: Option<Telemetry>,
}

impl Entry {
    fn new(rec: Record, handle: Rc<Handle>) -> Entry {
        Entry { rec, handle: Some(handle), aware: Aware::default(), tele: None }
    }
}

pub(super) struct Shrine {
    pub(super) entries: Vec<Entry>,
    pub(super) store: Store,
    pub(super) exe: PathBuf,
    pub(super) share: Option<PathBuf>,
    pub(super) socket: PathBuf,
    pub(super) quitting: bool,
    /// Every resident's size: the last client's grid.
    pub(super) size: (u16, u16),
    /// Bumped whenever the shrine changes, for `watch`.
    pub(super) changed: watch::Sender<u64>,
    /// `notify` events, for every `watch`.
    pub(super) notices: broadcast::Sender<Reply>,
    pub(super) views: HashMap<u64, View>,
    pub(super) conns: u64,
}

impl Shrine {
    /// `claude` on PATH, and the environment it runs in for resident `id` (none: the registry).
    pub(super) fn claude(&self, id: &str) -> Option<(PathBuf, Vec<(OsString, OsString)>)> {
        let program = launch::claude(std::env::var_os("PATH").as_deref())?;
        let bin = self.exe.parent().unwrap_or(Path::new("/")).to_path_buf();
        Some((program, launch::env(std::env::vars_os(), id, &bin, &self.socket)))
    }
}

pub(super) type Shared = Rc<RefCell<Shrine>>;

/// Per connection: the resident it views, and whether its terminal has focus.
pub(super) type View = (Option<String>, bool);

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
pub(super) fn list(shrine: &Shared, all: bool) -> Vec<proto::Resident> {
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

pub(super) fn valid_name(n: &str) -> bool {
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
    let (program, env) = sh.claude(o.id).ok_or("claude not found on PATH")?;
    let paths = launch::Paths { exe: &sh.exe, share: &share, socket: &sh.socket };
    let argv = launch::argv(&paths, o, cwd);
    let (cols, rows) = sh.size;
    let spawn = pty::Spawn { program: &program, args: &argv, env: &env, cwd, cols, rows };
    let handle = resident::start(spawn)
        .map_err(|e| format!("could not start {}: {e}", program.display()))?;
    let handle = Rc::new(handle);
    tokio::task::spawn_local(watch_exit(shrine.clone(), o.id.to_string(), handle.clone()));
    let argv = argv.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    Ok((program.to_string_lossy().into_owned(), argv, handle))
}

pub(super) fn summon(shrine: &Shared, s: Summon) -> Result<proto::Resident, String> {
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
pub(super) fn recall(shrine: &Shared, who: &str) -> Result<proto::Resident, String> {
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

/// Tells every `watch`er that the shrine changed.
pub(super) fn touch(sh: &Shrine) {
    sh.changed.send_modify(|v| *v += 1);
}

pub(super) fn taken(sh: &Shrine, n: &str) -> bool {
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
    // Its screen is gone: a recall under the same id is not on anyone's screen until viewed.
    for v in sh.views.values_mut().filter(|v| v.0.as_deref() == Some(id.as_str())) {
        v.0 = None;
    }
    touch(&sh);
    log(
        json!({"ev": "departed", "id": id, "name": rec.name, "exit": exit.code, "signal": exit.signal}),
    );
}

/// Id, name and handle of a resident still running.
pub(super) fn live(shrine: &Shared, who: &str) -> Result<(String, String, Rc<Handle>), String> {
    let sh = shrine.borrow();
    let i = find(&sh, who).ok_or_else(|| format!("no resident {who}"))?;
    let e = &sh.entries[i];
    match &e.handle {
        Some(h) if h.exit().is_none() => Ok((e.rec.id.clone(), e.rec.name.clone(), h.clone())),
        _ => Err(format!("{} has already departed", e.rec.name)),
    }
}

pub(super) async fn banish(shrine: &Shared, who: &str) -> Result<String, String> {
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

pub(super) async fn close(shrine: &Shared, who: &str) -> Result<String, String> {
    let (name, h) = {
        let mut sh = shrine.borrow_mut();
        if sh.quitting {
            return Err("the daemon is stopping".into());
        }
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
pub(super) async fn leave_all(shrine: &Shared) {
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
