//! The plumbing verbs: one request over the socket, starting the daemon when nobody answers.

mod conn;
mod doctor;
mod install;
mod login;
mod rituals;

pub use conn::{Error, connect_or_start, peer_pid, refusal, request};

use crate::paths;
use crate::proto::{Cast, Reply, Request, Summon};
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
    New(New),
    /// Bring a departed resident back into its old conversation
    #[command(visible_alias = "recall")]
    Resume {
        /// A name, a slot or an id
        who: String,
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
    /// Ask every resident to /exit, then stop the daemon
    Quit,
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
        Cmd::New(n) => new(n),
        Cmd::Resume { who } => resume(&who),
        Cmd::Banish { who } => say(request(Request::Banish { who }, false)),
        Cmd::Close { who } => say(request(Request::Close { who }, false)),
        Cmd::Broadcast { card, targets, with } => broadcast(card, targets, with),
        Cmd::Ritual { cmd } => rituals::main(cmd),
        Cmd::Quit => match request(Request::Quit, false) {
            Err(Error::NotRunning) => Ok(()),
            r => say(r),
        },
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
        let mut fields = crate::tele::fields(t, r.mode.as_deref(), r.branch.as_deref(), true);
        if let Some(at) = t.map(|t| t.at).filter(|at| *at > 0) {
            fields = format!("{fields} · {} ago", crate::tele::age(now.saturating_sub(at) as u64));
        }
        let cwd = crate::tele::clean(&r.cwd, usize::MAX);
        println!("{slot} {} {:<12} {state:<8} {cwd}  {fields}", r.state.glyph(), r.name);
    }
    let newest = residents.iter().filter_map(|r| r.telemetry.as_ref()).max_by_key(|t| t.at);
    let usage: Vec<String> = newest
        .into_iter()
        .flat_map(|t| [("5h", &t.five_hour), ("wk", &t.seven_day)])
        .filter_map(|(label, l)| l.as_ref().map(|l| crate::tele::usage(label, l, now, 10)))
        .collect();
    if !usage.is_empty() {
        println!("usage {}", usage.join("   "));
    }
    Ok(())
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

fn new(n: New) -> Result<(), String> {
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    let cwd = match n.dir.as_deref() {
        None | Some("") => here,
        Some(d) => here.join(d),
    };
    let s = Summon {
        cwd: cwd.to_string_lossy().into_owned(),
        name: n.name,
        model: n.model,
        effort: n.effort,
        mode: n.mode,
        prompt: n.prompt,
    };
    match request(Request::Summon(s), true)? {
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
