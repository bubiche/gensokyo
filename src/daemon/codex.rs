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

/// The `codex` to run: `$GENSOKYO_CODEX` unless empty, else the first on PATH that is not in
/// gensokyo's own shim directory, else the ChatGPT app's.
pub fn program(path: Option<&OsStr>) -> Option<PathBuf> {
    let set = std::env::var_os("GENSOKYO_CODEX").filter(|c| !c.is_empty());
    find(set, path, &crate::shim::dir(), Path::new(APP))
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
/// no prompt. A ritual's own flags (`--add-dir`) come just before either. `notify` is the
/// user's and left alone. `env` is what it is started with, for [`shell_env`].
pub fn argv(p: &Paths, o: &Options, env: &[(OsString, OsString)]) -> Result<Vec<OsString>, String> {
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
    // A resumed thread keeps those it started with, whatever these say.
    a.extend(["-c".into(), format!("developer_instructions={}", toml_str(&words)).into()]);
    // Idle or busy, and the thread once there is one, for the daemon to read.
    a.extend([
        "-c".into(),
        r#"tui.terminal_title=["run-state","thread-id","context-used"]"#.into(),
    ]);
    a.extend(shell_env(env).into_iter().map(OsString::from));
    a.extend(o.mode.and_then(mode_flags).into_iter().flatten().map(OsString::from));
    if let Some(m) = o.model {
        a.extend(["-m".into(), m.into()]);
    }
    if let Some(e) = o.effort {
        a.extend(["-c".into(), format!("model_reasoning_effort={e}").into()]);
    }
    a.extend(o.extra.iter().map(OsString::from));
    if o.resume {
        a.extend(["resume".into(), o.session.into()]);
    } else if let Some(prompt) = o.prompt {
        a.extend(["--".into(), prompt.into()]);
    }
    Ok(a)
}

/// `codex exec`'s argv for a headless ritual run, but its prompt, which goes after a `--`: its
/// events as JSONL on stdout, its answer in `answer`. Its mode's flags but the approval policy,
/// which exec has none of: it never asks. No hooks and none of gensokyo's words, as `claude -p`
/// has none: the role's alone, as its instructions. The ritual's directory is added for its
/// notes. `--skip-git-repo-check` always: without it exec refuses a directory Codex has not
/// trusted, and a headless run, Claude's too, starts with no trust dialog to pass. Its commands
/// get gensokyo's variables from `env` as `claude -p`'s do ([`shell_env`]).
pub fn exec(
    r: &crate::ritual::Ritual,
    role: Option<&str>,
    dir: &Path,
    answer: &Path,
    env: &[(OsString, OsString)],
) -> Result<Vec<String>, String> {
    check(r.mode.as_deref(), r.effort.as_deref())?;
    let mut a: Vec<String> = ["exec", "--json", "-o"].map(String::from).into();
    a.push(answer.to_string_lossy().into_owned());
    let mut mode = r.mode.as_deref().and_then(mode_flags).unwrap_or_default().iter();
    while let Some(f) = mode.next() {
        match *f {
            "-a" => _ = mode.next(),
            f => a.push(f.into()),
        }
    }
    if let Some(m) = &r.model {
        a.extend(["-m".into(), m.clone()]);
    }
    if let Some(e) = &r.effort {
        a.extend(["-c".into(), format!("model_reasoning_effort={e}")]);
    }
    if let Some(w) = role {
        a.extend(["-c".into(), format!("developer_instructions={}", toml_str(w))]);
    }
    a.extend(shell_env(env));
    a.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
    a.push("--skip-git-repo-check".into());
    Ok(a)
}

/// `-c shell_environment_policy.set.<name>=<value>` for each of gensokyo's variables in `env`,
/// by name. Codex's hooks see the environment it was started with, but its agent's commands
/// get a filtered one, which leaves these out: a `gensokyo` run there would reach the daemon
/// with no `GENSOKYO_RESIDENT`, and so with the user's rights.
pub fn shell_env(env: &[(OsString, OsString)]) -> Vec<String> {
    let mut ours: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
        .filter(|(k, _)| k.starts_with("GENSOKYO_"))
        .collect();
    ours.sort();
    let set = |(k, v): (String, String)| {
        ["-c".into(), format!("shell_environment_policy.set.{k}={}", toml_str(&v))]
    };
    ours.into_iter().flat_map(set).collect()
}

/// What a `codex exec --json` run's events say: its thread, whether its turn completed or why it
/// failed, what an error event said, and its tokens in and out. Error items are warnings, which
/// a failed turn has too, and a line that is no event is passed over. Items are not read at all:
/// they hold what the run did, commands' output with it, and say nothing of how it ended.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Exec {
    pub thread: Option<String>,
    pub completed: bool,
    pub failed: Option<String>,
    pub error: Option<String>,
    pub tokens: Option<(u64, u64)>,
}

impl Exec {
    pub fn read(jsonl: &str) -> Exec {
        let mut e = Exec::default();
        let events = jsonl.lines().filter(|l| !l.starts_with(r#"{"type":"item."#));
        for v in events.filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()) {
            match v["type"].as_str() {
                Some("thread.started") => e.thread = v["thread_id"].as_str().map(Into::into),
                Some("turn.completed") => {
                    e.completed = true;
                    let n = |k: &str| v["usage"][k].as_u64();
                    e.tokens = n("input_tokens").zip(n("output_tokens"));
                }
                Some("turn.failed") => {
                    let err = &v["error"];
                    let why = err["message"].as_str().or(err.as_str()).unwrap_or("its turn failed");
                    e.failed = Some(inner(why));
                }
                Some("error") => e.error = v["message"].as_str().map(inner),
                _ => {}
            }
        }
        e
    }

    /// Done: its turn completed, none failed, and it exited 0. `code` is none for an exit no
    /// daemon saw, when its events are all there is to go by.
    pub fn ok(&self, code: Option<i32>) -> bool {
        self.completed && self.failed.is_none() && code.is_none_or(|c| c == 0)
    }
}

/// An error's message, out of the API's own error when that is what the message is, as JSON:
/// `{"type":"error","status":400,"error":{"message":"…"}}`.
fn inner(message: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(message).unwrap_or_default();
    v["error"]["message"].as_str().or(v["message"].as_str()).unwrap_or(message).into()
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
/// afresh. A thread is a hex id with dashes, starting with a hex digit, so it never goes into
/// the argv as a flag.
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
            table = Some(named_header(line, "profiles"));
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
    match dotted(k)? {
        (mut k, "") if k.len() == 1 => k.pop(),
        _ => None,
    }
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
/// how Codex writes a trust it was given. Absolute ones only: a relative one would be read from
/// the daemon's own directory. A small scan, not a TOML parser: any other table
/// (`[projects."<dir>".x]` too) ends the one before it, and multi-line strings are skipped.
pub fn projects(config: &str) -> Vec<PathBuf> {
    let (mut found, mut table) = (Vec::new(), None::<String>);
    for line in plain_lines(config) {
        if line.starts_with('[') {
            table = named_header(line, "projects");
            continue;
        }
        let Some((k, v)) = dotted(line) else { continue };
        let v = v.strip_prefix('=').and_then(|v| toml_unquote(v.trim()));
        let trusted = v.is_some_and(|(v, r)| v == "trusted" && scalar(r).is_empty());
        if let Some(t) = table.as_ref().filter(|_| k == ["trust_level"] && trusted) {
            found.push(PathBuf::from(t));
        }
    }
    found.retain(|p| p.is_absolute());
    found
}

/// What a user's `config.toml` has that runs beside gensokyo, by name only, no value kept: the
/// plugins it enables, as `<name>@<marketplace>`; whether it has hooks of its own (under
/// `hooks`, but not the trust Codex keeps in `hooks.state`); and whether it sets `notify` at
/// its top level. And which of `shell_environment_policy`'s `include_only` and `filters` keep
/// gensokyo's variables from its agent's commands, as [`setup`] reads them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Setup {
    pub plugins: Vec<String>,
    pub hooks: bool,
    pub notify: bool,
    pub env_only: Vec<&'static str>,
}

/// `config`'s [`Setup`], by a small scan as `projects` is: each key is read with the table it is
/// in, so `[plugins."<name>@<marketplace>"]` then `enabled = true`, as Codex writes one, is the
/// same as `plugins."<name>@<marketplace>".enabled = true`. Inline tables are not read, but for
/// the first key of a `hooks = {…}` and the strings a `filters = {…}` maps its patterns to.
///
/// Codex keeps only what an `include_only` pattern, or a `filters` one set to `"include"`,
/// matches, and does so after it adds what `set` names: a list of them that matches no
/// `GENSOKYO_RESIDENT` leaves it out of every command, as one that cannot be read does not.
pub fn setup(config: &str) -> Setup {
    let mut s = Setup::default();
    let mut table = Vec::new();
    let mut only = Vec::new();
    for line in plain_lines(config) {
        if line.starts_with('[') {
            // A header not read is a table that is none of these.
            table = header(line).unwrap_or_else(|| vec![String::new()]);
            s.hooks |= hooks_in(&table, "");
            continue;
        }
        let Some((key, v)) = dotted(line) else { continue };
        let Some(v) = v.strip_prefix('=') else { continue };
        let path: Vec<String> = table.iter().cloned().chain(key).collect();
        s.notify |= path == ["notify"];
        s.hooks |= hooks_in(&path, v);
        if let [p, name, e] = &path[..]
            && p == "plugins"
            && e == "enabled"
            && scalar(v) == "true"
            && !s.plugins.contains(name)
        {
            s.plugins.push(name.clone());
        }
        let (list, found) = match &path[..] {
            [p, rest @ ..] if p == "shell_environment_policy" => match rest {
                [l] if l == "include_only" => ("include_only", strings(tail(config, v))),
                [l] if l == "filters" => ("filters", included(tail(config, v))),
                [l, pattern @ ..] if l == "filters" => {
                    let include = toml_unquote(v.trim()).is_some_and(|(v, _)| v == "include");
                    ("filters", Some(include.then(|| pattern.join(".")).into_iter().collect()))
                }
                _ => continue,
            },
            _ => continue,
        };
        let found = found.unwrap_or_default();
        if !found.is_empty() && !s.env_only.contains(&list) {
            s.env_only.push(list);
        }
        only.extend(found);
    }
    if only.iter().any(|p| wild(p, "GENSOKYO_RESIDENT")) {
        s.env_only.clear();
    }
    s
}

/// `config` from where `v`, a slice of it, starts, so a value is read on past its line.
fn tail<'a>(config: &'a str, v: &str) -> &'a str {
    &config[v.as_ptr() as usize - config.as_ptr() as usize..]
}

/// The strings of the array `v` opens; none if any item is no string.
fn strings(v: &str) -> Option<Vec<String>> {
    let (mut s, mut out) = (v.trim_start().strip_prefix('[')?, Vec::new());
    loop {
        s = between(s);
        if s.starts_with(']') {
            return Some(out);
        }
        let (item, rest) = toml_unquote(s)?;
        out.push(item);
        s = rest;
    }
}

/// The keys of the inline table `v` opens that it maps to `"include"`; none if any value is no
/// string.
fn included(v: &str) -> Option<Vec<String>> {
    let (mut s, mut out) = (v.trim_start().strip_prefix('{')?, Vec::new());
    loop {
        s = between(s);
        if s.starts_with('}') {
            return Some(out);
        }
        let (key, rest) = dotted(s)?;
        let (value, rest) = toml_unquote(rest.strip_prefix('=')?.trim_start())?;
        if value == "include" {
            out.push(key.join("."));
        }
        s = rest;
    }
}

/// `s` past the spaces, commas and comments between one item and the next.
fn between(mut s: &str) -> &str {
    loop {
        s = s.trim_start().trim_start_matches(',');
        match s.strip_prefix('#') {
            Some(c) => s = c.split_once('\n').map_or("", |(_, r)| r),
            None if s.starts_with(|c: char| c.is_whitespace()) => {}
            None => return s,
        }
    }
}

/// Whether `name` matches `pattern` as Codex matches a variable's: `*` for any run, `?` for any
/// one character, in any case.
pub fn wild(pattern: &str, name: &str) -> bool {
    let up = |s: &str| s.chars().map(|c| c.to_ascii_uppercase()).collect::<Vec<_>>();
    let (p, n) = (up(pattern), up(name));
    // Where the last `*` was, and how much of `name` it has taken so far.
    let (mut i, mut j, mut star) = (0, 0, None);
    while j < n.len() {
        match p.get(i) {
            Some('*') => (star, i) = (Some((i, j)), i + 1),
            Some(&c) if c == '?' || c == n[j] => (i, j) = (i + 1, j + 1),
            _ => match star {
                Some((at, took)) => (star, i, j) = (Some((at, took + 1)), at + 1, took + 1),
                None => return false,
            },
        }
    }
    p[i..].iter().all(|&c| c == '*')
}

/// Whether the key or table at `path`, its value `v`, holds hooks: anything under `hooks` but
/// `hooks.state`, and a `hooks = {…}` by its first key.
fn hooks_in(path: &[String], v: &str) -> bool {
    match path {
        [h, e, ..] => h == "hooks" && e != "state",
        [h] => {
            let first = v.trim_start().strip_prefix('{').and_then(dotted);
            h == "hooks" && first.is_some_and(|(k, _)| k[0] != "state")
        }
        [] => false,
    }
}

/// A value with its comment cut off.
fn scalar(v: &str) -> &str {
    v.split('#').next().unwrap_or("").trim()
}

/// A key's parts, each bare or quoted, joined by dots, and what follows it, trimmed.
fn dotted(s: &str) -> Option<(Vec<String>, &str)> {
    let (mut parts, mut s) = (Vec::new(), s.trim_start());
    loop {
        let (part, rest) = match toml_unquote(s) {
            Some(q) => q,
            None => {
                let bare = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
                let end = s.find(|c| !bare(c)).unwrap_or(s.len());
                (end > 0).then(|| (s[..end].to_string(), &s[end..]))?
            }
        };
        parts.push(part);
        s = rest.trim_start();
        match s.strip_prefix('.') {
            Some(r) => s = r.trim_start(),
            None => return Some((parts, s)),
        }
    }
}

/// The parts of the table a `[...]` or `[[...]]` header names.
fn header(line: &str) -> Option<Vec<String>> {
    let (inner, end) = match line.strip_prefix("[[") {
        Some(inner) => (inner, "]]"),
        None => (line.strip_prefix('[')?, "]"),
    };
    let (parts, rest) = dotted(inner)?;
    let rest = rest.strip_prefix(end)?.trim();
    (rest.is_empty() || rest.starts_with('#')).then_some(parts)
}

/// Whether `plugin`, `<name>@<marketplace>`, ships hooks in any version `home` caches in
/// `plugins/cache/<marketplace>/<name>/<version>/`: a `hooks/hooks.json` that holds any, or a
/// `hooks` in its `.codex-plugin/plugin.json`, a path or the hooks themselves. Only those two
/// files are opened, and no link is followed to a version.
pub fn plugin_hooks(home: &Path, plugin: &str) -> bool {
    let Some((name, market)) = plugin.rsplit_once('@') else { return false };
    if [name, market].iter().any(|p| p.is_empty() || p.starts_with('.') || p.contains('/')) {
        return false;
    }
    let versions = std::fs::read_dir(home.join("plugins/cache").join(market).join(name));
    let versions = versions.into_iter().flatten().flatten();
    let mut dirs = versions.filter(|v| v.file_type().is_ok_and(|t| t.is_dir())).map(|v| v.path());
    dirs.any(|v| {
        let manifest = json(&v.join(".codex-plugin/plugin.json"), false);
        let hooks = json(&v.join("hooks/hooks.json"), false);
        hooks.is_some_and(|h| holds(&h)) || manifest.is_some_and(|m| holds(&m["hooks"]))
    })
}

/// Whether `file`, a hooks.json of the user's own, holds any hooks; a link to one is followed.
pub fn hooks_file(file: &Path) -> bool {
    json(file, true).is_some_and(|h| holds(&h))
}

/// `file` as JSON, if it is a regular file, a link to one only when `follow`: a pipe there would
/// never end.
fn json(file: &Path, follow: bool) -> Option<serde_json::Value> {
    let meta = if follow { std::fs::metadata(file) } else { std::fs::symlink_metadata(file) };
    meta.ok().filter(std::fs::Metadata::is_file)?;
    serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
}

/// Whether hooks as JSON have any in them: a path, or what its `hooks` (or itself) holds.
fn holds(h: &serde_json::Value) -> bool {
    use serde_json::Value::*;
    match h.get("hooks").unwrap_or(h) {
        Null | Bool(false) => false,
        String(s) => !s.is_empty(),
        Array(a) => !a.is_empty(),
        Object(o) => !o.is_empty(),
        _ => true,
    }
}

/// `config`'s lines, trimmed, but those that open a multi-line string and those inside one or
/// inside an array or inline table a line before opened.
fn plain_lines(config: &str) -> impl Iterator<Item = &str> {
    let mut state = (None, 0);
    config.lines().filter_map(move |line| {
        let start = state;
        state = scan(line, state);
        (start == (None, 0) && state.0.is_none()).then(|| line.trim())
    })
}

/// The multi-line string open at a line's end, and how many brackets.
type Scan = (Option<&'static str>, u32);

/// The [`Scan`] after `line`, given the one as it starts: strings and comments are passed over
/// as TOML reads them.
fn scan(line: &str, (mut open, mut depth): Scan) -> Scan {
    let mut rest = line;
    loop {
        if let Some(q) = open {
            let Some(i) = close(rest, q) else { return (open, depth) };
            (open, rest) = (None, &rest[i + q.len()..]);
            continue;
        }
        let Some(i) = rest.find(['#', '"', '\'', '[', ']', '{', '}']) else { return (None, depth) };
        let (c, after) = (rest.as_bytes()[i], &rest[i + 1..]);
        rest = after;
        match c {
            b'#' => return (None, depth),
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {
                let q = if c == b'"' { "\"\"\"" } else { "'''" };
                if after.starts_with(&q[1..]) {
                    (open, rest) = (Some(q), &after[2..]);
                } else {
                    let Some(j) = close(after, &q[..1]) else { return (None, depth) };
                    rest = &after[j + 1..];
                }
            }
        }
    }
}

/// Where `q` closes the string `s` starts inside; in a basic one a `\` escapes what follows.
fn close(s: &str, q: &str) -> Option<usize> {
    let (s, mut i) = (s.as_bytes(), 0);
    while i < s.len() {
        match s[i] {
            b'\\' if q.starts_with('"') => i += 2,
            _ if s[i..].starts_with(q.as_bytes()) => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// The name a `[<table>.<name>]` header gives; none for any other header.
fn named_header(line: &str, table: &str) -> Option<String> {
    match header(line).filter(|_| !line.starts_with("[["))?.as_slice() {
        [t, name] if t == table => Some(name.clone()),
        _ => None,
    }
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
