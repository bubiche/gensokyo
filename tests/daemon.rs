//! The daemon end to end, with tests/stub-claude as every resident: each test gets its own state
//! dir and socket, drives the daemon over the socket, and quits it at the end.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_gensokyo");

struct Daemon {
    dir: PathBuf,
    env: Vec<(String, String)>,
}

impl Daemon {
    /// Started by a CLI call, as a user's first `gensokyo` starts it. `env` reaches the
    /// daemon and so every resident.
    fn start(name: &str, env: &[(&str, &str)]) -> Daemon {
        let d = Daemon::new(name, env);
        d.cli(&["list"]);
        d
    }

    fn new(name: &str, env: &[(&str, &str)]) -> Daemon {
        let dir =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("d-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut all = vec![
            ("GENSOKYO_STATE_DIR".to_string(), dir.display().to_string()),
            (
                "GENSOKYO_CLAUDE".into(),
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stub-claude").into(),
            ),
            ("STUB_STATE".into(), dir.join("stub").display().to_string()),
            ("CLAUDE_CODE_CHILD_SESSION".into(), "1".into()),
            ("CLAUDE_CONFIG_DIR".into(), dir.join("claude").display().to_string()),
            ("TERM_PROGRAM".into(), "iTerm.app".into()),
            ("ITERM_SESSION_ID".into(), "w0t0p0".into()),
        ];
        all.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        assert!(dir.join("run/gensokyo.sock").as_os_str().len() < 104);
        Daemon { dir, env: all }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.args(args).envs(self.env.iter().map(|(k, v)| (k, v))).env_remove("GENSOKYO_SOCKET");
        c
    }

    fn cli(&self, args: &[&str]) -> Output {
        let out = self.command(args).stdin(Stdio::null()).output().unwrap();
        assert!(
            out.status.success(),
            "gensokyo {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("run/gensokyo.sock")
    }

    /// hello, one request, its reply.
    fn req(&self, req: Value) -> Value {
        let s = UnixStream::connect(self.socket()).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let mut w = s.try_clone().unwrap();
        writeln!(w, "{}\n{req}", json!({"t": "hello", "proto": 1, "who": "test"})).unwrap();
        let mut lines = BufReader::new(s).lines();
        let welcome: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        assert_eq!(welcome["t"], "welcome");
        serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
    }

    fn summon(&self, extra: Value) -> Value {
        let mut req = json!({"t": "summon", "id": 7, "cwd": self.dir});
        req.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let r = self.req(req);
        assert_eq!(r["t"], "summoned", "{r}");
        assert_eq!(r["id"], 7);
        r["resident"].clone()
    }

    fn list(&self) -> Vec<Value> {
        self.req(json!({"t": "list", "id": 1}))["residents"].as_array().unwrap().clone()
    }

    fn stub(&self, id: &Value, ext: &str) -> String {
        // Everything else is written before `ready`, so it is whole by then.
        let file = |e: &str| self.dir.join("stub").join(format!("{}.{e}", id.as_str().unwrap()));
        wait(|| file("ready").exists(), "the stub to be ready");
        let p = file(ext);
        std::fs::read_to_string(p).unwrap()
    }

    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.join("daemon.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.command(&["quit"]).output();
    }
}

fn wait(mut f: impl FnMut() -> bool, what: &str) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn alive(pid: i64) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[test]
fn summon_launches_claude_with_our_argv_and_env() {
    // A locale that is not UTF-8 is replaced, not kept.
    let d = Daemon::start("summon", &[("LC_ALL", "C"), ("LANG", "C")]);
    let r = d.summon(json!({"name": "Reimu", "prompt": "hello there", "model": "haiku"}));
    assert_eq!((r["name"].as_str(), r["slot"].as_u64()), (Some("Reimu"), Some(1)));
    assert!(alive(r["pid"].as_i64().unwrap()));
    let id = r["id"].clone();

    // The record holds the whole launch, and the stub got exactly that.
    let rec: Value = serde_json::from_str(
        &std::fs::read_to_string(d.dir.join(format!("residents/{}.json", id.as_str().unwrap())))
            .unwrap(),
    )
    .unwrap();
    let argv: Vec<&str> =
        rec["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    assert_eq!(
        argv[..5],
        ["--disallowed-tools", "CronCreate", "CronList", "CronDelete", "--settings"]
    );
    assert_eq!(argv[argv.len() - 2..], ["--", "hello there"]);
    let at = |f: &str| argv[argv.iter().position(|a| *a == f).unwrap() + 1];
    assert_eq!(
        (at("--session-id"), at("--name"), at("--model")),
        (id.as_str().unwrap(), "Reimu", "haiku")
    );
    assert!(at("--plugin-dir").ends_with("/share/plugin"));
    assert!(at("--append-system-prompt").contains("your resident name is Reimu."));
    assert_eq!(d.stub(&id, "args").trim_end(), argv.join(" "));

    let settings: Value = serde_json::from_str(at("--settings")).unwrap();
    for h in ["SessionStart", "SessionEnd", "UserPromptSubmit", "Stop", "Notification"] {
        let cmd = settings["hooks"][h][0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(cmd, format!("'{BIN}' _hook"));
    }
    assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "AskUserQuestion");
    let status = settings["statusLine"]["command"].as_str().unwrap();
    assert_eq!(status, format!("'{BIN}' _statusline '{}'", id.as_str().unwrap()));
    let real = std::fs::canonicalize(d.socket()).unwrap();
    assert_eq!(settings["sandbox"]["network"]["allowUnixSockets"], json!([real]));

    let env = d.stub(&id, "env");
    let var = |k: &str| env.lines().find_map(|l| l.strip_prefix(&format!("{k}=")));
    assert_eq!(var("CLAUDE_CODE_CHILD_SESSION"), None);
    assert_eq!(var("TERM_PROGRAM"), None);
    assert_eq!(var("ITERM_SESSION_ID"), None);
    assert_eq!(var("CLAUDE_CONFIG_DIR"), Some(d.dir.join("claude").to_str().unwrap()));
    assert_eq!((var("TERM"), var("COLORTERM")), (Some("xterm-256color"), Some("truecolor")));
    assert_eq!(var("GENSOKYO_RESIDENT"), id.as_str());
    assert_eq!(var("GENSOKYO_SOCKET").map(PathBuf::from), Some(real));
    let bin = Path::new(BIN).parent().unwrap().to_str().unwrap();
    assert!(var("PATH").unwrap().starts_with(&format!("{bin}:")));
    assert_eq!((var("LC_ALL"), var("LANG")), (None, Some("en_US.UTF-8")));
    assert_eq!(
        var("PWD").map(|p| std::fs::canonicalize(p).unwrap()),
        Some(std::fs::canonicalize(&d.dir).unwrap())
    );

    // Names and slots.
    let r2 = d.summon(json!({}));
    assert_eq!(r2["slot"], 2);
    assert_ne!(r2["name"], "Reimu");
    for (bad, why) in [
        (json!({"name": "reimu"}), "already here"),
        (json!({"name": "9lives"}), "starts with a letter"),
        (json!({"cwd": "/no/such/dir"}), "no such directory"),
        (json!({"prompt": "two\nlines"}), "single line"),
    ] {
        let mut req = json!({"t": "summon", "id": 3, "cwd": d.dir});
        req.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        let e = d.req(req);
        assert_eq!(e["t"], "error");
        assert!(e["error"].as_str().unwrap().contains(why), "{e}");
    }
    assert_eq!(d.list().len(), 2);
}

#[test]
fn banish_then_close_moves_the_record_to_departed() {
    let d = Daemon::start("banish", &[]);
    let r = d.summon(json!({"name": "Marisa"}));
    let (id, pid) = (r["id"].as_str().unwrap().to_string(), r["pid"].as_i64().unwrap());
    d.stub(&r["id"], "ready");
    let t = Instant::now();
    let b = d.req(json!({"t": "banish", "id": 2, "who": "marisa"}));
    assert_eq!(b["t"], "done", "{b}");
    assert!(t.elapsed() < Duration::from_secs(1), "a HUP-obeying resident waited out the grace");
    assert!(!alive(pid));
    let l = d.list();
    assert_eq!((l[0]["exit"].as_i64(), l[0]["pid"].is_null()), (Some(129), true));
    assert!(l[0]["departed"].is_i64());
    assert!(d.dir.join(format!("residents/{id}.json")).exists());
    let again = d.req(json!({"t": "banish", "id": 3, "who": "1"}));
    assert!(again["error"].as_str().unwrap().contains("already departed"));

    let c = d.req(json!({"t": "close", "id": 4, "who": "1"}));
    assert_eq!(c["t"], "done", "{c}");
    assert!(d.list().is_empty());
    assert!(!d.dir.join(format!("residents/{id}.json")).exists());
    let rec: Value = serde_json::from_str(
        &std::fs::read_to_string(d.dir.join(format!("departed/{id}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(rec["exit"], 129);
    let ev: Vec<_> = d.log().into_iter().filter(|e| e["ev"] == "banished").collect();
    assert_eq!(ev.len(), 1);
    assert!(ev[0]["hup_to_exit_ms"].as_u64().unwrap() < 1000);
}

#[test]
fn banish_sweeps_a_stubborn_leader_and_its_detached_descendant() {
    let d = Daemon::start(
        "sweep",
        &[("STUB_HUP", "ignore"), ("STUB_TERM", "ignore"), ("STUB_DESCENDANT", "1")],
    );
    let r = d.summon(json!({}));
    let pid = r["pid"].as_i64().unwrap();
    d.stub(&r["id"], "ready");
    let desc: i64 = d.stub(&r["id"], "descendant").trim().parse().unwrap();
    assert!(alive(desc));
    let t = Instant::now();
    let b = d.req(json!({"t": "banish", "id": 2, "who": r["name"]}));
    assert_eq!(b["t"], "done", "{b}");
    // HUP and TERM are ignored, so it took the whole grace and the KILL.
    assert!(t.elapsed() >= Duration::from_secs(3), "{:?}", t.elapsed());
    assert!(!alive(pid) && !alive(desc));
    assert_eq!(d.list()[0]["signal"], 9);
    let ev = d.log().into_iter().find(|e| e["ev"] == "banished").unwrap();
    let sigs: Vec<_> =
        ev["stages"].as_array().unwrap().iter().map(|s| s["sig"].as_i64().unwrap()).collect();
    assert_eq!(sigs, [1, 15, 9]);
    let hup = ev["stages"][0]["pids"].as_array().unwrap();
    assert!(hup.contains(&json!(pid)) && hup.contains(&json!(desc)), "{ev}");
}

#[test]
fn close_asks_for_exit_and_asks_again_when_it_was_lost() {
    let d = Daemon::start("close", &[]);
    let r = d.summon(json!({"name": "Sakuya"}));
    d.stub(&r["id"], "ready");
    // The first /exit arrives a character short, as a lost keystroke leaves it.
    let eat = d.dir.join("stub").join(format!("{}.eat-exit", r["id"].as_str().unwrap()));
    std::fs::write(&eat, "1\n").unwrap();
    let c = d.req(json!({"t": "close", "id": 2, "who": "Sakuya"}));
    assert_eq!(c["t"], "done", "{c}");
    assert_eq!(std::fs::read_to_string(&eat).unwrap().trim(), "0");
    let l = d.list();
    assert_eq!((l[0]["exit"].as_i64(), l[0]["signal"].as_i64()), (Some(0), None));
    assert!(!alive(r["pid"].as_i64().unwrap()));
}

#[test]
fn quit_asks_everyone_to_leave_and_stops() {
    let d = Daemon::start("quit", &[]);
    let rs: Vec<Value> = (0..3).map(|_| d.summon(json!({}))).collect();
    rs.iter().for_each(|r| drop(d.stub(&r["id"], "ready")));
    let pids: Vec<i64> = rs.iter().map(|r| r["pid"].as_i64().unwrap()).collect();
    let daemon = d.log()[0]["pid"].as_i64().unwrap();
    let q = d.req(json!({"t": "quit", "id": 9}));
    assert_eq!(q["t"], "done", "{q}");
    wait(|| !alive(daemon), "the daemon to exit");
    assert!(pids.iter().all(|&p| !alive(p)));
    assert!(!d.socket().exists());
    assert_eq!(std::fs::read_dir(d.dir.join("residents")).unwrap().count(), 0);
    let departed: Vec<Value> = std::fs::read_dir(d.dir.join("departed"))
        .unwrap()
        .map(|e| {
            serde_json::from_str(&std::fs::read_to_string(e.unwrap().path()).unwrap()).unwrap()
        })
        .collect();
    assert_eq!(departed.len(), 3);
    assert!(departed.iter().all(|r| r["exit"] == 0 && r["departed"].is_i64()), "{departed:?}");
    // No resident needed the sweep: every one of them read its /exit.
    assert!(!d.log().iter().any(|e| e["ev"] == "quit-banish"));
}

#[test]
fn ten_clients_at_once_start_one_daemon() {
    let d = Daemon::new("ten", &[]);
    let kids: Vec<_> = (0..10)
        .map(|_| d.command(&["list", "--json"]).stdout(Stdio::piped()).spawn().unwrap())
        .collect();
    for k in kids {
        let out = k.wait_with_output().unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "[]");
    }
    let started = d.log().into_iter().filter(|e| e["ev"] == "started").count();
    assert_eq!(started, 1);
}

#[test]
fn a_second_daemon_leaves_quietly() {
    let d = Daemon::start("second", &[]);
    let t = Instant::now();
    let out = d.command(&["daemon"]).output().unwrap();
    assert!(out.status.success() && t.elapsed() < Duration::from_secs(2));
    assert!(d.log().iter().any(|e| e["why"] == "another daemon holds run/daemon.lock"));
    assert_eq!(d.req(json!({"t": "list", "id": 1}))["t"], "list");
}

#[test]
fn a_restart_retires_records_left_by_the_last_daemon() {
    let d = Daemon::new("restart", &[]);
    std::fs::create_dir_all(d.dir.join("residents")).unwrap();
    let rec = json!({"id": "old", "session": "old", "name": "Cirno", "slot": 3, "cwd": "/",
                     "program": "claude", "argv": [], "launched": 1});
    std::fs::write(d.dir.join("residents/old.json"), rec.to_string()).unwrap();
    d.cli(&["list"]);
    assert!(d.list().is_empty());
    let moved: Value =
        serde_json::from_str(&std::fs::read_to_string(d.dir.join("departed/old.json")).unwrap())
            .unwrap();
    assert_eq!(moved["name"], "Cirno");
    assert!(moved["departed"].is_i64());
}

#[test]
fn protocol_errors() {
    let d = Daemon::start("proto", &[]);
    let talk = |lines: &str| -> Vec<Value> {
        let s = UnixStream::connect(d.socket()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        (&s).write_all(lines.as_bytes()).unwrap();
        s.shutdown(std::net::Shutdown::Write).unwrap();
        BufReader::new(s).lines().map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect()
    };
    // Each is answered, then the connection closes.
    let r = talk("{\"t\":\"list\",\"id\":5}\n");
    assert_eq!((r.len(), &r[0]["error"]), (1, &json!("say hello first")));
    let r = talk("{\"t\":\"hello\",\"proto\":99,\"who\":\"x\"}\n");
    assert!(r[0]["error"].as_str().unwrap().contains("protocol 99"));
    let r = talk("not json\n");
    assert_eq!(r[0]["t"], "error");
    // After a hello, a bad line is answered and the connection stays.
    let r = talk(
        "{\"t\":\"hello\",\"proto\":1,\"who\":\"x\"}\n{\"t\":\"nope\",\"id\":1}\n{\"t\":\"list\",\"id\":2}\n",
    );
    let r: Vec<_> = r.iter().map(|v| v["t"].as_str().unwrap()).collect();
    assert_eq!(r, ["welcome", "error", "list"]);
    let e = d.req(json!({"t": "banish", "id": 1, "who": "nobody"}));
    assert_eq!(e["error"], "no resident nobody");
}

#[test]
fn hook_verbs_always_exit_zero_and_print_nothing() {
    for verb in [&["_hook"][..], &["_statusline", "some-id"]] {
        let mut c = Command::new(BIN)
            .args(verb)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(b"{\"hook_event_name\":\"Stop\"}").unwrap();
        let out = c.wait_with_output().unwrap();
        assert_eq!((out.status.code(), out.stdout.len()), (Some(0), 0));
    }
}

/// A connection that stays: events arrive on a thread, so a read can give up without losing
/// half a line.
struct Client {
    w: UnixStream,
    rx: std::sync::mpsc::Receiver<Value>,
}

impl Client {
    fn new(d: &Daemon) -> Client {
        let s = UnixStream::connect(d.socket()).expect("connect");
        let (tx, rx) = std::sync::mpsc::channel();
        let r = s.try_clone().unwrap();
        std::thread::spawn(move || {
            for l in BufReader::new(r).lines() {
                let Ok(l) = l else { return };
                if tx.send(serde_json::from_str(&l).unwrap()).is_err() {
                    return;
                }
            }
        });
        let mut c = Client { w: s, rx };
        c.send(json!({"t": "hello", "proto": 1, "who": "test"}));
        assert_eq!(c.next(Duration::from_secs(5)).unwrap()["t"], "welcome");
        c
    }

    fn send(&mut self, v: Value) {
        writeln!(self.w, "{v}").unwrap();
    }

    fn next(&mut self, within: Duration) -> Option<Value> {
        self.rx.recv_timeout(within).ok()
    }

    /// Skips events until one passes `f`.
    fn until(&mut self, what: &str, mut f: impl FnMut(&Value) -> bool) -> Value {
        let t = Instant::now();
        loop {
            let left = Duration::from_secs(10).saturating_sub(t.elapsed());
            let v = self.next(left).unwrap_or_else(|| panic!("timed out waiting for {what}"));
            if f(&v) {
                return v;
            }
        }
    }
}

/// The screen as a viewer holds it, one string per row.
#[derive(Default)]
struct Screen {
    rev: u64,
    rows: Vec<String>,
    cols: u64,
}

impl Screen {
    fn row(runs: &Value) -> String {
        let mut s = String::new();
        for r in runs.as_array().unwrap() {
            let col = r["col"].as_u64().unwrap() as usize;
            while s.chars().count() < col {
                s.push(' ');
            }
            s.push_str(r["text"].as_str().unwrap());
        }
        s
    }

    /// Takes a `frame` or a `damage`; anything else is not for it.
    fn apply(&mut self, ev: &Value) -> bool {
        match ev["t"].as_str() {
            Some("frame") => {
                let f = &ev["frame"];
                self.rows = f["rows"].as_array().unwrap().iter().map(Screen::row).collect();
                self.cols = f["cols"].as_u64().unwrap();
            }
            Some("damage") => {
                assert_eq!(ev["base"].as_u64(), Some(self.rev), "a damage off another base");
                for r in ev["rows"].as_array().unwrap() {
                    self.rows[r[0].as_u64().unwrap() as usize] = Screen::row(&r[1]);
                }
            }
            _ => return false,
        }
        self.rev = ev["rev"].as_u64().unwrap();
        true
    }

    fn has(&self, s: &str) -> bool {
        self.rows.iter().any(|r| r.contains(s))
    }

    /// Applies events until the screen shows `s`.
    fn wait_for(&mut self, c: &mut Client, s: &str) {
        c.until(&format!("{s:?} on screen"), |ev| self.apply(ev) && self.has(s));
    }
}

#[test]
fn a_watcher_sees_the_shrine_and_a_viewer_the_screen() {
    let d = Daemon::start("view", &[]);
    let mut c = Client::new(&d);
    c.send(json!({"t": "watch"}));
    let ev = c.until("the first residents", |v| v["t"] == "residents");
    assert_eq!(ev["residents"], json!([]));
    let r = d.summon(json!({"name": "Reimu"}));
    let id = r["id"].as_str().unwrap().to_string();
    let ev = c.until("a summon event", |v| v["t"] == "residents");
    assert_eq!(ev["residents"][0]["name"], "Reimu");
    d.stub(&r["id"], "ready");

    c.send(json!({"t": "view", "who": "reimu"}));
    let mut s = Screen::default();
    let f = c.until("a frame", |v| v["t"] == "frame");
    assert_eq!(f["who"], id.as_str());
    assert_eq!((f["rev"].as_u64(), f["modes"]["kitty"].as_u64()), (Some(1), Some(5)));
    assert_eq!(f["modes"]["paste"], true);
    s.apply(&f);
    s.wait_for(&mut c, &format!("stub-claude Reimu ({id})"));

    // Bytes go to the tty as they are; a key goes by the child's flags, kitty 5 here.
    c.send(json!({"t": "input", "who": "Reimu", "bytes": b"hello\r"}));
    s.wait_for(&mut c, "> hello");
    c.send(json!({"t": "input", "who": "1", "key": {"code": 13, "mods": 1, "event": 1}}));
    c.send(json!({"t": "input", "who": "1", "bytes": b"\r"}));
    let input = d.dir.join(format!("stub/{id}.input"));
    wait(
        || std::fs::read_to_string(&input).is_ok_and(|i| i.lines().any(|l| l == "\x1b[13;2u")),
        "the encoded Shift+Enter",
    );
    c.send(json!({"t": "input", "who": "nobody", "id": 4, "bytes": b"x"}));
    let e = c.until("an input error", |v| v["t"] == "error");
    assert_eq!((e["id"].as_u64(), &e["error"]), (Some(4), &json!("no resident nobody")));

    // A banish is answered on its own time; the shrine event says who left.
    c.send(json!({"t": "banish", "id": 8, "who": "Reimu"}));
    let (mut left, mut done) = (None, false);
    c.until("the departure event and the banish reply", |v| {
        if v["t"] == "residents" && v["residents"][0]["departed"].is_i64() {
            left = Some(v.clone());
        }
        done |= v["t"] == "done" && v["id"] == 8;
        left.is_some() && done
    });
    assert!(left.unwrap()["residents"][0]["pid"].is_null());
    c.send(json!({"t": "view", "id": 9, "who": "Reimu"}));
    let e = c.until("a view error", |v| v["t"] == "error");
    assert!(e["error"].as_str().unwrap().contains("already departed"), "{e}");
}

#[test]
fn a_resize_reaches_every_child_and_later_summons() {
    let d = Daemon::start("resize", &[("STUB_WINCH", "1")]);
    let r = d.summon(json!({}));
    let id = r["id"].as_str().unwrap().to_string();
    d.stub(&r["id"], "ready");
    let mut c = Client::new(&d);
    c.send(json!({"t": "resize", "cols": 100, "rows": 30}));
    let size = d.dir.join(format!("stub/{id}.size"));
    let read = || std::fs::read_to_string(&size).unwrap_or_default().trim().to_string();
    wait(|| read() == "30 100", "the child to see 30 100");

    // The first view on a connection nudges the size and back, which redraws Claude Code.
    std::fs::remove_file(&size).unwrap();
    c.send(json!({"t": "view", "who": id}));
    let mut s = Screen::default();
    s.apply(&c.until("a frame", |v| v["t"] == "frame"));
    assert_eq!((s.cols, s.rows.len()), (100, 30));
    // The two SIGWINCHes may reach the stub as one, so only the second size is sure to show.
    wait(|| read() == "30 100", "a SIGWINCH from the nudge, and the size back");

    let r2 = d.summon(json!({}));
    c.send(json!({"t": "view", "who": r2["id"]}));
    let f = c.until("the new resident's frame", |v| v["t"] == "frame" && v["who"] == r2["id"]);
    assert_eq!(
        (f["frame"]["cols"].as_u64(), f["frame"]["rows"].as_array().unwrap().len()),
        (Some(100), 30)
    );
}

#[test]
fn a_synchronized_block_is_shown_whole_unless_it_never_ends() {
    let d = Daemon::start("sync", &[]);
    let r = d.summon(json!({}));
    d.stub(&r["id"], "ready");
    let mut c = Client::new(&d);
    c.send(json!({"t": "view", "who": r["id"]}));
    let mut s = Screen::default();
    s.wait_for(&mut c, "stub-claude");
    // The emulator ends a block on a resize, so the first view's nudge has to be over.
    std::thread::sleep(Duration::from_millis(200));

    // A short block: nobody sees its first half alone.
    c.send(json!({"t": "input", "who": r["id"], "bytes": b"/sync 0.05\r"}));
    c.until("the short block's end", |ev| {
        s.apply(ev);
        assert!(!(s.has("SYNC-MID") && !s.has("SYNC-END")), "half a block was shown: {ev}");
        s.has("SYNC-END")
    });

    // A long one is shown anyway once the hold runs out.
    c.send(json!({"t": "input", "who": r["id"], "bytes": b"clear\r"}));
    s.wait_for(&mut c, "> clear");
    let mids = s.rows.iter().filter(|r| r.contains("SYNC-MID")).count();
    let t = Instant::now();
    c.send(json!({"t": "input", "who": r["id"], "bytes": b"/sync 2\r"}));
    c.until("the long block's middle", |ev| {
        s.apply(ev);
        s.rows.iter().filter(|r| r.contains("SYNC-MID")).count() > mids
    });
    assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
    assert_eq!(s.rows.iter().filter(|r| r.contains("SYNC-END")).count(), 1);
}

#[test]
fn unview_stops_the_screen() {
    let d = Daemon::start("unview", &[]);
    let r = d.summon(json!({}));
    d.stub(&r["id"], "ready");
    let mut c = Client::new(&d);
    c.send(json!({"t": "view", "who": r["id"]}));
    let mut s = Screen::default();
    s.wait_for(&mut c, "stub-claude");
    c.send(json!({"t": "unview"}));
    c.send(json!({"t": "list", "id": 3}));
    c.until("the list after unview", |v| v["t"] == "list");
    c.send(json!({"t": "input", "who": r["id"], "bytes": b"quiet\r"}));
    let input = d.dir.join(format!("stub/{}.input", r["id"].as_str().unwrap()));
    wait(|| std::fs::read_to_string(&input).is_ok_and(|i| i.contains("quiet")), "the input");
    std::thread::sleep(Duration::from_millis(300));
    while let Some(ev) = c.next(Duration::from_millis(100)) {
        assert!(!["frame", "damage"].contains(&ev["t"].as_str().unwrap()), "{ev}");
    }
    // Viewing again starts over with a whole frame.
    c.send(json!({"t": "view", "who": r["id"]}));
    let f = c.until("a new frame", |v| v["t"] == "frame");
    assert_eq!(f["rev"], 1);
}

#[test]
fn recall_resumes_a_banished_resident_with_its_flags() {
    let d = Daemon::start("recall", &[]);
    let r = d.summon(json!({"name": "Youmu", "model": "haiku", "prompt": "first"}));
    let id = r["id"].as_str().unwrap().to_string();
    let args = |d: &Daemon| d.stub(&r["id"], "args").trim_end().to_string();
    let before = args(&d);
    let e = d.req(json!({"t": "recall", "id": 2, "who": "Youmu"}));
    assert!(e["error"].as_str().unwrap().contains("still here"), "{e}");
    assert_eq!(d.req(json!({"t": "banish", "id": 3, "who": "Youmu"}))["t"], "done");
    std::fs::remove_file(d.dir.join(format!("stub/{id}.ready"))).unwrap();

    let back = d.req(json!({"t": "recall", "id": 4, "who": "youmu"}));
    assert_eq!(back["t"], "summoned", "{back}");
    assert_eq!(
        (back["resident"]["id"].as_str(), back["resident"]["slot"].as_u64()),
        (Some(id.as_str()), Some(1))
    );
    assert!(back["resident"]["departed"].is_null());
    let after = args(&d);
    let flag = |a: &str, f: &str| a.split(' ').skip_while(|x| *x != f).nth(1).map(str::to_string);
    assert_eq!(flag(&after, "--resume").as_deref(), Some(id.as_str()));
    assert_eq!(flag(&after, "--model").as_deref(), Some("haiku"));
    assert!(!after.contains("--session-id") && !after.ends_with(" -- first"), "{after}");
    let settings =
        |a: &str| a[a.find("--settings ").unwrap()..a.find(" --plugin-dir").unwrap()].to_string();
    assert_eq!(settings(&after), settings(&before));
    let l = d.list();
    assert_eq!((l.len(), l[0]["pid"].is_i64()), (1, true));
}

#[test]
fn list_all_shows_the_departed_of_an_earlier_run_and_resume_brings_one_back() {
    let d = Daemon::start("all", &[]);
    let a = d.summon(json!({"name": "Aya"}));
    d.stub(&a["id"], "ready");
    let b = d.summon(json!({"name": "Hatate"}));
    d.stub(&b["id"], "ready");
    assert_eq!(d.req(json!({"t": "banish", "id": 2, "who": "Hatate"}))["t"], "done");
    std::thread::sleep(Duration::from_millis(1100));
    let daemon = d.log()[0]["pid"].as_i64().unwrap();
    assert_eq!(d.req(json!({"t": "quit", "id": 3}))["t"], "done");
    wait(|| !alive(daemon), "the daemon to exit");

    d.cli(&["list"]);
    assert!(d.list().is_empty());
    let all = d.req(json!({"t": "list", "id": 4, "all": true}))["residents"].clone();
    let names: Vec<_> =
        all.as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    // Aya left last, at the quit.
    assert_eq!(names, ["Aya", "Hatate"]);
    assert!(all.as_array().unwrap().iter().all(|r| r["slot"].is_null() && r["departed"].is_i64()));

    std::fs::remove_file(d.dir.join(format!("stub/{}.ready", b["id"].as_str().unwrap()))).unwrap();
    let out = d.cli(&["resume", "hatate"]);
    // Its old slot is free, so it gets it back. It was never given a prompt, so Claude Code kept
    // no conversation to resume: it starts afresh under the same id and name.
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("recalled Hatate (slot 2)"));
    let bid = b["id"].as_str().unwrap();
    assert!(d.stub(&b["id"], "args").contains(&format!("--session-id {bid} --name Hatate")));
    assert!(!d.dir.join(format!("departed/{}.json", b["id"].as_str().unwrap())).exists());
    let all = d.req(json!({"t": "list", "id": 5, "all": true}))["residents"].clone();
    let names: Vec<_> =
        all.as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Hatate", "Aya"]);
}
