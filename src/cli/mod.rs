//! The plumbing verbs: one request over the socket, starting the daemon when nobody answers.

mod conn;
mod doctor;
mod install;
mod login;
mod rituals;

pub use conn::{Error, connect_or_start, peer_pid, refusal, request};

use crate::paths;
use crate::proto::{Cast, Reply, Request, Summon, Until, Wait, WorktreeAsk};
use clap::{Parser, Subcommand};
use std::process::ExitCode;

/// Several Claude Code sessions side by side, one shown at a time.
///
/// With no command, gensokyo opens the shrine: every resident in a sidebar, the one you pick
/// beside it, and Ctrl-] then a key for the rest (Ctrl-] ? lists them).
#[derive(Parser)]
#[command(name = "gensokyo", version, max_term_width = 100)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Who is in the shrine: slot, state, directory and what each is running on
    #[command(visible_alias = "ls")]
    List {
        /// As JSON, for scripts
        #[arg(long)]
        json: bool,
        /// Departed residents too
        #[arg(long)]
        all: bool,
    },
    /// Summon a resident: Claude Code in a directory of its own
    New(Box<New>),
    /// Bring a departed resident back into its old conversation
    #[command(visible_alias = "recall")]
    Resume {
        /// A name, a slot or an id
        who: String,
    },
    /// Start residents again on the claude installed now, each into its own conversation
    ///
    /// Each goes once it rests (no turn, dialog or half-typed prompt), and keeps its name, slot,
    /// lead and screen. With nobody named, everyone `list` marks ⇡: behind the installed claude.
    /// What lives only in the session goes: background tasks, a Monitor, a /loop.
    Renew {
        /// Names, slots or ids
        who: Vec<String>,
    },
    /// Hang up on a resident; it stays in the shrine as departed, to resume or close
    Banish {
        /// A name, a slot or an id
        who: String,
    },
    /// Ask a resident to /exit; a departed one leaves the shrine
    Close {
        /// A name, a slot or an id
        who: String,
    },
    /// Hold until residents have news (a turn ended, a dialog opened, or gone), then one JSON line each
    ///
    /// From inside a resident, about its own helpers: news since it was last told of them, by a
    /// wait that was met or a read. A departure is news once; a helper it was told of after that
    /// is left out. Exit 0 once the news came, 3 on the timeout, 4 when the daemon went away
    /// (a restart: wait again).
    Wait {
        /// Names, slots or ids
        #[arg(required = true)]
        who: Vec<String>,
        /// Back when any one of them has news, rather than each
        #[arg(long)]
        any: bool,
        /// Only this kind of news counts
        #[arg(long, value_enum)]
        until: Option<Until>,
        /// The longest to wait: 90s, 30m, 2h
        #[arg(long, default_value = "30m")]
        timeout: String,
    },
    /// What a resident answered last, kept after it departs; --screen for its screen as text
    Read {
        /// A name, a slot or an id
        who: String,
        /// Its screen now, instead
        #[arg(long)]
        screen: bool,
    },
    /// Cast a spell card at residents; with no card, list the cards
    #[command(visible_alias = "cast")]
    Broadcast {
        /// A card's name or title, or a part of one that picks out a single card
        card: Option<String>,
        /// all, awaiting, idle, or residents by name, slot or id
        targets: Vec<String>,
        /// For a pair card: the resident each target talks to
        #[arg(long, value_name = "NAME")]
        with: Option<String>,
    },
    /// Prompts on a schedule: list, add, run, pause and read what they did
    Ritual {
        #[command(subcommand)]
        cmd: Option<rituals::Cmd>,
    },
    /// Tag a resident with short key=value tokens, shown under it in the sidebar; alone, its tags
    ///
    /// key= removes one. At most 4, in the order first set; a key is up to 12 of a-z, 0-9, _
    /// and -; a value up to 24 letters, digits and _.:/#@-. From inside a resident, only itself
    /// and its own helpers.
    Tag {
        /// A name, a slot or an id
        who: String,
        /// key=value, or key= to remove it
        tags: Vec<String>,
        /// Remove every tag first
        #[arg(long)]
        clear: bool,
    },
    /// Ask every resident to /exit, then stop the daemon
    Quit,
    /// Keep the Mac from idle-sleeping while residents work; alone, whether it does and for whom
    ///
    /// On by default. A resident in a turn or a headless ritual run holds the Mac awake, with
    /// caffeinate -i, until 30 s after the work is done; a dialog, a question or a schedule
    /// does not. The display still sleeps, and a closed lid still sleeps the Mac. Kept in the
    /// config as KEEP_AWAKE; Ctrl-] w in the shrine does the same.
    Awake {
        #[arg(value_parser = ["on", "off"])]
        to: Option<String>,
    },
    /// Replace the running daemon, whatever its build, and bring its residents back
    ///
    /// Each is asked to /exit, the daemon stops, this binary's daemon starts, and each is
    /// recalled into its own conversation. A turn in progress is cut short.
    Restart,
    /// Which binary, which claude, the daemon, the login agent, and what may be wrong
    Doctor,
    /// Start the daemon when you log in; alone, whether it does
    Login {
        #[command(subcommand)]
        cmd: Option<login::Cmd>,
    },
    /// Swap this install for the latest release (or --version's)
    Update {
        /// Only say whether a newer release is out
        #[arg(long)]
        check: bool,
        /// That release instead of the latest
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,
    },
    /// Remove this install, its link and the login agent; asks before your config and state
    Uninstall {
        /// Remove without asking
        #[arg(short, long)]
        yes: bool,
        /// Keep the config, the rituals and the records
        #[arg(long)]
        keep_data: bool,
    },
    /// Print the version
    Version,
}

#[derive(clap::Args)]
struct New {
    /// Where it works; the current directory when left out
    dir: Option<String>,
    /// Its name; otherwise one from the shipped list
    #[arg(short, long)]
    name: Option<String>,
    /// The model it runs on (claude --model)
    #[arg(short, long)]
    model: Option<String>,
    /// Its effort (claude --effort)
    #[arg(short, long)]
    effort: Option<String>,
    /// Its permission mode (claude --permission-mode)
    #[arg(short = 'p', long = "permission-mode", value_name = "MODE")]
    mode: Option<String>,
    /// Its first prompt
    #[arg(long, allow_hyphen_values = true)]
    prompt: Option<String>,
    /// Its first prompt from a file, or with - from stdin: any number of lines
    #[arg(long, value_name = "FILE", conflicts_with = "prompt")]
    prompt_file: Option<String>,
    /// A tool it may use without asking (claude --allowedTools); again for more
    #[arg(long = "allowed-tools", value_name = "TOOL")]
    allowed_tools: Vec<String>,
    /// Appended to its system prompt: a role by name (reviewer, implementer, chief-of-staff,
    /// researcher, debugger, or one of yours in roles/), or a file by its path
    #[arg(long, value_name = "NAME|FILE")]
    role: Option<String>,
    /// Your own words appended to its system prompt, with the role if there is one
    #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
    system_prompt: Option<String>,
    /// Print the new resident as JSON: id, name, slot and the rest `list --json` gives
    #[arg(long)]
    json: bool,
    /// Work in a checkout of its own, <repo>/.claude/worktrees/<SLUG>, made or found first
    #[arg(long, value_name = "SLUG")]
    worktree: Option<String>,
    /// The worktree's branch; BRANCH_PREFIX and the slug when left out
    #[arg(long, requires = "worktree")]
    branch: Option<String>,
    /// What a new branch starts from; the remote's default branch, fetched, when left out
    #[arg(long, value_name = "REF", requires = "worktree")]
    base: Option<String>,
}

pub fn main(args: &[String]) -> ExitCode {
    let cli = match Cli::try_parse_from(
        std::iter::once("gensokyo".to_string()).chain(args.iter().cloned()),
    ) {
        Ok(c) => c,
        // Help and --version print and succeed; a usage error prints and exits 2.
        Err(e) => e.exit(),
    };
    let r = match cli.cmd {
        Cmd::List { json, all } => list(json, all),
        Cmd::New(n) => new(*n),
        Cmd::Resume { who } => resume(&who),
        Cmd::Renew { who } => say(request(Request::Renew { who }, false)),
        Cmd::Banish { who } => say(request(Request::Banish { who }, false)),
        Cmd::Close { who } => say(request(Request::Close { who }, false)),
        Cmd::Broadcast { card, targets, with } => broadcast(card, targets, with),
        Cmd::Wait { who, any, until, timeout } => return wait(who, any, until, &timeout),
        Cmd::Read { who, screen } => {
            say(request(Request::Read { who, screen, bare: false }, false))
        }
        Cmd::Ritual { cmd } => rituals::main(cmd),
        Cmd::Tag { who, tags, clear } => {
            say(request(Request::Tag { who, set: tags, clear }, false))
        }
        Cmd::Quit => match request(Request::Quit, false) {
            Err(Error::NotRunning) => Ok(()),
            r => say(r),
        },
        Cmd::Awake { to } => awake(to.map(|t| t == "on")),
        Cmd::Restart => conn::restart(),
        Cmd::Doctor => doctor::main(),
        Cmd::Login { cmd } => login::main(cmd),
        Cmd::Update { check, version } => install::update(check, version),
        Cmd::Uninstall { yes, keep_data } => install::uninstall(yes, keep_data),
        Cmd::Version => {
            println!("gensokyo {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gensokyo: {e}");
            ExitCode::FAILURE
        }
    }
}

fn say(r: Result<Reply, Error>) -> Result<(), String> {
    match r? {
        Reply::Done { message, .. } => {
            println!("{message}");
            Ok(())
        }
        other => Err(format!("unexpected reply {other:?}")),
    }
}

/// Asked of the daemon, which holds the Mac; with none running, set in the config for the next.
fn awake(on: Option<bool>) -> Result<(), String> {
    if on.is_some() && std::env::var_os("GENSOKYO_RESIDENT").is_some_and(|v| !v.is_empty()) {
        return Err("keeping the Mac awake is the user's to turn on or off".into());
    }
    match request(Request::Awake { on }, false) {
        Err(Error::NotRunning) => {
            if let Some(on) = on {
                let v = if on { "on" } else { "off" };
                paths::set_config("KEEP_AWAKE", v).map_err(|e| format!("the config: {e}"))?;
            }
            println!("{}", doctor::awake_in_config());
            Ok(())
        }
        r => say(r),
    }
}

fn list(json: bool, all: bool) -> Result<(), String> {
    let residents = match request(Request::List { all }, true)? {
        Reply::List { residents, .. } => residents,
        other => return Err(format!("unexpected reply {other:?}")),
    };
    if json {
        println!("{}", serde_json::to_string(&residents).unwrap_or_default());
        return Ok(());
    }
    let now = crate::daemon::store::now();
    for r in &residents {
        let slot = r.slot.map_or("-".into(), |s| s.to_string());
        let state = serde_json::to_value(r.state).ok();
        let state = state.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        let t = r.telemetry.as_ref();
        let mut fields = crate::tele::fields(t, r.mode.as_deref(), r.branch.as_deref(), true, now);
        if let Some(at) = t.map(|t| t.at).filter(|at| *at > 0) {
            fields = format!("{fields} · {} ago", crate::tele::age(now.saturating_sub(at) as u64));
        }
        if r.renewing {
            fields = format!("{fields} · ↻ renewing");
        } else if let Some(v) = &r.outdated {
            fields = format!("{fields} · ⇡ claude {v}: gensokyo renew");
        }
        let cwd = crate::tele::clean(r.here.as_ref().unwrap_or(&r.cwd), usize::MAX);
        println!("{slot} {} {:<12} {state:<8} {cwd}  {fields}", r.state.glyph(), r.name);
    }
    let reports = || residents.iter().filter_map(|r| r.telemetry.as_ref());
    let five = crate::tele::freshest(reports().filter_map(|t| t.five_hour.as_ref()), now);
    let wk = crate::tele::freshest(reports().filter_map(|t| t.seven_day.as_ref()), now);
    let usage: Vec<String> = [("5h", five), ("wk", wk)]
        .into_iter()
        .filter_map(|(label, l)| l.map(|l| crate::tele::usage(label, l, now, 10)))
        .collect();
    if !usage.is_empty() {
        println!("usage {}", usage.join("   "));
    }
    Ok(())
}

/// Exit 0 when the news came, 3 on the timeout, 4 when the daemon went away, 1 on an error.
/// Whatever the exit, a line with `news` has something to read: only a met wait collects it.
fn wait(who: Vec<String>, any: bool, until: Option<Until>, timeout: &str) -> ExitCode {
    let Some(timeout) = crate::ritual::seconds(timeout) else {
        eprintln!("gensokyo: --timeout {timeout} is not a length (90s, 30m, 2h)");
        return ExitCode::from(2);
    };
    match request(Request::Wait(Wait { who, any, until, timeout }), false) {
        Ok(Reply::Waited { met, residents, .. }) => {
            for r in &residents {
                println!("{}", serde_json::to_string(r).unwrap_or_default());
            }
            ExitCode::from(if met { 0 } else { 3 })
        }
        // Hung up on: the daemon went away (a restart). One not there at all is an error.
        Err(e @ Error::Io(_)) => {
            eprintln!("gensokyo: {e}");
            ExitCode::from(4)
        }
        Ok(other) => {
            eprintln!("gensokyo: unexpected reply {other:?}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("gensokyo: {e}");
            ExitCode::FAILURE
        }
    }
}

fn resume(who: &str) -> Result<(), String> {
    match request(Request::Recall { who: who.into() }, true)? {
        Reply::Summoned { resident: r, .. } => {
            let slot = r.slot.map_or(String::new(), |s| format!(" (slot {s})"));
            println!("recalled {}{slot} in {}", r.name, r.cwd);
            Ok(())
        }
        other => Err(format!("unexpected reply {other:?}")),
    }
}

/// A role that is a file, found from here: the daemon is somewhere else. One that is not there
/// is left for the daemon to say so.
pub(crate) fn role_here(here: &std::path::Path, r: String) -> String {
    if !r.contains('/') {
        return r;
    }
    let p = here.join(paths::expand(&r));
    std::fs::canonicalize(&p).unwrap_or(p).to_string_lossy().into_owned()
}

fn new(n: New) -> Result<(), String> {
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    let role = n.role.map(|r| role_here(&here, r));
    let cwd = match n.dir.as_deref() {
        None | Some("") => here,
        Some(d) => here.join(d),
    };
    let prompt = match n.prompt_file.as_deref() {
        None => n.prompt,
        Some("-") => {
            let mut p = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut p)
                .map_err(|e| format!("--prompt-file -: {e}"))?;
            Some(p)
        }
        Some(f) => Some(std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?),
    };
    let s = Summon {
        cwd: cwd.to_string_lossy().into_owned(),
        name: n.name,
        model: n.model,
        effort: n.effort,
        mode: n.mode,
        prompt: prompt.filter(|p| !p.trim().is_empty()),
        allowed_tools: n.allowed_tools,
        role,
        system_prompt: n.system_prompt.filter(|p| !p.trim().is_empty()),
        worktree: n.worktree.map(|slug| WorktreeAsk { slug, branch: n.branch, base: n.base }),
    };
    let reply = request(Request::Summon(s), true)?;
    if let Reply::Summoned { note: Some(note), .. } = &reply {
        eprintln!("gensokyo: {note}");
    }
    match reply {
        Reply::Summoned { resident: r, .. } if n.json => {
            println!("{}", serde_json::to_string(&r).unwrap_or_default());
            Ok(())
        }
        Reply::Summoned { resident: r, .. } => {
            let slot = r.slot.map_or(String::new(), |s| format!(" (slot {s})"));
            println!("summoned {}{slot} in {}", r.name, r.cwd);
            Ok(())
        }
        other => Err(format!("unexpected reply {other:?}")),
    }
}

/// `broadcast <card> <targets…> [--with <peer>]`; alone it lists the cards, off the files. No
/// free text: a prompt worth sending to everybody is worth a file in `spellcards/`.
fn broadcast(
    card: Option<String>,
    targets: Vec<String>,
    with: Option<String>,
) -> Result<(), String> {
    if let Some(card) = card {
        return say(request(Request::Cast(Cast { card, targets, peer: with }), false));
    }
    if with.is_some() {
        return Err("broadcast: which card? (gensokyo broadcast lists them)".into());
    }
    let mine = paths::config_dir().join("spellcards");
    let mut dirs = vec![mine.clone()];
    dirs.extend(rituals::share().map(|s| s.join("spellcards")));
    let (cards, unusable) = crate::card::load(&dirs);
    let mine = crate::paths::short(&mine.to_string_lossy());
    if cards.is_empty() {
        println!("no spell cards: put one in {mine}");
    } else {
        println!("  {:<18} {:<34} what it asks for", "card", "title");
        for k in cards.iter().map(|c| c.listed()) {
            let pair = if k.pair { " (needs --with <peer>)" } else { "" };
            println!("  {:<18} {:<34} {}{pair}", k.slug, k.title, k.summary);
        }
        println!();
        println!(
            "  gensokyo broadcast <card> all|awaiting|idle|<name> [--with <name>]   (the shrine's [cast c] button)"
        );
        println!(
            "  your own cards go in {mine}/<name>.md; {{self}} {{peer}} {{cwd}} {{residents}} are filled in"
        );
    }
    for f in unusable {
        eprintln!(
            "gensokyo: not a usable card name (letters, digits, . _ - and .md): {}",
            crate::paths::short(&f.to_string_lossy())
        );
    }
    Ok(())
}
