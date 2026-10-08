//! What a resident's status line reports, read from the JSON Claude Code gives the statusLine
//! command, and the formatting that its own line, the border, the sidebar and `list` share.
//! Also the two things the report lacks: the git branch and the settings chain.

use crate::proto::{Limit, Telemetry};
use serde_json::Value;
use std::path::Path;

/// The report, from Claude Code's statusLine JSON (2.1.260: `model.display_name`,
/// `effort.level`, `context_window.*`, `prompt_cache.{hit_ratio,warm,expires_at}`, `cost.*`,
/// and on Pro and Max `rate_limits.{five_hour,seven_day}`; `workspace.current_dir`, which
/// follows a `cd`, 2.1.291).
/// `advisor` and `at` are not in it.
pub fn from_statusline(j: &Value) -> Telemetry {
    let s = |p: &str| j.pointer(p).and_then(Value::as_str).map(|v| clean(v, 40));
    let n = |p: &str| j.pointer(p).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0);
    let u = |p: &str| n(p).map(|v| v as u64);
    let pct = |p: &str| n(p).map(|v| v.min(100.0) as u32);
    let limit = |p: &str| {
        let used = pct(&format!("{p}/used_percentage"))?;
        Some(Limit { used, resets: u(&format!("{p}/resets_at")).map(|v| v as i64) })
    };
    let turn_cache = {
        let c = |k: &str| u(&format!("/context_window/current_usage/{k}")).unwrap_or(0) as u128;
        let read = c("cache_read_input_tokens");
        let total = c("input_tokens") + read + c("cache_creation_input_tokens");
        (total > 0).then(|| (read * 100 / total) as u32)
    };
    Telemetry {
        model: s("/model/display_name"),
        advisor: None,
        effort: s("/effort/level"),
        ctx: pct("/context_window/used_percentage"),
        window: u("/context_window/context_window_size"),
        cache: n("/prompt_cache/hit_ratio").map(|r| (r * 100.0).min(100.0) as u32),
        turn_cache,
        cache_warm: j.pointer("/prompt_cache/warm").and_then(Value::as_bool),
        cache_until: u("/prompt_cache/expires_at").map(|v| v as i64),
        cost: n("/cost/total_cost_usd"),
        added: u("/cost/total_lines_added"),
        removed: u("/cost/total_lines_removed"),
        age: u("/cost/total_duration_ms").map(|ms| ms / 1000),
        five_hour: limit("/rate_limits/five_hour"),
        seven_day: limit("/rate_limits/seven_day"),
        at: 0,
        dir: ["/workspace/current_dir", "/cwd"]
            .iter()
            .find_map(|p| j.pointer(p).and_then(Value::as_str))
            .filter(|d| is_dir_path(d))
            .map(String::from),
    }
}

/// A directory Claude reports it works in, fit to show and to read a branch from: absolute,
/// one line, no control characters.
pub fn is_dir_path(d: &str) -> bool {
    d.starts_with('/') && d.len() < 4096 && !d.chars().any(char::is_control)
}

/// gensokyo's own line at the bottom of a resident: the numbers a working session watches.
/// `Sonnet 5→⚖ Opus · medium · █▏░░░░░░░░ 12% of 1M · ⚡93% (turn 99%) · $0.19 · +8/-0 · 5m`,
/// the context yellow from 70% and red from 90%, and the cache marked once it has gone cold.
/// Unknown fields are left out. Colour is for this line only, which Claude Code draws.
pub fn own_line(t: &Telemetry, now: i64) -> String {
    let mut out = model(t).unwrap_or_else(|| "Claude".into());
    let mut add = |s: String| {
        out.push_str(" · ");
        out.push_str(&s);
    };
    if let Some(e) = &t.effort {
        add(e.clone());
    }
    if let Some(c) = t.ctx {
        let of = t.window.map_or(String::new(), |w| format!(" of {}", tokens(w)));
        let ansi = match heat(c, 0) {
            Heat::Calm => "",
            Heat::Warn => "\x1b[33m",
            Heat::Hot => "\x1b[31m",
        };
        let off = if ansi.is_empty() { "" } else { "\x1b[0m" };
        add(format!("{ansi}{} {c}%{off}{of}", bar(c, 10)));
    }
    if let Some(c) = t.cache {
        let turn = t.turn_cache.map_or(String::new(), |c| format!(" (turn {c}%)"));
        let cold = if cold(t, now) { " \x1b[36mcold\x1b[0m" } else { "" };
        add(format!("⚡{c}%{turn}{cold}"));
    }
    if let Some(c) = t.cost {
        add(cost(c));
    }
    // Only once there is something to count: the first report reads "+0/-0 · 0s".
    let (a, r) = (t.added.unwrap_or(0), t.removed.unwrap_or(0));
    if a > 0 || r > 0 {
        add(format!("+{a}/-{r}"));
    }
    if let Some(s) = t.age.filter(|s| *s > 0) {
        add(age(s));
    }
    out
}

/// The " · "-joined fields of the border, or with `verbose` those of `list`, which adds the
/// context, the turn's cache rate and the branch.
pub fn fields(
    t: Option<&Telemetry>,
    mode: Option<&str>,
    branch: Option<&str>,
    verbose: bool,
    now: i64,
) -> String {
    let mut v: Vec<String> = Vec::new();
    if let Some(m) = t.and_then(model) {
        v.push(m);
    }
    if let Some(c) = t.and_then(|t| t.ctx).filter(|_| verbose) {
        v.push(format!("ctx {c}%"));
    }
    v.extend(t.and_then(|t| t.effort.clone()));
    v.extend(mode.map(mode_label));
    if let Some(t) = t {
        if let Some(c) = t.cache {
            let turn =
                t.turn_cache.filter(|_| verbose).map_or(String::new(), |c| format!(" (turn {c}%)"));
            let cold = if cold(t, now) { " cold" } else { "" };
            v.push(format!("⚡{c}%{turn}{cold}"));
        }
        v.extend(t.cost.map(cost));
    }
    v.extend(branch.filter(|_| verbose).map(|b| format!("⎇ {b}")));
    v.join(" · ")
}

/// `Sonnet 5→⚖ Opus`.
fn model(t: &Telemetry) -> Option<String> {
    let m = t.model.clone()?;
    Some(match &t.advisor {
        Some(a) => format!("{m}→⚖ {}", title_word(a)),
        None => m,
    })
}

/// "Sonnet 5" -> "Son", for the sidebar.
pub fn model_short(m: &str) -> String {
    m.split_whitespace().next().unwrap_or("").chars().take(3).collect()
}

/// A usage window as the freshest report has it, across every resident's: within a window use
/// only grows, so the highest of those on the latest window. One past its reset is gone (Claude
/// Code drops it too, but only from the reports it sends after).
pub fn freshest<'a>(limits: impl IntoIterator<Item = &'a Limit>, now: i64) -> Option<&'a Limit> {
    // Reports of one window can name its reset some seconds apart.
    const SAME: i64 = 600;
    let live: Vec<&Limit> =
        limits.into_iter().filter(|l| l.resets.is_none_or(|r| r > now)).collect();
    let last = live.iter().filter_map(|l| l.resets).max();
    live.into_iter()
        .filter(|l| last.is_none_or(|m| l.resets.is_some_and(|r| r + SAME >= m)))
        .max_by_key(|l| l.used)
}

/// A usage window, `5h █▊░░░ 37% ↻2h11m`; the countdown is dropped once it has reset.
pub fn usage(label: &str, l: &Limit, now: i64, cells: u32) -> String {
    format!("{label} {} {}", bar(l.used, cells), usage_tail(l, now))
}

/// What follows a usage window's bar: `37% ↻2h11m`.
pub fn usage_tail(l: &Limit, now: i64) -> String {
    let eta =
        l.resets.filter(|r| *r > now).map_or(String::new(), |r| format!(" ↻{}", eta(r - now)));
    format!("{}%{eta}", l.used)
}

/// How hard a number presses: calm, worth a look, or nearly out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heat {
    Calm,
    Warn,
    Hot,
}

/// A share used, warm from 70% and hot from 90%; with `ahead` set, also warm from half once it
/// runs that many points ahead of the clock.
pub fn heat(used: u32, ahead: i64) -> Heat {
    match used {
        90.. => Heat::Hot,
        70.. => Heat::Warn,
        50.. if ahead > 0 => Heat::Warn,
        _ => Heat::Calm,
    }
}

/// A usage window's heat: from half used, warm while use runs ahead of the time gone in a
/// window `span` seconds long, on course to run out before it resets.
pub fn usage_heat(l: &Limit, span: i64, now: i64) -> Heat {
    let gone = l.resets.map_or(0, |r| 100 - (r - now).clamp(0, span) * 100 / span.max(1));
    heat(l.used, i64::from(l.used) - gone)
}

/// The cached prefix has left its lifetime: the next request writes it all again.
pub fn cold(t: &Telemetry, now: i64) -> bool {
    t.cache.is_some() && (t.cache_warm == Some(false) || t.cache_until.is_some_and(|u| u <= now))
}

pub fn mode_label(m: &str) -> String {
    match m {
        "acceptEdits" => "accept-edits".into(),
        "bypassPermissions" => "bypass".into(),
        "dontAsk" => "dont-ask".into(),
        m => m.into(),
    }
}

fn title_word(w: &str) -> String {
    let mut c = w.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

pub fn cost(c: f64) -> String {
    format!("${c:.2}")
}

/// 1000000 -> 1M, 200000 -> 200k.
fn tokens(n: u64) -> String {
    match n {
        1_000_000.. => format!("{}M", n / 1_000_000),
        1000.. => format!("{}k", n / 1000),
        n => n.to_string(),
    }
}

/// `cells` cells filled by eighths, `█▊░░░` for 37 of 5.
pub fn bar(pct: u32, cells: u32) -> String {
    const PART: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = pct.min(100) * cells * 8 / 100;
    let (full, part) = ((eighths / 8) as usize, PART[(eighths % 8) as usize]);
    let left = cells as usize - full - usize::from(!part.is_empty());
    "█".repeat(full) + part + &"░".repeat(left)
}

/// 3d4h, 2h11m, 14m.
fn eta(s: i64) -> String {
    let (d, h, m) = (s / 86400, s % 86400 / 3600, s % 3600 / 60);
    match (d, h) {
        (1.., _) => format!("{d}d{h}h"),
        (_, 1..) => format!("{h}h{m}m"),
        _ => format!("{}m", m.max(1)),
    }
}

/// 12s, 3m, 2h, 1d.
pub fn age(s: u64) -> String {
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// One line of a resident's text, safe to show: control characters (an escape sequence in a
/// model's reply would reach the host terminal) become spaces, and at most `max` characters.
pub fn clean(s: &str, max: usize) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    line.chars().map(|c| if c.is_control() { ' ' } else { c }).take(max).collect()
}

/// The checked-out branch of the repository holding `dir` (a detached HEAD gives its short
/// hash), read from `HEAD` without running git, and cut to fit where it is shown. Worktrees'
/// `.git` files are followed.
pub fn git_branch(dir: &Path) -> Option<String> {
    git_head(dir).map(|b| clean(&b, 60))
}

/// The same, whole: what a branch is matched by.
pub fn git_head(dir: &Path) -> Option<String> {
    let top = dir.ancestors().find(|d| d.join(".git").exists())?;
    let dot = top.join(".git");
    let git = match std::fs::read_to_string(&dot) {
        Ok(f) => top.join(f.strip_prefix("gitdir:")?.trim()),
        Err(_) => dot,
    };
    let head = std::fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: refs/heads/") {
        Some(b) => clean(b, usize::MAX),
        None => clean(head, 7),
    })
}

/// The repository `dir` is in, as its `origin` remote names it: the path after the host,
/// without `.git` (`owner/repo`, `group/sub/repo`). Read from the common git dir's `config`
/// without running git: a worktree's `.git` file leads to its gitdir, whose `commondir` leads
/// to the repository's own.
pub fn git_repo(dir: &Path) -> Option<String> {
    let top = dir.ancestors().find(|d| d.join(".git").exists())?;
    let dot = top.join(".git");
    let git = match std::fs::read_to_string(&dot) {
        Ok(f) => top.join(f.strip_prefix("gitdir:")?.trim()),
        Err(_) => dot,
    };
    let common = match std::fs::read_to_string(git.join("commondir")) {
        Ok(c) => git.join(c.trim()),
        Err(_) => git,
    };
    let config = std::fs::read_to_string(common.join("config")).ok()?;
    let mut origin = false;
    let url = config.lines().map(str::trim).find_map(|l| {
        if l.starts_with('[') {
            origin = l.replace(' ', "") == "[remote\"origin\"]";
            return None;
        }
        let (k, v) = l.split_once('=')?;
        (origin && k.trim() == "url").then(|| v.trim().to_string())
    })?;
    repo_path(&url)
}

/// `git@host:path(.git)`, `ssh://git@host[:port]/path(.git)` or `https://host/path(.git)`: the
/// path. A local path or anything odd is none.
pub fn repo_path(url: &str) -> Option<String> {
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?.1,
        None => url.split_once(':').filter(|(h, _)| h.contains('@') || !h.contains('/'))?.1,
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_start_matches('/');
    let ok = |c: char| c.is_ascii_alphanumeric() || "._/-".contains(c);
    (!path.is_empty() && path.len() <= 200 && path.chars().all(ok)).then(|| path.to_string())
}

/// The first value at `pointer` along Claude Code's settings chain for `cwd`: the project's
/// `settings.local.json`, its `settings.json`, then the user's.
pub fn setting(cwd: &Path, pointer: &str) -> Option<Value> {
    let user = crate::paths::claude_dir()?;
    let files = [
        cwd.join(".claude/settings.local.json"),
        cwd.join(".claude/settings.json"),
        user.join("settings.json"),
    ];
    files.iter().find_map(|f| {
        let j: Value = serde_json::from_slice(&std::fs::read(f).ok()?).ok()?;
        j.pointer(pointer).filter(|v| !v.is_null()).cloned()
    })
}
