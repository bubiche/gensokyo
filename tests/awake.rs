//! Keeping the Mac from idle-sleeping while work runs: a resident in a turn or a headless run
//! holds it (tests/stub-caffeinate in place of caffeinate), a dialog or a schedule does not, a
//! lull lets it go after the linger, and the setting is the user's, kept in the config.

mod common;

use common::{Daemon, fresh, next, wait};
use serde_json::{Value, json};
use std::io::Write;
use std::time::Duration;

const STUB: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stub-caffeinate");

fn daemon(name: &str, config: Option<&str>, env: &[(&str, &str)]) -> Daemon {
    let dir = fresh(&format!("aw-{name}"));
    if let Some(c) = config {
        std::fs::create_dir_all(dir.join("conf")).unwrap();
        std::fs::write(dir.join("conf/config"), c).unwrap();
    }
    let mut extra = vec![
        ("STUB_HOOKS".to_string(), "1".to_string()),
        ("CLAUDE_CODE_CHILD_SESSION".into(), "1".into()),
        ("GENSOKYO_CAFFEINATE".into(), STUB.into()),
        ("GENSOKYO_AWAKE_LINGER_MS".into(), "300".into()),
    ];
    extra.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let d = Daemon::at(dir, extra);
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

fn input(d: &Daemon, who: &str, text: &str) {
    let bytes: Vec<u8> = format!("{text}\r").into_bytes();
    let (mut w, mut lines) = d.connect();
    let req = json!({"t": "input", "id": 1, "who": who, "bytes": bytes});
    writeln!(w, "{req}\n{}", json!({"t": "list", "id": 2})).unwrap();
    assert_eq!(next(&mut lines)["t"], "list");
}

fn state(d: &Daemon, who: &str) -> String {
    let r = d.list().into_iter().find(|r| r["name"] == who).unwrap();
    r["state"].as_str().unwrap().to_string()
}

/// Each start of the stub caffeinate, by its arguments.
fn calls(d: &Daemon) -> Vec<String> {
    let f = std::fs::read_to_string(d.dir.join("stub/caffeinate.calls")).unwrap_or_default();
    f.lines().map(String::from).collect()
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only asks whether it is there.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// The stub caffeinate started last, while it runs.
fn holding(d: &Daemon) -> Option<i32> {
    let pid = std::fs::read_to_string(d.dir.join("stub/caffeinate.pid")).ok()?;
    pid.trim().parse().ok().filter(|p| alive(*p))
}

fn daemon_pid(d: &Daemon) -> i64 {
    let log = d.log();
    log.iter().rev().find(|e| e["ev"] == "started").unwrap()["pid"].as_i64().unwrap()
}

fn awake(d: &Daemon, on: Option<bool>) -> Value {
    d.req(json!({"t": "awake", "id": 4, "on": on}))
}

/// What a client that starts watching now is told of keeping awake.
fn told(d: &Daemon) -> Value {
    let (mut w, mut lines) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    loop {
        let v = next(&mut lines);
        if v["t"] == "awake" {
            return v;
        }
    }
}

#[test]
fn a_turn_holds_the_mac_awake_and_its_end_lets_go_after_the_linger() {
    let d = daemon("turn", None, &[("STUB_HOLD", "1"), ("GENSOKYO_AWAKE_LINGER_MS", "3000")]);
    let r = summon(&d, json!({"name": "Reimu", "prompt": "work"}));
    wait(|| holding(&d).is_some(), "caffeinate to start");
    let pid = daemon_pid(&d);
    assert_eq!(calls(&d), [format!("-i -w {pid}")]);
    let held = told(&d);
    assert_eq!((held["on"].clone(), held["held"].clone()), (json!(true), json!(true)), "{held}");
    let status = awake(&d, None);
    assert_eq!(status["t"], "done", "{status}");
    assert!(status["message"].as_str().unwrap().contains("holding the Mac awake for Reimu"));

    input(&d, r["id"].as_str().unwrap(), "/finish");
    wait(|| state(&d, "Reimu") != "busy", "the turn to end");
    // Held through the linger, so a turn right after does not start another.
    let c = holding(&d).expect("still held as the turn ends");
    wait(|| !alive(c), "caffeinate to go after the linger");
    assert_eq!(told(&d)["held"], false);
    // One start for the whole turn.
    assert_eq!(calls(&d).len(), 1);
}

#[test]
fn a_resident_at_a_dialog_holds_nothing() {
    let d = daemon("dialog", None, &[]);
    let r = summon(&d, json!({"name": "Marisa"}));
    input(&d, r["id"].as_str().unwrap(), "/perm");
    wait(|| state(&d, "Marisa") == "awaits", "the dialog");
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(calls(&d), Vec::<String>::new());
}

#[test]
fn a_turn_stopped_at_a_dialog_or_a_question_lets_go() {
    let d = daemon("midturn", None, &[("STUB_HOLD", "1")]);
    let r = summon(&d, json!({"name": "Reimu", "prompt": "work"}));
    let id = r["id"].as_str().unwrap();
    wait(|| holding(&d).is_some(), "caffeinate to start");
    let c = holding(&d).unwrap();
    input(&d, id, "/perm");
    wait(|| state(&d, "Reimu") == "awaits", "the dialog");
    wait(|| !alive(c), "caffeinate to go at the dialog");

    // Answered: the turn goes on, and so does the hold; then a question stops it again.
    input(&d, id, "/esc");
    input(&d, id, "/busy 30");
    wait(|| state(&d, "Reimu") == "busy", "the turn again");
    wait(|| holding(&d).is_some_and(|p| p != c), "caffeinate again");
    let c = holding(&d).unwrap();
    input(&d, id, "/ask which way?");
    wait(|| state(&d, "Reimu") == "asked", "the question");
    wait(|| !alive(c), "caffeinate to go at the question");
}

#[test]
fn a_headless_run_holds_the_mac_awake_until_it_is_done() {
    let d = daemon("headless", None, &[("STUB_P_SLEEP", "2")]);
    std::fs::create_dir_all(d.dir.join("conf/rituals")).unwrap();
    let text = format!(
        "---\nschedule: \"@yearly\"\nheadless: true\ncwd: \"{}\"\n---\nSay the time.\n",
        d.dir.display()
    );
    std::fs::write(d.dir.join("conf/rituals/quiet.md"), text).unwrap();
    let r = d.req(json!({"t": "ritual", "id": 3, "verb": "run", "name": "quiet"}));
    assert_eq!(r["t"], "done", "{r}");
    wait(|| holding(&d).is_some(), "caffeinate to start");
    let status = awake(&d, None)["message"].as_str().unwrap().to_string();
    assert!(status.contains("a headless quiet run"), "{status}");
    let journal = || std::fs::read_to_string(d.dir.join("rituals/quiet/journal.jsonl"));
    wait(|| journal().unwrap_or_default().contains("done (headless"), "the run");
    let c = holding(&d).unwrap();
    wait(|| !alive(c), "caffeinate to go");
}

#[test]
fn off_in_the_config_holds_nothing() {
    let d = daemon("off", Some("# mine\nKEEP_AWAKE=off\n"), &[("STUB_HOLD", "1")]);
    summon(&d, json!({"name": "Reimu", "prompt": "work"}));
    wait(|| state(&d, "Reimu") == "busy", "the turn");
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(calls(&d), Vec::<String>::new());
    assert_eq!(told(&d)["on"], false);
    assert!(awake(&d, None)["message"].as_str().unwrap().contains("keep-awake is off"));
}

#[test]
fn the_toggle_lets_go_at_once_takes_hold_again_and_is_kept_in_the_config() {
    let config = "# my settings\nNOTIFY_BELL=off\nKEEP_AWAKE=on\n# the end\n";
    let d = daemon("toggle", Some(config), &[("STUB_HOLD", "1")]);
    summon(&d, json!({"name": "Reimu", "prompt": "work"}));
    wait(|| holding(&d).is_some(), "caffeinate to start");
    let c = holding(&d).unwrap();

    // Off: let go now, not after the linger, and written down with the rest left as it was.
    let r = awake(&d, Some(false));
    assert_eq!(r["t"], "done", "{r}");
    assert!(r["message"].as_str().unwrap().contains("keep-awake is off"), "{r}");
    wait(|| !alive(c), "caffeinate to go");
    let conf = || std::fs::read_to_string(d.dir.join("conf/config")).unwrap();
    assert_eq!(conf(), "# my settings\nNOTIFY_BELL=off\nKEEP_AWAKE=off\n# the end\n");
    assert_eq!(told(&d)["on"], false);

    // Kept past a restart of the daemon.
    d.cli(&["restart"]);
    wait(|| d.list().iter().any(|r| r["name"] == "Reimu" && r["departed"].is_null()), "Reimu back");
    assert_eq!(told(&d)["on"], false);

    // On again with a turn going: held at once.
    let id = d.list()[0]["id"].as_str().unwrap().to_string();
    input(&d, &id, "/busy 3");
    wait(|| state(&d, "Reimu") == "busy", "a turn");
    awake(&d, Some(true));
    wait(|| holding(&d).is_some(), "caffeinate to start again");
    assert_eq!(conf(), "# my settings\nNOTIFY_BELL=off\nKEEP_AWAKE=on\n# the end\n");
}

#[test]
fn a_config_with_no_line_for_it_gets_one_and_none_at_all_is_made() {
    let d = daemon("append", Some("NOTIFY_BELL=off"), &[]);
    awake(&d, Some(false));
    let conf = std::fs::read_to_string(d.dir.join("conf/config")).unwrap();
    assert_eq!(conf, "NOTIFY_BELL=off\nKEEP_AWAKE=off\n");

    let d = daemon("none", None, &[]);
    awake(&d, Some(false));
    let conf = std::fs::read_to_string(d.dir.join("conf/config")).unwrap();
    assert_eq!(conf, "KEEP_AWAKE=off\n");
}

#[test]
fn a_resident_may_ask_but_not_turn_it_on_or_off() {
    let d = daemon("rights", None, &[]);
    let r = summon(&d, json!({"name": "Reimu"}));
    let (mut w, mut lines) = {
        let s = std::os::unix::net::UnixStream::connect(d.socket()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let w = s.try_clone().unwrap();
        (w, std::io::BufRead::lines(std::io::BufReader::new(s)))
    };
    let hello =
        json!({"t": "hello", "proto": gensokyo::proto::PROTO, "who": "t", "resident": r["id"]});
    writeln!(w, "{hello}").unwrap();
    assert_eq!(next(&mut lines)["t"], "welcome");
    writeln!(w, "{}", json!({"t": "awake", "id": 1, "on": false})).unwrap();
    let e = next(&mut lines);
    assert_eq!(e["t"], "error", "{e}");
    assert!(e["error"].as_str().unwrap().contains("the user's"), "{e}");
    writeln!(w, "{}", json!({"t": "awake", "id": 2})).unwrap();
    assert_eq!(next(&mut lines)["t"], "done");
    assert!(!d.dir.join("conf/config").exists());
}

#[test]
fn the_cli_says_and_sets_it() {
    let d = daemon("cli", None, &[]);
    let out = common::out(&d.cli(&["awake"]));
    assert!(out.contains("keep-awake is on"), "{out}");
    let out = common::out(&d.cli(&["awake", "off"]));
    assert!(out.contains("keep-awake is off"), "{out}");
    let out = common::out(&d.cli(&["doctor"]));
    assert!(out.contains("awake") && out.contains("keep-awake is off"), "{out}");
    let refused = d.command(&["awake", "on"]).env("GENSOKYO_RESIDENT", "someone").output().unwrap();
    assert!(!refused.status.success());
}

/// The real caffeinate, as the daemon starts it: the system lists its assertion.
#[test]
fn the_real_caffeinate_asserts_against_idle_sleep() {
    let d = daemon("real", None, &[("STUB_HOLD", "1"), ("GENSOKYO_CAFFEINATE", "")]);
    summon(&d, json!({"name": "Reimu", "prompt": "work"}));
    let pid = daemon_pid(&d);
    let caffeinate = || {
        let ps = ["-P", &pid.to_string(), "caffeinate"];
        let out = std::process::Command::new("pgrep").args(ps).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    wait(|| !caffeinate().is_empty(), "caffeinate to start");
    let c = caffeinate();
    let asserted = || {
        let out = std::process::Command::new("pmset").args(["-g", "assertions"]).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.lines().any(|l| {
            l.contains(&format!("pid {c}(caffeinate)")) && l.contains("PreventUserIdleSystemSleep")
        })
    };
    wait(asserted, "the assertion");
    drop(d);
    wait(|| !alive(c.parse().unwrap()), "caffeinate to go with the daemon");
}
