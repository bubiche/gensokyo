//! What the integration tests share: a directory of a test's own, a daemon whose residents are
//! tests/stub-claude, the hello, polling, and the base64 of recorded sessions. Each test binary
//! uses only part of it.
#![allow(dead_code)]

use gensokyo::proto::PROTO;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub const BIN: &str = env!("CARGO_BIN_EXE_gensokyo");

pub type Lines = std::io::Lines<BufReader<UnixStream>>;

/// `<name>-<pid>` under the target's tmp dir, made empty.
pub fn fresh(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A daemon's state and config under `dir`, with tests/stub-claude as every resident.
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
        ("CLAUDE_CONFIG_DIR".into(), under("claude")),
    ]
}

/// The hello a client of this build says.
pub fn hello(who: &str) -> Value {
    json!({"t": "hello", "proto": PROTO, "who": who})
}

/// Connected and welcomed: the writer, and the lines that come back.
pub fn connect(sock: &Path) -> (UnixStream, Lines) {
    let s = UnixStream::connect(sock).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut w = s.try_clone().unwrap();
    writeln!(w, "{}", hello("test")).unwrap();
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
        let mut c = Command::new(BIN);
        c.args(args).envs(self.env.iter().map(|(k, v)| (k, v))).env_remove("GENSOKYO_SOCKET");
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
