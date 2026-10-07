//! `new --worktree`: a summon into `<repo>/.claude/worktrees/<slug>`, against real git, with a
//! bare repository beside it as its origin.

mod common;

use common::{Daemon, fresh};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// A daemon, and a repo `work` on main whose origin `remote.git` is one commit ahead of what
/// `work` last fetched.
fn setup(name: &str, prefix: &str) -> (Daemon, PathBuf) {
    // Git never looks above the test's own directory: it sits inside this repository, where a
    // summon outside `work` would otherwise make a worktree.
    let dir = fresh(&format!("w-{name}"));
    let env = vec![
        ("GENSOKYO_FETCH_LIMIT_MS".into(), "5000".into()),
        ("GIT_CEILING_DIRECTORIES".into(), dir.display().to_string()),
    ];
    let d = Daemon::at(dir, env);
    std::fs::create_dir_all(d.dir.join("conf")).unwrap();
    std::fs::write(d.dir.join("conf/config"), format!("BRANCH_PREFIX={prefix}\n")).unwrap();
    let work = d.dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&d.dir, &["init", "-q", "--bare", "remote.git"]);
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
    git(&work, &["remote", "add", "origin", "../remote.git"]);
    git(&work, &["push", "-q", "origin", "main"]);
    git(&work, &["remote", "set-head", "origin", "main"]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "two"]);
    git(&work, &["push", "-q", "origin", "main"]);
    git(&work, &["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
    git(&work, &["reset", "-q", "--hard", "HEAD~1"]);
    d.cli(&["list"]);
    (d, std::fs::canonicalize(&work).unwrap())
}

fn summon(d: &Daemon, cwd: &Path, extra: Value) -> Value {
    let mut req = json!({"t": "summon", "id": 7, "cwd": cwd});
    req.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    d.req(req)
}

#[test]
fn a_worktree_starts_from_the_fetched_default_and_is_reused_by_its_name() {
    let (d, work) = setup("new", "me/");
    let tip = git(&d.dir.join("remote.git"), &["rev-parse", "main"]);
    let r = summon(&d, &work, json!({"worktree": {"slug": "fix"}}));
    assert_eq!(r["t"], "summoned", "{r}");
    let wt = work.join(".claude/worktrees/fix");
    assert_eq!(r["resident"]["cwd"], json!(wt));
    assert_eq!(r.get("note"), None, "{r}");
    assert_eq!(git(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]), "me/fix");
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), tip, "from the fetched default");
    let up = Command::new("git").arg("-C").arg(&wt).args(["rev-parse", "@{u}"]).output().unwrap();
    assert!(!up.status.success(), "a new branch tracks nothing");
    assert!(git(&work, &["status", "--porcelain"]).is_empty(), "the worktrees are excluded");

    // Its name again: the same worktree. Another branch there is refused.
    let again = summon(&d, &work, json!({"name": "Youmu", "worktree": {"slug": "fix"}}));
    assert_eq!(again["resident"]["cwd"], json!(wt));
    let note = again["note"].as_str().unwrap();
    assert!(note.starts_with("reused ") && note.ends_with("worktrees/fix, on me/fix"), "{note}");
    let other = summon(&d, &work, json!({"worktree": {"slug": "fix", "branch": "else"}}));
    assert!(other["error"].as_str().unwrap().contains("is on me/fix, not else"), "{other}");

    // A second worktree writes no second exclude line; main, held by the checkout, is refused.
    summon(&d, &work, json!({"worktree": {"slug": "two"}}));
    let exclude = std::fs::read_to_string(work.join(".git/info/exclude")).unwrap();
    assert_eq!(exclude.matches("/.claude/worktrees/").count(), 1, "{exclude}");
    let held = summon(&d, &work, json!({"worktree": {"slug": "m", "branch": "main"}}));
    assert!(held["error"].as_str().unwrap().contains("main is checked out in"), "{held}");
}

#[test]
fn a_gitignore_that_shows_the_worktrees_gets_one_exclude_line_all_the_same() {
    let (d, work) = setup("shown", "me/");
    std::fs::write(work.join(".gitignore"), "!/.claude/worktrees/\n").unwrap();
    for slug in ["one", "two"] {
        let r = summon(&d, &work, json!({"worktree": {"slug": slug}}));
        assert_eq!(r["t"], "summoned", "{r}");
    }
    let exclude = std::fs::read_to_string(work.join(".git/info/exclude")).unwrap();
    assert_eq!(exclude.matches("/.claude/worktrees/").count(), 1, "{exclude}");
}

#[test]
fn a_branch_only_the_remote_has_is_tracked() {
    let (d, work) = setup("remote", "");
    git(
        &work,
        &["push", "-q", "origin", "HEAD:refs/heads/review-me", "HEAD:refs/heads/x/review-me"],
    );
    // A clone that fetches only main, as a single-branch one does.
    git(&work, &["config", "remote.origin.fetch", "+refs/heads/main:refs/remotes/origin/main"]);
    let r = summon(&d, &work, json!({"worktree": {"slug": "rev", "branch": "review-me"}}));
    assert_eq!(r["t"], "summoned", "{r}");
    let wt = work.join(".claude/worktrees/rev");
    assert_eq!(git(&wt, &["rev-parse", "--abbrev-ref", "@{u}"]), "origin/review-me");
}

#[test]
fn an_unreachable_or_missing_origin_falls_back_and_says_so() {
    let (d, work) = setup("offline", "");
    let last = git(&work, &["rev-parse", "origin/main"]);
    std::fs::rename(d.dir.join("remote.git"), d.dir.join("gone.git")).unwrap();
    let r = summon(&d, &work, json!({"worktree": {"slug": "off"}}));
    assert_eq!(r["t"], "summoned", "{r}");
    let note = r["note"].as_str().unwrap();
    assert!(note.contains("as last fetched: origin could not be reached"), "{note}");
    assert_eq!(git(&work.join(".claude/worktrees/off"), &["rev-parse", "HEAD"]), last);

    git(&work, &["remote", "remove", "origin"]);
    let r = summon(&d, &work, json!({"worktree": {"slug": "local"}}));
    assert!(r["note"].as_str().unwrap().contains("no origin"), "{r}");
}

#[test]
fn a_refused_summon_makes_no_worktree_and_odd_places_are_refused() {
    let (d, work) = setup("refused", "");
    summon(&d, &work, json!({"name": "Sakuya"}));
    let r = summon(&d, &work, json!({"name": "Sakuya", "worktree": {"slug": "nope"}}));
    assert!(r["error"].as_str().unwrap().contains("Sakuya is already here"), "{r}");
    assert!(!work.join(".claude/worktrees/nope").exists());

    let r = summon(&d, &d.dir.join("conf"), json!({"worktree": {"slug": "x"}}));
    assert!(r["error"].as_str().unwrap().contains("is not in a git repository"), "{r}");
    let r = summon(&d, &work, json!({"worktree": {"slug": "../up"}}));
    assert!(r["error"].as_str().unwrap().contains("a worktree's name is"), "{r}");

    std::fs::create_dir_all(d.dir.join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(d.dir.join("elsewhere"), work.join(".claude")).unwrap();
    let r = summon(&d, &work, json!({"worktree": {"slug": "linked"}}));
    assert!(r["error"].as_str().unwrap().contains("is a link"), "{r}");
}

#[test]
fn a_ritual_runs_in_its_worktree_in_the_same_subdirectory() {
    let (d, work) = setup("ritual", "");
    std::fs::create_dir_all(work.join("app")).unwrap();
    std::fs::write(work.join("app/f"), "x").unwrap();
    git(&work, &["add", "app/f"]);
    git(&work, &["commit", "-q", "-m", "app"]);
    git(&work, &["push", "-q", "-f", "origin", "HEAD:main"]);
    let text = format!(
        "---\nschedule: \"0 3 * * *\"\ncwd: {}\nworktree: nightly\n---\nTidy up.\n",
        work.join("app").display()
    );
    std::fs::create_dir_all(d.dir.join("conf/rituals")).unwrap();
    std::fs::write(d.dir.join("conf/rituals/nightly.md"), text).unwrap();
    let r = d.req(json!({"t": "ritual", "id": 3, "verb": "run", "name": "nightly"}));
    assert!(
        r["message"].as_str().unwrap_or_default().contains("making its worktree nightly"),
        "{r}"
    );
    let want = json!(work.join(".claude/worktrees/nightly/app"));
    common::wait(|| d.list().iter().any(|r| r["cwd"] == want), "the run in its worktree");
    let branch =
        git(&work.join(".claude/worktrees/nightly"), &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(branch, "nightly");
    // The exclude line went to the repository's own .git, not one beside the subdirectory.
    assert!(!work.join("app/.git").exists());
    let exclude = std::fs::read_to_string(work.join(".git/info/exclude")).unwrap();
    assert!(exclude.contains("/.claude/worktrees/"), "{exclude}");
}

#[test]
fn names_that_git_would_read_as_options_are_refused() {
    let (d, work) = setup("names", "");
    // Run by a fetch from a local origin, were it read as an option.
    let evil = "--upload-pack=touch${IFS}PWNED;git-upload-pack";
    let r = summon(&d, &work, json!({"worktree": {"slug": "a", "branch": evil}}));
    assert!(r["error"].as_str().unwrap().contains("cannot be a branch name"), "{r}");
    let r = summon(&d, &work, json!({"worktree": {"slug": "b", "branch": "a b"}}));
    assert!(r["error"].as_str().unwrap().contains("a b cannot be a branch name"), "{r}");
    let r = summon(&d, &work, json!({"worktree": {"slug": "c", "base": "--lock"}}));
    assert!(r["error"].as_str().unwrap().contains("--base --lock: no such commit"), "{r}");
    // The default branch is the remote's to name.
    let theirs = format!("refs/remotes/origin/{evil}");
    git(&work, &["update-ref", &theirs, "HEAD"]);
    git(&work, &["symbolic-ref", "refs/remotes/origin/HEAD", &theirs]);
    let r = summon(&d, &work, json!({"worktree": {"slug": "e"}}));
    assert!(r["error"].as_str().unwrap().contains("origin's default"), "{r}");
    for dir in [&work, &d.dir, &d.dir.join("remote.git")] {
        assert!(!dir.join("PWNED").exists(), "ran in {}", dir.display());
    }
}

#[test]
fn worktrees_go_under_the_main_checkout_and_broken_ones_are_refused() {
    let (d, work) = setup("broken", "");
    let r = summon(&d, &work, json!({"worktree": {"slug": "a"}}));
    assert_eq!(r["t"], "summoned", "{r}");
    // From inside a worktree: beside it, not in it.
    let r = summon(&d, &work.join(".claude/worktrees/a"), json!({"worktree": {"slug": "b"}}));
    assert_eq!(r["resident"]["cwd"], json!(work.join(".claude/worktrees/b")), "{r}");

    // Its directory removed by hand, and still registered.
    std::fs::remove_dir_all(work.join(".claude/worktrees/b")).unwrap();
    for ask in [json!({"slug": "b"}), json!({"slug": "c", "branch": "b"})] {
        let r = summon(&d, &work, json!({"worktree": ask}));
        let e = r["error"].as_str().unwrap();
        assert!(e.contains("worktrees/b is registered but its directory is gone"), "{r}");
    }

    // Cut off while git made it: still locked as it was while being made.
    let half = work.join(".claude/worktrees/half");
    let args = ["worktree", "add", "-q", "--lock", "--reason", "initializing", "-b", "half"];
    git(&work, &[&args[..], &[half.to_str().unwrap()]].concat());
    let r = summon(&d, &work, json!({"worktree": {"slug": "half"}}));
    assert!(r["error"].as_str().unwrap().contains("its checkout was cut off"), "{r}");
}
