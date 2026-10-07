//! A daemon that dies without stopping: the next one ends whatever claude it left running,
//! brings its residents back once into their own conversations and keeps the notice for the
//! first client; a second crash soon after leaves them to the user, and a stop is never a crash.

mod common;

use common::{Daemon, fresh, next, wait};
use serde_json::{Value, json};
use std::io::Write;
use std::process::Command;

fn daemon(name: &str, env: &[(&str, &str)]) -> Daemon {
    let env = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let d = Daemon::at(fresh(&format!("crash-{name}")), env);
    d.cli(&["list"]);
    d
}

fn summon(d: &Daemon, extra: Value) -> Value {
    let mut req = json!({"t": "summon", "id": 7, "cwd": d.dir});
    req.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    let r = d.req(req);
    assert_eq!(r["t"], "summoned", "{r}");
    d.stub(&r["resident"]["id"], "ready");
    r["resident"].clone()
}

fn alive(pid: i64) -> bool {
    // SAFETY: signal 0 only asks whether it is there.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// The running daemon's pid: the newest start in its log.
fn pid(d: &Daemon) -> i64 {
    let log = d.log();
    log.iter().rev().find(|e| e["ev"] == "started").unwrap()["pid"].as_i64().unwrap()
}

/// Gone, with `sig` and nothing else.
fn kill(d: &Daemon, sig: i32) {
    let p = pid(d);
    // SAFETY: plain kill(2), on the daemon this test started.
    unsafe { libc::kill(p as i32, sig) };
    wait(|| !alive(p), "the daemon to die");
}

/// What a client that starts watching now is told of a crash, if anything.
fn notice(d: &Daemon) -> Option<String> {
    let (mut w, mut lines) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    loop {
        let v = next(&mut lines);
        match v["t"].as_str() {
            Some("notice") => return Some(v["text"].as_str().unwrap().to_string()),
            Some("residents") => return None,
            _ => {}
        }
    }
}

fn names(d: &Daemon) -> Vec<String> {
    d.list().iter().map(|r| r["name"].as_str().unwrap().to_string()).collect()
}

fn evs(d: &Daemon, ev: &str) -> usize {
    d.log().iter().filter(|e| e["ev"] == ev).count()
}

#[test]
fn a_crash_brings_everyone_back_once_and_one_soon_after_leaves_them_to_the_user() {
    let d = daemon("twice", &[]);
    let reimu = summon(&d, json!({"name": "Reimu", "prompt": "hello"}));
    let cirno = summon(&d, json!({"name": "Cirno"}));
    let id = reimu["id"].as_str().unwrap();
    kill(&d, libc::SIGKILL);
    let ready = d.dir.join(format!("stub/{id}.ready"));
    std::fs::remove_file(&ready).unwrap();

    d.cli(&["list"]);
    // Reimu, under its own id, in its own conversation; Cirno never had one.
    assert_eq!(names(&d), ["Reimu"]);
    assert_eq!(d.list()[0]["id"], reimu["id"]);
    let args = d.stub(&reimu["id"], "args");
    assert!(args.contains(&format!("--resume {id}")), "{args}");
    let gone = d.dir.join(format!("departed/{}.json", cirno["id"].as_str().unwrap()));
    assert!(gone.exists());
    // Nobody watched as it came back: the first client to is told, and only that one.
    let said = notice(&d).expect("a notice");
    assert!(said.contains("crashed") && said.contains("turns off"), "{said}");
    assert!(said.contains("back in their conversations: Reimu"), "{said}");
    assert!(said.contains("Cirno (no conversation to resume)"), "{said}");
    assert_eq!(notice(&d), None);

    kill(&d, libc::SIGKILL);
    d.cli(&["list"]);
    assert!(names(&d).is_empty());
    assert_eq!(evs(&d, "recalled"), 1);
    let said = notice(&d).expect("a notice");
    assert!(said.contains("Reimu") && said.contains("recall them by hand"), "{said}");
    // By hand, it comes back.
    d.cli(&["resume", "Reimu"]);
    assert_eq!(names(&d), ["Reimu"]);
}

#[test]
fn a_crash_past_the_window_is_recovered_again() {
    let d = daemon("window", &[("GENSOKYO_CRASH_WINDOW_MS", "0")]);
    let r = summon(&d, json!({"name": "Reimu", "prompt": "hello"}));
    for _ in 0..2 {
        kill(&d, libc::SIGKILL);
        d.cli(&["list"]);
        assert_eq!(names(&d), ["Reimu"]);
        assert_eq!(d.list()[0]["id"], r["id"]);
    }
    assert_eq!(evs(&d, "recalled"), 2);
}

#[test]
fn a_claude_the_dead_daemon_left_running_is_ended_before_its_resume() {
    // The stub lingers 30 s on the hangup its PTY gives it as the daemon dies.
    let d = daemon("linger", &[("STUB_HUP", "30")]);
    let r = summon(&d, json!({"name": "Reimu", "prompt": "hello"}));
    let old = r["pid"].as_i64().unwrap();
    kill(&d, libc::SIGKILL);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(alive(old), "the old claude left on its own");

    d.cli(&["list"]);
    wait(|| !alive(old), "the old claude to be gone");
    let log = d.log();
    let ended = log.iter().position(|e| e["ev"] == "leftover" && e["pid"] == old);
    let back = log.iter().position(|e| e["ev"] == "recalled" && e["id"] == r["id"]);
    assert!(ended.is_some() && back.is_some() && ended < back, "{log:?}");
    assert_eq!(names(&d), ["Reimu"]);
}

#[test]
fn a_pid_another_process_has_now_is_left_alone() {
    let d = Daemon::at(fresh("crash-pid"), vec![]);
    let mut other = Command::new("sleep").arg("600").spawn().unwrap();
    std::fs::create_dir_all(d.dir.join("residents")).unwrap();
    // Its pid, but not its start time.
    let rec = json!({"id": "old", "session": "old", "name": "Cirno", "slot": 3, "cwd": "/",
                     "program": "claude", "argv": [], "launched": 1, "pid": [other.id(), 1]});
    std::fs::write(d.dir.join("residents/old.json"), rec.to_string()).unwrap();
    d.cli(&["list"]);
    assert!(other.try_wait().unwrap().is_none(), "the daemon signalled someone else's pid");
    assert_eq!(evs(&d, "leftover"), 0);
    assert!(names(&d).is_empty());
    let moved: Value =
        serde_json::from_str(&std::fs::read_to_string(d.dir.join("departed/old.json")).unwrap())
            .unwrap();
    assert!(moved["departed"].is_i64(), "{moved}");
    let said = notice(&d).expect("a notice");
    assert!(said.contains("Cirno (no conversation to resume)"), "{said}");
    let _ = other.kill();
}

#[test]
fn a_stop_of_any_kind_is_never_taken_for_a_crash() {
    let d = daemon("stops", &[]);
    let start = |d: &Daemon| {
        summon(d, json!({"name": "Reimu", "prompt": "hello"}));
    };
    start(&d);
    d.cli(&["quit"]);
    d.cli(&["list"]);
    assert!(names(&d).is_empty());

    start(&d);
    kill(&d, libc::SIGTERM);
    d.cli(&["list"]);
    assert!(names(&d).is_empty());

    // Killed partway through its stop, as launchd does at logout to one that takes too long:
    // the /exit is never heard, so the record is still live when it dies.
    let r = summon(&d, json!({"name": "Reimu", "prompt": "hello"}));
    let eat = d.dir.join(format!("stub/{}.eat-exit", r["id"].as_str().unwrap()));
    std::fs::write(eat, "9\n").unwrap();
    let stops = evs(&d, "stopping");
    // SAFETY: plain kill(2), on the daemon this test started.
    unsafe { libc::kill(pid(&d) as i32, libc::SIGTERM) };
    wait(|| evs(&d, "stopping") > stops, "the stop to begin");
    assert!(d.dir.join(format!("residents/{}.json", r["id"].as_str().unwrap())).exists());
    kill(&d, libc::SIGKILL);
    d.cli(&["list"]);
    assert!(names(&d).is_empty());

    // `restart` brings them back itself.
    start(&d);
    d.cli(&["restart"]);
    assert_eq!(names(&d), ["Reimu"]);

    assert_eq!(evs(&d, "crash"), 0);
    assert_eq!(notice(&d), None);
    assert!(!d.dir.join("run/crash").exists());
}
