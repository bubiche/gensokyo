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

/// `gensokyo wait <args>` from inside `lead`, in the background, once the daemon holds it.
fn waiting(d: &Daemon, lead: &Value, args: &[&str]) -> Child {
    let held = || d.log().iter().filter(|l| l["ev"] == "wait" && l["ids"].is_array()).count();
    let n = held();
    let c = spawn(d, lead, args);
    wait(|| held() > n, "the wait held");
    c
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
fn wait_tells_how_full_each_helpers_context_is_once_its_status_line_has_said() {
    let d = daemon("ctx", &[]);
    let lead = summon(&d, "Reimu");
    let m = helper(&d, &lead, "Marisa");
    helper(&d, &lead, "Sanae");
    let (mut w, _lines) = d.connect();
    let line = json!({"t": "statusline", "resident": m["id"], "telemetry": {"ctx": 72}});
    writeln!(w, "{line}").unwrap();
    wait(|| me(&d, "Marisa")["telemetry"]["ctx"] == 72, "Marisa's status line");

    let (code, l) = waited(inside(&d, &lead, &["wait", "Marisa", "Sanae", "--timeout", "5s"]));
    assert_eq!(code, 0, "{l:?}");
    assert_eq!((&l[0]["name"], &l[0]["ctx"]), (&json!("Marisa"), &json!(72)));
    // No status line yet: not known, so not there at all.
    assert_eq!(l[1]["name"], "Sanae");
    assert!(l[1].get("ctx").is_none(), "{}", l[1]);
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
fn a_bare_read_is_the_last_answer_alone_or_why_there_is_none() {
    let d = daemon("bare", &[("STUB_IDLE", "1")]);
    summon(&d, "Reimu");
    let bare = || d.req(json!({"t": "read", "id": 3, "who": "Reimu", "bare": true}));
    assert_eq!(bare()["error"], "Reimu has not finished a turn yet");
    input(&d, "Reimu", "hello there");
    wait(|| me(&d, "Reimu")["turns"] == 1, "its turn");
    let r = bare();
    assert_eq!((&r["t"], &r["message"]), (&json!("done"), &json!("echo: hello there")));
    input(&d, "Reimu", "/fail");
    wait(|| me(&d, "Reimu")["turns"] == 2, "the failed turn");
    assert_eq!(bare()["error"], "Reimu's last turn stopped on an API error: no answer to copy");
    input(&d, "Reimu", "/hang");
    wait(|| me(&d, "Reimu")["turns"] == 3, "the turn idle_prompt ended");
    assert_eq!(bare()["error"], "Reimu's last turn ended with no answer kept");
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
fn a_lead_closes_several_helpers_at_once_and_one_refused_leaves_the_rest_done() {
    let d = daemon("many", &[]);
    let lead = summon(&d, "Reimu");
    summon(&d, "Youmu");
    for name in ["Ran", "Chen", "Mystia"] {
        helper(&d, &lead, name);
    }
    let o = inside(&d, &lead, &["close", "Ran", "Youmu", "Chen", "Ran"]);
    assert!(!o.status.success());
    assert_eq!(out(&o), "Ran has left (/exit)\nChen has left (/exit)\n");
    assert_eq!(err(&o), "gensokyo: Youmu is not one of your helpers; only the user can do that\n");
    wait(|| ["Ran", "Chen"].iter().all(|n| me(&d, n)["state"] == "departed"), "both gone");
    assert_ne!(me(&d, "Youmu")["state"], "departed");

    let o = inside(&d, &lead, &["banish", "Mystia", "Nobody"]);
    assert!(!o.status.success());
    assert_eq!(out(&o), "banished Mystia\n");
    assert!(err(&o).contains("Nobody is not one of your helpers"), "{}", err(&o));
    // Departed, they leave the shrine.
    let o = inside(&d, &lead, &["close", "Ran", "Mystia"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(out(&o), "closed Ran\nclosed Mystia\n");
    let names: Vec<Value> = d.list().iter().map(|r| r["name"].clone()).collect();
    assert_eq!(names, [json!("Reimu"), json!("Youmu"), json!("Chen")]);
}

/// What `_hook` inside `me` prints for a SessionStart from `source`.
fn session_start(d: &Daemon, me: &Value, source: &str) -> String {
    let mut c = spawn(d, me, &["_hook"]);
    let hook = json!({"hook_event_name": "SessionStart", "session_id": me["id"], "source": source});
    c.stdin.take().unwrap().write_all(hook.to_string().as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    out(&o)
}

/// The context a SessionStart hook added, from what it printed.
fn context(printed: &str) -> String {
    let v: Value = serde_json::from_str(printed).unwrap_or_else(|_| panic!("{printed}"));
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart", "{v}");
    v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string()
}

#[test]
fn a_lead_hears_of_its_helpers_after_a_compact_or_clear_and_its_news_waits_still() {
    let d = daemon("brief", &[]);
    let lead = summon(&d, "Reimu");
    helper(&d, &lead, "Marisa");
    for source in ["compact", "clear"] {
        let told = context(&session_start(&d, &lead, source));
        assert!(told.contains("helpers you summoned"), "{told}");
        assert!(told.contains("- Marisa: resting, in "), "{told}");
        assert!(told.contains("a turn you have not read: `gensokyo read Marisa`"), "{told}");
        // Never what the helper wrote.
        assert!(!told.contains("echo:"), "{told}");
    }
    // A start that keeps the conversation, a helper, and a lead of nobody hear nothing.
    assert_eq!(session_start(&d, &lead, "startup"), "");
    assert_eq!(session_start(&d, &lead, "resume"), "");
    let marisa = me(&d, "Marisa");
    assert_eq!(session_start(&d, &marisa, "compact"), "");
    let other = summon(&d, "Youmu");
    assert_eq!(session_start(&d, &other, "compact"), "");

    // Told of, its turn is not collected: the wait still has it, and after that it is read.
    let (code, l) = waited(inside(&d, &lead, &["wait", "Marisa", "--timeout", "5s"]));
    assert_eq!((code, l[0]["news"].clone()), (0, json!(true)), "{l:?}");
    let told = context(&session_start(&d, &lead, "compact"));
    assert!(told.contains("- Marisa: resting, in ") && !told.contains("not read"), "{told}");

    // Departed, it is named until its lead is told so; then nobody is left to name.
    assert!(inside(&d, &lead, &["close", "Marisa"]).status.success());
    wait(|| me(&d, "Marisa")["state"] == "departed", "gone");
    let told = context(&session_start(&d, &lead, "compact"));
    assert!(told.contains("- Marisa: departed, in "), "{told}");
    let (code, _) = waited(inside(&d, &lead, &["wait", "Marisa", "--timeout", "5s"]));
    assert_eq!(code, 0);
    assert_eq!(session_start(&d, &lead, "compact"), "");
}

#[test]
fn a_lead_with_a_ledger_is_pointed_back_to_it_after_a_compact_or_clear() {
    let d = daemon("ledger", &[]);
    let roles = d.dir.join("conf/roles");
    std::fs::create_dir_all(&roles).unwrap();
    std::fs::write(roles.join("cos.md"), "---\nledger: resident\n---\nKeep track.\n").unwrap();
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Reimu", "role": "cos"}));
    assert_eq!(r["t"], "summoned", "{r}");
    let lead = r["resident"].clone();
    d.stub(&lead["id"], "ready");
    let args = d.stub(&lead["id"], "args");
    let i = args.find("ledger is `").expect("a ledger") + "ledger is `".len();
    let file = &args[i..i + args[i..].find('`').unwrap()];
    let pointer = format!("Your ledger is `{file}`: read it before anything else.");

    // No helpers yet: the ledger alone, and only when the conversation was taken.
    for source in ["compact", "clear"] {
        let told = context(&session_start(&d, &lead, source));
        assert_eq!(told, pointer, "{source}");
    }
    assert_eq!(session_start(&d, &lead, "startup"), "");
    assert_eq!(session_start(&d, &lead, "resume"), "");

    // A helper in the same role keeps no ledger: what it has in flight is its lead's.
    let mut c = spawn(&d, &lead, &["new", "--json", "--name", "Marisa", "--role", "cos"]);
    c.stdin.take().unwrap().write_all(b"").unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    let marisa: Value = serde_json::from_str(&out(&o)).unwrap();
    d.stub(&marisa["id"], "ready");
    let args = d.stub(&marisa["id"], "args");
    assert!(!args.contains("ledger is `") && !args.contains(&format!("Read(/{file})")), "{args}");
    assert_eq!(session_start(&d, &marisa, "compact"), "");

    // With helpers, the ledger first and then who is still out.
    let told = context(&session_start(&d, &lead, "compact"));
    let (first, rest) = told.split_once('\n').unwrap();
    assert_eq!(first, pointer);
    assert!(rest.contains("helpers you summoned") && rest.contains("- Marisa: "), "{told}");
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
fn a_helper_works_in_its_leads_mode_or_narrower_and_is_let_only_read() {
    let d = daemon("mode", &[]);
    let r = d
        .req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Reimu", "mode": "acceptEdits"}));
    let lead = r["resident"].clone();
    d.stub(&lead["id"], "ready");
    let new = |args: &[&str]| {
        let mut a = vec!["new", "--json", "--name", args[0]];
        a.extend(&args[1..]);
        inside(&d, &lead, &a)
    };
    let mode = |r: &Output| {
        let r: Value = serde_json::from_str(&out(r)).unwrap();
        let args = d.stub(&r["id"], "args");
        args.split(" --permission-mode ").nth(1).map(|m| m.split(' ').next().unwrap().to_string())
    };
    for wide in ["bypassPermissions", "auto", "dontAsk"] {
        let o = new(&["Cirno", "-p", wide]);
        let want =
            format!("a helper works in your mode (acceptEdits) or a narrower one, so not {wide}");
        assert!(err(&o).contains(&want), "{wide}: {}", err(&o));
    }
    // None named is the lead's own, never the user's defaultMode.
    let o = new(&["Marisa"]);
    assert_eq!(mode(&o).as_deref(), Some("acceptEdits"), "{}", err(&o));
    let o = new(&["Sanae", "-p", "plan"]);
    assert_eq!(mode(&o).as_deref(), Some("plan"), "{}", err(&o));

    for t in ["Bash", "Edit", "Read(x),Bash", "Read(x) Bash", "mcp__x__read", "Read(a)(b)"] {
        let o = new(&["Cirno", "--allowed-tools", t]);
        assert!(
            err(&o).contains("may be let use only Read, Glob, Grep, WebSearch, WebFetch"),
            "{t}: {}",
            err(&o)
        );
    }
    let o = new(&[
        "Chen",
        "--allowed-tools",
        "Grep",
        "--allowed-tools",
        "WebFetch(domain:example.com)",
    ]);
    assert!(o.status.success(), "{}", err(&o));

    // A role by its name only: claude reads a role file past every Read rule.
    let secret = d.dir.join("secret.md");
    std::fs::write(&secret, "Be odd.").unwrap();
    let o = new(&["Cirno", "--role", secret.to_str().unwrap()]);
    assert!(err(&o).contains("a helper you summon takes a role by its name"), "{}", err(&o));
    let o = new(&["Rumia", "--role", "reviewer"]);
    assert!(o.status.success(), "{}", err(&o));

    // Rituals run and resume at the user's hand.
    let o = inside(&d, &lead, &["ritual", "run", "tea"]);
    assert!(err(&o).contains("the user runs and resumes rituals"), "{}", err(&o));
    let mut s = std::os::unix::net::UnixStream::connect(d.socket()).unwrap();
    let mut me = common::hello("cli");
    me["resident"] = lead["id"].clone();
    let enable = json!({"t": "ritual", "id": 3, "verb": "enable", "name": "tea"});
    writeln!(s, "{me}\n{enable}").unwrap();
    let mut lines = std::io::BufRead::lines(std::io::BufReader::new(s));
    let r = watch_for(&mut lines, |v| v["id"] == 3);
    assert!(r["error"].as_str().unwrap_or_default().contains("the user runs and resumes"), "{r}");

    // The user's own summon is theirs to make.
    let wide = json!({"mode": "bypassPermissions", "allowed_tools": ["Bash"]});
    let mut r = json!({"t": "summon", "id": 8, "cwd": d.dir, "name": "Youmu"});
    r.as_object_mut().unwrap().extend(wide.as_object().unwrap().clone());
    let r = d.req(r);
    assert_eq!(r["t"], "summoned", "{r}");
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
    let bg = waiting(&d, &lead, &["wait", "Marisa", "--timeout", "15s"]);
    input(&d, "Marisa", "two");
    let (code, l) = waited(bg.wait_with_output().unwrap());
    assert_eq!((code, &l[0]["answer"]), (0, &json!("echo: two")));
    assert_eq!(me(&d, "Marisa")["state"], "resting");
    // Its dialogs ring all the same, waited on or not: the lead cannot answer them.
    let bg = waiting(&d, &lead, &["wait", "Marisa", "--until", "done", "--timeout", "15s"]);
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
    let helpers = || d.list().iter().filter(|r| r["owner"].is_string()).count();
    // Gone, then back an hour later: the clock stops, and when it goes again it starts over.
    d.cli(&["banish", "Reimu"]);
    settle();
    clock(&d, T0 + 3600);
    d.cli(&["resume", "Reimu"]);
    settle();
    d.cli(&["banish", "Reimu"]);
    settle();
    clock(&d, T0 + 2 * 3600 + 60);
    settle();
    assert_eq!(helpers(), 2, "two hours from the first departure, one from the second");

    // A restart brings everyone back, the lead too.
    d.cli(&["resume", "Reimu"]);
    let o = d.command(&["restart"]).stdin(Stdio::null()).output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    wait(|| d.list().iter().filter(|r| r["pid"].is_number()).count() == 3, "the comeback");

    // Gone for good: two hours of the helpers' idling later, they are closed.
    d.cli(&["close", "Reimu"]);
    settle();
    clock(&d, T0 + 4 * 3600);
    settle();
    assert_eq!(helpers(), 2, "not yet");
    clock(&d, T0 + 4 * 3600 + 180);
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
    let mut bg = waiting(&d, &lead, &["wait", "Marisa", "Sanae", "--timeout", "1m"]);
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
    let bg = waiting(&d, &lead, &["wait", "Marisa", "--until", "needs", "--timeout", "15s"]);
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
    let o = inside(&d, &lead, &["wait", "Sanae", "--timeout", "1m"]);
    assert_eq!(o.status.code(), Some(1), "told already");
    assert!(err(&o).contains("nobody to wait on"), "{}", err(&o));
    // Waited on with one still working, it is left out: the other's turn is enough.
    let bg = waiting(&d, &lead, &["wait", "Marisa", "Sanae", "--timeout", "1m"]);
    input(&d, "Marisa", "three");
    let (code, l) = waited(bg.wait_with_output().unwrap());
    assert_eq!((code, &l[0]["news"], &l[1]["news"]), (0, &json!(true), &json!(false)), "{l:?}");
}

#[test]
fn a_helper_closed_mid_turn_reports_that_turn_cut_short_not_the_one_before() {
    let d = daemon("left", &[("STUB_IDLE", "600")]);
    let lead = summon(&d, "Reimu");
    let m = helper(&d, &lead, "Marisa");
    assert_eq!(waited(inside(&d, &lead, &["wait", "Marisa", "--timeout", "5s"])).0, 0, "brief");
    input(&d, "Marisa", "/hang");
    status(&d, &m, "busy");
    wait(|| me(&d, "Marisa")["state"] == "busy", "at work");
    assert!(inside(&d, &lead, &["close", "Marisa"]).status.success());
    let (code, l) =
        waited(inside(&d, &lead, &["wait", "Marisa", "--until", "done", "--timeout", "5s"]));
    assert_eq!(
        (code, &l[0]["turns"], &l[0]["ended"], &l[0]["answer"]),
        (0, &json!(2), &json!("interrupted"), &Value::Null),
        "{l:?}"
    );
    let o = inside(&d, &lead, &["read", "Marisa"]);
    assert!(out(&o).contains("turn 2 (interrupted)"), "{}", out(&o));
    // The answer alone, for the user's clipboard: there is none.
    let r = d.req(json!({"t": "read", "id": 3, "who": "Marisa", "bare": true}));
    assert_eq!(
        r["error"],
        "Marisa's last turn was cut short, at a dialog or by a restart: no answer to copy"
    );
    // A restart does not count it a second time.
    let o = d.command(&["restart"]).stdin(Stdio::null()).output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    wait(|| me(&d, "Reimu")["pid"].is_number(), "the lead back");
    assert_eq!(me(&d, "Marisa")["turns"], 2);
}

#[test]
fn a_wait_ends_with_its_lead_and_leaves_its_helpers_turns_to_ring() {
    let d = daemon("leadgone", &[]);
    let lead = summon(&d, "Reimu");
    helper(&d, &lead, "Marisa");
    assert_eq!(waited(inside(&d, &lead, &["wait", "Marisa", "--timeout", "5s"])).0, 0, "brief");
    let (mut w, mut events) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    // Left running in the background as its lead goes: it ends then, not at its timeout.
    let bg = waiting(&d, &lead, &["wait", "Marisa", "--timeout", "1m"]);
    let t = Instant::now();
    d.cli(&["close", "Reimu"]);
    let o = bg.wait_with_output().unwrap();
    assert!(t.elapsed() < Duration::from_secs(20), "held to its timeout");
    assert_eq!(o.status.code(), Some(1), "{}", out(&o));
    assert!(err(&o).contains("you have left"), "{}", err(&o));
    // Nobody collects the helper's next turn, so the user hears of it.
    input(&d, "Marisa", "one");
    let n = watch_for(&mut events, |v| v["t"] == "notify" && v["name"] == "Marisa");
    assert_eq!(n["text"], "Marisa is done: echo: one");
    assert_eq!(me(&d, "Marisa")["state"], "awaits");
}

#[test]
fn a_cast_at_failed_reaches_whoever_an_error_cut_off_and_says_what_stopped_each() {
    let d = daemon("failed", &[]);
    let cards = d.dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    std::fs::write(cards.join("hi.md"), "---\ntitle: Hi\n---\nHello {self}.").unwrap();
    let lead = summon(&d, "Reimu");
    let mine = helper(&d, &lead, "Marisa");
    let other = summon(&d, "Youmu");
    let theirs = helper(&d, &other, "Sakuya");
    let free = summon(&d, "Sanae");
    let fine = summon(&d, "Chen");
    let busy = summon(&d, "Ran");
    let drafted = summon(&d, "Mystia");
    let turns = |n: &str| me(&d, n)["turns"].as_u64().unwrap();
    let fail = |names: &[&str]| {
        let before: Vec<u64> = names.iter().map(|n| turns(n)).collect();
        for n in names {
            input(&d, n, "/fail");
        }
        for (n, t) in names.iter().zip(before) {
            wait(|| turns(n) > t, "the failed turn");
        }
    };
    fail(&["Marisa", "Sakuya", "Sanae", "Ran", "Mystia"]);
    input(&d, "Chen", "hello");
    wait(|| turns("Chen") == 1, "Chen's turn");
    // Ran has gone back to work; Mystia has a prompt half typed.
    status(&d, &busy, "busy");
    wait(|| me(&d, "Ran")["state"] == "busy", "Ran busy");
    let (mut w, mut lines) = d.connect();
    let keys = json!({"t": "input", "id": 2, "who": "Mystia", "bytes": b"half".to_vec()});
    writeln!(w, "{keys}\n{}", json!({"t": "list", "id": 1})).unwrap();
    assert_eq!(next(&mut lines)["t"], "list");
    let draft = "has a prompt half typed into it (Ctrl-C there clears it)";
    assert_eq!(me(&d, "Mystia")["blocked"], draft);

    // The error is in the hook's log line and kept with the answer; a good turn keeps none.
    let failures: Vec<Value> =
        d.log().into_iter().filter(|l| l["ev"] == "hook" && l["event"] == "StopFailure").collect();
    assert_eq!(failures.len(), 5);
    assert!(failures.iter().all(|l| l["error"] == "rate_limit"), "{failures:?}");
    assert!(d.log().iter().filter(|l| l["event"] == "Stop").all(|l| l.get("error").is_none()));
    let kept = |r: &Value| -> Value {
        let f = d.dir.join(format!("answers/{}.json", r["id"].as_str().unwrap()));
        serde_json::from_slice(&std::fs::read(f).unwrap()).unwrap()
    };
    let a = kept(&mine);
    assert_eq!((&a["ended"], &a["error"]), (&json!("failed"), &json!("rate_limit")), "{a}");
    let a = kept(&fine);
    assert!(a["ended"] == "stop" && a.get("error").is_none(), "{a}");

    let cast = |from: Option<&Value>, targets: &[&str]| {
        let mut args = vec!["broadcast", "hi"];
        args.extend(targets);
        let o = match from {
            Some(r) => inside(&d, r, &args),
            None => d.command(&args).stdin(Stdio::null()).output().unwrap(),
        };
        if o.status.success() { out(&o) } else { err(&o) }.trim().to_string()
    };
    // From the user's terminal: everyone cut off and free, helpers too. Busy Ran and Chen,
    // whose turn ended well, are not named; Mystia is, as a group cast names anyone held.
    assert_eq!(
        cast(None, &["failed"]),
        format!(
            "cast Hi on 3 residents; Marisa, Sakuya and Sanae stopped on rate_limit; Mystia {draft}; left out"
        )
    );
    for r in [&mine, &theirs, &free] {
        wait(|| d.stub(&r["id"], "input").contains("Hello"), "the card");
    }
    for r in [&lead, &other, &fine, &busy, &drafted] {
        assert!(!d.stub(&r["id"], "input").contains("Hello"), "{r}");
    }
    // The card's own turns ended well: nobody is cut off now.
    for n in ["Marisa", "Sakuya", "Sanae"] {
        wait(|| kept(&me(&d, n))["ended"] == "stop", "the card's turn");
    }
    assert_eq!(
        cast(None, &["failed"]),
        format!("gensokyo: nobody to cast Hi at; Mystia {draft}; left out")
    );

    // From inside a lead: its own helper and residents nobody leads, never another lead's
    // helper. Its wait collects Marisa's failed turn, which clears her gold: still a target.
    fail(&["Marisa", "Sakuya", "Sanae"]);
    let (code, l) = waited(inside(&d, &lead, &["wait", "Marisa", "--timeout", "5s"]));
    assert_eq!((code, &l[0]["ended"]), (0, &json!("failed")), "{l:?}");
    assert_eq!(me(&d, "Marisa")["state"], "resting");
    assert_eq!(
        cast(Some(&lead), &["failed"]),
        format!(
            "cast Hi on Marisa and Sanae; Marisa and Sanae stopped on rate_limit; Mystia {draft}; left out"
        )
    );
    wait(|| d.stub(&free["id"], "input").matches("Hello Sanae.").count() == 2, "Sanae's card");
    assert_eq!(d.stub(&theirs["id"], "input").matches("Hello").count(), 1);
}

#[test]
fn a_prompt_or_a_clear_after_a_failure_takes_a_resident_out_of_failed_and_a_recall_does_not() {
    let d = daemon("unfailed", &[]);
    let cards = d.dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    std::fs::write(cards.join("hi.md"), "---\ntitle: Hi\n---\nHello {self}.").unwrap();
    let names = ["Sakuya", "Sanae", "Chen", "Ran", "Mystia"];
    let rs: Vec<Value> = names.iter().map(|n| summon(&d, n)).collect();
    for n in names {
        input(&d, n, "/fail");
    }
    for n in names {
        wait(|| me(&d, n)["turns"] == 1, "the failed turn");
    }
    // A fresh conversation.
    input(&d, "Sakuya", "/clear");
    wait(|| d.log().iter().any(|l| l["event"] == "SessionStart" && l["kind"] == "clear"), "clear");
    // A prompt, its turn ended by Esc, which no hook tells of: resting.
    input(&d, "Sanae", "/work");
    wait(|| d.stub(&rs[1]["id"], "input").contains("/work"), "Sanae's prompt");
    status(&d, &rs[1], "idle");
    // A prompt still at work, and one at a permission dialog.
    input(&d, "Chen", "/work");
    status(&d, &rs[2], "busy");
    input(&d, "Ran", "/work");
    input(&d, "Ran", "/perm");
    wait(|| me(&d, "Ran")["state"] == "awaits" && me(&d, "Chen")["state"] == "busy", "Ran, Chen");
    let prompted = |n: &str| {
        d.log()
            .iter()
            .filter(|l| l["event"] == "UserPromptSubmit" && l["id"] == me(&d, n)["id"])
            .count()
    };
    wait(|| prompted("Sanae") == 2, "Sanae's prompt hook");
    wait(|| me(&d, "Sanae")["state"] == "resting", "Sanae resting");
    // Recalled into its conversation, Mystia is still where the error left her.
    d.cli(&["banish", "Mystia"]);
    d.cli(&["resume", "Mystia"]);
    wait(|| d.stub(&rs[4]["id"], "args").contains("--resume"), "the recall");
    wait(
        || me(&d, "Mystia")["blocked"].is_null() && me(&d, "Mystia")["pid"].is_number(),
        "Mystia back",
    );

    let o = d.cli(&["broadcast", "hi", "failed"]);
    assert_eq!(out(&o).trim(), "cast Hi on Mystia; Mystia stopped on rate_limit");
    wait(|| d.stub(&rs[4]["id"], "input").contains("Hello Mystia."), "Mystia's card");
    for r in &rs[..4] {
        assert!(!d.stub(&r["id"], "input").contains("Hello"), "{r}");
    }
}

#[test]
fn a_recall_into_a_conversation_cleared_after_a_failure_is_not_cut_off() {
    let d = daemon("stale", &[]);
    let cards = d.dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    std::fs::write(cards.join("hi.md"), "---\ntitle: Hi\n---\nHello {self}.").unwrap();
    let r = summon(&d, "Reimu");
    input(&d, "Reimu", "/fail");
    wait(|| me(&d, "Reimu")["turns"] == 1, "the failed turn");
    input(&d, "Reimu", "/clear");
    wait(|| d.log().iter().any(|l| l["ev"] == "session" && l["id"] == r["id"]), "the new session");
    d.cli(&["banish", "Reimu"]);
    wait(|| me(&d, "Reimu")["state"] == "departed", "gone");
    // The cleared conversation has a file of its own, so the recall resumes it; the failed
    // answer kept is still the one from before the clear.
    let id = r["id"].as_str().unwrap();
    let file = d.dir.join(format!("residents/{id}.json"));
    let rec: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    let session = rec["session"].as_str().unwrap().to_string();
    assert_ne!(session, id);
    std::fs::write(d.dir.join(format!("claude/projects/stub/{session}.jsonl")), "").unwrap();
    std::fs::write(d.dir.join(format!("stub/{session}.name")), "Reimu\n").unwrap();
    let a: Value =
        serde_json::from_slice(&std::fs::read(d.dir.join(format!("answers/{id}.json"))).unwrap())
            .unwrap();
    assert_eq!(a["ended"], "failed");
    d.cli(&["resume", "Reimu"]);
    // The stub keeps its files under the session it runs as.
    let now = json!(session);
    wait(|| d.stub(&now, "args").contains(&format!("--resume {session}")), "the recall");
    wait(|| me(&d, "Reimu")["blocked"].is_null() && me(&d, "Reimu")["pid"].is_number(), "back");
    let o = d.command(&["broadcast", "hi", "failed"]).stdin(Stdio::null()).output().unwrap();
    assert_eq!(err(&o).trim(), "gensokyo: nobody to cast Hi at");
    assert!(!d.stub(&now, "input").contains("Hello"));
}
