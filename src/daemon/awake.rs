//! Keeping the Mac from idle-sleeping while work runs: a resident in a turn (helpers and ritual
//! runs are residents too) or a headless run. `caffeinate -i -w <daemon>` holds it, so the
//! display still sleeps, the screen still locks and a closed lid still sleeps the Mac; `-w` ends
//! it with the daemon, however that goes. A dialog or a question is not work, so a resident
//! stuck at one never holds the Mac, nor does a schedule: a fire the Mac slept through comes on
//! waking. The setting is the user's, `KEEP_AWAKE` in the config.

use super::log::log;
use super::shrine::{Shared, Shrine, touch};
use crate::paths;
use crate::proto::State;
use serde_json::json;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::time::Instant;

/// How long it holds on once the work has run out, so back-to-back turns do not start it again.
const LINGER: Duration = Duration::from_secs(30);

fn linger() -> Duration {
    let ms = std::env::var("GENSOKYO_AWAKE_LINGER_MS").ok().and_then(|v| v.parse().ok());
    ms.map_or(LINGER, Duration::from_millis)
}

pub(super) struct Awake {
    /// The user's setting.
    pub(super) on: bool,
    /// The caffeinate holding the Mac, while one does.
    held: Option<Child>,
    /// Since the work ran out, while it still holds.
    lull: Option<Instant>,
    /// Caffeinate would not start: tried again only once the work has run out.
    failed: bool,
}

impl Awake {
    pub(super) fn from_config() -> Awake {
        let on = paths::config("KEEP_AWAKE").as_deref() != Some("off");
        Awake { on, held: None, lull: None, failed: false }
    }

    pub(super) fn held(&self) -> bool {
        self.held.is_some()
    }
}

/// What holds the Mac awake now: whoever is in a turn, then each ritual running headless.
fn work(sh: &Shrine) -> Vec<String> {
    let busy = sh.entries.iter().filter(|e| e.handle.is_some() && e.aware.state() == State::Busy);
    let mut w: Vec<String> = busy.map(|e| e.rec.name.clone()).collect();
    let mut runs: Vec<&String> =
        sh.rites.headless.iter().filter(|(_, n)| **n > 0).map(|(s, _)| s).collect();
    runs.sort();
    w.extend(runs.into_iter().map(|slug| format!("a headless {slug} run")));
    w
}

/// Looks again whenever the shrine changes, and once a lull has lasted.
pub(super) async fn keep(shrine: Shared) {
    let mut rx = shrine.borrow().changed.subscribe();
    loop {
        rx.borrow_and_update();
        let again = step(&mut shrine.borrow_mut());
        tokio::select! {
            c = rx.changed() => if c.is_err() { return },
            _ = tokio::time::sleep_until(again.unwrap_or_else(Instant::now)), if again.is_some() => {}
        }
    }
}

/// Caffeinate started or let go as the work and the setting say; when a lull should let it go
/// later, the instant to look again.
fn step(sh: &mut Shrine) -> Option<Instant> {
    let working = !work(sh).is_empty();
    let on = sh.awake.on && !sh.quitting;
    let a = &mut sh.awake;
    let was = a.held.is_some();
    // Gone on its own (killed by hand, say): started again below.
    if a.held.as_mut().is_some_and(|c| !matches!(c.try_wait(), Ok(None))) {
        a.held = None;
    }
    let mut again = None;
    if on && working {
        a.lull = None;
        if a.held.is_none() && !a.failed {
            match start() {
                Ok(c) => a.held = Some(c),
                Err(e) => {
                    a.failed = true;
                    log(json!({"ev": "awake", "error": format!("caffeinate: {e}")}));
                }
            }
        }
    } else if let Some(mut c) = a.held.take() {
        let since = *a.lull.get_or_insert_with(Instant::now);
        // Turned off or stopping lets go at once; a lull, once it has lasted.
        match on && since.elapsed() < linger() {
            true => {
                again = Some(since + linger());
                a.held = Some(c);
            }
            false => {
                let _ = c.start_kill();
                tokio::task::spawn_local(async move { c.wait().await });
                a.lull = None;
            }
        }
    }
    if !working {
        a.failed = false;
    }
    let held = a.held.is_some();
    if held != was {
        log(json!({"ev": "awake", "held": held, "for": work(sh)}));
        touch(sh);
    }
    again
}

fn start() -> std::io::Result<Child> {
    let program = std::env::var_os("GENSOKYO_CAFFEINATE").filter(|p| !p.is_empty());
    let mut c = Command::new(program.unwrap_or_else(|| "/usr/bin/caffeinate".into()));
    c.args(["-i", "-w", &std::process::id().to_string()]);
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    c.spawn()
}

/// The setting turned on or off: kept in the config first, so a config that cannot be written
/// changes nothing.
pub(super) fn set(sh: &mut Shrine, on: bool) -> Result<(), String> {
    let v = if on { "on" } else { "off" };
    paths::set_config("KEEP_AWAKE", v).map_err(|e| format!("could not write the config: {e}"))?;
    log(json!({"ev": "awake", "on": on}));
    sh.awake.on = on;
    touch(sh);
    step(sh);
    Ok(())
}

/// The setting, and whom it holds the Mac awake for.
pub(super) fn status(sh: &Shrine) -> String {
    if !sh.awake.on {
        return "keep-awake is off: the Mac idle-sleeps as it would without gensokyo".into();
    }
    let w = work(sh);
    match (w.is_empty(), sh.awake.held()) {
        (false, true) => format!("keep-awake is on: holding the Mac awake for {}", w.join(", ")),
        (false, false) => "keep-awake is on, but caffeinate would not start (daemon.log)".into(),
        (true, true) => "keep-awake is on: the work is done, and it lets the Mac sleep soon".into(),
        (true, false) => "keep-awake is on: nothing is running, so the Mac may sleep".into(),
    }
}
