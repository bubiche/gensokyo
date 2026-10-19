//! `new --cloud`: a cloud session started from a pushed branch of a GitHub repository, against
//! real git with a bare repository standing in for GitHub, and tests/stub-claude's `--cloud`.

mod common;

use common::{Daemon, err, fresh, out};
use gensokyo::cloud;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const LINK: &str = "https://claude.ai/code/session_01NewStub";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A daemon, and a repo `work` on main, pushed, whose origin reads as GitHub's
/// `git@github.com:o/r.git` and is the bare `remote.git` beside it.
fn setup(name: &str, config: &str) -> (Daemon, PathBuf) {
    let dir = fresh(&format!("cn-{name}"));
    let env = vec![
        ("GENSOKYO_FETCH_LIMIT_MS".into(), "5000".into()),
        ("GIT_CEILING_DIRECTORIES".into(), dir.display().to_string()),
    ];
    let d = Daemon::at(dir, env);
    std::fs::create_dir_all(d.dir.join("conf")).unwrap();
    std::fs::write(d.dir.join("conf/config"), format!("BRANCH_PREFIX=me/\n{config}")).unwrap();
    let work = d.dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(d.dir.join("stub")).unwrap();
    git(&d.dir, &["init", "-q", "--bare", "remote.git"]);
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
    git(&work, &["remote", "add", "origin", "git@github.com:o/r.git"]);
    let bare = d.dir.join("remote.git").display().to_string();
    git(&work, &["config", &format!("url.{bare}.insteadOf"), "git@github.com:o/r.git"]);
    git(&work, &["push", "-q", "-u", "origin", "main"]);
    git(&work, &["remote", "set-head", "origin", "main"]);
    d.cli(&["list"]);
    (d, std::fs::canonicalize(&work).unwrap())
}

fn summon(d: &Daemon, cwd: &Path, extra: Value) -> Value {
    let mut req = json!({"t": "summon", "id": 7, "cwd": cwd, "cloud": true});
    req.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    d.req(req)
}

/// What the stub was run with for the session it made, and where.
fn made(d: &Daemon) -> Option<(Vec<String>, String)> {
    let args = std::fs::read_to_string(d.dir.join("stub/cloud.new.args")).ok()?;
    let cwd = std::fs::read_to_string(d.dir.join("stub/cloud.new.cwd")).unwrap();
    let args = args.strip_suffix('\0').unwrap_or(&args).split('\0').map(String::from).collect();
    Some((args, cwd.trim().to_string()))
}

fn record(d: &Daemon, id: &Value) -> Value {
    let f = d.dir.join(format!("residents/{}.json", id.as_str().unwrap()));
    serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap()
}

#[test]
fn a_repository_on_github_is_known_by_its_remote_and_an_alias_is_not() {
    for url in [
        "git@github.com:o/r.git",
        "git@github.com:o/r",
        "https://github.com/o/r.git",
        "https://github.com/o/r",
        "https://github.com/o/r/",
        "ssh://git@github.com/o/r.git",
    ] {
        assert_eq!(cloud::github(url).as_deref(), Ok("o/r"), "{url}");
    }
    let alias = cloud::github("git@github.com-me:o/r.git").unwrap_err();
    assert!(alias.contains("SSH host alias"), "{alias}");
    assert!(alias.contains("git remote set-url origin git@github.com:o/r.git"), "{alias}");
    assert!(alias.contains("set-url --push origin git@github.com-me:o/r.git"), "{alias}");
    for url in
        ["../remote.git", "git@gitlab.com:o/r.git", "https://github.com/o", "git@github.com:o/r/x"]
    {
        let e = cloud::github(url).unwrap_err();
        assert!(e.contains("not a GitHub repository"), "{url}: {e}");
    }
    assert!(cloud::github("git@github.com:-o/r.git").is_err());
    assert_eq!(cloud::github("git@github.com:o/.github.git").as_deref(), Ok("o/.github"));
    let e = cloud::github("https://x:ghp_secret@gitlab.com/o/r").unwrap_err();
    assert!(!e.contains("ghp_secret"), "{e}");
}

#[test]
fn what_claude_prints_for_a_new_session_is_read_strictly() {
    let said = |view: &str, resume: &str| {
        format!(
            "Created cloud session: fix it\nView: {view}\nResume with: claude --teleport {resume}\n"
        )
    };
    let ok = said(&format!("{LINK}?from=cli&m=0"), "session_01NewStub");
    assert_eq!(cloud::created(&ok), Ok(("cse_01NewStub".into(), LINK.into())));
    // A carriage return ending a line changes nothing.
    let crlf = ok.replace('\n', "\r\n");
    assert_eq!(cloud::created(&crlf), Ok(("cse_01NewStub".into(), LINK.into())));
    for bad in [
        said(LINK, "session_01Other"),
        said("https://claude.ai/code/session_01New;x", "session_01New;x"),
        said("https://evil.example/code/session_01NewStub", "session_01NewStub"),
        "Created cloud session: fix it\nResume with: claude --teleport session_01NewStub\n".into(),
        format!("Created cloud session: x\nView: {LINK}\n"),
        "Error: not logged in\n".into(),
    ] {
        assert!(cloud::created(&bad).is_err(), "{bad:?}");
    }
    let e = cloud::created("Error: not logged in\n").unwrap_err();
    assert!(e.contains("not logged in"), "{e}");
    let e = cloud::created("Created cloud session: x\nView: (soon)\n").unwrap_err();
    assert!(e.contains("claude made a session"), "{e}");
}

#[test]
fn a_cloud_session_starts_from_the_pushed_branch_and_is_an_entry_with_no_process() {
    let (d, work) = setup("new", "CLOUD_ENV=env_01A\n");
    let r = summon(&d, &work, json!({"name": "Aya", "prompt": "fix the bug\nand say so"}));
    assert_eq!(r["t"], "summoned", "{r}");
    let me = &r["resident"];
    assert_eq!(
        (&me["name"], &me["cloud"], &me["pid"]),
        (&json!("Aya"), &json!(LINK), &Value::Null)
    );
    assert_eq!((&me["departed"], &me["detail"]), (&Value::Null, &json!("starting")), "{me}");
    let (args, cwd) = made(&d).expect("claude --cloud ran");
    let settings = json!({"remote": {"defaultEnvironmentId": "env_01A"}}).to_string();
    assert_eq!(args, ["--settings", &settings, "--cloud", "fix the bug\nand say so"]);
    assert_eq!(cwd, work.display().to_string());
    let rec = record(&d, &me["id"]);
    let want = json!({"session": "cse_01NewStub", "url": LINK, "repo": "o/r", "base": "main",
                      "env": "env_01A"});
    assert_eq!(rec["cloud"], want, "{rec}");
    assert_eq!(rec["prompt"], "fix the bug\nand say so");
    let listed = d.list();
    assert_eq!((listed.len(), &listed[0]["cloud"]), (1, &json!(LINK)));

    // Its own environment, and none: no --settings.
    std::fs::write(d.dir.join("conf/config"), "").unwrap();
    std::fs::write(d.dir.join("stub/cloud.new"), "Created cloud session: x\nView: https://claude.ai/code/session_02B\nResume with: claude --teleport session_02B\n").unwrap();
    let r = summon(&d, &work, json!({"prompt": "two", "env": "env_02B"}));
    assert_eq!(r["resident"]["cloud"], "https://claude.ai/code/session_02B", "{r}");
    let settings = json!({"remote": {"defaultEnvironmentId": "env_02B"}}).to_string();
    assert_eq!(made(&d).unwrap().0, ["--settings", &settings, "--cloud", "two"]);
    std::fs::write(d.dir.join("stub/cloud.new"), "Created cloud session: x\nView: https://claude.ai/code/session_03C\nResume with: claude --teleport session_03C\n").unwrap();
    let r = summon(&d, &work, json!({"prompt": "three", "model": "sonnet"}));
    assert_eq!(r["t"], "summoned", "{r}");
    assert_eq!(made(&d).unwrap().0, ["--model", "sonnet", "--cloud", "three"]);
    assert_eq!(record(&d, &r["resident"]["id"])["cloud"].get("env"), None);
}

#[test]
fn a_cloud_session_in_a_worktree_pushes_its_new_branch_first() {
    let (d, work) = setup("wt", "");
    let r = summon(&d, &work, json!({"prompt": "go", "worktree": {"slug": "fix"}}));
    assert_eq!(r["t"], "summoned", "{r}");
    let wt = work.join(".claude/worktrees/fix");
    assert_eq!(made(&d).unwrap().1, wt.display().to_string());
    let tip = git(&wt, &["rev-parse", "HEAD"]);
    assert_eq!(git(&d.dir.join("remote.git"), &["rev-parse", "me/fix"]), tip, "pushed");
    assert_eq!(git(&wt, &["rev-parse", "--abbrev-ref", "@{u}"]), "origin/me/fix");
    let rec = record(&d, &r["resident"]["id"]);
    assert_eq!((&rec["cloud"]["base"], &rec["cwd"]), (&json!("me/fix"), &json!(wt)), "{rec}");
}

#[test]
fn a_repo_the_cloud_cannot_clone_or_a_branch_it_would_not_see_is_refused_before_claude_runs() {
    let (d, work) = setup("refused", "");
    let refused = |extra: Value, want: &str| {
        let r = summon(&d, &work, extra);
        let e = r["error"].as_str().unwrap_or_else(|| panic!("{r}"));
        assert!(e.contains(want), "{want}: {e}");
        assert!(made(&d).is_none(), "claude ran for: {want}");
    };
    refused(json!({"prompt": "  "}), "a cloud session needs a prompt");
    refused(json!({"prompt": "-x"}), "starts with -");
    for task in ["session_01X", "cse_01X", "https://claude.ai/code/session_01X", "https://x.y/z"] {
        refused(json!({"prompt": task}), "which claude would attach to");
    }
    refused(json!({"prompt": "go", "env": "prod"}), "env_");
    for f in ["role", "system_prompt", "teleport", "mode", "effort"] {
        refused(json!({"prompt": "go", f: "x"}), "a cloud session");
    }
    refused(json!({"prompt": "go", "allowed_tools": ["Read"]}), "a cloud session");

    git(&work, &["commit", "-q", "--allow-empty", "-m", "local"]);
    refused(json!({"prompt": "go"}), "origin's main is not where yours is");
    git(&work, &["switch", "-q", "-c", "new"]);
    refused(
        json!({"prompt": "go"}),
        "new is not on origin: push it first (git push -u origin new)",
    );
    git(&work, &["switch", "-q", "--detach"]);
    refused(json!({"prompt": "go"}), "not on a branch");
    git(&work, &["switch", "-q", "main"]);
    git(&work, &["push", "-q", "origin", "main"]);

    git(&work, &["remote", "set-url", "origin", "git@github.com-me:o/r.git"]);
    refused(json!({"prompt": "go"}), "SSH host alias");
    // Refused before the worktree is made, too.
    refused(json!({"prompt": "go", "worktree": {"slug": "nope"}}), "SSH host alias");
    assert!(!work.join(".claude/worktrees/nope").exists());
    git(&work, &["remote", "set-url", "origin", "../remote.git"]);
    refused(json!({"prompt": "go"}), "not a GitHub repository");
    git(&work, &["remote", "remove", "origin"]);
    refused(json!({"prompt": "go"}), "no origin");
    let elsewhere = d.dir.join("plain");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let r = summon(&d, &elsewhere, json!({"prompt": "go"}));
    assert!(r["error"].as_str().unwrap().contains("not in a git repository"), "{r}");
    assert!(d.list().is_empty());
}

#[test]
fn what_claude_says_instead_of_a_session_is_told_with_any_link_it_gave() {
    let (d, work) = setup("odd", "");
    std::fs::write(d.dir.join("stub/cloud.new"), "Error: you are not logged in\n").unwrap();
    let r = summon(&d, &work, json!({"prompt": "go"}));
    assert!(r["error"].as_str().unwrap().contains("you are not logged in"), "{r}");
    std::fs::write(
        d.dir.join("stub/cloud.new"),
        format!("View: {LINK}\nResume with: claude --teleport session_01Other\n"),
    )
    .unwrap();
    let r = summon(&d, &work, json!({"prompt": "go"}));
    let e = r["error"].as_str().unwrap();
    assert!(e.contains(LINK), "a session it may have made is named: {e}");
    assert!(e.contains("look there first"), "{e}");
    assert!(d.list().is_empty());
}

#[test]
fn only_the_user_starts_a_cloud_session() {
    let (d, work) = setup("lead", "");
    let r = d.req_as(
        "11111111-2222-3333-4444-555555555555",
        json!({"t": "summon", "id": 7, "cwd": work, "cloud": true, "prompt": "go"}),
    );
    assert!(r["error"].as_str().unwrap().contains("the user starts cloud sessions"), "{r}");
    assert!(made(&d).is_none());
}

#[test]
fn a_cloud_entry_outlives_a_restart_and_a_quit_and_is_never_started_here() {
    let (d, work) = setup("back", "");
    let r = summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    let id = r["resident"]["id"].clone();
    let started = |d: &Daemon| {
        std::fs::read_dir(d.dir.join("stub")).unwrap().flatten().any(|f| {
            f.file_name().to_string_lossy().ends_with(".args")
                && !f.file_name().to_string_lossy().starts_with("cloud")
        })
    };
    let o = d.cli(&["restart"]);
    assert!(out(&o).contains("recalled Aya"), "{}", out(&o));
    let listed = d.list();
    assert_eq!((&listed[0]["id"], &listed[0]["cloud"]), (&id, &json!(LINK)));
    assert!(listed[0]["departed"].is_null() && listed[0]["pid"].is_null(), "{}", listed[0]);

    // Banished or typed into, it says why not.
    let o = d.command(&["banish", "Aya"]).output().unwrap();
    assert!(err(&o).contains("Aya is a cloud session: it has no process here"), "{}", err(&o));

    // Quit, it is departed; resumed, it is followed again, and nothing is started for it.
    d.cli(&["quit"]);
    d.cli(&["list"]);
    let gone = d.req(json!({"t": "list", "id": 1, "all": true}))["residents"][0].clone();
    assert!(gone["departed"].is_i64(), "{gone}");
    let o = d.cli(&["resume", "Aya"]);
    assert!(out(&o).contains("recalled Aya"), "{}", out(&o));
    assert!(d.list()[0]["departed"].is_null());
    assert!(!started(&d), "no claude was started for a cloud session");

    // Closed, it leaves the shrine and says where the session lives on.
    let o = d.cli(&["close", "Aya"]);
    assert!(out(&o).contains(LINK), "{}", out(&o));
    assert!(d.list().is_empty());
    let gone = d.req(json!({"t": "list", "id": 1, "all": true}))["residents"][0].clone();
    assert!(gone["departed"].is_i64(), "closed, it is departed: {gone}");
}

/// The daemon's pid, newest first started.
fn daemon_pid(d: &Daemon) -> i32 {
    let log = std::fs::read_to_string(d.dir.join("daemon.log")).unwrap();
    let started = log.lines().rfind(|l| l.contains(r#""ev":"started""#)).unwrap();
    serde_json::from_str::<Value>(started).unwrap()["pid"].as_i64().unwrap() as i32
}

#[test]
fn a_cloud_entry_comes_back_after_a_crash_and_a_logout() {
    let (d, work) = setup("crash", "");
    summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    for sig in [libc::SIGKILL, libc::SIGTERM] {
        let pid = daemon_pid(&d);
        // SAFETY: plain libc call, on the daemon this test started.
        unsafe { libc::kill(pid, sig) };
        common::wait(|| unsafe { libc::kill(pid, 0) } != 0, "the daemon to stop");
        d.cli(&["list"]);
        let listed = d.list();
        assert_eq!((&listed[0]["name"], &listed[0]["cloud"]), (&json!("Aya"), &json!(LINK)));
        assert!(listed[0]["departed"].is_null(), "{sig}: {}", listed[0]);
    }
}

#[test]
fn new_cloud_on_the_command_line() {
    let (d, work) = setup("cli", "");
    let o = d
        .command(&["new", "--cloud", "-n", "Aya", "--prompt", "go"])
        .current_dir(&work)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(out(&o).trim(), format!("started Aya in the cloud: {LINK}"));
    let o = d.command(&["new", "--env", "env_01A", "--prompt", "go"]).output().unwrap();
    assert!(!o.status.success() && err(&o).contains("--cloud"), "{}", err(&o));
    let o =
        d.command(&["new", "--cloud", "--role", "reviewer", "--prompt", "go"]).output().unwrap();
    assert!(!o.status.success() && err(&o).contains("cannot be used with"), "{}", err(&o));
}
