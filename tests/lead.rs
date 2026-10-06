//! A lead and its helpers: residents another resident summoned, through the CLI from inside it,
//! as its Bash tool runs it. `wait` for their news, `read` their answers, who may touch them,
//! which of their turns ring, and helpers left behind when their lead goes. The ritual clock is
//! moved by hand (`GENSOKYO_NOW_FILE`, ticking every 100 ms).

mod common;

use common::{Daemon, Lines, err, fresh, next, out, wait, wait_for};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

const T0: i64 = 1_790_002_800;

fn daemon(name: &str, env: &[(&str, &str)]) -> Daemon {
    let dir = fresh(&format!("l-{name}"));
    let mut extra = vec![
        ("STUB_HOOKS".to_string(), "1".to_string()),
        ("GENSOKYO_NOW_FILE".into(), dir.join("now").display().to_string()),
        ("GENSOKYO_TICK_MS".into(), "100".into()),
    ];
    extra.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let d = Daemon::at(dir, extra);
    clock(&d, T0);
    d.cli(&["list"]);
    d
}

fn clock(d: &Daemon, t: i64) {
    std::fs::write(d.dir.join("now.tmp"), t.to_string()).unwrap();
    std::fs::rename(d.dir.join("now.tmp"), d.dir.join("now")).unwrap();
}

/// The user's summon.
fn summon(d: &Daemon, name: &str) -> Value {
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": name}));
    assert_eq!(r["t"], "summoned", "{r}");
    d.stub(&r["resident"]["id"], "ready");
    r["resident"].clone()
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

/// A helper `lead` summons, briefed on stdin.
fn helper(d: &Daemon, lead: &Value, name: &str) -> Value {
    let mut c = spawn(d, lead, &["new", "--json", "--name", name, "--prompt-file", "-"]);
    c.stdin.take().unwrap().write_all(b"brief\n").unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    let r: Value = serde_json::from_str(&out(&o)).unwrap();
    d.stub(&r["id"], "ready");
    // Its brief's turn, counted before anyone waits.
    wait(|| me(d, name)["turns"] == 1, "the brief's turn");
    r
}

/// Its line in `list`.
fn me(d: &Daemon, name: &str) -> Value {
    let all = d.req(json!({"t": "list", "id": 1, "all": true}))["residents"].clone();
    let all = all.as_array().unwrap();
    all.iter().find(|r| r["name"] == name).cloned().unwrap_or(Value::Null)
}

fn input(d: &Daemon, who: &str, text: &str) {
    let (mut w, mut lines) = d.connect();
    let input = json!({"t": "input", "id": 2, "who": who, "bytes": format!("{text}\r").as_bytes()});
    writeln!(w, "{input}\n{}", json!({"t": "list", "id": 1})).unwrap();
    assert_eq!(next(&mut lines)["t"], "list");
}

/// A `wait`'s exit code and lines.
fn waited(o: Output) -> (i32, Vec<Value>) {
    let lines = out(&o).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    (o.status.code().unwrap(), lines)
}

fn status(d: &Daemon, r: &Value, s: &str) {
    std::fs::write(d.dir.join(format!("stub/{}.status", r["id"].as_str().unwrap())), s).unwrap();
}

/// Events from a `watch` until `f` picks one, within 15 s.
fn watch_for(lines: &mut Lines, mut f: impl FnMut(&Value) -> bool) -> Value {
    let t = Instant::now();
    loop {
        assert!(t.elapsed() < Duration::from_secs(15), "no such event");
        let v = next(lines);
        if f(&v) {
            return v;
        }
    }
}

fn notify(lines: &mut Lines) -> Value {
    watch_for(lines, |v| v["t"] == "notify")
}

#[test]
fn wait_any_hears_of_turns_that_ended_before_it_was_seen_or_at_a_dialog() {
    let d = daemon("any", &[]);
    let lead = summon(&d, "Reimu");
    let (_, s) = (helper(&d, &lead, "Marisa"), helper(&d, &lead, "Sanae"));
    assert_eq!(me(&d, "Sanae")["owner"], lead["id"]);
    let args = d.stub(&s["id"], "args");
    assert!(args.contains("the resident Reimu summoned you"), "{args}");
    assert!(!d.stub(&lead["id"], "args").contains("You are a helper"));

    // Both briefs ended before the first wait: news, by the counters.
    let (code, l) = waited(inside(&d, &lead, &["wait", "Marisa", "Sanae", "--timeout", "5s"]));
    assert_eq!(code, 0, "{l:?}");
    assert_eq!(
        (&l[0]["news"], &l[0]["ended"], &l[0]["answer"]),
        (&json!(true), &json!("stop"), &json!("done"))
    );
    // Told once: nothing new now.
    let (code, l) =
        waited(inside(&d, &lead, &["wait", "Marisa", "Sanae", "--any", "--timeout", "1s"]));
    assert_eq!((code, &l[0]["news"]), (3, &json!(false)));

    // A turn the user watched end still counts, with no gold for anyone to clear.
    let (mut w, _events) = d.connect();
    writeln!(w, "{}\n{}", json!({"t": "view", "who": "Sanae"}), json!({"t": "focus", "on": true}))
        .unwrap();
    input(&d, "Sanae", "second");
    let (code, l) =
        waited(inside(&d, &lead, &["wait", "Marisa", "Sanae", "--any", "--timeout", "10s"]));
    assert_eq!((code, &l[1]["news"], &l[1]["answer"]), (0, &json!(true), &json!("echo: second")));
    assert_eq!(l[0]["news"], false);
    drop(w);

    // Esc at a dialog ends the turn with no Stop.
    input(&d, "Marisa", "/perm");
    wait(|| me(&d, "Marisa")["state"] == "awaits", "Marisa's dialog");
    input(&d, "Marisa", "/esc");
    let (code, l) =
        waited(inside(&d, &lead, &["wait", "Marisa", "--until", "done", "--timeout", "15s"]));
    assert_eq!((code, &l[0]["ended"]), (0, &json!("interrupted")), "{l:?}");
    assert_eq!(l[0]["answer"], Value::Null, "no answer from the turn before");
    let o = inside(&d, &lead, &["read", "Marisa"]);
    assert!(
        out(&o).contains("turn 2 (interrupted)") && out(&o).contains("cut short"),
        "{}",
        out(&o)
    );
}

#[test]
fn wait_until_needs_hears_of_each_dialog_once() {
    let d = daemon("needs", &[]);
    let lead = summon(&d, "Reimu");
    helper(&d, &lead, "Marisa");
    let needs = |t: &str| {
        waited(inside(&d, &lead, &["wait", "Marisa", "--until", "needs", "--timeout", t]))
    };
    input(&d, "Marisa", "/perm");
    let (code, l) = needs("10s");
    assert_eq!((code, &l[0]["needs"], &l[0]["state"]), (0, &json!(1), &json!("awaits")));
    assert_eq!(needs("1s").0, 3, "the same dialog again");
    // Its turn is still news to a wait for turns: needs did not swallow it.
    let (code, l) =
        waited(inside(&d, &lead, &["wait", "Marisa", "--until", "done", "--timeout", "1s"]));
    assert_eq!((code, &l[0]["turns"]), (0, &json!(1)));
    input(&d, "Marisa", "/esc");
    wait(|| me(&d, "Marisa")["turns"] == 2, "the dismissal");
    input(&d, "Marisa", "/ask Red or blue?");
    assert_eq!(needs("10s").0, 0, "a question is a need too");
}

#[test]
fn turns_answers_and_owner_survive_a_recall_and_a_restart() {
    let d = daemon("survive", &[("STUB_IDLE", "1")]);
    let lead = summon(&d, "Reimu");
    let m = helper(&d, &lead, "Marisa");
    input(&d, "Marisa", "/fail");
    wait(|| me(&d, "Marisa")["turns"] == 2, "the failed turn");
    input(&d, "Marisa", "/hang");
    wait(|| me(&d, "Marisa")["turns"] == 3, "the turn idle_prompt ended");
    input(&d, "Marisa", "/flood 2000000");
    wait(|| me(&d, "Marisa")["turns"] == 4, "a 2 MB answer's turn");
    let a: Value = serde_json::from_slice(
        &std::fs::read(d.dir.join(format!("answers/{}.json", m["id"].as_str().unwrap()))).unwrap(),
    )
    .unwrap();
    let text = a["text"].as_str().unwrap();
    assert!(text.len() < 64 * 1024 + 20 && text.ends_with("x\n[cut at 64 KB]"), "{}", text.len());
    let (_, l) = waited(inside(&d, &lead, &["wait", "Marisa", "--timeout", "1s"]));
    assert_eq!((&l[0]["turns"], &l[0]["answer"]), (&json!(4), &json!("flood 2000000")));

    // The lead may recall its own helper.
    let o = inside(&d, &lead, &["banish", "Marisa"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(inside(&d, &lead, &["resume", "Marisa"]).status.success());
    wait(|| d.stub(&m["id"], "args").contains("--resume"), "the recall");
    assert!(d.stub(&m["id"], "args").contains("the resident Reimu summoned you"));
    let r = me(&d, "Marisa");
    assert_eq!((&r["owner"], &r["turns"]), (&lead["id"], &json!(4)));

    // A restart cuts its turn short: once both are back, the lead hears of that.
    input(&d, "Marisa", "/hang");
    wait(|| me(&d, "Marisa")["turns"] == 5, "the hang's own turn end");
    status(&d, &m, "busy");
    wait(|| me(&d, "Marisa")["state"] == "busy", "busy, as the registry says");
    let o = d.command(&["restart"]).stdin(Stdio::null()).output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    wait(|| me(&d, "Marisa")["state"].as_str().is_some_and(|s| s != "departed"), "the comeback");
    let r = me(&d, "Marisa");
    assert_eq!((&r["owner"], &r["turns"]), (&lead["id"], &json!(6)));
    let lead = me(&d, "Reimu");
    let (code, l) = waited(inside(&d, &lead, &["wait", "Marisa", "--timeout", "5s"]));
    assert_eq!((code, &l[0]["ended"]), (0, &json!("interrupted")), "{l:?}");
}

#[test]
fn a_departed_helper_can_still_be_read_and_only_by_its_lead_or_the_user() {
    let d = daemon("read", &[]);
    let lead = summon(&d, "Reimu");
    let other = summon(&d, "Youmu");
    helper(&d, &lead, "Marisa");
    input(&d, "Marisa", "hello there");
    wait(|| me(&d, "Marisa")["turns"] == 2, "its turn");
    let o = inside(&d, &lead, &["read", "--screen", "Marisa"]);
    assert!(out(&o).contains("> hello there"), "{}", out(&o));
    assert!(inside(&d, &lead, &["close", "Marisa"]).status.success());
    wait(|| me(&d, "Marisa")["state"] == "departed", "gone");
    let o = inside(&d, &lead, &["read", "Marisa"]);
    let text = out(&o);
    assert!(text.starts_with("[Marisa's report, turn 2 (stop)"), "{text}");
    assert!(text.contains("\necho: hello there\n[end of Marisa's report]"), "{text}");
    assert!(d.cli(&["read", "Marisa"]).status.success());
    let o = inside(&d, &other, &["read", "Marisa"]);
    assert!(err(&o).contains("Marisa is not one of your helpers"), "{}", err(&o));
    let o = inside(&d, &lead, &["read", "--screen", "Marisa"]);
    assert!(err(&o).contains("has departed"), "{}", err(&o));
}

#[test]
fn helpers_are_capped_cannot_summon_and_belong_to_their_lead_alone() {
    let d = daemon("cap", &[]);
    std::fs::create_dir_all(d.dir.join("conf")).unwrap();
    std::fs::write(d.dir.join("conf/config"), "HELPERS=2\n").unwrap();
    let lead = summon(&d, "Reimu");
    let other = summon(&d, "Youmu");
    let m = helper(&d, &lead, "Marisa");
    helper(&d, &lead, "Sanae");
    let o = inside(&d, &lead, &["new", "--name", "Cirno"]);
    assert!(err(&o).contains("you have 2 helpers here, the most you may"), "{}", err(&o));
    let o = inside(&d, &json!({"id": "not-a-resident"}), &["new", "--name", "Cirno"]);
    assert!(err(&o).contains("GENSOKYO_RESIDENT names no resident here"), "{}", err(&o));
    let o = inside(&d, &m, &["new", "--name", "Cirno"]);
    assert!(
        err(&o).contains("a helper does not summon residents (Reimu leads you)"),
        "{}",
        err(&o)
    );
    // Over the count by a recall too.
    assert!(inside(&d, &lead, &["banish", "Marisa"]).status.success());
    helper(&d, &lead, "Chen");
    let o = inside(&d, &lead, &["resume", "Marisa"]);
    assert!(err(&o).contains("the most you may"), "{}", err(&o));

    for args in [["banish", "Sanae"], ["close", "Sanae"], ["wait", "Sanae"]] {
        let o = inside(&d, &other, &args);
        assert!(err(&o).contains("Sanae is not one of your helpers"), "{args:?}: {}", err(&o));
    }
    // Another resident's cast at everyone leaves out the helpers it does not lead.
    let cards = d.dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    std::fs::write(cards.join("hi.md"), "---\ntitle: Hi\n---\nHello {self}.").unwrap();
    wait(|| d.list().iter().all(|r| r["blocked"].is_null()), "nobody blocked");
    let o = inside(&d, &other, &["broadcast", "hi", "all"]);
    assert_eq!(out(&o).trim(), "cast Hi on Reimu", "{}", err(&o));
    let o = inside(&d, &lead, &["broadcast", "hi", "all"]);
    assert_eq!(out(&o).trim(), "cast Hi on 3 residents", "{}", err(&o));
}

#[test]
fn a_helpers_turn_rings_only_when_its_lead_will_not_collect_it() {
    let d = daemon("bell", &[]);
    let lead = summon(&d, "Reimu");
    helper(&d, &lead, "Marisa");
    let (mut w, mut events) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    // Nobody waits: it rings, and the lead's read clears the gold.
    input(&d, "Marisa", "one");
    let n = notify(&mut events);
    assert_eq!((&n["name"], &n["text"]), (&json!("Marisa"), &json!("Marisa is done: echo: one")));
    assert!(inside(&d, &lead, &["read", "Marisa"]).status.success());
    wait(|| me(&d, "Marisa")["state"] == "resting", "collected");

    // Its lead waits on it: quiet, and nothing waits on the user afterwards.
    let bg = spawn(&d, &lead, &["wait", "Marisa", "--timeout", "15s"]);
    wait(|| d.log().iter().any(|l| l["ev"] == "hook" && l["event"] == "SessionStart"), "up");
    std::thread::sleep(Duration::from_millis(300));
    input(&d, "Marisa", "two");
    let (code, l) = waited(bg.wait_with_output().unwrap());
    assert_eq!((code, &l[0]["answer"]), (0, &json!("echo: two")));
    assert_eq!(me(&d, "Marisa")["state"], "resting");
    // Its dialogs ring all the same, waited on or not: the lead cannot answer them.
    let bg = spawn(&d, &lead, &["wait", "Marisa", "--until", "done", "--timeout", "15s"]);
    std::thread::sleep(Duration::from_millis(300));
    input(&d, "Marisa", "/perm");
    let n = notify(&mut events);
    assert_eq!(
        (&n["name"], &n["state"]),
        (&json!("Marisa"), &json!("awaits")),
        "the done of 'two' never rang"
    );
    input(&d, "Marisa", "/esc");
    assert_eq!(waited(bg.wait_with_output().unwrap()).0, 0);

    // Done while its lead is busy: held, and rung once the lead's turn ends without it.
    status(&d, &lead, "busy");
    wait(|| me(&d, "Reimu")["state"] == "busy", "the lead busy");
    input(&d, "Marisa", "three");
    // Its brief, one, two, the dismissed dialog, and this.
    wait(|| me(&d, "Marisa")["turns"] == 5, "its turn");
    assert_eq!(me(&d, "Marisa")["state"], "resting", "held, not gold");
    status(&d, &lead, "idle");
    input(&d, "Reimu", "carry on");
    let n = watch_for(&mut events, |v| v["t"] == "notify" && v["name"] == "Marisa");
    assert_eq!(n["text"], "Marisa is done");
    assert_eq!(me(&d, "Marisa")["state"], "awaits");
}

#[test]
fn helpers_left_behind_are_closed_two_hours_on_unless_their_lead_comes_back() {
    let d = daemon("orphan", &[]);
    let lead = summon(&d, "Reimu");
    helper(&d, &lead, "Marisa");
    helper(&d, &lead, "Sanae");
    let settle = || std::thread::sleep(Duration::from_millis(600));
    // Gone, then back an hour later: the clock stops.
    d.cli(&["banish", "Reimu"]);
    settle();
    clock(&d, T0 + 3600);
    d.cli(&["resume", "Reimu"]);
    settle();
    clock(&d, T0 + 3 * 3600);
    settle();
    assert_eq!(d.list().len(), 3, "the lead came back");

    // A restart brings everyone back, the lead too: nothing to close.
    let o = d.command(&["restart"]).stdin(Stdio::null()).output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    wait(|| d.list().iter().filter(|r| r["pid"].is_number()).count() == 3, "the comeback");
    clock(&d, T0 + 6 * 3600);
    settle();
    assert_eq!(d.list().len(), 3);

    // Gone for good: two hours of the helpers' idling later, they are closed.
    d.cli(&["close", "Reimu"]);
    let helpers = || d.list().iter().filter(|r| r["owner"].is_string()).count();
    settle();
    clock(&d, T0 + 8 * 3600 - 60);
    settle();
    assert_eq!(helpers(), 2, "not yet");
    clock(&d, T0 + 8 * 3600 + 60);
    wait_for(Duration::from_secs(30), || helpers() == 0, "both closed");
    assert_eq!(me(&d, "Marisa")["owner"], lead["id"], "in recall, still its lead's");
}

#[test]
fn a_wait_that_ends_without_collecting_leaves_its_news_and_its_bells() {
    let d = daemon("uncollected", &[]);
    let lead = summon(&d, "Reimu");
    helper(&d, &lead, "Marisa");
    helper(&d, &lead, "Sanae");
    let (code, _) = waited(inside(&d, &lead, &["wait", "Marisa", "Sanae", "--timeout", "5s"]));
    assert_eq!(code, 0, "the briefs");
    let (mut w, mut events) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();

    // Waited on for each of two, killed after one finished: that one rings.
    let mut bg = spawn(&d, &lead, &["wait", "Marisa", "Sanae", "--timeout", "1m"]);
    std::thread::sleep(Duration::from_millis(300));
    input(&d, "Marisa", "one");
    wait(|| d.log().iter().any(|l| l["ev"] == "quiet"), "quiet while waited on");
    bg.kill().unwrap();
    bg.wait().unwrap();
    let n = notify(&mut events);
    assert_eq!((&n["name"], &n["text"]), (&json!("Marisa"), &json!("Marisa is done")));

    // Timed out with news: it is still news to the next wait.
    let (code, l) = waited(inside(&d, &lead, &["wait", "Marisa", "Sanae", "--timeout", "1s"]));
    assert_eq!((code, &l[0]["news"], &l[1]["news"]), (3, &json!(true), &json!(false)));
    let (code, l) = waited(inside(&d, &lead, &["wait", "Marisa", "--timeout", "1s"]));
    assert_eq!((code, &l[0]["answer"]), (0, &json!("echo: one")));

    // A wait for dialogs does not keep a finished turn quiet.
    let bg = spawn(&d, &lead, &["wait", "Marisa", "--until", "needs", "--timeout", "15s"]);
    std::thread::sleep(Duration::from_millis(300));
    input(&d, "Marisa", "two");
    let n = notify(&mut events);
    assert_eq!(n["text"], "Marisa is done: echo: two");
    input(&d, "Marisa", "/perm");
    assert_eq!(waited(bg.wait_with_output().unwrap()).0, 0);

    // A departure ends a wait for turns, and only once.
    d.cli(&["banish", "Sanae"]);
    let (code, l) =
        waited(inside(&d, &lead, &["wait", "Sanae", "--until", "done", "--timeout", "5s"]));
    assert_eq!((code, &l[0]["state"]), (0, &json!("departed")));
    let (code, _) = waited(inside(&d, &lead, &["wait", "Sanae", "--any", "--timeout", "1s"]));
    assert_eq!(code, 3, "told already");
}
