//! A resident in a checkout of its own: `<repo>/.claude/worktrees/<slug>`, where Claude Code
//! puts its own, made with `git worktree add`. Not `claude --worktree`, whose `/exit` asks
//! whether to keep or remove the worktree, a dialog every `close` and `quit` would stop at.
//! Each git step runs as a child of its own with a time limit: a checkout of a large repo
//! takes seconds, and the daemon has one thread. Nothing here removes a worktree or a branch.
//! A branch or base can come from a resident, so none reaches git where it could be read as
//! an option: names are checked, and refs follow `--end-of-options`.

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
    /// The worktree, or the same subdirectory of it as `dir` is of its checkout.
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

/// One worktree of the repo, from `git worktree list --porcelain`.
#[derive(Default)]
struct Tree {
    path: PathBuf,
    /// Its branch, if it is on one.
    branch: Option<String>,
    bare: bool,
    /// Locked with this reason: `initializing` while `git worktree add` makes it, and after,
    /// when that was cut off.
    locked: Option<String>,
    /// Registered, and its directory gone.
    prunable: bool,
}

fn worktrees(porcelain: &str) -> Vec<Tree> {
    let mut v: Vec<Tree> = Vec::new();
    for l in porcelain.lines() {
        if let Some(p) = l.strip_prefix("worktree ") {
            v.push(Tree { path: p.into(), ..Default::default() });
            continue;
        }
        let Some(t) = v.last_mut() else { continue };
        let (word, rest) = l.split_once(' ').unwrap_or((l, ""));
        match word {
            "branch" => t.branch = rest.strip_prefix("refs/heads/").map(Into::into),
            "bare" => t.bare = true,
            "locked" => t.locked = Some(rest.into()),
            "prunable" => t.prunable = true,
            _ => {}
        }
    }
    v
}

/// `b` as git takes a new branch's name, or why not. `@{-1}` and the like are refused, not
/// expanded.
async fn branch_ok(top: &Path, b: &str) -> Result<(), String> {
    match git(top, &["check-ref-format", "--branch", b], LOCAL).await {
        Ok(out) if out == b => Ok(()),
        _ => Err(format!("{b} cannot be a branch name")),
    }
}

/// Origin's branch `b` alone, into `origin/<b>` even where origin's refspec names fewer
/// branches. A fetch naming a ref the remote lacks fetches nothing at all.
async fn fetch(top: &Path, b: &str) -> Result<String, String> {
    let spec = format!("+refs/heads/{b}:refs/remotes/origin/{b}");
    git(top, &["fetch", "-q", "origin", "--end-of-options", &spec], net()).await
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
    let here = PathBuf::from(lines.next().unwrap_or_default());
    let common = dir.join(lines.next().unwrap_or(".git"));
    let all = worktrees(&git(&here, &["worktree", "list", "--porcelain"], LOCAL).await?);
    // Under the main checkout, as Claude Code puts its own, even from inside another
    // worktree: one nested in another goes when that one is removed.
    let top = all.first().filter(|t| !t.bare).map_or(here.clone(), |t| t.path.clone());
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
    let real = std::fs::canonicalize(&path).unwrap_or(path.clone());
    let found = all.iter().find(|t| t.path == path || t.path == real);
    let gone = |p: &Path| {
        let p = short(&p.to_string_lossy());
        format!("{p} is registered but its directory is gone: `git worktree prune`, then again")
    };
    match found {
        Some(t) if t.prunable => return Err(gone(&t.path)),
        Some(t) if t.locked.as_deref() == Some("initializing") => {
            return Err(format!(
                "{shown} is being made, or its checkout was cut off: if nothing is making it, \
                 `git worktree remove --force {shown}`, then again"
            ));
        }
        Some(t) => {
            let on = t.branch.clone().unwrap_or_else(|| "a detached HEAD".into());
            if ask.branch.as_ref().is_some_and(|b| *b != on) {
                let b = ask.branch.as_deref().unwrap_or("");
                return Err(format!("{shown} is on {on}, not {b}"));
            }
            return Ok(Made::at(&path, &here, dir, Some(format!("reused {shown}, on {on}"))));
        }
        None if path.exists() => {
            return Err(format!("{shown} is there and is not a worktree of this repository"));
        }
        None => {}
    }
    let branch = ask.branch.clone().unwrap_or_else(|| format!("{prefix}{slug}"));
    branch_ok(&top, &branch).await?;
    if let Some(t) = all.iter().find(|t| t.branch.as_deref() == Some(branch.as_str())) {
        if t.prunable {
            return Err(gone(&t.path));
        }
        return Err(format!(
            "{branch} is checked out in {} already: summon there instead",
            short(&t.path.to_string_lossy())
        ));
    }
    exclude(&top, &common, slug).await;
    let p = path.to_string_lossy().into_owned();
    let mut note = None;
    let refname = format!("refs/heads/{branch}");
    if git(&top, &["rev-parse", "--verify", "--quiet", &refname], LOCAL).await.is_ok() {
        git(&top, &["worktree", "add", "-q", &p, &branch], LOCAL).await?;
        return Ok(Made::at(&path, &here, dir, note));
    }
    let origin = git(&top, &["remote", "get-url", "origin"], LOCAL).await.is_ok();
    // Out of reach, it is not asked again: the base is the last fetched, and the note says so.
    let mut unreachable = None;
    // One the remote has: a branch to review, or to carry on.
    if origin {
        match git(&top, &["ls-remote", "--heads", "origin", &refname], net()).await {
            Ok(heads) if !heads.is_empty() => {
                fetch(&top, &branch).await?;
                let theirs = format!("origin/{branch}");
                let args = ["worktree", "add", "-q", "--track", "-b", &branch, &p, &theirs];
                if git(&top, &args, LOCAL).await.is_err() {
                    // A clone that fetches only some branches (single-branch) cannot track
                    // one outside them, until origin's refspec names it too.
                    let add = ["remote", "set-branches", "--add", "origin", &branch];
                    git(&top, &add, LOCAL).await?;
                    git(&top, &args, LOCAL).await?;
                }
                return Ok(Made::at(&path, &here, dir, note));
            }
            Ok(_) => {}
            Err(e) => unreachable = Some(e),
        }
    }
    let base = match &ask.base {
        Some(b) => {
            let commit = format!("{b}^{{commit}}");
            let args = ["rev-parse", "--verify", "--quiet", "--end-of-options", &commit];
            git(&top, &args, LOCAL).await.map_err(|_| format!("--base {b}: no such commit"))?
        }
        None if origin => {
            let head =
                git(&top, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], LOCAL).await;
            match head {
                Ok(h) => {
                    // Named by the remote: checked as any branch name is.
                    let name = h.strip_prefix("origin/").unwrap_or(&h).to_string();
                    branch_ok(&top, &name).await.map_err(|e| format!("origin's default: {e}"))?;
                    let failed = match unreachable {
                        Some(e) => Some(format!("origin could not be reached ({e})")),
                        None => fetch(&top, &name)
                            .await
                            .err()
                            .map(|e| format!("the fetch failed ({e})")),
                    };
                    if let Some(why) = failed {
                        note = Some(format!("started from {h} as last fetched: {why}"));
                    }
                    format!("refs/remotes/origin/{name}")
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
    git(&top, &["worktree", "add", "-q", "--no-track", "-b", &branch, &p, &base], LOCAL).await?;
    Ok(Made::at(&path, &here, dir, note))
}

/// The GitHub repository a cloud session started in `dir` would clone, `owner/name`, from
/// origin's URL as set.
pub(super) async fn github(dir: &Path) -> Result<String, String> {
    let shown = short(&dir.to_string_lossy());
    let not_repo = |_| format!("{shown} is not in a git repository");
    git(dir, &["rev-parse", "--show-toplevel"], LOCAL).await.map_err(not_repo)?;
    let url = git(dir, &["config", "--get", "remote.origin.url"], LOCAL).await;
    let url =
        url.map_err(|_| format!("{shown} has no origin: a cloud session clones from GitHub"))?;
    crate::cloud::github(&url)
}

/// The branch `dir` is on, pushed to origin and tracked there.
pub(super) async fn push(dir: &Path) -> Result<(), String> {
    let b = on(dir).await?;
    let args = ["push", "-q", "-u", "origin", "--end-of-options", &b];
    git(dir, &args, LOCAL).await.map(drop).map_err(|e| format!("git push: {e}"))
}

/// The branch `dir` is on, when origin has it just as it is here: a cloud session starts from
/// origin's, where commits not pushed are not. Asked where pushes go, which reads the same
/// repository and may be the only way in (an SSH host alias).
pub(super) async fn pushed(dir: &Path) -> Result<String, String> {
    let b = on(dir).await?;
    let head = git(dir, &["rev-parse", "HEAD"], LOCAL).await?;
    let url = git(dir, &["remote", "get-url", "--push", "origin"], LOCAL).await?;
    let refname = format!("refs/heads/{b}");
    let heads = git(dir, &["ls-remote", "--end-of-options", &url, &refname], net()).await;
    let heads = heads.map_err(|e| format!("origin could not be reached ({e})"))?;
    let theirs = heads.lines().find_map(|l| l.split_once('\t').filter(|(_, r)| *r == refname));
    match theirs {
        None => Err(format!("{b} is not on origin: push it first (git push -u origin {b})")),
        Some((h, _)) if h == head => Ok(b),
        Some(_) => Err(format!(
            "origin's {b} is not where yours is: push or pull first, since a cloud session \
             starts from origin's"
        )),
    }
}

/// The branch on origin a cloud session pushed its work to: a `claude/…` one whose newest commit
/// names the session's `link` (Claude Code's `Claude-Session:` trailer). Tips in `checked` were
/// looked at already: only the others are fetched, from where pushes go, into no ref here. What
/// was found, and the tips looked at now.
pub(super) async fn claude_branch(
    dir: &Path,
    link: &str,
    checked: &[String],
) -> Result<(Option<String>, Vec<String>), String> {
    let url = git(dir, &["remote", "get-url", "--push", "origin"], LOCAL).await?;
    let ls = ["ls-remote", "--end-of-options", &url, "refs/heads/claude/*"];
    let heads = git(dir, &ls, net()).await?;
    let heads: Vec<(&str, &str)> = heads
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(h, r)| Some((h, r.strip_prefix("refs/heads/")?)))
        .filter(|(h, r)| h.bytes().all(|b| b.is_ascii_hexdigit()) && plain_ref(r))
        .filter(|(h, _)| !checked.iter().any(|c| c == h))
        .collect();
    if heads.is_empty() {
        return Ok((None, Vec::new()));
    }
    let refs: Vec<String> = heads.iter().map(|(_, r)| format!("refs/heads/{r}")).collect();
    let mut fetch =
        vec!["fetch", "-q", "--no-tags", "--no-write-fetch-head", "--end-of-options", &url];
    fetch.extend(refs.iter().map(String::as_str));
    git(dir, &fetch, net()).await?;
    let fmt = "--format=%H%x00%(trailers:key=Claude-Session,valueonly)%x01";
    let mut log = vec!["log", "--no-walk=unsorted", fmt, "--end-of-options"];
    log.extend(heads.iter().map(|(h, _)| *h));
    let named = git(dir, &log, LOCAL).await?;
    let mine = named.split('\x01').filter_map(|c| c.trim().split_once('\0'));
    let mine: Vec<&str> =
        mine.filter(|(_, t)| t.lines().any(|l| l.trim() == link)).map(|(h, _)| h).collect();
    let found = heads.iter().find(|(h, _)| mine.contains(h)).map(|(_, r)| r.to_string());
    Ok((found, heads.iter().map(|(h, _)| h.to_string()).collect()))
}

/// A branch's name with nothing in it a terminal or a shell would act on.
fn plain_ref(r: &str) -> bool {
    let ok = |b: u8| b.is_ascii_alphanumeric() || b"._/-".contains(&b);
    r.len() <= 200 && !r.starts_with('-') && !r.contains("..") && r.bytes().all(ok)
}

/// The branch `dir` is on.
async fn on(dir: &Path) -> Result<String, String> {
    let b = git(dir, &["symbolic-ref", "--short", "-q", "HEAD"], LOCAL).await;
    b.map_err(|_| {
        let shown = short(&dir.to_string_lossy());
        format!("{shown} is not on a branch: a cloud session starts from a pushed one")
    })
}

/// `.claude/worktrees/` kept out of `git status`, in the repo's own exclude file, unless
/// something already ignores it. Never twice: a `.gitignore` that un-ignores it wins over the
/// line however often it is there.
async fn exclude(top: &Path, common: &Path, slug: &str) {
    let probe = format!(".claude/worktrees/{slug}");
    if git(top, &["check-ignore", "-q", &probe], LOCAL).await.is_ok() {
        return;
    }
    let file = common.join("info/exclude");
    let _ = std::fs::create_dir_all(common.join("info"));
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    if text.lines().any(|l| l == "/.claude/worktrees/") {
        return;
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("/.claude/worktrees/\n");
    let _ = std::fs::write(&file, text);
}
