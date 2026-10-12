//! The `claude` a resident runs by name: gensokyo's own, first on its PATH. It runs the next
//! `claude` on PATH with the tools residents are refused denied in that one's settings too, so a
//! resident, or whatever it read, cannot get them back by starting a claude of its own. A path
//! straight to the real one gets past it: it stops slips, not intent.

use crate::daemon::launch::{DISALLOWED, sh_quote};
use serde_json::{Value, json};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// A directory of gensokyo's own: the one gensokyo is installed in may hold the real `claude`.
pub fn dir() -> PathBuf {
    crate::paths::state_dir().join("shim")
}

/// Written at every daemon start, for the binary started. The script names its own directory,
/// which the lookup skips, whatever the state dir is by the time it runs.
pub fn write(exe: &Path) -> std::io::Result<()> {
    let d = dir();
    std::fs::create_dir_all(&d)?;
    let tmp = d.join(".claude.tmp");
    let quote = |p: &Path| sh_quote(&p.to_string_lossy());
    std::fs::write(&tmp, format!("#!/bin/sh\nexec {} _claude {} \"$@\"\n", quote(exe), quote(&d)))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    // Renamed into place: a resident may be running it right now.
    std::fs::rename(&tmp, d.join("claude"))
}

/// `args` with the refused tools added to the deny list of the `--settings` they give, or of
/// one of their own put first, before any subcommand. Theirs stays in its place, where it ends
/// the list a flag such as `--allowedTools` starts; Claude Code keeps only the last, so each is
/// replaced by that one merged. `read` reads a settings file.
pub fn args(
    args: &[OsString],
    read: impl Fn(&Path) -> std::io::Result<String>,
) -> Result<Vec<OsString>, String> {
    // Where in `out` a settings value is.
    let (mut out, mut at) = (Vec::<OsString>::new(), Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            out.push(a.clone());
            out.extend(it.by_ref().cloned());
        } else if a == "--settings" {
            out.push(a.clone());
            at.push(out.len());
            out.push(it.next().ok_or("--settings needs a value")?.clone());
        } else if let Some(v) = a.as_bytes().strip_prefix(b"--settings=") {
            out.push("--settings".into());
            at.push(out.len());
            out.push(OsStr::from_bytes(v).into());
        } else {
            out.push(a.clone());
        }
    }
    let parse = |s: &str, from: &str| {
        serde_json::from_str::<Value>(s).map_err(|e| format!("--settings {from}: {e}"))
    };
    let mut settings = match at.last().map(|&i| Path::new(&out[i])) {
        None => json!({}),
        Some(v) => match v.to_str().filter(|s| s.trim_start().starts_with('{')) {
            Some(inline) => parse(inline, "")?,
            None => {
                let f = v.display().to_string();
                parse(&read(v).map_err(|e| format!("--settings {f}: {e}"))?, &f)?
            }
        },
    };
    let deny = settings
        .as_object_mut()
        .map(|s| s.entry("permissions").or_insert_with(|| json!({})))
        .and_then(Value::as_object_mut)
        .map(|p| p.entry("deny").or_insert_with(|| json!([])))
        .and_then(Value::as_array_mut)
        .ok_or("--settings: settings whose permissions.deny is not a list")?;
    for t in DISALLOWED {
        if !deny.iter().any(|d| d == t) {
            deny.push(t.into());
        }
    }
    if at.is_empty() {
        out.splice(0..0, ["--settings".into(), OsString::new()]);
        at.push(1);
    }
    for i in at {
        out[i] = settings.to_string().into();
    }
    Ok(out)
}

/// `gensokyo _claude <its dir> <args>`: what the script runs.
pub fn main(argv: &[OsString]) -> ExitCode {
    let Some((own, argv)) = argv.split_first() else { return ExitCode::from(2) };
    let path = std::env::var_os("PATH");
    let Some(claude) = crate::daemon::launch::on_path(path.as_deref(), Path::new(own)) else {
        eprintln!("gensokyo: no claude on PATH but gensokyo's own");
        return ExitCode::from(127);
    };
    let a = match args(argv, |f| std::fs::read_to_string(f)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("gensokyo: {e}");
            return ExitCode::from(2);
        }
    };
    let e = std::process::Command::new(&claude).arg0("claude").args(a).exec();
    eprintln!("gensokyo: {}: {e}", claude.display());
    ExitCode::from(126)
}
