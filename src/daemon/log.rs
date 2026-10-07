//! `daemon.log`: one JSON line per event, stderr and panics included, whoever writes them. Past
//! `MOST` it moves to `daemon.log.1`, over the one before, and a new log starts.

use super::store;
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The log's size, in bytes, past which it moves aside.
const MOST: u64 = 5 << 20;

/// `MOST`, or `$GENSOKYO_LOG_BYTES` for tests, read once.
fn most() -> u64 {
    static MOST_NOW: OnceLock<u64> = OnceLock::new();
    *MOST_NOW.get_or_init(|| {
        std::env::var("GENSOKYO_LOG_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(MOST)
    })
}

struct Log {
    file: File,
    path: PathBuf,
    /// Its length when opened and every write since: no stat per line.
    size: u64,
}

static LOG: Mutex<Option<Log>> = Mutex::new(None);

/// The log from now on: appended to, with stderr and panics going there too.
pub fn open(path: &Path) {
    let log = start(path, None);
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = log;
    std::panic::set_hook(Box::new(panic_hook));
}

/// `path` opened to append, with stderr onto it, and stdout too when stdout was `was` (launchd
/// points both at the log).
fn start(path: &Path, was: Option<&File>) -> Option<Log> {
    let file = OpenOptions::new().create(true).append(true).open(path).ok()?;
    // Whatever else reaches stderr lands in the log too, however the daemon was started.
    // SAFETY: dup2 onto fd 2, which this process owns; the file stays open in `LOG`.
    unsafe { libc::dup2(file.as_raw_fd(), 2) };
    if was.is_some_and(|w| same(w.as_raw_fd(), 1)) {
        // SAFETY: as above, onto fd 1.
        unsafe { libc::dup2(file.as_raw_fd(), 1) };
    }
    let size = file.metadata().map_or(0, |m| m.len());
    Some(Log { file, path: path.into(), size })
}

/// Whether fds `a` and `b` are the one file.
fn same(a: i32, b: i32) -> bool {
    // SAFETY: fstat fills the struct it is given, and both are zeroed first.
    let (mut x, mut y) = unsafe { (std::mem::zeroed::<libc::stat>(), std::mem::zeroed()) };
    let both = unsafe { libc::fstat(a, &mut x) == 0 && libc::fstat(b, &mut y) == 0 };
    both && (x.st_dev, x.st_ino) == (y.st_dev, y.st_ino)
}

/// One JSON line in `daemon.log`.
pub fn log(v: Value) {
    write_log(&mut LOG.lock().unwrap_or_else(|e| e.into_inner()), v);
}

fn write_log(log: &mut Option<Log>, mut v: Value) {
    v["ts"] = json!(store::now());
    let Some(l) = log else { return };
    // One write per line: Display alone would write it in pieces.
    let line = format!("{v}\n");
    if l.file.write_all(line.as_bytes()).is_ok() {
        l.size += line.len() as u64;
    }
    if l.size > most() {
        // A log that cannot be moved, or opened again, carries on as it is for another `MOST`.
        let moved = std::fs::rename(&l.path, l.path.with_extension("log.1")).is_ok();
        match moved.then(|| start(&l.path.clone(), Some(&l.file))).flatten() {
            Some(new) => *l = new,
            None => l.size = 0,
        }
    }
}

/// `path` cut back to its first `keep` bytes once past `most`, in place: whoever appends to it
/// goes on at the new end. A cut that keeps some says where it was made.
pub(super) fn cut(path: &Path, most: u64, keep: u64) {
    let Ok(mut f) = OpenOptions::new().append(true).open(path) else { return };
    if f.metadata().is_ok_and(|m| m.len() > most) && f.set_len(keep).is_ok() && keep > 0 {
        let _ = write!(f, "\n--- cut here by gensokyo, past {} KB\n", most >> 10);
    }
}

/// A panic as one log line. Tokio catches a task's panic and carries on without the task, so
/// this line is all there is to say it happened. Never waits for the log: the panic may have
/// come from inside `log`.
fn panic_hook(p: &std::panic::PanicHookInfo) {
    let at = p.location().map(|l| format!("{}:{}", l.file(), l.line()));
    let v = json!({"ev": "panic", "msg": p.payload_as_str(), "at": at});
    match LOG.try_lock() {
        Ok(mut l) => write_log(&mut l, v),
        Err(_) => eprintln!("{v}"),
    }
}
