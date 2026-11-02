//! A Codex resident's launch on its own: which codex runs, its argv and the TOML in it, the
//! modes both ways, gensokyo's paragraph, and Codex's trust read from its config.

mod common;

use common::fresh;
use gensokyo::daemon::codex::{self, Launched};
use gensokyo::daemon::launch::{self, Options, Paths};
use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn opts<'a>() -> Options<'a> {
    Options {
        id: "id",
        session: "id",
        name: "Nitori",
        model: None,
        effort: None,
        mode: None,
        prompt: None,
        resume: false,
        extra: &[],
        lead: None,
        role: None,
        ledger: None,
        system_prompt: None,
        teleport: None,
        remote: true,
    }
}

fn argv(exe: &str, o: &Options) -> Vec<String> {
    let p = Paths { exe: Path::new(exe), share: Path::new("/share"), socket: Path::new("/s") };
    let a = codex::argv(&p, o).unwrap();
    a.into_iter().map(|a| a.into_string().unwrap()).collect()
}

/// The `-c` value whose key is `key`.
fn value<'a>(a: &'a [String], key: &str) -> Option<&'a str> {
    let at = |w: &[String]| w[0] == "-c" && w[1].starts_with(&format!("{key}="));
    a.windows(2).find(|w| at(w)).map(|w| &w[1][key.len() + 1..])
}

fn script(dir: &Path, name: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let f = dir.join(name);
    std::fs::write(&f, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&f, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    f
}

#[test]
fn codex_is_found_where_it_was_set_then_on_path_then_in_the_app() {
    let dir = fresh("codex-find");
    let (own, real) = (dir.join("own"), dir.join("real"));
    script(&own, "codex");
    let theirs = script(&real, "codex");
    let app = script(&dir.join("app"), "codex");
    let path = std::env::join_paths([&own, &real]).unwrap();
    let set = Some(OsString::from("/set/codex"));
    assert_eq!(codex::find(set, Some(&path), &own, &app), Some("/set/codex".into()));
    assert_eq!(codex::find(None, Some(&path), &own, &app), Some(theirs));
    // gensokyo's own directory is never where it is found.
    let own_only = std::env::join_paths([&own]).unwrap();
    assert_eq!(codex::find(None, Some(&own_only), &own, &app), Some(app.clone()));
    assert_eq!(codex::find(None, None, &own, &app), Some(app));
    // An app that is not there, or does not run, is no codex.
    assert_eq!(codex::find(None, None, &own, &dir.join("none")), None);
    let plain = dir.join("plain");
    std::fs::write(&plain, "").unwrap();
    assert_eq!(codex::find(None, None, &own, &plain), None);
}

#[test]
fn a_toml_string_says_exactly_what_it_was_given() {
    let cases = [
        ("plain", r#""plain""#),
        ("a \"quote\" and a \\", r#""a \"quote\" and a \\""#),
        ("two\nlines\tand a tab", r#""two\nlines\tand a tab""#),
        ("bell\u{7} cr\r esc\u{1b} del\u{7f}", r#""bell\u0007 cr\u000D esc\u001B del\u007F""#),
        ("ünïcödé ✓ 'single'", r#""ünïcödé ✓ 'single'""#),
    ];
    for (s, quoted) in cases {
        assert_eq!(codex::toml_str(s), quoted);
        assert_eq!(codex::toml_unquote(quoted), Some((s.to_string(), "")), "{s:?}");
    }
    assert_eq!(codex::toml_unquote("'lit\\eral' # c"), Some(("lit\\eral".into(), " # c")));
    assert_eq!(codex::toml_unquote("\"open"), None);
    assert_eq!(codex::toml_unquote("bare"), None);
}

#[test]
fn every_hook_runs_gensokyo_from_a_path_with_a_space_and_a_quote() {
    let exe = "/Users/o'brien/my apps/gensokyo";
    let a = argv(exe, &opts());
    assert_eq!(a[0], "--dangerously-bypass-hook-trust");
    let hooks: Vec<&str> = a.windows(2).filter(|w| w[0] == "-c").map(|w| w[1].as_str()).collect();
    let hooks: Vec<&str> = hooks.into_iter().filter(|v| v.starts_with("hooks.")).collect();
    assert_eq!(hooks.len(), 7);
    let want = format!("{} _hook", launch::sh_quote(exe));
    for (v, e) in hooks.iter().zip(codex::HOOKS) {
        let head = format!("hooks.{e}=[{{hooks=[{{type=\"command\",command=");
        let cmd = v.strip_prefix(&head).unwrap_or_else(|| panic!("{v}"));
        let (cmd, rest) = codex::toml_unquote(cmd).unwrap();
        assert_eq!((cmd.as_str(), rest), (want.as_str(), "}]}]"));
    }
    assert_eq!(
        codex::HOOKS,
        [
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "Stop",
            "PermissionRequest",
            "PostToolUse",
            "Interrupt"
        ]
    );
    // The shell gets the path back whole.
    let printf = format!("printf %s {}", launch::sh_quote(exe));
    let out = Command::new("sh").arg("-c").arg(printf).output().unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), exe);
}

#[test]
fn a_codex_argv_says_who_it_is_then_the_mode_model_effort_and_prompt_last() {
    let o = Options {
        mode: Some("acceptEdits"),
        model: Some("gpt-5.6"),
        effort: Some("high"),
        prompt: Some("-starts with a dash\nand goes on"),
        lead: Some("Seiran"),
        role: Some("Review \"carefully\".\n\tTabbed."),
        system_prompt: Some("Be brief \\ terse."),
        ..opts()
    };
    let a = argv("/g", &o);
    let words = codex::toml_unquote(value(&a, "developer_instructions").unwrap()).unwrap().0;
    let para = codex::paragraph("Nitori", Some("Seiran"), None, true);
    assert_eq!(words, format!("{para}\n\nReview \"carefully\".\n\tTabbed.\n\nBe brief \\ terse."));
    assert_eq!(
        value(&a, "tui.terminal_title"),
        Some(r#"["run-state","thread-id","context-used"]"#)
    );
    let n = a.len();
    assert_eq!(
        a[n - 10..],
        [
            "-s",
            "workspace-write",
            "-a",
            "on-request",
            "-m",
            "gpt-5.6",
            "-c",
            "model_reasoning_effort=high",
            "--",
            "-starts with a dash\nand goes on"
        ]
    );
    // Nothing of Claude's, and never Codex's notify.
    for not in ["--settings", "--plugin-dir", "--remote-control", "--session-id", "--allowedTools"]
    {
        assert!(!a.iter().any(|w| w == not), "{not}");
    }
    assert!(!a.iter().any(|w| w.starts_with("notify")));
    // Read back as launched.
    let back = codex::launched(&a);
    let want = Launched {
        model: Some("gpt-5.6".into()),
        effort: Some("high".into()),
        mode: Some("acceptEdits".into()),
        profile: None,
    };
    assert_eq!(back, want);

    // Unset, each is Codex's own: no flag for it, and no prompt.
    let a = argv("/g", &opts());
    assert_eq!(codex::launched(&a), Launched::default());
    assert!(!a.iter().any(|w| ["--", "-m", "-s", "-a"].contains(&w.as_str())));
    assert!(value(&a, "model_reasoning_effort").is_none());
}

/// A resume is the same start, its flags first, then `resume <thread>` in place of a prompt.
#[test]
fn a_resumed_codex_argv_has_its_flags_then_resume_and_its_thread_and_no_prompt() {
    let o = Options {
        mode: Some("default"),
        model: Some("gpt-5.6"),
        effort: Some("high"),
        lead: Some("Seiran"),
        role: Some("Review."),
        prompt: Some("brief"),
        ..opts()
    };
    let first = argv("/g", &o);
    let back = argv("/g", &Options { session: "019a-thread", resume: true, ..o });
    let (n, m) = (first.len(), back.len());
    assert_eq!(first[n - 2..], ["--", "brief"]);
    assert_eq!(back[..m - 2], first[..n - 2]);
    assert_eq!(back[m - 2..], ["resume", "019a-thread"]);
    assert!(!back.iter().any(|w| w == "--" || w == "brief"), "{back:?}");
    assert_eq!(codex::launched(&back), codex::launched(&first));
}

/// A thread is resumed only when Codex kept it: its rollout, by name, three levels down.
#[test]
fn a_codex_thread_is_found_by_its_rollouts_name_in_its_days_directory_only() {
    let home = fresh("cx-rollout");
    let t = "01a19cc5-82ba-7ba3-b403-76fbc00bb608";
    let name = |t: &str| format!("rollout-2026-11-02T22-59-13-{t}.jsonl");
    assert!(!codex::has_thread(&home, t), "no sessions at all");
    let day = home.join("sessions/2026/11/02");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(day.join(name(t)), "").unwrap();
    assert!(codex::has_thread(&home, t));
    // A resident's own id, before Codex named its thread; part of the thread's id; none.
    for not in ["5f0c1e9a-41d2-4c3b-9a8e-7d6c5b4a3f21", "76fbc00bb608", "b403-76fbc00bb608", ""] {
        assert!(!codex::has_thread(&home, not), "{not:?}");
    }
    // One level too deep, one too shallow, and a directory by a rollout's name: none of them.
    let deep = "0b1c2d3e-0000-4000-8000-000000000001";
    std::fs::create_dir_all(day.join("x")).unwrap();
    std::fs::write(day.join("x").join(name(deep)), "").unwrap();
    let shallow = "0b1c2d3e-0000-4000-8000-000000000002";
    std::fs::write(home.join("sessions/2026/11").join(name(shallow)), "").unwrap();
    let dir = "0b1c2d3e-0000-4000-8000-000000000003";
    std::fs::create_dir_all(day.join(name(dir))).unwrap();
    for not in [deep, shallow, dir] {
        assert!(!codex::has_thread(&home, not), "{not}");
    }
    // Only a hex id with dashes, so none is ever a flag after `resume`, whatever is planted.
    for flag in ["--foo", "--ab", "-a", "x-1", "ab cd", "ab/cd"] {
        std::fs::write(day.join(name(flag)), "").unwrap_or_default();
        assert!(!codex::has_thread(&home, flag), "{flag}");
    }
}

#[test]
fn each_mode_has_codex_flags_that_read_back_as_it() {
    let p = Paths { exe: Path::new("/g"), share: Path::new("/share"), socket: Path::new("/s") };
    for (mode, flags, back) in [
        ("plan", &["-s", "read-only", "-a", "on-request"][..], "default"),
        ("default", &["-s", "read-only", "-a", "on-request"], "default"),
        ("acceptEdits", &["-s", "workspace-write", "-a", "on-request"], "acceptEdits"),
        ("auto", &["--approve-for-me"], "auto"),
        ("bypassPermissions", &["--dangerously-bypass-approvals-and-sandbox"], "bypassPermissions"),
    ] {
        assert_eq!(codex::mode_flags(mode), Some(flags));
        let a = argv("/g", &Options { mode: Some(mode), prompt: Some("go"), ..opts() });
        assert!(a.windows(flags.len()).any(|w| w == flags), "{mode}");
        assert_eq!(codex::mode_of(&a), Some(back), "{mode}");
    }
    // Flags in the prompt are the prompt's.
    let a: Vec<String> = ["--", "--approve-for-me"].map(String::from).into();
    assert_eq!(codex::mode_of(&a), None);
    // A mode Codex has no flags for, or an effort that is not one word, starts nothing.
    let e = codex::argv(&p, &Options { mode: Some("dontAsk"), ..opts() }).unwrap_err();
    assert!(e.contains("Codex has no dontAsk mode"), "{e}");
    let e = codex::argv(&p, &Options { effort: Some("high x=1"), ..opts() }).unwrap_err();
    assert!(e.contains("effort high x=1"), "{e}");
}

#[test]
fn a_codex_resident_hears_its_name_and_a_helper_how_to_report_and_ask() {
    let plain = codex::paragraph("Nitori", None, None, false);
    assert!(plain.contains("inside gensokyo") && plain.contains("resident name is Nitori"));
    assert!(!plain.contains("helper"));
    let helper = codex::paragraph("Nitori", Some("Seiran"), None, false);
    for says in [
        "the resident Seiran summoned you",
        "your last message is your report to Seiran",
        "end your turn with the question",
    ] {
        assert!(helper.contains(says), "{says}");
    }
    let ledger = codex::paragraph("Nitori", None, Some(Path::new("/l/proj.md")), false);
    assert!(ledger.contains("ledger is `/l/proj.md`"));
    // Nothing a Codex resident has no use for, in Codex's sandbox or out of it.
    let boxed = codex::paragraph("Nitori", None, None, true);
    let boxed_helper = codex::paragraph("Nitori", Some("Seiran"), None, true);
    for p in [plain, helper, ledger, boxed, boxed_helper] {
        for not in ["SendMessage", "skill", "Agent tool", "Edit tool", "claude agents"] {
            assert!(!p.contains(not), "{not}: {p}");
        }
    }
}

/// Codex's sandbox keeps git's files read-only: in every mode but bypassPermissions, and in
/// none, a resident hears that it cannot commit, and a helper to say in its report what it
/// changed.
#[test]
fn a_sandboxed_codex_resident_hears_it_cannot_commit() {
    let words = |mode, lead| {
        let a = argv("/g", &Options { mode, lead, ..opts() });
        codex::toml_unquote(value(&a, "developer_instructions").unwrap()).unwrap().0
    };
    let line = "Your sandbox keeps git's own files read-only, so `git add` and `git commit` fail \
                in it: leave your changes uncommitted";
    for mode in codex::MODES.iter().map(|(m, _)| Some(*m)).chain([None]) {
        let (plain, helper) = (words(mode, None), words(mode, Some("Seiran")));
        if mode == Some("bypassPermissions") {
            for w in [plain, helper] {
                assert!(!w.contains("git commit"), "{w}");
            }
            continue;
        }
        assert!(plain.contains(&format!(" {line}.")), "{mode:?}: {plain}");
        assert!(!plain.contains("report"), "{mode:?}: {plain}");
        let report = format!(" {line}, and say in your report what you changed.");
        assert!(helper.contains(&report), "{mode:?}: {helper}");
    }
}

#[test]
fn a_resident_gets_none_of_the_codex_session_it_was_started_from() {
    let base = [
        ("CODEX_THREAD_ID", "t"),
        ("CODEX_SANDBOX", "seatbelt"),
        ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
        ("CODEX_HOME", "/h/.codex"),
        ("CODEX_API_KEY", "not-a-key"),
        ("CODEX_CA_CERTIFICATE", "/h/ca.pem"),
        ("CODEXY", "kept"),
        ("HOME", "/h"),
    ];
    let base = base.map(|(k, v)| (OsString::from(k), OsString::from(v)));
    let env = launch::env(base, "id", Path::new("/g"), Path::new("/shim"), Path::new("/s"));
    let has = |k: &str| env.iter().any(|(n, _)| n == k);
    for gone in ["CODEX_THREAD_ID", "CODEX_SANDBOX", "CODEX_SANDBOX_NETWORK_DISABLED"] {
        assert!(!has(gone), "{gone}");
    }
    // The user's own Codex setup goes through.
    for kept in ["CODEX_HOME", "CODEX_API_KEY", "CODEX_CA_CERTIFICATE", "CODEXY", "HOME"] {
        assert!(has(kept), "{kept}");
    }
}

#[test]
fn trust_is_read_from_projects_tables_only() {
    let dir = fresh("codex-trust-scan");
    let (a, b, c, d) = (dir.join("a"), dir.join("b c"), dir.join("q\"d"), dir.join("lit"));
    for p in [&a, &b, &c, &d] {
        std::fs::create_dir_all(p).unwrap();
    }
    let config = format!(
        r#"model = "gpt-5.6"
notify = ["x"]
developer_instructions = """
[projects."{none}"]
trust_level = "trusted"
"""

[projects."{a}"]  # mine
trust_level = "trusted"

[projects."{b}"]
trust_level="trusted" # spaces
[projects."{bq}".sub]
trust_level = "trusted"

[projects.'{d}']
other = 1
trust_level = 'trusted'

[projects."{c_esc}"]
trust_level = "untrusted"

[elsewhere]
trust_level = "trusted"
"#,
        none = dir.join("none").display(),
        a = a.display(),
        b = b.display(),
        bq = dir.display(),
        d = d.display(),
        c_esc = c.display().to_string().replace('"', "\\\""),
    );
    let got = codex::projects(&config);
    assert_eq!(got, [a.clone(), b.clone(), d.clone()]);
    let real = |p: &Path| std::fs::canonicalize(p).unwrap();
    assert!(codex::trusted_in(&config, &real(&a), None));
    assert!(!codex::trusted_in(&config, &real(&c), None));
    // Never a directory above it.
    let under = a.join("sub");
    std::fs::create_dir_all(&under).unwrap();
    assert!(!codex::trusted_in(&config, &real(&under), None));
    // Unless it is the main checkout the directory is in.
    assert!(codex::trusted_in(&config, &real(&under), Some(&real(&a))));
    assert!(!codex::trusted_in(&config, &real(&c), Some(&real(&a))));
}

#[test]
fn without_a_profile_the_model_is_read_from_the_configs_top_level() {
    assert_eq!(codex::config_model("model = \"gpt-5.5\"\n", None).as_deref(), Some("gpt-5.5"));
    let c = "# mine\nmodel_provider = \"x\"\nmodel='o4' # set by hand\n[tui]\nmodel = \"no\"\n";
    assert_eq!(codex::config_model(c, None).as_deref(), Some("o4"));
    // In a table, or a multi-line string, it is not the one Codex runs.
    assert_eq!(codex::config_model("[profiles.a]\nmodel = \"gpt-5.5\"\n", None), None);
    assert_eq!(codex::config_model("x = \"\"\"\nmodel = \"no\"\n\"\"\"\n", None), None);
    assert_eq!(codex::config_model("model_reasoning_effort = \"high\"\n", None), None);
    assert_eq!(codex::config_model("model = \"gpt-5.5\" trailing\n", None), None);
    // `-m` first, then the config in `home`, read only.
    let home = fresh("cx-model");
    let h = Some(home.as_path());
    let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(codex::model(&argv(&["-m", "gpt-x", "--", "hi"]), h).as_deref(), Some("gpt-x"));
    assert_eq!(codex::model(&argv(&["--", "hi"]), h), None);
    std::fs::write(home.join("config.toml"), "model = \"gpt-5.4\"\n").unwrap();
    assert_eq!(codex::model(&argv(&["--", "-m"]), h).as_deref(), Some("gpt-5.4"));
    assert_eq!(codex::model(&argv(&["--", "hi"]), None), None);
}

#[test]
fn the_model_of_the_profile_launched_is_read_before_the_top_levels() {
    let config = r#"
model = "top"
profile = "quiet"

[profiles.quiet]
"model" = "o4-mini" # quoted key
[profiles."deep"]
model = 'gpt-5.5'
[profiles.deep.tools]
model = "no"
[profiles.empty]
approval_policy = "never"
"#;
    let cm = |p| codex::config_model(config, p);
    // The profile named at the top, else the one launched.
    assert_eq!(cm(None).as_deref(), Some("o4-mini"));
    assert_eq!(cm(Some("deep")).as_deref(), Some("gpt-5.5"));
    // One with no model, or none of that name: the top level's.
    assert_eq!(cm(Some("empty")).as_deref(), Some("top"));
    assert_eq!(cm(Some("missing")).as_deref(), Some("top"));
    assert_eq!(
        codex::config_model("[profiles.a]\nmodel = \"x\"\n", Some("a")).as_deref(),
        Some("x")
    );
    // A dotted key is not read.
    assert_eq!(codex::config_model("profiles.a.model = \"x\"\n", Some("a")), None);
    // `-p`, `--profile` and `--profile=` pick it, before the config's; `-m` before them all.
    let home = fresh("cx-profile");
    std::fs::write(home.join("config.toml"), config).unwrap();
    let h = Some(home.as_path());
    let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    for a in [&["-p", "deep"][..], &["--profile", "deep"], &["--profile=deep"]] {
        assert_eq!(codex::launched(&argv(a)).profile.as_deref(), Some("deep"), "{a:?}");
        assert_eq!(codex::model(&argv(a), h).as_deref(), Some("gpt-5.5"), "{a:?}");
    }
    assert_eq!(codex::model(&argv(&["--", "-p", "deep"]), h).as_deref(), Some("o4-mini"));
    assert_eq!(codex::model(&argv(&["-p", "deep", "-m", "mine"]), h).as_deref(), Some("mine"));
    assert_eq!(std::fs::read_to_string(home.join("config.toml")).unwrap(), config);
}

#[test]
fn a_codex_model_is_made_fit_to_show_as_claudes_is() {
    let home = fresh("cx-clean");
    let h = Some(home.as_path());
    let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // Its first line, control characters blanked.
    let m = codex::model(&argv(&["-m", "gpt\x1b[31m-5.5\nmore", "--", "hi"]), h);
    assert_eq!(m.as_deref(), Some("gpt [31m-5.5"));
    // At most 40.
    let m = codex::model(&argv(&["-m", &"x".repeat(60)]), h).unwrap();
    assert_eq!(m, "x".repeat(40));
    // Nothing to show is none.
    assert_eq!(codex::model(&argv(&["-m", " \n\t"]), h), None);
    // From the config too.
    std::fs::write(home.join("config.toml"), "model = \"o4\\u0007-mini\"\n").unwrap();
    assert_eq!(codex::model(&argv(&["--", "hi"]), h).as_deref(), Some("o4 -mini"));
}

#[test]
fn the_context_used_is_read_from_the_titles_last_part() {
    assert_eq!(codex::context("Ready | Context 0% used"), Some(0));
    assert_eq!(
        codex::context("Working ⠋ | 01a18db1-0831-77e0-b80c-4b71d... | Context 42% used"),
        Some(42)
    );
    assert_eq!(codex::context("Ready | 01a18db1-0831-77e0-b80c-4b71d..."), None);
    assert_eq!(codex::context(""), None);
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", dir.parent().unwrap())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A worktree inside a trusted repository starts with no prompt; one elsewhere, or a repository
/// trusted nowhere, needs its own entry.
#[test]
fn a_worktree_is_trusted_through_its_main_checkout() {
    let dir = std::fs::canonicalize(fresh("codex-trust-git")).unwrap();
    let (repo, other) = (dir.join("repo"), dir.join("other"));
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "x"]);
    let inside = repo.join(".claude/worktrees/w");
    git(&repo, &["worktree", "add", "-q", "-b", "w", inside.to_str().unwrap()]);
    git(&repo, &["worktree", "add", "-q", "-b", "o", other.to_str().unwrap()]);
    assert_eq!(codex::main_root(&inside), Some(repo.clone()));
    assert_eq!(codex::main_root(&other), Some(repo.clone()));
    assert_eq!(codex::main_root(&repo.join(".claude")), Some(repo.clone()));

    let home = dir.join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    assert!(!codex::trusted(&home, &inside), "no config is trusted nowhere");
    std::fs::write(
        home.join("config.toml"),
        format!("[projects.\"{}\"]\ntrust_level = \"trusted\"\n", repo.display()),
    )
    .unwrap();
    assert!(codex::trusted(&home, &repo));
    assert!(codex::trusted(&home, &inside));
    assert!(!codex::trusted(&home, &other), "outside the trusted checkout");
    std::fs::write(
        home.join("config.toml"),
        format!("[projects.\"{}\"]\ntrust_level = \"trusted\"\n", other.display()),
    )
    .unwrap();
    assert!(codex::trusted(&home, &other));
    assert!(!codex::trusted(&home, &inside));
}

/// A repository in `dir` named with a space, a quote and a backslash, and a linked worktree
/// inside it, as `new --worktree` makes one: both real paths.
fn repo_and_worktree(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let repo = dir.join("my \"odd\\ repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "x"]);
    let tree = repo.join(".claude/worktrees/w");
    git(&repo, &["worktree", "add", "-q", "-b", "w", tree.to_str().unwrap()]);
    (std::fs::canonicalize(repo).unwrap(), std::fs::canonicalize(tree).unwrap())
}

/// A `.git` file's gitdir and a gitdir's `commondir` may each be relative, to the one naming it.
#[test]
fn a_relative_gitdir_and_commondir_lead_to_the_main_checkout() {
    let dir = std::fs::canonicalize(fresh("codex-rel")).unwrap();
    let (common, tree) = (dir.join("main/.git"), dir.join("tree"));
    let git = common.join("worktrees/t");
    std::fs::create_dir_all(&git).unwrap();
    std::fs::create_dir_all(&tree).unwrap();
    std::fs::write(tree.join(".git"), "gitdir: ../main/.git/worktrees/t\n").unwrap();
    std::fs::write(git.join("commondir"), "../..\n").unwrap();
    assert_eq!(codex::main_root(&tree), Some(dir.join("main")));
}

/// A daemon whose Codex residents are tests/stub-codex, with Codex trusting its directory or
/// not.
fn daemon(name: &str, trusted: bool) -> common::Daemon {
    daemon_with(name, trusted, &[])
}

/// The same, with `env` for the daemon and so every resident.
fn daemon_with(name: &str, trusted: bool, env: &[(&str, &str)]) -> common::Daemon {
    let dir = fresh(&format!("cx-{name}"));
    std::fs::create_dir_all(dir.join("codex")).unwrap();
    if trusted {
        let config = format!("[projects.\"{}\"]\ntrust_level = \"trusted\"\n", dir.display());
        std::fs::write(dir.join("codex/config.toml"), config).unwrap();
    }
    let env = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let d = common::Daemon::at(dir, env);
    d.cli(&["list"]);
    d
}

/// A Claude lead, in acceptEdits.
fn lead(d: &common::Daemon) -> Value {
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Seiran",
                         "mode": "acceptEdits"}));
    assert_eq!(r["t"], "summoned", "{r}");
    ready(d, &r["resident"]["id"]);
    r["resident"].clone()
}

/// Once resident `id`'s stand-in reads its keys.
fn ready(d: &common::Daemon, id: &Value) {
    let f = d.dir.join(format!("stub/{}.ready", id.as_str().unwrap()));
    common::wait_for(LONG, || f.exists(), "the stand-in to be ready");
}

/// `gensokyo <args>` from inside resident `me`.
fn inside(d: &common::Daemon, me: &Value, args: &[&str]) -> Output {
    let mut c = d.command(args);
    c.env("GENSOKYO_SOCKET", d.socket()).env("GENSOKYO_RESIDENT", me["id"].as_str().unwrap());
    c.current_dir(&d.dir).stdin(Stdio::null()).output().unwrap()
}

/// A Codex helper `lead` summons with `prompt`.
fn helper(d: &common::Daemon, lead: &Value, name: &str, prompt: &str) -> Value {
    let args = ["new", "--agent", "codex", "--json", "--name", name, "--prompt", prompt];
    let o = inside(d, lead, &args);
    assert!(o.status.success(), "{}", common::err(&o));
    serde_json::from_slice(&o.stdout).unwrap()
}

/// `wait` as `lead` runs it: its exit code and one line per helper.
fn waited(d: &common::Daemon, lead: &Value, args: &[&str]) -> (i32, Vec<Value>) {
    let o = inside(d, lead, &[&["wait"], args].concat());
    let lines = common::out(&o).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    (o.status.code().unwrap(), lines)
}

/// Its line in `list --all`.
fn me(d: &common::Daemon, name: &str) -> Value {
    let all = d.req(json!({"t": "list", "id": 1, "all": true}))["residents"].clone();
    all.as_array().unwrap().iter().find(|r| r["name"] == name).cloned().unwrap_or(Value::Null)
}

/// Keys typed into `who`, taken in before the reply to a list after them.
fn keys(d: &common::Daemon, who: &str, bytes: &[u8]) {
    let (mut w, mut lines) = d.connect();
    let input = json!({"t": "input", "id": 2, "who": who, "bytes": bytes});
    writeln!(w, "{input}\n{}", json!({"t": "list", "id": 1})).unwrap();
    assert_eq!(common::next(&mut lines)["t"], "list");
}

/// Long enough for "started" (up to two 3 s beats) and a loaded machine.
const LONG: Duration = Duration::from_secs(60);

/// The daemon reads a Codex title once a beat, every 3 s.
const BEAT: Duration = Duration::from_secs(3);

/// How long the stand-in keeps its last `Working` frame after a Stop or an Interrupt: longer than
/// a beat, so one reads it.
const TAIL: &str = "5";

/// `who` in `state` all through `long`, whatever titles Codex draws meanwhile.
fn stays(d: &common::Daemon, who: &str, state: &str, long: Duration, why: &str) {
    let t = Instant::now();
    while t.elapsed() < long {
        assert_eq!(me(d, who)["state"], state, "{why}");
    }
}

fn record(d: &common::Daemon, id: &Value) -> Value {
    let f = d.dir.join(format!("residents/{}.json", id.as_str().unwrap()));
    serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect()
}

#[test]
fn a_codex_resident_is_started_listed_left_and_resumed_on_its_thread() {
    let d = daemon("a", false);
    let dir = d.dir.display().to_string();
    let o = d.cli(&[
        "new",
        "--agent",
        "codex",
        "--json",
        "--name",
        "Nitori",
        "-p",
        "acceptEdits",
        "-m",
        "gpt-x",
        "-e",
        "high",
        "--prompt",
        "hi",
        &dir,
    ]);
    let r: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!((&r["agent"], &r["mode"]), (&json!("codex"), &json!("acceptEdits")), "{r}");
    let rec = record(&d, &r["id"]);
    assert_eq!(rec["agent"], "codex");
    let stub = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stub-codex");
    assert_eq!(rec["program"], stub);
    let argv = strings(&rec["argv"]);
    assert_eq!(argv[0], "--dangerously-bypass-hook-trust");
    assert_eq!(argv[argv.len() - 2..], ["--", "hi"]);
    let launched = codex::launched(&argv);
    assert_eq!(launched.model.as_deref(), Some("gpt-x"));
    assert_eq!(launched.effort.as_deref(), Some("high"));
    // The stand-in got exactly that.
    ready(&d, &r["id"]);
    let got = d.stub(&r["id"], "args");
    assert_eq!(got.split('\0').filter(|a| !a.is_empty()).collect::<Vec<_>>(), argv);

    let o = d.cli(&["list", "--json"]);
    let l: Vec<Value> = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(l.iter().find(|r| r["name"] == "Nitori").unwrap()["agent"], "codex");
    assert!(common::out(&d.cli(&["list"])).contains("codex"));

    // Renewed onto a claude it never ran on: no.
    let e = d.req(json!({"t": "renew", "id": 2, "who": ["Nitori"]}));
    assert!(e["error"].as_str().unwrap().contains("Nitori runs Codex"), "{e}");

    // Its turn over, a Ctrl-C at its empty prompt is enough: it has left, and exited 0.
    common::wait_for(LONG, || me(&d, "Nitori")["turns"] == 1, "its turn to end");
    let c = d.req(json!({"t": "close", "id": 3, "who": "Nitori"}));
    assert_eq!(c["message"], "Nitori has left (/exit)", "{c}");
    // Its SessionEnd, as it went, was not taken for a turn begun under the `/exit`.
    assert!(!d.log().iter().any(|l| l["ev"] == "exit" && l["held"].is_string()), "asked twice");
    let gone = me(&d, "Nitori");
    assert_eq!((&gone["exit"], &gone["agent"]), (&json!(0), &json!("codex")), "{gone}");

    // Back as it was summoned, on its own thread, with no prompt to say again.
    let thread = d.stub(&r["id"], "thread").trim().to_string();
    assert_eq!(record(&d, &r["id"])["session"], thread.as_str());
    std::fs::remove_file(d.dir.join(format!("stub/{}.ready", r["id"].as_str().unwrap()))).unwrap();
    let b = d.req(json!({"t": "recall", "id": 4, "who": "Nitori"}));
    assert_eq!(b["t"], "summoned", "{b}");
    let again = strings(&record(&d, &r["id"])["argv"]);
    assert_eq!(again[..again.len() - 2], argv[..argv.len() - 2]);
    assert_eq!(again[again.len() - 2..], ["resume", thread.as_str()]);
    assert_eq!(me(&d, "Nitori")["mode"], "acceptEdits");
    ready(&d, &r["id"]);
    let got = d.stub(&r["id"], "args");
    assert_eq!(got.split('\0').filter(|a| !a.is_empty()).collect::<Vec<_>>(), again);
    assert_eq!(d.stub(&r["id"], "thread").trim(), thread);
    assert_eq!(record(&d, &r["id"])["session"], thread.as_str());
    // Up as a fresh boot is, by its titles, with no prompt.
    common::wait_for(LONG, || me(&d, "Nitori")["blocked"].is_null(), "it to have started");
    assert_eq!(me(&d, "Nitori")["state"], "resting");
    let c = d.req(json!({"t": "close", "id": 5, "who": "Nitori"}));
    assert_eq!(c["message"], "Nitori has left (/exit)", "{c}");
}

#[test]
fn what_only_claude_code_has_is_refused_for_codex() {
    let d = daemon("b", true);
    let ask = |extra: Value| {
        let mut s = json!({"t": "summon", "id": 7, "cwd": d.dir, "agent": "codex"});
        s.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let e = d.req(s);
        assert_eq!(e["t"], "error", "{e}");
        e["error"].as_str().unwrap().to_string()
    };
    assert!(ask(json!({"allowed_tools": ["Read"]})).contains("Codex has no tools allowed"));
    assert!(ask(json!({"teleport": "session_01"})).contains("taken over by Claude Code"));
    assert!(ask(json!({"cloud": true, "prompt": "fix it"})).contains("a cloud session is Claude"));
    assert!(ask(json!({"mode": "dontAsk"})).contains("Codex has no dontAsk mode"));
    assert!(ask(json!({"effort": "x y"})).contains("Codex takes one word"));
    assert!(d.list().is_empty());
}

#[test]
fn a_codex_helper_works_in_its_leads_mode_where_codex_is_trusted() {
    let d = daemon("c", false);
    let lead = lead(&d);
    let ask = |extra: Value| {
        let mut s = json!({"t": "summon", "id": 8, "cwd": d.dir, "agent": "codex",
                           "name": "Kappa", "prompt": "brief"});
        s.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        d.req_as(lead["id"].as_str().unwrap(), s)
    };
    let e = ask(json!({}));
    assert!(e["error"].as_str().unwrap().contains("Codex does not trust"), "{e}");
    let config = format!("[projects.\"{}\"]\ntrust_level = \"trusted\"\n", d.dir.display());
    std::fs::write(d.dir.join("codex/config.toml"), config).unwrap();
    let e = ask(json!({"mode": "bypassPermissions"}));
    assert!(e["error"].as_str().unwrap().contains("not bypassPermissions"), "{e}");

    let r = ask(json!({}));
    assert_eq!(r["t"], "summoned", "{r}");
    let h = &r["resident"];
    assert_eq!((&h["agent"], &h["mode"]), (&json!("codex"), &json!("acceptEdits")), "{h}");
    let argv = strings(&record(&d, &h["id"])["argv"]);
    let words = value(&argv, "developer_instructions").unwrap();
    let words = codex::toml_unquote(words).unwrap().0;
    assert!(words.contains("the resident Seiran summoned you"), "{words}");
    assert!(argv.windows(2).any(|w| w == ["-s", "workspace-write"]), "{argv:?}");
}

/// A helper's turn, from its brief to its answer: busy by its hooks, waited on, read, closed.
#[test]
fn a_codex_helpers_turn_is_waited_on_read_and_it_leaves_on_close() {
    let d = daemon_with("turn", true, &[("STUB_TAIL", TAIL)]);
    let lead = lead(&d);
    let h = helper(&d, &lead, "Kappa", "hello there");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!(code, 0, "{l:?}");
    assert_eq!((&l[0]["ended"], &l[0]["answer"]), (&json!("stop"), &json!("echo: hello there")));
    // Busy from its prompt to its Stop, which leaves its answer to be read.
    let busy = |e: &str| {
        let log = d.log();
        let mine = log.iter().filter(|l| l["id"] == h["id"] && l["event"] == e);
        mine.map(|l| l["state"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>()
    };
    assert_eq!(busy("UserPromptSubmit"), ["busy"]);
    assert_eq!(busy("Stop"), ["awaits"]);
    // Codex draws Working a while longer after its Stop, the answer collected: no busier for it.
    stays(&d, "Kappa", "resting", BEAT + BEAT / 2, "a Working title after Stop made it busy");
    let o = inside(&d, &lead, &["read", "Kappa"]);
    assert!(common::out(&o).contains("echo: hello there"), "{}", common::out(&o));
    let o = inside(&d, &lead, &["close", "Kappa"]);
    assert!(o.status.success(), "{}", common::err(&o));
    let gone = me(&d, "Kappa");
    assert_eq!((&gone["state"], &gone["exit"]), (&json!("departed"), &json!(0)), "{gone}");
}

/// An approval holds the turn while Codex's spinner keeps turning; granted, the turn goes on to
/// its Stop.
#[test]
fn a_codex_approval_is_needed_until_it_is_granted() {
    let d = daemon("perm", true);
    let lead = lead(&d);
    helper(&d, &lead, "Kappa", "/perm");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "needs", "--timeout", "60s"]);
    assert_eq!(code, 0, "{l:?}");
    let k = me(&d, "Kappa");
    assert_eq!((&k["state"], &k["needs"]), (&json!("awaits"), &json!(1)), "{k}");
    // Codex's spinner draws a Working frame every tenth of a second meanwhile: through two beats
    // that read them, it is still waiting on the user.
    stays(&d, "Kappa", "awaits", 2 * BEAT + BEAT / 3, "the spinner cleared the approval");
    keys(&d, "Kappa", b"y");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!(code, 0, "{l:?}");
    assert_eq!((&l[0]["ended"], &l[0]["answer"]), (&json!("stop"), &json!("echo: /perm")));
    let k = me(&d, "Kappa");
    assert_eq!((&k["state"], &k["needs"]), (&json!("resting"), &json!(1)), "{k}");
}

#[test]
fn a_codex_approval_refused_ends_the_turn_interrupted() {
    let d = daemon_with("esc", true, &[("STUB_TAIL", TAIL)]);
    let lead = lead(&d);
    helper(&d, &lead, "Kappa", "/perm");
    let (code, _) = waited(&d, &lead, &["Kappa", "--until", "needs", "--timeout", "60s"]);
    assert_eq!(code, 0);
    keys(&d, "Kappa", b"\x1b");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!((code, &l[0]["ended"]), (0, &json!("interrupted")), "{l:?}");
    assert_eq!(l[0]["answer"], Value::Null);
    // Codex draws Working a while longer after the Interrupt: not working for it.
    let why = "a Working title after Interrupt made it busy";
    stays(&d, "Kappa", "resting", BEAT + BEAT / 2, why);
}

/// With no prompt there is no thread until one comes, but its title says it has started; and a
/// Ctrl-C at its empty prompt is all a close takes.
#[test]
fn a_promptless_codex_resident_starts_and_leaves_on_one_ctrl_c() {
    let d = daemon("bare", false);
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "agent": "codex",
                         "name": "Nitori"}));
    assert_eq!(r["t"], "summoned", "{r}");
    assert_eq!(me(&d, "Nitori")["blocked"], "is still starting up");
    common::wait_for(LONG, || me(&d, "Nitori")["blocked"].is_null(), "it to have started");
    assert_eq!(me(&d, "Nitori")["state"], "resting");
    let t = Instant::now();
    let c = d.req(json!({"t": "close", "id": 3, "who": "Nitori"}));
    assert_eq!(c["message"], "Nitori has left (/exit)", "{c}");
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert!(!d.log().iter().any(|l| l["ev"] == "exit" && l["held"].is_string()), "asked twice");
    assert_eq!(me(&d, "Nitori")["exit"], 0);

    // Never prompted, it has no thread to resume: back afresh, on its own id.
    let id = &r["resident"]["id"];
    std::fs::remove_file(d.dir.join(format!("stub/{}.ready", id.as_str().unwrap()))).unwrap();
    let b = d.req(json!({"t": "recall", "id": 4, "who": "Nitori"}));
    assert_eq!(b["t"], "summoned", "{b}");
    ready(&d, id);
    let argv = strings(&record(&d, id)["argv"]);
    assert!(!argv.iter().any(|a| a == "resume"), "{argv:?}");
    assert_eq!(record(&d, id)["session"], *id);
    assert!(!d.stub(id, "args").split('\0').any(|a| a == "resume"));
}

/// Codex has no status line: its model is the one it was launched with, or else its config's,
/// and its context comes from its title, for the sidebar, the box and `list` alike.
#[test]
fn a_codex_residents_model_and_context_are_listed() {
    let d = daemon("tele", true);
    let config = d.dir.join("codex/config.toml");
    let trust = std::fs::read_to_string(&config).unwrap();
    let written = format!("model = \"gpt-5.4-mini\"\n{trust}");
    std::fs::write(&config, &written).unwrap();
    let dir = d.dir.display().to_string();
    let new = |name: &str, model: &[&str]| {
        let args = [&["new", "--agent", "codex", "--json", "--name", name][..], model]
            .concat()
            .into_iter()
            .chain(["--prompt", "hi", dir.as_str()])
            .collect::<Vec<_>>();
        let o = d.cli(&args);
        assert!(o.status.success(), "{}", common::err(&o));
    };
    new("Nitori", &["-m", "gpt-5.5"]);
    new("Takane", &[]);
    let listed = |name: &str| {
        let o = d.cli(&["list", "--json"]);
        let l: Vec<Value> = serde_json::from_slice(&o.stdout).unwrap();
        l.into_iter().find(|r| r["name"] == name).map(|r| r["telemetry"].clone())
    };
    // Its first turn over, its title says 2% used.
    for (name, model) in [("Nitori", "gpt-5.5"), ("Takane", "gpt-5.4-mini")] {
        let want = json!({"model": model, "ctx": 2});
        let seen = |t: &Value| json!({"model": t["model"], "ctx": t["ctx"]}) == want;
        common::wait_for(LONG, || listed(name).is_some_and(|t| seen(&t)), name);
        let t = listed(name).unwrap();
        assert!(t["effort"].is_null() && t["cost"].is_null(), "{t}");
    }
    // The config was only read.
    assert_eq!(std::fs::read_to_string(&config).unwrap(), written);
    let line = common::out(&d.cli(&["list"]));
    assert!(line.contains("gpt-5.5 · ctx 2%"), "{line}");
}

/// A sandboxed Codex keeps git's files read-only, and gensokyo opens none of them: a resident in
/// a linked worktree is started exactly as one in its main checkout, in every mode and none.
#[test]
fn a_codex_resident_in_a_worktree_is_started_as_anywhere_else() {
    let d = daemon("wt", false);
    let (repo, tree) = repo_and_worktree(&std::fs::canonicalize(&d.dir).unwrap());
    let modes = codex::MODES.iter().map(|(m, _)| Some(*m)).chain([None]);
    for (i, mode) in modes.enumerate() {
        // Its argv, its name taken out of it.
        let argv = |name: &str, cwd: &Path| {
            let r = d.req(json!({"t": "summon", "id": 7, "cwd": cwd, "agent": "codex",
                                 "name": name, "mode": mode}));
            assert_eq!(r["t"], "summoned", "{r}");
            let a = strings(&record(&d, &r["resident"]["id"])["argv"]);
            a.into_iter().map(|w| w.replace(name, "<name>")).collect::<Vec<_>>()
        };
        let here = argv(&format!("Kappa{i}"), &tree);
        assert_eq!(here, argv(&format!("Kogasa{i}"), &repo), "{mode:?}");
        assert!(!here.iter().any(|w| w == "--add-dir" || w.contains("writable_roots")), "{here:?}");
    }
}

#[test]
fn a_queued_message_is_joined_to_its_flags_so_a_dash_cannot_make_it_one() {
    let a = codex::queue("019a-thread", "-m gpt, then the rest");
    let a: Vec<_> = a.iter().map(|a| a.to_str().unwrap()).collect();
    assert_eq!(a, ["queue", "--thread=019a-thread", "--message=-m gpt, then the rest"]);
}

/// `gensokyo say <name>` from inside resident `me`: its stdout, or its error.
fn say(d: &common::Daemon, me: &Value, who: &str, text: &str) -> Result<String, String> {
    let o = inside(d, me, &["say", who, text]);
    match o.status.success() {
        true => Ok(common::out(&o)),
        false => Err(common::err(&o)),
    }
}

/// A follow-up to a Codex helper is `codex queue` on its thread, the one its SessionStart named:
/// refused before there is one, after it has gone, and when it is not the lead's to send.
#[test]
fn a_lead_says_more_to_its_codex_helper_by_codex_queue_on_its_thread() {
    let d = daemon("say", true);
    let lead = lead(&d);
    let refused = |who: &str, text: &str, why: &str| {
        let e = say(&d, &lead, who, text).unwrap_err();
        assert!(e.contains(why), "{e}");
    };
    // No prompt yet, so no thread.
    let o = inside(&d, &lead, &["new", "--agent", "codex", "--json", "--name", "Nue"]);
    let nue: Value = serde_json::from_slice(&o.stdout).unwrap();
    ready(&d, &nue["id"]);
    refused("Nue", "hello", "Nue has not started yet");
    let args = d.dir.join("stub/queue.args");
    assert!(!args.exists());

    let h = helper(&d, &lead, "Kappa", "hello there");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!(code, 0, "{l:?}");
    let thread = d.stub(&h["id"], "thread").trim().to_string();
    assert_eq!(record(&d, &h["id"])["session"], thread.as_str());
    // A message that starts with a dash arrives whole, and is its next turn.
    let text = "-m gpt-5, and then: the rest";
    let said = say(&d, &lead, "Kappa", text).unwrap();
    assert!(said.contains("queued for Kappa"), "{said}");
    let got = std::fs::read_to_string(&args).unwrap();
    let message = format!("--message={text}");
    let thread_arg = format!("--thread={thread}");
    assert_eq!(got.split_terminator('\0').collect::<Vec<_>>(), ["queue", &thread_arg, &message]);
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!((code, &l[0]["answer"]), (0, &json!(format!("echo: {text}"))), "{l:?}");
    // As a card is cleaned: no escape reaches codex, a newline and a tab do.
    say(&d, &lead, "Kappa", "red \x1b[31mtext\x1b[0m\tand\nmore").unwrap();
    let got = std::fs::read_to_string(&args).unwrap();
    let clean = "red [31mtext[0m\tand\nmore";
    let message = format!("--message={clean}");
    assert_eq!(got.split_terminator('\0').collect::<Vec<_>>(), ["queue", &thread_arg, &message]);
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!(code, 0, "{l:?}");

    // What codex says when it will not, as it said it.
    let why = "Error: a codex app-server is running (pid 4242).\nStop it, then queue again.";
    std::fs::write(d.dir.join("stub/queue.fail"), format!("{why}\n")).unwrap();
    refused("Kappa", "more", &format!("codex queue: {why}"));
    std::fs::remove_file(d.dir.join("stub/queue.fail")).unwrap();

    refused("Kappa", " \n ", "nothing to say");
    refused("Kappa", &"x".repeat(64 * 1024 + 1), "at most 64 KB");
    let nul = json!({"t": "say", "id": 9, "who": "Kappa", "text": "a\0b"});
    let e = d.req_as(lead["id"].as_str().unwrap(), nul);
    assert!(e["error"].as_str().unwrap().contains("NUL"), "{e}");
    // Someone else's helper is not this lead's to send to; nor is the lead itself.
    let other = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Youmu"}));
    let other = other["resident"].clone();
    ready(&d, &other["id"]);
    let e = say(&d, &other, "Kappa", "hi").unwrap_err();
    assert!(e.contains("Kappa is not one of your helpers"), "{e}");
    refused("Seiran", "hi", "Seiran is you");

    // Gone, it hears nothing; its thread gone from Codex too (`codex delete`), it is back
    // afresh, on no thread until its first prompt.
    let o = inside(&d, &lead, &["close", "Kappa"]);
    assert!(o.status.success(), "{}", common::err(&o));
    refused("Kappa", "hi", "Kappa has departed");
    std::fs::remove_dir_all(d.dir.join("codex/sessions")).unwrap();
    let named = record(&d, &h["id"])["session_at"].as_i64().unwrap();
    let b = d.req(json!({"t": "recall", "id": 4, "who": "Kappa"}));
    assert_eq!(b["t"], "summoned", "{b}");
    refused("Kappa", "hi", "Kappa has not started yet");
    // Later than the old thread's SessionStart, which, come late, would not bring it back.
    let rec = record(&d, &h["id"]);
    assert_eq!(rec["session"], h["id"]);
    assert!(rec["session_at"].as_i64().unwrap() > named, "{rec}");
    // Its old thread got the messages said to it, and nothing since.
    let q = std::fs::read_to_string(d.dir.join(format!("stub/{thread}.queue"))).unwrap();
    assert_eq!(q.split_terminator('\0').collect::<Vec<_>>(), [text, clean]);
}

/// A follow-up to a Claude Code helper is typed in as a card is: several lines in one paste, one
/// prompt, never a prompt per line; and not while something is in its way.
#[test]
fn a_lead_says_more_to_its_claude_helper_in_one_paste() {
    let d = daemon_with("sayc", true, &[("STUB_HOOKS", "1")]);
    let lead = lead(&d);
    let o = inside(&d, &lead, &["new", "--json", "--name", "Mima", "--prompt", "brief"]);
    assert!(o.status.success(), "{}", common::err(&o));
    let h: Value = serde_json::from_slice(&o.stdout).unwrap();
    ready(&d, &h["id"]);
    common::wait_for(LONG, || me(&d, "Mima")["turns"] == 1, "its brief's turn");
    let said = say(&d, &lead, "Mima", "line one\nline two").unwrap();
    assert!(said.contains("said to Mima"), "{said}");
    common::wait_for(LONG, || me(&d, "Mima")["turns"] == 2, "the message's turn");
    let input = d.stub(&h["id"], "input");
    assert!(input.contains("\x1b[200~line one\nline two\x1b[201~\n"), "{input:?}");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(me(&d, "Mima")["turns"], 2, "a line went in as a prompt of its own");

    // A prompt half typed into it: refused, with why.
    keys(&d, "Mima", b"half");
    let e = say(&d, &lead, "Mima", "more").unwrap_err();
    assert!(e.contains("Mima has a prompt half typed into it"), "{e}");
}

/// After an interrupted turn Codex runs nothing queued until its pane is typed into: `say` is
/// refused until a turn begins again.
#[test]
fn a_codex_helper_whose_turn_was_interrupted_is_said_nothing_until_a_turn_begins() {
    let d = daemon_with("sayi", true, &[("STUB_WORK", "3")]);
    let lead = lead(&d);
    helper(&d, &lead, "Kappa", "/perm");
    let (code, _) = waited(&d, &lead, &["Kappa", "--until", "needs", "--timeout", "60s"]);
    assert_eq!(code, 0);
    keys(&d, "Kappa", b"\x1b");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!((code, &l[0]["ended"]), (0, &json!("interrupted")), "{l:?}");
    let e = say(&d, &lead, "Kappa", "carry on").unwrap_err();
    assert!(e.contains("Kappa's last turn was interrupted"), "{e}");
    assert!(!d.dir.join("stub/queue.args").exists());

    // Typed into by the user: a new turn, and the words go in behind it.
    keys(&d, "Kappa", b"again\r");
    common::wait_for(LONG, || me(&d, "Kappa")["state"] == "busy", "its new turn");
    let said = say(&d, &lead, "Kappa", "carry on").unwrap();
    assert!(said.contains("queued for Kappa"), "{said}");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!((code, &l[0]["answer"]), (0, &json!("echo: again")), "{l:?}");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!((code, &l[0]["answer"]), (0, &json!("echo: carry on")), "{l:?}");
}

/// Closed and recalled, a Codex helper resumes its own thread: the same session, its hooks say
/// so at its next prompt, and an interrupted turn from before it left holds nothing back.
#[test]
fn a_recalled_codex_helper_resumes_its_thread_and_a_turn_cut_off_before_holds_nothing() {
    let d = daemon_with("resume", true, &[("STUB_TAIL", TAIL)]);
    let lead = lead(&d);
    let h = helper(&d, &lead, "Kappa", "/perm");
    let (code, _) = waited(&d, &lead, &["Kappa", "--until", "needs", "--timeout", "60s"]);
    assert_eq!(code, 0);
    keys(&d, "Kappa", b"\x1b");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!((code, &l[0]["ended"]), (0, &json!("interrupted")), "{l:?}");
    let e = say(&d, &lead, "Kappa", "carry on").unwrap_err();
    assert!(e.contains("Kappa's last turn was interrupted"), "{e}");
    let thread = d.stub(&h["id"], "thread").trim().to_string();
    let o = inside(&d, &lead, &["close", "Kappa"]);
    assert!(o.status.success(), "{}", common::err(&o));

    std::fs::remove_file(d.dir.join(format!("stub/{}.ready", h["id"].as_str().unwrap()))).unwrap();
    let b = d.req(json!({"t": "recall", "id": 4, "who": "Kappa"}));
    assert_eq!(b["t"], "summoned", "{b}");
    let args = d.stub(&h["id"], "args");
    let args: Vec<&str> = args.split_terminator('\0').collect();
    assert_eq!(args[args.len() - 2..], ["resume", thread.as_str()]);
    assert_eq!(d.stub(&h["id"], "thread").trim(), thread);
    common::wait_for(LONG, || me(&d, "Kappa")["blocked"].is_null(), "it to have started");
    // A new Codex, which runs what is queued: the old interrupt is not in the way.
    let said = say(&d, &lead, "Kappa", "carry on").unwrap();
    assert!(said.contains("queued for Kappa"), "{said}");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!((code, &l[0]["answer"]), (0, &json!("echo: carry on")), "{l:?}");
    let o = inside(&d, &lead, &["read", "Kappa"]);
    assert!(common::out(&o).contains("echo: carry on"), "{}", common::out(&o));
    // On the same thread all along, its SessionStart saying it was resumed.
    let log = d.log();
    let starts = log.iter().filter(|l| l["id"] == h["id"] && l["event"] == "SessionStart");
    let kinds: Vec<&Value> = starts.map(|l| &l["kind"]).collect();
    assert_eq!(kinds, [&json!("startup"), &json!("resume")]);
    assert_eq!(record(&d, &h["id"])["session"], thread.as_str());
}
