//! `gensokyo cloud …`: the user's routines on claude.ai, as the daemon watches them.

use super::{Error, request, say};
use crate::cloud::{self, on};
use crate::paths;
use crate::proto::{CloudAct, CloudAdd, CloudWatch, Reply, Request, RoutineInfo};
use clap::Subcommand;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum Cmd {
    /// The routines as the watch last listed them, each with its newest run's link
    List,
    /// Watch the routines on claude.ai, and say when a run's turn ends; alone, whether it does
    ///
    /// Off by default: every look is a short `claude -p` on haiku (about $0.001), on an API
    /// Claude Code does not document. On, the daemon lists the routines hourly and after each
    /// one's time, and a run's turn until it ends. Kept in the config as CLOUD_WATCH.
    Watch {
        #[arg(value_parser = ["on", "off"])]
        to: Option<String>,
    },
    /// Open a routine's newest run in the browser, where it can be read and continued; alone,
    /// the newest run of any
    Open {
        /// A routine's name or id
        routine: Option<String>,
    },
    /// A page of a run's log: what it was asked, what it called, how it ended
    ///
    /// Not from inside a resident: a run's log quotes whatever the run read.
    Log {
        /// A routine's name or id, for its newest run; or a run's id or link
        who: String,
    },
    /// Say something to a run, as its next turn: it answers on claude.ai, where the link goes;
    /// the user's alone
    Say {
        /// A routine's name or id, for its newest run; or a run's id or link
        who: String,
        #[arg(allow_hyphen_values = true)]
        text: String,
    },
    /// Make a routine on claude.ai, paused: it runs once the user resumes it
    ///
    /// The schedule is cron in this Mac's time, as a ritual's; claude.ai keeps it in UTC, and
    /// runs a routine at most hourly. Each connector needs a `CLOUD_CONNECTOR_<name>=<uuid>
    /// <url>` line in the config, and every tool of each runs unasked, writes too.
    /// `CLOUD_ENV=env_…` in the config names the cloud environment runs start in.
    Add {
        name: String,
        /// Five cron fields in local time, or @daily and the like
        #[arg(long)]
        schedule: String,
        /// The routine's prompt: everything its run is told, so it says where to report
        #[arg(long)]
        prompt_file: PathBuf,
        /// A GitHub repository its run checks out (org/repo); again for more
        #[arg(long = "repo")]
        repos: Vec<String>,
        /// A connector it is given; again for more. None named, it gets none
        #[arg(long = "connector")]
        connectors: Vec<String>,
        #[arg(long)]
        model: Option<String>,
    },
    /// A routine as claude.ai has it: schedule, model, repos, connectors and prompt
    Show { routine: String },
    /// Pause a routine: it fires no more until resumed
    Pause { routine: String },
    /// Resume a routine, after showing what it does; the user's alone
    Resume {
        routine: String,
        /// Without asking first
        #[arg(long)]
        yes: bool,
    },
    /// Run a routine now, paused or not, after showing what it does; the user's alone
    Run {
        routine: String,
        /// Without asking first
        #[arg(long)]
        yes: bool,
    },
}

pub fn main(cmd: Option<Cmd>) -> Result<(), String> {
    match cmd.unwrap_or(Cmd::List) {
        Cmd::List => list(),
        Cmd::Watch { to } => watch(to.as_deref()),
        Cmd::Open { routine } => open(routine.as_deref()),
        Cmd::Log { who } => say(request(Request::CloudLog { who }, false)),
        Cmd::Add { name, schedule, prompt_file, repos, connectors, model } => {
            let prompt = std::fs::read_to_string(&prompt_file)
                .map_err(|e| format!("{}: {e}", prompt_file.display()))?;
            let a = CloudAdd { name, schedule, prompt, repos, connectors, model };
            say(request(Request::CloudAdd(a), false))
        }
        Cmd::Say { who, text } => say(request(Request::CloudSay { who, text }, false)),
        Cmd::Show { routine } => say(request(Request::CloudShow { who: routine }, false)),
        Cmd::Pause { routine } => act(routine, CloudAct::Pause, true),
        Cmd::Resume { routine, yes } => act(routine, CloudAct::Resume, yes),
        Cmd::Run { routine, yes } => act(routine, CloudAct::Run, yes),
    }
}

/// A routine paused, or, once the user has read what it does and said so, resumed or run.
fn act(who: String, act: CloudAct, yes: bool) -> Result<(), String> {
    let doing = || request(Request::CloudAct { who: who.clone(), act }, false);
    if act == CloudAct::Pause {
        return say(doing());
    }
    if resident() {
        return Err(format!(
            "the user runs and resumes routines, after reading what {who} does (gensokyo cloud \
             show); tell them it is ready"
        ));
    }
    // What is resumed is what was shown: by its id, whatever a list in between says.
    let shown = match request(Request::CloudShow { who: who.clone() }, false)? {
        Reply::Done { message, .. } => message,
        other => return Err(format!("unexpected reply {other:?}")),
    };
    println!("{shown}");
    let page = shown.lines().find_map(|l| l.trim_start().strip_prefix("page"));
    let id = page.and_then(|p| p.trim().strip_prefix(&cloud::routine_link("")));
    let id =
        id.filter(|id| cloud::valid_id(id, "trig_")).ok_or("no routine's id in what was shown")?;
    let doing = || request(Request::CloudAct { who: id.to_string(), act }, false);
    if !yes {
        let q = if act == CloudAct::Run { "run it now" } else { "resume it" };
        if !std::io::stdin().is_terminal() {
            return Err(format!("no terminal to ask, so it is left as it was (--yes to {q})"));
        }
        print!("{q}? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        let _ = std::io::stdin().lock().read_line(&mut answer);
        if !matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES") {
            println!("left as it was");
            return Ok(());
        }
    }
    say(doing())
}

fn resident() -> bool {
    std::env::var_os("GENSOKYO_RESIDENT").is_some_and(|r| !r.is_empty())
}

fn watch(to: Option<&str>) -> Result<(), String> {
    if let Some(v) = to {
        if resident() {
            return Err("the cloud watch spends the user's plan: theirs to turn on or off".into());
        }
        paths::set_config("CLOUD_WATCH", v).map_err(|e| format!("the config: {e}"))?;
    }
    println!("{}", status());
    Ok(())
}

/// The watch's setting, for `cloud watch` and the doctor.
pub(super) fn status() -> String {
    match on() {
        true => "the cloud watch is on: the daemon looks at the routines on claude.ai".into(),
        false => "the cloud watch is off (gensokyo cloud watch on)".into(),
    }
}

/// What the running daemon's watch has seen; none while it is off. Nothing here asks claude.ai:
/// only the watch spends the plan.
fn seen() -> Result<Option<CloudWatch>, String> {
    if !on() {
        return Ok(None);
    }
    match request(Request::Rituals, false) {
        Ok(Reply::Rituals { cloud, .. }) => Ok(cloud),
        Ok(other) => Err(format!("unexpected reply {other:?}")),
        Err(Error::NotRunning) => Err("the daemon is not running, and it is what watches".into()),
        Err(e) => Err(e.into()),
    }
}

fn list() -> Result<(), String> {
    let Some(w) = seen()? else {
        println!("{}", status());
        return Ok(());
    };
    let now = crate::daemon::store::now();
    match w.listed {
        Some(t) => println!("listed {} ago", crate::tele::age((now - t).max(0) as u64)),
        None => println!("not listed yet: the watch looks once the daemon has been up a minute"),
    }
    if let Some(e) = &w.failing {
        println!("the last look failed: {e}");
    }
    if w.listed.is_some() && w.routines.is_empty() {
        println!("no routines on claude.ai: {}routines", cloud::WEB);
    }
    let wide = w.routines.iter().map(|r| r.name.chars().count()).max().unwrap_or(0).max(7);
    for (i, r) in w.routines.iter().enumerate() {
        if i == 0 {
            println!(
                "  {:<wide$} {:<4} {:<18} {:<26} newest run",
                "routine", "", "schedule", "next"
            );
        }
        let on = if r.enabled { "on" } else { "off" };
        let cron = r.cron.as_ref().map_or("by hand".into(), |c| format!("{c} UTC"));
        let next = match (r.enabled, r.next_run, &r.next_run_local) {
            (false, ..) => "paused".to_string(),
            (true, Some(t), Some(l)) => {
                format!("{l} (in {})", crate::tele::age((t - now).max(0) as u64))
            }
            _ => String::new(),
        };
        let going = match r.ran {
            Some(_) if r.going => " (going)".to_string(),
            Some(t) => format!(" ({} ago)", crate::tele::age((now - t).max(0) as u64)),
            None => String::new(),
        };
        println!("  {:<wide$} {on:<4} {cron:<18} {next:<26} {}{going}", r.name, r.url);
    }
    Ok(())
}

fn open(routine: Option<&str>) -> Result<(), String> {
    let w = seen()?.ok_or_else(status)?;
    let r = pick(&w.routines, routine)?;
    println!("{}: {}", r.name, r.url);
    cloud::browse(&r.url)
}

/// The routine named, or with none, the one whose newest run is newest.
fn pick<'a>(rs: &'a [RoutineInfo], name: Option<&str>) -> Result<&'a RoutineInfo, String> {
    match name {
        Some(n) => match rs.iter().filter(|r| r.name == n || r.id == n).collect::<Vec<_>>()[..] {
            [r] => Ok(r),
            [] => Err(format!("no routine {n} on claude.ai (gensokyo cloud lists them)")),
            _ => Err(format!("more than one routine is named {n}: give its id (trig_…)")),
        },
        None => rs
            .iter()
            .filter(|r| r.ran.is_some())
            .max_by_key(|r| r.ran)
            .ok_or_else(|| "no routine has run yet (gensokyo cloud lists them)".into()),
    }
}
