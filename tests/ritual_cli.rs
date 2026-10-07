//! `gensokyo ritual …` without a daemon: the verbs that read and write the files.

mod common;

use common::{err, fresh, out};
use gensokyo::ritual::Dir;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Env {
    root: PathBuf,
}

impl Env {
    /// A config, state, share and home of its own; `trusted` is the one directory Claude Code
    /// has been trusted in.
    fn new(name: &str) -> Env {
        let root = fresh(&format!("rc-{name}"));
        for d in ["config", "state", "home", "claude", "share/rituals", "trusted", "untrusted"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("share");
        std::fs::copy(src.join("names.txt"), root.join("share/names.txt")).unwrap();
        for f in std::fs::read_dir(src.join("rituals")).unwrap().flatten() {
            std::fs::copy(f.path(), root.join("share/rituals").join(f.file_name())).unwrap();
        }
        let e = Env { root: std::fs::canonicalize(&root).unwrap() };
        let trusted = e.path("trusted");
        let claude = serde_json::json!({"projects": {
            trusted.to_string_lossy(): {"hasTrustDialogAccepted": true}}});
        std::fs::write(e.path("claude/.claude.json"), claude.to_string()).unwrap();
        e
    }

    fn path(&self, p: &str) -> PathBuf {
        self.root.join(p)
    }

    fn run_in(&self, cwd: &Path, args: &[&str], stdin: &str) -> Output {
        self.command(cwd, env!("CARGO_BIN_EXE_gensokyo"), args, stdin)
    }

    /// With a terminal for stdin (an editor wants one), through script(1); its output is the
    /// terminal's, stdout and stderr together.
    fn run_tty(&self, args: &[&str]) -> Output {
        let mut a = vec!["-q", "/dev/null", env!("CARGO_BIN_EXE_gensokyo")];
        a.extend(args);
        self.command(&self.path("trusted"), "/usr/bin/script", &a, "")
    }

    fn command(&self, cwd: &Path, program: &str, args: &[&str], stdin: &str) -> Output {
        self.command_env(cwd, program, args, stdin, &[])
    }

    fn command_env(
        &self,
        cwd: &Path,
        program: &str,
        args: &[&str],
        stdin: &str,
        env: &[(&str, &str)],
    ) -> Output {
        let mut c = Command::new(program);
        c.args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.path("home"))
            .env("TZ", "America/New_York")
            .env("GENSOKYO_CONFIG_DIR", self.path("config"))
            .env("GENSOKYO_STATE_DIR", self.path("state"))
            .env("GENSOKYO_SHARE", self.path("share"))
            .env("GENSOKYO_SOCKET", self.path("state/no.sock"))
            .env("CLAUDE_CONFIG_DIR", self.path("claude"))
            .env("EDITOR", "true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(env.iter().copied());
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_in(&self.path("trusted"), args, "")
    }

    /// From inside a resident, as its Bash tool runs it.
    fn inside(&self, args: &[&str]) -> Output {
        let (exe, cwd) = (env!("CARGO_BIN_EXE_gensokyo"), self.path("trusted"));
        self.command_env(&cwd, exe, args, "", &[("GENSOKYO_RESIDENT", "r-1")])
    }

    fn mine(&self, name: &str) -> PathBuf {
        self.path(&format!("config/rituals/{name}.md"))
    }

    fn list(&self) -> Vec<Value> {
        let o = self.run(&["ritual", "list", "--json"]);
        assert!(o.status.success(), "{}", err(&o));
        serde_json::from_slice(&o.stdout).unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn add_writes_a_ritual_and_refuses_what_would_not_fire() {
    let e = Env::new("add");
    let add = |extra: &[&str]| {
        let mut a = vec!["ritual", "add", "--name", "tea", "--prompt", "Make tea."];
        a.extend(extra);
        e.run(&a)
    };
    let o = add(&["--schedule", "5 9 * * 1-5", "--allowed-tools", "Read,Grep", "--model", "haiku"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("wrote ") && out(&o).contains("next fire"), "{}", out(&o));
    assert!(out(&o).contains("never in UTC"), "{}", out(&o));
    let text = std::fs::read_to_string(e.mine("tea")).unwrap();
    assert!(text.contains("schedule: \"5 9 * * 1-5\""), "{text}");
    assert!(text.contains("  - \"Read\"\n  - \"Grep\""), "{text}");
    assert!(text.contains(&format!("cwd: \"{}\"", e.path("trusted").display())), "{text}");
    assert!(text.ends_with("---\nMake tea.\n"), "{text}");

    let o = add(&["--schedule", "@daily"]);
    assert!(!o.status.success() && err(&o).contains("already there"), "{}", err(&o));

    std::fs::remove_file(e.mine("tea")).unwrap();
    let o = add(&["--schedule", "5 9 * *"]);
    assert!(!o.status.success() && err(&o).contains("schedule: 5 9 * *"), "{}", err(&o));
    let o = add(&["--schedule", "0 0 30 2 *"]);
    assert!(!o.status.success() && err(&o).contains("never comes round"), "{}", err(&o));
    // A missing directory is refused, and so is anything behind it that would also be wrong.
    let o = add(&["--schedule", "@daily", "--cwd", "/nope"]);
    assert!(!o.status.success() && err(&o).contains("not a directory"), "{}", err(&o));
    let untrusted = e.path("untrusted");
    let o =
        add(&["--schedule", "@daily", "--cwd", untrusted.to_str().unwrap(), "--overlap", "twice"]);
    assert!(!o.status.success() && err(&o).contains("overlap: twice"), "{}", err(&o));
    let o = add(&["--schedule", "@daily", "--headless", "--keep", "1h"]);
    assert!(!o.status.success() && err(&o).contains("--keep is only"), "{}", err(&o));
    let o = add(&["--schedule", "@daily", "--target", "Sakuya", "--keep", "1h"]);
    assert!(!o.status.success() && err(&o).contains("--keep is only"), "{}", err(&o));
    // An empty variable before a flag: the flag is not the value.
    let o = add(&["--schedule", "@daily", "--description", "--disabled"]);
    assert!(
        !o.status.success() && err(&o).contains("value is required for '--description"),
        "{}",
        err(&o)
    );
    let o = e.run(&[
        "ritual",
        "add",
        "--name",
        "tea",
        "--schedule",
        "@daily",
        "--prompt",
        "--disabled",
    ]);
    assert!(!o.status.success() && err(&o).contains("is the next option"), "{}", err(&o));
    assert!(!e.mine("tea").exists(), "a refused ritual leaves no file");
}

#[test]
fn add_warns_about_an_untrusted_directory_and_reads_the_prompt_from_stdin() {
    let e = Env::new("untrusted");
    let o = e.run_in(
        &e.path("untrusted"),
        &["ritual", "add", "--name", "outside", "--schedule", "every 30m", "--prompt-file", "-"],
        "Line one.\nLine two.\n",
    );
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("gensokyo: ritual add: cwd: nothing has answered"), "{}", err(&o));
    assert!(out(&o).contains("not firing until that is fixed"), "{}", out(&o));
    assert!(!out(&o).contains("next fire"), "{}", out(&o));
    let text = std::fs::read_to_string(e.mine("outside")).unwrap();
    assert!(text.ends_with("---\nLine one.\nLine two.\n"), "{text}");
    let r = e.list().into_iter().find(|r| r["name"] == "outside").unwrap();
    assert!(r["problem"].as_str().unwrap().starts_with("cwd: "), "{r}");
    assert_eq!(r["next_fire"], Value::Null, "a ritual with a problem has no next fire");
}

#[test]
fn list_json_carries_what_the_skill_reads() {
    let e = Env::new("list");
    let o = e.run(&[
        "ritual",
        "add",
        "--name",
        "nightly",
        "--schedule",
        "0 2 * * *",
        "--prompt",
        "Check.",
        "--description",
        "the night's checks",
        "--keep",
        "30m",
    ]);
    assert!(o.status.success(), "{}", err(&o));
    let all = e.list();
    let names: Vec<&str> = all.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["inbox-zero", "nightly", "nightly-checks", "slack-morning"]);
    let r = &all[1];
    for k in [
        "name",
        "enabled",
        "schedule",
        "next_fire",
        "last_run",
        "target",
        "headless",
        "keep",
        "overlap",
        "cwd",
        "problem",
        "description",
        "path",
    ] {
        assert!(r.get(k).is_some(), "{k} missing from {r}");
    }
    assert_eq!(r["enabled"], true);
    assert_eq!(r["target"], "new");
    assert_eq!(r["keep"], "30m");
    assert_eq!(r["overlap"], "skip");
    assert_eq!(r["headless"], false);
    assert_eq!(r["problem"], Value::Null);
    assert!(r["next_fire"].as_i64().is_some(), "{r}");
    assert!(r["next_fire_local"].as_str().unwrap().ends_with(" 02:00"), "{r}");
    assert_eq!(all[0]["enabled"], false, "the shipped examples arrive paused");
    assert_eq!(all[0]["shipped"], true);

    let text = out(&e.run(&["ritual"]));
    assert!(text.contains("nightly") && text.contains("paused"), "{text}");
    assert!(text.contains("nothing fires until it is"), "{text}");
}

#[test]
fn enabling_a_shipped_example_makes_it_the_users_own() {
    let e = Env::new("toggle");
    let o = e.run(&["ritual", "enable", "slack"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("the shipped slack-morning.md is now yours"), "{}", out(&o));
    let mine = std::fs::read_to_string(e.mine("slack-morning")).unwrap();
    assert!(mine.contains("enabled: true") && !mine.contains("enabled: false"), "{mine}");
    let shipped = std::fs::read_to_string(e.path("share/rituals/slack-morning.md")).unwrap();
    assert!(shipped.contains("enabled: false"), "the shipped file is left alone");
    let o = e.run(&["ritual", "disable", "slack-morning"]);
    assert!(o.status.success() && !out(&o).contains("now yours"), "{}", out(&o));
    let mine = std::fs::read_to_string(e.mine("slack-morning")).unwrap();
    assert!(mine.contains("enabled: false"), "{mine}");
    let r = e.list().into_iter().find(|r| r["name"] == "slack-morning").unwrap();
    assert_eq!(r["shipped"], false);
    // Back on, it is stamped now: the fires it missed while off are not caught up.
    let d = Dir::at(&e.path("state"), "slack-morning");
    d.set_stamp(1_000_020).unwrap();
    let o = e.run(&["ritual", "enable", "slack-morning"]);
    assert!(o.status.success(), "{}", err(&o));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let stamp = d.stamp().unwrap();
    assert!(stamp % 60 == 0 && (now.as_secs() as i64 - stamp) < 120, "{stamp}");
}

#[test]
fn a_residents_ritual_arrives_paused_and_only_the_user_resumes_it() {
    let e = Env::new("resident");
    let add = |extra: &[&str]| {
        let mut a = vec!["ritual", "add", "--name", "tea", "--prompt", "Tea.", "--schedule"];
        a.extend(["@daily", "--mode", "bypassPermissions", "--allowed-tools", "Bash"]);
        a.extend(extra);
        e.inside(&a)
    };
    std::fs::write(e.path("home/mcp.json"), "{}").unwrap();
    let o = add(&["--mcp-config", e.path("home/mcp.json").to_str().unwrap()]);
    assert!(!o.status.success() && err(&o).contains("is not in"), "{}", err(&o));
    std::fs::write(e.path("config/mcp.json"), "{}").unwrap();
    let o = add(&["--mcp-config", e.path("config/mcp.json").to_str().unwrap()]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("paused: the user resumes it"), "{}", out(&o));
    let text = std::fs::read_to_string(e.mine("tea")).unwrap();
    assert!(text.contains("enabled: false"), "{text}");

    let o = e.inside(&["ritual", "enable", "tea"]);
    assert!(!o.status.success() && err(&o).contains("the user resumes a ritual"), "{}", err(&o));
    let o = e.inside(&["ritual", "edit", "tea"]);
    assert!(!o.status.success() && err(&o).contains("file is the user's"), "{}", err(&o));
    assert!(e.inside(&["ritual", "disable", "tea"]).status.success());

    // The user sees what it may do where they turn it on.
    let o = e.run(&["ritual", "enable", "tea"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("  mode       bypassPermissions\n  tools      Bash\n"), "{}", out(&o));
    assert!(out(&o).contains("  mcp        "), "{}", out(&o));
    let r = e.list().into_iter().find(|r| r["name"] == "tea").unwrap();
    assert_eq!(
        (&r["mode"], &r["allowed_tools"]),
        (&"bypassPermissions".into(), &serde_json::json!(["Bash"]))
    );
}

#[test]
fn log_prints_the_journal() {
    let e = Env::new("log");
    let o = e.run(&["ritual", "log", "nightly-checks"]);
    assert!(o.status.success() && out(&o).contains("has not done anything yet"), "{}", out(&o));
    let d = Dir::at(&e.path("state"), "nightly-checks");
    // 2026-09-28 09:05 in New York.
    d.note(1_790_600_700, "ran", "ran (due 2026-09-28 09:05)");
    d.note(1_790_600_760, "skipped", "skipped (due 2026-09-28 09:06): the last run is still going");
    let o = e.run(&["ritual", "log", "nightly-checks"]);
    let text = out(&o);
    assert!(text.contains("  2026-09-28 09:05  ran (due 2026-09-28 09:05)"), "{text}");
    assert!(text.contains("journal.jsonl"), "{text}");
    let o = e.run(&["ritual", "log", "nightly-checks", "-n", "1"]);
    assert!(!out(&o).contains("ran (due") && out(&o).contains("skipped"), "{}", out(&o));
}

#[test]
fn remove_refuses_a_shipped_example_and_deletes_the_users_own() {
    let e = Env::new("remove");
    let o = e.run(&["ritual", "remove", "inbox-zero"]);
    assert!(!o.status.success() && err(&o).contains("ritual disable inbox-zero"), "{}", err(&o));
    assert!(e.path("share/rituals/inbox-zero.md").exists());
    e.run(&["ritual", "add", "--name", "mine", "--schedule", "@hourly", "--prompt", "Go."]);
    Dir::at(&e.path("state"), "mine").note(1, "ran", "ran (by hand)");
    let o = e.run(&["ritual", "remove", "min"]);
    assert!(!o.status.success() && err(&o).contains("did you mean mine?"), "{}", err(&o));
    let o = e.run(&["ritual", "remove", "mine"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("its notes and its journal went too"), "{}", out(&o));
    assert!(!e.mine("mine").exists() && !e.path("state/rituals/mine").exists());
}

#[test]
fn new_and_edit_write_the_file_the_editor_opens() {
    let e = Env::new("new");
    // Without a terminal (a Claude Code session's Bash tool) there is no editor to wait on.
    let o = e.run(&["ritual", "new", "morning"]);
    assert!(!o.status.success() && err(&o).contains("opens an editor"), "{}", err(&o));
    assert!(!e.mine("morning").exists());
    let o = e.run_tty(&["ritual", "new", "morning"]);
    assert!(o.status.success(), "{}", out(&o));
    let text = std::fs::read_to_string(e.mine("morning")).unwrap();
    assert!(text.contains("name: morning") && text.contains("enabled: false"), "{text}");
    assert!(out(&o).contains("disabled, so it will not fire"), "{}", out(&o));
    let o = e.run_tty(&["ritual", "new", "morning"]);
    assert!(!o.status.success() && out(&o).contains("already there"), "{}", out(&o));
    let o = e.run_tty(&["ritual", "new", "inbox-zero"]);
    assert!(out(&o).contains("yours shadows it"), "{}", out(&o));
    std::fs::remove_file(e.mine("inbox-zero")).unwrap();
    let o = e.run_tty(&["ritual", "edit", "inbox-zero"]);
    assert!(o.status.success() && out(&o).contains("now yours"), "{}", out(&o));
    assert!(e.mine("inbox-zero").exists());
}
