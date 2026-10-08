//! Rituals fired by the daemon, with tests/stub-claude as every resident and the ritual clock
//! moved by hand (`GENSOKYO_NOW_FILE`, ticking every 100 ms), so nothing waits for a minute.

mod common;

use common::{Daemon, Lines, fresh, next, wait, wait_for};
use serde_json::{Value, json};
use std::io::Write;
use std::time::{Duration, Instant};

/// On the hour, UTC: `0 * * * *` fires at T0, T0 + 3600, …
const T0: i64 = 1_790_002_800;

impl Daemon {
    /// With the clock at `at` and these rituals written, started as a user's first `gensokyo`
    /// starts it.
    fn start(name: &str, at: i64, rituals: &[(&str, &str)], env: &[(&str, &str)]) -> Daemon {
        let d = Daemon::new(name, at, rituals, env);
        d.cli(&["list"]);
        d
    }

    /// The same, not started yet.
    fn new(name: &str, at: i64, rituals: &[(&str, &str)], env: &[(&str, &str)]) -> Daemon {
        let dir = fresh(&format!("f-{name}"));
        std::fs::create_dir_all(dir.join("conf/rituals")).unwrap();
        let mut extra = vec![
            ("STUB_HOOKS".to_string(), "1".to_string()),
            ("CLAUDE_CODE_CHILD_SESSION".into(), "1".into()),
            ("GENSOKYO_NOW_FILE".into(), dir.join("now").display().to_string()),
            ("GENSOKYO_TICK_MS".into(), "100".into()),
            ("GENSOKYO_IDLE_HOLD_MS".into(), "300".into()),
            ("TZ".into(), "UTC".into()),
        ];
        extra.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let d = Daemon::at(dir, extra);
        d.clock(at);
        for (slug, text) in rituals {
            d.ritual(slug, text);
        }
        d
    }

    fn clock(&self, t: i64) {
        std::fs::write(self.dir.join("now.tmp"), t.to_string()).unwrap();
        std::fs::rename(self.dir.join("now.tmp"), self.dir.join("now")).unwrap();
    }

    /// A ritual file; `@cwd` in it is the test's directory.
    fn ritual(&self, slug: &str, text: &str) {
        let text = text.replace("@cwd", &self.dir.display().to_string());
        std::fs::write(self.dir.join(format!("conf/rituals/{slug}.md")), text).unwrap();
    }

    fn verb(&self, verb: &str, name: &str) -> Value {
        self.req(json!({"t": "ritual", "id": 3, "verb": verb, "name": name}))
    }

    fn live(&self) -> Vec<Value> {
        self.list().into_iter().filter(|r| r["departed"].is_null()).collect()
    }

    fn input(&self, who: &str, text: &str) {
        // Answered only when it fails, so a list follows it: its reply is the first line then.
        let (mut w, mut lines) = self.connect();
        let input = json!({"t": "input", "id": 2, "who": who, "bytes": text.as_bytes()});
        writeln!(w, "{input}\n{}", json!({"t": "list", "id": 1})).unwrap();
        let r = next(&mut lines);
        assert_eq!(r["t"], "list", "{r}");
    }

    fn journal(&self, slug: &str) -> Vec<Value> {
        let p = self.dir.join(format!("rituals/{slug}/journal.jsonl"));
        let text = std::fs::read_to_string(p).unwrap_or_default();
        text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    fn evs(&self, slug: &str, ev: &str) -> Vec<String> {
        let j = self.journal(slug);
        j.iter().filter(|e| e["ev"] == ev).map(|e| e["text"].as_str().unwrap().into()).collect()
    }

    fn stamp(&self, slug: &str) -> Option<i64> {
        let p = self.dir.join(format!("rituals/{slug}/stamp"));
        std::fs::read_to_string(p).ok()?.trim().parse().ok()
    }

    /// Until the daemon has asked the registry since now, and had time to read the answer:
    /// whoever is up by now is listed.
    fn listed(&self) {
        let calls = || {
            let f = std::fs::read_to_string(self.dir.join("stub/agents.calls")).unwrap_or_default();
            f.lines().count()
        };
        let n = calls();
        wait_for(Duration::from_secs(15), || calls() > n, "an ask of the registry");
        self.settle();
    }

    /// A few ticks' worth, for asserting that nothing more happens.
    fn settle(&self) {
        std::thread::sleep(Duration::from_millis(600));
    }
}

fn hourly(extra: &str) -> String {
    format!(
        "---\nschedule: \"0 * * * *\"\ncwd: \"@cwd\"\n{extra}---\nDo the rounds.\nThen report.\n"
    )
}

/// Events from a `watch` connection until `f` picks one, within 15 s.
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

#[test]
fn a_due_fire_starts_one_run_with_its_flags_and_its_notes() {
    let text = hourly("allowed_tools:\n  - Read\n  - Bash(ls:*)\nmodel: haiku\nrole: reviewer\n");
    // Two minutes past the hour: this hour's fire is not this ritual's (it was not seen then).
    let d = Daemon::start("due", T0 + 120, &[("rounds", &text)], &[]);
    wait(|| d.stamp("rounds") == Some(T0 + 120), "first sight's stamp");
    d.settle();
    assert!(d.live().is_empty(), "first sight fired a past minute");

    d.clock(T0 + 3600 + 5);
    wait(|| d.live().len() == 1, "the run");
    d.settle();
    let live = d.live();
    assert_eq!(live.len(), 1, "one fire, however many ticks land in its minute");
    assert_eq!(live[0]["name"], "rounds");
    assert_eq!(d.stamp("rounds"), Some(T0 + 3600));
    assert_eq!(d.evs("rounds", "ran"), vec!["ran (due 2026-09-21 16:00)".to_string()]);

    let args = d.stub(&live[0]["id"], "args");
    let notes = d.dir.join("rituals/rounds");
    let real = std::fs::canonicalize(&notes).unwrap().join("memory.md");
    let tools = format!(
        "--add-dir {} --allowedTools Read Bash(ls:*) Edit(/{}) --session-id",
        notes.display(),
        real.display()
    );
    assert!(args.contains(&tools), "{args}");
    assert!(args.contains("--model haiku"), "{args}");
    assert!(args.contains("/share/roles/reviewer.md --model"), "{args}");
    let memory = notes.join("memory.md");
    assert!(args.trim_end().ends_with(&format!(
        "Your notes from previous runs are at `{}`. Read them first; update them before you finish.",
        memory.display()
    )));
    assert!(memory.is_file());
}

#[test]
fn a_fire_missed_while_down_is_made_up_once_at_the_start() {
    // Last run three hours before; the daemon comes up ten minutes after the newest fire.
    let d = Daemon::new("catch", T0 + 600, &[("rounds", &hourly(""))], &[]);
    std::fs::create_dir_all(d.dir.join("rituals/rounds")).unwrap();
    std::fs::write(d.dir.join("rituals/rounds/stamp"), (T0 - 3 * 3600).to_string()).unwrap();
    d.cli(&["list"]);
    wait(|| d.live().len() == 1, "the catch-up run");
    d.settle();
    assert_eq!(d.live().len(), 1);
    assert_eq!(d.evs("rounds", "ran"), vec!["ran (catch-up 2026-09-21 15:00)".to_string()]);
    assert_eq!(d.stamp("rounds"), Some(T0));
}

#[test]
fn a_lid_shut_through_fires_makes_up_the_newest_once_and_the_next_comes_as_usual() {
    let text = "---\nschedule: every 10m\nheadless: true\ncwd: \"@cwd\"\n---\nSay the time.\n";
    let d = Daemon::start("lid", T0 + 120, &[("lid", text)], &[]);
    wait(|| d.stamp("lid") == Some(T0 + 120), "first sight's stamp");
    let done = |n: usize| d.evs("lid", "done").len() == n;
    d.clock(T0 + 600 + 5);
    wait(|| done(1), "the 15:10 run");

    // Shut at 15:12, open at 15:37: 15:20 and 15:30 went by while the daemon was not ticking.
    d.clock(T0 + 37 * 60 + 20);
    wait(|| done(2), "the catch-up run");
    d.settle();
    let ran = ["ran (due 2026-09-21 15:10)", "ran (catch-up 2026-09-21 15:30)"];
    assert_eq!(d.evs("lid", "ran"), ran, "one run for both misses");
    assert_eq!(d.stamp("lid"), Some(T0 + 1800));
    let log = std::fs::read_to_string(d.dir.join("daemon.log")).unwrap();
    assert!(log.contains(r#""gap_s":1635"#), "the gap is logged");

    // The next fire is due as usual, neither swallowed nor doubled.
    d.clock(T0 + 2400 + 5);
    wait(|| done(3), "the 15:40 run");
    // Shut again between fires, 15:41 to 15:48: nothing was missed, so nothing runs.
    d.clock(T0 + 48 * 60);
    d.settle();
    assert_eq!(d.evs("lid", "ran").len(), 3, "{:?}", d.evs("lid", "ran"));
    assert_eq!(d.stamp("lid"), Some(T0 + 2400));
}

#[test]
fn overlap_skips_queues_or_runs_alongside_a_run_still_going() {
    let rituals = [
        ("skipper", hourly("")),
        ("queuer", hourly("overlap: queue\n")),
        ("twin", hourly("overlap: parallel\n")),
    ];
    let rituals: Vec<(&str, &str)> = rituals.iter().map(|(a, b)| (*a, b.as_str())).collect();
    let d = Daemon::start("overlap", T0 + 120, &rituals, &[("STUB_HOLD", "1")]);
    wait(|| d.stamp("twin").is_some(), "first sight");
    d.clock(T0 + 3600 + 5);
    wait(|| d.live().len() == 3, "three runs");
    d.clock(T0 + 7200 + 5);
    wait(|| d.live().len() == 4, "the parallel run");
    wait(|| !d.evs("queuer", "queued").is_empty(), "the queued fire");
    d.settle();
    assert_eq!(d.live().len(), 4);
    assert_eq!(
        d.evs("skipper", "skipped"),
        vec!["skipped (due 2026-09-21 17:00): the last run is still going".to_string()]
    );
    assert_eq!(
        d.evs("twin", "ran")[1],
        "ran (due 2026-09-21 17:00), alongside the run that was still going"
    );

    // The run in the queue's way finishes: the fire it held goes now.
    d.input("queuer", "/finish\r");
    wait(|| d.live().len() == 5, "the queued run");
    assert_eq!(d.evs("queuer", "ran")[1], "ran (queued 2026-09-21 17:00)");
}

#[test]
fn keep_closes_a_finished_run_and_typing_puts_its_life_back() {
    let d = Daemon::start("keep", T0 + 120, &[("rounds", &hourly("keep: 30m\n"))], &[]);
    wait(|| d.stamp("rounds").is_some(), "first sight");
    d.clock(T0 + 3600 + 5);
    wait(|| d.live().len() == 1, "the run");
    let id = d.live()[0]["id"].clone();
    d.stub(&id, "ready");
    // Finished only once the registry lists it: not stuck at a trust dialog.
    d.listed();
    // Finished at +5; typed into at +1000, and finished again at once.
    d.clock(T0 + 3600 + 1000);
    d.settle();
    d.input("rounds", "one more thing\r");
    // The daemon has heard the turn's hooks: one still on its way would come in the middle of
    // the /exit, which then holds back its Enter and leaves the run be.
    wait(|| d.live()[0]["turns"] == 2, "the typed turn");
    d.settle();
    d.clock(T0 + 3600 + 5 + 1800 + 10);
    d.settle();
    assert_eq!(d.live().len(), 1, "taken while its life had been put back");
    d.clock(T0 + 3600 + 1000 + 1800 + 5);
    // The stub may miss the first /exit after the Ctrl-C; the second is 8 s behind it.
    wait_for(Duration::from_secs(40), || d.list().is_empty(), "the run to leave the shrine");
    assert_eq!(
        d.evs("rounds", "closed"),
        vec!["closed rounds, idle 30m since the run finished (keep)".to_string()]
    );
    // Retired, so it is still there to recall.
    let all = d.req(json!({"t": "list", "id": 1, "all": true}))["residents"].clone();
    assert_eq!(all[0]["name"], "rounds");
}

#[test]
fn a_persistent_ritual_keeps_one_session_and_recalls_it() {
    let d = Daemon::start("persist", T0 + 120, &[("diary", &hourly("target: persistent\n"))], &[]);
    wait(|| d.stamp("diary").is_some(), "first sight");
    d.clock(T0 + 3600 + 5);
    wait(|| d.live().len() == 1, "the session");
    let id = d.live()[0]["id"].clone();
    let session =
        || std::fs::read_to_string(d.dir.join("rituals/diary/session")).unwrap_or_default();
    wait(|| session().trim() == id.as_str().unwrap(), "the session file");

    d.clock(T0 + 7200 + 5);
    wait(|| d.evs("diary", "sent").len() == 1, "the prompt typed in");
    assert!(d.stub(&id, "input").contains("Do the rounds."));
    assert_eq!(d.live().len(), 1);

    let r = d.req(json!({"t": "banish", "id": 4, "who": "diary"}));
    assert_eq!(r["t"], "done", "{r}");
    d.clock(T0 + 10800 + 5);
    wait(|| d.evs("diary", "sent").len() == 2, "the prompt typed into the recalled session");
    let live = d.live();
    assert_eq!((live.len(), &live[0]["id"]), (1, &id), "the same record, recalled");
    assert!(d.stub(&id, "args").contains("--resume"));
}

#[test]
fn a_named_resident_gets_the_prompt_alone_and_a_missing_one_is_reported() {
    let rituals = [("ask", hourly("target: Sakuya\n")), ("lost", hourly("target: Nobody\n"))];
    let rituals: Vec<(&str, &str)> = rituals.iter().map(|(a, b)| (*a, b.as_str())).collect();
    let d = Daemon::start("named", T0 + 120, &rituals, &[]);
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Sakuya"}));
    assert_eq!(r["t"], "summoned", "{r}");
    let id = r["resident"]["id"].clone();
    d.stub(&id, "ready");
    wait(|| d.stamp("lost").is_some(), "first sight");
    let (mut w, mut events) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();

    d.clock(T0 + 3600 + 5);
    let n = watch_for(&mut events, |v| v["t"] == "notice");
    assert_eq!(
        n["text"],
        "⏲ lost: there is no resident called Nobody, so the fire was not delivered"
    );
    wait(|| d.evs("ask", "sent").len() == 1, "the prompt typed in");
    let input = d.stub(&id, "input");
    assert!(input.contains("Do the rounds.") && !input.contains("Your notes"), "{input}");
    assert_eq!(
        d.evs("lost", "not-sent"),
        vec!["not sent: there is no resident called Nobody".to_string()]
    );
    // Kept for clients to come, as heard: one was watching. When it came is on the wall clock.
    let text = n["text"].as_str().unwrap().to_string();
    assert_eq!(kept(&d), [(text, false)]);
    let (mut w, mut events) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    let at = watch_for(&mut events, |v| v["t"] == "notices")["notices"][0]["at"].as_i64().unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    assert!((now.as_secs() as i64 - at).abs() < 60, "{at}");
}

/// The notices a client that starts watching now is handed, newest first: each one's text, and
/// whether no client had it before.
fn kept(d: &Daemon) -> Vec<(String, bool)> {
    let (mut w, mut events) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    let n = watch_for(&mut events, |v| v["t"] == "notices");
    let all = n["notices"].as_array().unwrap().iter();
    all.map(|n| (n["text"].as_str().unwrap().into(), n["missed"] == true)).collect()
}

#[test]
fn notices_nobody_watched_are_missed_until_a_client_has_them_and_only_the_last_twenty_kept() {
    let rituals: Vec<(String, String)> = (0..22)
        .map(|i| (format!("lost{i:02}"), hourly(&format!("target: Nobody{i:02}\n"))))
        .collect();
    let rituals: Vec<(&str, &str)> =
        rituals.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let d = Daemon::start("kept", T0 + 120, &rituals, &[]);
    wait(|| (0..22).all(|i| d.stamp(&format!("lost{i:02}")).is_some()), "first sight");
    assert_eq!(kept(&d), []);
    d.clock(T0 + 3600 + 5);
    let fired = || (0..22).all(|i| d.evs(&format!("lost{i:02}"), "not-sent").len() == 1);
    wait(fired, "every fire");
    let first = kept(&d);
    assert_eq!(first.len(), 20);
    assert!(first.iter().all(|(t, missed)| *missed && t.contains("so the fire was not delivered")));
    // Had once, they are heard: the next client gets them all the same.
    let second: Vec<(String, bool)> = first.into_iter().map(|(t, _)| (t, false)).collect();
    assert_eq!(kept(&d), second);
}

fn headless_run(name: &str, env: &[(&str, &str)]) -> (Daemon, String) {
    let text = hourly("headless: true\nrole: researcher\n");
    let d = Daemon::start(name, T0 + 120, &[("quiet", &text)], env);
    wait(|| d.stamp("quiet").is_some(), "first sight");
    d.clock(T0 + 3600 + 5);
    wait(|| d.journal("quiet").iter().any(|e| e["ev"] == "done" || e["ev"] == "failed"), "the run");
    let runs = d.dir.join("rituals/quiet/runs");
    let log = std::fs::read_dir(&runs)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "log"))
        .unwrap();
    let log = std::fs::read_to_string(log).unwrap();
    assert!(d.live().is_empty(), "a headless run has no resident");
    (d, log)
}

#[test]
fn what_a_headless_run_leaves_behind_goes_with_it() {
    let (d, _) = headless_run("hl-left", &[("STUB_P_LEAVE", "1")]);
    let left = std::fs::read_to_string(d.dir.join("stub/left.pid")).unwrap();
    let left: i32 = left.trim().parse().unwrap();
    wait(|| !alive(left), "what the run left behind to go");
}

#[test]
fn a_headless_run_logs_what_it_said_and_what_it_was_refused() {
    let (d, log) = headless_run("headless", &[("STUB_P_DENY", "Bash")]);
    assert!(log.contains("stub -p ran in"), "{log}");
    let args = std::fs::read_to_string(d.dir.join("stub/print.args")).unwrap();
    assert!(
        args.contains("--append-system-prompt-file /")
            && args.contains("/share/roles/researcher.md --"),
        "{args}"
    );
    assert!(log.contains(
        "refused  Bash - a run with nobody to ask needs it in the ritual's allowed_tools"
    ));
    assert!(log.contains(
        "to read the whole transcript: claude --resume 11111111-2222-3333-4444-555555555555"
    ));
    assert!(log.contains(", 2 turns"));
    let done = d.evs("quiet", "done");
    assert!(
        done[0].starts_with("done (headless, ") && done[0].contains(": stub -p ran in "),
        "{done:?}"
    );
    let args = std::fs::read_to_string(d.dir.join("stub/print.args")).unwrap();
    assert!(
        args.starts_with(
            "-p --output-format json --disallowed-tools CronCreate CronList CronDelete"
        ),
        "{args}"
    );
    assert!(args.contains("--add-dir"));
}

#[test]
fn a_headless_run_that_says_too_much_is_cut_but_its_answer_is_kept_whole() {
    let answer = "y".repeat(5000);
    let env = [
        ("GENSOKYO_RUN_BYTES", "2048"),
        ("STUB_P_NOISE", "9000"),
        ("STUB_P_RESULT", answer.as_str()),
    ];
    let (d, log) = headless_run("hl-much", &env);
    assert!(log.contains("--- cut at 2 KB: all of it is in "), "{log}");
    assert!(!log.contains(&"y".repeat(2049)), "{} bytes of log", log.len());
    let file = |ext: &str| {
        let runs = std::fs::read_dir(d.dir.join("rituals/quiet/runs")).unwrap().flatten();
        let mut f = runs.map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == ext));
        std::fs::read_to_string(f.next().unwrap()).unwrap()
    };
    let err = file("err");
    assert!(err.ends_with("\n--- cut here by gensokyo, past 2 KB\n"), "{err}");
    assert_eq!(err.len(), 2048 + "\n--- cut here by gensokyo, past 2 KB\n".len());
    assert!(file("json").contains(&answer), "the answer itself is whole");
}

#[test]
fn a_headless_run_that_fails_says_so_either_way() {
    let (d, log) = headless_run("hl-fail", &[("STUB_P_FAIL", "no login")]);
    assert!(log.contains("--- claude exited 1, and said:\nstub-claude: no login"), "{log}");
    let failed = d.evs("quiet", "failed");
    assert!(failed[0].starts_with("failed (headless, "), "{failed:?}");
    assert!(failed[0].ends_with("): claude exited 1: stub-claude: no login"), "{failed:?}");
    drop(d);
    let (d, log) =
        headless_run("hl-error", &[("STUB_P_ERROR", "There's an issue with the selected model")]);
    assert!(log.contains("There's an issue with the selected model"), "{log}");
    let failed = d.evs("quiet", "failed");
    assert!(
        failed[0].ends_with("claude exited 1: There's an issue with the selected model"),
        "{failed:?}"
    );
}

#[test]
fn a_headless_run_past_its_limit_is_stopped_and_frees_its_ritual() {
    let env = [("STUB_P_SLEEP", "30"), ("GENSOKYO_HEADLESS_MS", "500")];
    let (d, log) = headless_run("hl-limit", &env);
    assert!(log.contains("--- stopped: still running after "), "{log}");
    let failed = d.evs("quiet", "failed");
    assert!(failed[0].ends_with(": still running, so it was stopped"), "{failed:?}");
    let r = d.verb("run", "quiet");
    assert_eq!(r["t"], "done", "{r}");
}

/// The `.pid` file of the one headless run in `runs`, and the pid in it.
fn pid_file(runs: &std::path::Path) -> Option<(std::path::PathBuf, i32)> {
    let files = std::fs::read_dir(runs).into_iter().flatten().flatten();
    let p = files.map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "pid"))?;
    let text = std::fs::read_to_string(&p).ok()?;
    Some((p, text.split_whitespace().next()?.parse().ok()?))
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only asks whether it is there.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[test]
fn a_headless_run_outlives_its_daemon_and_the_next_one_sees_it_out() {
    let env = [("STUB_P_SLEEP", "30"), ("GENSOKYO_HEADLESS_MS", "4000")];
    let d = Daemon::start("hl-adopt", T0 + 120, &[("quiet", &hourly("headless: true\n"))], &env);
    wait(|| d.stamp("quiet").is_some(), "first sight");
    d.clock(T0 + 3600 + 5);
    let runs = d.dir.join("rituals/quiet/runs");
    wait(|| pid_file(&runs).is_some(), "the run's pid file");
    let (_, pid) = pid_file(&runs).unwrap();
    d.cli(&["quit"]);
    assert!(alive(pid), "the run went with its daemon");

    // The next daemon counts it as running, and stops it at the limit its start set.
    d.cli(&["list"]);
    let r = d.verb("run", "quiet");
    assert_eq!(r["t"], "error", "{r}");
    assert!(r["error"].as_str().unwrap().contains("running headless right now"), "{r}");
    wait(|| !d.evs("quiet", "failed").is_empty(), "the adopted run to be stopped");
    let failed = d.evs("quiet", "failed");
    assert!(failed[0].ends_with(": still running, so it was stopped"), "{failed:?}");
    wait(|| !alive(pid), "the run to be gone");
    assert!(pid_file(&runs).is_none(), "its pid file stayed");
    assert!(d.log().iter().any(|l| l["adopted"] == pid), "no adopted line in the daemon's log");
}

#[test]
fn a_headless_run_that_ended_between_daemons_is_journaled_by_the_next() {
    // Its stderr is cut as it is seen out, which leaves the time of its last write alone.
    let env = [("STUB_P_SLEEP", "2"), ("STUB_P_NOISE", "9000"), ("GENSOKYO_RUN_BYTES", "2048")];
    let d = Daemon::start("hl-between", T0 + 120, &[("quiet", &hourly("headless: true\n"))], &env);
    wait(|| d.stamp("quiet").is_some(), "first sight");
    d.clock(T0 + 3600 + 5);
    let runs = d.dir.join("rituals/quiet/runs");
    wait(|| pid_file(&runs).is_some(), "the run's pid file");
    let (_, pid) = pid_file(&runs).unwrap();
    d.cli(&["quit"]);
    // It finishes, and writes its result, while no daemon is there.
    wait(|| !alive(pid), "the run to finish on its own");
    assert!(d.evs("quiet", "done").is_empty());
    // A while later: the time it took is to its last write, not to the next daemon.
    std::thread::sleep(Duration::from_secs(4));
    d.cli(&["list"]);
    wait(|| !d.evs("quiet", "done").is_empty(), "the next daemon's journal line");
    let done = d.evs("quiet", "done");
    assert!(done[0].contains(": stub -p ran in "), "{done:?}");
    let secs = done[0].strip_prefix("done (headless, ").and_then(|t| t.split('s').next());
    assert!(secs.and_then(|s| s.parse::<u64>().ok()).is_some_and(|s| s <= 4), "{done:?}");
    assert!(pid_file(&runs).is_none(), "its pid file stayed");
}

#[test]
fn a_ritual_that_cannot_fire_says_so_once() {
    let text = "---\nschedule: \"0 * * * *\"\ncwd: /no/such/dir\n---\nDo the rounds.\n";
    let d = Daemon::start("problem", T0 + 120, &[("broken", text)], &[]);
    wait(|| d.stamp("broken").is_some(), "first sight");
    let (mut w, mut events) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    d.clock(T0 + 3600 + 5);
    let n = watch_for(&mut events, |v| v["t"] == "notice");
    assert_eq!(n["text"], "⏲ broken: cwd: /no/such/dir is not a directory");
    d.settle();
    assert_eq!(
        d.evs("broken", "not-run"),
        vec!["not run: cwd: /no/such/dir is not a directory".to_string()]
    );
    // Not stamped: it has not run.
    assert_eq!(d.stamp("broken"), Some(T0 + 120));
}

#[test]
fn the_timetable_verbs_and_its_push() {
    let d = Daemon::start("verbs", T0 + 120, &[("mine", &hourly("enabled: false\n"))], &[]);
    let (mut w, mut events) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    let first = watch_for(&mut events, |v| v["t"] == "rituals");
    let names: Vec<&str> =
        first["rituals"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"mine") && names.contains(&"slack-morning"), "{names:?}");

    // By hand, disabled or not.
    let r = d.verb("run", "mine");
    assert_eq!(r["t"], "done", "{r}");
    assert!(
        r["message"].as_str().unwrap().contains("mine is disabled: this run is by hand"),
        "{r}"
    );
    wait(|| d.live().len() == 1, "the run by hand");
    assert_eq!(d.evs("mine", "ran"), vec!["ran (by hand)".to_string()]);

    // A shipped example becomes the user's own before it is changed.
    let r = d.verb("enable", "slack-morning");
    assert_eq!(r["t"], "done", "{r}");
    let copy = d.dir.join("conf/rituals/slack-morning.md");
    assert!(std::fs::read_to_string(&copy).unwrap().contains("enabled: true"));
    let pushed = watch_for(&mut events, |v| {
        v["t"] == "rituals"
            && v["rituals"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "slack-morning" && r["enabled"] == true)
    });
    assert_eq!(pushed["id"], 0);
    assert_eq!(d.verb("disable", "slack-morning")["t"], "done");
    assert!(std::fs::read_to_string(&copy).unwrap().contains("enabled: false"));

    let r = d.verb("remove", "inbox-zero");
    assert!(r["error"].as_str().unwrap().contains("one of the examples gensokyo ships"), "{r}");
    let r = d.verb("remove", "min");
    assert!(r["error"].as_str().unwrap().contains("remove wants the whole name"), "{r}");
    // Refused while its run is going: finished means the registry lists it, too.
    let mut r = Value::Null;
    wait(
        || {
            r = d.verb("remove", "mine");
            r["t"] == "done"
        },
        "the run to finish",
    );
    assert!(!d.dir.join("conf/rituals/mine.md").exists());
    assert!(!d.dir.join("rituals/mine").exists());
    assert!(r["message"].as_str().unwrap().contains("a run of it is still in the shrine"), "{r}");
}

#[test]
fn catch_up_false_makes_up_nothing() {
    let d = Daemon::new("no-catch", T0 + 600, &[("rounds", &hourly("catch_up: false\n"))], &[]);
    std::fs::create_dir_all(d.dir.join("rituals/rounds")).unwrap();
    std::fs::write(d.dir.join("rituals/rounds/stamp"), (T0 - 3 * 3600).to_string()).unwrap();
    d.cli(&["list"]);
    wait(|| d.req(json!({"t": "rituals", "id": 1}))["t"] == "rituals", "the daemon");
    d.settle();
    assert!(d.live().is_empty(), "caught up although catch_up is false");
    assert!(d.evs("rounds", "ran").is_empty());
}

#[test]
fn a_run_that_cannot_start_says_so_once() {
    let d = Daemon::start(
        "no-start",
        T0 + 120,
        &[("rounds", &hourly(""))],
        &[("GENSOKYO_CLAUDE", "/no/such/claude")],
    );
    wait(|| d.stamp("rounds").is_some(), "first sight");
    let (mut w, mut events) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    d.clock(T0 + 3600 + 5);
    let n = watch_for(&mut events, |v| v["t"] == "notice");
    assert!(n["text"].as_str().unwrap().contains("could not start /no/such/claude"), "{n}");
    d.clock(T0 + 7200 + 5);
    wait(|| d.stamp("rounds") == Some(T0 + 7200), "the second fire");
    d.settle();
    let not = d.evs("rounds", "not-run");
    assert_eq!(not.len(), 1, "said once: {not:?}");
    assert!(d.evs("rounds", "ran").is_empty());
}

#[test]
fn a_recalled_run_does_not_hold_up_the_next_fire() {
    let d = Daemon::start("recalled", T0 + 120, &[("rounds", &hourly(""))], &[]);
    wait(|| d.stamp("rounds").is_some(), "first sight");
    d.clock(T0 + 3600 + 5);
    wait(|| d.live().len() == 1, "the run");
    let id = d.live()[0]["id"].clone();
    d.stub(&id, "ready");
    let r = d.req(json!({"t": "banish", "id": 4, "who": "rounds"}));
    assert_eq!(r["t"], "done", "{r}");
    let r = d.req(json!({"t": "recall", "id": 5, "who": "rounds"}));
    assert_eq!(r["t"], "summoned", "{r}");
    wait(|| d.stub(&id, "args").contains("--resume"), "the resumed session");
    // Listed by the registry, so it is not at a trust dialog.
    d.listed();
    d.clock(T0 + 7200 + 5);
    wait(|| d.evs("rounds", "ran").len() == 2, "the next run");
    assert!(d.evs("rounds", "skipped").is_empty());
}

#[test]
fn the_queue_is_one_fire_deep_and_drops_one_an_hour_late() {
    // `late` fires at 16:00 and 17:00 only, so nothing newer takes its queued fire's place.
    let late = hourly("overlap: queue\n").replace("0 * * * *", "0 16,17 * * *");
    let rituals = [("queuer", hourly("overlap: queue\n")), ("late", late)];
    let rituals: Vec<(&str, &str)> = rituals.iter().map(|(a, b)| (*a, b.as_str())).collect();
    let d = Daemon::start("queue", T0 + 120, &rituals, &[("STUB_HOLD", "1")]);
    wait(|| d.stamp("late").is_some(), "first sight");
    d.clock(T0 + 3600 + 5);
    wait(|| d.live().len() == 2, "two runs, held");
    d.clock(T0 + 7200 + 5);
    wait(|| d.evs("queuer", "queued").len() == 1 && d.evs("late", "queued").len() == 1, "17:00");
    // Newest wins: 18:00 takes 17:00's place behind the same run.
    d.clock(T0 + 10800 + 5);
    wait(|| d.evs("queuer", "queued").len() == 2, "18:00 queued");
    // Over an hour after the fire `late` holds.
    d.clock(T0 + 10800 + 125);
    wait(|| !d.evs("late", "dropped").is_empty(), "the late fire dropped");
    assert_eq!(
        d.evs("late", "dropped"),
        vec![
            "dropped the fire queued for 2026-09-21 17:00: the run in its way took over 1h"
                .to_string()
        ]
    );
    d.input("queuer", "/finish\r");
    wait(|| d.evs("queuer", "ran").len() == 2, "the queued run");
    d.settle();
    let ran = d.evs("queuer", "ran");
    assert_eq!(ran, vec!["ran (due 2026-09-21 16:00)", "ran (queued 2026-09-21 18:00)"]);
    d.input("late", "/finish\r");
    d.settle();
    assert_eq!(d.evs("late", "ran").len(), 1, "a dropped fire does not run once the way is clear");
}

#[test]
fn a_resident_at_a_dialog_is_not_typed_into() {
    let ask = hourly("target: Sakuya\ndeliver: now\n");
    let d = Daemon::start("dialog", T0 + 120, &[("ask", &ask)], &[]);
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Sakuya"}));
    let id = r["resident"]["id"].clone();
    d.stub(&id, "ready");
    // As Claude Code's registry shows a dialog while it is open.
    std::fs::write(d.dir.join(format!("stub/{}.status", id.as_str().unwrap())), "waiting").unwrap();
    d.input("Sakuya", "/perm\r");
    wait(|| d.live()[0]["state"] == "awaits", "the permission dialog");
    wait(|| d.stamp("ask").is_some(), "first sight");
    let (mut w, mut events) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    d.clock(T0 + 3600 + 5);
    let n = watch_for(&mut events, |v| v["t"] == "notice");
    assert!(n["text"].as_str().unwrap().contains("Sakuya has a dialog waiting for you"), "{n}");
    let not = d.evs("ask", "not-sent");
    assert_eq!(not.len(), 1, "{not:?}");
    assert!(!d.stub(&id, "input").contains("Do the rounds."), "typed into a dialog");
}

#[test]
fn a_directory_claude_code_has_not_trusted_is_refused_by_the_clock() {
    let d = Daemon::new("untrusted", T0 + 120, &[("rounds", &hourly(""))], &[]);
    std::fs::create_dir_all(d.dir.join("claude")).unwrap();
    std::fs::write(d.dir.join("claude/.claude.json"), r#"{"projects": {}}"#).unwrap();
    d.cli(&["list"]);
    wait(|| d.stamp("rounds").is_some(), "first sight");
    d.clock(T0 + 3600 + 5);
    wait(|| !d.evs("rounds", "not-run").is_empty(), "the refusal");
    d.clock(T0 + 7200 + 5);
    d.settle();
    let not = d.evs("rounds", "not-run");
    assert_eq!(not.len(), 1, "said once: {not:?}");
    assert!(
        not[0].starts_with("not run: cwd: nothing has answered Claude Code's trust"),
        "{not:?}"
    );
    assert!(d.live().is_empty());
}

/// Started at T0 with a minutely ritual on a probe in the config dir's `probes/`, which counts
/// its calls in `calls`, prints `feed`, fails with the status in `fail`, and with `slow` there
/// hangs with a child of its own (its pid in `kid`). All in the ritual's cwd.
fn feeding(name: &str, extra: &str, env: &[(&str, &str)]) -> Daemon {
    let text = format!(
        "---\nschedule: \"* * * * *\"\ncwd: \"@cwd\"\nwhen: feed\noverlap: parallel\n{extra}---\n\
         Keep the page current.\n"
    );
    let d = Daemon::new(name, T0 + 5, &[("watch", &text)], env);
    let probes = d.dir.join("conf/probes");
    std::fs::create_dir_all(&probes).unwrap();
    let script = "#!/bin/sh\n\
        echo >> calls\n\
        if [ -f fail ]; then echo boom >&2; exit \"$(cat fail)\"; fi\n\
        if [ -f slow ]; then sleep 30 & echo $! > kid; wait; fi\n\
        cat feed\n";
    std::fs::write(probes.join("feed"), script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(probes.join("feed"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(d.dir.join("feed"), "line-one\n").unwrap();
    d.cli(&["list"]);
    // Due in the minute it starts in: the first fire, and the first time always fires.
    wait(|| d.runs() == 1, "the first fire");
    d
}

impl Daemon {
    /// Resident `id` mid-turn, as Claude Code's registry shows it, until it is told otherwise:
    /// then a prompt and its Stop end the turn.
    fn busy(&self, id: &Value, on: bool) {
        let status = self.dir.join(format!("stub/{}.status", id.as_str().unwrap()));
        std::fs::write(&status, if on { "busy" } else { "idle" }).unwrap();
        // Typing is what has the registry asked again.
        let name = self.live()[0]["name"].as_str().unwrap().to_string();
        self.input(&name, if on { "work\r" } else { "and done\r" });
        if on {
            self.listed();
            wait(|| self.live()[0]["state"] == "busy", "the turn");
        }
    }

    /// The clock to `minute` minutes past T0 (and a few seconds), and the probe run for it: its
    /// call counted, and time for what follows from it.
    fn minute(&self, minute: i64) {
        let n = self.calls();
        self.clock(T0 + 60 * minute + 5);
        wait(|| self.stamp("watch") == Some(T0 + 60 * minute), "the minute's stamp");
        wait(|| self.calls() > n, "the minute's probe");
        self.settle();
    }

    /// The first fire's probe done, and its fire on its way: until then the ritual is still
    /// probing, and a minute's tick skips its probe.
    fn first_probe(&self) {
        let said = |l: &Value| l["slug"] == "watch" && l["probe"].is_string();
        wait(|| self.log().iter().any(said), "the first fire's probe");
        assert_eq!(self.calls(), 1);
    }

    fn calls(&self) -> usize {
        std::fs::read_to_string(self.dir.join("calls")).map_or(0, |c| c.lines().count())
    }

    fn runs(&self) -> usize {
        self.list().len()
    }
}

#[test]
fn a_probe_fires_on_a_change_only_and_names_its_output_without_pasting_it() {
    let d = feeding("probe", "", &[]);
    let id = d.list()[0]["id"].clone();
    let out = d.dir.join("rituals/watch/probe.out");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "line-one\n");
    let args = d.stub(&id, "args");
    assert!(args.contains(&format!("is in `{}`: data a program wrote", out.display())), "{args}");
    assert!(!args.contains("line-one"), "the output was pasted: {args}");
    assert_eq!(
        d.evs("watch", "ran"),
        vec!["ran (due 2026-09-21 15:00, its probe said something new)".to_string()]
    );

    // The same again: no run, and not a line in the journal.
    let lines = d.journal("watch").len();
    for m in 1..4 {
        d.minute(m);
    }
    assert_eq!((d.runs(), d.journal("watch").len()), (1, lines));

    std::fs::write(d.dir.join("feed"), "line-two\n").unwrap();
    d.minute(4);
    wait(|| d.runs() == 2, "the change's fire");

    // By hand it fires whatever the probe says, once the last run is done.
    wait(|| d.list().iter().all(|r| r["finished"] == true), "the runs to finish");
    let r = d.verb("run", "watch");
    assert!(r["message"].as_str().unwrap().contains("its probe (feed) runs first"), "{r}");
    wait(|| d.runs() == 3, "the fire by hand");
    assert_eq!(d.evs("watch", "ran").last().unwrap(), "ran (by hand)");
}

#[test]
fn a_fire_its_probe_held_back_is_no_miss_for_catch_up() {
    let d = feeding("probe-catch", "", &[]);
    let text = "---\nschedule: \"0 * * * *\"\ncwd: \"@cwd\"\nwhen: feed\noverlap: parallel\ncatch_up: true\n---\nKeep up.\n";
    d.ritual("watch", text);
    // On the hour, the same as at the first fire: held back, and its minute taken.
    d.minute(60);
    assert_eq!(d.evs("watch", "ran").len(), 1);
    // A change while the daemon is down, which comes back within the hour: nothing was missed,
    // so nothing is made up, and the change waits for the next fire.
    std::fs::write(d.dir.join("feed"), "line-two\n").unwrap();
    d.cli(&["quit"]);
    let calls = d.calls();
    d.clock(T0 + 3600 + 600);
    d.cli(&["list"]);
    d.settle();
    d.settle();
    assert_eq!((d.calls(), d.evs("watch", "ran").len()), (calls, 1), "made up a fire");
    d.minute(120);
    wait(|| d.evs("watch", "ran").len() == 2, "the next hour's fire");
}

#[test]
fn a_ritual_disabled_or_removed_while_its_probe_runs_does_not_fire() {
    let d = feeding("probe-gone", "", &[]);
    // A probe held up by a child of its own, which then lets it finish with a change to say.
    let held = |minute: i64| {
        std::fs::write(d.dir.join("slow"), "").unwrap();
        d.clock(T0 + 60 * minute + 5);
        wait(|| d.dir.join("kid").exists(), "the probe under way");
    };
    let let_go = || {
        std::fs::remove_file(d.dir.join("slow")).unwrap();
        let kid = std::fs::read_to_string(d.dir.join("kid")).unwrap();
        std::fs::remove_file(d.dir.join("kid")).unwrap();
        unsafe { libc::kill(kid.trim().parse().unwrap(), libc::SIGKILL) };
    };
    std::fs::write(d.dir.join("feed"), "line-two\n").unwrap();
    held(1);
    let r = d.verb("disable", "watch");
    assert_eq!(r["t"], "done", "{r}");
    let_go();
    d.settle();
    assert_eq!((d.runs(), d.evs("watch", "ran").len()), (1, 1), "fired once disabled");

    // Enabled again, the change it held is still news.
    d.verb("enable", "watch");
    d.minute(2);
    wait(|| d.runs() == 2, "the change's fire");

    // Its file deleted while the probe runs: no fire either.
    std::fs::write(d.dir.join("feed"), "line-three\n").unwrap();
    held(3);
    std::fs::remove_file(d.dir.join("conf/rituals/watch.md")).unwrap();
    let_go();
    d.settle();
    assert_eq!(d.runs(), 2, "fired once removed");
}

#[test]
fn a_change_turned_away_by_a_run_still_going_fires_at_the_next_tick() {
    let text = "---\nschedule: \"* * * * *\"\ncwd: \"@cwd\"\nwhen: feed\n---\nKeep up.\n";
    let d = feeding("probe-skip", "", &[("STUB_HOLD", "1")]);
    d.ritual("watch", text);
    std::fs::write(d.dir.join("feed"), "line-two\n").unwrap();
    // Turned away before its probe runs, and quietly: the probe has said nothing yet.
    let calls = d.calls();
    d.clock(T0 + 60 + 5);
    wait(|| d.stamp("watch") == Some(T0 + 60), "the minute's stamp");
    d.settle();
    assert_eq!((d.runs(), d.calls()), (1, calls));
    assert!(d.evs("watch", "skipped").is_empty());
    d.input("watch", "/finish\r");
    wait(|| d.list()[0]["finished"] == true, "the run to finish");
    d.minute(2);
    wait(|| d.runs() == 2, "the change, a minute late");
}

#[test]
fn a_failing_probe_fires_once_then_again_when_it_works_and_a_slow_one_is_killed_whole() {
    // Room for the first run of a script just written, which a busy machine scans for seconds.
    let d = feeding("probe-fail", "", &[("GENSOKYO_PROBE_LIMIT_MS", "6000")]);
    std::fs::write(d.dir.join("fail"), "3").unwrap();
    d.minute(2);
    wait(|| d.runs() == 2, "the failure's fire");
    let id = d.list()[1]["id"].clone();
    let args = d.stub(&id, "args");
    assert!(args.contains("has just failed (exit status 3)"), "{args}");
    let err = d.dir.join("rituals/watch/probe.err");
    assert!(args.contains(&format!("stderr is in `{}`", err.display())), "{args}");
    assert_eq!(std::fs::read_to_string(&err).unwrap(), "boom\n");
    // What it printed when it worked stays for the run to read.
    let out = d.dir.join("rituals/watch/probe.out");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "line-one\n");
    d.minute(3);
    d.minute(4);
    assert_eq!(d.runs(), 2, "failing the same way fired again");
    std::fs::remove_file(d.dir.join("fail")).unwrap();
    d.minute(5);
    wait(|| d.runs() == 3, "the recovery's fire");

    // Past its limit: a failure too, and its whole group goes, the child it started included.
    std::fs::write(d.dir.join("slow"), "").unwrap();
    d.clock(T0 + 60 * 6 + 5);
    wait(|| d.dir.join("kid").exists(), "the slow probe's child");
    wait(|| d.runs() == 4, "the timeout's fire");
    let id = d.list()[3]["id"].clone();
    assert!(d.stub(&id, "args").contains("still running after 6s, and was stopped"));
    let err = std::fs::read_to_string(d.dir.join("rituals/watch/probe.err")).unwrap();
    assert_eq!(err, "gensokyo: it was still running after 6s, and was stopped\n");
    let kid: i32 = std::fs::read_to_string(d.dir.join("kid")).unwrap().trim().parse().unwrap();
    wait(|| !alive(kid), "the probe's child killed with it");
    // Timing out again: once the second slow probe has been stopped, nothing more.
    std::fs::remove_file(d.dir.join("kid")).unwrap();
    d.minute(7);
    wait(|| d.dir.join("kid").exists(), "the second slow probe's child");
    let kid: i32 = std::fs::read_to_string(d.dir.join("kid")).unwrap().trim().parse().unwrap();
    wait(|| !alive(kid), "the second slow probe stopped");
    d.settle();
    assert_eq!(d.runs(), 4, "timing out again fired again");
}

#[test]
fn a_probe_must_come_from_a_probes_dir() {
    let base = "---\nschedule: \"* * * * *\"\ncwd: \"@cwd\"\n";
    let rituals = [
        ("abs", format!("{base}when: /bin/echo hi\n---\nGo.\n")),
        ("up", format!("{base}when: ../feed\n---\nGo.\n")),
        ("nope", format!("{base}when: nothere\n---\nGo.\n")),
        ("flat", format!("{base}when: flat\n---\nGo.\n")),
        ("loud", format!("{base}when: feed\nheadless: true\nquiet: true\n---\nGo.\n")),
        ("kept", format!("{base}target: persistent\nquiet: true\n---\nGo.\n")),
        ("out", format!("{base}when: out -c true\n---\nGo.\n")),
        ("inner", format!("{base}when: inner\n---\nGo.\n")),
    ];
    let rituals: Vec<(&str, &str)> = rituals.iter().map(|(a, b)| (*a, b.as_str())).collect();
    let d = Daemon::new("probe-dirs", T0 + 5, &rituals, &[]);
    // The dir a link, as a dotfiles checkout makes it: what is in it still counts as in it.
    std::fs::create_dir_all(d.dir.join("mine")).unwrap();
    std::os::unix::fs::symlink(d.dir.join("mine"), d.dir.join("conf/probes")).unwrap();
    std::os::unix::fs::symlink("/bin/sh", d.dir.join("conf/probes/out")).unwrap();
    std::os::unix::fs::symlink("feed", d.dir.join("conf/probes/inner")).unwrap();
    std::fs::write(d.dir.join("conf/probes/flat"), "#!/bin/sh\necho\n").unwrap();
    std::fs::write(d.dir.join("conf/probes/feed"), "#!/bin/sh\necho\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    let x = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(d.dir.join("conf/probes/feed"), x).unwrap();
    d.cli(&["list"]);
    let r = d.req(json!({"t": "rituals", "id": 1}));
    let problem = |n: &str| {
        let rs = r["rituals"].as_array().unwrap();
        let p = &rs.iter().find(|x| x["name"] == n).unwrap()["problem"];
        p.as_str().unwrap_or_default().to_string()
    };
    assert!(problem("abs").starts_with("when: /bin/echo is not a probe's name"), "{r}");
    assert!(problem("up").starts_with("when: ../feed is not a probe's name"), "{r}");
    assert!(problem("nope").starts_with("when: there is no probe called nothere in"), "{r}");
    assert!(problem("flat").ends_with("flat is not executable (chmod +x it)"), "{r}");
    assert!(problem("loud").starts_with("quiet: true is about the fresh resident"), "{r}");
    assert!(problem("kept").ends_with("and persistent is not one"), "{r}");
    assert!(problem("out").contains("out leads out of"), "{r}");
    assert_eq!(problem("inner"), "", "{r}");
}

#[test]
fn a_quiet_runs_finished_turn_neither_rings_nor_turns_gold_but_its_dialogs_ring() {
    let rituals = [("hush", hourly("quiet: true\n")), ("loud", hourly(""))];
    let rituals: Vec<(&str, &str)> = rituals.iter().map(|(a, b)| (*a, b.as_str())).collect();
    let d = Daemon::start("quiet", T0 + 120, &rituals, &[]);
    wait(|| d.stamp("loud").is_some(), "first sight");
    let (mut w, mut events) = d.connect();
    writeln!(w, "{}", json!({"t": "watch"})).unwrap();
    d.clock(T0 + 3600 + 5);
    let n = watch_for(&mut events, |v| v["t"] == "notify");
    assert_eq!(n["name"], "loud", "{n}");
    let state = |n: &str| d.list().into_iter().find(|r| r["name"] == n).unwrap()["state"].clone();
    wait(|| d.log().iter().any(|l| l["ev"] == "quiet" && l["ritual"] == "hush"), "hush's turn");
    assert_eq!((state("hush"), state("loud")), (json!("resting"), json!("awaits")));
    d.input("hush", "/perm\r");
    let n = watch_for(&mut events, |v| v["t"] == "notify");
    assert_eq!((&n["name"], &n["state"]), (&json!("hush"), &json!("awaits")));
}

#[test]
fn a_probe_that_says_too_much_fails_at_once_and_a_chatty_one_still_works() {
    let base = "---\nschedule: \"* * * * *\"\ncwd: \"@cwd\"\n";
    let rituals = [
        ("big", format!("{base}when: big\n---\nGo.\n")),
        ("chatty", format!("{base}when: chatty\n---\nGo.\n")),
    ];
    let rituals: Vec<(&str, &str)> = rituals.iter().map(|(a, b)| (*a, b.as_str())).collect();
    // A probe blocked on a full pipe would only be stopped at this limit, past every wait here.
    let d = Daemon::new("probe-caps", T0 + 5, &rituals, &[("GENSOKYO_PROBE_LIMIT_MS", "60000")]);
    let probes = d.dir.join("conf/probes");
    std::fs::create_dir_all(&probes).unwrap();
    use std::os::unix::fs::PermissionsExt;
    for (name, script) in [
        ("big", "#!/bin/sh\nhead -c 300000 /dev/zero | tr '\\0' x\n"),
        ("chatty", "#!/bin/sh\nhead -c 100000 /dev/zero | tr '\\0' x >&2\necho fine\n"),
    ] {
        std::fs::write(probes.join(name), script).unwrap();
        std::fs::set_permissions(probes.join(name), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    d.cli(&["list"]);
    wait(|| d.runs() == 2, "both fires");
    let args = |n: &str| {
        let r = d.list().into_iter().find(|r| r["name"] == n).unwrap();
        d.stub(&r["id"], "args")
    };
    assert!(args("big").contains("has just failed (it printed over 256 KB)"), "{}", args("big"));
    assert!(
        args("chatty").contains("has just run, and what it printed is in"),
        "{}",
        args("chatty")
    );
    let out = std::fs::read_to_string(d.dir.join("rituals/chatty/probe.out")).unwrap();
    assert_eq!(out, "fine\n");
    let err = std::fs::read(d.dir.join("rituals/chatty/probe.err")).unwrap();
    assert_eq!(err.len(), 64 * 1024, "kept up to its cap");
}

#[test]
fn a_probed_fire_into_a_resident_still_starting_is_sent_once_and_measures_the_next() {
    let d = Daemon::start("probe-named", T0 + 5, &[], &[]);
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Sakuya"}));
    let id = r["resident"]["id"].clone();
    d.stub(&id, "ready");
    // As Claude Code's registry lists a session it has not finished starting.
    let status = d.dir.join(format!("stub/{}.status", id.as_str().unwrap()));
    std::fs::write(&status, "null").unwrap();
    d.listed();
    let probes = d.dir.join("conf/probes");
    std::fs::create_dir_all(&probes).unwrap();
    std::fs::write(probes.join("feed"), "#!/bin/sh\necho >> calls\ncat feed\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(probes.join("feed"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(d.dir.join("feed"), "line-one\n").unwrap();
    let text = "---\nschedule: \"* * * * *\"\ncwd: \"@cwd\"\nwhen: feed\ntarget: Sakuya\n\
        deliver: now\n---\nKeep the page current.\n";
    // Seen in a minute of its schedule, so it fires at once: the first time always does.
    d.ritual("watch", text);
    d.first_probe();

    // Two more minutes of the change while the prompt waits for Sakuya: not probed, so not sent
    // again behind it.
    for minute in [1, 2] {
        d.clock(T0 + 60 * minute + 5);
        wait(|| d.stamp("watch") == Some(T0 + 60 * minute), "the minute's stamp");
    }
    d.settle();
    assert_eq!(d.calls(), 1, "probed while its fire was on its way");
    std::fs::write(&status, "idle").unwrap();
    wait(|| d.evs("watch", "sent").len() == 1, "the prompt typed in");
    d.minute(3);
    let typed = || d.stub(&id, "input").matches("Keep the page current.").count();
    assert_eq!((typed(), d.evs("watch", "sent").len()), (1, 1), "sent twice");

    // What reached Sakuya is what the next probe is measured against.
    d.minute(4);
    assert_eq!(typed(), 1, "the same output sent again");
    std::fs::write(d.dir.join("feed"), "line-two\n").unwrap();
    d.minute(5);
    wait(|| typed() == 2, "the change sent");
}

#[test]
fn a_fire_waits_for_its_residents_turn_to_end_and_the_newest_one_is_typed() {
    let d = Daemon::start("idle", T0 + 120, &[("ask", &hourly("target: Sakuya\n"))], &[]);
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Sakuya"}));
    let id = r["resident"]["id"].clone();
    d.stub(&id, "ready");
    d.listed();
    wait(|| d.stamp("ask").is_some(), "first sight");
    d.busy(&id, true);
    let typed = || d.stub(&id, "input").matches("Do the rounds.").count();

    // Mid-turn, both fires wait; the second takes the first's place.
    d.clock(T0 + 3600 + 5);
    wait(|| d.evs("ask", "held").len() == 1, "the first fire held");
    d.clock(T0 + 7200 + 5);
    let replaced = || d.log().iter().any(|l| l["slug"] == "ask" && !l["replaced"].is_null());
    wait(replaced, "the second fire in the first's place");
    assert_eq!(d.evs("ask", "held").len(), 1, "held is said once");
    d.settle();
    assert_eq!(typed(), 0, "typed mid-turn");

    // The turn ends: one prompt, the newest fire's, once the hold has passed.
    d.busy(&id, false);
    wait(|| d.evs("ask", "sent").len() == 1, "the prompt typed in");
    d.settle();
    assert_eq!(typed(), 1);
    assert_eq!(d.evs("ask", "sent"), ["sent to Sakuya (due 2026-09-21 17:00)"]);
    assert!(d.evs("ask", "not-sent").is_empty());
}

#[test]
fn a_fire_waits_no_longer_than_its_limit_and_deliver_now_does_not_wait() {
    let ask = hourly("target: Sakuya\n").replace("0 * * * *", "0 16 * * *");
    let rituals = [("ask", ask.as_str()), ("now", &hourly("target: Sakuya\ndeliver: now\n"))];
    let d = Daemon::start("idle-most", T0 + 120, &rituals, &[]);
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Sakuya"}));
    d.stub(&r["resident"]["id"], "ready");
    d.listed();
    wait(|| d.stamp("ask").is_some() && d.stamp("now").is_some(), "first sight");
    d.input("Sakuya", "/busy 60\r");
    wait(|| d.live()[0]["state"] == "busy", "the turn");
    // `now` types into the turn, which Claude Code queues; `ask` waits.
    d.clock(T0 + 3600 + 5);
    wait(|| d.evs("now", "sent").len() == 1, "deliver: now sent mid-turn");
    wait(|| d.evs("ask", "held").len() == 1, "the daily fire held");
    assert!(d.evs("ask", "held")[0].contains("which is busy"), "{:?}", d.evs("ask", "held"));
    // Four hours on, by the ritual clock, it gives up.
    d.clock(T0 + 3600 + 4 * 3600 + 10);
    wait(|| d.evs("ask", "not-sent").len() == 1, "the held fire dropped");
    assert_eq!(d.evs("ask", "not-sent"), ["not sent: Sakuya was not idle for 4h: it is busy"]);
    assert!(d.evs("ask", "sent").is_empty());
}

#[test]
fn fires_that_take_each_others_place_are_given_up_four_hours_from_the_first() {
    let d = Daemon::start("idle-chain", T0 + 120, &[("ask", &hourly("target: Sakuya\n"))], &[]);
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Sakuya"}));
    let id = r["resident"]["id"].clone();
    d.stub(&id, "ready");
    d.listed();
    wait(|| d.stamp("ask").is_some(), "first sight");
    d.busy(&id, true);
    d.clock(T0 + 3600 + 5);
    wait(|| d.evs("ask", "held").len() == 1, "held");
    // The next hour's fire takes its place, and the hour after the fourth gives up on both.
    d.clock(T0 + 2 * 3600 + 5);
    wait(|| d.log().iter().any(|l| l["replaced"].is_string()), "replaced");
    d.clock(T0 + 3600 + 4 * 3600 + 10);
    wait(|| d.evs("ask", "not-sent").len() == 1, "the held fires dropped");
    assert_eq!(d.evs("ask", "not-sent"), ["not sent: Sakuya was not idle for 4h: it is busy"]);
    assert_eq!(d.evs("ask", "held").len(), 1, "said once for the run of them");
}

#[test]
fn a_fire_into_a_resident_the_registry_never_lists_is_given_up_after_a_while() {
    let other = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/agents-2.1.260.json");
    let env = [("STUB_REGISTRY", other), ("GENSOKYO_READY_WAIT_MS", "2000")];
    let d = Daemon::start("unlisted", T0 + 120, &[("ask", &hourly("target: Sakuya\n"))], &env);
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Sakuya"}));
    d.stub(&r["resident"]["id"], "ready");
    d.listed();
    wait(|| d.stamp("ask").is_some(), "first sight");
    d.clock(T0 + 3600 + 5);
    wait(|| d.evs("ask", "not-sent").len() == 1, "given up");
    assert_eq!(d.evs("ask", "not-sent"), ["not sent: Sakuya is still starting up"]);
}

#[test]
fn fires_held_behind_one_turn_go_in_one_at_a_time() {
    let body = |b: &str| {
        format!(
            "---\nschedule: \"0 * * * *\"\ncwd: \"@cwd\"\ntarget: Sakuya\n---\n{b}\nIts second line.\n"
        )
    };
    let (one, two, three) = (body("Ritual one."), body("Ritual two."), body("Ritual three."));
    let rituals = [("one", one.as_str()), ("two", &two), ("three", &three)];
    let d = Daemon::start("idle-two", T0 + 120, &rituals, &[]);
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Sakuya"}));
    let id = r["resident"]["id"].clone();
    d.stub(&id, "ready");
    d.listed();
    let slugs = ["one", "two", "three"];
    wait(|| slugs.iter().all(|s| d.stamp(s).is_some()), "first sight");
    d.busy(&id, true);
    d.clock(T0 + 3600 + 5);
    wait(|| slugs.iter().all(|s| d.evs(s, "held").len() == 1), "all held");
    d.busy(&id, false);
    wait(|| slugs.iter().all(|s| d.evs(s, "sent").len() == 1), "all sent");
    // Each pasted whole and sent with its own Enter, never two in one prompt. Sent is said once
    // the Enter is written: the stub may not have read it yet.
    let whole = |s: &str| format!("\x1b[200~Ritual {s}.\nIts second line.\x1b[201~\n");
    let typed = || d.stub(&id, "input");
    wait(|| slugs.iter().all(|s| typed().contains(&whole(s))), "each typed whole, alone");
    assert_eq!(typed().matches("\x1b[200~").count(), 3, "{:?}", typed());
}

#[test]
fn a_held_probe_fire_types_the_newest_once_and_a_fire_by_hand_still_sends() {
    let d = Daemon::start("idle-probe", T0 + 5, &[], &[]);
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Sakuya"}));
    let id = r["resident"]["id"].clone();
    d.stub(&id, "ready");
    d.listed();
    let probes = d.dir.join("conf/probes");
    std::fs::create_dir_all(&probes).unwrap();
    std::fs::write(probes.join("feed"), "#!/bin/sh\necho >> calls\ncat feed\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(probes.join("feed"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(d.dir.join("feed"), "line-one\n").unwrap();
    d.busy(&id, true);
    let text = "---\nschedule: \"* * * * *\"\ncwd: \"@cwd\"\nwhen: feed\ntarget: Sakuya\n---\n\
        Keep the page current.\n";
    d.ritual("watch", text);
    d.first_probe();
    // The output changes while the fire is held: the probe runs on, and the newer one replaces it.
    std::fs::write(d.dir.join("feed"), "line-two\n").unwrap();
    d.minute(1);
    d.minute(2);
    let typed = || d.stub(&id, "input").matches("Keep the page current.").count();
    assert_eq!(typed(), 0, "typed mid-turn");
    // Held long enough to be journaled, once for all three.
    wait(|| d.evs("watch", "held").len() == 1, "held said");
    d.busy(&id, false);
    wait(|| d.evs("watch", "sent").len() == 1, "the prompt typed in");
    d.settle();
    assert_eq!(typed(), 1, "sent twice");
    assert_eq!(d.evs("watch", "held").len(), 1, "{:?}", d.evs("watch", "held"));
    let fired = std::fs::read_to_string(d.dir.join("rituals/watch/probe.fired")).unwrap();
    assert_eq!(fired, "ok\nline-two\n", "the newest output is what was sent");
    // By hand, with nothing new, it is sent all the same.
    d.minute(3);
    assert_eq!(typed(), 1);
    assert_eq!(d.verb("run", "watch")["t"], "done");
    wait(|| typed() == 2, "the fire by hand typed in");
}

#[test]
fn a_branch_ritual_tells_the_resident_on_each_branch_only_what_is_new() {
    let d = Daemon::start("branch", T0 + 5, &[], &[]);
    // A repo on main whose origin is acme/app, and a worktree of it on nebel95/fix.
    let (repo, wt) = (d.dir.join("app"), d.dir.join("app/.claude/worktrees/fix"));
    std::fs::create_dir_all(repo.join(".git/worktrees/fix")).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let config =
        "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:acme/app.git\n";
    std::fs::write(repo.join(".git/config"), config).unwrap();
    let fix = repo.join(".git/worktrees/fix");
    std::fs::write(fix.join("HEAD"), "ref: refs/heads/nebel95/fix\n").unwrap();
    std::fs::write(fix.join("commondir"), "../..\n").unwrap();
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", fix.display())).unwrap();

    let r = d.req(json!({"t": "summon", "id": 7, "cwd": repo, "name": "Sakuya"}));
    let id = r["resident"]["id"].clone();
    d.stub(&id, "ready");
    d.listed();
    let probes = d.dir.join("conf/probes");
    std::fs::create_dir_all(&probes).unwrap();
    std::fs::write(probes.join("feed"), "#!/bin/sh\necho >> calls\ncat feed\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(probes.join("feed"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let feed = |v: Value| std::fs::write(d.dir.join("feed"), v.to_string()).unwrap();
    feed(json!({"acme/app:main": {"ci": "FAILURE@abc1234", "_pr": 7}}));
    let text = "---\nschedule: \"* * * * *\"\ncwd: \"@cwd\"\nwhen: feed\ntarget: branch\n---\n\
        News on {branch} (PR {_pr}{_none}{_abcdefghijklmnopqrstuvwx}){_ see}{_none}: {facts}\n";
    d.ritual("watch", text);
    let typed = || d.stub(&id, "input").matches("News on").count();
    wait(|| typed() == 1, "the first news");
    // A `{_name}` not given is left empty, and a `{_` that is not one is kept.
    assert!(d.stub(&id, "input").contains("News on main (PR 7){_ see}: ci: FAILURE@abc1234"));

    // The same again is no news; the fact going away sends nothing; its return does.
    d.minute(1);
    feed(json!({"acme/app:main": {"_pr": 7}}));
    d.minute(2);
    assert_eq!(typed(), 1);
    feed(json!({"acme/app:main": {"ci": "FAILURE@abc1234", "_pr": 7}}));
    d.minute(3);
    wait(|| typed() == 2, "the failure back");

    // Sakuya moves into the worktree: main's news has nobody, fix's reaches it. A key and a
    // fact that are not tokens are left out, each said once.
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let hook = json!({"event": "Stop", "at": at.as_millis() as i64, "cwd": wt});
    let (mut w, mut lines) = d.connect();
    let list = json!({"t": "list", "id": 2});
    writeln!(w, "{}\n{list}", json!({"t": "hook", "resident": id, "hook": hook})).unwrap();
    assert_eq!(next(&mut lines)["residents"][0]["branch"], "nebel95/fix");
    feed(json!({
        "acme/app:main": {"ci": "FAILURE@def5678"},
        "acme/app:nebel95/fix": {"review": "CHANGES_REQUESTED", "Title": "Fix it", "note": "two words"},
        "not a key": {"ci": "x"},
    }));
    d.minute(4);
    wait(|| typed() == 3, "the worktree's news");
    let input = d.stub(&id, "input");
    assert!(
        input.contains("News on nebel95/fix (PR ){_ see}: review: CHANGES_REQUESTED\x1b[201~"),
        "{input}"
    );
    assert!(!input.contains("def5678") && !input.contains("two words"), "{input}");
    assert_eq!(d.evs("watch", "left-out").len(), 3, "{:?}", d.evs("watch", "left-out"));

    // Held mid-turn, then the news clears before the turn ends: nothing is typed.
    d.busy(&id, true);
    feed(json!({"acme/app:nebel95/fix": {"review": "CHANGES_REQUESTED", "conflicts": true}}));
    d.minute(5);
    wait(|| d.evs("watch", "held").len() == 1, "held");
    feed(json!({"acme/app:nebel95/fix": {"review": "CHANGES_REQUESTED"}}));
    d.minute(6);
    d.busy(&id, false);
    d.settle();
    d.settle();
    assert_eq!(typed(), 3, "cleared news typed");

    // Held, and Sakuya leaves the branch before it is free: dropped, unrecorded. Back on it, the
    // next fire brings the news.
    let go = |dir: &std::path::Path| {
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
        let hook = json!({"event": "Stop", "at": at.as_millis() as i64, "cwd": dir});
        let (mut w, mut lines) = d.connect();
        let list = json!({"t": "list", "id": 2});
        writeln!(w, "{}\n{list}", json!({"t": "hook", "resident": id, "hook": hook})).unwrap();
        next(&mut lines)["residents"][0]["branch"].clone()
    };
    d.busy(&id, true);
    feed(json!({"acme/app:nebel95/fix": {"review": "CHANGES_REQUESTED", "conflicts": true}}));
    d.minute(7);
    wait(|| d.evs("watch", "held").len() == 2, "held again");
    assert_eq!(go(&repo), "main");
    d.settle();
    d.settle();
    assert_eq!(typed(), 3, "typed after it left the branch");
    assert_eq!(go(&wt), "nebel95/fix");
    d.minute(8);
    wait(|| typed() == 4, "the news once back");
    assert!(d.stub(&id, "input").contains("conflicts: true"));

    // Held, and the branch leaves the output (its PR merged): nothing is typed, or recorded.
    d.busy(&id, true);
    let fix = json!({"review": "CHANGES_REQUESTED", "conflicts": true, "ci": "FAILURE@9a9a9a9"});
    feed(json!({"acme/app:nebel95/fix": fix}));
    d.minute(9);
    wait(|| d.evs("watch", "held").len() == 3, "held a third time");
    feed(json!({}));
    d.minute(10);
    d.busy(&id, false);
    d.settle();
    d.settle();
    assert_eq!(typed(), 4, "a gone branch's news typed");
    let sent = std::fs::read_to_string(d.dir.join("rituals/watch/branches.json")).unwrap();
    assert!(!sent.contains("nebel95/fix"), "{sent}");

    // By hand, nothing new: sent all the same.
    feed(json!({"acme/app:nebel95/fix": {"review": "CHANGES_REQUESTED"}}));
    assert_eq!(d.verb("run", "watch")["t"], "done");
    wait(|| typed() == 5, "the fire by hand");

    // The repo's case is not the origin's: the same repo all the same.
    feed(json!({"Acme/App:nebel95/fix": {"ci": "FAILURE@0f0f0f0"}}));
    d.minute(11);
    wait(|| typed() == 6, "news under the repo in another case");
    assert!(d.stub(&id, "input").contains("ci: FAILURE@0f0f0f0"));
}
