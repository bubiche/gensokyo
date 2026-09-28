//! `daemon.log`: one JSON line per event, stderr and panics included, whoever writes them.

use super::store;
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

static LOG: Mutex<Option<File>> = Mutex::new(None);

/// The log from now on: appended to, with stderr and panics going there too.
pub fn open(path: &Path) {
    let f = OpenOptions::new().create(true).append(true).open(path).ok();
    // Whatever else reaches stderr lands in the log too, however the daemon was started.
    if let Some(f) = &f {
        use std::os::fd::AsRawFd;
        // SAFETY: dup2 onto fd 2, which this process owns; the file stays open in `LOG`.
        unsafe { libc::dup2(f.as_raw_fd(), 2) };
    }
    *LOG.lock().unwrap_or_else(|e| e.into_inner()) = f;
    std::panic::set_hook(Box::new(panic_hook));
}

/// One JSON line in `daemon.log`.
pub fn log(v: Value) {
    write_log(LOG.lock().unwrap_or_else(|e| e.into_inner()).as_mut(), v);
}

fn write_log(f: Option<&mut File>, mut v: Value) {
    v["ts"] = json!(store::now());
    // One write per line: Display alone would write it in pieces.
    if let Some(f) = f {
        let _ = f.write_all(format!("{v}\n").as_bytes());
    }
}

/// A panic as one log line. Tokio catches a task's panic and carries on without the task, so
/// this line is all there is to say it happened. Never waits for the log: the panic may have
/// come from inside `log`.
fn panic_hook(p: &std::panic::PanicHookInfo) {
    let at = p.location().map(|l| format!("{}:{}", l.file(), l.line()));
    let v = json!({"ev": "panic", "msg": p.payload_as_str(), "at": at});
    match LOG.try_lock() {
        Ok(mut f) => write_log(f.as_mut(), v),
        Err(_) => eprintln!("{v}"),
    }
}
