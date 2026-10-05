//! `update` and `uninstall`: the two that touch gensokyo's own install. Downloading, verifying
//! and swapping are the tree's install.sh, and what may be deleted is its uninstall.sh, so each
//! rule is written once, and both work with this binary gone.

use crate::paths;
use std::io::{BufRead, IsTerminal, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;

/// The release tree, or why there is none to act on.
fn root(fix: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    paths::release_root(&exe).ok_or_else(|| {
        let at = std::fs::canonicalize(&exe).unwrap_or(exe);
        format!("{} is a build, not a release install: {fix}", paths::short(&at.to_string_lossy()))
    })
}

/// Where the releases are; a test points it at a directory (curl speaks file://), a fork at its
/// own.
fn base() -> String {
    std::env::var("GENSOKYO_RELEASE_BASE").unwrap_or_else(|_| {
        let repo = std::env::var("GENSOKYO_REPO").unwrap_or_else(|_| "bubiche/gensokyo".into());
        format!("https://github.com/{repo}/releases")
    })
}

/// The newest release, from the one-line VERSION asset GitHub serves at releases/latest: the API
/// would say too, and rate-limits by address.
fn latest() -> Result<String, String> {
    let url = format!("{}/latest/download/VERSION", base());
    let out = Command::new("curl").args(["-fsSL", "--retry", "2", &url]).output();
    let out = out.map_err(|e| format!("curl: {e}"))?;
    let v = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_string();
    let ok = !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || ".+_-".contains(c));
    match out.status.success() && ok {
        true => Ok(v),
        false => Err(format!("cannot tell which release is the latest ({url} did not answer)")),
    }
}

fn running() -> bool {
    UnixStream::connect(paths::socket_path()).is_ok()
}

pub fn update(check: bool, version: Option<String>) -> Result<(), String> {
    let root = root("`git pull` and `cargo build --release` update it")?;
    let have = env!("CARGO_PKG_VERSION");
    let want = match version {
        Some(v) => v,
        None => latest()?,
    };
    if want == have {
        println!("gensokyo {have} is already the release you asked for");
        return Ok(());
    }
    if check {
        println!("gensokyo {have} is installed; {want} is out (`gensokyo update` fetches it)");
        return Ok(());
    }
    let st = Command::new("/bin/sh")
        .arg(root.join("install.sh"))
        .args(["--update", "--version", &want])
        .status()
        .map_err(|e| format!("install.sh: {e}"))?;
    if !st.success() {
        return Err(format!("nothing was changed in {}", paths::short(&root.to_string_lossy())));
    }
    // The tree is swapped by path, so the link and the login agent need nothing; the daemon is
    // still the old binary, already loaded.
    if running() {
        println!("the daemon is still {have}: `gensokyo restart` brings it up on {want} (a turn");
        println!("in progress is cut short)");
    }
    Ok(())
}

pub fn uninstall(yes: bool, keep: bool) -> Result<(), String> {
    if running() {
        return Err("the daemon is running: `gensokyo quit` first (each resident is asked to \
                    /exit), then uninstall"
            .into());
    }
    let root = root(
        "nothing here is a release's to remove; `gensokyo login remove` takes out the login agent",
    )?;
    let run = |yes: bool| {
        let mut c = Command::new("/bin/sh");
        c.arg(root.join("uninstall.sh")).arg("--dir").arg(&root);
        c.args(yes.then_some("--yes")).args(keep.then_some("--keep-data"));
        match c.status() {
            Ok(s) if s.success() => Ok(()),
            Ok(_) => Err("uninstall.sh stopped; see above".to_string()),
            Err(e) => Err(format!("uninstall.sh: {e}")),
        }
    };
    // Without --yes the script only lists; the question is asked here, where there is a terminal.
    run(yes)?;
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        println!("no terminal to ask, so nothing was deleted (--yes deletes)");
        return Ok(());
    }
    print!("remove all of that? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().lock().read_line(&mut answer);
    match answer.trim() {
        "y" | "Y" | "yes" | "Yes" | "YES" => run(true),
        _ => {
            println!("kept all of it");
            Ok(())
        }
    }
}
