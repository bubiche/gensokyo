//! The `claude` a resident runs by name: gensokyo's own, first on its PATH. It runs the next
//! `claude` on PATH with the tools residents are refused denied in that one's settings too, so a
//! resident, or whatever it read, cannot get them back by starting a claude of its own. A path
//! straight to the real one gets past it: it stops slips, not intent.

use crate::daemon::launch::{DISALLOWED, sh_quote};
use serde_json::{Value, json};
use std::ffi::OsString;
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
/// one of their own. Claude Code keeps only the last `--settings`, so theirs is taken out and
/// the merged one goes first, before any subcommand and any `--`. `read` reads a settings file.
pub fn args(
    args: &[OsString],
    read: impl Fn(&str) -> std::io::Result<String>,
) -> Result<Vec<OsString>, String> {
    let (mut rest, mut given) = (Vec::new(), None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("--") => {
                rest.push(a.clone());
                rest.extend(it.by_ref().cloned());
            }
            Some("--settings") => {
                let v = it.next().ok_or("--settings needs a value")?;
                given = Some(v.to_string_lossy().into_owned());
            }
            Some(s) if s.starts_with("--settings=") => given = Some(s[11..].to_string()),
            _ => rest.push(a.clone()),
        }
    }
    let parse = |s: &str, from: &str| {
        serde_json::from_str::<Value>(s).map_err(|e| format!("--settings {from}: {e}"))
    };
    let mut settings = match given {
        None => json!({}),
        Some(v) if v.trim_start().starts_with('{') => parse(&v, "")?,
        Some(f) => parse(&read(&f).map_err(|e| format!("--settings {f}: {e}"))?, &f)?,
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
    let mut out = vec!["--settings".into(), settings.to_string().into()];
    out.extend(rest);
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
