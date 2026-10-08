//! Residents behind the Claude Code installed now: the marker on them, and `renew`, which starts
//! each again into its own conversation once it rests. `claude` is a link to a copy of the stub,
//! moved to another copy as the native installer moves it.

mod common;

use common::{Daemon, err, fresh, next, out, wait};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::{Child, Output, Stdio};
use std::time::Duration;

fn daemon(name: &str) -> Daemon {
    let dir = fresh(&format!("rn-{name}"));
    install(&dir, "2.1.282");
    let claude = dir.join("bin/claude").display().to_string();
    let extra = vec![
        ("STUB_HOOKS".to_string(), "1".to_string()),
        ("STUB_STATUS".into(), "1".into()),
        ("STUB_IDLE".into(), "3".into()),
        ("GENSOKYO_CLAUDE".into(), claude),
    ];
    let d = Daemon::at(dir, extra);
    d.cli(&["list"]);
    d
}

/// Claude Code `v` installed: its own copy of the stub, and `claude` moved to it.
fn install(dir: &Path, v: &str) {
    let bin = dir.join("versions").join(v);
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stub-claude"), bin.join("claude"))
        .unwrap();
    std::fs::create_dir_all(dir.join("stub")).unwrap();
    std::fs::write(dir.join("stub/installed"), v).unwrap();
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let _ = std::fs::remove_file(dir.join("bin/claude.new"));
    std::os::unix::fs::symlink(bin.join("claude"), dir.join("bin/claude.new")).unwrap();
    std::fs::rename(dir.join("bin/claude.new"), dir.join("bin/claude")).unwrap();
}

fn summon(d: &Daemon, name: &str) -> Value {
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": name}));
    assert_eq!(r["t"], "summoned", "{r}");
    d.stub(&r["resident"]["id"], "ready");
    r["resident"].clone()
}

fn me(d: &Daemon, name: &str) -> Value {
    d.list().into_iter().find(|r| r["name"] == name).unwrap_or(Value::Null)
}

fn input(d: &Daemon, who: &str, text: &str) {
    let (mut w, mut lines) = d.connect();
    let input = json!({"t": "input", "id": 2, "who": who, "bytes": format!("{text}\r").as_bytes()});
    writeln!(w, "{input}\n{}", json!({"t": "list", "id": 1})).unwrap();
    assert_eq!(next(&mut lines)["t"], "list");
}

/// `gensokyo <args>` from inside resident `me`, started.
fn spawn(d: &Daemon, me: &Value, args: &[&str]) -> Child {
    let mut c = d.command(args);
    c.env("GENSOKYO_SOCKET", d.socket()).env("GENSOKYO_RESIDENT", me["id"].as_str().unwrap());
    c.current_dir(&d.dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    c.spawn().unwrap()
}

fn inside(d: &Daemon, me: &Value, args: &[&str]) -> Output {
    let mut c = spawn(d, me, args);
    drop(c.stdin.take());
    c.wait_with_output().unwrap()
}

/// Started again: another pid, done renewing.
fn renewed(d: &Daemon, name: &str, pid: &Value) -> Value {
    let back = |r: &Value| r["pid"].is_number() && r["pid"] != *pid && r.get("renewing").is_none();
    wait(|| back(&me(d, name)), &format!("{name} renewed"));
    me(d, name)
}

/// Until the daemon has asked the installed claude its version `n` times.
fn asked(d: &Daemon, n: usize) {
    let count = || d.log().iter().filter(|l| l["ev"] == "claude").count();
    wait(|| count() == n, &format!("the installed version, asked {n} times"));
}

fn runs(d: &Daemon, name: &str, v: &str) {
    wait(|| me(d, name)["telemetry"]["version"] == v, &format!("{name} to report {v}"));
}

#[test]
fn those_behind_the_installed_claude_are_marked() {
    let d = daemon("mark");
    summon(&d, "Reimu");
    runs(&d, "Reimu", "2.1.282");
    asked(&d, 1);
    assert_eq!(me(&d, "Reimu").get("outdated"), None);

    install(&d.dir, "2.1.300");
    wait(|| me(&d, "Reimu")["outdated"] == "2.1.300", "the marker");
    assert!(out(&d.cli(&["list"])).contains("⇡ claude 2.1.300"));
    // One summoned since runs the new one.
    summon(&d, "Marisa");
    runs(&d, "Marisa", "2.1.300");
    assert_eq!(me(&d, "Marisa").get("outdated"), None);
    std::thread::sleep(Duration::from_secs(4));
    asked(&d, 2);
}

#[test]
fn a_resting_resident_is_renewed_in_place_into_its_conversation() {
    let d = daemon("rest");
    let r = summon(&d, "Reimu");
    input(&d, "Reimu", "hello");
    wait(|| me(&d, "Reimu")["turns"] == 1, "its turn");
    runs(&d, "Reimu", "2.1.282");
    let before = me(&d, "Reimu");
    asked(&d, 1);
    // Nobody behind yet: nothing to do.
    assert_eq!(out(&d.cli(&["renew"])).trim(), "nobody here is behind claude 2.1.282");

    install(&d.dir, "2.1.300");
    wait(|| me(&d, "Reimu")["outdated"] == "2.1.300", "the marker");
    wait(|| me(&d, "Reimu")["blocked"].is_null(), "the registry to list it");
    assert_eq!(out(&d.cli(&["renew"])).trim(), "renewing Reimu");
    let after = renewed(&d, "Reimu", &before["pid"]);
    assert_eq!(
        (&after["id"], &after["slot"], &after["turns"]),
        (&before["id"], &before["slot"], &json!(1))
    );
    wait(|| d.stub(&r["id"], "args").contains("--resume"), "the resume");
    runs(&d, "Reimu", "2.1.300");
    assert_eq!(me(&d, "Reimu").get("outdated"), None);
    let log = d.log();
    assert!(log.iter().any(|l| l["ev"] == "renewed" && l["resumed"] == true), "{log:?}");
    assert!(!log.iter().any(|l| l["ev"] == "departed"), "it never departed");

    // One named goes whether or not it is behind; a departed one is a recall's.
    wait(|| me(&d, "Reimu")["blocked"].is_null(), "the registry to list it again");
    assert_eq!(out(&d.cli(&["renew", "Reimu"])).trim(), "renewing Reimu");
    let pid = me(&d, "Reimu")["pid"].clone();
    renewed(&d, "Reimu", &pid);
    d.cli(&["banish", "Reimu"]);
    let o = d.command(&["renew", "1"]).output().unwrap();
    assert!(err(&o).contains("Reimu has departed: a recall starts it"), "{}", err(&o));
}

#[test]
fn a_busy_resident_goes_once_it_rests_and_a_leads_wait_hears_nothing_of_it() {
    let d = daemon("busy");
    let lead = summon(&d, "Reimu");
    let mut c = spawn(&d, &lead, &["new", "--json", "--name", "Marisa", "--prompt-file", "-"]);
    c.stdin.take().unwrap().write_all(b"brief\n").unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    wait(|| me(&d, "Marisa")["turns"] == 1, "the brief's turn");
    assert!(inside(&d, &lead, &["wait", "Marisa", "--timeout", "1s"]).status.success());
    // The lead waits on its helper the whole time.
    let held = || d.log().iter().filter(|l| l["ev"] == "wait" && l["ids"].is_array()).count();
    let waiting = spawn(&d, &lead, &["wait", "Marisa", "--timeout", "60s"]);
    wait(|| held() == 2, "the wait held");

    // A turn that ends on its own a few seconds in.
    input(&d, "Marisa", "/hang");
    wait(|| me(&d, "Marisa")["state"] == "busy", "busy");
    let pid = me(&d, "Marisa")["pid"].clone();
    // The lead may renew only its own helpers, and by name.
    let o = inside(&d, &lead, &["renew"]);
    assert!(err(&o).contains("name your helpers"), "{}", err(&o));
    let o = inside(&d, &lead, &["renew", "Marisa"]);
    assert_eq!(out(&o).trim(), "renewing Marisa once they rest", "{}", err(&o));
    assert_eq!(me(&d, "Marisa")["renewing"], true);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(me(&d, "Marisa")["pid"], pid, "not while its turn runs");
    let turns = me(&d, "Marisa")["turns"].clone();
    assert_eq!(turns, 1);
    // The turn's end is the wait's news; the renew after it is none.
    let o = waiting.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(out(&o).contains("\"turns\":2"), "{}", out(&o));
    let mut waiting = spawn(&d, &lead, &["wait", "Marisa", "--timeout", "60s"]);
    wait(|| held() == 3, "the second wait held");
    renewed(&d, "Marisa", &pid);
    std::thread::sleep(Duration::from_millis(500));
    assert!(waiting.try_wait().unwrap().is_none(), "a renew is not news");
    input(&d, "Marisa", "after");
    let o = waiting.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(out(&o).contains("\"turns\":3") && !out(&o).contains("departed"), "{}", out(&o));
}

#[test]
fn a_close_takes_over_from_a_renew_and_it_stays_departed() {
    let d = daemon("close");
    summon(&d, "Reimu");
    input(&d, "Reimu", "/hang");
    wait(|| me(&d, "Reimu")["state"] == "busy", "busy");
    assert_eq!(out(&d.cli(&["renew", "Reimu"])).trim(), "renewing Reimu once they rest");
    d.cli(&["close", "Reimu"]);
    wait(|| me(&d, "Reimu")["state"] == "departed", "the close");
    // Its turn would have ended by now, and the renew gone ahead.
    std::thread::sleep(Duration::from_secs(5));
    let r = me(&d, "Reimu");
    assert_eq!((&r["state"], r.get("renewing")), (&json!("departed"), None));
    assert!(!d.log().iter().any(|l| l["ev"] == "renewed"));
}
