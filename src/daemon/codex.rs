//! What a Codex resident is started with: the `codex` to run, its argv (hooks, gensokyo's words
//! and the mode as `-c`s and flags, nothing written to `~/.codex`), and whether Codex trusts the
//! directory it starts in.

use super::launch::{Options, Paths, executable, on_path, sh_quote};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The ChatGPT app's own copy, for when there is no other.
pub const APP: &str = "/Applications/ChatGPT.app/Contents/Resources/codex";

/// The hooks a Codex resident's state comes from.
pub const HOOKS: [&str; 7] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "Stop",
    "PermissionRequest",
    "PostToolUse",
    "Interrupt",
];

/// gensokyo's modes as Codex's flags. Codex cannot ask before an edit only, so plan and default
/// both read only and ask for the rest; auto has Codex's reviewer answer its requests.
pub const MODES: [(&str, &[&str]); 5] = [
    ("plan", &["-s", "read-only", "-a", "on-request"]),
    ("default", &["-s", "read-only", "-a", "on-request"]),
    ("acceptEdits", &["-s", "workspace-write", "-a", "on-request"]),
    ("auto", &["--approve-for-me"]),
    ("bypassPermissions", &["--dangerously-bypass-approvals-and-sandbox"]),
];

/// Codex's flags for gensokyo's `mode`; none for a mode it has no flags for.
pub fn mode_flags(mode: &str) -> Option<&'static [&'static str]> {
    MODES.iter().find(|(m, _)| *m == mode).map(|(_, f)| *f)
}

/// The mode a launch argv runs in, by its flags before any `--`: plan's are default's, so it
/// reads back as default.
pub fn mode_of(argv: &[String]) -> Option<&'static str> {
    let flags: Vec<&str> = argv.iter().map(String::as_str).take_while(|a| *a != "--").collect();
    MODES
        .iter()
        .filter(|(m, _)| *m != "plan")
        .find_map(|(m, f)| flags.windows(f.len()).any(|w| w == *f).then_some(*m))
}

/// Model, effort, mode and config profile, as a launch argv has them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Launched {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub profile: Option<String>,
}

pub fn launched(argv: &[String]) -> Launched {
    let flags: Vec<&str> = argv.iter().map(String::as_str).take_while(|a| *a != "--").collect();
    let pairs = || flags.windows(2).map(|w| (w[0], w[1]));
    let effort =
        pairs().find_map(|(f, v)| v.strip_prefix("model_reasoning_effort=").filter(|_| f == "-c"));
    let profile = pairs().find(|(f, _)| ["-p", "--profile"].contains(f)).map(|(_, v)| v);
    let profile = profile.or_else(|| flags.iter().find_map(|f| f.strip_prefix("--profile=")));
    Launched {
        model: pairs().find(|(f, _)| *f == "-m").map(|(_, v)| v.into()),
        effort: effort.map(Into::into),
        mode: mode_of(argv).map(String::from),
        profile: profile.map(Into::into),
    }
}

/// What a launch or a `say` is told when `program` finds none.
pub const MISSING: &str = "codex not found: not on PATH, in GENSOKYO_CODEX or in the ChatGPT app";

/// The `codex` to run: `$GENSOKYO_CODEX`, else the first on PATH that is not in gensokyo's own
/// shim directory, else the ChatGPT app's.
pub fn program(path: Option<&OsStr>) -> Option<PathBuf> {
    find(std::env::var_os("GENSOKYO_CODEX"), path, &crate::shim::dir(), Path::new(APP))
}

/// `set` when given, else the first `codex` on `path` outside `skip`, else `app` if it runs.
pub fn find(
    set: Option<OsString>,
    path: Option<&OsStr>,
    skip: &Path,
    app: &Path,
) -> Option<PathBuf> {
    if let Some(c) = set {
        return Some(c.into());
    }
    on_path(path, skip, "codex").or_else(|| executable(app).then(|| app.to_path_buf()))
}

/// The argv for a start, or why there is none: a mode Codex has no flags for, or an effort that
/// is not one word. A fresh start's prompt goes last, after `--`, so one that starts with `-`
/// stays whole; with `resume`, `resume <thread>` goes last instead, after the same flags, and
/// no prompt. `notify` is the user's and left alone.
pub fn argv(p: &Paths, o: &Options) -> Result<Vec<OsString>, String> {
    check(o.mode, o.effort)?;
    // gensokyo's hooks run unreviewed: Codex would otherwise hold each new one for the user.
    let mut a: Vec<OsString> = vec!["--dangerously-bypass-hook-trust".into()];
    let cmd = toml_str(&format!("{} _hook", sh_quote(&p.exe.to_string_lossy())));
    for e in HOOKS {
        a.push("-c".into());
        a.push(format!("hooks.{e}=[{{hooks=[{{type=\"command\",command={cmd}}}]}}]").into());
    }
    let sandboxed = o.mode != Some("bypassPermissions");
    let mut words = paragraph(o.name, o.lead, o.ledger, sandboxed);
    for s in [o.role, o.system_prompt].into_iter().flatten() {
        words = format!("{words}\n\n{s}");
    }
    a.extend(["-c".into(), format!("developer_instructions={}", toml_str(&words)).into()]);
    // Idle or busy, and the thread once there is one, for the daemon to read.
    a.extend([
        "-c".into(),
        r#"tui.terminal_title=["run-state","thread-id","context-used"]"#.into(),
    ]);
    a.extend(o.mode.and_then(mode_flags).into_iter().flatten().map(OsString::from));
    if let Some(m) = o.model {
        a.extend(["-m".into(), m.into()]);
    }
    if let Some(e) = o.effort {
        a.extend(["-c".into(), format!("model_reasoning_effort={e}").into()]);
    }
    if o.resume {
        a.extend(["resume".into(), o.session.into()]);
    } else if let Some(prompt) = o.prompt {
        a.extend(["--".into(), prompt.into()]);
    }
    Ok(a)
}

/// `codex queue`'s argv: `text` as the next prompt of `thread`, once it is idle. Each value
/// joined to its flag, so text that starts with `-` is never taken for a flag.
pub fn queue(thread: &str, text: &str) -> Vec<OsString> {
    vec!["queue".into(), format!("--thread={thread}").into(), format!("--message={text}").into()]
}

/// Why a Codex resident cannot start in `mode` with `effort`, if it cannot.
pub fn check(mode: Option<&str>, effort: Option<&str>) -> Result<(), String> {
    if let Some(m) = mode.filter(|m| mode_flags(m).is_none()) {
        let all: Vec<&str> = MODES.iter().map(|(m, _)| *m).collect();
        return Err(format!("Codex has no {m} mode: {}", all.join(", ")));
    }
    match effort.filter(|e| !one_word(e)) {
        Some(e) => Err(format!("effort {e}: Codex takes one word (low, medium, high…)")),
        None => Ok(()),
    }
}

fn one_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// gensokyo's paragraph for a Codex resident, before the role's words and the user's: its name;
/// a helper hears who leads it and how its report and its questions reach them. One in Codex's
/// sandbox, which keeps git's files read-only, hears to leave its changes uncommitted, and a
/// helper to tell its lead what they are. A role that keeps a ledger hears where it is.
pub fn paragraph(name: &str, lead: Option<&str>, ledger: Option<&Path>, sandboxed: bool) -> String {
    let tell = lead.map_or("", |_| ", and say in your report what you changed");
    let git = match sandboxed {
        true => format!(
            " Your sandbox keeps git's own files read-only, so `git add` and `git commit` fail \
             in it: leave your changes uncommitted{tell}."
        ),
        false => String::new(),
    };
    let lead = lead.map_or(String::new(), |lead| {
        format!(
            " You are a helper: the resident {lead} summoned you and gave you your task. When it \
             is done, your last message is your report to {lead}, who reads it as it stands, so \
             make it complete; leave no background task running. To ask {lead} something, end \
             your turn with the question: one asked any other way goes to the user."
        )
    });
    let ledger = ledger.map_or(String::new(), |f| {
        format!(
            " This project's ledger is `{}`: a file that outlasts this conversation and this \
             session, for whoever holds your role here next. Read it before you start, and keep \
             it as your role says.",
            f.display()
        )
    });
    format!(
        "You are running inside gensokyo, which runs several coding agents (residents) side by \
         side on this machine; your resident name is {name}.{lead}{git}{ledger}"
    )
}

/// `s` as a TOML basic string, quotes included: what `-c key=value` reads.
pub fn toml_str(s: &str) -> String {
    let mut q = String::with_capacity(s.len() + 2);
    q.push('"');
    for c in s.chars() {
        match c {
            '\\' => q.push_str(r"\\"),
            '"' => q.push_str("\\\""),
            '\n' => q.push_str(r"\n"),
            '\t' => q.push_str(r"\t"),
            c if c.is_control() && (c as u32) < 0x80 => q.push_str(&format!("\\u{:04X}", c as u32)),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

/// The TOML string `s` starts with, basic or literal, and what follows it.
pub fn toml_unquote(s: &str) -> Option<(String, &str)> {
    let mut chars = s.char_indices();
    let open = chars.next()?.1;
    let mut out = String::new();
    if open == '\'' {
        let end = s[1..].find('\'')? + 1;
        return Some((s[1..end].to_string(), &s[end + 1..]));
    }
    if open != '"' {
        return None;
    }
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &s[i + 1..])),
            '\\' => {
                let e = chars.next()?.1;
                out.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    '"' | '\\' => e,
                    'u' | 'U' => {
                        let n = if e == 'u' { 4 } else { 8 };
                        let hex: String =
                            (0..n).filter_map(|_| chars.next().map(|c| c.1)).collect();
                        char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
                    }
                    _ => return None,
                });
            }
            c => out.push(c),
        }
    }
    None
}

/// `$CODEX_HOME`, else `~/.codex`.
pub fn home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME").filter(|h| !h.is_empty()) {
        Some(h) => Some(h.into()),
        None => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")),
    }
}

/// Whether Codex keeps `thread` in `home`, for `codex resume`: a rollout in
/// `sessions/<year>/<month>/<day>/`, named `rollout-<YYYY-MM-DDThh-mm-ss>-<thread>.jsonl` as
/// Codex writes one. Names only, at exactly that depth: nothing is opened, nothing deeper looked
/// at. A symlinked year, month or day directory, or rollout, is skipped: the resident starts
/// afresh. A thread is a hex id with dashes, starting with a digit, so it never goes into the
/// argv as a flag.
pub fn has_thread(home: &Path, thread: &str) -> bool {
    let hex = thread.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    if !(hex && thread.starts_with(|c: char| c.is_ascii_hexdigit())) {
        return false;
    }
    let ours = |n: &str| {
        let rest = n.strip_prefix("rollout-").and_then(|r| r.strip_suffix(".jsonl"));
        let stamp = rest.and_then(|r| r.strip_suffix(thread)?.strip_suffix('-'));
        stamp.is_some_and(|s| s.len() == 19)
    };
    let kids = |d: &Path, dir: bool| {
        let all = std::fs::read_dir(d).into_iter().flatten().flatten();
        all.filter(move |e| e.file_type().is_ok_and(|t| if dir { t.is_dir() } else { t.is_file() }))
    };
    let days = kids(&home.join("sessions"), true)
        .flat_map(|y| kids(&y.path(), true))
        .flat_map(|m| kids(&m.path(), true));
    days.flat_map(|d| kids(&d.path(), false)).any(|f| ours(&f.file_name().to_string_lossy()))
}

/// The model a Codex launch runs: its `-m`, else the one the `config.toml` in `home` sets for
/// it, read only. Only that user config is read: a project's `.codex/` and any managed layer are
/// not, so a model set there is not known. Made fit to show as Claude's is: one line, no
/// control characters, at most 40.
pub fn model(argv: &[String], home: Option<&Path>) -> Option<String> {
    let launch = launched(argv);
    let m = launch.model.or_else(|| {
        let config = std::fs::read_to_string(home?.join("config.toml")).ok()?;
        config_model(&config, launch.profile.as_deref())
    })?;
    Some(crate::tele::clean(&m, 40)).filter(|m| !m.is_empty())
}

/// The model `config` sets: its `[profiles.<name>]` table's, for `profile` or else the
/// `profile = "…"` at its top level, then the `model = "…"` at its top level. A small scan, as
/// `projects` is: dotted keys and inline tables are not read.
pub fn config_model(config: &str, profile: Option<&str>) -> Option<String> {
    // The table each line is in: none at the top, a profile's name, or `Some(None)` for another.
    let mut table = None::<Option<String>>;
    let (mut top, mut chosen, mut theirs) = (None, None, Vec::new());
    for line in plain_lines(config) {
        if line.starts_with('[') {
            table = Some(profile_header(line));
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let Some((v, rest)) = toml_unquote(v.trim()) else { continue };
        let rest = rest.trim();
        if !(rest.is_empty() || rest.starts_with('#')) {
            continue;
        }
        match (&table, bare_key(k).as_deref()) {
            (None, Some("model")) => top = top.or(Some(v)),
            (None, Some("profile")) => chosen = chosen.or(Some(v)),
            (Some(Some(p)), Some("model")) => theirs.push((p.clone(), v)),
            _ => {}
        }
    }
    let profile = profile.map(String::from).or(chosen);
    let mine = profile.and_then(|p| theirs.into_iter().find(|(n, _)| *n == p).map(|(_, m)| m));
    mine.or(top)
}

/// A key as written, `model` or `"model"`; none for a dotted one.
fn bare_key(k: &str) -> Option<String> {
    let k = k.trim();
    match toml_unquote(k) {
        Some((k, rest)) => rest.trim().is_empty().then_some(k),
        None => {
            k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-').then(|| k.into())
        }
    }
}

/// The name a `[profiles.<name>]` header gives, bare or quoted; none for any other header.
fn profile_header(line: &str) -> Option<String> {
    let rest = line.strip_prefix('[')?.trim_start();
    let rest = rest.strip_prefix("profiles")?.trim_start().strip_prefix('.')?.trim_start();
    let (name, rest) = match toml_unquote(rest) {
        Some(q) => q,
        None => {
            let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'));
            let (name, rest) = rest.split_at(end.unwrap_or(rest.len()));
            (name.to_string(), rest)
        }
    };
    let rest = rest.trim_start().strip_prefix(']')?.trim();
    (!name.is_empty() && (rest.is_empty() || rest.starts_with('#'))).then_some(name)
}

/// The context used, percent, as the last part of its title says: `Context 12% used`.
pub fn context(title: &str) -> Option<u32> {
    let last = title.rsplit(" | ").next()?;
    last.strip_prefix("Context ")?.strip_suffix("% used")?.parse().ok()
}

/// Whether Codex starts in `dir` with no trust prompt, by its `config.toml` in `home`, read only.
pub fn trusted(home: &Path, dir: &Path) -> bool {
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap_or_default();
    let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.into());
    trusted_in(&config, &real, main_root(&real).as_deref())
}

/// Trusted by `config`: its own entry, or its main checkout's when `cwd` (a real path) is inside
/// that. Never a directory further up: Codex may map a worktree to its main checkout, or match
/// any parent, and only these two are trusted either way.
pub fn trusted_in(config: &str, cwd: &Path, root: Option<&Path>) -> bool {
    projects(config).into_iter().any(|p| {
        let p = std::fs::canonicalize(&p).unwrap_or(p);
        p == cwd || (Some(p.as_path()) == root && cwd.starts_with(&p))
    })
}

/// The directories `config` has `trust_level = "trusted"` for, each as `[projects."<dir>"]`:
/// how Codex writes a trust it was given. A small scan, not a TOML parser: any other table
/// (`[projects."<dir>".x]` too) ends the one before it, and multi-line strings are skipped.
pub fn projects(config: &str) -> Vec<PathBuf> {
    let (mut found, mut table) = (Vec::new(), None::<String>);
    for line in plain_lines(config) {
        if line.starts_with('[') {
            table = project_header(line);
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = toml_unquote(v.trim());
        let rest = v.as_ref().map(|(_, r)| r.trim());
        let trusted = v.as_ref().is_some_and(|(v, _)| v == "trusted")
            && rest.is_some_and(|r| r.is_empty() || r.starts_with('#'));
        if let Some(t) = table.as_ref().filter(|_| k.trim() == "trust_level" && trusted) {
            found.push(PathBuf::from(t));
        }
    }
    found
}

/// `config`'s lines, trimmed, but those inside a multi-line string.
fn plain_lines(config: &str) -> impl Iterator<Item = &str> {
    let mut long = None::<&str>;
    config.lines().filter_map(move |line| {
        if let Some(q) = long {
            if line.matches(q).count() % 2 == 1 {
                long = None;
            }
            return None;
        }
        if let Some(q) = ["\"\"\"", "'''"].into_iter().find(|q| line.matches(q).count() % 2 == 1) {
            long = Some(q);
            return None;
        }
        Some(line.trim())
    })
}

/// The directory a `[projects."<dir>"]` header names; none for any other header.
fn project_header(line: &str) -> Option<String> {
    let rest = line.strip_prefix('[')?.trim_start();
    if rest.starts_with('[') {
        return None;
    }
    let rest = rest.strip_prefix("projects")?.trim_start().strip_prefix('.')?.trim_start();
    let (dir, rest) = toml_unquote(rest)?;
    let rest = rest.trim_start().strip_prefix(']')?.trim();
    (rest.is_empty() || rest.starts_with('#')).then_some(dir)
}

/// The main checkout `dir` belongs to: the directory holding the common git dir, read from the
/// files as git leaves them (a worktree's `.git` names its gitdir, whose `commondir` names the
/// repository's own) rather than by running git.
pub fn main_root(dir: &Path) -> Option<PathBuf> {
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
    std::fs::canonicalize(common).ok()?.parent().map(Path::to_path_buf)
}
