//! A resident in a checkout of its own: `<repo>/.claude/worktrees/<slug>`, where Claude Code
//! puts its own, made with `git worktree add`. Not `claude --worktree`, whose `/exit` asks
//! whether to keep or remove the worktree, a dialog every `close` and `quit` would stop at.
//! Each git step runs as a child of its own with a time limit: a checkout of a large repo
//! takes seconds, and the daemon has one thread. Nothing here removes a worktree or a branch.

use crate::frontmatter::{NAME_RULE, name_ok};
use crate::paths::short;
use crate::proto::WorktreeAsk;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// A fetch or `ls-remote`, which may wait on the network.
const NET: Duration = Duration::from_secs(30);
/// Anything else, a checkout included.
const LOCAL: Duration = Duration::from_secs(300);

/// Where it is, and what the user should know about how it came to be.
pub(super) struct Made {
    /// The worktree, or the same subdirectory of it as `dir` is of its repository.
    pub(super) path: String,
    pub(super) note: Option<String>,
}

impl Made {
    /// Inside the worktree at `wt`, where `dir` is inside `top`.
    fn at(wt: &Path, top: &Path, dir: &Path, note: Option<String>) -> Made {
        let dir = std::fs::canonicalize(dir).unwrap_or(dir.into());
        let sub = dir.strip_prefix(top).ok().filter(|s| wt.join(s).is_dir());
        let path = sub.map_or(wt.to_path_buf(), |s| wt.join(s));
        Made { path: path.to_string_lossy().into_owned(), note }
    }
}

/// `git -C dir args`: whether it worked, and its stdout, else its stderr's first line.
async fn git(dir: &Path, args: &[&str], limit: Duration) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    // A fetch never asks for a password: there is nobody at this terminal.
    cmd.env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(limit, cmd.output()).await;
    let out = out.map_err(|_| format!("git {} took over {}s", args[0], limit.as_secs()))?;
    let out = out.map_err(|e| format!("git could not run: {e}"))?;
    match out.status.success() {
        true => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
        false => {
            let err = String::from_utf8_lossy(&out.stderr);
            let line = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("it failed");
            Err(crate::tele::clean(line, 200))
        }
    }
}

fn net() -> Duration {
    let ms = std::env::var("GENSOKYO_FETCH_LIMIT_MS").ok().and_then(|v| v.parse().ok());
    ms.map_or(NET, Duration::from_millis)
}

/// Each worktree of the repo: its path and its branch, if it is on one.
fn worktrees(porcelain: &str) -> Vec<(PathBuf, Option<String>)> {
    let mut v: Vec<(PathBuf, Option<String>)> = Vec::new();
    for l in porcelain.lines() {
        if let Some(p) = l.strip_prefix("worktree ") {
            v.push((p.into(), None));
        } else if let (Some(b), Some(last)) = (l.strip_prefix("branch refs/heads/"), v.last_mut()) {
            last.1 = Some(b.into());
        }
    }
    v
}

/// The worktree `ask` names, in the repo holding `dir`: found, or made.
pub(super) async fn make(dir: &Path, ask: &WorktreeAsk, prefix: &str) -> Result<Made, String> {
    let slug = ask.slug.as_str();
    if !name_ok(slug) {
        return Err(format!("--worktree {slug}: a worktree's name is {NAME_RULE}"));
    }
    let not_repo = |_| format!("{} is not in a git repository", short(&dir.to_string_lossy()));
    let out = git(dir, &["rev-parse", "--show-toplevel", "--git-common-dir"], LOCAL).await;
    let out = out.map_err(not_repo)?;
    let mut lines = out.lines();
    let top = PathBuf::from(lines.next().unwrap_or_default());
    let common = dir.join(lines.next().unwrap_or(".git"));
    // Claude Code refuses these too: a link would put the checkout somewhere else.
    for d in [top.join(".claude"), top.join(".claude/worktrees")] {
        if d.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "{} is a link; worktrees go in a real directory",
                short(&d.to_string_lossy())
            ));
        }
    }
    let path = top.join(".claude/worktrees").join(slug);
    let shown = short(&path.to_string_lossy());
    let all = worktrees(&git(&top, &["worktree", "list", "--porcelain"], LOCAL).await?);
    if path.exists() {
        let real = std::fs::canonicalize(&path).unwrap_or(path.clone());
        let found = all.iter().find(|(p, _)| std::fs::canonicalize(p).unwrap_or(p.clone()) == real);
        let Some((_, on)) = found else {
            return Err(format!("{shown} is there and is not a worktree of this repository"));
        };
        let on = on.clone().unwrap_or_else(|| "a detached HEAD".into());
        if ask.branch.as_ref().is_some_and(|b| *b != on) {
            return Err(format!("{shown} is on {on}, not {}", ask.branch.as_deref().unwrap_or("")));
        }
        return Ok(Made::at(&path, &top, dir, Some(format!("reused {shown}, on {on}"))));
    }
    let branch = ask.branch.clone().unwrap_or_else(|| format!("{prefix}{slug}"));
    if let Some((p, _)) = all.iter().find(|(_, b)| b.as_deref() == Some(branch.as_str())) {
        return Err(format!(
            "{branch} is checked out in {} already: summon there instead",
            short(&p.to_string_lossy())
        ));
    }
    exclude(&top, &common, slug).await;
    let p = path.to_string_lossy().into_owned();
    let mut note = None;
    let refname = format!("refs/heads/{branch}");
    if git(&top, &["rev-parse", "--verify", "--quiet", &refname], LOCAL).await.is_ok() {
        git(&top, &["worktree", "add", &p, &branch], LOCAL).await?;
        return Ok(Made::at(&path, &top, dir, note));
    }
    let origin = git(&top, &["remote", "get-url", "origin"], LOCAL).await.is_ok();
    // Out of reach, it is not asked again: the base is the last fetched, and the note says so.
    let mut unreachable = None;
    // One the remote has: a branch to review, or to carry on. Fetched alone: a fetch naming a
    // ref the remote lacks fetches nothing at all.
    if origin {
        match git(&top, &["ls-remote", "--heads", "origin", &branch], net()).await {
            Ok(heads) if !heads.is_empty() => {
                git(&top, &["fetch", "origin", &branch], net()).await?;
                let theirs = format!("origin/{branch}");
                git(&top, &["worktree", "add", "--track", "-b", &branch, &p, &theirs], LOCAL)
                    .await?;
                return Ok(Made::at(&path, &top, dir, note));
            }
            Ok(_) => {}
            Err(e) => unreachable = Some(e),
        }
    }
    let base = match &ask.base {
        Some(b) => b.clone(),
        None if origin => {
            let head =
                git(&top, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], LOCAL).await;
            match head {
                Ok(h) => {
                    let name = h.strip_prefix("origin/").unwrap_or(&h);
                    let failed = match unreachable {
                        Some(e) => Some(format!("origin could not be reached ({e})")),
                        None => git(&top, &["fetch", "origin", name], net())
                            .await
                            .err()
                            .map(|e| format!("the fetch failed ({e})")),
                    };
                    if let Some(why) = failed {
                        note = Some(format!("started from {h} as last fetched: {why}"));
                    }
                    h
                }
                Err(_) => {
                    note = Some("started from HEAD: origin names no default branch (git remote set-head origin --auto)".into());
                    "HEAD".into()
                }
            }
        }
        None => {
            note = Some("started from HEAD: the repository has no origin".into());
            "HEAD".into()
        }
    };
    git(&top, &["worktree", "add", "--no-track", "-b", &branch, &p, &base], LOCAL).await?;
    Ok(Made::at(&path, &top, dir, note))
}

/// `.claude/worktrees/` kept out of `git status`, in the repo's own exclude file, unless
/// something already ignores it.
async fn exclude(top: &Path, common: &Path, slug: &str) {
    let probe = format!(".claude/worktrees/{slug}");
    if git(top, &["check-ignore", "-q", &probe], LOCAL).await.is_ok() {
        return;
    }
    let file = common.join("info/exclude");
    let _ = std::fs::create_dir_all(common.join("info"));
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("/.claude/worktrees/\n");
    let _ = std::fs::write(&file, text);
}
