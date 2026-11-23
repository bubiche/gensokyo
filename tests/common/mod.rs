//! What the integration tests share: a directory of a test's own, a daemon whose residents are
//! tests/stub-claude, the hello, polling, and the base64 of recorded sessions. Each test binary
//! uses only part of it.
#![allow(dead_code)]

use gensokyo::proto::PROTO;
use serde_json::{Value, json};
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub const BIN: &str = env!("CARGO_BIN_EXE_gensokyo");

pub type Lines = std::io::Lines<BufReader<UnixStream>>;

/// `<name>-<pid>` under a short dir in `/tmp`, made empty. Not under the target's tmp dir: from
/// a deep checkout, a daemon's `run/gensokyo.sock` there is past the 104 bytes a socket path may
/// hold. Canonical (`/private/tmp` on macOS), as the daemon compares the paths it is given with
/// the ones it finds.
pub fn fresh(name: &str) -> PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let root = ROOT.get_or_init(|| {
        // SAFETY: plain libc call.
        let uid = unsafe { libc::getuid() };
        let root = std::fs::canonicalize("/tmp").unwrap().join(format!("gensokyo-tests-{uid}"));
        sweep(&root);
        root
    });
    let d = root.join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Removes the `<name>-<pid>` dirs in `root` whose test process has exited: unlike the target's
/// tmp dir, nothing else clears them.
pub fn sweep(root: &Path) {
    let gone = |pid: i32| {
        // SAFETY: signal 0 only asks whether the process is there.
        let r = unsafe { libc::kill(pid, 0) };
        r != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    };
    for d in root.read_dir().into_iter().flatten().flatten() {
        let name = d.file_name();
        let pid = name.to_str().and_then(|n| n.rsplit_once('-')?.1.parse().ok());
        if pid.is_some_and(gone) {
            let _ = std::fs::remove_dir_all(d.path());
        }
    }
}

/// `program`, with none of the `GENSOKYO_*` variables of the shell running the tests: from inside
/// a resident they name the user's own daemon, state and resident. Every gensokyo a test runs is
/// made here (or with `inherited` for a pseudo-terminal's), and what it should have goes on top.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    command_from(program, std::env::vars_os())
}

/// `command`, as if the tests ran with `parent` for their environment.
pub fn command_from(
    program: impl AsRef<OsStr>,
    parent: impl IntoIterator<Item = (OsString, OsString)>,
) -> Command {
    let mut c = Command::new(program);
    for k in inherited(parent) {
        c.env_remove(k);
    }
    c
}

/// The `GENSOKYO_*` names in `parent`.
pub fn inherited(parent: impl IntoIterator<Item = (OsString, OsString)>) -> Vec<OsString> {
    let ours = |k: &OsString| k.as_encoded_bytes().starts_with(b"GENSOKYO_");
    parent.into_iter().map(|(k, _)| k).filter(ours).collect()
}

/// A daemon's state and config under `dir`, with tests/stub-claude as every Claude resident and
/// tests/stub-codex as every Codex one: never a real one from PATH or the ChatGPT app.
pub fn stub_env(dir: &Path) -> Vec<(String, String)> {
    let under = |p: &str| dir.join(p).display().to_string();
    vec![
        ("GENSOKYO_STATE_DIR".into(), dir.display().to_string()),
        ("GENSOKYO_CONFIG_DIR".into(), under("conf")),
        (
            "GENSOKYO_CLAUDE".into(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stub-claude").into(),
        ),
        ("STUB_STATE".into(), under("stub")),
        // Never the real one, which would keep the machine running the tests awake.
        (
            "GENSOKYO_CAFFEINATE".into(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stub-caffeinate").into(),
        ),
        ("CLAUDE_CONFIG_DIR".into(), under("claude")),
        ("GENSOKYO_CODEX".into(), concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stub-codex").into()),
        ("CODEX_HOME".into(), under("codex")),
    ]
}

/// The hello a client of this build says.
pub fn hello(who: &str) -> Value {
    json!({"t": "hello", "proto": PROTO, "who": who})
}

/// Connected and welcomed: the writer, and the lines that come back.
pub fn connect(sock: &Path) -> (UnixStream, Lines) {
    connect_saying(sock, hello("test"))
}

fn connect_saying(sock: &Path, hello: Value) -> (UnixStream, Lines) {
    let s = UnixStream::connect(sock).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut w = s.try_clone().unwrap();
    writeln!(w, "{hello}").unwrap();
    let mut lines = BufReader::new(s).lines();
    let welcome = next(&mut lines);
    assert_eq!(welcome["t"], "welcome", "{welcome}");
    (w, lines)
}

/// The next line, as JSON.
pub fn next(lines: &mut Lines) -> Value {
    serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
}

pub fn wait(f: impl FnMut() -> bool, what: &str) {
    wait_for(Duration::from_secs(15), f, what)
}

pub fn wait_for(most: Duration, mut f: impl FnMut() -> bool, what: &str) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < most, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

pub fn base64(s: &str) -> Vec<u8> {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for &ch in s.trim_end_matches('=').as_bytes() {
        acc = acc << 6 | ABC.iter().position(|&a| a == ch).expect("base64") as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// A daemon of a test's own, quit when dropped so a failed test leaves nothing running. Each
/// test binary says how it is made.
pub struct Daemon {
    pub dir: PathBuf,
    pub env: Vec<(String, String)>,
}

impl Daemon {
    /// In `dir`, not started yet. `extra` goes on top of `stub_env` and reaches the daemon and
    /// so every resident.
    pub fn at(dir: PathBuf, extra: Vec<(String, String)>) -> Daemon {
        let mut env = stub_env(&dir);
        env.extend(extra);
        assert!(dir.join("run/gensokyo.sock").as_os_str().len() < 104);
        Daemon { dir, env }
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut c = command(BIN);
        c.args(args).envs(self.env.iter().map(|(k, v)| (k, v)));
        c
    }

    pub fn cli(&self, args: &[&str]) -> Output {
        let out = self.command(args).stdin(Stdio::null()).output().unwrap();
        assert!(out.status.success(), "gensokyo {args:?}: {}", err(&out));
        out
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("run/gensokyo.sock")
    }

    pub fn connect(&self) -> (UnixStream, Lines) {
        connect(&self.socket())
    }

    /// hello, one request, its reply.
    pub fn req(&self, req: Value) -> Value {
        let (mut w, mut lines) = self.connect();
        writeln!(w, "{req}").unwrap();
        next(&mut lines)
    }

    /// One request as resident `id` would make it, its reply.
    pub fn req_as(&self, id: &str, req: Value) -> Value {
        let mut hello = hello("test");
        hello["resident"] = json!(id);
        let (mut w, mut lines) = connect_saying(&self.socket(), hello);
        writeln!(w, "{req}").unwrap();
        next(&mut lines)
    }

    pub fn list(&self) -> Vec<Value> {
        self.req(json!({"t": "list", "id": 1}))["residents"].as_array().unwrap().clone()
    }

    /// One of the stub's files for resident `id`, once it is ready: everything else is written
    /// before `ready`, so it is whole by then.
    pub fn stub(&self, id: &Value, ext: &str) -> String {
        let file = |e: &str| self.dir.join("stub").join(format!("{}.{e}", id.as_str().unwrap()));
        wait(|| file("ready").exists(), "the stub to be ready");
        std::fs::read_to_string(file(ext)).unwrap_or_default()
    }

    pub fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.join("daemon.log"))
            .unwrap_or_default()
            .lines()
            // The last line may be half written.
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.command(&["quit"]).output();
    }
}
