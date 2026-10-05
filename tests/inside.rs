//! What a resident may do through the CLI and the socket, as one does from its Bash tool: its
//! hello names it, and the user's screen, keyboard and other residents are not its own.

mod common;

use common::{Daemon, err, fresh, next, out, wait};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Output, Stdio};

fn daemon(name: &str) -> Daemon {
    let d = Daemon::at(fresh(&format!("in-{name}")), vec![("STUB_HOOKS".into(), "1".into())]);
    d.cli(&["list"]);
    d
}

fn summon(d: &Daemon, name: &str) -> Value {
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": name}));
    assert_eq!(r["t"], "summoned", "{r}");
    d.stub(&r["resident"]["id"], "ready");
    r["resident"].clone()
}

/// `gensokyo <args>` from inside resident `me`.
fn inside(d: &Daemon, me: &Value, args: &[&str]) -> Output {
    let mut c = d.command(args);
    c.env("GENSOKYO_SOCKET", d.socket()).env("GENSOKYO_RESIDENT", me["id"].as_str().unwrap());
    c.current_dir(&d.dir).stdin(Stdio::null()).output().unwrap()
}

/// One request on a connection whose hello names `resident`.
fn as_resident(d: &Daemon, resident: &str, req: Value) -> Value {
    let s = std::os::unix::net::UnixStream::connect(d.socket()).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let mut w = s.try_clone().unwrap();
    let mut hello = common::hello("cli");
    hello["resident"] = json!(resident);
    writeln!(w, "{hello}\n{req}").unwrap();
    let mut lines = std::io::BufRead::lines(std::io::BufReader::new(s));
    assert_eq!(next(&mut lines)["t"], "welcome");
    next(&mut lines)
}

fn alive(r: &Value) -> bool {
    unsafe { libc::kill(r["pid"].as_i64().unwrap() as i32, 0) == 0 }
}

#[test]
fn a_resident_may_not_quit_type_into_or_close_the_others() {
    let d = daemon("rights");
    let (a, b) = (summon(&d, "Reimu"), summon(&d, "Marisa"));
    let refused = |args: &[&str], want: &str| {
        let o = inside(&d, &a, args);
        assert!(!o.status.success() && err(&o).contains(want), "{args:?}: {}", err(&o));
    };
    refused(&["quit"], "quit stops every resident, this one too");
    refused(&["close", "reimu"], "Reimu is you: finish your turn instead");
    refused(&["banish", "1"], "Reimu is you");
    refused(&["banish", "Marisa"], "Marisa is not one of your helpers; only the user can do that");
    refused(&["close", "2"], "Marisa is not one of your helpers");
    refused(&["resume", "Marisa"], "Marisa is not one of your helpers");
    refused(&["resume", "Nobody"], "Nobody is not one of your helpers");
    // Seeing and summoning are still its to do.
    assert!(inside(&d, &a, &["list"]).status.success());

    let id = a["id"].as_str().unwrap();
    for req in [
        json!({"t": "input", "id": 2, "who": "Marisa", "bytes": b"hi\r".to_vec()}),
        json!({"t": "view", "id": 2, "who": "Marisa"}),
        json!({"t": "scroll", "id": 2, "who": "Marisa", "rows": -1}),
        json!({"t": "resize", "id": 2, "cols": 40, "rows": 10}),
        json!({"t": "focus", "id": 2, "on": true}),
    ] {
        let r = as_resident(&d, id, req.clone());
        assert_eq!(r["t"], "error", "{req}: {r}");
        assert!(r["error"].as_str().unwrap().contains("the user's"), "{r}");
    }
    // An id that names nobody still has a resident's rights, and owns nobody.
    let r = as_resident(&d, "not-a-resident", json!({"t": "quit", "id": 2}));
    assert_eq!(r["t"], "error", "{r}");
    assert!(alive(&a) && alive(&b));
    assert!(!d.stub(&b["id"], "input").contains("hi"));
    assert_eq!(d.list().len(), 2);

    // The user can.
    let o = d.cli(&["close", "Marisa"]);
    assert!(out(&o).contains("Marisa has left"), "{}", out(&o));
}

#[test]
fn a_resident_casting_at_everyone_is_left_out_itself() {
    let d = daemon("cast");
    let cards = d.dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    std::fs::write(cards.join("hi.md"), "---\ntitle: Hi\n---\nHello {self}.").unwrap();
    let (a, b) = (summon(&d, "Reimu"), summon(&d, "Marisa"));
    wait(|| d.list().iter().all(|r| r["state"] == "resting"), "both resting");
    let o = inside(&d, &a, &["broadcast", "hi", "all"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(out(&o).trim(), "cast Hi on Marisa");
    wait(|| d.stub(&b["id"], "input").contains("Hello Marisa."), "Marisa's card");
    assert!(!d.stub(&a["id"], "input").contains("Hello"));
    let o = inside(&d, &a, &["broadcast", "hi", "Reimu"]);
    assert!(err(&o).contains("Hi reached nobody; Reimu is you; not cast at"), "{}", err(&o));
}

#[test]
fn new_from_a_resident_takes_a_long_prompt_and_its_tools_and_answers_in_json() {
    let d = daemon("new");
    let a = summon(&d, "Reimu");
    let mut c = d.command(&[
        "new",
        "--name",
        "Sanae",
        "--prompt-file",
        "-",
        "--allowed-tools",
        "Read",
        "--allowed-tools",
        "Bash(git log:*)",
        "--json",
    ]);
    c.env("GENSOKYO_SOCKET", d.socket()).env("GENSOKYO_RESIDENT", a["id"].as_str().unwrap());
    let c = c.current_dir(&d.dir).stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut c = c.stderr(Stdio::piped()).spawn().unwrap();
    c.stdin.take().unwrap().write_all(b"-first line\nsecond line\n").unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    let r: Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!((&r["name"], &r["slot"]), (&json!("Sanae"), &json!(2)));
    assert_eq!(d.list()[1]["id"], r["id"]);
    let args = d.stub(&r["id"], "args");
    assert!(args.contains("--allowedTools Read Bash(git log:*) --session-id"), "{args}");
    assert!(args.ends_with("-- -first line\nsecond line\n\n"), "{args:?}");
    // Recalled, it keeps them.
    d.cli(&["banish", "Sanae"]);
    d.cli(&["resume", "Sanae"]);
    wait(|| d.stub(&r["id"], "args").contains("--resume"), "the recall's args");
    let args = d.stub(&r["id"], "args");
    assert!(args.contains("--allowedTools Read Bash(git log:*) --resume"), "{args}");
}

#[test]
fn a_resident_cannot_summon_where_claude_code_was_never_trusted_and_the_user_can() {
    let d = daemon("trust");
    let a = summon(&d, "Reimu");
    let away = d.dir.join("away");
    std::fs::create_dir_all(&away).unwrap();
    let trusted = json!({"projects": {std::fs::canonicalize(&d.dir).unwrap().to_str().unwrap():
                                      {"hasTrustDialogAccepted": true}}});
    std::fs::create_dir_all(d.dir.join("claude")).unwrap();
    std::fs::write(d.dir.join("claude/.claude.json"), trusted.to_string()).unwrap();
    let o = inside(&d, &a, &["new", "/"]);
    assert!(!o.status.success(), "{}", out(&o));
    assert!(
        err(&o).contains("nothing has answered Claude Code's trust prompt for /"),
        "{}",
        err(&o)
    );
    // Below a trusted directory is trusted, as Claude Code counts it.
    assert!(inside(&d, &a, &["new", "away"]).status.success());
    d.cli(&["new", "/"]);
    assert_eq!(d.list().len(), 3);
}
