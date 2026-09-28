//! The plumbing verbs: one request over the socket, starting the daemon when nobody answers.

use crate::proto::{self, Cast, Envelope, Reply, Request, RitualVerb, Summon};
use crate::ritual::{self, Dir, Ritual, Trust};
use jiff::tz::TimeZone;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

/// tmux's algorithm: connect; failing that, take the client lock, connect again (another client
/// may have started the daemon meanwhile), and only then spawn it, holding the lock until the
/// socket answers so the clients queued behind it connect instead of spawning.
pub fn connect_or_start() -> Result<UnixStream, String> {
    let sock = proto::socket_path();
    if let Ok(s) = UnixStream::connect(&sock) {
        return Ok(s);
    }
    let run = proto::state_dir().join("run");
    std::fs::create_dir_all(&run).map_err(|e| format!("{}: {e}", run.display()))?;
    let lock = std::fs::File::create(run.join("client.lock")).map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| format!("client lock: {e}"))?;
    if let Ok(s) = UnixStream::connect(&sock) {
        return Ok(s);
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            (libc::setsid() != -1).then_some(()).ok_or_else(std::io::Error::last_os_error)
        })
    };
    let mut child = cmd.spawn().map_err(|e| format!("start the daemon: {e}"))?;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(5) {
        if let Ok(s) = UnixStream::connect(&sock) {
            // It stays running on its own; reap it if it ever exits while we run.
            std::thread::spawn(move || child.wait());
            return Ok(s);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err("the daemon did not come up in 5 s".into())
}

/// hello/welcome, then one request and its reply.
pub fn request(req: Request, start: bool) -> Result<Reply, String> {
    let s = if start {
        connect_or_start()?
    } else {
        UnixStream::connect(proto::socket_path()).map_err(|_| "the daemon is not running")?
    };
    let mut w = s.try_clone().map_err(|e| e.to_string())?;
    let hello = Envelope { id: 0, req: Request::Hello { proto: proto::PROTO, who: "cli".into() } };
    let line = |e: &Envelope| serde_json::to_string(e).unwrap_or_default();
    writeln!(w, "{}\n{}", line(&hello), line(&Envelope { id: 1, req }))
        .map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(s).lines();
    let mut next = || -> Result<Reply, String> {
        let l = lines.next().ok_or("the daemon hung up")?.map_err(|e| e.to_string())?;
        serde_json::from_str(&l).map_err(|e| format!("{e}: {l}"))
    };
    match next()? {
        Reply::Welcome { .. } => next(),
        other => Ok(other),
    }
    .and_then(|r| match r {
        Reply::Error { error, .. } => Err(error),
        r => Ok(r),
    })
}

pub fn main(args: &[String]) -> ExitCode {
    let verb = args.first().map(String::as_str).unwrap_or("");
    let rest = &args[1.min(args.len())..];
    let r = match verb {
        "list" => list(rest),
        "new" => new(rest),
        "resume" => match rest {
            [who] => resume(who),
            _ => Err("usage: gensokyo resume <name|id>".into()),
        },
        "banish" | "close" => match rest {
            [who] if verb == "banish" => say(request(Request::Banish { who: who.clone() }, false)),
            [who] => say(request(Request::Close { who: who.clone() }, false)),
            _ => Err(format!("usage: gensokyo {verb} <name|slot>")),
        },
        "broadcast" => broadcast(rest),
        "ritual" => ritual_cmd(rest),
        "quit" => match request(Request::Quit, false) {
            Err(e) if e == "the daemon is not running" => Ok(()),
            r => say(r),
        },
        _ => Err(format!("unknown command {verb:?}")),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("gensokyo: {e}");
            ExitCode::FAILURE
        }
    }
}

fn say(r: Result<Reply, String>) -> Result<(), String> {
    match r? {
        Reply::Done { message, .. } => {
            println!("{message}");
            Ok(())
        }
        other => Err(format!("unexpected reply {other:?}")),
    }
}

fn list(args: &[String]) -> Result<(), String> {
    let json = args.iter().any(|a| a == "--json");
    let all = args.iter().any(|a| a == "--all");
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

fn new(args: &[String]) -> Result<(), String> {
    let mut s = Summon::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("new: {a} needs a value"));
        match a.as_str() {
            "-n" | "--name" => s.name = Some(val()?),
            "-m" | "--model" => s.model = Some(val()?),
            "-e" | "--effort" => s.effort = Some(val()?),
            "-p" | "--permission-mode" => s.mode = Some(val()?),
            "--prompt" => s.prompt = Some(val()?),
            _ if a.starts_with('-') => return Err(format!("new: unknown option {a}")),
            _ if !s.cwd.is_empty() => return Err("new: one directory only".into()),
            _ => s.cwd = a.clone(),
        }
    }
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    s.cwd = match s.cwd.as_str() {
        "" => here.to_string_lossy().into_owned(),
        c => here.join(c).to_string_lossy().into_owned(),
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

/// `broadcast <card> <all|awaiting|idle|name...> [--with <peer>]`; alone it lists the cards. No
/// free text: a prompt worth sending to everybody is worth a file in `spellcards/`.
fn broadcast(args: &[String]) -> Result<(), String> {
    let mut c = Cast::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--with" => c.peer = Some(it.next().cloned().ok_or("broadcast: --with needs a name")?),
            _ if a.starts_with('-') => {
                return Err(format!(
                    "broadcast: unknown option {a} (usage: gensokyo broadcast <card> <all|awaiting|idle|name> [--with peer])"
                ));
            }
            _ if c.card.is_empty() => c.card = a.clone(),
            _ => c.targets.push(a.clone()),
        }
    }
    if !c.card.is_empty() {
        return say(request(Request::Cast(c), false));
    }
    let (cards, unusable) = match request(Request::Cards, true)? {
        Reply::Cards { cards, unusable, .. } => (cards, unusable),
        other => return Err(format!("unexpected reply {other:?}")),
    };
    let tilde = ritual::tilde;
    let mine = tilde(&proto::config_dir().join("spellcards").to_string_lossy());
    if cards.is_empty() {
        println!("no spell cards: put one in {mine}");
    } else {
        println!("  {:<18} {:<34} what it asks for", "card", "title");
        for k in &cards {
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
            tilde(&f)
        );
    }
    Ok(())
}

const RITUAL_USAGE: &str = "usage: gensokyo ritual [list [--json]]
       gensokyo ritual add --name <n> --schedule <s> [--cwd <dir>] --prompt-file <f|-> [...]
       gensokyo ritual run|enable|disable|remove|edit <name>
       gensokyo ritual log <name> [-n N] [--all]
       gensokyo ritual new <name>";

/// `ritual …`: the scheduled work. Reading and writing the files needs no daemon; firing one
/// does, and removing one asks it first whether a run is still going.
fn ritual_cmd(args: &[String]) -> Result<(), String> {
    let sub = args.first().map_or("list", String::as_str);
    let rest = &args[1.min(args.len())..];
    let one = |verb: &str| match rest {
        [name] => Ok(name.clone()),
        [] => Err(format!("ritual {verb}: which one? (gensokyo ritual lists them)")),
        _ => Err(format!("ritual {verb}: one ritual at a time")),
    };
    match sub {
        "list" | "ls" => ritual_list(rest),
        "add" => ritual_add(rest),
        "run" => say(request(Request::Ritual { verb: RitualVerb::Run, name: one("run")? }, true)),
        v @ ("enable" | "disable") => ritual_toggle(v == "enable", &one(v)?),
        "remove" | "delete" | "rm" => ritual_remove(&one("remove")?),
        "log" => ritual_log(rest),
        "edit" => ritual_edit(&one("edit")?),
        "new" => ritual_new(&one("new")?),
        "help" | "-h" | "--help" => {
            println!("{RITUAL_USAGE}");
            Ok(())
        }
        _ => Err(format!("ritual: nothing called '{sub}' to do\n{RITUAL_USAGE}")),
    }
}

fn share() -> Option<std::path::PathBuf> {
    proto::share_dir(&std::env::current_exe().ok()?)
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

fn ritual_list(args: &[String]) -> Result<(), String> {
    let json = match args {
        [] => false,
        [a] if a == "--json" => true,
        _ => {
            let a = args.iter().find(|a| *a != "--json").unwrap_or(&args[0]);
            return Err(format!(
                "ritual list: unknown option {a} (usage: gensokyo ritual list [--json])"
            ));
        }
    };
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
        let mine = ritual::tilde(&ritual::mine_dir().to_string_lossy());
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
            println!("  {:<18} not firing: {p} ({})", "", ritual::tilde(&r.path));
        }
    }
    if !infos.is_empty() {
        println!();
        println!(
            "  gensokyo ritual run <name>      fire one now, which is how its prompts get approved once"
        );
        println!("  gensokyo ritual log <name>      what it has done; edit <name> opens the file");
        if UnixStream::connect(proto::socket_path()).is_err() {
            println!("  gensokyo is not running, so nothing fires until it is (run gensokyo)");
        }
    }
    for f in unusable {
        eprintln!(
            "gensokyo: not a usable ritual name (letters, digits, . _ - and .md): {}",
            ritual::tilde(&f.to_string_lossy())
        );
    }
    Ok(())
}

fn ritual_add(args: &[String]) -> Result<(), String> {
    let mut a = ritual::Add::default();
    let (mut prompt, mut file, mut cwd) = (None::<String>, None::<String>, None::<String>);
    let mut it = args.iter();
    while let Some(k) = it.next() {
        // The next option is not a value: `--description $EMPTY --disabled` would otherwise
        // write an enabled ritual. A prompt may begin with `--` when it is prose.
        let mut val = || match it.next() {
            Some(v) if v.starts_with("--") && !(k == "--prompt" && v.contains(' ')) => {
                Err(format!("ritual add: {k} needs a value, and {v} is the next option"))
            }
            Some(v) => Ok(v.clone()),
            None => Err(format!("ritual add: {k} needs a value")),
        };
        match k.as_str() {
            "--name" => a.name = val()?,
            "--schedule" => a.schedule = val()?,
            "--cwd" => cwd = Some(val()?),
            "--description" => a.description = Some(val()?),
            "--model" => a.model = Some(val()?),
            "--effort" => a.effort = Some(val()?),
            "--mode" | "--permission-mode" => a.mode = Some(val()?),
            "--mcp-config" => a.mcp_config = Some(val()?),
            "--target" => a.target = Some(val()?),
            "--keep" => a.keep = Some(val()?),
            "--overlap" => a.overlap = Some(val()?),
            "--catch-up" => a.catch_up = Some(val()?),
            "--prompt" => prompt = Some(val()?),
            "--prompt-file" => file = Some(val()?),
            "--headless" => a.headless = true,
            "--disabled" => a.disabled = true,
            "--allowed-tools" | "--allowed-tool" => {
                let v = val()?;
                a.allowed_tools.extend(
                    v.split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from),
                );
            }
            _ => return Err(format!("ritual add: unknown option {k}\n{RITUAL_USAGE}")),
        }
    }
    a.prompt = match (prompt, file) {
        (Some(_), Some(_)) => return Err("ritual add: --prompt or --prompt-file, not both".into()),
        (Some(p), None) => p,
        (None, Some(f)) if f == "-" => {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)
                .map_err(|e| format!("ritual add: stdin: {e}"))?;
            s
        }
        (None, Some(f)) => {
            let f = match f.strip_prefix("~/") {
                Some(rest) => format!("{}/{rest}", std::env::var("HOME").unwrap_or_default()),
                None => f,
            };
            std::fs::read_to_string(&f)
                .map_err(|e| format!("ritual add: no such prompt file: {f} ({e})"))?
        }
        (None, None) => String::new(),
    };
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    a.cwd = match cwd.as_deref() {
        None | Some("") => here.to_string_lossy().into_owned(),
        Some(c) => match c.strip_prefix("~") {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                format!("{}{rest}", std::env::var("HOME").unwrap_or_default())
            }
            _ => here.join(c).to_string_lossy().into_owned(),
        },
    };
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
            ritual::tilde(&path.to_string_lossy())
        );
    }
    Ok(path)
}

fn ritual_toggle(on: bool, name: &str) -> Result<(), String> {
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
fn ritual_remove(name: &str) -> Result<(), String> {
    let r = found(name).map_err(|e| format!("ritual remove: {e}"))?;
    if !r.slug.eq_ignore_ascii_case(name) {
        return Err(format!(
            "ritual remove: no ritual called '{name}' - did you mean {}? (remove wants the whole name)",
            r.slug
        ));
    }
    match request(Request::Ritual { verb: RitualVerb::Remove, name: r.slug.clone() }, false) {
        Err(e) if e == "the daemon is not running" => {
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

fn ritual_log(args: &[String]) -> Result<(), String> {
    let (mut name, mut n) = (None::<&String>, 20usize);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-n" | "--lines" => {
                n = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("ritual log: -n takes a number of lines")?
            }
            "--all" => n = 0,
            _ if a.starts_with('-') => {
                return Err(format!(
                    "ritual log: unknown option {a} (usage: gensokyo ritual log <name> [-n N] [--all])"
                ));
            }
            _ if name.is_some() => return Err("ritual log: one ritual at a time".into()),
            _ => name = Some(a),
        }
    }
    let name = name.ok_or("ritual log: which one? (gensokyo ritual lists them)")?;
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
        ritual::tilde(&journal.to_string_lossy())
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
            ritual::tilde(&newest.to_string_lossy())
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
            ritual::tilde(&path.to_string_lossy())
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

fn ritual_edit(name: &str) -> Result<(), String> {
    terminal("edit")?;
    let r = found(name).map_err(|e| format!("ritual edit: {e}"))?;
    let path = mine(&r).map_err(|e| format!("ritual edit: {e}"))?;
    open_editor(&path).map_err(|e| format!("ritual edit: {e}"))?;
    report(&path);
    Ok(())
}

fn ritual_new(name: &str) -> Result<(), String> {
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
            format!("ritual new: could not write {}: {e}", ritual::tilde(&path.to_string_lossy()))
        })?;
    println!("wrote {}", ritual::tilde(&path.to_string_lossy()));
    open_editor(&path).map_err(|e| format!("ritual new: {e}"))?;
    report(&path);
    Ok(())
}
