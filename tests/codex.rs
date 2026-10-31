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
    let para = codex::paragraph("Nitori", Some("Seiran"), None);
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
    };
    assert_eq!(back, want);

    // Unset, each is Codex's own: no flag for it, and no prompt.
    let a = argv("/g", &opts());
    assert_eq!(codex::launched(&a), Launched::default());
    assert!(!a.iter().any(|w| ["--", "-m", "-s", "-a"].contains(&w.as_str())));
    assert!(value(&a, "model_reasoning_effort").is_none());
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
    let plain = codex::paragraph("Nitori", None, None);
    assert!(plain.contains("inside gensokyo") && plain.contains("resident name is Nitori"));
    assert!(!plain.contains("helper"));
    let helper = codex::paragraph("Nitori", Some("Seiran"), None);
    for says in [
        "the resident Seiran summoned you",
        "your last message is your report to Seiran",
        "end your turn with the question",
    ] {
        assert!(helper.contains(says), "{says}");
    }
    let ledger = codex::paragraph("Nitori", None, Some(Path::new("/l/proj.md")));
    assert!(ledger.contains("ledger is `/l/proj.md`"));
    // Nothing a Codex resident has no use for.
    for p in [plain, helper, ledger] {
        for not in ["SendMessage", "skill", "Agent tool", "Edit tool", "claude agents"] {
            assert!(!p.contains(not), "{not}: {p}");
        }
    }
}

#[test]
fn a_resident_gets_none_of_the_codex_session_it_was_started_from() {
    let base = [
        ("CODEX_THREAD_ID", "t"),
        ("CODEX_SANDBOX", "seatbelt"),
        ("CODEX_HOME", "/h/.codex"),
        ("CODEXY", "kept"),
        ("HOME", "/h"),
    ];
    let base = base.map(|(k, v)| (OsString::from(k), OsString::from(v)));
    let env = launch::env(base, "id", Path::new("/g"), Path::new("/shim"), Path::new("/s"));
    let has = |k: &str| env.iter().any(|(n, _)| n == k);
    assert!(!has("CODEX_THREAD_ID") && !has("CODEX_SANDBOX"));
    assert!(has("CODEX_HOME") && has("CODEXY") && has("HOME"));
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

/// A daemon whose Codex residents are tests/stub-codex, with Codex trusting its directory or
/// not.
fn daemon(name: &str, trusted: bool) -> common::Daemon {
    let dir = fresh(&format!("cx-{name}"));
    std::fs::create_dir_all(dir.join("codex")).unwrap();
    if trusted {
        let config = format!("[projects.\"{}\"]\ntrust_level = \"trusted\"\n", dir.display());
        std::fs::write(dir.join("codex/config.toml"), config).unwrap();
    }
    let d = common::Daemon::at(dir, vec![]);
    d.cli(&["list"]);
    d
}

/// A Claude lead, in acceptEdits.
fn lead(d: &common::Daemon) -> Value {
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": "Seiran",
                         "mode": "acceptEdits"}));
    assert_eq!(r["t"], "summoned", "{r}");
    d.stub(&r["resident"]["id"], "ready");
    r["resident"].clone()
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

fn record(d: &common::Daemon, id: &Value) -> Value {
    let f = d.dir.join(format!("residents/{}.json", id.as_str().unwrap()));
    serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect()
}

#[test]
fn a_codex_resident_is_started_listed_left_and_started_afresh() {
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

    // Back as it was summoned, with no thread to resume and no prompt to say again.
    std::fs::remove_file(d.dir.join(format!("stub/{}.ready", r["id"].as_str().unwrap()))).unwrap();
    let b = d.req(json!({"t": "recall", "id": 4, "who": "Nitori"}));
    assert_eq!(b["t"], "summoned", "{b}");
    let again = strings(&record(&d, &r["id"])["argv"]);
    assert_eq!(again[..], argv[..argv.len() - 2]);
    assert_eq!(me(&d, "Nitori")["mode"], "acceptEdits");
    d.stub(&r["id"], "ready");
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
    let d = daemon("turn", true);
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
    // Codex's spinner draws a Working frame every tenth of a second meanwhile: for two seconds of
    // them, it is still waiting on the user.
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(2) {
        assert_eq!(me(&d, "Kappa")["state"], "awaits", "the spinner cleared the approval");
    }
    keys(&d, "Kappa", b"y");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!(code, 0, "{l:?}");
    assert_eq!((&l[0]["ended"], &l[0]["answer"]), (&json!("stop"), &json!("echo: /perm")));
    let k = me(&d, "Kappa");
    assert_eq!((&k["state"], &k["needs"]), (&json!("resting"), &json!(1)), "{k}");
}

#[test]
fn a_codex_approval_refused_ends_the_turn_interrupted() {
    let d = daemon("esc", true);
    let lead = lead(&d);
    helper(&d, &lead, "Kappa", "/perm");
    let (code, _) = waited(&d, &lead, &["Kappa", "--until", "needs", "--timeout", "60s"]);
    assert_eq!(code, 0);
    keys(&d, "Kappa", b"\x1b");
    let (code, l) = waited(&d, &lead, &["Kappa", "--until", "done", "--timeout", "60s"]);
    assert_eq!((code, &l[0]["ended"]), (0, &json!("interrupted")), "{l:?}");
    assert_eq!(l[0]["answer"], Value::Null);
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
}
