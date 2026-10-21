//! Which project a directory is in, for its ledger: real repositories, made with git.

mod common;

use common::fresh;
use gensokyo::ledger::root;
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(["-c", "protocol.file.allow=always", "-c", "init.defaultBranch=main"])
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_project_is_its_repository_whichever_checkout_of_it_you_are_in() {
    let d = std::fs::canonicalize(fresh("ledger-root")).unwrap();
    let repo = |p: &Path| {
        std::fs::create_dir_all(p).unwrap();
        git(p, &["init", "-q"]);
        git(p, &["commit", "-q", "--allow-empty", "-m", "one"]);
    };
    let proj = d.join("proj");
    repo(&proj);
    std::fs::create_dir_all(proj.join("src/deep")).unwrap();
    assert_eq!(root(&proj.join("src/deep")), proj);
    // A worktree anywhere is the same project.
    git(&proj, &["worktree", "add", "-q", d.join("wt").to_str().unwrap()]);
    assert_eq!(root(&d.join("wt")), proj);

    // Two bare repositories side by side are two projects, each with its worktrees.
    for name in ["a", "b"] {
        git(&d, &["clone", "-q", "--bare", proj.to_str().unwrap(), &format!("{name}.git")]);
        let tree = d.join(format!("{name}-wt"));
        git(&d.join(format!("{name}.git")), &["worktree", "add", "-q", tree.to_str().unwrap()]);
        assert_eq!(root(&tree), d.join(format!("{name}.git")));
    }

    // A submodule is a project of its own, and its worktree is in it.
    let sup = d.join("super");
    repo(&sup);
    git(&sup, &["submodule", "add", "-q", proj.to_str().unwrap(), "sub"]);
    let sub = root(&sup.join("sub"));
    assert_ne!(sub, sup);
    git(&sup.join("sub"), &["worktree", "add", "-q", d.join("sub-wt").to_str().unwrap()]);
    assert_eq!(root(&d.join("sub-wt")), sub);

    // Outside any repository, the directory itself.
    let plain = std::env::temp_dir().join(format!("gensokyo-ledger-{}", std::process::id()));
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(root(&plain), plain);
    std::fs::remove_dir_all(&plain).unwrap();
}
