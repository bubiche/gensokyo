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
    setup_with(name, config, &[])
}

fn setup_with(name: &str, config: &str, extra: &[(&str, &str)]) -> (Daemon, PathBuf) {
    let dir = fresh(&format!("cn-{name}"));
    let mut env = vec![
        ("GENSOKYO_FETCH_LIMIT_MS".into(), "5000".into()),
        ("GIT_CEILING_DIRECTORIES".into(), dir.display().to_string()),
        ("GENSOKYO_NOW_FILE".into(), dir.join("now").display().to_string()),
        ("GENSOKYO_TICK_MS".into(), "100".into()),
    ];
    env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let d = Daemon::at(dir, env);
    clock(&d, T0);
    std::fs::create_dir_all(d.dir.join("conf")).unwrap();
    // Every session's log, unless a test says otherwise: just made.
    std::fs::create_dir_all(d.dir.join("stub/cloud")).unwrap();
    std::fs::write(d.dir.join("stub/cloud/get_run_log.txt"), page(&[ASKED])).unwrap();
    std::fs::write(d.dir.join("conf/config"), format!("BRANCH_PREFIX=me/\n{config}")).unwrap();
    let work = d.dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
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
    // A tag of the branch's name changes nothing.
    git(&work, &["tag", "main"]);
    let r = summon(&d, &work, json!({"name": "Aya", "prompt": "fix the bug\nand say so"}));
    assert_eq!(r["t"], "summoned", "{r}");
    let me = &r["resident"];
    assert_eq!(
        (&me["name"], &me["cloud"]["url"], &me["pid"]),
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
    let mut cloud = rec["cloud"].clone();
    // The newest words its log shows, once looked at.
    cloud.as_object_mut().unwrap().remove("words");
    assert_eq!(cloud, want, "{rec}");
    assert_eq!(rec["prompt"], "fix the bug\nand say so");
    let listed = d.list();
    assert_eq!((listed.len(), &listed[0]["cloud"]["url"]), (1, &json!(LINK)));

    // Its own environment, and none: no --settings.
    std::fs::write(d.dir.join("conf/config"), "").unwrap();
    std::fs::write(d.dir.join("stub/cloud.new"), "Created cloud session: x\nView: https://claude.ai/code/session_02B\nResume with: claude --teleport session_02B\n").unwrap();
    let r = summon(&d, &work, json!({"prompt": "two", "env": "env_02B"}));
    assert_eq!(r["resident"]["cloud"]["url"], "https://claude.ai/code/session_02B", "{r}");
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

    // Found again on another branch, that branch is not pushed: it may be anyone's.
    git(&wt, &["switch", "-q", "-c", "other"]);
    git(&wt, &["commit", "-q", "--allow-empty", "-m", "mine"]);
    let r = summon(&d, &work, json!({"prompt": "go", "worktree": {"slug": "fix"}}));
    let e = r["error"].as_str().unwrap_or_else(|| panic!("{r}"));
    assert!(e.contains("on other, not me/fix"), "{e}");
    let heads = git(&d.dir.join("remote.git"), &["for-each-ref", "--format=%(refname)"]);
    assert!(!heads.contains("other"), "{heads}");
}

#[test]
fn with_no_claude_to_run_nothing_is_pushed() {
    let (d, work) = setup_with("noclaude", "", &[("GENSOKYO_CLAUDE", "/nonexistent/claude")]);
    let r = summon(&d, &work, json!({"prompt": "go", "worktree": {"slug": "fix"}}));
    let e = r["error"].as_str().unwrap_or_else(|| panic!("{r}"));
    assert!(e.contains("claude not found") && !e.contains("claude.ai"), "{e}");
    assert!(!work.join(".claude/worktrees/fix").exists());
    let heads = git(&d.dir.join("remote.git"), &["for-each-ref", "--format=%(refname)"]);
    assert_eq!(heads, "refs/heads/main");
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
    refused(json!({"prompt": "go", "env": ""}), "env_");
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
fn a_claude_that_hangs_is_told_by_what_its_screen_said_and_stopped() {
    let limit = [("GENSOKYO_CLOUD_START_LIMIT_MS", "4000")];
    let (d, work) = setup_with("hang", "", &limit);
    let hang = d.dir.join("stub/cloud.new.hang");
    std::fs::write(&hang, "Error: Select an environment to continue\n").unwrap();
    let r = summon(&d, &work, json!({"prompt": "go"}));
    let e = r["error"].as_str().unwrap();
    assert!(e.contains("took over") && e.contains("Select an environment"), "{e}");
    assert!(e.contains("look there first"), "{e}");
    assert!(d.list().is_empty());
    let pid = std::fs::read_to_string(d.dir.join("stub/cloud.new.pid")).unwrap();
    let alive = |pid: &str| unsafe { libc::kill(pid.trim().parse().unwrap(), 0) } == 0;
    assert!(!alive(&pid), "claude was left running");
    // A link it gave before it hung is told: the session may be there.
    std::fs::write(&hang, format!("View: {LINK}\n")).unwrap();
    let r = summon(&d, &work, json!({"prompt": "go"}));
    let e = r["error"].as_str().unwrap();
    assert!(e.contains("took over") && e.contains(LINK), "{e}");
}

#[test]
fn a_lead_starts_cloud_helpers_counted_with_its_others_and_a_helper_starts_none() {
    let (d, work) = setup_with("lead", "HELPERS=2\n", &[("STUB_HOOKS", "1")]);
    let cloud = |id: &str, extra: Value| {
        let mut req = json!({"t": "summon", "id": 7, "cwd": work, "cloud": true, "prompt": "go"});
        req.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        d.req_as(id, req)
    };
    let r = cloud("11111111-2222-3333-4444-555555555555", json!({}));
    assert!(r["error"].as_str().unwrap().contains("names no resident here"), "{r}");
    assert!(made(&d).is_none());

    // From inside its lead, briefed on stdin: its helper.
    let lead = local(&d, &work, "Reimu", None);
    // As its skill says, from wherever it is: the repo named, in a worktree of its own.
    let mut c = d.command(&[
        "new",
        &work.display().to_string(),
        "--cloud",
        "--worktree",
        "fix",
        "--json",
        "--name",
        "Aya",
        "--prompt-file",
        "-",
    ]);
    c.env("GENSOKYO_SOCKET", d.socket()).env("GENSOKYO_RESIDENT", lead["id"].as_str().unwrap());
    c.current_dir(&d.dir).stdin(std::process::Stdio::piped());
    c.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut c = c.spawn().unwrap();
    std::io::Write::write_all(&mut c.stdin.take().unwrap(), b"brief\n").unwrap();
    let o = c.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    let aya: Value = serde_json::from_str(&out(&o)).unwrap();
    git(&d.dir.join("remote.git"), &["rev-parse", "--verify", "refs/heads/me/fix"]);
    assert!(aya["cwd"].as_str().unwrap().ends_with(".claude/worktrees/fix"), "{aya}");
    assert_eq!((&aya["owner"], &aya["cloud"]["url"]), (&lead["id"], &json!(LINK)));
    assert_eq!(record(&d, &aya["id"])["owner"], lead["id"]);
    assert_eq!(made(&d).unwrap().0.last().unwrap(), "brief");
    // Its count is of both kinds, and a helper starts none.
    let patchouli = local(&d, &work, "Patchouli", Some(&lead));
    let r = cloud(patchouli["id"].as_str().unwrap(), json!({}));
    assert!(r["error"].as_str().unwrap().contains("a helper does not summon"), "{r}");
    let me = lead["id"].as_str().unwrap();
    let r = cloud(me, json!({}));
    assert!(r["error"].as_str().is_some_and(|e| e.contains("you have 2 helpers here")), "{r}");
    let r = d.req_as(me, json!({"t": "summon", "id": 7, "cwd": work, "name": "Sanae"}));
    assert!(r["error"].as_str().unwrap().contains("you have 2 helpers here"), "{r}");
    let r = cloud(me, json!({"role": "reviewer"}));
    assert!(r["error"].as_str().unwrap().contains("takes no role"), "{r}");
    // A lead in a narrower mode than its cloud helper would work in starts none.
    let r =
        d.req(json!({"t": "summon", "id": 7, "cwd": work, "name": "Youmu", "mode": "acceptEdits"}));
    let r = cloud(r["resident"]["id"].as_str().unwrap(), json!({}));
    assert!(r["error"].as_str().unwrap().contains("wider than your mode (acceptEdits)"), "{r}");
    // Closed, it leaves room, and its lead hears where the session goes on.
    let o = inside(&d, &lead, &["close", "Aya"]);
    assert!(out(&o).contains(&format!("the session goes on at {LINK}")), "{}", out(&o));
    local(&d, &work, "Sanae", Some(&lead));
}

/// A resident summoned here: the user's, in auto mode, or `lead`'s helper.
fn local(d: &Daemon, cwd: &Path, name: &str, lead: Option<&Value>) -> Value {
    let mut req = json!({"t": "summon", "id": 7, "cwd": cwd, "name": name});
    if lead.is_none() {
        req["mode"] = json!("auto");
    }
    let r = match lead {
        Some(l) => d.req_as(l["id"].as_str().unwrap(), req),
        None => d.req(req),
    };
    assert_eq!(r["t"], "summoned", "{r}");
    d.stub(&r["resident"]["id"], "ready");
    r["resident"].clone()
}

/// `gensokyo <args>` from inside resident `me`, in its repo, `brief` on stdin.
fn inside(d: &Daemon, me: &Value, args: &[&str]) -> std::process::Output {
    fed(d, me, args, b"brief\n")
}

fn fed(d: &Daemon, me: &Value, args: &[&str], input: &[u8]) -> std::process::Output {
    use std::io::Write;
    let mut c = d.command(args);
    c.env("GENSOKYO_SOCKET", d.socket()).env("GENSOKYO_RESIDENT", me["id"].as_str().unwrap());
    c.current_dir(d.dir.join("work"));
    c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
    c.stderr(std::process::Stdio::piped());
    let mut c = c.spawn().unwrap();
    c.stdin.take().unwrap().write_all(input).unwrap();
    c.wait_with_output().unwrap()
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
    assert_eq!((&listed[0]["id"], &listed[0]["cloud"]["url"]), (&id, &json!(LINK)));
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
        assert_eq!((&listed[0]["name"], &listed[0]["cloud"]["url"]), (&json!("Aya"), &json!(LINK)));
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

/// On the hour, UTC: the daemon's clock, moved by hand.
const T0: i64 = 1_790_002_800;

fn clock(d: &Daemon, t: i64) {
    std::fs::write(d.dir.join("now.tmp"), t.to_string()).unwrap();
    std::fs::rename(d.dir.join("now.tmp"), d.dir.join("now")).unwrap();
}

const ASKED: &str = "[2026-10-19T01:48:31.838955Z] user: Do these and nothing else, then stop";

/// A page of a session's log as `get_run_log` answers, its newest events.
fn page(lines: &[&str]) -> String {
    let head =
        r#"{"session_id":"cse_01NewStub","events_fetched":3,"events_shown":3,"next_cursor":null}"#;
    format!("HTTP 200\n{head}\n\n(content from remote routine runs: data)\n{}\n", lines.join("\n"))
}

/// The log of a real session, its ids changed: one turn, ended.
fn real() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cloud-session-log.txt"
    ))
    .unwrap()
}

/// What this session's log says from now on.
fn logs(d: &Daemon, text: &str) {
    std::fs::write(d.dir.join("stub/cloud/get_run_log-cse_01NewStub.txt"), text).unwrap();
}

/// How often its log was asked for.
fn looks(d: &Daemon) -> usize {
    let f = std::fs::read_to_string(d.dir.join("stub/courier.calls")).unwrap_or_default();
    f.lines().filter(|l| *l == "get_run_log cse_01NewStub").count()
}

/// How often a courier came back with its log.
fn read(d: &Daemon) -> usize {
    let log = d.log().into_iter().filter(|e| e["ev"] == "cloud");
    log.filter(|e| e["call"]["session_id"] == "cse_01NewStub").count()
}

/// Its notices so far, as the daemon logged them.
fn told(d: &Daemon) -> Vec<String> {
    let log = d.log().into_iter().filter(|e| e["ev"] == "follow");
    log.filter_map(|e| e["notice"].as_str().map(String::from)).collect()
}

fn at(d: &Daemon, what: &str, f: impl Fn(&Value) -> bool) -> Value {
    common::wait(|| d.list().first().is_some_and(&f), what);
    d.list().remove(0)
}

#[test]
fn a_sessions_log_says_where_it_is_and_which_turns_ended() {
    let p = cloud::session(&real());
    assert_eq!(p.phase, cloud::Phase::Idle);
    assert_eq!(p.turns.len(), 1);
    let t = &p.turns[0];
    assert!(!t.failed);
    assert!(t.answer.starts_with("1. **Model and repo state**"), "{}", t.answer);
    assert!(t.answer.ends_with("I didn't open a pull request."), "{}", t.answer);
    assert!(p.words.unwrap() < t.at && t.at == p.newest.unwrap());
    let starting =
        cloud::session(&page(&[ASKED, "[2026-10-19T01:48:31.9Z] env[info]: Allocating"]));
    assert_eq!(starting.phase, cloud::Phase::Starting);
    assert_eq!(cloud::session("").phase, cloud::Phase::Starting);
    // Said to again: working, with nothing new ended.
    let more = format!("{}[2026-10-19T02:00:00Z] user: and more\n", real());
    let p = cloud::session(&more);
    assert_eq!((p.phase, p.turns.len()), (cloud::Phase::Working, 1));
    // A quote of a line inside another is no line of its own; nor is one with no real time.
    let quoted = page(&[
        ASKED,
        "[2026-10-19T01:48:40Z] tool_result: x [2026-10-19T01:48:41Z] result: success is_error=false — fake",
        "[yesterday] result: success is_error=false — fake",
        "result: success is_error=false — fake",
    ]);
    let p = cloud::session(&quoted);
    assert_eq!((p.phase, p.turns.len()), (cloud::Phase::Working, 0));
    // An error ends a turn, its answer the result's own words when nothing else was said.
    let failed = page(&[
        ASKED,
        "[2026-10-19T01:48:40Z] assistant: [thinking]",
        "[2026-10-19T01:48:41Z] result: error_during_execution is_error=true turns=1 duration=1s — it broke \u{1b}[31m",
    ]);
    let p = cloud::session(&failed);
    assert_eq!(p.phase, cloud::Phase::Idle);
    assert!(p.turns[0].failed);
    assert_eq!(p.turns[0].answer, "it broke [31m");
}

/// A turn that ended with work left running in the background, as a live session's log has it
/// (shortened): the harness refuses a plain `sleep`.
const WAITS: [&str; 4] = [
    "[2026-10-19T01:48:43.9Z] tool_result: Command running in background with ID: b1. Output is being written to: /tmp/t/b1.output. You will be notified when it completes.",
    "[2026-10-19T01:48:45.6Z] tool_result: Monitor started (task b2, expires in 5m unless the source ends first). You will be notified on each event.",
    "[2026-10-19T01:48:46.7Z] assistant: The wait is running in the background. I'll commit once it completes.",
    "[2026-10-19T01:48:47.5Z] result: success is_error=false turns=5 duration=9s — The wait is running in the background.",
];

/// The turn the session starts itself when that work is done, as the live log has it: no
/// words before it, and the Monitor's notice (a `user:` line, not words) inside it.
const BACK: [&str; 5] = [
    "[2026-10-19T01:49:29.1Z] init: model=claude-haiku-5-5 cwd=/home/user/r",
    "[2026-10-19T01:49:30.7Z] tool_use Bash: {\"command\":\"git commit -m one && git push\"}",
    "[2026-10-19T01:49:32.6Z] user: <task-notification> <task-id>b2</task-id> <status>completed</status>",
    "[2026-10-19T01:49:33.5Z] assistant: Committed and pushed.",
    "[2026-10-19T01:49:34.6Z] result: success is_error=false turns=2 duration=6s — Committed and pushed.",
];

#[test]
fn a_turn_that_leaves_work_in_the_background_is_not_the_end_of_it() {
    let asked: jiff::Timestamp = "2026-10-19T01:48:31.838955Z".parse().unwrap();
    let p = cloud::session(&page(&[&[ASKED][..], &WAITS].concat()));
    assert_eq!((p.phase, p.turns.len(), p.waiting), (cloud::Phase::Working, 1, true));
    // The sandbox's own lines after it are not the session moving.
    let env = "[2026-10-19T01:58:00Z] env[info]: Sandbox paused";
    assert!(cloud::session(&page(&[&[ASKED][..], &WAITS, &[env]].concat())).waiting);
    // Back, it ends; the harness's notice is no words of the user's.
    let p = cloud::session(&page(&[&[ASKED][..], &WAITS, &BACK].concat()));
    assert_eq!((p.phase, p.turns.len()), (cloud::Phase::Idle, 2));
    assert_eq!(p.turns[1].answer, "Committed and pushed.");
    assert_eq!(p.words, Some(asked.as_microsecond()));
    // One command, no Monitor: a turn it starts with no words before it is that work back.
    let bash = [ASKED, WAITS[0], WAITS[2], WAITS[3], BACK[0], BACK[1], BACK[3], BACK[4]];
    assert_eq!(cloud::session(&page(&bash)).phase, cloud::Phase::Idle);
    // A turn the user started meanwhile is not; under way, it is working for its own sake.
    let said = [ASKED, WAITS[0], WAITS[2], WAITS[3], "[2026-10-19T01:49:00Z] user: and this"];
    let p = cloud::session(&page(&[&said[..], &[BACK[0], BACK[1]]].concat()));
    assert_eq!((p.phase, p.waiting), (cloud::Phase::Working, false));
    let said = [&said[..], &[BACK[0], BACK[3], BACK[4]]].concat();
    assert_eq!(cloud::session(&page(&said)).phase, cloud::Phase::Working);
}

#[test]
fn a_session_whose_turn_left_work_in_the_background_is_followed_until_it_comes_back() {
    let (d, work) = setup("background", "");
    logs(&d, &page(&[&[ASKED][..], &WAITS].concat()));
    summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    let me = at(&d, "a turn, still working", |r| r["turns"] == 1);
    assert_eq!((&me["state"], &me["detail"]), (&json!("busy"), &json!("working")), "{me}");
    assert_eq!(told(&d), [format!("☁ Aya answered: {LINK}")]);
    // Nobody says a thing: it goes on by itself, and is seen to.
    logs(&d, &page(&[&[ASKED][..], &WAITS, &BACK].concat()));
    clock(&d, T0 + 40);
    let me = at(&d, "the turn it started itself", |r| r["turns"] == 2);
    assert_eq!(me["detail"], "idle");
    assert!(out(&d.cli(&["read", "Aya"])).contains("Committed and pushed."));
    assert_eq!(told(&d).len(), 2);
}

#[test]
fn work_left_in_the_background_and_never_heard_of_again_is_taken_for_done() {
    let (d, work) = setup("background-lost", "");
    // Half an hour and more since the turn ended, the work never back.
    let t = |s: i64| jiff::Timestamp::from_second(T0 - 3600 + s).unwrap();
    let lines = [
        format!("[{}] user: go", t(0)),
        format!("[{}] tool_result: Command running in background with ID: b1.", t(5)),
        format!("[{}] result: success is_error=false turns=2 duration=9s — Waiting.", t(9)),
    ];
    logs(&d, &page(&lines.iter().map(String::as_str).collect::<Vec<_>>()));
    summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    let me = at(&d, "its turn", |r| r["turns"] == 1);
    assert_eq!((&me["state"], &me["detail"]), (&json!("resting"), &json!("idle")), "{me}");
}

#[test]
fn said_to_while_work_is_in_the_background_it_is_followed_past_the_works_return() {
    let (d, work) = setup("background-say", "");
    logs(&d, &page(&[&[ASKED][..], &WAITS].concat()));
    summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    at(&d, "a turn, still working", |r| r["turns"] == 1);
    d.cli(&["cloud", "say", "Aya", "and then this"]);
    // The work comes back first, the words still queued behind it: those are not the words.
    logs(&d, &page(&[&[ASKED][..], &WAITS, &BACK].concat()));
    clock(&d, T0 + 40);
    at(&d, "the turn it started itself", |r| r["turns"] == 2);
    let n = looks(&d);
    clock(&d, T0 + 80);
    common::wait(|| looks(&d) > n, "a look 40s on, the words still queued");
}

#[test]
fn a_cloud_session_is_followed_by_its_log_and_each_turn_is_counted_once() {
    let (d, work) = setup("follow", "");
    summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    common::wait(|| read(&d) >= 1, "a first look");
    let me = at(&d, "starting", |r| r["detail"] == "starting");
    assert_eq!(me["state"], "busy");
    // Starting, it is looked at again 40s on.
    clock(&d, T0 + 40);
    common::wait(|| read(&d) >= 2, "a second look");
    // A long turn, its first events off the page.
    logs(
        &d,
        &page(&[
            "[2026-10-19T01:48:40Z] assistant: [thinking]",
            "[2026-10-19T01:48:41Z] tool_use Bash: {}",
        ]),
    );
    clock(&d, T0 + 80);
    at(&d, "working", |r| r["detail"] == "working");
    let wait = d.command(&["wait", "Aya", "--timeout", "60s"]).spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    logs(&d, &real());
    clock(&d, T0 + 120);
    let me = at(&d, "idle", |r| r["detail"] == "idle");
    assert_eq!((&me["state"], &me["turns"]), (&json!("resting"), &json!(1)), "{me}");
    assert!(wait.wait_with_output().unwrap().status.success(), "the wait ended with the turn");
    let o = d.cli(&["read", "Aya"]);
    assert!(out(&o).contains("Pushed to branch: `claude/live-txt-test-h9ynw5`"), "{}", out(&o));
    assert_eq!(told(&d), [format!("☁ Aya answered: {LINK}")]);
    let rec = record(&d, &me["id"]);
    assert!(rec["cloud"]["ended"].is_i64(), "{rec}");

    // Idle, it is looked at seldom. (A jump of over a minute is a Mac waking: everyone is
    // looked at.)
    let n = looks(&d);
    clock(&d, T0 + 160);
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(looks(&d), n);

    // Started again, the daemon counts nothing twice.
    d.cli(&["restart"]);
    common::wait(|| looks(&d) > n, "a look after the restart");
    at(&d, "idle again", |r| r["detail"] == "idle");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!((d.list()[0]["turns"].clone(), told(&d).len()), (json!(1), 1));

    // Idle, it is not looked at at all, not even by a Mac waking a day later: on the screen, it
    // is, once a minute at most.
    let n = looks(&d);
    clock(&d, T0 + 86400);
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(looks(&d), n);
    shown(&d, &me["id"]);
    common::wait(|| looks(&d) > n, "a look once on screen");
    clock(&d, T0 + 86430);
    shown(&d, &me["id"]);
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(looks(&d), n + 1);

    // Two turns ended between looks (the user went on with it on the web): both counted, the
    // newer's answer kept.
    let two = format!(
        "{}[2026-10-19T03:00:00Z] user: again\n[2026-10-19T03:00:05Z] result: success is_error=false turns=1 duration=1s — two\n\
         [2026-10-19T03:01:00Z] user: and again\n[2026-10-19T03:01:05Z] result: error_during_execution is_error=true turns=1 duration=1s — it broke\n",
        real()
    );
    logs(&d, &two);
    clock(&d, T0 + 86500);
    shown(&d, &me["id"]);
    at(&d, "three turns", |r| r["turns"] == 3);
    assert!(out(&d.cli(&["read", "Aya"])).contains("it broke"));
    assert_eq!(told(&d).last().unwrap(), &format!("☁ Aya stopped on an error: {LINK}"));

    // A look that fails is tried again, backing off, until one gets through.
    logs(&d, "HTTP 500\n{\"error\":{\"type\":\"api_error\",\"message\":\"down\"}}\n");
    clock(&d, T0 + 86600);
    shown(&d, &me["id"]);
    let failed = || d.log().iter().any(|e| e["ev"] == "follow" && e["fails"] == 1);
    common::wait(failed, "a failed look");
    let n = looks(&d);
    clock(&d, T0 + 86660);
    std::thread::sleep(std::time::Duration::from_millis(300));
    clock(&d, T0 + 86720);
    common::wait(|| looks(&d) > n, "tried again");
}

/// The user has it on screen. No reply: the list after it says it was taken.
fn shown(d: &Daemon, id: &Value) {
    let (mut w, mut lines) = d.connect();
    let shown = json!({"t": "shown", "id": 2, "who": id});
    let list = json!({"t": "list", "id": 1});
    std::io::Write::write_all(&mut w, format!("{shown}\n{list}\n").as_bytes()).unwrap();
    common::next(&mut lines);
}

#[test]
fn a_session_quiet_for_half_an_hour_is_looked_at_seldom() {
    let (d, work) = setup("quiet", "");
    let hour_ago = jiff::Timestamp::from_second(T0 - 3600).unwrap();
    logs(&d, &page(&[ASKED, &format!("[{hour_ago}] tool_use Bash: {{}}")]));
    summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    at(&d, "working", |r| r["detail"] == "working");
    for t in [40, 80] {
        clock(&d, T0 + t);
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert_eq!(read(&d), 1, "looked at {t}s on");
    }
}

#[test]
fn said_to_a_session_is_looked_at_often_until_it_answers() {
    let (d, work) = setup("say", "");
    logs(&d, &real());
    summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    at(&d, "idle", |r| r["detail"] == "idle");
    let n = looks(&d);
    let o = d.cli(&["cloud", "say", "Aya", "more please"]);
    assert_eq!(out(&o).trim(), format!("said to {LINK}: gensokyo tells when it has answered"));
    let args = std::fs::read_to_string(d.dir.join("stub/cloud.args")).unwrap();
    assert!(args.contains("cse_01NewStub\n"), "{args}");
    common::wait(|| looks(&d) > n, "a look at once");
    // Its log still reads idle, the words queued: looked at again as a working one is.
    clock(&d, T0 + 40);
    common::wait(|| looks(&d) > n + 1, "a look 40s on");
    // The words show: still working.
    logs(&d, &format!("{}[2026-10-19T03:00:00Z] user: more please\n", real()));
    clock(&d, T0 + 80);
    at(&d, "working", |r| r["detail"] == "working");

    // Said to again while it works: the turn under way ends, and the words wait behind it, so it
    // is still looked at as a working one is.
    d.cli(&["cloud", "say", "Aya", "and then this"]);
    let n = read(&d);
    common::wait(|| read(&d) > n, "a look at once");
    logs(
        &d,
        &format!(
            "{}[2026-10-19T03:00:00Z] user: more please\n[2026-10-19T03:00:09Z] result: success is_error=false turns=1 duration=1s — done\n",
            real()
        ),
    );
    clock(&d, T0 + 120);
    at(&d, "two turns", |r| r["turns"] == 2);
    let n = read(&d);
    clock(&d, T0 + 160);
    common::wait(|| read(&d) > n, "a look 40s on, the words still queued");
}

#[test]
fn said_to_while_a_look_from_before_a_restart_is_out_it_is_still_followed() {
    let (d, work) = setup("say-restart", "");
    logs(&d, &real());
    summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    at(&d, "idle", |r| r["detail"] == "idle");
    // Started again, its first look slow: the page it brings is from before the words.
    std::fs::write(d.dir.join("stub/courier.sleep"), "2").unwrap();
    let (n, r) = (looks(&d), read(&d));
    d.cli(&["restart"]);
    common::wait(|| looks(&d) > n, "the look after the restart");
    d.cli(&["cloud", "say", "Aya", "more please"]);
    assert_eq!(read(&d), r, "said while that look was out");
    common::wait(|| read(&d) > r, "that look back");
    std::fs::remove_file(d.dir.join("stub/courier.sleep")).unwrap();
    common::wait(|| read(&d) >= r + 2, "the look the words asked for");
    // Its log still reads idle, the words queued: looked at again as a working one is.
    let n = looks(&d);
    for (t, more) in [(40, 1), (80, 2)] {
        clock(&d, T0 + t);
        common::wait(|| looks(&d) >= n + more, &format!("a look {t}s on"));
    }
}

/// The clock moved on to `to` in steps a wake would not be taken for.
fn walk(d: &Daemon, from: i64, to: i64) {
    for t in (from..=to).step_by(50).skip(1).chain([to]) {
        clock(d, t);
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

#[test]
fn a_session_quiet_after_a_day_of_work_is_looked_at_every_ten_minutes_for_a_day() {
    let (d, work) = setup("long", "");
    let ts = |t: i64| jiff::Timestamp::from_second(t).unwrap();
    logs(&d, &page(&[ASKED, &format!("[{}] tool_use Bash: {{}}", ts(T0))]));
    summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    at(&d, "working", |r| r["detail"] == "working");
    // Working a day on (a wake, so looked at), then quiet half an hour.
    let day = T0 + 86400;
    logs(&d, &page(&[ASKED, &format!("[{}] tool_use Bash: {{}}", ts(day))]));
    clock(&d, day);
    common::wait(|| looks(&d) >= 2, "a look a day on");
    clock(&d, day + 1800);
    common::wait(|| looks(&d) >= 3, "a look on the wake");
    walk(&d, day + 1800, day + 2400);
    common::wait(|| looks(&d) >= 4, "a look ten minutes on");
}

#[test]
fn a_session_gone_from_claude_ai_departs_and_a_recall_follows_it_again() {
    let (d, work) = setup("gone", "");
    logs(
        &d,
        "HTTP 404\n{\"error\": {\"message\": \"Session cse_01NewStub not found\", \"type\": \"not_found_error\"}}",
    );
    summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    at(&d, "departed", |r| r["departed"].is_i64());
    assert_eq!(told(&d), ["☁ Aya is gone from claude.ai: archived or deleted there"]);
    // Anything else is only a failed look: a 404 that is not about the session too.
    logs(&d, "HTTP 404\n{\"error\": {\"message\": \"no route\", \"type\": \"route_error\"}}");
    let o = d.cli(&["resume", "Aya"]);
    assert!(out(&o).contains("recalled Aya"), "{}", out(&o));
    let n = looks(&d);
    common::wait(|| looks(&d) > n, "a look");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(d.list()[0]["departed"].is_null());
}

#[test]
fn the_branch_a_session_pushed_to_is_found_by_its_trailer() {
    let (d, work) = setup("branch", "");
    // Made on origin itself, as the cloud pushes it: a push from `work` to a URL that is origin's
    // writes `origin/<name>` there too, since git 2.56.
    let bare = d.dir.join("remote.git");
    let push = |name: &str, link: &str| {
        let msg = format!("work\n\nClaude-Session: {link}");
        let tip = git(&bare, &["commit-tree", "main^{tree}", "-p", "main", "-m", &msg]);
        git(&bare, &["update-ref", &format!("refs/heads/{name}"), &tip]);
    };
    push("claude/other-a1b2c3", "https://claude.ai/code/session_01Other");
    push("claude/live-txt-test-h9ynw5", LINK);
    logs(&d, &real());
    let r = summon(&d, &work, json!({"name": "Aya", "prompt": "go"}));
    let me = at(&d, "the branch", |r| !r["cloud"]["branch"].is_null());
    assert_eq!(me["cloud"]["branch"], "claude/live-txt-test-h9ynw5");
    assert_eq!(record(&d, &r["resident"]["id"])["cloud"]["branch"], "claude/live-txt-test-h9ynw5");
    // Nothing of it was made here.
    assert_eq!(git(&work, &["for-each-ref", "refs/remotes/origin/claude"]), "");
}

#[test]
fn at_most_three_couriers_go_at_once() {
    let (d, work) = setup("couriers", "");
    std::fs::write(d.dir.join("stub/courier.sleep"), "4").unwrap();
    for _ in 0..4 {
        assert_eq!(summon(&d, &work, json!({"prompt": "go"}))["t"], "summoned");
    }
    common::wait(|| looks(&d) >= 3, "three couriers");
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(looks(&d), 3, "the fourth waits its turn");
    common::wait_for(std::time::Duration::from_secs(15), || looks(&d) >= 4, "the fourth");
}

/// `gensokyo wait <args>` from inside `lead`, once the daemon holds it.
fn waiting(d: &Daemon, lead: &Value, args: &[&str]) -> std::process::Child {
    let held = || d.log().iter().filter(|l| l["ev"] == "wait" && l["ids"].is_array()).count();
    let n = held();
    let mut c = d.command(&[&["wait"], args].concat());
    c.env("GENSOKYO_SOCKET", d.socket()).env("GENSOKYO_RESIDENT", lead["id"].as_str().unwrap());
    c.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let c = c.spawn().unwrap();
    common::wait(|| held() > n, "the wait held");
    c
}

/// A page of its log with `n` turns ended, the last one's answer cut by the log.
fn turns(n: usize) -> String {
    let mut lines = vec![ASKED.to_string()];
    for t in 1..=n {
        lines.push(format!("[2026-10-19T0{t}:49:00Z] user: turn {t}"));
        lines.push(format!("[2026-10-19T0{t}:49:05Z] assistant: Report {t}, long … [+512 chars]"));
        lines.push(format!(
            "[2026-10-19T0{t}:49:06Z] result: success is_error=false turns=1 duration=1s — Report {t}"
        ));
    }
    page(&lines.iter().map(String::as_str).collect::<Vec<_>>())
}

#[test]
fn a_cloud_helpers_turn_wakes_its_leads_wait_and_rings_only_when_the_lead_will_not_collect_it() {
    let (d, work) = setup_with("helper", "", &[("STUB_HOOKS", "1")]);
    let lead = local(&d, &work, "Reimu", None);
    let o = inside(&d, &lead, &["new", "--cloud", "--name", "Aya", "--prompt-file", "-"]);
    assert!(o.status.success(), "{}", err(&o));
    common::wait(|| read(&d) >= 1, "a first look");
    let bg = waiting(&d, &lead, &["Aya", "--timeout", "60s"]);
    logs(&d, &turns(1));
    clock(&d, T0 + 40);
    let o = bg.wait_with_output().unwrap();
    assert!(o.status.success(), "{}", err(&o));
    let l: Value = serde_json::from_str(out(&o).lines().next().unwrap()).unwrap();
    assert_eq!((&l["news"], &l["state"]), (&json!(true), &json!("resting")), "{l}");
    assert_eq!(l["answer"], "Report 1, long … [+512 chars]");
    // Its lead collected it: nothing rang.
    assert!(told(&d).is_empty(), "{:?}", told(&d));
    // The answer as the log cut it, and where the whole of it is.
    let o = inside(&d, &lead, &["read", "Aya"]);
    assert!(out(&o).contains("Report 1, long … [+512 chars]"), "{}", out(&o));
    assert!(
        out(&o).contains(&format!("[end of Aya's report; all of it at {LINK}]")),
        "{}",
        out(&o)
    );
    let o = inside(&d, &lead, &["read", "Aya", "--screen"]);
    assert!(err(&o).contains(&format!("Aya works on claude.ai, with no screen here: {LINK}")));

    // Its lead says more: looked at at once, and its next turn counted.
    let n = looks(&d);
    let o = inside(&d, &lead, &["cloud", "say", "Aya", "more"]);
    assert!(out(&o).contains("gensokyo tells when it has answered"), "{}", err(&o));
    common::wait(|| looks(&d) > n, "a look at once");
    // Nobody waits on it, and its lead rests: the user hears.
    logs(&d, &turns(2));
    clock(&d, T0 + 80);
    common::wait(|| told(&d).len() == 1, "the user told");
    assert_eq!(told(&d), [format!("☁ Aya answered: {LINK}")]);
    // Its lead busy: held, and rung once the lead's turn ends without it.
    let status = |s: &str| {
        let f = d.dir.join(format!("stub/{}.status", lead["id"].as_str().unwrap()));
        std::fs::write(f, s).unwrap()
    };
    status("busy");
    at_one(&d, "Reimu", |r| r["state"] == "busy");
    // Idle, it is looked at again when its lead says more.
    let more = || assert!(inside(&d, &lead, &["cloud", "say", "Aya", "more"]).status.success());
    logs(&d, &turns(3));
    more();
    at_one(&d, "Aya", |r| r["turns"] == 3);
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(told(&d).len(), 1, "held");
    status("idle");
    // Input has no reply: the list after it says it was taken.
    let (mut w, mut lines) = d.connect();
    let input = json!({"t": "input", "id": 2, "who": "Reimu", "bytes": b"carry on\r"});
    std::io::Write::write_all(
        &mut w,
        format!("{input}\n{}\n", json!({"t": "list", "id": 1})).as_bytes(),
    )
    .unwrap();
    common::next(&mut lines);
    common::wait(|| told(&d).len() == 2, "rung when its lead rests");

    // Waited on with another that has no news: held, and rung when the wait times out.
    local(&d, &work, "Patchouli", Some(&lead));
    let bg = waiting(&d, &lead, &["Aya", "Patchouli", "--timeout", "3s"]);
    logs(&d, &turns(4));
    more();
    at_one(&d, "Aya", |r| r["turns"] == 4);
    assert_eq!(told(&d).len(), 2, "held while the wait holds");
    assert_eq!(bg.wait_with_output().unwrap().status.code(), Some(3), "timed out");
    common::wait(|| told(&d).len() == 3, "rung once the wait ended without it");
}

/// Resident `name`'s line in `list`, once `f` holds for it.
fn at_one(d: &Daemon, name: &str, f: impl Fn(&Value) -> bool) -> Value {
    let one = || d.list().into_iter().find(|r| r["name"] == name);
    common::wait(|| one().is_some_and(|r| f(&r)), name);
    one().unwrap()
}

#[test]
fn a_lead_says_more_only_to_a_cloud_helper_of_its_own_that_is_still_here() {
    let (d, work) = setup_with("say-lead", "", &[("STUB_HOOKS", "1")]);
    let reimu = local(&d, &work, "Reimu", None);
    let sanae = local(&d, &work, "Sanae", None);
    let o = inside(&d, &reimu, &["new", "--cloud", "--name", "Aya", "--prompt-file", "-"]);
    assert!(o.status.success(), "{}", err(&o));
    local(&d, &work, "Patchouli", Some(&reimu));
    let said = || std::fs::read_to_string(d.dir.join("stub/cloud.args")).is_ok();
    let say = |me: &Value, who: &str| err(&inside(&d, me, &["cloud", "say", who, "more"]));
    assert!(say(&sanae, "Aya").contains("Aya is not one of your helpers"));
    assert!(say(&reimu, "Patchouli").contains("Patchouli works here, not on claude.ai"));
    // Not a session of its own: a routine's run by name or id, a session's link.
    for who in ["morning", "cse_01NewStub", LINK] {
        let e = say(&reimu, who);
        assert!(e.contains("the user says what a cloud run is told next"), "{who}: {e}");
    }
    assert!(!said(), "nothing reached claude");
    // Its log stays the user's, its own helper's too.
    let o = inside(&d, &reimu, &["cloud", "log", "Aya"]);
    assert!(!o.status.success() && err(&o).contains("stays out of residents"), "{}", err(&o));
    // Closed, it is followed no more, so it hears nothing either.
    inside(&d, &reimu, &["close", "Aya"]);
    assert!(say(&reimu, "Aya").contains("Aya is closed: gensokyo recall Aya follows it again"));
    assert!(!said());
}

/// What `lead` is told of its helpers after a compact.
fn briefed(d: &Daemon, lead: &Value) -> String {
    let hook =
        json!({"hook_event_name": "SessionStart", "session_id": lead["id"], "source": "compact"});
    let o = fed(d, lead, &["_hook"], hook.to_string().as_bytes());
    let v: Value = serde_json::from_str(&out(&o)).unwrap_or_default();
    v["hookSpecificOutput"]["additionalContext"].as_str().unwrap_or_default().to_string()
}

#[test]
fn a_lead_hears_of_its_cloud_helpers_after_a_compact_and_one_left_behind_closes_once_idle() {
    let (d, work) = setup_with("orphan", "", &[("STUB_HOOKS", "1")]);
    let lead = local(&d, &work, "Reimu", None);
    let o = inside(&d, &lead, &["new", "--cloud", "--name", "Aya", "--prompt-file", "-"]);
    assert!(o.status.success(), "{}", err(&o));
    common::wait(|| read(&d) >= 1, "a first look");
    let told = briefed(&d, &lead);
    assert!(told.contains(&format!("- Aya: working on claude.ai, {LINK}")), "{told}");
    logs(&d, &turns(1));
    clock(&d, T0 + 40);
    at_one(&d, "Aya", |r| r["turns"] == 1);
    let told = briefed(&d, &lead);
    assert!(told.contains("- Aya: resting on claude.ai"), "{told}");
    assert!(told.contains("a turn you have not read: `gensokyo read Aya`"), "{told}");

    // Its lead gone, it works on: still followed two hours on.
    logs(&d, &format!("{}[2026-10-19T02:00:00Z] user: more\n", turns(1)));
    assert!(inside(&d, &lead, &["cloud", "say", "Aya", "more"]).status.success());
    at_one(&d, "Aya", |r| r["detail"] == "working");
    d.cli(&["close", "Reimu"]);
    at_one(&d, "Reimu", |r| r["state"] == "departed");
    let settle = || std::thread::sleep(std::time::Duration::from_millis(600));
    settle();
    clock(&d, T0 + 2 * 3600 + 120);
    settle();
    assert!(at_one(&d, "Aya", |_| true)["departed"].is_null(), "working, so kept");
    // Its turn over, two idle hours later it is closed: in recall, still its lead's.
    logs(&d, &turns(2));
    clock(&d, T0 + 2 * 3600 + 200);
    at_one(&d, "Aya", |r| r["detail"] == "idle");
    // Seen idle by a tick before the clock moves on: the two hours count from there.
    settle();
    clock(&d, T0 + 4 * 3600 + 200);
    common::wait(|| d.list().iter().all(|r| r["name"] != "Aya"), "closed");
    // The user hears where it goes on: they never started it.
    let kept = kept(&d);
    assert!(kept.contains(&format!("☁ Aya closed, its lead gone: the session goes on at {LINK}")));
    let all = d.req(json!({"t": "list", "id": 1, "all": true}))["residents"].clone();
    let aya = all.as_array().unwrap().iter().find(|r| r["name"] == "Aya").cloned().unwrap();
    assert_eq!(aya["owner"], lead["id"]);
}

#[test]
fn a_helper_gone_from_claude_ai_and_left_behind_closes_without_saying_it_goes_on() {
    let (d, work) = setup_with("orphan-gone", "", &[("STUB_HOOKS", "1")]);
    let lead = local(&d, &work, "Reimu", None);
    let o = inside(&d, &lead, &["new", "--cloud", "--name", "Aya", "--prompt-file", "-"]);
    assert!(o.status.success(), "{}", err(&o));
    logs(
        &d,
        "HTTP 404\n{\"error\": {\"message\": \"Session cse_01NewStub not found\", \"type\": \"not_found_error\"}}",
    );
    clock(&d, T0 + 40);
    at_one(&d, "Aya", |r| !r["departed"].is_null());
    d.cli(&["close", "Reimu"]);
    at_one(&d, "Reimu", |r| r["state"] == "departed");
    std::thread::sleep(std::time::Duration::from_millis(600));
    clock(&d, T0 + 2 * 3600 + 120);
    common::wait(|| d.list().iter().all(|r| r["name"] != "Aya"), "closed");
    let kept = kept(&d);
    assert!(kept.contains("☁ Aya is gone from claude.ai") && !kept.contains("goes on"), "{kept}");
}

/// The notices the daemon keeps, as a client watching is given them.
fn kept(d: &Daemon) -> String {
    let (mut w, mut lines) = d.connect();
    std::io::Write::write_all(&mut w, format!("{}\n", json!({"t": "watch"})).as_bytes()).unwrap();
    loop {
        let v = common::next(&mut lines);
        if v["t"] == "notices" {
            break v["notices"].to_string();
        }
    }
}
