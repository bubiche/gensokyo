//! What a resident is started with: the `claude` argv, the injected `--settings`, and an
//! environment built from scratch.

use serde_json::{Value, json};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Refused in every resident: CronCreate schedules into a store that dies with the session, and
/// with it gone CronList and CronDelete can only ever answer "nothing scheduled".
const DISALLOWED: [&str; 3] = ["CronCreate", "CronList", "CronDelete"];

pub struct Options<'a> {
    /// The resident's id, which its hooks and status line report under.
    pub id: &'a str,
    /// The Claude Code session: `--session-id`, or with `resume` `--resume`.
    pub session: &'a str,
    pub name: &'a str,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub mode: Option<&'a str>,
    pub prompt: Option<&'a str>,
    /// Recall: `--resume <session>` instead of `--session-id <session> --name <name>`.
    pub resume: bool,
    /// A ritual's own flags. They may end in a variadic list, so a flag always follows them.
    pub extra: &'a [String],
    /// A helper's lead, by name.
    pub lead: Option<&'a str>,
}

/// This binary, as the hooks and the status line run it.
pub struct Paths<'a> {
    pub exe: &'a Path,
    pub share: &'a Path,
    /// The socket's realpath, which is what the sandbox allowlist matches.
    pub socket: &'a Path,
}

/// The `claude` to run: `$GENSOKYO_CLAUDE`, else the first on PATH.
pub fn claude(path: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(c) = std::env::var_os("GENSOKYO_CLAUDE") {
        return Some(c.into());
    }
    std::env::split_paths(path?).map(|d| d.join("claude")).find(|p| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// Whether Claude Code kept a conversation for `session`, which it writes only once a prompt is
/// submitted: `--resume` of any other session exits 1 with "No conversation found".
pub fn has_conversation(session: &str) -> bool {
    let Some(projects) = crate::paths::claude_dir().map(|c| c.join("projects")) else {
        return false;
    };
    let file = format!("{session}.jsonl");
    std::fs::read_dir(projects)
        .into_iter()
        .flatten()
        .flatten()
        .any(|d| d.path().join(&file).is_file())
}

/// Hooks, the status line and the socket allowlist: per session, so nothing in `~/.claude`
/// changes. Claude Code merges this over the user's and the project's own settings; the
/// statusLine key is replaced whole, so the user's `padding` is carried over.
pub fn settings(p: &Paths, id: &str, cwd: &Path) -> String {
    let exe = sh_quote(&p.exe.to_string_lossy());
    let cmd = json!({"type": "command", "command": format!("{exe} _hook")});
    let hook = json!([{"hooks": [cmd]}]);
    let ask = json!([{"matcher": "AskUserQuestion", "hooks": [cmd]}]);
    let mut status = json!({"type": "command",
                            "command": format!("{exe} _statusline {}", sh_quote(id))});
    if let Some(pad) = crate::tele::setting(cwd, "/statusLine/padding").filter(Value::is_number) {
        status["padding"] = pad;
    }
    json!({
        "hooks": {
            "SessionStart": hook, "SessionEnd": hook, "UserPromptSubmit": hook,
            "Stop": hook, "StopFailure": hook, "Notification": hook, "PreToolUse": ask,
            "PostToolUse": ask,
        },
        "statusLine": status,
        "sandbox": {"network": {"allowUnixSockets": [p.socket]}},
        "permissions": {"deny": config_rules()},
    })
    .to_string()
}

/// No file tool writes in gensokyo's config dir: a ritual, probe or MCP config there runs later
/// with whatever it says, so it is the user's to write. `gensokyo ritual add` still can, paused.
/// `//` starts an absolute path in a rule; Edit's rules hold for every tool that writes files.
fn config_rules() -> Vec<String> {
    let dir = crate::paths::config_dir();
    let real = std::fs::canonicalize(&dir).ok().filter(|r| *r != dir);
    [Some(dir), real].into_iter().flatten().map(|d| format!("Edit(/{}/**)", d.display())).collect()
}

pub fn argv(p: &Paths, o: &Options, cwd: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["--disallowed-tools".into()];
    // The variadic list ends at the next flag, so it goes first.
    a.extend(DISALLOWED.map(OsString::from));
    a.extend(["--settings".into(), settings(p, o.id, cwd).into()]);
    a.extend(["--plugin-dir".into(), p.share.join("plugin").into_os_string()]);
    a.extend(["--append-system-prompt".into(), system_paragraph(o.name, o.lead).into()]);
    for (flag, v) in [("--model", o.model), ("--effort", o.effort), ("--permission-mode", o.mode)] {
        if let Some(v) = v {
            a.extend([flag.into(), v.into()]);
        }
    }
    a.extend(o.extra.iter().map(OsString::from));
    if o.resume {
        a.extend(["--resume".into(), o.session.into()]);
    } else {
        a.extend(["--session-id".into(), o.session.into(), "--name".into(), o.name.into()]);
        // `--` keeps a prompt that starts with `-` from being read as a flag.
        if let Some(prompt) = o.prompt {
            a.extend(["--".into(), prompt.into()]);
        }
    }
    a
}

/// One paragraph appended to the system prompt. A resident's context is the user's budget, so it
/// says only what no button can: its name, that the others can be written to (through the skill,
/// which is not reached for unless named here), where a standing schedule goes, and how to lead
/// helpers; a helper hears who its lead is instead.
pub fn system_paragraph(name: &str, lead: Option<&str>) -> String {
    let role = match lead {
        None => "To hand work to residents you summon, brief, wait on and close yourself \
                 (helpers), use the `gensokyo-lead` skill."
            .to_string(),
        Some(lead) => format!(
            "You are a helper: the resident {lead} summoned you and gave you your task. When it \
             is done, your last message is your report to {lead}, who reads it as it stands, so \
             make it complete; leave no background task running. You cannot summon residents: \
             use subagents (the Agent tool) to split your own work."
        ),
    };
    format!(
        "You are running inside gensokyo, a cockpit that runs several Claude Code sessions \
         (residents) side by side on this machine; your resident name is {name}. The other \
         sessions in `claude agents` are residents too and you can message them with SendMessage \
         by name, but never compose the first message of an exchange you want an answer to \
         yourself: use the `gensokyo-peers` skill to write it. For any standing or repeating \
         schedule, and to answer any question about what is scheduled or to change one, use the \
         `gensokyo-ritual` skill and never the built-in `schedule` skill, CronCreate or scheduled \
         tasks; a schedule here is read on this machine's own clock, so never convert a time you \
         are given to UTC. {role}"
    )
}

/// The daemon's environment minus what belongs to the terminal or session it was started from,
/// plus what a resident needs. `bin` goes first on PATH so the skills' `gensokyo` resolves to
/// this copy.
pub fn env(
    base: impl IntoIterator<Item = (OsString, OsString)>,
    id: &str,
    bin: &Path,
    socket: &Path,
) -> Vec<(OsString, OsString)> {
    // Whatever names or describes the host terminal: the resident's terminal is ours.
    const PREFIXES: [&str; 9] = [
        "TMUX",
        "TERM_PROGRAM",
        "LC_TERMINAL",
        "ITERM_",
        "KITTY_",
        "GHOSTTY_",
        "ALACRITTY_",
        "WEZTERM_",
        "KONSOLE_",
    ];
    const NAMES: [&str; 13] = [
        "TERM_SESSION_ID",
        "TERM_FEATURES",
        "TERMINFO",
        "TERMINFO_DIRS",
        "COLORFGBG",
        "VTE_VERSION",
        "WT_SESSION",
        "TERMINAL_EMULATOR",
        "__CFBundleIdentifier",
        "COLUMNS",
        "LINES",
        "TERM",
        "COLORTERM",
    ];
    let drop = |k: &str| {
        (k.starts_with("CLAUDE") && k != "CLAUDE_CONFIG_DIR")
            || (k.starts_with("GENSOKYO_")
                && !["GENSOKYO_STATE_DIR", "GENSOKYO_CONFIG_DIR"].contains(&k))
            || PREFIXES.iter().any(|p| k.starts_with(p))
            || NAMES.contains(&k)
    };
    let mut env: Vec<(OsString, OsString)> =
        base.into_iter().filter(|(k, _)| !drop(&k.to_string_lossy())).collect();
    let get = |env: &[(OsString, OsString)], k: &str| {
        env.iter().find(|(n, _)| n == k).map(|(_, v)| v.to_string_lossy().into_owned())
    };
    let utf8 = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|k| get(&env, k).filter(|v| !v.is_empty()))
        .is_some_and(|v| v.to_ascii_lowercase().replace('-', "").contains("utf8"));
    // Joined by hand: `join_paths` refuses the whole list over one entry holding a `:`.
    let mut path = bin.as_os_str().to_owned();
    let old = env.iter().position(|(k, _)| k == "PATH").map(|i| env.remove(i).1);
    for d in old.iter().flat_map(std::env::split_paths) {
        if !d.as_os_str().is_empty() && d != bin {
            path.push(":");
            path.push(d);
        }
    }
    if !utf8 {
        env.retain(|(k, _)| !["LC_ALL", "LC_CTYPE", "LANG"].iter().any(|n| k == n));
        env.push(("LANG".into(), "en_US.UTF-8".into()));
    }
    // Claude Code marks links (OSC 8) only in a terminal it knows has them, and ours passes
    // them on to the host. A value of the user's own stands.
    if get(&env, "FORCE_HYPERLINK").is_none() {
        env.push(("FORCE_HYPERLINK".into(), "1".into()));
    }
    env.extend([
        ("PATH".into(), path),
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("GENSOKYO_RESIDENT".into(), id.into()),
        ("GENSOKYO_SOCKET".into(), socket.into()),
    ]);
    env
}

/// POSIX single quotes, for the hook commands Claude Code hands to a shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
