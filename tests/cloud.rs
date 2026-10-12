//! The cloud courier's pieces: the gate that holds it to one call, and the reply read from its
//! stream rather than from what the model says. The two recorded streams are a real courier's
//! (Claude Code 2.1.296), cut down.

mod common;

use gensokyo::cloud::{self, Routine, Run};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn fixture(name: &str) -> String {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    std::fs::read_to_string(format!("{p}{name}")).unwrap()
}

/// A courier's stream as Claude Code writes it: init, the calls, their results, the end.
fn stream(calls: &[(Value, Value)]) -> String {
    let mut out = vec![json!({"type": "system", "subtype": "init", "tools": ["RemoteTrigger"]})];
    for (i, (input, content)) in calls.iter().enumerate() {
        let id = format!("toolu_{i}");
        out.push(json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": id, "name": "RemoteTrigger", "input": input}]}}));
        out.push(json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "content": content}]}}));
    }
    out.push(json!({"type": "result", "is_error": false, "result": "OK"}));
    out.iter().map(|v| format!("{v}\n")).collect()
}

#[test]
fn the_reply_is_the_tools_own_result_for_the_call_asked() {
    let list = json!({"action": "list"});
    let (code, body) = cloud::reply(&fixture("cloud-stream-list.jsonl"), &list).unwrap();
    assert_eq!((code, body.as_str()), (200, r#"{"data":[],"has_more":false}"#));
    assert_eq!(cloud::routines(&body).unwrap(), (vec![], false));
    // Content in blocks, as a tool result may also come.
    let blocks =
        json!([{"type": "text", "text": "HTTP 404\n{\"error\": {\"message\": \"gone\"}}"}]);
    let (code, body) = cloud::reply(&stream(&[(list.clone(), blocks)]), &list).unwrap();
    assert_eq!((code, cloud::error_of(&body).as_str()), (404, "gone"));
}

#[test]
fn a_courier_that_strays_from_its_one_call_gets_no_reply() {
    let list = json!({"action": "list"});
    let deny = fixture("cloud-stream-deny.jsonl");
    // It called create when sent for list: whatever came back is not list's, and what it
    // wrote is not repeated, only the action.
    let e = cloud::reply(&deny, &list).unwrap_err();
    assert_eq!(e, "the courier tried another call (create), and the gate refused it");
    // Sent for that create, the gate's refusal is the result.
    let create = json!({"action": "create", "body": {}});
    let e = cloud::reply(&deny, &create).unwrap_err();
    assert!(e.contains("hook error: gate: create not allowed"), "{e}");
    let ok = json!("HTTP 200\n{}");
    // A second call met the gate; the first was the one asked for, and stands.
    let then = stream(&[(list.clone(), ok.clone()), (json!({"action": "run"}), json!("denied"))]);
    assert_eq!(cloud::reply(&then, &list).unwrap(), (200, "{}".into()));
    let first = stream(&[(json!({"action": "run"}), json!("denied")), (list.clone(), ok.clone())]);
    assert!(cloud::reply(&first, &list).is_err());
    let extra = json!({"action": "list", "trigger_id": "trig_x"});
    assert!(cloud::reply(&stream(&[(extra, ok)]), &list).is_err());
    let none = stream(&[]);
    assert_eq!(cloud::reply(&none, &list).unwrap_err(), "the courier made no call: OK");
    let bare = stream(&[(list.clone(), json!("Not logged in"))]);
    assert_eq!(cloud::reply(&bare, &list).unwrap_err(), "not an HTTP reply: Not logged in");
}

#[test]
fn the_gate_allows_the_one_call_and_nothing_else() {
    let call = json!({"action": "list_runs", "trigger_id": "trig_01A"});
    let p = |tool: &str, input: Value| json!({"tool_name": tool, "tool_input": input});
    assert_eq!(cloud::gate(&p("RemoteTrigger", call.clone()), &call), None);
    let reordered = json!({"trigger_id": "trig_01A", "action": "list_runs"});
    assert_eq!(cloud::gate(&p("RemoteTrigger", reordered), &call), None);
    for bad in [
        json!({"action": "run", "trigger_id": "trig_01A"}),
        json!({"action": "list_runs", "trigger_id": "trig_02B"}),
        json!({"action": "list_runs", "trigger_id": "trig_01A", "body": {}}),
    ] {
        assert!(cloud::gate(&p("RemoteTrigger", bad.clone()), &call).is_some(), "{bad}");
    }
    assert!(cloud::gate(&p("Bash", call.clone()), &call).is_some());
}

/// The gate's command from a courier's `--settings`, run by a shell as Claude Code runs it: its
/// exit status and what it printed.
fn gate_says(args: &[String], payload: &Value) -> (Option<i32>, String) {
    let at = args.iter().position(|a| a == "--settings").unwrap();
    let settings: Value = serde_json::from_str(&args[at + 1]).unwrap();
    let hook = &settings["hooks"]["PreToolUse"][0];
    assert_eq!(hook["matcher"], "*");
    let cmd = hook["hooks"][0]["command"].as_str().unwrap();
    let mut c = Command::new("sh")
        .args(["-c", cmd])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    let out = c.wait_with_output().unwrap();
    (out.status.code(), String::from_utf8(out.stdout).unwrap())
}

fn run_gate(args: &[String], payload: &Value) -> String {
    let (code, out) = gate_says(args, payload);
    assert_eq!(code, Some(0));
    out
}

#[test]
fn the_courier_carries_its_gate_and_nothing_else() {
    let call = json!({"action": "get_run_log", "session_id": "cse_01A"});
    let args = cloud::args(&call, common::BIN);
    for f in [
        "-p",
        "stream-json",
        "--no-session-persistence",
        "--strict-mcp-config",
        "--disable-slash-commands",
    ] {
        assert!(args.iter().any(|a| a == f), "{f}: {args:?}");
    }
    let tools = args.iter().position(|a| a == "--tools").unwrap();
    assert_eq!(args[tools + 1], "RemoteTrigger");
    assert_eq!(args[args.len() - 2], "--");
    assert!(args.last().unwrap().ends_with(&format!("\n{call}")));
    let p = |input: Value| {
        json!({"hook_event_name": "PreToolUse", "tool_name": "RemoteTrigger",
                                  "tool_input": input})
    };
    assert_eq!(run_gate(&args, &p(call.clone())), "");
    let denied: Value =
        serde_json::from_str(&run_gate(&args, &p(json!({"action": "run"})))).unwrap();
    let out = &denied["hookSpecificOutput"];
    assert_eq!(
        (&out["hookEventName"], &out["permissionDecision"]),
        (&json!("PreToolUse"), &json!("deny"))
    );
    // A gate that cannot run (gensokyo moved by an update) blocks: any status but 2 would let
    // the call through.
    let gone = cloud::args(&call, "/nonexistent/gensokyo");
    assert_eq!(gate_says(&gone, &p(call.clone())).0, Some(2));
    assert!(args.windows(2).any(|w| w[0] == "--max-turns"));
    // Nothing it can read, and no call to allow: refused either way.
    let bad = Command::new(common::BIN).arg("_gate").stdin(Stdio::null()).output().unwrap();
    assert!(String::from_utf8_lossy(&bad.stdout).contains("\"deny\""));
}

#[test]
fn routines_and_runs_from_their_bodies_and_nothing_that_is_not_an_id_or_a_link() {
    let list = json!({"data": [
        {"id": "trig_01KGA", "name": "morning slack", "enabled": true, "cron_expression": "0 2 * * 1-5",
         "next_run_at": "2026-10-13T02:00:41.373491206Z", "last_fired_at": "2026-10-12T02:00:39Z"},
        {"id": "trig_01B", "name": "on\u{202e}ce\u{200b}", "enabled": false, "cron_expression": "",
         "next_run_at": null},
        {"id": "trig_$(rm)", "name": "bad", "enabled": true},
    ], "has_more": false});
    let (rs, more) = cloud::routines(&list.to_string()).unwrap();
    assert!(!more);
    assert_eq!(
        rs,
        [
            Routine {
                id: "trig_01KGA".into(),
                name: "morning slack".into(),
                enabled: true,
                cron: Some("0 2 * * 1-5".into()),
                next_run: Some(1_791_856_841),
                last_fired: Some(1_791_770_439),
            },
            Routine { id: "trig_01B".into(), name: "once".into(), ..Default::default() },
        ]
    );
    let runs = json!({"trigger_id": "trig_01KGA", "next_cursor": null, "data": [
        {"id": "cse_011t", "title": "⚡ morning slack\u{1b}[2J", "status": "active",
         "worker_status": "idle", "created_at": "2026-10-12T07:09:10.473741Z",
         "last_event_at": "2026-10-12T07:09:20.739086Z",
         "url": "https://claude.ai/code/session_011t"},
        {"id": "cse_022u", "title": "elsewhere", "status": "active", "worker_status": "idle",
         "url": "https://evil.example/code/session_022u"},
        {"id": "cse_033v", "title": "quoted", "status": "active", "worker_status": "idle",
         "url": "https://claude.ai/code/session_033v?x=$(id)"},
    ]});
    let got = cloud::runs(&runs.to_string()).unwrap();
    assert_eq!(
        got,
        [Run {
            id: "cse_011t".into(),
            title: "⚡ morning slack [2J".into(),
            status: "active".into(),
            worker: "idle".into(),
            created: 1_791_788_950,
            last_event: 1_791_788_960,
            url: "https://claude.ai/code/session_011t".into(),
        }]
    );
    assert!(cloud::valid_id("cse_011t", "cse_") && !cloud::valid_id("cse_", "cse_"));
    assert!(!cloud::valid_id("trig_01'", "trig_"));
}

#[test]
fn the_watch_looks_when_a_fire_comes_while_one_goes_and_hourly_else() {
    use cloud::{HOURLY, MARGIN, Pace};
    let t = 1_790_002_800;
    let routine = |enabled, next| Routine { enabled, next_run: Some(next), ..Default::default() };
    let rs = [routine(true, t + 600), routine(false, t + 300)];
    assert!(Pace::default().due(&rs, t, false), "the first round");
    let p = Pace { listed: Some(t), ..Default::default() };
    assert!(!p.due(&rs, t + 600, false), "a fire is given its margin");
    assert!(p.due(&rs, t + 600 + MARGIN, false), "then looked for");
    assert!(!p.due(&rs[1..], t + 300 + MARGIN, false), "a paused one's time means nothing");
    assert!(p.due(&[], t + HOURLY, false) && !p.due(&[], t + HOURLY - 1, false));
    assert!(p.due(&[], t + 1, true), "after a gap in the ticking");
    // Looked at again after half of how long it has been pending: a minute at least, ten at most.
    let going = |since| Pace { listed: Some(t), pending: Some(since), fails: 0 };
    assert!(!going(t).due(&[], t + 59, false) && going(t).due(&[], t + 60, false));
    assert!(!going(t - 1200).due(&[], t + 599, false) && going(t - 1200).due(&[], t + 600, false));
    let failed = |fails| Pace { listed: Some(t), pending: None, fails };
    assert!(!failed(1).due(&[], t + 119, false) && failed(1).due(&[], t + 120, false));
    assert!(!failed(3).due(&[], t + 479, false) && failed(3).due(&[], t + 480, false));
    // Failing, a run still going waits for the retry too.
    let both = Pace { listed: Some(t), pending: Some(t - 1200), fails: 2 };
    assert!(!both.due(&[], t + 239, false) && both.due(&[], t + 240, false));
}

/// On the hour, UTC.
const T0: i64 = 1_790_002_800;

/// A daemon whose clock is moved by hand, with the watch on or off; the stub answers couriers
/// from `stub/cloud/`.
fn watching(name: &str, on: bool, env: &[(&str, &str)]) -> common::Daemon {
    let dir = common::fresh(&format!("cl-{name}"));
    std::fs::create_dir_all(dir.join("conf")).unwrap();
    std::fs::create_dir_all(dir.join("stub/cloud")).unwrap();
    if on {
        std::fs::write(dir.join("conf/config"), "CLOUD_WATCH=on\n").unwrap();
    }
    let mut extra = vec![
        ("GENSOKYO_NOW_FILE".to_string(), dir.join("now").display().to_string()),
        ("GENSOKYO_TICK_MS".into(), "100".into()),
        ("TZ".into(), "UTC".into()),
    ];
    extra.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let d = common::Daemon::at(dir, extra);
    clock(&d, T0 + 120);
    d
}

fn clock(d: &common::Daemon, t: i64) {
    std::fs::write(d.dir.join("now.tmp"), t.to_string()).unwrap();
    std::fs::rename(d.dir.join("now.tmp"), d.dir.join("now")).unwrap();
}

/// What a call answers from now on: `HTTP <code>` and the body.
fn answer(d: &common::Daemon, call: &str, code: u16, body: Value) {
    let f = d.dir.join(format!("stub/cloud/{call}.txt"));
    std::fs::write(f, format!("HTTP {code}\n{body}")).unwrap();
}

fn calls(d: &common::Daemon) -> Vec<String> {
    let f = std::fs::read_to_string(d.dir.join("stub/courier.calls")).unwrap_or_default();
    f.lines().map(String::from).collect()
}

/// The watch's notices so far, as the daemon logged them.
fn told(d: &common::Daemon) -> Vec<String> {
    let log = d.log().into_iter().filter(|e| e["ev"] == "cloud");
    log.filter_map(|e| e["notice"].as_str().map(String::from)).collect()
}

/// Once `n` couriers have been and their round is over, and a few ticks more.
fn rounds(d: &common::Daemon, n: usize) {
    common::wait(|| calls(d).len() >= n, &format!("{n} couriers"));
    let ended = || d.log().iter().filter(|e| e["ev"] == "cloud" && !e["call"].is_null()).count();
    common::wait(|| ended() >= n, "their rounds");
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(calls(d).len(), n, "{:?}", calls(d));
}

fn at(t: i64) -> String {
    jiff::Timestamp::from_second(t).unwrap().to_string()
}

fn routines(next: i64, fired: i64) -> Value {
    listing(&[("R1", next, fired)], false)
}

/// Routines by id (`trig_<id>`, named `n<id>`), each with its next run and its last fire.
fn listing(rs: &[(&str, i64, i64)], more: bool) -> Value {
    let data: Vec<Value> = rs
        .iter()
        .map(|(id, next, fired)| {
            let name = if *id == "R1" { "morning".into() } else { format!("n{id}") };
            json!({"id": format!("trig_{id}"), "name": name, "enabled": true,
                "cron_expression": "0 * * * *", "next_run_at": at(*next), "last_fired_at": at(*fired)})
        })
        .collect();
    json!({"data": data, "has_more": more})
}

/// A routine's newest runs: id, worker status, last event.
fn runs(rs: &[(&str, &str, i64)]) -> Value {
    let data: Vec<Value> = rs
        .iter()
        .map(|(id, worker, last)| {
            json!({"id": format!("cse_{id}"), "title": "⚡ morning", "status": "active",
                "worker_status": worker, "created_at": at(*last - 10),
                "last_event_at": at(*last), "url": format!("https://claude.ai/code/session_{id}")})
        })
        .collect();
    json!({"trigger_id": "trig_R1", "data": data, "next_cursor": null})
}

#[test]
fn the_watch_is_off_until_the_user_turns_it_on() {
    let d = watching("off", false, &[]);
    answer(&d, "list", 200, routines(T0 + 3600, T0 - 100));
    d.cli(&["list"]);
    std::thread::sleep(std::time::Duration::from_millis(800));
    assert_eq!(calls(&d), Vec::<String>::new());
    // A resident may not spend the user's plan.
    let inside = d.command(&["cloud", "watch", "on"]).env("GENSOKYO_RESIDENT", "x").output();
    let inside = inside.unwrap();
    assert!(!inside.status.success() && common::err(&inside).contains("theirs to turn on"));
    let said = common::out(&d.cli(&["cloud", "watch", "on"]));
    assert!(said.starts_with("the cloud watch is on"), "{said}");
    rounds(&d, 2);
    assert!(common::out(&d.cli(&["doctor"])).contains("the cloud watch is on"));
}

#[test]
fn a_turn_that_ends_is_told_once_with_its_link_and_what_was_there_first_is_not() {
    let d = watching("flow", true, &[]);
    answer(&d, "list", 200, routines(T0 + 3600, T0 - 100));
    answer(&d, "list_runs-trig_R1", 200, runs(&[("A", "idle", T0 - 90)]));
    d.cli(&["list"]);
    // First sight: its runs are read and only recorded.
    rounds(&d, 2);
    assert_eq!(calls(&d), ["list", "list_runs trig_R1"]);
    assert_eq!(told(&d), Vec::<String>::new());
    // It fires on the hour: a run going is seen, and told nothing of yet.
    answer(&d, "list", 200, routines(T0 + 7200, T0 + 3601));
    answer(
        &d,
        "list_runs-trig_R1",
        200,
        runs(&[("B", "running", T0 + 3605), ("A", "idle", T0 - 90)]),
    );
    clock(&d, T0 + 3665);
    rounds(&d, 4);
    assert_eq!(told(&d), Vec::<String>::new());
    let journal = std::fs::read_to_string(d.dir.join("cloud/trig_R1/journal.jsonl")).unwrap();
    assert!(journal.contains("fired: https://claude.ai/code/session_B"), "{journal}");
    // Its turn ends.
    answer(&d, "list_runs-trig_R1", 200, runs(&[("B", "idle", T0 + 3700), ("A", "idle", T0 - 90)]));
    clock(&d, T0 + 3730);
    rounds(&d, 6);
    assert_eq!(told(&d), ["morning: finished https://claude.ai/code/session_B"]);
    // Nothing going: an hour on, the list alone, and nothing told again.
    clock(&d, T0 + 3730 + 3600);
    rounds(&d, 7);
    assert_eq!(calls(&d)[6], "list");
    // A follow-up turn in the same run is a turn of its own.
    answer(&d, "list", 200, routines(T0 + 7200 + 3600, T0 + 7300));
    answer(&d, "list_runs-trig_R1", 200, runs(&[("B", "idle", T0 + 7310), ("A", "idle", T0 - 90)]));
    clock(&d, T0 + 7400 + 3600);
    rounds(&d, 9);
    assert_eq!(told(&d).len(), 2, "{:?}", told(&d));
    // A daemon started again remembers what it told.
    d.cli(&["quit"]);
    d.cli(&["list"]);
    rounds(&d, 10);
    assert_eq!(told(&d).len(), 2, "{:?}", told(&d));
}

#[test]
fn a_routine_or_a_run_deleted_on_the_web_is_dropped_quietly() {
    let d = watching("gone", true, &[]);
    answer(&d, "list", 200, routines(T0 + 3600, T0 - 100));
    answer(&d, "list_runs-trig_R1", 200, runs(&[("A", "running", T0 - 90)]));
    d.cli(&["list"]);
    rounds(&d, 2);
    // Missing from a first page that is not the whole list, it is kept.
    let seen = || std::fs::read_to_string(d.dir.join("cloud/seen.json")).unwrap();
    answer(&d, "list", 200, json!({"data": [], "has_more": true}));
    clock(&d, T0 + 3700);
    rounds(&d, 3);
    assert!(seen().contains("cse_A") && seen().contains("trig_R1"), "{}", seen());
    // The run went with the routine.
    answer(&d, "list", 200, json!({"data": [], "has_more": false}));
    clock(&d, T0 + 7400);
    rounds(&d, 4);
    clock(&d, T0 + 11000);
    rounds(&d, 5);
    assert_eq!(calls(&d)[2..], ["list", "list", "list"]);
    assert_eq!(told(&d), Vec::<String>::new());
    let seen = std::fs::read_to_string(d.dir.join("cloud/seen.json")).unwrap();
    assert!(!seen.contains("cse_A") && !seen.contains("trig_R1"), "{seen}");
}

#[test]
fn a_watch_that_keeps_failing_says_so_once_and_again_when_it_works() {
    let d = watching("broken", true, &[]);
    answer(&d, "list", 500, json!({"error": {"message": "overloaded"}}));
    d.cli(&["list"]);
    for (n, t) in (1..=4).zip([0, 3600, 7200, 10800]) {
        clock(&d, T0 + 120 + t);
        rounds(&d, n);
    }
    let broken = "cloud watch: list: HTTP 500: overloaded (gensokyo cloud watch off stops it)";
    assert_eq!(told(&d), [broken]);
    answer(&d, "list", 200, json!({"data": [], "has_more": false}));
    clock(&d, T0 + 120 + 14400);
    rounds(&d, 5);
    assert_eq!(told(&d), [broken, "cloud watch: it works again"]);
}

#[test]
fn a_courier_that_tries_another_call_is_stopped_by_its_gate() {
    let call = r#"{"action":"run","trigger_id":"trig_R1"}"#;
    let d = watching("gate", true, &[("STUB_COURIER_CALL", call)]);
    answer(&d, "run-trig_R1", 200, json!({"session_id": "cse_X"}));
    d.cli(&["list"]);
    rounds(&d, 1);
    assert!(d.dir.join("stub/gate.denied").exists());
    let failed = d.log().into_iter().find(|e| e["ev"] == "cloud" && !e["failed"].is_null());
    let failed = failed.unwrap()["failed"].as_str().unwrap().to_string();
    assert_eq!(failed, "the courier tried another call (run), and the gate refused it");
}

#[test]
fn one_routine_failing_holds_up_none_of_the_others_and_a_run_listed_late_is_still_told() {
    let d = watching("apart", true, &[]);
    let both =
        |r2_fired| listing(&[("R1", T0 + 3600, T0 - 100), ("R2", T0 + 3600, r2_fired)], false);
    answer(&d, "list", 200, both(T0 - 100));
    answer(&d, "list_runs-trig_R1", 500, json!({"error": {"message": "overloaded"}}));
    answer(&d, "list_runs-trig_R2", 200, runs(&[("A", "idle", T0 - 90)]));
    d.cli(&["list"]);
    rounds(&d, 3);
    let failed = d.log().into_iter().filter(|e| e["routine"] == "trig_R1").count();
    assert_eq!(failed, 1);
    // R2 fires, and its run is not listed yet.
    answer(&d, "list_runs-trig_R1", 200, runs(&[("X", "idle", T0 - 90)]));
    let mut fired = both(T0 + 3601);
    fired["data"][1]["next_run_at"] = json!(at(T0 + 7200));
    answer(&d, "list", 200, fired);
    clock(&d, T0 + 3665);
    rounds(&d, 6);
    assert_eq!(calls(&d)[3..], ["list", "list_runs trig_R1", "list_runs trig_R2"]);
    // It is looked for again; it comes in a state not seen before, and is watched.
    answer(
        &d,
        "list_runs-trig_R2",
        200,
        runs(&[("B", "starting", T0 + 3650), ("A", "idle", T0 - 90)]),
    );
    clock(&d, T0 + 3730);
    rounds(&d, 8);
    assert_eq!(told(&d), Vec::<String>::new());
    answer(&d, "list_runs-trig_R2", 200, runs(&[("B", "idle", T0 + 3790), ("A", "idle", T0 - 90)]));
    clock(&d, T0 + 3800);
    rounds(&d, 10);
    assert_eq!(told(&d), ["nR2: finished https://claude.ai/code/session_B"]);
}

#[test]
fn a_runs_log_is_shown_as_lines_with_nothing_a_terminal_would_act_on() {
    assert_eq!(cloud::run_id("cse_01AB").as_deref(), Some("cse_01AB"));
    assert_eq!(cloud::run_id("https://claude.ai/code/session_01AB").as_deref(), Some("cse_01AB"));
    for no in ["morning", "https://claude.ai/code/routines/trig_1", "https://x.ai/code/session_1"] {
        assert_eq!(cloud::run_id(no), None, "{no}");
    }
    let body = "{\"session_id\":\"cse_A\",\"next_cursor\":null}\n\n[t] user: hi\x1b]52;c;eA==\x07\n\
                [t] result: \u{202E}ok\u{61C}\u{2066}\u{FEFF}\tdone\r\n";
    assert_eq!(cloud::run_log(body, "cse_A"), "[t] user: hi]52;c;eA==\n[t] result: ok done");
    let more = "{\"next_cursor\":\"c2\"}\n[t] user: hi";
    assert_eq!(
        cloud::run_log(more, "cse_A"),
        "[t] user: hi\n(the log goes on past this page: https://claude.ai/code/session_A)"
    );
}

#[test]
fn the_routines_are_listed_and_opened_from_what_the_watch_saw_and_a_log_is_the_users_alone() {
    let dir = common::fresh("cl-open-browser");
    let browser = dir.join("browser");
    std::fs::write(&browser, format!("#!/bin/sh\necho \"$1\" >> {}/opened\n", dir.display()))
        .unwrap();
    std::fs::set_permissions(&browser, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .unwrap();
    let d = watching("open", true, &[("GENSOKYO_OPEN", &browser.display().to_string())]);
    let both = listing(&[("R1", T0 + 3600, T0 - 100), ("R2", T0 + 7200, T0 - 100)], false);
    answer(&d, "list", 200, both);
    answer(&d, "list_runs-trig_R1", 200, runs(&[("A", "running", T0 - 90)]));
    answer(&d, "list_runs-trig_R2", 200, json!({"data": [], "next_cursor": null}));
    d.cli(&["list"]);
    // A client watching is sent them as the round ends, below the rituals.
    let (mut w, mut lines) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    let mut timetable = move |has: &dyn Fn(&Value) -> bool| loop {
        let v = common::next(&mut lines);
        if v["t"] == "rituals" && has(&v) {
            return v;
        }
    };
    let seen = timetable(&|v| v["cloud"]["routines"][0]["going"] == true);
    assert_eq!(seen["cloud"]["routines"][1]["name"], "nR2");
    rounds(&d, 3);

    // Listed and opened from what the watch saw: no courier for either.
    let said = common::out(&d.cli(&["cloud"]));
    // (Ago and in are by the wall clock, which the test's is not.)
    assert!(said.starts_with("listed ") && !said.contains("failed"), "{said}");
    let row = said.lines().find(|l| l.contains("morning")).unwrap();
    assert!(row.contains("0 * * * * UTC") && row.contains("2026-09-21 16:00"), "{row}");
    assert!(row.ends_with("https://claude.ai/code/session_A (going)"), "{row}");
    let row = said.lines().find(|l| l.contains("nR2")).unwrap();
    assert!(row.ends_with("https://claude.ai/code/routines/trig_R2"), "{row}");
    d.cli(&["cloud", "open"]);
    d.cli(&["cloud", "open", "nR2"]);
    let opened = std::fs::read_to_string(dir.join("opened")).unwrap();
    assert_eq!(
        opened,
        "https://claude.ai/code/session_A\nhttps://claude.ai/code/routines/trig_R2\n"
    );
    let nope = d.command(&["cloud", "open", "nope"]).output().unwrap();
    assert!(common::err(&nope).contains("no routine nope on claude.ai"), "{}", common::err(&nope));
    assert_eq!(calls(&d).len(), 3);

    // A routine's newest run's log, for the user.
    let page = "{\"session_id\":\"cse_A\",\"next_cursor\":null}\n[t] result: success — posted";
    std::fs::write(d.dir.join("stub/cloud/get_run_log-cse_A.txt"), format!("HTTP 200\n{page}"))
        .unwrap();
    let log = common::out(&d.cli(&["cloud", "log", "morning"]));
    assert_eq!(log, "[t] result: success — posted\n");
    assert_eq!(calls(&d)[3], "get_run_log cse_A");
    // Never into a resident: what a run read is no resident's to read.
    let inside = d.command(&["cloud", "log", "morning"]).env("GENSOKYO_RESIDENT", "x").output();
    let inside = inside.unwrap();
    assert!(common::err(&inside).contains("stays out of residents"), "{}", common::err(&inside));
    assert_eq!(calls(&d).len(), 4);
    // A run gone from claude.ai; and with the watch off, a run's link still does.
    let gone = d.command(&["cloud", "log", "https://claude.ai/code/session_Z"]).output().unwrap();
    assert!(common::err(&gone).contains("cse_Z is gone from claude.ai"), "{}", common::err(&gone));
    d.cli(&["cloud", "watch", "off"]);
    timetable(&|v| v["cloud"].is_null());
    let off = d.command(&["cloud", "log", "morning"]).output().unwrap();
    assert!(common::err(&off).contains("the cloud watch is off"), "{}", common::err(&off));
    assert!(common::out(&d.cli(&["cloud"])).starts_with("the cloud watch is off"));
    let by_link = common::out(&d.cli(&["cloud", "log", "https://claude.ai/code/session_A"]));
    assert_eq!(by_link, log);
}

#[test]
fn a_local_schedule_becomes_the_one_utc_cron_that_keeps_it_all_year_or_none() {
    let tz = |n: &str| jiff::tz::TimeZone::get(n).unwrap();
    let utc = |spec: &str, zone: &str| cloud::utc_cron(spec, &tz(zone), T0);
    let saigon = "Asia/Ho_Chi_Minh";
    assert_eq!(utc("0 9 * * 1-5", saigon).unwrap(), "0 2 * * 1-5");
    // Across midnight the weekdays move with it.
    assert_eq!(utc("30 6 * * 1-5", saigon).unwrap(), "30 23 * * 0-4");
    assert_eq!(utc("0 6 * * 1,3", saigon).unwrap(), "0 23 * * 0,2");
    assert_eq!(utc("@daily", saigon).unwrap(), "0 17 * * *");
    assert_eq!(utc("0 */6 * * *", saigon).unwrap(), "0 5,11,17,23 * * *");
    assert_eq!(utc("0 9 * * *", "Asia/Kolkata").unwrap(), "30 3 * * *");
    assert_eq!(utc("0 9 15 * *", saigon).unwrap(), "0 2 15 * *");
    assert_eq!(utc("@hourly", "UTC").unwrap(), "0 * * * *");
    // A date that crosses midnight has no cron; nor has a zone that changes its clocks.
    assert!(utc("0 6 1 * *", saigon).unwrap_err().contains("no UTC cron"));
    assert!(utc("0 9 * * *", "America/New_York").unwrap_err().contains("no UTC cron"));
    assert!(utc("*/30 * * * *", saigon).unwrap_err().contains("hourly"));
    assert!(utc("0 9 30 2 *", saigon).unwrap_err().contains("within a year"));
    assert!(utc("nonsense", saigon).is_err());
}

fn add(name: &str, prompt: &str) -> gensokyo::proto::CloudAdd {
    gensokyo::proto::CloudAdd {
        name: name.into(),
        schedule: "0 9 * * 1-5".into(),
        prompt: prompt.into(),
        ..Default::default()
    }
}

#[test]
fn a_routine_is_made_paused_with_the_connectors_named_and_no_others() {
    let slack =
        cloud::connector("Slack", "ff53db50-7833-41f6-b8bd-846f5123e9d3 https://mcp.slack.com/mcp");
    let slack = slack.unwrap();
    let mut a = add("digest", "Say \"hi\" in #me.\nThen stop.");
    a.repos = vec!["https://github.com/org/app.git".into(), "org/lib".into()];
    let call =
        cloud::create(&a, "0 2 * * 1-5", "env_01X", std::slice::from_ref(&slack), "u-1").unwrap();
    assert_eq!(call["action"], "create");
    let b = &call["body"];
    assert_eq!((b["name"].as_str(), b["enabled"].as_bool()), (Some("digest"), Some(false)));
    assert_eq!(b["cron_expression"], "0 2 * * 1-5");
    assert_eq!(
        b["mcp_connections"],
        json!([{"connector_uuid": slack.uuid, "name": "Slack", "url": "https://mcp.slack.com/mcp"}])
    );
    let ccr = &b["job_config"]["ccr"];
    assert_eq!(ccr["environment_id"], "env_01X");
    assert_eq!(ccr["session_context"]["model"], cloud::MODEL);
    assert_eq!(
        ccr["session_context"]["sources"],
        json!([{"git_repository": {"url": "https://github.com/org/app"}},
            {"git_repository": {"url": "https://github.com/org/lib"}}])
    );
    let first = &ccr["events"][0]["data"];
    assert_eq!(first["message"], json!({"content": a.prompt, "role": "user"}));
    assert_eq!((first["uuid"].as_str(), first["session_id"].as_str()), (Some("u-1"), Some("")));
    // None named is none, said so: left out, claude.ai would give it every one.
    let bare = cloud::create(&add("d", "p"), "0 2 * * *", "env_01X", &[], "u").unwrap();
    assert_eq!(bare["body"]["mcp_connections"], json!([]));

    let no = |a: &gensokyo::proto::CloudAdd, env: &str| {
        cloud::create(a, "0 2 * * *", env, &[], "u").unwrap_err()
    };
    assert!(no(&add("", "p"), "env_1").contains("name"));
    assert!(no(&add("a\nb", "p"), "env_1").contains("name"));
    assert!(no(&add("a\u{202e}b", "p"), "env_1").contains("name"));
    assert!(no(&add("d", " "), "env_1").contains("prompt"));
    assert!(no(&add("d", &"x".repeat(cloud::PROMPT_MOST + 1)), "env_1").contains("prompt"));
    assert!(no(&add("d", "p"), "env_\"x").contains("environment"));
    let mut a = add("d", "p");
    a.repos = vec!["org/app;rm".into()];
    assert!(no(&a, "env_1").contains("not a GitHub repository"));
    a.repos = vec![];
    a.model = Some("x y".into());
    assert!(no(&a, "env_1").contains("model"));

    for bad in [
        "uuid https://x",
        "ff53db50-7833-41f6-b8bd-846f5123e9d3",
        "ff53db50-7833-41f6-b8bd-846f5123e9d3 http://x",
    ] {
        assert!(cloud::connector("Slack", bad).is_err(), "{bad}");
    }
    assert!(cloud::connector("Sl ack", "ff53db50-7833-41f6-b8bd-846f5123e9d3 https://x").is_err());
    let u = cloud::uuid4().unwrap();
    assert!(u.len() == 36 && u.as_bytes()[14] == b'4' && u != cloud::uuid4().unwrap(), "{u}");
}

/// A routine as claude.ai gives one back: in a write's reply, or `get`'s.
fn trigger(id: &str, name: &str, enabled: bool, conns: &[(&str, &str)]) -> Value {
    let conns: Vec<Value> = conns
        .iter()
        .map(|(n, u)| json!({"connector_uuid": u, "name": n, "url": "https://x", "permitted_tools": []}))
        .collect();
    json!({"id": format!("trig_{id}"), "name": name, "enabled": enabled,
        "cron_expression": "0 2 * * 1-5", "next_run_at": at(T0 + 7200),
        "mcp_connections": conns,
        "derived_state": {"prompt": "Read #me.\nPost a \u{1b}[31msummary\u{202e}."},
        "job_config": {"ccr": {"session_context": {"model": "claude-sonnet-5-5",
            "sources": [{"git_repository": {"url": "https://github.com/org/app"}}]}}}})
}

/// The last call a courier made but the watch's own.
fn wrote(d: &common::Daemon) -> Value {
    let all = std::fs::read_to_string(d.dir.join("stub/courier.json")).unwrap();
    let mut calls = all.lines().map(|l| serde_json::from_str::<Value>(l).unwrap());
    calls.rfind(|c| !c["action"].as_str().unwrap().starts_with("list")).unwrap()
}

const SLACK: &str = "ff53db50-7833-41f6-b8bd-846f5123e9d3";

#[test]
fn routines_are_made_paused_and_only_the_user_runs_or_resumes_one_after_reading_it() {
    let d = watching("make", true, &[("TZ", "Asia/Ho_Chi_Minh")]);
    let conf = d.dir.join("conf/config");
    let prompt = d.dir.join("prompt.md");
    std::fs::write(&prompt, "Read \"#me\".\nPost what needs me.").unwrap();
    let p = prompt.display().to_string();
    let adding = |extra: &[&str]| {
        let mut a =
            vec!["cloud", "add", "digest", "--schedule", "0 9 * * 1-5", "--prompt-file", &p];
        a.extend(extra);
        d.command(&a).stdin(Stdio::null()).output().unwrap()
    };
    answer(&d, "list", 200, routines(T0 + 3600, T0 - 100));
    answer(&d, "list_runs-trig_R1", 200, runs(&[]));
    d.cli(&["list"]);
    rounds(&d, 2);

    // What it needs is the user's to name, in the config; and nothing is spent without it.
    let o = adding(&["--connector", "Slack"]);
    assert!(common::err(&o).contains("CLOUD_ENV=env_"), "{}", common::err(&o));
    std::fs::write(&conf, "CLOUD_WATCH=on\nCLOUD_ENV=env_01X\n").unwrap();
    let o = adding(&["--connector", "Slack"]);
    assert!(common::err(&o).contains("CLOUD_CONNECTOR_Slack=<uuid> <url>"), "{}", common::err(&o));
    std::fs::write(
        &conf,
        format!("CLOUD_WATCH=on\nCLOUD_ENV=env_01X\nCLOUD_CONNECTOR_Slack={SLACK} https://mcp.slack.com/mcp\n"),
    )
    .unwrap();
    assert_eq!(calls(&d).len(), 2);

    // Made from a resident too, paused, at the UTC of the local time asked.
    let made = json!({"outcome": "CREATE_TRIGGER_OUTCOME_CREATED",
        "trigger": trigger("N", "digest", false, &[("Slack", SLACK)])});
    answer(&d, "create", 200, made);
    let o = d
        .command(&[
            "cloud",
            "add",
            "digest",
            "--schedule",
            "0 9 * * 1-5",
            "--prompt-file",
            &p,
            "--connector",
            "Slack",
        ])
        .env("GENSOKYO_RESIDENT", "x")
        .output()
        .unwrap();
    let said = common::out(&o);
    assert!(o.status.success(), "{}", common::err(&o));
    assert!(said.starts_with("made digest, paused, from 0 9 * * 1-5 local"), "{said}");
    assert!(said.contains("0 2 * * 1-5 UTC, next 2026-09-22 00:00"), "{said}");
    assert!(
        said.contains("connectors  Slack") && said.contains("every tool of Slack runs"),
        "{said}"
    );
    // Nothing a terminal would act on, from what claude.ai keeps.
    assert!(said.contains("    Post a [31msummary.") && !said.contains('\u{1b}'), "{said}");
    let body = &wrote(&d)["body"];
    assert_eq!(
        (body["enabled"].as_bool(), body["cron_expression"].as_str()),
        (Some(false), Some("0 2 * * 1-5"))
    );
    assert_eq!(body["mcp_connections"][0]["connector_uuid"], SLACK);
    let content = &body["job_config"]["ccr"]["events"][0]["data"]["message"]["content"];
    assert_eq!(content, "Read \"#me\".\nPost what needs me.");
    // The watch lists them all again, now.
    common::wait(|| calls(&d).iter().filter(|c| *c == "list").count() == 2, "a list after");

    // Given more than it was asked for, it says so, with where to fix it.
    let more =
        json!({"trigger": trigger("N", "digest", false, &[("Slack", SLACK), ("Linear", "0000")])});
    answer(&d, "create", 200, more);
    let o = adding(&["--connector", "Slack"]);
    assert!(
        common::err(&o).contains("not as asked (it has other connectors)"),
        "{}",
        common::err(&o)
    );
    assert!(common::err(&o).contains("https://claude.ai/code/routines/trig_N"));
    let denied = json!({"type": "error", "error": {"type": "permission_error",
        "message": "You don't have access to a repository this routine uses.", "sub_reason": "repo_access_denied"}});
    answer(&d, "create", 403, denied);
    let o = adding(&["--repo", "org/app"]);
    assert!(common::err(&o).contains("Claude GitHub App"), "{}", common::err(&o));

    // Paused by anyone, by name.
    answer(&d, "update-trig_R1", 200, json!({"trigger": trigger("R1", "morning", false, &[])}));
    let o =
        d.command(&["cloud", "pause", "morning"]).env("GENSOKYO_RESIDENT", "x").output().unwrap();
    assert_eq!(common::out(&o), "paused morning\n", "{}", common::err(&o));
    assert_eq!(
        wrote(&d),
        json!({"action": "update", "trigger_id": "trig_R1", "body": {"enabled": false}})
    );

    // Resumed and run only by the user, and only once they have read it. (The watch lists them
    // again after each change, in between.)
    let asked = |d: &common::Daemon| {
        calls(d).into_iter().filter(|c| !c.starts_with("list")).collect::<Vec<_>>()
    };
    let n = asked(&d).len();
    for verb in ["resume", "run"] {
        let o =
            d.command(&["cloud", verb, "morning", "--yes"]).env("GENSOKYO_RESIDENT", "x").output();
        assert!(common::err(&o.unwrap()).contains("the user runs and resumes routines"));
    }
    // Nor by a resident's own request.
    let s = std::os::unix::net::UnixStream::connect(d.socket()).unwrap();
    let mut w2 = s.try_clone().unwrap();
    let mut back = std::io::BufRead::lines(std::io::BufReader::new(s));
    let hello =
        json!({"t": "hello", "proto": gensokyo::proto::PROTO, "who": "cli", "resident": "x"});
    writeln!(w2, "{hello}").unwrap();
    common::next(&mut back);
    writeln!(w2, "{}", json!({"t": "cloud_act", "id": 7, "who": "morning", "act": "run"})).unwrap();
    let r = common::next(&mut back);
    assert!(r["error"].as_str().unwrap().contains("the user runs and resumes routines"), "{r}");
    assert_eq!(asked(&d).len(), n);

    answer(&d, "get-trig_R1", 200, trigger("R1", "morning", false, &[]));
    let o = d.command(&["cloud", "resume", "morning"]).stdin(Stdio::null()).output().unwrap();
    assert!(common::out(&o).contains("routine     morning  (paused)"), "{}", common::out(&o));
    assert!(common::err(&o).contains("no terminal to ask"), "{}", common::err(&o));
    assert_eq!(asked(&d)[n..], ["get trig_R1"]);

    answer(&d, "update-trig_R1", 200, json!({"trigger": trigger("R1", "morning", true, &[])}));
    let o = common::out(&d.cli(&["cloud", "resume", "morning", "--yes"]));
    assert!(o.contains("repos       https://github.com/org/app"), "{o}");
    assert!(o.ends_with("resumed morning: next run 2026-09-22 00:00\n"), "{o}");
    assert_eq!(wrote(&d)["body"], json!({"enabled": true}));
    let fired = json!({"session_id": "cse_F1", "trigger": trigger("R1", "morning", true, &[])});
    answer(&d, "run-trig_R1", 200, fired);
    let o = common::out(&d.cli(&["cloud", "run", "trig_R1", "--yes"]));
    assert!(o.ends_with("morning runs now: https://claude.ai/code/session_F1\n"), "{o}");
    assert_eq!(wrote(&d), json!({"action": "run", "trigger_id": "trig_R1"}));

    // With the watch off no name is known, and an id still does.
    d.cli(&["cloud", "watch", "off"]);
    let o = d.command(&["cloud", "pause", "morning"]).output().unwrap();
    assert!(common::err(&o).contains("give its id (trig_"), "{}", common::err(&o));
    assert_eq!(common::out(&d.cli(&["cloud", "pause", "trig_R1"])), "paused morning\n");
}
