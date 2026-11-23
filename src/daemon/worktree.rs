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
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// A fetch or `ls-remote`, which may wait on the network.
const NET: Duration = Duration::from_secs(30);
/// Anything else, a checkout included.
const LOCAL: Duration = Duration::from_secs(300);
/// How long one killed for taking too long is waited for.
const KILLED: Duration = Duration::from_secs(5);

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

/// `cmd` run to its end within `limit`, as `what`: its stdout, else its stderr's first line.
async fn run(cmd: tokio::process::Command, what: &str, limit: Duration) -> Result<Vec<u8>, String> {
    run_in(cmd, what, limit, &[], &[0]).await
}

/// `run`, with `input` on its stdin, and any exit code in `ok` taken for success. One that
/// takes too long is killed, and waited for: what it wrote is all there is once this returns.
async fn run_in(
    mut cmd: tokio::process::Command,
    what: &str,
    limit: Duration,
    input: &[u8],
    ok: &[i32],
) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncWriteExt;
    let stdin = if input.is_empty() { Stdio::null() } else { Stdio::piped() };
    cmd.stdin(stdin).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("{what} could not run: {e}"))?;
    let (sin, sout, serr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let talk = async {
        let feed = async move {
            if let Some(mut sin) = sin {
                let _ = sin.write_all(input).await;
            }
        };
        let ((), out, err) = tokio::join!(feed, drain(sout), drain(serr));
        (out, err, child.wait().await)
    };
    let done = tokio::time::timeout(limit, talk).await;
    let (out, err, status) = match done {
        Ok((out, err, status)) => {
            (out, err, status.map_err(|e| format!("{what} could not run: {e}"))?)
        }
        Err(_) => {
            // Not forever: one stuck in a read the kill cannot cut short is left to it.
            let _ = child.start_kill();
            let late = format!("{what} took over {}s", limit.as_secs());
            return match tokio::time::timeout(KILLED, child.wait()).await {
                Ok(_) => Err(late),
                Err(_) => {
                    Err(format!("{late}, and had not stopped {}s after a kill", KILLED.as_secs()))
                }
            };
        }
    };
    if status.code().is_some_and(|c| ok.contains(&c)) {
        return Ok(out);
    }
    let err = String::from_utf8_lossy(&err);
    let line = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("it failed");
    Err(crate::tele::clean(line, 200))
}

/// All `pipe` gives until it closes.
async fn drain(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buf).await;
    }
    buf
}

/// `git -C dir args`: whether it worked, and its stdout, else its stderr's first line.
async fn git(dir: &Path, args: &[&str], limit: Duration) -> Result<String, String> {
    let out = git_raw(dir, args, limit).await?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// `git` with its stdout as it is, for paths: `-z` output, which a trim would cut.
async fn git_raw(dir: &Path, args: &[&str], limit: Duration) -> Result<Vec<u8>, String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    // A fetch never asks for a password: there is nobody at this terminal.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    run(cmd, &format!("git {}", args[0]), limit).await
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
    let note = add(&top, &path, &branch, ask.base.as_deref()).await?;
    let trees: Vec<&Path> = all.iter().map(|t| t.path.as_path()).chain([path.as_path()]).collect();
    let copied = include(&top, &path, &trees).await;
    let note: Vec<String> = note.into_iter().chain(copied).collect();
    Ok(Made::at(&path, &here, dir, (!note.is_empty()).then(|| note.join("; "))))
}

/// The worktree at `path`, made on `branch`: one there is, one origin has, or a new one from
/// `base`. What the user should know about how.
async fn add(
    top: &Path,
    path: &Path,
    branch: &str,
    base: Option<&str>,
) -> Result<Option<String>, String> {
    let p = path.to_string_lossy().into_owned();
    let mut note = None;
    let refname = format!("refs/heads/{branch}");
    if git(top, &["rev-parse", "--verify", "--quiet", &refname], LOCAL).await.is_ok() {
        git(top, &["worktree", "add", "-q", &p, branch], LOCAL).await?;
        return Ok(note);
    }
    let origin = git(top, &["remote", "get-url", "origin"], LOCAL).await.is_ok();
    // Out of reach, it is not asked again: the base is the last fetched, and the note says so.
    let mut unreachable = None;
    // One the remote has: a branch to review, or to carry on.
    if origin {
        match git(top, &["ls-remote", "--heads", "origin", &refname], net()).await {
            Ok(heads) if !heads.is_empty() => {
                fetch(top, branch).await?;
                let theirs = format!("origin/{branch}");
                let args = ["worktree", "add", "-q", "--track", "-b", branch, &p, &theirs];
                if git(top, &args, LOCAL).await.is_err() {
                    // A clone that fetches only some branches (single-branch) cannot track
                    // one outside them, until origin's refspec names it too.
                    let add = ["remote", "set-branches", "--add", "origin", branch];
                    git(top, &add, LOCAL).await?;
                    git(top, &args, LOCAL).await?;
                }
                return Ok(note);
            }
            Ok(_) => {}
            Err(e) => unreachable = Some(e),
        }
    }
    let base = match base {
        Some(b) => {
            let commit = format!("{b}^{{commit}}");
            let args = ["rev-parse", "--verify", "--quiet", "--end-of-options", &commit];
            git(top, &args, LOCAL).await.map_err(|_| format!("--base {b}: no such commit"))?
        }
        None if origin => {
            let head =
                git(top, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"], LOCAL).await;
            match head {
                Ok(h) => {
                    // Named by the remote: checked as any branch name is.
                    let name = h.strip_prefix("origin/").unwrap_or(&h).to_string();
                    branch_ok(top, &name).await.map_err(|e| format!("origin's default: {e}"))?;
                    let failed = match unreachable {
                        Some(e) => Some(format!("origin could not be reached ({e})")),
                        None => {
                            fetch(top, &name).await.err().map(|e| format!("the fetch failed ({e})"))
                        }
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
    git(top, &["worktree", "add", "-q", "--no-track", "-b", branch, &p, &base], LOCAL).await?;
    Ok(note)
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

/// The branch `dir` is on, which is to be `own`, pushed to origin and tracked there.
pub(super) async fn push(dir: &Path, own: &str) -> Result<(), String> {
    let b = on(dir).await?;
    if b != own {
        let shown = short(&dir.to_string_lossy());
        return Err(format!("{shown} is on {b}, not {own}: switch it back, or name another"));
    }
    let refname = format!("refs/heads/{b}");
    let args = ["push", "-q", "-u", "origin", "--end-of-options", &refname];
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
    // Not `--short`, which a tag of the same name makes `heads/<b>`.
    let b = git(dir, &["symbolic-ref", "-q", "HEAD"], LOCAL).await;
    b.ok().and_then(|b| b.strip_prefix("refs/heads/").map(String::from)).ok_or_else(|| {
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

/// What `.worktreeinclude` at the main checkout's top names, copied from there into the new
/// worktree `wt`, as Claude Code does for its own: the files git ignores that its patterns
/// (`.gitignore`'s syntax) match, so a tracked file is never copied. Git does the matching, and
/// the patterns reach it only as a file it reads. Nothing is copied that the new branch does
/// not ignore too, over what it has, through a link, or that is, holds or lies in a worktree
/// (`trees`). A clone where the disk can make one, a copy where not. What was left out, for
/// the user.
async fn include(top: &Path, wt: &Path, trees: &[&Path]) -> Option<String> {
    if !top.join(".worktreeinclude").is_file() {
        return None;
    }
    let list = ["ls-files", "-z", "--others", "--ignored", "--directory"];
    // Relative, as git runs in `top`.
    let from = [&list[..], &["--exclude-from=.worktreeinclude"]].concat();
    let ignored = git_raw(top, &[&list[..], &["--exclude-standard"]].concat(), LOCAL).await;
    let named = git_raw(top, &from, LOCAL).await;
    let (ignored, named) = match (ignored, named) {
        (Ok(i), Ok(n)) => (i, n),
        (Err(e), _) | (_, Err(e)) => return Some(format!(".worktreeinclude was not read: {e}")),
    };
    let split = |b: &[u8]| -> Vec<Vec<u8>> {
        b.split(|c| *c == 0).filter(|p| !p.is_empty()).map(<[u8]>::to_vec).collect()
    };
    let trees: Vec<Vec<u8>> = trees
        .iter()
        .filter_map(|t| t.strip_prefix(top).ok())
        .filter(|t| !t.as_os_str().is_empty())
        .map(|t| [t.as_os_str().as_bytes(), b"/"].concat())
        .collect();
    let (picked, held) = copies(&split(&named), &split(&ignored), &trees);
    let mut left: Vec<String> = held
        .iter()
        .map(|h| format!("{} not copied: it holds worktrees", plain(h).display()))
        .collect();
    let mut sure = Vec::new();
    for rel in &picked {
        let rel = plain(rel);
        match vet(top, wt, &rel) {
            Ok(dir) => sure.push((rel, dir)),
            Err(why) => left.push(format!("{} {why}", rel.display())),
        }
    }
    // What the new branch ignores, asked of it: what it does not would be there to commit.
    let ask: Vec<u8> = sure
        .iter()
        .flat_map(|(rel, dir)| {
            let slash: &[u8] = if *dir { b"/" } else { b"" };
            [rel.as_os_str().as_bytes(), slash, b"\0"].concat()
        })
        .collect();
    let theirs = if ask.is_empty() {
        Ok(Vec::new())
    } else {
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("-C").arg(wt).args(["check-ignore", "--stdin", "-z"]);
        run_in(cmd, "git check-ignore", LOCAL, &ask, &[0, 1]).await
    };
    let theirs = match theirs {
        Ok(t) => split(&t),
        Err(e) => return Some(format!(".worktreeinclude: the new branch's ignores unknown: {e}")),
    };
    for (rel, dir) in sure {
        let asked = [rel.as_os_str().as_bytes(), if dir { b"/" } else { b"" }].concat();
        let why = match theirs.contains(&asked) {
            true => copy(top, wt, &rel).await,
            false => Err("not copied: the branch does not ignore it".into()),
        };
        if let Err(why) = why {
            left.push(format!("{} {why}", rel.display()));
        }
    }
    (!left.is_empty()).then(|| format!(".worktreeinclude: {}", left.join(", ")))
}

/// A path from git, relative, without the `/` a directory ends in.
fn plain(p: &[u8]) -> PathBuf {
    Path::new(std::ffi::OsStr::from_bytes(p)).components().collect()
}

/// `a` is `b`, or inside it: `ls-files --directory` ends a directory's name with `/`.
fn within(a: &[u8], b: &[u8]) -> bool {
    a == b || (b.ends_with(b"/") && a.starts_with(b))
}

/// What to copy, from two `ls-files --others --ignored --directory` lists of one checkout:
/// `named` by `.worktreeinclude`'s patterns, `ignored` by git's own. Each lists a directory
/// whole when all in it matches, so the entries of one inside an entry of the other are what
/// both hold. Never what is or lies in one of `trees`, and nothing inside another; what holds
/// one of them comes second, to say it was left out.
fn copies(
    named: &[Vec<u8>],
    ignored: &[Vec<u8>],
    trees: &[Vec<u8>],
) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut v: Vec<Vec<u8>> = Vec::new();
    for n in named {
        if ignored.iter().any(|i| within(n, i)) {
            v.push(n.clone());
        } else {
            v.extend(ignored.iter().filter(|i| within(i, n)).cloned());
        }
    }
    v.retain(|p| !trees.iter().any(|t| within(p, t)));
    let (mut held, mut v): (Vec<_>, Vec<_>) =
        v.into_iter().partition(|p| trees.iter().any(|t| within(t, p)));
    for l in [&mut v, &mut held] {
        l.sort();
        l.dedup();
        let all = l.clone();
        l.retain(|p| !all.iter().any(|o| o != p && within(p, o)));
    }
    (v, held)
}

/// Whether `rel` may go from the checkout at `top` into the worktree `wt`: a plain path, not a
/// link, nothing there yet, and only real directories on the way. Whether it is a directory.
fn vet(top: &Path, wt: &Path, rel: &Path) -> Result<bool, String> {
    use std::path::Component;
    let parts: Vec<_> = rel.components().collect();
    if parts.is_empty() || parts.iter().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("not copied: not a path inside the checkout".into());
    }
    let meta = top.join(rel).symlink_metadata().map_err(|e| format!("not copied ({e})"))?;
    if meta.file_type().is_symlink() {
        return Err("not copied: it is a link".into());
    }
    // Never through a directory the branch has as a link, which could point anywhere.
    let mut at = wt.to_path_buf();
    for part in &parts[..parts.len() - 1] {
        at.push(part);
        match at.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!("not copied: {} is a link there", part_of(wt, &at)));
            }
            Ok(m) if !m.is_dir() => {
                return Err(format!("not copied: {} is a file there", part_of(wt, &at)));
            }
            Ok(_) => {}
            Err(_) => return Ok(meta.is_dir()),
        }
    }
    match wt.join(rel).symlink_metadata() {
        Ok(_) => Err("left as the branch has it".into()),
        Err(_) => Ok(meta.is_dir()),
    }
}

/// `rel`, vetted, from the checkout at `top` into the worktree `wt`. What a failed copy made is
/// removed again.
async fn copy(top: &Path, wt: &Path, rel: &Path) -> Result<(), String> {
    // The directories on the way the branch lacks, one at a time: those it has are real ones.
    let mut at = wt.to_path_buf();
    for part in rel.parent().into_iter().flat_map(Path::components) {
        at.push(part);
        if at.symlink_metadata().is_err() {
            std::fs::create_dir(&at).map_err(|e| format!("not copied ({e})"))?;
        }
    }
    let dst = wt.join(rel);
    let cp = |clone: bool| {
        let mut cmd = tokio::process::Command::new("cp");
        // `-c` clones on APFS, and copies where it cannot. Run in the checkout, so the source
        // is `rel` itself, and `--` keeps one that starts with a `-` a path.
        cmd.args(if clone { &["-c", "-R", "-P", "-p"][..] } else { &["-R", "-P", "-p"][..] });
        cmd.arg("--").arg(rel).arg(&dst).current_dir(top);
        // Its messages in English, which is what says that `-c` is no option of its.
        cmd.env("LC_ALL", "C");
        run(cmd, "cp", copy_limit())
    };
    let mut done = cp(true).await;
    // A cp that has no `-c` (GNU's) says so before it copies anything.
    if done.as_ref().is_err_and(|e| e.contains("invalid option") || e.contains("illegal option")) {
        done = cp(false).await;
    }
    let Err(e) = done else { return Ok(()) };
    let gone = dst.clone();
    let cleared = tokio::task::spawn_blocking(move || clear(&gone)).await;
    match cleared {
        Ok(Ok(())) => Err(format!("not copied ({e})")),
        Ok(Err(c)) => Err(format!("not copied ({e}), and what was copied is still there ({c})")),
        Err(c) => Err(format!("not copied ({e}), and what was copied is still there ({c})")),
    }
}

fn copy_limit() -> Duration {
    let ms = std::env::var("GENSOKYO_COPY_LIMIT_MS").ok().and_then(|v| v.parse().ok());
    ms.map_or(LOCAL, Duration::from_millis)
}

/// `p` and all in it removed, nothing followed through a link: directories made writable first,
/// as `cp -p` keeps a read-only one read-only. Nothing there is nothing to do.
fn clear(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = p.symlink_metadata() else { return Ok(()) };
    if !meta.is_dir() {
        return std::fs::remove_file(p);
    }
    let mode = meta.permissions().mode() | 0o700;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))?;
    for e in std::fs::read_dir(p)? {
        clear(&e?.path())?;
    }
    std::fs::remove_dir(p)
}

/// `p` inside the worktree `wt`, as the user would name it.
fn part_of(wt: &Path, p: &Path) -> String {
    p.strip_prefix(wt).unwrap_or(p).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::copies;

    fn list(v: &[&str]) -> Vec<Vec<u8>> {
        v.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    #[test]
    fn copies_are_what_both_lists_hold_and_never_a_worktree() {
        let ignored = list(&["vendor/", ".env", "target/", ".claude/", "big/a/"]);
        let named = list(&["vendor/x/", ".env", "notes.txt", "big/", ".claude/", "big/a/f"]);
        let trees = list(&[".claude/worktrees/new/"]);
        let (got, held) = copies(&named, &ignored, &trees);
        // Named inside an ignored directory, or an ignored one inside a named one; what is not
        // ignored, holds a worktree, or lies in a copied directory is left out.
        assert_eq!(got, list(&[".env", "big/a/", "vendor/x/"]));
        assert_eq!(held, list(&[".claude/"]), "said to be left out");
        assert_eq!(copies(&list(&["a/"]), &list(&["a/b/", "a/c"]), &[]).0.len(), 2);
        let (got, held) = copies(&list(&["x/", "x/y/z"]), &list(&["x/"]), &list(&["x/y/"]));
        assert!(got.is_empty() && held == list(&["x/"]), "{got:?} {held:?}");
    }
}
