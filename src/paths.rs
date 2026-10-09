//! Where things are: the state, config and share dirs, the socket, Claude Code's own config
//! dir, and `~` in paths as they are shown and typed. Every environment read that names a place
//! is here.

use std::path::{Path, PathBuf};

/// `$HOME`, or empty.
pub fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// `$GENSOKYO_STATE_DIR`, else `~/.local/state/gensokyo`.
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("GENSOKYO_STATE_DIR") {
        return d.into();
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/".into());
    PathBuf::from(home).join(".local/state/gensokyo")
}

/// `$GENSOKYO_CONFIG_DIR`, else `$XDG_CONFIG_HOME/gensokyo`, else `~/.config/gensokyo`.
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("GENSOKYO_CONFIG_DIR") {
        return d.into();
    }
    match std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        Some(x) => PathBuf::from(x).join("gensokyo"),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
            .join(".config/gensokyo"),
    }
}

/// The ids the next daemon recalls as it starts, one a line: `gensokyo restart`'s, or a daemon's
/// own as a signal stops it (under a `# <signal>` line).
pub fn comeback_path() -> PathBuf {
    state_dir().join("run/comeback")
}

/// `KEY=value` from the config file, the last such line winning; `#` lines are comments.
pub fn config(key: &str) -> Option<String> {
    let text = std::fs::read_to_string(config_dir().join("config")).ok()?;
    let lines = text.lines().filter(|l| !l.starts_with('#'));
    let mut hits = lines.filter_map(|l| l.split_once('=')).filter(|(k, _)| *k == key);
    hits.next_back().map(|(_, v)| v.into())
}

/// `KEY=value` in the config file: every line that sets `key` says `value` now, or a line for it
/// goes at the end; the rest stays as it was, comments too. A config that is a link (into a
/// dotfiles repo, say) is written where it points.
pub fn set_config(key: &str, value: &str) -> std::io::Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("config");
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let text = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        r => r?,
    };
    let line = format!("{key}={value}");
    let mut found = false;
    let mut lines: Vec<&str> = text.lines().collect();
    for l in lines.iter_mut().filter(|l| !l.starts_with('#')) {
        if l.split_once('=').is_some_and(|(k, _)| k == key) {
            (*l, found) = (&line, true);
        }
    }
    if !found {
        lines.push(&line);
    }
    crate::daemon::store::write_atomic(&path, format!("{}\n", lines.join("\n")).as_bytes())
}

/// `$GENSOKYO_SHARE`, else `share/` beside the binary or up to three levels above it (a build
/// tree), looking from the binary a symlink on `PATH` points at too.
pub fn share_dir(exe: &Path) -> Option<PathBuf> {
    if let Some(s) = std::env::var_os("GENSOKYO_SHARE") {
        return Some(s.into());
    }
    let near = |exe: &Path| {
        let mut dirs = exe.ancestors().skip(1).take(4).map(|d| d.join("share"));
        dirs.find(|s| s.join("names.txt").is_file())
    };
    near(exe).or_else(|| near(&std::fs::canonicalize(exe).ok()?))
}

/// The release tree this binary was unpacked into: `<root>/bin/gensokyo`, with the `VERSION` and
/// `install.sh` a release puts beside `bin/`. A build under `target/` is not one.
pub fn release_root(exe: &Path) -> Option<PathBuf> {
    let exe = std::fs::canonicalize(exe).ok()?;
    let bin = exe.parent().filter(|b| b.file_name().is_some_and(|n| n == "bin"))?;
    let root = bin.parent()?;
    (root.join("VERSION").is_file() && root.join("install.sh").is_file()).then(|| root.into())
}

/// `$GENSOKYO_SOCKET`, else `run/gensokyo.sock` in the state dir, else, when that is past the
/// 104 bytes a socket path may hold, a name in `$TMPDIR` derived from the state dir.
pub fn socket_path() -> PathBuf {
    if let Some(s) = std::env::var_os("GENSOKYO_SOCKET") {
        return s.into();
    }
    let p = state_dir().join("run/gensokyo.sock");
    if p.as_os_str().len() < 104 {
        return p;
    }
    // FNV-1a: stable across builds, unlike std's hasher.
    let h = p
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    std::env::temp_dir().join(format!("gensokyo-{h:016x}.sock"))
}

/// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
}

/// `$CLAUDE_CONFIG_DIR/.claude.json`, else `~/.claude.json`: which directories are trusted.
pub fn claude_json() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(PathBuf::new, PathBuf::from)
        .join(".claude.json")
}

/// `~` for `home`, as paths are shown.
pub fn tilde(p: &str, home: &str) -> String {
    match p.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => p.to_string(),
    }
}

/// `$HOME/x` as `~/x`.
pub fn short(p: &str) -> String {
    tilde(p, &home())
}

/// `~` and `~/…` as `home` and under it; anything else as it is.
pub fn untilde(p: &str, home: &str) -> String {
    match p.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => p.to_string(),
    }
}

/// `~` and `~/…` under `$HOME`.
pub fn expand(p: &str) -> String {
    untilde(p, &home())
}
