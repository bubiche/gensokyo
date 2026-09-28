//! PTY supervision on pty-process: a serialised spawner with a signal-reset `pre_exec`, and a
//! HUP -> TERM -> KILL sweep over a resident's session plus its ppid tree.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub use pty_process::{OwnedReadPty, OwnedWritePty, Pty, Size};

/// open + spawn run one at a time: macOS cannot open the master close-on-exec in one call, so a
/// fork between `posix_openpt` and `fcntl(FD_CLOEXEC)` would leak it into another child. Any
/// other fork in the daemon has to hold it too.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());

const RESET: [libc::c_int; 10] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGTSTP,
    libc::SIGTTIN,
    libc::SIGTTOU,
    libc::SIGCHLD,
    libc::SIGPIPE,
    libc::SIGALRM,
];

/// Runs in the child between fork and exec: async-signal-safe calls only, no allocation.
fn reset_signals() -> std::io::Result<()> {
    // SAFETY: signal, sigemptyset and sigprocmask are async-signal-safe and touch only this
    // process, which is about to exec.
    unsafe {
        for s in RESET {
            libc::signal(s, libc::SIG_DFL);
        }
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
    }
    Ok(())
}

pub struct Spawn<'a> {
    pub program: &'a Path,
    pub args: &'a [OsString],
    /// The whole environment: nothing is inherited.
    pub env: &'a [(OsString, OsString)],
    pub cwd: &'a Path,
    pub cols: u16,
    pub rows: u16,
}

/// Runs `f`, which forks, under the spawn lock.
pub fn locked<T>(f: impl FnOnce() -> T) -> T {
    let _g = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

pub fn spawn(s: Spawn) -> pty_process::Result<(Pty, tokio::process::Child)> {
    let _g = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (pty, pts) = pty_process::open()?;
    pty.resize(Size::new(s.rows.max(1), s.cols.max(1)))?;
    let cmd = pty_process::Command::new(s.program)
        .args(s.args)
        .env_clear()
        .envs(s.env.iter().map(|(k, v)| (k, v)))
        .current_dir(s.cwd);
    // SAFETY: reset_signals only makes async-signal-safe calls.
    let cmd = unsafe { cmd.pre_exec(reset_signals) };
    let child = cmd.spawn(pts)?;
    Ok((pty, child))
}

struct Proc {
    pid: i32,
    ppid: i32,
    sid: i32,
}

fn processes() -> Vec<Proc> {
    // SAFETY: a null buffer asks only for the count.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) }.max(0) as usize;
    // Room for the processes started since the count was taken.
    let mut pids = vec![0i32; count + 256];
    let bytes = (pids.len() * 4) as libc::c_int;
    // SAFETY: the buffer is `bytes` long; the call writes at most that many.
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    pids.truncate(n.max(0) as usize);
    pids.into_iter()
        .filter(|&p| p > 0)
        .filter_map(|pid| {
            let info = bsdinfo(pid)?;
            // SAFETY: plain getsid(2).
            let sid = unsafe { libc::getsid(pid) };
            (sid >= 0).then_some(Proc { pid, ppid: info.pbi_ppid as i32, sid })
        })
        .collect()
}

fn bsdinfo(pid: i32) -> Option<libc::proc_bsdinfo> {
    // SAFETY: proc_bsdinfo is plain data; proc_pidinfo writes at most `size` bytes.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let got =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (got == size).then_some(info)
}

/// When `pid` started, in microseconds: with the pid, what names one process for good.
pub fn start_id(pid: i32) -> Option<u64> {
    bsdinfo(pid).map(|i| i.pbi_start_tvsec * 1_000_000 + i.pbi_start_tvusec)
}

/// Every live pid in the leader's session, plus every descendant of the leader by ppid (which
/// catches children that called setsid while their parent is still alive).
pub fn snapshot(leader: i32) -> BTreeSet<i32> {
    let all = processes();
    let mut set: BTreeSet<i32> = all.iter().filter(|p| p.sid == leader).map(|p| p.pid).collect();
    set.insert(leader);
    loop {
        let before = set.len();
        for p in &all {
            if set.contains(&p.ppid) {
                set.insert(p.pid);
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the pid exists and may be signalled.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// What each stage signalled: (signal, pids alive when it was sent).
pub type SweepLog = Vec<(i32, Vec<i32>)>;

/// HUP, then up to `grace` for the leader to leave, TERM, up to 250 ms, KILL. The set is
/// cumulative: taken before the first signal and re-unioned at every stage, because once the
/// leader dies its children reparent to launchd and a fresh ppid walk no longer reaches them.
/// Descendants that outlive the leader's exit get the TERM without waiting out the grace.
pub async fn sweep(leader: i32, grace: Duration) -> (BTreeSet<i32>, SweepLog) {
    let mut set = BTreeSet::new();
    let mut log = Vec::new();
    let stages = [
        (libc::SIGHUP, grace),
        (libc::SIGTERM, Duration::from_millis(250)),
        (libc::SIGKILL, Duration::ZERO),
    ];
    for (sig, pause) in stages {
        set.extend(snapshot(leader));
        let live: Vec<i32> = set.iter().copied().filter(|&p| !gone(p)).collect();
        if live.is_empty() {
            break;
        }
        for &p in &live {
            // SAFETY: plain kill(2) on pids taken from this resident's tree.
            unsafe { libc::kill(p, sig) };
        }
        log.push((sig, live));
        let until = Instant::now() + pause;
        let waiting = |s: &BTreeSet<i32>| {
            if sig == libc::SIGHUP { !gone(leader) } else { s.iter().any(|&p| !gone(p)) }
        };
        while Instant::now() < until && waiting(&set) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    (set, log)
}

/// Dead, or a zombie waiting to be reaped: either way it will run no more. kill(0) still
/// succeeds on a zombie, and proc_pidinfo fails on one (ESRCH), so a live pid it cannot
/// describe is a zombie.
pub fn gone(pid: i32) -> bool {
    !alive(pid) || bsdinfo(pid).is_none()
}
