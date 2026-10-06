//! Writing ritual files: `ritual add`, the user's own copy of a shipped one, turning one on or
//! off, the template `ritual new` opens, and removing one.

use super::{Dir, NAME_RULE, Ritual, SCHEDULES, Target, Trust, check, name_ok, parse, when};
use crate::daemon::store::write_atomic;
use crate::frontmatter::quote;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use std::path::{Path, PathBuf};

/// What `ritual add` takes. `cwd` is absolute: the caller resolves it against its own directory.
#[derive(Debug, Clone, Default)]
pub struct Add {
    pub name: String,
    pub schedule: String,
    pub cwd: String,
    pub description: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub mcp_config: Option<String>,
    pub target: Option<String>,
    pub keep: Option<String>,
    pub overlap: Option<String>,
    pub headless: bool,
    pub catch_up: Option<String>,
    pub prompt: String,
    pub disabled: bool,
    pub allowed_tools: Vec<String>,
}

/// A plain word: no quoting, and nothing that would read as something else.
fn word(k: &str, v: &str) -> Result<String, String> {
    if v.is_empty() || v.contains(|c: char| c.is_whitespace() || "#\"'".contains(c)) {
        return Err(format!("--{} {v:?} is not a single word", k.replace('_', "-")));
    }
    Ok(v.into())
}

/// Writes `<rituals>/<name>.md`, checked first as the daemon will read it: anything wrong
/// writes nothing, except a cwd Claude Code has not been trusted in, which is written with the
/// warning (the user can answer that dialog). Returns the file and what to say: lines starting
/// `warning: ` are warnings.
pub fn add(
    a: &Add,
    now: i64,
    tz: &TimeZone,
    trust: &Trust,
    rituals: &Path,
    share: Option<&Path>,
) -> Result<(PathBuf, Vec<String>), String> {
    let name = &a.name;
    if name.is_empty() {
        return Err("--name is the ritual's name, and the name of its file".into());
    }
    if !name_ok(name) {
        return Err(format!("'{name}' cannot be a ritual name ({NAME_RULE})"));
    }
    if a.prompt.trim().is_empty() {
        return Err("--prompt-file <file> (or --prompt \"one line\", or --prompt-file - for \
                    stdin) is what the run is asked to do"
            .into());
    }
    if a.schedule.is_empty() {
        return Err(format!("--schedule \"3 9 * * 1-5\" ({SCHEDULES})"));
    }
    // `keep` is how long a run of its own stays; a resident that is already there, or a run
    // with no pane, has none.
    if a.keep.is_some() && (a.headless || a.target.as_deref().is_some_and(|t| t != "new")) {
        return Err("--keep is only for a run of its own: not with --target or --headless".into());
    }
    let cwd = std::fs::canonicalize(&a.cwd)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| a.cwd.clone());
    let path = rituals.join(format!("{name}.md"));
    if path.exists() {
        return Err(format!(
            "{name} is already there: {} (edit that file, or gensokyo ritual edit {name})",
            crate::paths::short(&path.to_string_lossy())
        ));
    }
    let mut f = format!("---\nname: {name}\n");
    if let Some(d) = &a.description {
        f += &format!("description: {}\n", quote(d)?);
    }
    f += &format!("schedule: {}\n", quote(&a.schedule)?);
    if let Some(t) = &a.target {
        f += &format!("target: {}\n", word("target", t)?);
    }
    f += &format!("cwd: {}\n", quote(&cwd)?);
    for (k, v) in [("model", &a.model), ("effort", &a.effort), ("mode", &a.mode)] {
        if let Some(v) = v {
            f += &format!("{k}: {}\n", word(k, v)?);
        }
    }
    if !a.allowed_tools.is_empty() {
        f += "allowed_tools:\n";
        for t in &a.allowed_tools {
            f += &format!("  - {}\n", quote(t)?);
        }
    }
    if let Some(m) = &a.mcp_config {
        f += &format!("mcp_config: {}\n", quote(m)?);
    }
    for (k, v) in [("keep", &a.keep), ("overlap", &a.overlap), ("catch_up", &a.catch_up)] {
        if let Some(v) = v {
            f += &format!("{k}: {}\n", word(k, v)?);
        }
    }
    if a.headless {
        f += "headless: true\n";
    }
    if a.disabled {
        f += "enabled: false\n";
    }
    f += &format!("---\n{}\n", a.prompt.trim_end());
    let r = parse(name, &path, false, &f);
    // Every check but trust, which is the one the user can fix after the fact.
    let s = check(&r, now, tz, &Trust::from_json(None))
        .map_err(|p| format!("{p}\nnothing was written"))?;
    let own = matches!(r.target(), Target::New | Target::Persistent);
    let untrusted = own && !r.headless() && !trust.trusted(Path::new(&cwd));
    std::fs::create_dir_all(rituals).map_err(|e| format!("{}: {e}", rituals.display()))?;
    write_atomic(&path, f.as_bytes())
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    let mut out = vec![format!("wrote {}", crate::paths::short(&path.to_string_lossy()))];
    if share.is_some_and(|s| s.join("rituals").join(format!("{name}.md")).exists()) {
        out.push(format!(
            "warning: {name} is also one of the shipped examples; yours shadows it from now on"
        ));
    }
    if untrusted {
        out.push(format!("warning: {}", check(&r, now, tz, trust).err().unwrap_or_default()));
        out.push("  not firing until that is fixed".into());
    } else if !r.enabled() {
        out.push(format!("  disabled, so it will not fire: gensokyo ritual enable {name}"));
    } else if let Some(next) = s.next(now, tz) {
        let secs = (next - now).max(0) as u64;
        out.push(format!("  next fire  {} (in {})", when(next, tz), crate::tele::age(secs)));
    }
    let clock = Timestamp::from_second(now)
        .map(|t| t.to_zoned(tz.clone()).strftime("%H:%M %z").to_string())
        .unwrap_or_default();
    out.push(format!(
        "  {} is this machine's clock, now {clock} - a ritual is never in UTC",
        a.schedule
    ));
    if r.headless() {
        out.push(format!(
            "  headless: no pane to watch and nobody to answer a prompt, so what it needs goes in \
             allowed_tools; what each run says keeps in {}",
            crate::paths::short(&Dir::of(name).runs().to_string_lossy())
        ));
    }
    out.push(format!(
        "  gensokyo ritual run {name}   fires it now: what it asks permission for goes in allowed_tools"
    ));
    Ok((path, out))
}

/// The user's own copy of a ritual: a shipped one is copied into `rituals` first, so an
/// update cannot overwrite the user's version.
pub fn mine(r: &Ritual, rituals: &Path) -> Result<PathBuf, String> {
    if !r.shipped {
        return Ok(r.path.clone());
    }
    let dst = rituals.join(format!("{}.md", r.slug));
    if !dst.exists() {
        std::fs::create_dir_all(rituals)
            .and_then(|_| std::fs::copy(&r.path, &dst).map(|_| ()))
            .map_err(|e| format!("could not copy {} into {}: {e}", r.slug, rituals.display()))?;
    }
    Ok(dst)
}

/// `enabled: true|false` in the frontmatter, replacing the line that is there (and any repeat
/// of it) or added at the end of the block; the rest of the file as it was.
pub fn set_enabled(path: &Path, on: bool) -> Result<(), String> {
    let shown = crate::paths::short(&path.to_string_lossy());
    // Through a symlink (a dotfiles repo, say), not over it.
    let path = &std::fs::canonicalize(path).map_err(|e| format!("{shown}: {e}"))?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{shown}: {e}"))?;
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.first() != Some(&"---") {
        return Err(format!(
            "{shown} has no frontmatter, so there is no schedule to turn on or off"
        ));
    }
    let end = lines.iter().skip(1).position(|l| *l == "---").map(|i| i + 1);
    let Some(end) = end else {
        return Err(format!("{shown} has no end to its frontmatter (a --- line)"));
    };
    let line = format!("enabled: {on}");
    let is_key =
        |l: &str| l.trim_start().split_once(':').is_some_and(|(k, _)| k.trim() == "enabled");
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    for (i, l) in lines.drain(..).enumerate() {
        if i > 0 && i < end && is_key(l) {
            if !done {
                out.push(line.clone());
                done = true;
            }
            continue;
        }
        if i == end && !done {
            out.push(line.clone());
            done = true;
        }
        out.push(l.to_string());
    }
    // A CRLF file stays one.
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut body = out.join(nl);
    if text.ends_with('\n') {
        body.push_str(nl);
    }
    write_atomic(path, body.as_bytes()).map_err(|e| format!("could not write {shown}: {e}"))
}

/// On or off, the user's own copy of a shipped one first. Turned on, it is stamped with this
/// minute: the fires it missed while off were not missed, and must not be caught up.
pub fn toggle(r: &Ritual, on: bool, now: i64, rituals: &Path) -> Result<Vec<String>, String> {
    let path = mine(r, rituals)?;
    let mut out = Vec::new();
    if path != r.path {
        out.push(format!(
            "the shipped {}.md is now yours, in {}",
            r.slug,
            crate::paths::short(&path.to_string_lossy())
        ));
    }
    set_enabled(&path, on)?;
    if on {
        let _ = Dir::of(&r.slug).set_stamp(now - now.rem_euclid(60));
    }
    out.push(format!("{} is {}", r.slug, if on { "enabled" } else { "disabled" }));
    Ok(out)
}

/// What `ritual new` opens in the editor. It arrives disabled.
pub fn template(name: &str, cwd: &str) -> String {
    format!(
        "---\n\
         name: {name}\n\
         description: what this ritual is for, in one line\n\
         schedule: \"0 9 * * 1-5\"     # five cron fields, local time; also @hourly @daily @weekly @monthly @yearly, \"every 30m\", \"every 2h\"\n\
         cwd: \"{cwd}\"                   # the directory the run works in\n\
         enabled: false              # true once you are happy with it\n\
         ---\n\
         What the resident should do, written for a session that has never seen this job\n\
         before: it starts fresh every time. It is told where its notes from previous\n\
         runs are - it reads them first, and updates them before it finishes.\n"
    )
}

/// The file, its notes and its journal: gone. Never a shipped example, which an update would
/// put back.
pub fn remove(r: &Ritual, dir: &Dir, share: Option<&Path>) -> Result<Vec<String>, String> {
    if r.shipped {
        return Err(format!(
            "{0} is one of the examples gensokyo ships, and an update would put it back: \
             'gensokyo ritual disable {0}' is how you stop it firing",
            r.slug
        ));
    }
    let shown = crate::paths::short(&r.path.to_string_lossy());
    std::fs::remove_file(&r.path).map_err(|e| format!("could not delete {shown}: {e}"))?;
    let mut out = vec![format!("{} is gone, and {shown} with it", r.slug)];
    if dir.path.is_dir() {
        std::fs::remove_dir_all(&dir.path)
            .map_err(|e| format!("could not delete {}: {e}", dir.path.display()))?;
        out.push(format!(
            "  its notes and its journal went too, from {}",
            crate::paths::short(&dir.path.to_string_lossy())
        ));
    }
    if share.is_some_and(|s| s.join("rituals").join(format!("{}.md", r.slug)).exists()) {
        out.push("  the example gensokyo ships with that name is in the listing again".into());
    }
    Ok(out)
}
