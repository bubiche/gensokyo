//! `gensokyo cloud …`: the user's routines on claude.ai, as the daemon watches them.

use super::{Error, request, say};
use crate::cloud;
use crate::paths;
use crate::proto::{CloudWatch, Reply, Request, RoutineInfo};
use clap::Subcommand;

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
}

pub fn main(cmd: Option<Cmd>) -> Result<(), String> {
    match cmd.unwrap_or(Cmd::List) {
        Cmd::List => list(),
        Cmd::Watch { to } => watch(to.as_deref()),
        Cmd::Open { routine } => open(routine.as_deref()),
        Cmd::Log { who } => say(request(Request::CloudLog { who }, false)),
    }
}

fn watch(to: Option<&str>) -> Result<(), String> {
    if let Some(v) = to {
        if std::env::var_os("GENSOKYO_RESIDENT").is_some_and(|r| !r.is_empty()) {
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

fn on() -> bool {
    paths::config("CLOUD_WATCH").as_deref() == Some("on")
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
    for (i, r) in w.routines.iter().enumerate() {
        if i == 0 {
            println!("  {:<18} {:<4} {:<18} {:<26} newest run", "routine", "", "schedule", "next");
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
        println!("  {:<18} {on:<4} {cron:<18} {next:<26} {}{going}", r.name, r.url);
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
        Some(n) => rs
            .iter()
            .find(|r| r.name == n || r.id == n)
            .ok_or_else(|| format!("no routine {n} on claude.ai (gensokyo cloud lists them)")),
        None => rs
            .iter()
            .filter(|r| r.ran.is_some())
            .max_by_key(|r| r.ran)
            .ok_or_else(|| "no routine has run yet (gensokyo cloud lists them)".into()),
    }
}
