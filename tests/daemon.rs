//! The daemon end to end, with tests/stub-claude as every resident: each test gets its own state
//! dir and socket, drives the daemon over the socket, and quits it at the end.

mod common;

use common::{BIN, Daemon, fresh, hello, next, wait};
use gensokyo::proto::PROTO;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};

impl Daemon {
    /// Started by a CLI call, as a user's first `gensokyo` starts it. `env` reaches the
    /// daemon and so every resident.
    fn start(name: &str, env: &[(&str, &str)]) -> Daemon {
        let d = Daemon::new(name, env);
        d.cli(&["list"]);
        d
    }

    fn new(name: &str, env: &[(&str, &str)]) -> Daemon {
        let mut extra = vec![
            ("CLAUDE_CODE_CHILD_SESSION".to_string(), "1".to_string()),
            ("TERM_PROGRAM".into(), "iTerm.app".into()),
            ("ITERM_SESSION_ID".into(), "w0t0p0".into()),
        ];
        extra.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Daemon::at(fresh(&format!("d-{name}")), extra)
    }

    fn summon(&self, extra: Value) -> Value {
        let mut req = json!({"t": "summon", "id": 7, "cwd": self.dir});
        req.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let r = self.req(req);
        assert_eq!(r["t"], "summoned", "{r}");
        assert_eq!(r["id"], 7);
        r["resident"].clone()
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
    // No file tool writes a ritual, probe or MCP config: they run later with what they say.
    let deny = json!(format!("Edit(/{}/**)", d.dir.join("conf").display()));
    assert!(settings["permissions"]["deny"].as_array().unwrap().contains(&deny), "{settings}");

    let env = d.stub(&id, "env");
    let var = |k: &str| env.lines().find_map(|l| l.strip_prefix(&format!("{k}=")));
    assert_eq!(var("CLAUDE_CODE_CHILD_SESSION"), None);
    assert_eq!(var("TERM_PROGRAM"), None);
    assert_eq!(var("ITERM_SESSION_ID"), None);
    assert_eq!(var("CLAUDE_CONFIG_DIR"), Some(d.dir.join("claude").to_str().unwrap()));
    assert_eq!((var("TERM"), var("COLORTERM")), (Some("xterm-256color"), Some("truecolor")));
    assert_eq!(var("FORCE_HYPERLINK"), Some("1"));
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
        (json!({"prompt": "x".repeat(65 * 1024)}), "at most 64 KB"),
        (json!({"allowed_tools": ["Read", "-p"]}), "-p starts with -"),
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
fn a_resident_marks_links_unless_the_user_said_otherwise() {
    use gensokyo::daemon::launch;
    let (bin, sock) = (Path::new("/bin"), Path::new("/s"));
    let forced = |base: Vec<(&str, &str)>| -> Vec<String> {
        let base = base.into_iter().map(|(k, v)| (k.into(), v.into()));
        let env = launch::env(base, "id", bin, sock);
        let f = env.iter().filter(|(k, _)| k == "FORCE_HYPERLINK");
        f.map(|(_, v)| v.to_string_lossy().into_owned()).collect()
    };
    assert_eq!(forced(vec![("HOME", "/h")]), ["1"]);
    assert_eq!(forced(vec![("FORCE_HYPERLINK", "0")]), ["0"]);
    assert_eq!(forced(vec![("FORCE_HYPERLINK", "1"), ("TERM_PROGRAM", "iTerm.app")]), ["1"]);
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
fn close_sends_no_enter_once_a_turn_has_started_after_its_ctrl_c() {
    let d = Daemon::start("close-busy", &[("STUB_HOOKS", "1")]);
    let r = d.summon(json!({"name": "Sakuya"}));
    d.stub(&r["id"], "ready");
    // Its hooks keep coming through the Ctrl-C: the Enter could answer a dialog just opened.
    let n = d.log().len();
    let (mut w, mut lines) = d.connect();
    let input = json!({"t": "input", "id": 2, "who": "Sakuya", "bytes": b"/busy 30\r"});
    writeln!(w, "{input}\n{}", json!({"t": "list", "id": 1})).unwrap();
    assert_eq!(next(&mut lines)["t"], "list");
    let tools = || d.log()[n..].iter().filter(|e| e["event"] == "PreToolUse").count();
    wait(|| tools() > 1, "its tools at work");
    let c = d.req(json!({"t": "close", "id": 3, "who": "Sakuya"}));
    assert_eq!(c["t"], "error", "{c}");
    assert!(alive(r["pid"].as_i64().unwrap()), "it took the Enter");
    let held = d.log().iter().filter(|e| e["ev"] == "exit" && e["held"].is_string()).count();
    assert_eq!(held, 2, "asked twice, and held back both times");
    assert!(!d.stub(&r["id"], "input").lines().any(|l| l.ends_with("/exit")));
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
fn protocol_errors() {
    let d = Daemon::start("proto", &[]);
    let talk = |lines: &str| raw(&d, lines);
    // Each is answered, then the connection closes.
    let r = talk("{\"t\":\"list\",\"id\":5}\n");
    assert_eq!((r.len(), &r[0]["error"]), (1, &json!("say hello first")));
    let r = talk("{\"t\":\"hello\",\"proto\":99,\"who\":\"x\"}\n");
    assert!(r[0]["error"].as_str().unwrap().contains("protocol 99"));
    let r = talk("not json\n");
    assert_eq!(r[0]["t"], "error");
    // After a hello, a bad line is answered and the connection stays.
    let r = talk(&format!(
        "{}\n{{\"t\":\"nope\",\"id\":1}}\n{{\"t\":\"list\",\"id\":2}}\n",
        hello("x")
    ));
    let r: Vec<_> = r.iter().map(|v| v["t"].as_str().unwrap()).collect();
    assert_eq!(r, ["welcome", "error", "list"]);
    let e = d.req(json!({"t": "banish", "id": 1, "who": "nobody"}));
    assert_eq!(e["error"], "no resident nobody");
}

/// `lines` as they are, and every line that comes back before the connection closes.
fn raw(d: &Daemon, lines: &str) -> Vec<Value> {
    let s = UnixStream::connect(d.socket()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    (&s).write_all(lines.as_bytes()).unwrap();
    s.shutdown(std::net::Shutdown::Write).unwrap();
    let lines = BufReader::new(s).lines().map_while(Result::ok);
    lines.map(|l| serde_json::from_str(&l).unwrap()).collect()
}

/// hello in `proto` as `who`, then `rest`, and every line that comes back.
fn talk(d: &Daemon, proto: u32, who: &str, rest: &[Value]) -> Vec<Value> {
    let hello = json!({"t": "hello", "proto": proto, "who": who});
    raw(d, &rest.iter().fold(format!("{hello}\n"), |b, r| b + &format!("{r}\n")))
}

#[test]
fn a_hook_in_another_protocol_is_heard_and_nothing_else_from_it() {
    let d = Daemon::start("oldhook", &[]);
    let r = d.summon(json!({"name": "Reimu"}));
    d.stub(&r["id"], "ready");
    // A binary updated under a running resident: its hooks say hello in the new protocol.
    let stop = json!({"t": "hook", "id": 0, "resident": r["id"],
                      "hook": {"event": "Stop", "at": 1, "text": "all done"}});
    for _ in 0..2 {
        assert_eq!(talk(&d, 99, "hook", std::slice::from_ref(&stop))[0]["t"], "welcome");
    }
    wait(|| d.list()[0]["state"] == "awaits", "the hook");
    let r = talk(&d, 99, "hook", &[json!({"t": "list", "id": 1})]);
    assert_eq!((&r[0]["t"], &r[1]["error"]), (&json!("welcome"), &json!("say hello first")));
    assert_eq!(talk(&d, 99, "cli", &[])[0]["error"], format!("protocol 99, want {PROTO}"));
    let refused: Vec<_> = d.log().into_iter().filter(|e| e["ev"] == "refused").collect();
    assert_eq!(refused.len(), 2, "once per asker and protocol: {refused:?}");
    assert_eq!(refused[0]["took"], "its hooks and status lines");
}

#[test]
fn a_hook_a_daemon_refuses_is_spooled_and_one_it_is_slow_with_is_not() {
    let d = Daemon::new("refusing", &[]);
    let (sock, spool) = (d.dir.join("fake.sock"), d.dir.join("spool.jsonl"));
    let error = r#"{"t":"error","id":0,"error":"protocol 4, want 5"}"#;
    let welcome = json!({"t": "welcome", "proto": PROTO, "pid": 1}).to_string();
    for (answer, spooled) in
        [(Some(error.to_string()), true), (Some(welcome), false), (None, false)]
    {
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_file(&spool);
        let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        // The fake keeps the connection open until the hook has exited: one that waited for
        // an answer that never comes could only have given up by itself.
        let (done, hung_up) = std::sync::mpsc::channel::<()>();
        let said = answer.clone();
        let fake = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut hello = String::new();
            BufReader::new(&s).read_line(&mut hello).unwrap();
            if let Some(a) = said {
                writeln!(&s, "{a}").unwrap();
            }
            let _ = hung_up.recv_timeout(Duration::from_secs(20));
            hello
        });
        let mut c = d.command(&["_hook"]);
        c.env("GENSOKYO_SOCKET", &sock).env("GENSOKYO_RESIDENT", "x");
        let mut c = c.stdin(Stdio::piped()).stdout(Stdio::null()).spawn().unwrap();
        c.stdin.take().unwrap().write_all(b"{\"hook_event_name\":\"Stop\"}").unwrap();
        assert!(c.wait().unwrap().success());
        done.send(()).unwrap();
        assert!(fake.join().unwrap().contains("\"who\":\"hook\""));
        assert_eq!(spool.exists(), spooled, "{answer:?}");
    }
}

#[test]
fn a_daemon_that_cannot_take_its_lock_says_why() {
    let d = Daemon::new("nolock", &[]);
    std::fs::create_dir_all(d.dir.join("run/daemon.lock")).unwrap();
    assert!(!d.command(&["daemon"]).output().unwrap().status.success());
    let why = d.log().into_iter().find(|e| e["ev"] == "exit").unwrap()["why"].clone();
    assert!(why.as_str().unwrap().starts_with("run/daemon.lock: "), "{why}");
}

#[test]
fn quits_and_a_sigterm_at_once_ask_everyone_to_leave_once() {
    let d = Daemon::start("quits", &[]);
    let r = d.summon(json!({}));
    d.stub(&r["id"], "ready");
    let daemon = d.log()[0]["pid"].as_i64().unwrap();
    let asks: Vec<_> = (0..2)
        .map(|n| {
            let s = UnixStream::connect(d.socket()).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            let hello = hello("test");
            writeln!(&s, "{hello}\n{}", json!({"t": "quit", "id": n})).unwrap();
            s
        })
        .collect();
    unsafe { libc::kill(daemon as i32, libc::SIGTERM) };
    for s in asks {
        let mut lines = BufReader::new(s).lines();
        let replies: Vec<Value> = (0..2).map(|_| next(&mut lines)).collect();
        assert_eq!((&replies[0]["t"], &replies[1]["t"]), (&json!("welcome"), &json!("done")));
    }
    wait(|| !alive(daemon), "the daemon to exit");
    assert_eq!(d.log().iter().filter(|e| e["ev"] == "stopping").count(), 1);
    let input = d.stub(&r["id"], "input");
    assert_eq!(input.matches("/exit").count(), 1, "{input:?}");
}

#[test]
fn a_resident_that_stops_reading_does_not_hold_up_the_connection() {
    let d = Daemon::start("stuck", &[]);
    let r = d.summon(json!({"name": "Cirno"}));
    d.stub(&r["id"], "ready");
    let pid = r["pid"].as_i64().unwrap() as i32;
    unsafe { libc::kill(pid, libc::SIGSTOP) };
    let s = UnixStream::connect(d.socket()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let w = s.try_clone().unwrap();
    // Whole lines: a tty drops a line it has no room for, and holds back once one is waiting.
    let bytes = b"frozen\n".repeat(2048);
    std::thread::spawn(move || {
        let mut w = std::io::BufWriter::new(w);
        writeln!(w, "{}", hello("test")).unwrap();
        for id in 0..70 {
            writeln!(w, "{}", json!({"t": "input", "id": id, "who": "Cirno", "bytes": bytes}))
                .unwrap();
        }
        writeln!(w, "{}", json!({"t": "list", "id": 99})).unwrap();
    });
    let t = Instant::now();
    let mut errors = Vec::new();
    for l in BufReader::new(s).lines() {
        let v: Value = serde_json::from_str(&l.unwrap()).unwrap();
        match v["t"].as_str() {
            Some("list") => break,
            Some("error") => errors.push(v["error"].as_str().unwrap().to_string()),
            _ => {}
        }
    }
    assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
    assert!(!errors.is_empty() && errors.iter().all(|e| e == "Cirno is not reading its input"));
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

#[test]
fn a_ritual_keeps_its_newest_runs_in_departed_and_every_other_record() {
    let d = Daemon::new("trim", &[]);
    for dir in ["residents", "departed", "rituals/rounds"] {
        std::fs::create_dir_all(d.dir.join(dir)).unwrap();
    }
    let rec = |id: &str, ritual: Option<&str>, at: Option<i64>| {
        json!({"id": id, "session": id, "name": "Run", "slot": null, "cwd": "/",
               "program": "claude", "argv": [], "launched": at.unwrap_or(9000),
               "departed": at, "ritual": ritual})
        .to_string()
    };
    for n in 0..55 {
        let id = format!("run{n:02}");
        std::fs::write(
            d.dir.join(format!("departed/{id}.json")),
            rec(&id, Some("rounds"), Some(1000 + n)),
        )
        .unwrap();
    }
    std::fs::write(d.dir.join("departed/mine.json"), rec("mine", None, Some(1))).unwrap();
    // The session a persistent ritual keeps is never trimmed, however old.
    std::fs::write(d.dir.join("rituals/rounds/session"), "run00\n").unwrap();
    // Retired at start, as the newest run.
    std::fs::write(d.dir.join("residents/last.json"), rec("last", Some("rounds"), None)).unwrap();
    d.cli(&["list"]);
    let mut left: Vec<String> = std::fs::read_dir(d.dir.join("departed"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().trim_end_matches(".json").to_string())
        .collect();
    left.sort();
    let mut want: Vec<String> = (6..55).map(|n| format!("run{n:02}")).collect();
    want.extend(["last", "mine", "run00"].map(String::from));
    want.sort();
    assert_eq!(left, want);
    let all =
        d.req(json!({"t": "list", "id": 1, "all": true}))["residents"].as_array().unwrap().len();
    assert_eq!(all, 52);
}

/// `gensokyo <args>` as Claude Code runs it inside a resident: `stdin` in, stdout back.
fn inside(d: &Daemon, args: &[&str], env: &[(&str, &str)], stdin: &[u8]) -> Output {
    let mut c = d.command(args);
    c.env("GENSOKYO_SOCKET", d.socket()).envs(env.iter().copied()).current_dir(&d.dir);
    let mut c = c.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = c.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    out
}

#[test]
fn hook_verbs_always_exit_zero_and_a_hook_prints_nothing() {
    let d = Daemon::new("verbs", &[]);
    for input in [&b"not json"[..], b"{\"hook_event_name\":\"Stop\"}"] {
        assert_eq!(inside(&d, &["_hook"], &[("GENSOKYO_RESIDENT", "x")], input).stdout, b"");
        let out = inside(&d, &["_statusline", "x"], &[], input);
        let want: &[u8] = if input[0] == b'{' { b"Claude\n" } else { b"" };
        assert_eq!(out.stdout, want);
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
        c.send(hello("test"));
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
    c.send(json!({"t": "input", "who": r["id"], "bytes": b"/sync 0\r"}));
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

/// How far back a frame or a damage says the view is.
fn back(ev: &Value) -> Option<u64> {
    match ev["t"].as_str() {
        Some("frame") => ev["frame"]["back"].as_u64(),
        Some("damage") => ev["back"].as_u64(),
        _ => None,
    }
}

#[test]
fn scrollback_is_the_residents_and_typing_brings_it_back_to_the_live_screen() {
    let d = Daemon::start("scroll", &[("STUB_HOOKS", "1")]);
    let cards = d.dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    std::fs::write(cards.join("hi.md"), "---\ntitle: Hi\n---\nSay hi to {self}.").unwrap();
    let r = d.summon(json!({}));
    d.stub(&r["id"], "ready");
    let mut c = Client::new(&d);
    c.send(json!({"t": "view", "who": r["id"]}));
    let mut s = Screen::default();
    s.wait_for(&mut c, "stub-claude");
    // One paste, which the stub echoes whole without a hook per line.
    let lines: Vec<String> = (0..60).map(|i| format!("line{i}")).collect();
    let paste = format!("\x1b[200~{}\x1b[201~\r", lines.join("\r"));
    c.send(json!({"t": "input", "who": r["id"], "bytes": paste.as_bytes()}));
    s.wait_for(&mut c, "> line59");

    c.send(json!({"t": "scroll", "who": r["id"], "rows": -10}));
    let ev = c.until("the view 10 rows back", |ev| s.apply(ev) && back(ev) == Some(10));
    assert!(ev["history"].as_u64().or(ev["frame"]["history"].as_u64()).unwrap() > 10, "{ev}");
    assert!(!s.has("> line59") && s.has("> line49"), "{:?}", s.rows);
    // Another client showing it sees the same rows: the view is the resident's.
    let mut c2 = Client::new(&d);
    c2.send(json!({"t": "view", "who": r["id"]}));
    let f = c2.until("the second viewer's frame", |v| v["t"] == "frame");
    assert_eq!(back(&f), Some(10));
    // A focus report is not typing, and leaves the view where it is.
    c.send(json!({"t": "input", "who": r["id"], "bytes": b"\x1b[I"}));
    c.send(json!({"t": "list", "id": 5}));
    c.until("the list after the focus report", |v| v["t"] == "list");
    let mut c3 = Client::new(&d);
    c3.send(json!({"t": "view", "who": r["id"]}));
    assert_eq!(back(&c3.until("a third frame", |v| v["t"] == "frame")), Some(10));

    c.send(json!({"t": "input", "who": r["id"], "bytes": b"typed\r"}));
    c.until("the live screen", |ev| s.apply(ev) && back(ev) == Some(0));
    // The stub's answer, not the tty's echo: output that comes while scrolled back keeps the
    // view on its rows. It reads the focus report as part of the line, a tab on screen.
    let answer = |s: &Screen| s.rows.iter().any(|r| r.starts_with("> ") && r.contains("typed"));
    c.until("the stub's answer", |ev| s.apply(ev) && answer(&s));
    c2.until("the live screen, for the other viewer too", |ev| back(ev) == Some(0));

    // A card is looked for on the live screen, so casting one scrolls there first.
    c.send(json!({"t": "scroll", "who": r["id"], "rows": -5}));
    c.until("the view 5 rows back", |ev| s.apply(ev) && back(ev) == Some(5));
    let done = d.req(json!({"t": "cast", "id": 6, "card": "hi", "targets": [r["id"]]}));
    assert_eq!(done["t"], "done", "{done}");
    c.until("the live screen after the cast", |ev| s.apply(ev) && back(ev) == Some(0));

    c.send(json!({"t": "scroll", "id": 7, "who": "nobody", "rows": -1}));
    let e = c.until("a scroll error", |v| v["t"] == "error");
    assert_eq!((e["id"].as_u64(), &e["error"]), (Some(7), &json!("no resident nobody")));
    d.req(json!({"t": "banish", "who": r["id"]}));
    c.send(json!({"t": "scroll", "id": 8, "who": r["id"]}));
    let e = c.until("a scroll error for the departed", |v| v["t"] == "error" && v["id"] == 8);
    assert!(e["error"].as_str().unwrap().contains("already departed"), "{e}");
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

#[test]
fn the_registry_is_asked_less_while_everyone_rests_and_again_once_one_types_or_draws() {
    let d = Daemon::start("backoff", &[]);
    let r = d.summon(json!({"name": "Reimu"}));
    let id = r["id"].as_str().unwrap().to_string();
    d.stub(&r["id"], "ready");
    let calls = || {
        let f = std::fs::read_to_string(d.dir.join("stub/agents.calls")).unwrap_or_default();
        f.lines().count()
    };
    // One ask after the stub is up lists it, resting, with nothing in its way.
    let n = calls();
    wait(|| calls() > n, "an ask that lists it");
    let n = calls();
    std::thread::sleep(Duration::from_millis(6500));
    assert_eq!(calls(), n, "asked again while it rested and nothing changed");
    input(&d, &id, "/later");
    common::wait_for(Duration::from_secs(5), || calls() > n, "an ask after the typing");
    // Drawing with nobody typing counts too.
    let n = calls();
    common::wait_for(Duration::from_secs(9), || calls() > n, "an ask after it drew");
}

/// The resident's record as the daemon last wrote it, from the shrine or from `departed/`.
fn record(d: &Daemon, id: &str) -> Value {
    let p = |dir: &str| d.dir.join(format!("{dir}/{id}.json"));
    let f = if p("residents").exists() { p("residents") } else { p("departed") };
    serde_json::from_slice(&std::fs::read(f).unwrap()).unwrap()
}

fn input(d: &Daemon, who: &str, text: &str) {
    let bytes: Vec<u8> = format!("{text}\r").into_bytes();
    // Input is answered only when it fails, so a list after it says it was taken.
    let (mut w, mut lines) = d.connect();
    let req = json!({"t": "input", "id": 1, "who": who, "bytes": bytes});
    writeln!(w, "{req}\n{}", json!({"t": "list", "id": 2})).unwrap();
    let r = next(&mut lines);
    assert_eq!(r["t"], "list", "{r}");
}

#[test]
fn hooks_and_the_registry_light_the_glyphs_and_a_watched_resident_rings_nothing() {
    let d = Daemon::start("aware", &[("STUB_HOOKS", "1")]);
    let mut w = Client::new(&d);
    w.send(json!({"t": "watch"}));
    let r = d.summon(json!({"name": "Reimu"}));
    let id = r["id"].as_str().unwrap().to_string();
    d.stub(&r["id"], "ready");
    let state = |w: &mut Client, want: &str| {
        w.until(&format!("Reimu {want}"), |v| {
            v["t"] == "residents" && v["residents"][0]["state"] == want
        })
    };
    state(&mut w, "resting");

    // A prompt, then its Stop: a finished turn nobody has answered, and nobody watches.
    input(&d, "Reimu", "hello");
    let n = w.until("a notify", |v| v["t"] == "notify");
    assert_eq!(
        n,
        json!({"t": "notify", "who": id, "name": "Reimu", "state": "awaits",
                         "text": "Reimu is done: echo: hello", "watched": false})
    );
    // The shrine's own event may come before the notify or after it.
    let l = &d.list()[0];
    assert_eq!(
        (&l["state"], &l["detail"], &l["mode"]),
        (&json!("awaits"), &json!("echo: hello"), &json!("default"))
    );

    // Someone looks at it in a focused terminal: the finished turn has been seen, and the
    // question it asks rings nothing.
    let mut v = Client::new(&d);
    v.send(json!({"t": "view", "who": "Reimu"}));
    v.until("the screen", |ev| ev["t"] == "frame");
    assert_eq!(d.list()[0]["state"], "awaits", "on screen, but not in a focused terminal");
    v.send(json!({"t": "focus", "on": true}));
    state(&mut w, "resting");
    input(&d, "Reimu", "/ask Tea or coffee?");
    let n = w.until("a notify", |v| v["t"] == "notify");
    assert_eq!(
        (&n["state"], &n["text"], &n["watched"]),
        (&json!("asked"), &json!("Reimu asks: Tea or coffee?"), &json!(true))
    );
    input(&d, "Reimu", "/answer");
    // A turn that finishes while watched is seen as it finishes.
    let stops = |d: &Daemon| d.log().iter().filter(|l| l["event"] == "Stop").count();
    input(&d, "Reimu", "watched");
    wait(|| stops(&d) == 2, "the second Stop");
    assert_eq!(d.list()[0]["state"], "resting");

    // The registry: busy clears the flag, and a dialog it sees rings once the terminal is left.
    std::fs::write(d.dir.join(format!("stub/{id}.status")), "busy").unwrap();
    state(&mut w, "busy");
    v.send(json!({"t": "focus", "on": false}));
    std::fs::write(d.dir.join(format!("stub/{id}.status")), "waiting").unwrap();
    let n = w.until("a notify", |v| v["t"] == "notify");
    assert_eq!(
        (&n["state"], &n["text"], &n["watched"]),
        (&json!("awaits"), &json!("Reimu needs your permission"), &json!(false))
    );
    let events: Vec<_> =
        d.log().into_iter().filter(|l| l["ev"] == "notify").map(|l| l["watched"].clone()).collect();
    assert_eq!(events, [false, true, false]);
}

#[test]
fn the_status_line_reports_and_prints_its_own_line_or_the_users() {
    let d = Daemon::start("statusline", &[]);
    let conf = d.dir.join("conf");
    std::fs::create_dir_all(d.dir.join(".claude")).unwrap();
    std::fs::create_dir_all(&conf).unwrap();
    let settings = json!({"advisorModel": "opus", "statusLine": {"type": "command", "command": "cat; echo tail", "padding": 2}});
    std::fs::write(d.dir.join(".claude/settings.json"), settings.to_string()).unwrap();
    let r = d.summon(json!({"name": "Sakuya"}));
    let id = r["id"].as_str().unwrap().to_string();
    // The user's padding is carried into the statusLine we put in its place.
    assert!(d.stub(&r["id"], "args").contains("\"padding\":2"));

    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/statusline-2.1.260.json");
    let mut j: Value = serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
    // Claude has gone into a subdirectory: the settings are still the project's.
    j["workspace"]["project_dir"] = json!(d.dir);
    j["workspace"]["current_dir"] = json!(d.dir.join("src"));
    // Windows not yet reset, and a cache still warm: past those times they show otherwise.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    j["rate_limits"]["five_hour"]["resets_at"] = json!(now.as_secs() + 3600);
    j["rate_limits"]["seven_day"]["resets_at"] = json!(now.as_secs() + 86400);
    j["prompt_cache"]["expires_at"] = json!(now.as_secs() + 600);
    let payload = j.to_string();
    let env = [("GENSOKYO_CONFIG_DIR", conf.to_str().unwrap())];
    let out = inside(&d, &["_statusline", &id], &env, payload.as_bytes());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "Sonnet 5→⚖ Opus · medium · ▌░░░░░░░░░ 5% of 1M · ⚡93% (turn 99%) · $0.19 · +8/-0 · 5m\n"
    );
    wait(|| !d.list()[0]["telemetry"].is_null(), "the report");
    let t = &d.list()[0]["telemetry"];
    assert_eq!(
        (&t["model"], &t["advisor"], &t["ctx"]),
        (&json!("Sonnet 5"), &json!("opus"), &json!(5))
    );
    let list = String::from_utf8(d.cli(&["list"]).stdout).unwrap();
    assert!(
        list.contains("Sonnet 5→⚖ Opus · ctx 5% · medium · ⚡93% (turn 99%) · $0.19"),
        "{list}"
    );
    assert!(list.contains("\nusage 5h ███▌░░░░░░ 36%"), "{list}");

    // STATUSLINE=user: the user's command gets the same JSON, and its output goes out as it is.
    std::fs::write(conf.join("config"), "# mine\nSTATUSLINE=own\nSTATUSLINE=user\n").unwrap();
    let out = inside(&d, &["_statusline", &id], &env, payload.as_bytes());
    assert_eq!(out.stdout, format!("{payload}tail\n").into_bytes());
}

#[test]
fn a_clear_moves_the_session_on_and_a_recall_after_a_restart_resumes_it_with_its_hooks() {
    let d = Daemon::start("clear", &[("STUB_HOOKS", "1")]);
    let r = d.summon(json!({"name": "Youmu", "prompt": "first"}));
    let id = r["id"].as_str().unwrap().to_string();
    d.stub(&r["id"], "ready");
    input(&d, "Youmu", "/clear");
    wait(|| record(&d, &id)["session"] != json!(id), "the session to move on");
    let cleared = record(&d, &id)["session"].as_str().unwrap().to_string();
    input(&d, "Youmu", "second");
    wait(|| d.log().iter().any(|l| l["ev"] == "hook" && l["event"] == "Stop"), "the Stop");
    let daemon = d.log()[0]["pid"].as_i64().unwrap();
    assert_eq!(d.req(json!({"t": "quit", "id": 3}))["t"], "done");
    wait(|| !alive(daemon), "the daemon to exit");

    // With no daemon to hear it, a hook is spooled; the next daemon replays it.
    let later = "33333333-4444-4555-8666-777777777777";
    let convo = d.dir.join(format!("claude/projects/stub/{later}.jsonl"));
    std::fs::write(&convo, "compacted\n").unwrap();
    let hook = json!({"hook_event_name": "SessionStart", "session_id": later, "source": "compact"});
    inside(&d, &["_hook"], &[("GENSOKYO_RESIDENT", &id)], hook.to_string().as_bytes());
    assert!(d.dir.join("spool.jsonl").exists());
    // And one older than the record's session, which it would otherwise move back to, is dropped.
    let stale = json!({"t": "hook", "resident": id, "hook": {"event": "SessionStart", "session": id, "at": 1}});
    let mut spool =
        std::fs::OpenOptions::new().append(true).open(d.dir.join("spool.jsonl")).unwrap();
    writeln!(spool, "{stale}").unwrap();

    // The resume starts the daemon, which has replayed the spool before it answers.
    d.cli(&["resume", "youmu"]);
    let args = d.stub(&json!(later), "args");
    assert!(args.contains(&format!("--resume {later}")), "{args}");
    assert!(!d.dir.join("spool.jsonl").exists());
    let moves: Vec<_> =
        d.log().iter().filter(|l| l["ev"] == "session").map(|l| l["session"].clone()).collect();
    assert_eq!(moves, [json!(cleared), json!(later)]);
    assert!(args.contains("_hook"), "{args}");
    // Its hooks still reach the shrine, under the same resident.
    wait(
        || {
            d.log()
                .iter()
                .any(|l| l["ev"] == "hook" && l["id"] == json!(id) && l["kind"] == "resume")
        },
        "the resumed session's SessionStart",
    );
}

#[test]
fn a_card_reaches_everyone_free_and_names_whoever_holds_a_dialog() {
    let d = Daemon::start("cast", &[("STUB_HOOKS", "1")]);
    let cards = d.dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    let roll = "---\ntitle: Roll Call\nsummary: who is here\n---\n\nYou are {self}.\nThe others: {residents}.\n";
    std::fs::write(cards.join("roll-call.md"), roll).unwrap();
    std::fs::write(cards.join("pair.md"), "---\ntitle: Pair\npeer: required\n---\nTalk to {peer}.")
        .unwrap();
    let out = d.cli(&["broadcast"]);
    let listing = String::from_utf8_lossy(&out.stdout);
    assert!(listing.contains("roll-call") && listing.contains("who is here"), "{listing}");
    assert!(listing.contains("Pair") && listing.contains("(needs --with <peer>)"), "{listing}");

    let (a, b) = (d.summon(json!({"name": "Reimu"})), d.summon(json!({"name": "Marisa"})));
    d.stub(&a["id"], "ready");
    d.stub(&b["id"], "ready");
    // Marisa at a permission dialog, as Claude Code shows one: its hook, and `waiting`.
    let bid = b["id"].as_str().unwrap();
    std::fs::write(d.dir.join(format!("stub/{bid}.status")), "waiting").unwrap();
    input(&d, "Marisa", "/perm");
    wait(|| d.list()[1]["state"] == "awaits", "Marisa at her dialog");
    let cast = |card: &str, targets: &[&str], peer: Option<&str>| {
        let mut req = json!({"t": "cast", "id": 3, "card": card, "targets": targets});
        if let Some(p) = peer {
            req["peer"] = json!(p);
        }
        let r = d.req(req);
        assert_eq!(r["id"], 3, "{r}");
        (
            r["t"].as_str().unwrap().to_string(),
            r[if r["t"] == "done" { "message" } else { "error" }].as_str().unwrap().to_string(),
        )
    };
    assert_eq!(
        cast("roll", &["all"], None),
        (
            "done".into(),
            "cast Roll Call on Reimu; Marisa has a dialog waiting for you; left out".into()
        )
    );
    let reimu = || d.stub(&a["id"], "input");
    wait(|| reimu().contains("\x1b[201~"), "the whole card");
    assert!(
        reimu().contains("\x1b[200~You are Reimu.\nThe others: Marisa.\x1b[201~\n"),
        "{}",
        reimu()
    );
    assert!(!d.stub(&b["id"], "input").contains("You are"));
    // The card went in as her prompt, and its turn ended.
    wait(|| d.list()[0]["state"] == "awaits", "the card's turn");
    let log: Vec<Value> = d.log().into_iter().filter(|l| l["ev"] == "cast").collect();
    assert_eq!((&log[0]["card"], &log[0]["sent"]), (&json!("roll-call"), &json!(["Reimu"])));

    // Named, she is still not typed into; a card that reaches nobody is an error.
    let (t, m) = cast("Roll Call", &["Marisa"], None);
    assert_eq!(t, "error");
    assert_eq!(
        m,
        "Roll Call reached nobody; Marisa has a dialog waiting for you; not cast at, or the card would answer it"
    );
    // A pair card: one target and a peer, who may be held but is told of.
    assert_eq!(cast("pair", &["Reimu"], None).1, "Pair needs a peer: --with <name>");
    assert_eq!(
        cast("pair", &["Reimu", "Marisa"], Some("Reimu")).1,
        "Pair is cast at one resident, with a peer; 2 were named"
    );
    assert_eq!(cast("pair", &["Reimu"], Some("reimu")).1, "Reimu cannot be its own peer");
    assert_eq!(
        cast("roll", &["Reimu"], Some("Marisa")).1,
        "Roll Call names no peer, so --with has nowhere to go"
    );
    assert_eq!(
        cast("pair", &["1"], Some("Marisa")),
        ("done".into(), "cast Pair on Reimu; Marisa has a dialog waiting for you, so it may not answer until you have seen to that".into())
    );
    wait(|| reimu().contains("Talk to Marisa."), "the pair card");
    let e = d.command(&["broadcast", "nope", "all"]).output().unwrap();
    assert!(!e.status.success());
    assert!(String::from_utf8_lossy(&e.stderr).contains("no spell card 'nope'"));
}

#[test]
fn a_prompt_half_typed_keeps_cards_out_until_it_is_sent_or_cleared() {
    let d = Daemon::start("draft", &[("STUB_HOOKS", "1")]);
    let cards = d.dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    std::fs::write(cards.join("hi.md"), "---\ntitle: Hi\n---\nHello {self}.").unwrap();
    let r = d.summon(json!({"name": "Reimu"}));
    d.stub(&r["id"], "ready");
    let me = || d.list()[0].clone();
    wait(|| me()["state"] == "resting" && me()["blocked"].is_null(), "Reimu free");
    // Keys as the client sends them, legacy and kitty: answered only when they fail.
    let keys = |bytes: &[u8]| {
        let (mut w, mut lines) = d.connect();
        let req = json!({"t": "input", "id": 1, "who": "Reimu", "bytes": bytes});
        writeln!(w, "{req}\n{}", json!({"t": "list", "id": 2})).unwrap();
        assert_eq!(next(&mut lines)["t"], "list");
    };
    let cast = |targets: &[&str]| {
        let r = d.req(json!({"t": "cast", "id": 3, "card": "hi", "targets": targets}));
        r[if r["t"] == "done" { "message" } else { "error" }].as_str().unwrap().to_string()
    };
    let draft = "has a prompt half typed into it (Ctrl-C there clears it)";
    keys(b"half");
    assert_eq!(me()["blocked"], draft);
    assert_eq!(
        cast(&["Reimu"]),
        format!("Hi reached nobody; Reimu {draft}; not cast at, or the card would go in with it")
    );
    assert_eq!(cast(&["all"]), format!("nobody to cast Hi at; Reimu {draft}; left out"));
    // Arrows, Backspace and a focus report leave it as it was; the prompt is still there.
    keys(b"\x1b[D\x7f\x1b[I");
    assert_eq!(me()["blocked"], draft);
    // Ctrl-C clears the line (the tty drops it), and the card goes in alone.
    keys(b"\x03");
    assert!(me()["blocked"].is_null());
    assert_eq!(cast(&["Reimu"]), "cast Hi on Reimu");
    wait(|| d.stub(&r["id"], "input").contains("Hello Reimu."), "the card");
    assert!(!d.stub(&r["id"], "input").contains("half"), "{}", d.stub(&r["id"], "input"));
    // A key for the daemon to encode, then sent: the prompt's own hook clears it.
    let (mut w, mut lines) = d.connect();
    let key =
        json!({"t": "input", "id": 1, "who": "Reimu", "key": {"code": 109, "mods": 0, "event": 1}});
    writeln!(w, "{key}\n{}", json!({"t": "list", "id": 2})).unwrap();
    assert_eq!(next(&mut lines)["t"], "list");
    assert_eq!(me()["blocked"], draft);
    keys(b"ore\r");
    wait(|| me()["blocked"].is_null(), "the prompt sent");
    assert_eq!(cast(&["Reimu"]), "cast Hi on Reimu");
}

/// A resident with scrollback holding an earlier paste, as `[Pasted text …]`, on view.
fn with_a_pasted_past(d: &Daemon) -> (Value, Client, Screen) {
    let r = d.summon(json!({}));
    d.stub(&r["id"], "ready");
    let mut c = Client::new(d);
    c.send(json!({"t": "view", "who": r["id"]}));
    let mut s = Screen::default();
    s.wait_for(&mut c, "stub-claude");
    let lines: Vec<String> = (0..60).map(|i| format!("line{i}")).collect();
    let paste = format!("\x1b[200~{}\x1b[201~\r", lines.join("\r"));
    c.send(json!({"t": "input", "who": r["id"], "bytes": paste.as_bytes()}));
    s.wait_for(&mut c, "> line59");
    // Drawn before its hook says it went in: until then it is a draft, and no card goes in.
    wait(|| d.list().iter().any(|x| x["id"] == r["id"] && x["blocked"].is_null()), "sent");
    (r, c, s)
}

/// A cast at `r` while another client keeps scrolling it back.
fn cast_while_scrolling(d: &Daemon, r: &Value) -> Value {
    use std::sync::atomic::{AtomicBool, Ordering};
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let (st, sock, who) = (stop.clone(), d.socket(), r["id"].clone());
    let wheel = std::thread::spawn(move || {
        let (mut w, _l) = common::connect(&sock);
        while !st.load(Ordering::Relaxed) {
            let _ = writeln!(w, "{}", json!({"t": "scroll", "who": who, "rows": -3}));
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    std::thread::sleep(Duration::from_millis(50));
    let done = d.req(json!({"t": "cast", "id": 6, "card": "hi", "targets": [r["id"]]}));
    stop.store(true, Ordering::Relaxed);
    wheel.join().unwrap();
    done
}

#[test]
fn a_card_is_looked_for_on_the_live_screen_while_someone_scrolls_back() {
    let d = Daemon::start("castscroll", &[("STUB_HOOKS", "1")]);
    let cards = d.dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    std::fs::write(cards.join("hi.md"), "---\ntitle: Hi\n---\nSay hi to {self}.").unwrap();
    // It lands: seen on the live screen, however far back the view is.
    let (r, ..) = with_a_pasted_past(&d);
    let done = cast_while_scrolling(&d, &r);
    assert_eq!(done["t"], "done", "{done}");
    // It never shows: an old paste in the scrollback is not taken for it, and no Enter follows.
    let (r, mut c, mut s) = with_a_pasted_past(&d);
    c.send(json!({"t": "input", "who": r["id"], "bytes": b"/mute\r"}));
    s.wait_for(&mut c, "muted");
    // `/mute` fires no hook: its Enter, a command's, is what leaves the line empty.
    wait(|| d.list().iter().any(|x| x["id"] == r["id"] && x["blocked"].is_null()), "cleared");
    let done = cast_while_scrolling(&d, &r);
    assert!(done["error"].as_str().unwrap_or("").contains("never showed"), "{done}");
    // It may be in the input line all the same: the next card would go in with it.
    let blocked = d.list().into_iter().find(|x| x["id"] == r["id"]).unwrap()["blocked"].clone();
    assert_eq!(blocked, "has a prompt half typed into it (Ctrl-C there clears it)");
    // Muted, it would sit out the quit's /exit.
    d.req(json!({"t": "banish", "id": 7, "who": r["id"]}));
}

#[test]
fn a_dialog_drawn_over_the_card_before_its_enter_keeps_the_enter() {
    // A gap long enough to draw over the card in, well after it shows.
    let d = Daemon::start("castcover", &[("STUB_HOOKS", "1"), ("GENSOKYO_ENTER_GAP_MS", "4000")]);
    let cards = d.dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    std::fs::write(cards.join("hi.md"), "---\ntitle: Hi\n---\nSay hi to {self}.").unwrap();
    let r = d.summon(json!({"name": "Reimu"}));
    d.stub(&r["id"], "ready");
    wait(|| d.list()[0]["blocked"].is_null(), "the registry to list it");
    // A dialog that no hook or registry has told of yet, drawn while the Enter is held. A
    // command's Enter leaves the line empty, so nothing is in the card's way.
    input(&d, "Reimu", "/cover 1.5");
    let done = d.req(json!({"t": "cast", "id": 3, "card": "hi", "targets": ["Reimu"]}));
    assert_eq!(
        done["error"],
        "Hi reached nobody; Reimu has something over its input line, a dialog most likely; the \
         card waits in its input line, not submitted",
        "{done}"
    );
    assert!(!d.stub(&r["id"], "input").contains("Say hi"), "{}", d.stub(&r["id"], "input"));
    // It may be in the input line still: the next card would go in with it.
    assert_eq!(d.list()[0]["blocked"], "has a prompt half typed into it (Ctrl-C there clears it)");
}

#[test]
fn hooks_and_the_status_line_say_where_a_resident_works_and_the_newer_wins() {
    let d = Daemon::start("here", &[]);
    // A repo on main, and a worktree of it on nebel95/fix, as git lays them out.
    let (repo, wt) = (d.dir.join("repo"), d.dir.join("repo/.claude/worktrees/fix"));
    std::fs::create_dir_all(repo.join(".git/worktrees/fix")).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(repo.join(".git/worktrees/fix/HEAD"), "ref: refs/heads/nebel95/fix\n").unwrap();
    std::fs::write(
        wt.join(".git"),
        format!("gitdir: {}\n", repo.join(".git/worktrees/fix").display()),
    )
    .unwrap();
    let r = d.summon(json!({"name": "Youmu", "cwd": repo}));
    let id = r["id"].as_str().unwrap().to_string();
    d.stub(&r["id"], "ready");
    let me = || d.list().into_iter().find(|r| r["id"] == json!(id)).unwrap();
    assert_eq!((me()["branch"].clone(), me().get("here").cloned()), (json!("main"), None));

    // Claude went into the worktree: its next hook says so.
    let env = [("GENSOKYO_RESIDENT", id.as_str())];
    let hook = json!({"hook_event_name": "Stop", "cwd": wt, "last_assistant_message": "done"});
    inside(&d, &["_hook"], &env, hook.to_string().as_bytes());
    wait(|| me()["branch"] == "nebel95/fix", "the branch to follow the hook");
    assert_eq!(me()["here"], json!(wt));
    assert_eq!(me()["cwd"], json!(repo), "the launch dir stays, for recall");

    // And back out, by the status line; then a hook from before that, late from the spool,
    // changes nothing.
    let line = json!({"workspace": {"current_dir": repo, "project_dir": repo}});
    inside(&d, &["_statusline", &id], &env, line.to_string().as_bytes());
    wait(|| me()["branch"] == "main", "the branch to follow the status line");
    assert_eq!(me().get("here"), None);
    assert_eq!(me()["telemetry"].get("dir"), None, "taken out as here");
    // The status line says the same again; a hook from before that but after the move, late
    // from the spool, changes nothing. One connection keeps them in order.
    let ms = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let mid = ms().as_millis() as i64;
    std::thread::sleep(Duration::from_millis(20));
    let (mut w, mut lines) = d.connect();
    let again = json!({"t": "statusline", "resident": id, "telemetry": {"dir": repo}});
    let stale =
        json!({"t": "hook", "resident": id, "hook": {"event": "Stop", "at": mid, "cwd": wt}});
    writeln!(w, "{again}\n{stale}\n{}", json!({"t": "list", "id": 2})).unwrap();
    let after = next(&mut lines)["residents"].clone();
    assert_eq!(after[0]["branch"], "main", "{after}");
}

#[test]
fn a_restart_sweeps_answers_whose_record_is_gone() {
    let d = Daemon::new("answers", &[]);
    for dir in ["departed", "answers"] {
        std::fs::create_dir_all(d.dir.join(dir)).unwrap();
    }
    let rec = json!({"id": "kept", "session": "kept", "name": "Cirno", "slot": null, "cwd": "/",
                     "program": "claude", "argv": [], "launched": 1, "departed": 2});
    std::fs::write(d.dir.join("departed/kept.json"), rec.to_string()).unwrap();
    for id in ["kept", "stray"] {
        std::fs::write(d.dir.join(format!("answers/{id}.json")), "{}").unwrap();
    }
    d.cli(&["list"]);
    assert!(d.dir.join("answers/kept.json").exists());
    assert!(!d.dir.join("answers/stray.json").exists());
}

#[test]
fn the_log_moves_aside_once_past_its_size() {
    let d = Daemon::start("logsize", &[("GENSOKYO_LOG_BYTES", "2000")]);
    // Each new asker in another protocol is one line.
    for i in 0..40 {
        talk(&d, 99, &format!("asker{i}"), &[]);
    }
    let old = std::fs::read_to_string(d.dir.join("daemon.log.1")).unwrap();
    assert!(old.lines().all(|l| serde_json::from_str::<Value>(l).is_ok()), "{old}");
    let now = std::fs::metadata(d.dir.join("daemon.log")).unwrap().len();
    assert!(now < 2000, "{now} bytes");
    talk(&d, 99, "last", &[]);
    assert!(d.log().iter().any(|e| e["who"] == "last"), "{:?}", d.log());
}

#[test]
fn a_release_whose_selection_went_says_so_and_a_resident_cannot_select() {
    let d = Daemon::start("select", &[]);
    let r = d.summon(json!({}));
    d.stub(&r["id"], "ready");
    let release = json!({"t": "select", "id": 2, "who": r["id"], "how": "release", "x": 0, "y": 0});
    // No selection held, as after a resize let it go: nothing copied, and why.
    let e = d.req(release.clone());
    assert!(e["error"].as_str().unwrap().starts_with("nothing copied: the selection went"), "{e}");
    let me = json!({"t": "hello", "proto": PROTO, "who": "t", "resident": r["id"]});
    let back = raw(&d, &format!("{me}\n{release}\n"));
    let e = back.iter().find(|v| v["id"] == 2).unwrap();
    assert!(e["error"].as_str().unwrap().starts_with("the screens and keyboards"), "{e}");
}

#[test]
fn a_long_login_log_is_emptied_as_the_daemon_starts_and_a_short_one_kept() {
    let d = Daemon::new("loginlog", &[]);
    let log = d.dir.join("login.log");
    std::fs::write(&log, "launchd caught this\n".repeat(60_000)).unwrap();
    d.cli(&["list"]);
    assert_eq!(std::fs::metadata(&log).unwrap().len(), 0);
    std::fs::write(&log, "launchd caught this\n").unwrap();
    d.cli(&["quit"]);
    d.cli(&["list"]);
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "launchd caught this\n");
}
