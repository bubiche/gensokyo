//! What a resident is doing, from hook payloads (recorded from Claude Code 2.1.260) and
//! registry snapshots; and its status line report.

mod common;

use gensokyo::daemon::aware::{Aware, Key, Line, Registry, kitty_key};
use gensokyo::daemon::registry;
use gensokyo::hooks::{SPOOL_MOST, reduce, spool, take_spool};
use gensokyo::proto::{Hook, State};
use gensokyo::tele;
use serde_json::{Value, json};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// The recorded payload of `event` (and notification type), as `_hook` reduces it at `at`.
fn recorded(event: &str, kind: Option<&str>, at: i64) -> Hook {
    let text = std::fs::read_to_string(format!("{FIXTURES}/hook-payloads-2.1.260.jsonl")).unwrap();
    let j = text
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|j| j["hook_event_name"] == event && kind.is_none_or(|k| j["notification_type"] == k))
        .unwrap_or_else(|| panic!("no {event} {kind:?} in the fixture"));
    reduce(&j, at)
}

fn hook(v: Value, at: i64) -> Hook {
    reduce(&v, at)
}

#[test]
fn a_payload_is_reduced_to_what_the_shrine_needs_and_never_the_prompt() {
    let p = recorded("UserPromptSubmit", None, 1);
    assert_eq!(
        (p.event.as_str(), p.mode.as_deref(), p.text),
        ("UserPromptSubmit", Some("default"), None)
    );
    assert_eq!(recorded("Stop", None, 1).text.as_deref(), Some("pong"));
    let n = recorded("Notification", Some("permission_prompt"), 1);
    assert_eq!(n.kind.as_deref(), Some("permission_prompt"));
    assert_eq!(n.text.as_deref(), Some("Claude needs your permission"));
    let s = recorded("SessionStart", None, 1);
    assert_eq!(
        (s.kind.as_deref(), s.session.as_deref()),
        (Some("startup"), Some("ed82e343-81ce-4b9e-8fdb-9b32d8136a5c"))
    );
    let q = hook(
        json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
               "tool_input": {"questions": [{"question": "Red\u{1b}[31m or\nblue?"}]}}),
        1,
    );
    assert_eq!(
        (q.tool.as_deref(), q.text.as_deref()),
        (Some("AskUserQuestion"), Some("Red [31m or"))
    );
}

#[test]
fn a_finished_turn_awaits_until_a_prompt_whatever_the_registry_says_of_idle() {
    let mut a = Aware::default();
    a.hook(&recorded("UserPromptSubmit", None, 10));
    assert_eq!(a.state(), State::Busy);
    a.hook(&recorded("Stop", None, 20));
    assert_eq!(a.state(), State::Awaits);
    assert_eq!(a.notice("Reimu"), "Reimu is done: pong");
    a.registry(Some(Registry::Idle), 30);
    assert_eq!(a.state(), State::Awaits);
    // A minute on, idle_prompt changes nothing.
    a.hook(&recorded("Notification", Some("idle_prompt"), 80));
    assert_eq!(a.state(), State::Awaits);
    a.hook(&recorded("UserPromptSubmit", None, 90));
    assert_eq!(a.state(), State::Busy);
}

#[test]
fn a_registry_snapshot_older_than_the_last_hook_counts_for_nothing() {
    let mut a = Aware::default();
    a.hook(&recorded("UserPromptSubmit", None, 10));
    // The poll began at 15, the turn ended at 20, and the poll's "busy" arrived after.
    a.hook(&recorded("Stop", None, 20));
    a.registry(Some(Registry::Busy), 15);
    assert_eq!(a.state(), State::Awaits);
    // One that began after the Stop says a new turn is running, and the flag goes.
    a.registry(Some(Registry::Busy), 25);
    assert_eq!(a.state(), State::Busy);
    a.registry(Some(Registry::Idle), 30);
    assert_eq!(a.state(), State::Resting);
}

#[test]
fn a_permission_awaits_until_the_registry_sees_it_granted() {
    let mut a = Aware::default();
    a.hook(&recorded("UserPromptSubmit", None, 10));
    a.hook(&recorded("Notification", Some("permission_prompt"), 20));
    assert_eq!(a.state(), State::Awaits);
    assert_eq!(a.notice("Marisa"), "Marisa needs your permission");
    a.registry(Some(Registry::Waiting), 25);
    assert_eq!(a.state(), State::Awaits);
    a.registry(Some(Registry::Busy), 30);
    assert_eq!(a.state(), State::Busy);
    a.hook(&recorded("Stop", None, 40));
    assert_eq!(a.state(), State::Awaits);
}

#[test]
fn a_dialog_the_registry_sees_awaits_without_a_hook() {
    let mut a = Aware::default();
    a.hook(&recorded("UserPromptSubmit", None, 10));
    a.registry(Some(Registry::Waiting), 20);
    assert_eq!(a.state(), State::Awaits);
    assert_eq!(a.notice("Cirno"), "Cirno needs your permission");
}

#[test]
fn a_question_outranks_everything_until_it_is_answered() {
    let mut a = Aware::default();
    a.hook(&recorded("UserPromptSubmit", None, 10));
    let ask = |ev: &str, at| {
        hook(
            json!({"hook_event_name": ev, "tool_name": "AskUserQuestion",
                    "tool_input": {"questions": [{"question": "Which colour?"}]}}),
            at,
        )
    };
    a.hook(&ask("PreToolUse", 20));
    a.registry(Some(Registry::Busy), 25);
    assert_eq!(a.state(), State::Asked);
    assert_eq!(a.notice("Sakuya"), "Sakuya asks: Which colour?");
    a.hook(&recorded("Notification", Some("idle_prompt"), 30));
    assert_eq!(a.state(), State::Asked);
    a.hook(&ask("PostToolUse", 40));
    assert_eq!(a.state(), State::Busy);
}

#[test]
fn an_older_hook_is_dropped_and_clear_starts_afresh() {
    let mut a = Aware::default();
    a.hook(&recorded("Stop", None, 20));
    assert!(!a.hook(&recorded("UserPromptSubmit", None, 10)));
    assert_eq!(a.state(), State::Awaits);
    // One at the same instant is not older: a spool replayed twice changes nothing more.
    assert!(a.hook(&recorded("Stop", None, 20)));
    assert_eq!(a.state(), State::Awaits);
    a.hook(&hook(
        json!({"hook_event_name": "SessionStart", "source": "clear", "session_id": "b"}),
        30,
    ));
    assert_eq!(a.state(), State::Resting);
}

#[test]
fn the_registry_is_read_by_session() {
    let b = std::fs::read(format!("{FIXTURES}/agents-2.1.260.json")).unwrap();
    let list = registry::parse(&b).unwrap();
    let s = list.iter().find(|s| s.session_id == "ed82e343-81ce-4b9e-8fdb-9b32d8136a5c").unwrap();
    assert_eq!(
        (s.name.as_deref(), s.status(), s.pid),
        (Some("Marisa"), Some(Registry::Idle), Some(74525))
    );
    assert!(registry::parse(b"{\"not\": \"a list\"}").is_none());
    let odd = br#"[{"sessionId": null}, {"sessionId": "a", "name": 3}, {"sessionId": "b", "status": "busy"}]"#;
    let list = registry::parse(odd).unwrap();
    assert_eq!(
        list.iter().map(|s| (s.session_id.as_str(), s.status())).collect::<Vec<_>>(),
        [("b", Some(Registry::Busy))]
    );
}

#[test]
fn a_status_line_report_and_the_lines_drawn_from_it() {
    let j: Value = serde_json::from_slice(
        &std::fs::read(format!("{FIXTURES}/statusline-2.1.260.json")).unwrap(),
    )
    .unwrap();
    let mut t = tele::from_statusline(&j);
    assert_eq!(
        (t.model.as_deref(), t.ctx, t.window, t.cache, t.turn_cache),
        (Some("Sonnet 5"), Some(5), Some(1_000_000), Some(93), Some(99))
    );
    assert_eq!(t.five_hour.map(|l| (l.used, l.resets)), Some((36, Some(1788543000))));
    assert_eq!((t.cache_warm, t.cache_until), (Some(true), Some(1788534053)));
    assert_eq!(t.version.as_deref(), Some("2.1.260"));
    t.advisor = Some("opus".into());
    // A minute before the cache goes cold, and when it has.
    let (warm, cold) = (1788534053 - 60, 1788534053);
    assert_eq!(
        tele::own_line(&t, warm),
        "Sonnet 5→⚖ Opus · medium · ▌░░░░░░░░░ 5% of 1M · ⚡93% (turn 99%) · $0.19 · +8/-0 · 5m"
    );
    assert_eq!(
        tele::own_line(&t, cold),
        "Sonnet 5→⚖ Opus · medium · ▌░░░░░░░░░ 5% of 1M · ⚡93% (turn 99%) \x1b[36mcold\x1b[0m · \
         $0.19 · +8/-0 · 5m"
    );
    assert_eq!(
        tele::fields(Some(&t), Some("acceptEdits"), Some("main"), false, warm),
        "Sonnet 5→⚖ Opus · medium · accept-edits · ⚡93% · $0.19"
    );
    assert_eq!(
        tele::fields(Some(&t), None, Some("main"), true, cold),
        "Sonnet 5→⚖ Opus · ctx 5% · medium · ⚡93% (turn 99%) cold · $0.19 · ⎇ main"
    );
    assert_eq!(
        tele::usage("5h", &t.five_hour.unwrap(), 1788543000 - 2 * 3600 - 11 * 60, 5),
        "5h █▊░░░ 36% ↻2h11m"
    );
    // A context filling up is yellow from 70% and red from 90%, on its own line only.
    t.ctx = Some(75);
    assert!(tele::own_line(&t, warm).contains(" · \x1b[33m███████▌░░ 75%\x1b[0m of 1M · "));
    t.ctx = Some(93);
    assert!(tele::own_line(&t, warm).contains(" · \x1b[31m█████████▎ 93%\x1b[0m of 1M · "));
    // A cache Claude reports cold is cold whatever its time says.
    t.cache_warm = Some(false);
    assert!(tele::fields(Some(&t), None, None, false, warm).contains("⚡93% cold"));
    // Before the first API call there is little to say.
    assert_eq!(
        tele::own_line(&tele::from_statusline(&json!({"model": {"display_name": "Haiku 4.5"}})), 0),
        "Haiku 4.5"
    );
}

#[test]
fn a_usage_window_warms_once_use_runs_ahead_of_the_clock() {
    use gensokyo::proto::Limit;
    const FIVE: i64 = 5 * 3600;
    let heat =
        |used, left| tele::usage_heat(&Limit { used, resets: Some(1000 + left) }, FIVE, 1000);
    // Half the window gone: 60% used runs ahead, 45% is under half and calm.
    assert_eq!(heat(60, FIVE / 2), tele::Heat::Warn);
    assert_eq!(heat(45, FIVE / 2), tele::Heat::Calm);
    // Four fifths gone: 60% is behind the clock.
    assert_eq!(heat(60, FIVE / 5), tele::Heat::Calm);
    // From 70% it is worth a look, and from 90% nearly out, whatever the clock.
    assert_eq!(heat(75, 60), tele::Heat::Warn);
    assert_eq!(heat(92, 60), tele::Heat::Hot);
    assert_eq!(tele::bar(0, 5), "░░░░░");
    assert_eq!(tele::bar(100, 5), "█████");
    assert_eq!(tele::bar(50, 5), "██▌░░");
}

#[test]
fn a_question_stays_a_question_when_its_dialog_brings_a_permission_prompt() {
    // Claude Code 2.1.283 sends permission_prompt some seconds into an AskUserQuestion dialog.
    let mut a = Aware::default();
    a.hook(&hook(
        json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
                        "tool_input": {"questions": [{"question": "Tea?"}]}}),
        10,
    ));
    a.hook(&recorded("Notification", Some("permission_prompt"), 16));
    assert_eq!((a.state(), a.notice("Reimu").as_str()), (State::Asked, "Reimu asks: Tea?"));
}

#[test]
fn a_permission_denied_leaves_it_resting() {
    // Esc at the dialog ends the turn with no Stop; the registry then says idle.
    let mut a = Aware::default();
    a.hook(&recorded("UserPromptSubmit", None, 10));
    a.registry(Some(Registry::Waiting), 15);
    a.hook(&recorded("Notification", Some("permission_prompt"), 18));
    a.registry(Some(Registry::Waiting), 19);
    assert_eq!(a.state(), State::Awaits);
    a.registry(Some(Registry::Idle), 25);
    assert_eq!(a.state(), State::Resting);
    // A minute on, idle_prompt does not call the dismissed turn done.
    a.hook(&recorded("Notification", Some("idle_prompt"), 80));
    assert_eq!(a.state(), State::Resting);
}

#[test]
fn a_question_dismissed_with_esc_rests_but_an_unverified_dialog_keeps_its_flag() {
    let mut a = Aware::default();
    a.hook(&hook(
        json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
                        "tool_input": {"questions": [{"question": "Tea?"}]}}),
        10,
    ));
    a.registry(Some(Registry::Waiting), 15);
    assert_eq!(a.state(), State::Asked);
    a.registry(Some(Registry::Idle), 20);
    assert_eq!(a.state(), State::Resting);
    // What the registry shows for an elicitation is not known: an idle snapshot leaves it.
    a.hook(&hook(
        json!({"hook_event_name": "Notification", "notification_type": "elicitation_dialog"}),
        30,
    ));
    a.registry(Some(Registry::Idle), 35);
    assert_eq!(a.state(), State::Awaits);
}

#[test]
fn seeing_it_clears_a_finished_turn_but_not_a_dialog() {
    let mut a = Aware::default();
    a.hook(&recorded("Stop", None, 10));
    assert!(a.seen());
    assert_eq!(a.state(), State::Resting);
    // Nor does idle_prompt bring back a turn already seen.
    a.hook(&recorded("Notification", Some("idle_prompt"), 15));
    assert_eq!(a.state(), State::Resting);
    a.hook(&recorded("Notification", Some("permission_prompt"), 20));
    assert!(!a.seen());
    assert_eq!(a.state(), State::Awaits);
}

#[test]
fn idle_prompt_stands_in_for_a_stop_that_never_came() {
    // A permission granted, then the turn ends with no Stop (an interrupt).
    let mut a = Aware::default();
    a.hook(&recorded("UserPromptSubmit", None, 10));
    a.hook(&recorded("Notification", Some("permission_prompt"), 20));
    a.registry(Some(Registry::Busy), 30);
    a.registry(Some(Registry::Idle), 40);
    assert_eq!(a.state(), State::Resting);
    a.hook(&recorded("Notification", Some("idle_prompt"), 100));
    assert_eq!((a.state(), a.notice("Reimu").as_str()), (State::Awaits, "Reimu is done"));
}

#[test]
fn the_spool_is_read_in_order_and_what_was_just_renamed_waits_a_beat() {
    let root = common::fresh("spool");
    let line = |at: i64| {
        json!({"t": "hook", "resident": "r", "hook": {"event": "Stop", "at": at}}).to_string()
    };
    std::fs::write(root.join("spool.1.jsonl"), format!("{}\nnot json\n{}\n", line(30), line(10)))
        .unwrap();
    std::fs::write(root.join("spool.2.jsonl"), format!("{}\n", line(20))).unwrap();
    std::fs::write(root.join("spool.jsonl"), format!("{}\n", line(40))).unwrap();
    let ats: Vec<_> = take_spool(&root, 1000).iter().map(|(_, h)| h.at).collect();
    assert_eq!(ats, [10, 20, 30]);
    // The live spool was renamed aside, and a hook that opened it just before may still write.
    // A line this build cannot read is kept for a later one, not lost.
    let mut left: Vec<_> =
        std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name()).collect();
    left.sort();
    assert_eq!(left.len(), 2, "{left:?}");
    assert_ne!(left[0], "spool.jsonl");
    assert_eq!(left[1], "spool.later.jsonl");
    assert_eq!(std::fs::read_to_string(root.join("spool.later.jsonl")).unwrap(), "not json\n");
    assert!(take_spool(&root, 1000).is_empty());
    // At start there is nobody to wait for. (A rename in the same ms would land on the first.)
    std::thread::sleep(std::time::Duration::from_millis(5));
    std::fs::write(root.join("spool.jsonl"), format!("{}\n", line(50))).unwrap();
    let ats: Vec<_> = take_spool(&root, 0).iter().map(|(_, h)| h.at).collect();
    assert_eq!(ats, [40, 50]);
    // Tried again at each start, and kept again while still unread.
    assert_eq!(std::fs::read_to_string(root.join("spool.later.jsonl")).unwrap(), "not json\n");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_spool_nobody_reads_keeps_its_newest_hooks() {
    let root = common::fresh("spool-most");
    let line = |at: i64| json!({"t": "hook", "resident": "r", "hook": {"event": "Stop", "at": at}});
    let hook = |at: i64| spool(&root, &serde_json::from_value(line(at)).unwrap());
    std::fs::write(root.join("spool.0.jsonl"), format!("{}\n", line(1))).unwrap();
    let full = format!("{}\n", line(2));
    std::fs::write(root.join("spool.jsonl"), full.repeat(SPOOL_MOST as usize / full.len()))
        .unwrap();
    hook(3);
    assert!(!root.join("spool.jsonl").exists(), "a spool past its size moves aside");
    hook(4);
    let ats: Vec<_> = take_spool(&root, 0).iter().map(|(_, h)| h.at).collect();
    assert!(!ats.contains(&1), "the oldest went");
    assert_eq!(ats[ats.len() - 3..], [2, 3, 4]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn usage_is_the_latest_windows_highest_and_none_past_its_reset() {
    use gensokyo::proto::Limit;
    let l = |used, resets| Limit { used, resets: Some(resets) };
    let pick = |ls: &[Limit], now| tele::freshest(ls, now).map(|l| (l.used, l.resets.unwrap()));
    // One window, its reset given a few seconds apart: use only grows, so the highest.
    assert_eq!(pick(&[l(40, 5000), l(55, 5003), l(30, 5001)], 1000), Some((55, 5003)));
    // A new window's use, however low, outdates the last one's.
    assert_eq!(pick(&[l(90, 5000), l(4, 23000)], 6000), Some((4, 23000)));
    assert_eq!(pick(&[l(90, 5000), l(4, 23000)], 1000), Some((4, 23000)));
    // Past its reset, nothing is known.
    assert_eq!(pick(&[l(90, 5000)], 5000), None);
    assert_eq!(pick(&[], 0), None);
}

#[test]
fn resident_text_is_one_line_of_printable_characters() {
    assert_eq!(tele::clean("\r\n\t a\u{9b}b\u{7f}\u{1b}]9;x\u{7}", 80), "a b  ]9;x ");
    assert_eq!(tele::clean("日本語テキスト", 3), "日本語");
    let r = reduce(
        &json!({"hook_event_name": 3, "tool_input": {"questions": []}, "message": " \n "}),
        1,
    );
    assert_eq!((r.event.as_str(), r.text), ("", None));
    let l = gensokyo::proto::Limit { used: 100, resets: Some(1059) };
    assert_eq!(tele::usage("7d", &l, 1000, 5), "7d █████ 100% ↻1m");
    assert_eq!(tele::usage("7d", &l, 1059, 5), "7d █████ 100%");
}

#[test]
fn a_card_may_go_to_a_finished_turn_but_never_into_a_dialog_or_an_unlisted_session() {
    let mut a = Aware::default();
    assert_eq!(a.blocked(), Some("is still starting up"), "no snapshot yet: the trust dialog");
    a.registry(Some(Registry::Idle), 5);
    assert_eq!(a.blocked(), None);
    a.hook(&recorded("UserPromptSubmit", None, 10));
    a.hook(&recorded("Stop", None, 20));
    assert_eq!((a.state(), a.blocked()), (State::Awaits, None));
    // The hook's permission prompt, before the registry has caught up.
    a.hook(&recorded("UserPromptSubmit", None, 30));
    a.hook(&recorded("Notification", Some("permission_prompt"), 40));
    assert_eq!(a.blocked(), Some("has a dialog waiting for you"));
    a.registry(Some(Registry::Busy), 50);
    assert_eq!(a.blocked(), None, "granted");
    // A dialog only the registry sees.
    a.registry(Some(Registry::Waiting), 60);
    assert_eq!(a.blocked(), Some("has a dialog waiting for you"));
    a.hook(&hook(
        json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
               "tool_input": {"questions": [{"question": "Which?"}]}}),
        70,
    ));
    assert_eq!(a.blocked(), Some("is asking you a question"));
    // Gone from the snapshot: whatever it shows now, nothing is known of it.
    a.hook(&hook(json!({"hook_event_name": "PostToolUse", "tool_name": "AskUserQuestion"}), 80));
    a.registry(None, 90);
    assert!(!a.listed());
    assert_eq!(a.blocked(), Some("is still starting up"));
}

/// Whether these keys, sent one after another, leave a draft, from a line that had one or not.
fn draft(had: bool, keys: &[&[u8]]) -> bool {
    let mut l = Line::default();
    l.keys(if had { b"x" } else { b"" });
    keys.iter().for_each(|k| l.keys(k));
    l.draft
}

#[test]
fn what_the_user_types_leaves_a_draft_or_clears_one_and_keys_that_type_nothing_say_nothing() {
    // Text, a paste, and kitty-encoded letters (shifted too) leave something in the line.
    for b in [&b"a"[..], b"\x1b[200~card\x1b[201~", b"\x1b[97u", b"\x1b[65;2u", "é".as_bytes()] {
        assert!(draft(false, &[b]), "{b:?}");
    }
    // Ctrl-C clears it, legacy or kitty; the last of several keys wins.
    for b in [&b"\x03"[..], b"\x1b[99;5u", b"abc\x03", b"\x1b[99;5:1u"] {
        assert!(!draft(true, &[b]), "{b:?}");
    }
    assert!(draft(false, &[b"\x03x"]));
    // Up brings an earlier prompt back into the line.
    assert!(draft(false, &[b"\x1b[A"]) && draft(false, &[b"\x1bOA"]));
    // Tab, Backspace, Esc, arrows, F1, Alt-x, focus, SGR and X10 mouse reports, a key's
    // release, Ctrl-A, Shift-Enter and Alt-Enter: nothing typed, nothing cleared.
    for b in [
        &b"\t"[..],
        b"\x7f",
        b"\x1b",
        b"\x1b[B",
        b"\x1b[1;5A",
        b"\x1bOP",
        b"\x1bx",
        b"\x1b[I",
        b"\x1b[<0;12;5M",
        b"\x1b[M !!",
        b"\x1b[97;1:3u",
        b"\x1b[97;5u",
        b"\x1b[13;2u",
        b"\x1b\r",
        b"\x1b[57399u",
    ] {
        assert!(draft(true, &[b]) && !draft(false, &[b]), "{b:?}");
    }
    assert_eq!(kitty_key(99, 4 | 64, 1), Some(Key::Clear), "Ctrl-C with Caps Lock on");
    assert_eq!(kitty_key(97, 2, 1), None, "Alt-a");
    assert_eq!(kitty_key(13, 64, 1), Some(Key::Enter), "Enter with Caps Lock on");
}

#[test]
fn enter_empties_the_line_only_of_a_command_which_no_hook_reports() {
    // `/context` or `!ls` and Enter: run, the line empty, legacy or kitty, whole or key by key.
    for keys in [
        &[&b"/context\r"[..]][..],
        &[b"/", b"c", b"o", b"\r"],
        &[b"!ls", b"\x1b[13u"],
        &[b"\x03/cost\r"],
    ] {
        assert!(!draft(false, keys), "{keys:?}");
    }
    // A prompt's Enter waits for its hook, which says that it went in.
    assert!(draft(false, &[b"look at @src/ma", b"\r"]));
    // A command after other text is that text; `\` then Enter, and Shift-Enter, are newlines.
    assert!(draft(true, &[b"/context\r"]));
    assert!(draft(false, &[b"/btw one", b"\\", b"\r"]));
    assert!(draft(false, &[b"/btw one\x1b[13;2u"]));
    assert!(!draft(false, &[b"/btw one\\", b"\r", b"two\r"]), "the newline, then the run");
    // Enter inside a paste is text, in one chunk or across two.
    assert!(draft(false, &[b"\x1b[200~/one\rtwo\r\x1b[201~"]));
    assert!(draft(false, &[b"\x1b[200~/one\r", b"two\r\x1b[201~"]));
    assert!(!draft(false, &[b"\x1b[200~/one\x1b[201~\r"]), "a pasted command, run");
    // Up, whatever it brings back, waits for a hook.
    assert!(draft(false, &[b"\x1b[A\r"]));
}

#[test]
fn a_draft_is_kept_from_the_keys_to_the_next_prompt_and_never_typed_into_a_dialog() {
    let draft = |a: &Aware| a.blocked().is_some_and(|b| b.contains("half typed"));
    let mut a = Aware::default();
    // Keys before the registry lists it go to its input line, or to a trust dialog: a draft.
    a.typed(b"x");
    a.registry(Some(Registry::Idle), 1);
    assert!(draft(&a) && a.dialog().is_none());
    a.hook(&hook(json!({"hook_event_name": "UserPromptSubmit"}), 2));
    assert_eq!(a.blocked(), None);
    // Typed during the turn, it is still in the line when the turn ends; which still ends.
    a.typed(b"x");
    a.hook(&hook(json!({"hook_event_name": "Stop", "last_assistant_message": "ok"}), 3));
    assert!(draft(&a));
    assert!(a.finished(), "a draft holds back cards, not the end of a turn");
    a.hook(&hook(json!({"hook_event_name": "SessionStart", "source": "clear"}), 4));
    assert_eq!(a.blocked(), None);
    // Keys while a permission prompt is open answer it.
    let perm = json!({"hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "m"});
    a.hook(&hook(perm, 5));
    a.typed(b"x");
    a.registry(Some(Registry::Idle), 6);
    assert_eq!(a.blocked(), None, "answered, and nothing left in the line");
}

#[test]
fn what_is_typed_after_a_prompt_is_sent_outlasts_that_prompts_late_hook() {
    let draft = |a: &Aware| a.blocked().is_some_and(|b| b.contains("half typed"));
    let prompt = |at| hook(json!({"hook_event_name": "UserPromptSubmit"}), at);
    let mut a = Aware::default();
    a.registry(Some(Registry::Idle), 1);
    // The next prompt begun before the hook of the one sent lands: it is still in the line.
    a.typed(b"one\r");
    a.typed(b"tw");
    a.hook(&prompt(2));
    assert!(draft(&a));
    a.typed(b"o\r");
    a.hook(&prompt(3));
    assert_eq!(a.blocked(), None);
    // So is a key after a card's Enter, before the card's hook.
    let m = a.mark();
    a.entered(m, 4);
    a.typed(b"m");
    a.hook(&prompt(5));
    assert!(draft(&a));
    a.typed(b"\x03");
    assert_eq!(a.blocked(), None);
    // A command typed after the Enter runs, and Ctrl-C clears; either leaves nothing.
    for keys in [&b"one\r/cost\r"[..], b"one\rtw\x03"] {
        a.typed(keys);
        a.hook(&prompt(6));
        assert_eq!(a.blocked(), None, "{keys:?}");
    }
}

#[test]
fn a_remote_url_names_its_repo_by_the_path_after_the_host() {
    let cases = [
        ("git@github.com:acme/app.git", Some("acme/app")),
        ("git@github.com:acme/app", Some("acme/app")),
        ("https://github.com/acme/app.git", Some("acme/app")),
        ("https://user@gitlab.com/group/sub/app/", Some("group/sub/app")),
        ("ssh://git@host.example:2222/acme/app.git", Some("acme/app")),
        ("/srv/git/app.git", None),
        ("../remote.git", None),
        ("git@github.com:acme/app name.git", None),
    ];
    for (url, want) in cases {
        assert_eq!(tele::repo_path(url).as_deref(), want, "{url}");
    }
}

#[test]
fn a_branch_is_cut_to_show_and_whole_to_match() {
    let dir = std::env::temp_dir().join(format!("gsk-head-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let long = format!("someone/{}", "a-long-branch-name-".repeat(5));
    std::fs::write(dir.join(".git/HEAD"), format!("ref: refs/heads/{long}\n")).unwrap();
    assert_eq!(tele::git_head(&dir).as_deref(), Some(long.as_str()));
    assert_eq!(tele::git_branch(&dir).map(|b| b.chars().count()), Some(60));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_resident_being_typed_into_takes_nothing_else_until_its_prompt_goes_in() {
    use gensokyo::daemon::aware::TYPING;
    let mut a = Aware::default();
    a.registry(Some(Registry::Idle), 5);
    a.hook(&recorded("Stop", None, 10));
    a.seen();
    assert!(a.idle() && a.blocked().is_none());
    let m = a.mark();
    assert_eq!(a.blocked(), Some(TYPING));
    assert!(!a.idle() && a.in_way().is_none(), "the one typing is not in its own way");
    // A prompt that went in before this Enter is some other prompt.
    a.hook(&recorded("UserPromptSubmit", None, 20));
    assert_eq!(a.blocked(), Some(TYPING));
    a.entered(m, 30);
    a.hook(&recorded("Stop", None, 25));
    assert_eq!(a.blocked(), Some(TYPING));
    // Any hook after it says it was read: the prompt's may come second and be dropped as older.
    a.hook(&hook(json!({"hook_event_name": "PreToolUse", "tool_name": "Bash"}), 31));
    assert_eq!(a.blocked(), None);
    // Given up with nothing sent; an old mark handed back clears nothing.
    let m = a.mark();
    a.unmark(m);
    assert_eq!(a.blocked(), None);
    let old = a.mark();
    a.unmark(old);
    let _new = a.mark();
    a.unmark(old);
    assert_eq!(a.blocked(), Some(TYPING));
}
