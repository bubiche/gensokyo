//! Rituals fired by the daemon, with tests/stub-claude as every resident and the ritual clock
//! moved by hand (`GENSOKYO_NOW_FILE`, ticking every 100 ms), so nothing waits for a minute.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_gensokyo");

/// On the hour, UTC: `0 * * * *` fires at T0, T0 + 3600, …
const T0: i64 = 1_790_002_800;

struct Daemon {
    dir: PathBuf,
    env: Vec<(String, String)>,
}

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
        let dir =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("f-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("conf/rituals")).unwrap();
        let mut all = vec![
            ("GENSOKYO_STATE_DIR".to_string(), dir.display().to_string()),
            (
                "GENSOKYO_CLAUDE".into(),
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stub-claude").into(),
            ),
            ("STUB_STATE".into(), dir.join("stub").display().to_string()),
            ("STUB_HOOKS".into(), "1".into()),
            ("CLAUDE_CODE_CHILD_SESSION".into(), "1".into()),
            ("CLAUDE_CONFIG_DIR".into(), dir.join("claude").display().to_string()),
            ("GENSOKYO_CONFIG_DIR".into(), dir.join("conf").display().to_string()),
            ("GENSOKYO_NOW_FILE".into(), dir.join("now").display().to_string()),
            ("GENSOKYO_TICK_MS".into(), "100".into()),
            ("TZ".into(), "UTC".into()),
        ];
        all.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let d = Daemon { dir, env: all };
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

    fn cli(&self, args: &[&str]) -> Output {
        let mut c = Command::new(BIN);
        c.args(args).envs(self.env.iter().map(|(k, v)| (k, v))).env_remove("GENSOKYO_SOCKET");
        let out = c.stdin(Stdio::null()).output().unwrap();
        assert!(
            out.status.success(),
            "gensokyo {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("run/gensokyo.sock")
    }

    fn connect(&self) -> (UnixStream, std::io::Lines<BufReader<UnixStream>>) {
        let s = UnixStream::connect(self.socket()).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut w = s.try_clone().unwrap();
        writeln!(w, "{}", json!({"t": "hello", "proto": 4, "who": "test"})).unwrap();
        let mut lines = BufReader::new(s).lines();
        let welcome: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        assert_eq!(welcome["t"], "welcome");
        (w, lines)
    }

    fn req(&self, req: Value) -> Value {
        let (mut w, mut lines) = self.connect();
        writeln!(w, "{req}").unwrap();
        serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
    }

    fn verb(&self, verb: &str, name: &str) -> Value {
        self.req(json!({"t": "ritual", "id": 3, "verb": verb, "name": name}))
    }

    fn list(&self) -> Vec<Value> {
        self.req(json!({"t": "list", "id": 1}))["residents"].as_array().unwrap().clone()
    }

    fn live(&self) -> Vec<Value> {
        self.list().into_iter().filter(|r| r["departed"].is_null()).collect()
    }

    fn input(&self, who: &str, text: &str) {
        // Answered only when it fails, so a list follows it: its reply is the first line then.
        let (mut w, mut lines) = self.connect();
        let input = json!({"t": "input", "id": 2, "who": who, "bytes": text.as_bytes()});
        writeln!(w, "{input}\n{}", json!({"t": "list", "id": 1})).unwrap();
        let r: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
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

    fn stub(&self, id: &Value, ext: &str) -> String {
        let file = |e: &str| self.dir.join("stub").join(format!("{}.{e}", id.as_str().unwrap()));
        wait(|| file("ready").exists(), "the stub to be ready");
        std::fs::read_to_string(file(ext)).unwrap_or_default()
    }

    /// A few ticks' worth, for asserting that nothing more happens.
    fn settle(&self) {
        std::thread::sleep(Duration::from_millis(600));
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let mut c = Command::new(BIN);
        c.arg("quit").envs(self.env.iter().map(|(k, v)| (k, v))).env_remove("GENSOKYO_SOCKET");
        let _ = c.output();
    }
}

fn wait(f: impl FnMut() -> bool, what: &str) {
    wait_for(Duration::from_secs(15), f, what)
}

fn wait_for(most: Duration, mut f: impl FnMut() -> bool, what: &str) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < most, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn hourly(extra: &str) -> String {
    format!(
        "---\nschedule: \"0 * * * *\"\ncwd: \"@cwd\"\n{extra}---\nDo the rounds.\nThen report.\n"
    )
}

/// Events from a `watch` connection until `f` picks one, within 15 s.
fn watch_for(
    lines: &mut std::io::Lines<BufReader<UnixStream>>,
    mut f: impl FnMut(&Value) -> bool,
) -> Value {
    let t = Instant::now();
    loop {
        assert!(t.elapsed() < Duration::from_secs(15), "no such event");
        let v: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        if f(&v) {
            return v;
        }
    }
}

#[test]
fn a_due_fire_starts_one_run_with_its_flags_and_its_notes() {
    let text = hourly("allowed_tools:\n  - Read\n  - Bash(ls:*)\nmodel: haiku\n");
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
    let tools =
        format!("--add-dir {} --allowedTools Read Bash(ls:*) --session-id", notes.display());
    assert!(args.contains(&tools), "{args}");
    assert!(args.contains("--model haiku"), "{args}");
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
    // Finished only once the registry lists it (a poll every 3 s): not stuck at a trust dialog.
    std::thread::sleep(Duration::from_millis(3500));
    // Finished at +5; typed into at +1000, and finished again at once.
    d.clock(T0 + 3600 + 1000);
    d.settle();
    d.input("rounds", "one more thing\r");
    wait(|| d.stub(&id, "input").contains("one more thing"), "the typing");
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
}

fn headless_run(name: &str, env: &[(&str, &str)]) -> (Daemon, String) {
    let d = Daemon::start(name, T0 + 120, &[("quiet", &hourly("headless: true\n"))], env);
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
fn a_headless_run_logs_what_it_said_and_what_it_was_refused() {
    let (d, log) = headless_run("headless", &[("STUB_P_DENY", "Bash")]);
    assert!(log.contains("stub -p ran in"), "{log}");
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
    // Listed by the registry (a poll every 3 s), so it is not at a trust dialog.
    std::thread::sleep(Duration::from_millis(3500));
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
    let d = Daemon::start("dialog", T0 + 120, &[("ask", &hourly("target: Sakuya\n"))], &[]);
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
