//! `gensokyo cloud …`: the user's routines on claude.ai, as the daemon watches them.

use crate::paths;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Cmd {
    /// Watch the routines on claude.ai, and say when a run's turn ends; alone, whether it does
    ///
    /// Off by default: every look is a short `claude -p` on haiku (about $0.001), on an API
    /// Claude Code does not document. On, the daemon lists the routines hourly and after each
    /// one's time, and a run's turn until it ends. Kept in the config as CLOUD_WATCH.
    Watch {
        #[arg(value_parser = ["on", "off"])]
        to: Option<String>,
    },
}

pub fn main(cmd: Option<Cmd>) -> Result<(), String> {
    match cmd.unwrap_or(Cmd::Watch { to: None }) {
        Cmd::Watch { to } => watch(to.as_deref()),
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
    match paths::config("CLOUD_WATCH").as_deref() == Some("on") {
        true => "the cloud watch is on: the daemon looks at the routines on claude.ai".into(),
        false => "the cloud watch is off (gensokyo cloud watch on)".into(),
    }
}
