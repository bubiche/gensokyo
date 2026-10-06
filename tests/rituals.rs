//! Rituals on a fake clock: schedules, when a tick fires, the files and what they say.

mod common;

use gensokyo::cron::{self, Schedule, Why};
use gensokyo::ritual::{self, Add, Dir, Target, Trust};
use jiff::civil::date;
use jiff::tz::TimeZone;
use serde_json::json;
use std::path::{Path, PathBuf};

fn ny() -> TimeZone {
    TimeZone::get("America/New_York").unwrap()
}

/// Epoch seconds of a New York wall time (the earlier one when it is ambiguous).
fn at(y: i16, mo: i8, d: i8, h: i8, mi: i8) -> i64 {
    date(y, mo, d).at(h, mi, 0, 0).to_zoned(ny()).unwrap().timestamp().as_second()
}

fn tmp(name: &str) -> PathBuf {
    common::fresh(&format!("rit-{name}"))
}

#[test]
fn schedules_read_as_croner_reads_them() {
    for ok in [
        "5 9 * * 1-5",
        "@hourly",
        "@daily",
        "@midnight",
        "@weekly",
        "@monthly",
        "@yearly",
        "@annually",
        "every 30m",
        "every 1 minute",
        "every 15 mins",
        "every 2h",
        "every 12 hours",
        "*/15 * * * *",
        "0 9 * * mon-fri",
        "  0   9 * * *  ",
    ] {
        assert!(Schedule::parse(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "@reboot",
        "@sometimes",
        "every 45m",
        "every 0m",
        "every 60m",
        "every 5h",
        "every 30",
        "every 30 fortnights",
        "0 9 * *",
        "0 0 9 * * *",
        "61 * * * *",
        "tomorrow",
    ] {
        assert!(Schedule::parse(bad).is_err(), "{bad}");
    }
    let s = Schedule::parse("5 9 * * 1-5").unwrap();
    let fri = at(2026, 9, 25, 9, 5);
    assert_eq!(s.latest(fri + 30, &ny()), Some(fri));
    assert_eq!(s.latest(fri, &ny()), Some(fri), "at or before");
    assert_eq!(s.next(fri, &ny()), Some(at(2026, 9, 28, 9, 5)), "strictly after, past the weekend");
    assert_eq!(s.next(fri - 1, &ny()), Some(fri));
}

#[test]
fn a_schedule_that_never_comes_round_is_found_quickly() {
    let t = std::time::Instant::now();
    let s = Schedule::parse("0 0 31 4 *").unwrap();
    assert_eq!(s.next(at(2026, 9, 27, 12, 0), &ny()), None);
    let first = t.elapsed();
    // Learnt once: every later parse and search of it is free.
    let t = std::time::Instant::now();
    let s = Schedule::parse("0 0 31 4 *").unwrap();
    for _ in 0..100 {
        assert_eq!((s.next(0, &ny()), s.latest(0, &ny())), (None, None));
    }
    let later = t.elapsed();
    eprintln!("never: first {first:?}, 100 more {later:?}");
    assert!(first.as_millis() < 2000 && later.as_millis() < 5, "{first:?} {later:?}");
    // Rare is not never: 29 February comes round.
    let leap = Schedule::parse("0 0 29 2 *").unwrap();
    assert_eq!(leap.next(at(2026, 9, 27, 12, 0), &ny()), Some(at(2028, 2, 29, 0, 0)));
}

/// A tick every 20 s through `decide`, stamping each fire: the minutes it fires for.
fn ticks(s: &Schedule, from: i64, to: i64, tz: &TimeZone) -> Vec<i64> {
    let (mut stamp, mut fired) = (Some(from), Vec::new());
    let mut now = from;
    while now <= to {
        if let Some((why, m)) = cron::decide(s, stamp, now, false, tz) {
            assert_eq!(why, Why::Due);
            fired.push(m);
            stamp = Some(m);
        }
        now += 20;
    }
    fired
}

#[test]
fn ticks_fire_on_croners_own_occurrences_through_both_dst_weeks() {
    let tz = ny();
    for (from, to) in [
        (at(2026, 3, 5, 0, 0), at(2026, 3, 12, 0, 0)),
        (at(2026, 10, 29, 0, 0), at(2026, 11, 5, 0, 0)),
    ] {
        for spec in ["30 1 * * *", "30 2 * * *", "*/10 * * * *", "0 */2 * * *"] {
            let s = Schedule::parse(spec).unwrap();
            let want = s.between(from, to, &tz);
            let got = ticks(&s, from, to, &tz);
            assert_eq!(got, want, "{spec} from {}", ritual::when(from, &tz));
        }
    }
    // What croner does, pinned: at the spring gap 02:30 runs at 03:00, and at the fall back
    // 01:30 runs once, in the first 01:30.
    let s = Schedule::parse("30 2 * * *").unwrap();
    let gap = s.between(at(2026, 3, 7, 12, 0), at(2026, 3, 8, 12, 0), &tz);
    assert_eq!(gap, vec![at(2026, 3, 8, 3, 0)]);
    let s = Schedule::parse("30 1 * * *").unwrap();
    let back = s.between(at(2026, 10, 31, 12, 0), at(2026, 11, 1, 12, 0), &tz);
    assert_eq!(back, vec![at(2026, 11, 1, 1, 30)]);
}

#[test]
fn a_fire_is_due_once_and_only_while_fresh() {
    let tz = ny();
    let s = Schedule::parse("5 9 * * *").unwrap();
    let nine = at(2026, 9, 27, 9, 5);
    let before = Some(nine - 86400);
    // Every tick inside the minute, once stamped, is nothing.
    assert_eq!(cron::decide(&s, before, nine + 5, false, &tz), Some((Why::Due, nine)));
    for t in [nine + 5, nine + 25, nine + 45] {
        assert_eq!(cron::decide(&s, Some(nine), t, false, &tz), None);
    }
    // A late tick still catches it inside the window; past it, only a catch-up does.
    assert_eq!(cron::decide(&s, before, nine + 110, false, &tz), Some((Why::Due, nine)));
    assert_eq!(cron::decide(&s, before, nine + 300, false, &tz), None);
    assert_eq!(cron::decide(&s, before, nine + 300, true, &tz), Some((Why::CatchUp, nine)));
}

#[test]
fn a_ritual_never_seen_has_missed_nothing() {
    let tz = ny();
    let s = Schedule::parse("5 9 * * *").unwrap();
    let nine = at(2026, 9, 27, 9, 5);
    assert_eq!(cron::decide(&s, None, nine + 3600, true, &tz), None, "no catch-up without a stamp");
    // In the very minute of a fire it is due, as a stamped one would be.
    assert_eq!(cron::decide(&s, None, nine + 30, false, &tz), Some((Why::Due, nine)));
}

#[test]
fn a_catch_up_is_the_newest_miss_within_a_week_and_after_the_stamp() {
    let tz = ny();
    let s = Schedule::parse("0 8 * * *").unwrap();
    let now = at(2026, 9, 27, 12, 0);
    let stamp = Some(at(2026, 9, 24, 8, 0));
    assert_eq!(
        cron::decide(&s, stamp, now, true, &tz),
        Some((Why::CatchUp, at(2026, 9, 27, 8, 0)))
    );
    assert_eq!(cron::decide(&s, stamp, now, false, &tz), None, "a miss without a catch is skipped");
    // Stamped today already: nothing.
    assert_eq!(cron::decide(&s, Some(at(2026, 9, 27, 8, 0)), now, true, &tz), None);
    // Over a week ago: not caught up.
    let weekly = Schedule::parse("0 8 1 1 *").unwrap();
    let stamp = Some(at(2025, 1, 1, 8, 0));
    assert_eq!(cron::decide(&weekly, stamp, at(2026, 1, 9, 12, 0), true, &tz), None);
    assert_eq!(
        cron::decide(&weekly, stamp, at(2026, 1, 5, 12, 0), true, &tz),
        Some((Why::CatchUp, at(2026, 1, 1, 8, 0)))
    );
}

#[test]
fn a_ten_minute_gap_on_every_10m_is_one_run() {
    let tz = ny();
    let s = Schedule::parse("every 10m").unwrap();
    let stamp = Some(at(2026, 9, 27, 10, 10));
    // The lid shut at 10:12 and opened at 10:47: 10:20, 10:30 and 10:40 went by.
    let wake = at(2026, 9, 27, 10, 47) + 3;
    assert_eq!(
        cron::decide(&s, stamp, wake, true, &tz),
        Some((Why::CatchUp, at(2026, 9, 27, 10, 40)))
    );
    // Then the next is 10:50, due as usual.
    let next = at(2026, 9, 27, 10, 50);
    assert_eq!(
        cron::decide(&s, Some(at(2026, 9, 27, 10, 40)), next + 10, false, &tz),
        Some((Why::Due, next))
    );
}

fn rit(text: &str) -> ritual::Ritual {
    ritual::parse("morning", Path::new("/x/morning.md"), false, text)
}

#[test]
fn frontmatter_is_read_by_hand() {
    let r = rit("---\n\
                 # a comment\n\
                 name: morning\n\
                 schedule: \"3 9 * * 1-5\"   # weekdays\n\
                 description: the morning's mail # sorted\n\
                 cwd: ~/dev/x\n\
                 mode: acceptEdits\n\
                 allowed_tools:\n  - Read\n  - \"Bash(npm test:*)\"\n  - Bash(a, b)\n\
                 keep: forever\n\
                 enabled: no\n\
                 schedul: typo\n\
                 ---\n\n\n  Do the thing.\n\nThen stop.\n\n");
    assert_eq!(r.schedule, "3 9 * * 1-5");
    assert_eq!(r.description.as_deref(), Some("the morning's mail"));
    assert_eq!(r.cwd, Some(format!("{}/dev/x", std::env::var("HOME").unwrap())));
    assert_eq!(r.allowed_tools, ["Read", "Bash(npm test:*)", "Bash(a, b)"]);
    assert_eq!(r.mode.as_deref(), Some("acceptEdits"));
    assert_eq!(r.keep_secs(), None);
    assert!(!r.enabled());
    assert_eq!(r.unknown, ["schedul"]);
    assert_eq!(r.prompt, "  Do the thing.\n\nThen stop.");
    assert_eq!(r.target(), Target::New);
    let r = rit(
        "---\nallowed_tools: [\"Read\", Grep]\nallowedTools: Glob, WebFetch\npermission_mode: bypassPermissions\ntarget: Sakuya\nkeep: 30m\n---\nx",
    );
    assert_eq!(r.allowed_tools, ["Glob", "WebFetch"], "the last line wins");
    assert_eq!(r.mode.as_deref(), Some("bypassPermissions"));
    assert_eq!(r.target(), Target::Resident("Sakuya".into()));
    assert_eq!(r.keep_secs(), Some(1800));
    let r = rit("no fence at all\n");
    assert_eq!((r.schedule.as_str(), r.prompt.as_str()), ("", "no fence at all"));
    assert!(r.enabled() && r.catch_up() && !r.headless());
}

#[test]
fn odd_values_are_read_or_refused_without_a_panic() {
    let r = rit("---\nkeep: 2ч\n---\nx");
    assert_eq!(r.keep_secs(), None);
    let trust = Trust::from_json(None);
    let t = at(2026, 9, 27, 12, 0);
    let p = |r: &ritual::Ritual| ritual::problem(r, t, &ny(), &trust).unwrap_or_default();
    let r = rit("---\nschedule: \"@daily\"\ncwd: /\nkeep: 2ч\n---\nx");
    assert!(p(&r).starts_with("keep: 2ч"), "{}", p(&r));
    let r = rit("---\nallowed_tools: \"Bash(git log:*)\", Read # the two\n---\nx");
    assert_eq!(r.allowed_tools, ["Bash(git log:*)", "Read"]);
    // A misspelt key is named before what it left missing.
    let r = rit("---\nschedul: \"@daily\"\ncwd: /\n---\nx");
    assert_eq!(p(&r), "not a ritual setting: schedul");
    let r = rit("---\nschedule: \"@daily\"\ncwd: /\nallowed_tools: Read, --settings\n---\nx");
    assert!(p(&r).contains("--settings starts with -"), "{}", p(&r));
}

#[test]
fn the_journal_keeps_its_newer_half_past_its_size() {
    let d = Dir::at(&tmp("cap"), "busy");
    for i in 0..2000 {
        d.note(i, "skipped", "skipped (due 2026-09-27 09:05): the last run is still going");
    }
    let all = d.journal(0);
    assert!(all.len() < 1500 && all.len() > 400, "{}", all.len());
    assert_eq!(all.last().unwrap().at, 1999);
}

#[test]
fn every_problem_is_named() {
    let tz = ny();
    let now = at(2026, 9, 27, 12, 0);
    let dir = tmp("problem");
    let d = dir.to_string_lossy();
    let trust = Trust::from_json(Some(
        &json!({"projects": {d.as_ref(): {"hasTrustDialogAccepted": true}}}),
    ));
    let good = format!("---\nschedule: \"@daily\"\ncwd: {d}\n---\nhi\n");
    assert_eq!(ritual::problem(&rit(&good), now, &tz, &trust), None);
    let cases = [
        ("name: other\n", "name: other is not the file's name"),
        ("schedule: \"\"\n", "schedule: missing"),
        ("schedule: every 45m\n", "is not a schedule gensokyo can read"),
        ("schedule: \"0 0 30 2 *\"\n", "never comes round"),
        ("target: 3\n", "target: 3 is neither new, persistent"),
        ("headless: true\ntarget: persistent\n", "goes with target: new"),
        ("cwd: \"\"\n", "cwd: missing"),
        ("cwd: dev/x\n", "is not a full path"),
        ("cwd: /no/such/dir\n", "is not a directory"),
        ("overlap: twice\n", "overlap: twice is not a thing to do"),
        ("target: persistent\noverlap: queue\n", "is only about a target of new"),
        ("keep: 2 hours\n", "keep: 2 hours is not a length"),
        ("keep: 1234567890d\n", "is not a length"),
        ("enabled: ture\n", "enabled: ture is neither true nor false"),
        ("catch_up: maybe\n", "catch_up: maybe is neither"),
        ("colour: red\n", "not a ritual setting: colour"),
    ];
    for (line, want) in cases {
        // A later line wins, so each case overrides the good ritual.
        let text = good.replacen("---\nhi", &format!("{line}---\nhi"), 1);
        let p = ritual::problem(&rit(&text), now, &tz, &trust).unwrap_or_default();
        assert!(p.contains(want), "{line:?}: {p}");
    }
    let bare = format!("---\nschedule: \"@daily\"\ncwd: {d}\n---\n");
    assert!(ritual::problem(&rit(&bare), now, &tz, &trust).unwrap().starts_with("no prompt"));
    // A resident target has no directory to check; a slotless name must start with a letter.
    let to = "---\nschedule: \"@daily\"\ntarget: Sakuya\n---\nhi\n";
    assert_eq!(ritual::problem(&rit(to), now, &tz, &trust), None);
}

#[test]
fn trust_goes_by_the_real_directory() {
    let root = std::fs::canonicalize(tmp("real")).unwrap();
    for d in ["trusted", "other"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    let t = Trust::from_json(Some(&json!({"projects": {
        root.join("trusted").to_string_lossy(): {"hasTrustDialogAccepted": true}}})));
    assert!(t.trusted(&root.join("trusted")));
    assert!(!t.trusted(&root.join("trusted/../other")), "not inside trusted, whatever it says");
}

#[test]
fn trust_is_inherited_from_a_parent_and_headless_needs_none() {
    let tz = ny();
    let now = at(2026, 9, 27, 12, 0);
    let root = tmp("trust");
    let child = root.join("a/b");
    std::fs::create_dir_all(&child).unwrap();
    let real = std::fs::canonicalize(&root).unwrap();
    let parent = Trust::from_json(Some(
        &json!({"projects": {real.to_string_lossy(): {"hasTrustDialogAccepted": true}}}),
    ));
    let nobody = Trust::from_json(Some(&json!({"projects": {}})));
    let unreadable = Trust::from_json(None);
    assert!(parent.trusted(&child));
    assert!(!nobody.trusted(&child));
    assert!(unreadable.trusted(&child), "no file to read answers nothing");
    let text = format!("---\nschedule: \"@daily\"\ncwd: {}\n---\nhi\n", child.display());
    let p = ritual::problem(&rit(&text), now, &tz, &nobody).unwrap();
    assert!(p.contains("nothing has answered Claude Code's trust prompt"), "{p}");
    let headless = text.replacen("---\nhi", "headless: true\n---\nhi", 1);
    assert_eq!(ritual::problem(&rit(&headless), now, &tz, &nobody), None);
}

#[test]
fn prompts_and_flags() {
    let r =
        rit("---\nmodel: haiku\nmcp_config: ~/m.json\nallowed_tools: Read, Bash(x:*)\n---\nDo it.");
    let notes = Path::new("/s/rituals/morning/memory.md");
    assert_eq!(
        ritual::prompt_text(&r, notes),
        "Do it.\n\nYour notes from previous runs are at `/s/rituals/morning/memory.md`. Read them \
         first; update them before you finish."
    );
    let home = std::env::var("HOME").unwrap();
    assert_eq!(
        ritual::args(&r, Path::new("/s/rituals/morning")),
        [
            "--model",
            "haiku",
            "--mcp-config",
            &format!("{home}/m.json"),
            "--add-dir",
            "/s/rituals/morning",
            "--allowedTools",
            "Read",
            "Bash(x:*)",
            "Edit(//s/rituals/morning/memory.md)"
        ]
    );
    // The rule names the real path: the run is told the notes are under it.
    let link = tmp("args-link");
    let real = tmp("args-real");
    std::fs::remove_dir(&link).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let real = std::fs::canonicalize(&real).unwrap();
    let a = ritual::args(&rit("---\n---\nDo it."), &link);
    assert_eq!(a.last().unwrap(), &format!("Edit(/{}/memory.md)", real.display()));
    let named = rit("---\ntarget: Sakuya\n---\nDo it.");
    assert_eq!(ritual::prompt_text(&named, notes), "Do it.");
}

#[test]
fn a_rituals_dir_keeps_its_stamp_session_journal_and_runs() {
    let state = tmp("dir");
    let d = Dir::at(&state, "morning");
    assert_eq!((d.stamp(), d.session(), d.last_run()), (None, None, None));
    d.set_stamp(1_790_000_000).unwrap();
    assert_eq!(d.stamp(), Some(1_790_000_000));
    assert_eq!(d.last_run(), None, "first sight's stamp is not a run");
    d.set_session("abc").unwrap();
    assert_eq!(d.session().as_deref(), Some("abc"));
    d.note(10, "ran", "ran (due 2026-09-27 09:05)");
    d.note(20, "skipped", "skipped: the last run is still going");
    d.note(30, "sent", "sent to Sakuya");
    d.note(40, "done", "done (headless, 1m): ok");
    assert_eq!(d.last_run(), Some(30));
    assert_eq!(d.journal(0).len(), 4);
    let last2: Vec<_> = d.journal(2).into_iter().map(|e| e.ev).collect();
    assert_eq!(last2, ["sent", "done"]);
    let mem = d.memory();
    assert!(std::fs::read_to_string(&mem).unwrap().starts_with("# morning\n"));
    std::fs::write(&mem, "mine").unwrap();
    d.memory();
    assert_eq!(std::fs::read_to_string(&mem).unwrap(), "mine", "never overwritten");
    std::fs::create_dir_all(d.runs()).unwrap();
    for i in 1..=5 {
        std::fs::write(d.runs().join(format!("2026092{i}-090000.1.log")), "").unwrap();
        std::fs::write(d.runs().join(format!("2026092{i}-090000.1.json")), "").unwrap();
    }
    // The oldest is still going: its files stay whatever its age.
    std::fs::write(d.runs().join("20260921-090000.1.pid"), "").unwrap();
    d.trim_runs(2);
    let mut left: Vec<String> = std::fs::read_dir(d.runs())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [
            "20260921-090000.1.json",
            "20260921-090000.1.log",
            "20260921-090000.1.pid",
            "20260924-090000.1.json",
            "20260924-090000.1.log",
            "20260925-090000.1.json",
            "20260925-090000.1.log"
        ]
    );
}

#[test]
fn rituals_load_mine_first_and_are_found_by_part_of_a_name() {
    let root = tmp("load");
    let (mine, share) = (root.join("conf/rituals"), root.join("share"));
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::create_dir_all(share.join("rituals")).unwrap();
    std::fs::write(
        share.join("rituals/slack-morning.md"),
        "---\nschedule: \"@daily\"\n---\nshipped\n",
    )
    .unwrap();
    std::fs::write(share.join("rituals/nightly.md"), "---\nschedule: \"@daily\"\n---\nshipped\n")
        .unwrap();
    std::fs::write(mine.join("nightly.md"), "---\nschedule: \"@daily\"\n---\nmine\n").unwrap();
    std::fs::write(mine.join("bad name.md"), "x").unwrap();
    std::fs::write(mine.join(".hidden.md"), "x").unwrap();
    let (rs, bad) = ritual::load(&[mine.clone(), share.join("rituals")], Some(&share));
    let names: Vec<(&str, bool, &str)> =
        rs.iter().map(|r| (r.slug.as_str(), r.shipped, r.prompt.as_str())).collect();
    assert_eq!(names, [("nightly", false, "mine"), ("slack-morning", true, "shipped")]);
    assert_eq!(bad, [mine.join("bad name.md")]);
    assert_eq!(ritual::find(&rs, "SLACK").unwrap().slug, "slack-morning");
    assert_eq!(ritual::find(&rs, "nightly").unwrap().slug, "nightly");
    assert!(ritual::find(&rs, "i").unwrap_err().contains("which one"));
    assert!(ritual::find(&rs, "zzz").unwrap_err().contains("no ritual called 'zzz'"));
}

#[test]
fn add_writes_a_file_that_reads_back_the_same() {
    let tz = ny();
    let now = at(2026, 9, 27, 12, 0);
    let root = tmp("add");
    let (mine, share) = (root.join("rituals"), root.join("share"));
    std::fs::create_dir_all(share.join("rituals")).unwrap();
    std::fs::write(share.join("rituals/slack-morning.md"), "---\n---\nx\n").unwrap();
    let cwd = std::fs::canonicalize(&root).unwrap().to_string_lossy().into_owned();
    let trust = Trust::from_json(Some(
        &json!({"projects": {cwd.clone(): {"hasTrustDialogAccepted": true}}}),
    ));
    let a = Add {
        name: "slack-morning".into(),
        schedule: "5 9 * * 1-5".into(),
        cwd: cwd.clone(),
        description: Some("what was said: # overnight".into()),
        model: Some("haiku".into()),
        allowed_tools: vec!["mcp__claude_ai_Slack__*".into(), "Bash(a, b)".into()],
        keep: Some("30m".into()),
        prompt: "Read Slack.\n\nSay who waits.\n".into(),
        ..Add::default()
    };
    let (path, lines) = ritual::add(&a, now, &tz, &trust, &mine, Some(&share)).unwrap();
    assert!(lines[0].starts_with("wrote "), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.starts_with("warning: slack-morning is also one of the shipped"))
    );
    assert!(lines.iter().any(|l| l.contains("next fire  2026-09-28 09:05")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("is this machine's clock, now 12:00 -0400")),
        "{lines:?}"
    );
    let r = ritual::parse("slack-morning", &path, false, &std::fs::read_to_string(&path).unwrap());
    assert_eq!(r.description.as_deref(), Some("what was said: # overnight"));
    assert_eq!(r.allowed_tools, a.allowed_tools);
    assert_eq!((r.cwd.as_deref(), r.keep.as_str()), (Some(cwd.as_str()), "30m"));
    assert_eq!(r.prompt, "Read Slack.\n\nSay who waits.");
    assert_eq!(ritual::problem(&r, now, &tz, &trust), None);
    // Never over a file that is there.
    assert!(
        ritual::add(&a, now, &tz, &trust, &mine, None).unwrap_err().contains("is already there")
    );
    // A probe the install ships, and quiet runs.
    let prs = Add {
        name: "prs".into(),
        when: Some("gh-prs".into()),
        quiet: true,
        keep: Some("1m".into()),
        ..a.clone()
    };
    let (path, _) = ritual::add(&prs, now, &tz, &trust, &mine, None).unwrap();
    let r = ritual::parse("prs", &path, false, &std::fs::read_to_string(&path).unwrap());
    assert_eq!((r.when.as_deref(), r.quiet()), (Some("gh-prs"), true));
    let probe = ritual::probe(&r).unwrap().unwrap();
    assert!(probe[0].ends_with("/share/probes/gh-prs") && probe.len() == 1, "{probe:?}");
    let far = Add { name: "far".into(), when: Some("/bin/echo".into()), ..a.clone() };
    let e = ritual::add(&far, now, &tz, &trust, &mine, None).unwrap_err();
    assert!(e.starts_with("when: /bin/echo is not a probe's name"), "{e}");
    // Anything wrong but the directory writes nothing.
    let bad = Add { name: "b".into(), overlap: Some("twice".into()), ..a.clone() };
    let e = ritual::add(&bad, now, &tz, &trust, &mine, None).unwrap_err();
    assert!(e.contains("overlap: twice") && e.ends_with("nothing was written"), "{e}");
    assert!(!mine.join("b.md").exists());
    // An untrusted directory is written, with the warning.
    let nobody = Trust::from_json(Some(&json!({"projects": {}})));
    let (_, lines) =
        ritual::add(&Add { name: "c".into(), ..a.clone() }, now, &tz, &nobody, &mine, None)
            .unwrap();
    assert!(lines.iter().any(|l| l.starts_with("warning: cwd: nothing has answered")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("not firing until that is fixed")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("next fire")), "{lines:?}");
    // With a directory that is not there, nothing is written at all.
    let gone = Add { name: "g".into(), cwd: "/nope".into(), ..a.clone() };
    let e = ritual::add(&gone, now, &tz, &nobody, &mine, None).unwrap_err();
    assert!(e.contains("not a directory"), "{e}");
    assert!(!mine.join("g.md").exists());
    // Any one-line value is written so that it reads back as it was.
    let odd = r#"a "b" it's C:\x # not a comment"#;
    let q = Add { name: "q".into(), description: Some(odd.into()), ..a.clone() };
    let (path, _) = ritual::add(&q, now, &tz, &trust, &mine, None).unwrap();
    let r = ritual::parse("q", &path, false, &std::fs::read_to_string(&path).unwrap());
    assert_eq!(r.description.as_deref(), Some(odd));
    let dis = Add { name: "d".into(), disabled: true, headless: true, keep: None, ..a.clone() };
    let (_, lines) = ritual::add(&dis, now, &tz, &trust, &mine, None).unwrap();
    assert!(lines.iter().any(|l| l.contains("disabled, so it will not fire")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("headless: no pane")), "{lines:?}");
    for (a, want) in [
        (Add { name: "9 x".into(), ..a.clone() }, "cannot be a ritual name"),
        (Add { name: "e".into(), prompt: " ".into(), ..a.clone() }, "--prompt-file"),
        (Add { name: "e".into(), schedule: "0 0 30 2 *".into(), ..a.clone() }, "never comes round"),
        (
            Add { name: "e".into(), model: Some("two words".into()), ..a.clone() },
            "not a single word",
        ),
    ] {
        let e = ritual::add(&a, now, &tz, &trust, &mine, None).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
}

#[test]
fn enabling_copies_a_shipped_ritual_and_keeps_the_rest_of_the_file() {
    let root = tmp("enable");
    let (mine, share) = (root.join("rituals"), root.join("share"));
    std::fs::create_dir_all(share.join("rituals")).unwrap();
    let text = "---\n# keep this comment\nname: inbox\nschedule: \"0 8 * * 1-5\"  # weekdays\nenabled: false\n---\nRead mail.\n";
    std::fs::write(share.join("rituals/inbox.md"), text).unwrap();
    let (rs, _) = ritual::load(&[mine.clone(), share.join("rituals")], Some(&share));
    let path = ritual::mine(&rs[0], &mine).unwrap();
    assert_eq!(path, mine.join("inbox.md"));
    ritual::set_enabled(&path, true).unwrap();
    let got = std::fs::read_to_string(&path).unwrap();
    assert_eq!(got, text.replace("enabled: false", "enabled: true"));
    assert_eq!(
        std::fs::read_to_string(share.join("rituals/inbox.md")).unwrap(),
        text,
        "the example untouched"
    );
    // No line yet: added at the end of the block.
    std::fs::write(&path, "---\nschedule: \"@daily\"\n---\nx\n").unwrap();
    ritual::set_enabled(&path, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "---\nschedule: \"@daily\"\nenabled: false\n---\nx\n"
    );
    std::fs::write(&path, "just a prompt\n").unwrap();
    assert!(ritual::set_enabled(&path, true).unwrap_err().contains("no frontmatter"));
    // The template parses, arrives disabled, and names its directory.
    let t = ritual::template("fresh", "/tmp");
    let r = ritual::parse("fresh", Path::new("/x/fresh.md"), false, &t);
    assert!(!r.enabled());
    assert_eq!((r.schedule.as_str(), r.cwd.as_deref()), ("0 9 * * 1-5", Some("/tmp")));
    assert!(r.unknown.is_empty() && !r.prompt.is_empty());
}

#[test]
fn remove_takes_the_file_and_its_dir_but_never_an_example() {
    let root = tmp("remove");
    let (mine, share, state) = (root.join("rituals"), root.join("share"), root.join("state"));
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::create_dir_all(share.join("rituals")).unwrap();
    std::fs::write(mine.join("x.md"), "---\n---\nx\n").unwrap();
    std::fs::write(share.join("rituals/x.md"), "---\n---\nx\n").unwrap();
    std::fs::write(share.join("rituals/y.md"), "---\n---\ny\n").unwrap();
    let (rs, _) = ritual::load(&[mine.clone(), share.join("rituals")], Some(&share));
    let d = Dir::at(&state, "x");
    d.note(1, "ran", "ran");
    let lines = ritual::remove(&rs[0], &d, Some(&share)).unwrap();
    assert!(!mine.join("x.md").exists() && !d.path.exists());
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[2].contains("in the listing again"));
    let e = ritual::remove(&rs[1], &Dir::at(&state, "y"), Some(&share)).unwrap_err();
    assert!(e.contains("one of the examples gensokyo ships"), "{e}");
    assert!(share.join("rituals/y.md").exists());
}

#[test]
fn info_is_what_the_timetable_shows() {
    let tz = ny();
    let now = at(2026, 9, 27, 12, 0);
    let root = tmp("info");
    let d = std::fs::canonicalize(&root).unwrap();
    let trust = Trust::from_json(None);
    let on = format!(
        "---\nschedule: \"5 9 * * *\"\ncwd: {}\ndescription: morning\n---\nhi\n",
        d.display()
    );
    let dir = Dir::at(&root, "morning");
    dir.set_stamp(now - 60).unwrap();
    let i = ritual::info(&rit(&on), now, &tz, &trust, &dir);
    assert_eq!(i.next_fire, Some(at(2026, 9, 28, 9, 5)));
    assert_eq!(i.next_fire_local.as_deref(), Some("2026-09-28 09:05"));
    assert_eq!((i.last_run, i.last.clone()), (None, None));
    assert_eq!((i.target.as_str(), i.keep.as_str(), i.overlap.as_str()), ("new", "2h", "skip"));
    assert!(i.enabled && !i.headless && i.problem.is_none());
    dir.note(at(2026, 9, 27, 9, 5), "ran", "ran (due 2026-09-27 09:05)");
    let i = ritual::info(&rit(&on), now, &tz, &trust, &dir);
    assert_eq!(i.last_run, Some(at(2026, 9, 27, 9, 5)));
    assert_eq!(i.last.as_deref(), Some("2026-09-27 09:05  ran (due 2026-09-27 09:05)"));
    let off = on.replacen("---\nhi", "enabled: false\n---\nhi", 1);
    let i = ritual::info(&rit(&off), now, &tz, &trust, &dir);
    assert_eq!((i.enabled, i.next_fire), (false, None));
    let broken = on.replacen("---\nhi", "overlap: twice\n---\nhi", 1);
    let i = ritual::info(&rit(&broken), now, &tz, &trust, &dir);
    assert!(i.problem.is_some() && i.next_fire.is_none());
}
