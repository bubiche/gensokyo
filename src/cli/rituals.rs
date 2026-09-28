//! `gensokyo ritual …`: the scheduled work. Reading and writing the files needs no daemon;
//! firing one does, and removing one asks it first whether a run is still going.

use super::{Error, request, say};
use crate::paths;
use crate::proto::{self, Request, RitualVerb};
use crate::ritual::{self, Dir, Ritual, Trust};
use clap::Subcommand;
use jiff::tz::TimeZone;
use std::os::unix::net::UnixStream;
use std::process::Command;

#[derive(Subcommand)]
pub enum Cmd {
    /// What is scheduled, when each fires next, and why one is not firing
    #[command(visible_alias = "ls")]
    List {
        /// As JSON, for scripts and residents
        #[arg(long)]
        json: bool,
    },
    /// Write a ritual: a schedule, a directory and a prompt
    Add(Box<Add>),
    /// Fire one now, whatever its schedule says (the schedule is untouched)
    Run { name: String },
    /// Turn one back on
    Enable { name: String },
    /// Pause one: the file stays, the schedule stops
    Disable { name: String },
    /// Delete one with its notes and its journal
    #[command(visible_aliases = ["delete", "rm"])]
    Remove {
        /// The whole name
        name: String,
    },
    /// What one has done: its fires, skips and complaints, newest last
    Log {
        name: String,
        /// How many lines
        #[arg(short = 'n', long = "lines", value_name = "N", default_value_t = 20)]
        n: usize,
        /// Every line there is
        #[arg(long)]
        all: bool,
    },
    /// Open one in $VISUAL or $EDITOR
    Edit { name: String },
    /// Write a commented template for a new one and open it
    New { name: String },
}

#[derive(clap::Args)]
pub struct Add {
    /// Its file name and its runs' resident name: letters, digits, . _ -
    #[arg(long)]
    name: String,
    /// Five cron fields in local time, @hourly @daily @weekly @monthly @yearly, or "every 30m"
    #[arg(long)]
    schedule: String,
    /// Where it runs; the current directory when left out
    #[arg(long)]
    cwd: Option<String>,
    /// One line for the listing
    #[arg(long)]
    description: Option<String>,
    /// The prompt, on the command line
    #[arg(long, allow_hyphen_values = true, conflicts_with = "prompt_file")]
    prompt: Option<String>,
    /// The prompt from a file, or - for stdin
    #[arg(long, value_name = "FILE")]
    prompt_file: Option<String>,
    /// The model its runs use (claude --model); haiku for reading and summarising
    #[arg(long)]
    model: Option<String>,
    /// Its runs' effort (claude --effort)
    #[arg(long)]
    effort: Option<String>,
    /// The permission mode its runs start in
    #[arg(long, visible_alias = "permission-mode", value_name = "MODE")]
    mode: Option<String>,
    /// Tools its runs may use unasked; repeated, or a comma-separated list
    #[arg(long = "allowed-tools", visible_alias = "allowed-tool", value_name = "TOOLS")]
    allowed_tools: Vec<String>,
    /// MCP servers for its runs (claude --mcp-config)
    #[arg(long, value_name = "FILE")]
    mcp_config: Option<String>,
    /// new (a fresh resident per run), persistent, or a resident's name
    #[arg(long)]
    target: Option<String>,
    /// How long a finished run stays before it is asked to leave (2h, or forever)
    #[arg(long)]
    keep: Option<String>,
    /// When a fire finds the last run still going: skip, queue or parallel
    #[arg(long)]
    overlap: Option<String>,
    /// Whether a fire missed while gensokyo was not running is made up once (true or false)
    #[arg(long, value_name = "BOOL")]
    catch_up: Option<String>,
    /// A background claude -p with no pane; what it said goes to its log
    #[arg(long)]
    headless: bool,
    /// Written paused
    #[arg(long)]
    disabled: bool,
}

pub fn main(cmd: Option<Cmd>) -> Result<(), String> {
    match cmd.unwrap_or(Cmd::List { json: false }) {
        Cmd::List { json } => list(json),
        Cmd::Add(a) => add(*a),
        Cmd::Run { name } => say(request(Request::Ritual { verb: RitualVerb::Run, name }, true)),
        Cmd::Enable { name } => toggle(true, &name),
        Cmd::Disable { name } => toggle(false, &name),
        Cmd::Remove { name } => remove(&name),
        Cmd::Log { name, n, all } => log(&name, if all { 0 } else { n }),
        Cmd::Edit { name } => edit(&name),
        Cmd::New { name } => new(&name),
    }
}

pub(super) fn share() -> Option<std::path::PathBuf> {
    paths::share_dir(&std::env::current_exe().ok()?)
}

fn rituals() -> (Vec<Ritual>, Vec<std::path::PathBuf>) {
    let share = share();
    ritual::load(&ritual::dirs(share.as_deref()), share.as_deref())
}

fn found(name: &str) -> Result<Ritual, String> {
    let (rs, _) = rituals();
    ritual::find(&rs, name).cloned()
}

fn now() -> i64 {
    crate::daemon::store::now()
}

fn list(json: bool) -> Result<(), String> {
    let (rs, unusable) = rituals();
    let (now, tz, trust) = (now(), TimeZone::system(), Trust::load());
    let infos: Vec<proto::RitualInfo> =
        rs.iter().map(|r| ritual::info(r, now, &tz, &trust, &Dir::of(&r.slug))).collect();
    if json {
        // `running` is the daemon's to know; here it is always false.
        println!("{}", serde_json::to_string(&infos).unwrap_or_default());
        return Ok(());
    }
    if infos.is_empty() {
        println!("nothing is scheduled yet: gensokyo ritual new <name>");
        let mine = paths::short(&ritual::mine_dir().to_string_lossy());
        println!("  your rituals live in {mine}/<name>.md");
    }
    for (i, r) in infos.iter().enumerate() {
        if i == 0 {
            println!(
                "  {:<18} {:<4} {:<14} {:<26} what it does",
                "ritual", "", "schedule", "next fire"
            );
        }
        let next = match (r.enabled, r.next_fire, &r.next_fire_local) {
            (false, ..) => "paused".to_string(),
            (true, Some(t), Some(l)) => {
                format!("{l} (in {})", crate::tele::age((t - now).max(0) as u64))
            }
            _ => String::new(),
        };
        let on = if r.enabled { "on" } else { "off" };
        let desc = r.description.as_deref().unwrap_or("");
        println!("  {:<18} {on:<4} {:<14} {next:<26} {desc}", r.name, r.schedule);
        if let Some(t) = r.last_run {
            let ago = crate::tele::age(now.saturating_sub(t) as u64);
            println!("  {:<18} last ran {} ({ago} ago)", "", ritual::when(t, &tz));
        }
        if r.headless {
            println!("  {:<18} headless: no pane, and its own log of what each run said", "");
        }
        if let Some(p) = &r.problem {
            println!("  {:<18} not firing: {p} ({})", "", paths::short(&r.path));
        }
    }
    if !infos.is_empty() {
        println!();
        println!(
            "  gensokyo ritual run <name>      fire one now, which is how its prompts get approved once"
        );
        println!("  gensokyo ritual log <name>      what it has done; edit <name> opens the file");
        if UnixStream::connect(paths::socket_path()).is_err() {
            println!("  gensokyo is not running, so nothing fires until it is (run gensokyo)");
        }
    }
    for f in unusable {
        eprintln!(
            "gensokyo: not a usable ritual name (letters, digits, . _ - and .md): {}",
            paths::short(&f.to_string_lossy())
        );
    }
    Ok(())
}

fn add(o: Add) -> Result<(), String> {
    // Prose may begin with a hyphen; a lone flag is a value left out before it.
    if let Some(p) = o.prompt.as_deref().filter(|p| p.starts_with("--") && !p.contains(' ')) {
        return Err(format!("ritual add: --prompt needs a value, and {p} is the next option"));
    }
    let prompt = match (o.prompt, o.prompt_file) {
        (Some(p), _) => p,
        (None, Some(f)) if f == "-" => {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)
                .map_err(|e| format!("ritual add: stdin: {e}"))?;
            s
        }
        (None, Some(f)) => {
            let f = paths::expand(&f);
            std::fs::read_to_string(&f)
                .map_err(|e| format!("ritual add: no such prompt file: {f} ({e})"))?
        }
        (None, None) => String::new(),
    };
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    let cwd = match o.cwd.as_deref() {
        None | Some("") => here.to_string_lossy().into_owned(),
        Some(c) => here.join(paths::expand(c)).to_string_lossy().into_owned(),
    };
    // Field by field: a field added to Add later keeps its default here.
    let mut a = ritual::Add::default();
    (a.name, a.schedule, a.cwd, a.prompt) = (o.name, o.schedule, cwd, prompt);
    (a.description, a.model, a.effort, a.mode) = (o.description, o.model, o.effort, o.mode);
    (a.mcp_config, a.target, a.keep, a.overlap) = (o.mcp_config, o.target, o.keep, o.overlap);
    (a.catch_up, a.headless, a.disabled) = (o.catch_up, o.headless, o.disabled);
    a.allowed_tools = o.allowed_tools.iter().flat_map(|v| crate::frontmatter::items(v)).collect();
    let share = share();
    let (_, report) = ritual::add(
        &a,
        now(),
        &TimeZone::system(),
        &Trust::load(),
        &ritual::mine_dir(),
        share.as_deref(),
    )
    .map_err(|e| format!("ritual add: {e}"))?;
    for l in report {
        match l.strip_prefix("warning: ") {
            Some(w) => eprintln!("gensokyo: ritual add: {w}"),
            None => println!("{l}"),
        }
    }
    Ok(())
}

/// The next fire, or why there is none, after a ritual changed.
fn report(path: &std::path::Path) {
    let slug = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let Ok(r) = found(&slug) else { return };
    if r.path != path {
        return;
    }
    let (now, tz) = (now(), TimeZone::system());
    let i = ritual::info(&r, now, &tz, &Trust::load(), &Dir::of(&r.slug));
    match (&i.problem, i.enabled, i.next_fire_local) {
        (Some(p), ..) => println!("  not firing: {p}"),
        (None, false, _) => {
            println!("  disabled, so it will not fire: gensokyo ritual enable {slug}")
        }
        (None, true, Some(l)) => println!("  next fire  {l}"),
        _ => {}
    }
}

/// A shipped ritual becomes the user's before it is changed.
fn mine(r: &Ritual) -> Result<std::path::PathBuf, String> {
    let path = ritual::mine(r, &ritual::mine_dir())?;
    if path != r.path {
        println!(
            "the shipped {}.md is now yours, in {}",
            r.slug,
            paths::short(&path.to_string_lossy())
        );
    }
    Ok(path)
}

fn toggle(on: bool, name: &str) -> Result<(), String> {
    let verb = if on { "enable" } else { "disable" };
    let r = found(name).map_err(|e| format!("ritual {verb}: {e}"))?;
    let lines = ritual::toggle(&r, on, now(), &ritual::mine_dir())
        .map_err(|e| format!("ritual {verb}: {e}"))?;
    lines.iter().for_each(|l| println!("{l}"));
    if on {
        let own = ritual::mine_dir().join(format!("{}.md", r.slug));
        report(if r.shipped { &own } else { &r.path });
    }
    Ok(())
}

/// Through the daemon, which knows whether a run is still going; with none running, nothing is.
fn remove(name: &str) -> Result<(), String> {
    let r = found(name).map_err(|e| format!("ritual remove: {e}"))?;
    if !r.slug.eq_ignore_ascii_case(name) {
        return Err(format!(
            "ritual remove: no ritual called '{name}' - did you mean {}? (remove wants the whole name)",
            r.slug
        ));
    }
    match request(Request::Ritual { verb: RitualVerb::Remove, name: r.slug.clone() }, false) {
        Err(Error::NotRunning) => {
            let share = share();
            for l in ritual::remove(&r, &Dir::of(&r.slug), share.as_deref())
                .map_err(|e| format!("ritual remove: {e}"))?
            {
                println!("{l}");
            }
            Ok(())
        }
        reply => say(reply),
    }
}

/// `n` lines of its journal, newest last; 0 is all of them.
fn log(name: &str, n: usize) -> Result<(), String> {
    let r = found(name).map_err(|e| format!("ritual log: {e}"))?;
    let dir = Dir::of(&r.slug);
    let lines = dir.journal(n);
    if lines.is_empty() {
        println!("{0} has not done anything yet (gensokyo ritual run {0} fires it now)", r.slug);
        return Ok(());
    }
    let tz = TimeZone::system();
    for e in &lines {
        println!("  {}  {}", ritual::when(e.at, &tz), e.text);
    }
    println!();
    let journal = dir.path.join("journal.jsonl");
    println!(
        "  {}   (its notes are beside it, in memory.md)",
        paths::short(&journal.to_string_lossy())
    );
    let mut runs: Vec<_> = std::fs::read_dir(dir.runs())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "log"))
        .collect();
    runs.sort();
    if let Some(newest) = runs.last() {
        println!(
            "  {}   (what the last headless run said, in full)",
            paths::short(&newest.to_string_lossy())
        );
    }
    Ok(())
}

/// `$VISUAL`, else `$EDITOR`, else vi, on the terminal this runs in.
fn open_editor(path: &std::path::Path) -> Result<(), String> {
    let ed = ["VISUAL", "EDITOR"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "vi".into());
    // Through the shell: an editor setting is often a command with flags (`code -w`).
    let ok = Command::new("/bin/sh")
        .args(["-c", &format!("{ed} \"$1\""), "sh"])
        .arg(path)
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{ed} did not finish cleanly; the file is there: {}",
            paths::short(&path.to_string_lossy())
        ))
    }
}

/// An editor needs the terminal: a session without one would sit in vi for ever.
fn terminal(verb: &str) -> Result<(), String> {
    // SAFETY: plain libc call.
    match unsafe { libc::isatty(0) } == 1 {
        true => Ok(()),
        false => Err(format!(
            "ritual {verb} opens an editor; a session without a terminal edits the file at its \
             path instead (gensokyo ritual list --json gives it)"
        )),
    }
}

fn edit(name: &str) -> Result<(), String> {
    terminal("edit")?;
    let r = found(name).map_err(|e| format!("ritual edit: {e}"))?;
    let path = mine(&r).map_err(|e| format!("ritual edit: {e}"))?;
    open_editor(&path).map_err(|e| format!("ritual edit: {e}"))?;
    report(&path);
    Ok(())
}

fn new(name: &str) -> Result<(), String> {
    if !ritual::name_ok(name) {
        return Err(format!(
            "ritual new: '{name}' cannot be a ritual name (a letter or digit first, then letters, digits, . _ -)"
        ));
    }
    terminal("new")?;
    let dir = ritual::mine_dir();
    let path = dir.join(format!("{name}.md"));
    if path.exists() {
        return Err(format!("ritual new: {name} is already there: gensokyo ritual edit {name}"));
    }
    if share().is_some_and(|s| s.join("rituals").join(format!("{name}.md")).exists()) {
        eprintln!(
            "gensokyo: ritual new: {name} is also one of the shipped examples; yours shadows it"
        );
    }
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(&path, ritual::template(name, &here.to_string_lossy())))
        .map_err(|e| {
            format!("ritual new: could not write {}: {e}", paths::short(&path.to_string_lossy()))
        })?;
    println!("wrote {}", paths::short(&path.to_string_lossy()));
    open_editor(&path).map_err(|e| format!("ritual new: {e}"))?;
    report(&path);
    Ok(())
}
