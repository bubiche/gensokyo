//! Rituals in the daemon: the clock that fires them, every target, `overlap`, `catch_up`, and
//! the timetable's requests; headless runs are `headless.rs` and `keep` is `keep.rs`. The files and the rules about them
//! are `crate::ritual`; the schedule is `crate::cron`. This decides and acts.

use super::aware::{STARTING, TYPING};
use super::cards::deliver;
use super::log::log;
use super::registry;
use super::shrine::{Shared, Shrine, Start, recall, start, taken, valid_name};
use crate::cron::{self, Schedule, Why};
use crate::hooks;
use crate::proto::{self, Reply, RitualInfo, RitualVerb};
use crate::ritual::{self, Dir, Ritual, Target, Trust, when};
use crate::tele;
use jiff::tz::TimeZone;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

/// How often the schedules are read. A wall-clock tick, never a sleep until the next fire: a
/// sleep across a lid close wakes up however late it likes.
const TICK: Duration = Duration::from_secs(20);

/// A fire queued behind a run that took this long to get out of its way is dropped: the prompt
/// was written for its minute.
const QUEUE_LIFE: i64 = 3600;

/// How long a persistent ritual's recalled session, or a named resident, may take to start up
/// (to be listed by the registry, which is asked every few seconds meanwhile) before the fire
/// is given up.
const READY_WAIT: Duration = Duration::from_secs(60);

/// `deliver: idle`: how long a resident must have been idle, by its last hook, before a fire is
/// typed into it. A Stop is its last hook while it rests, so this is the time since its turn
/// ended: long enough for a reply the user is reading to be seen first.
const IDLE_HOLD: Duration = Duration::from_secs(10);

/// `deliver: idle`: fires still waiting after this, on the ritual clock, are dropped: from the
/// first of those that took each other's place, so a ritual that fires often is still given up
/// on, and says so, while its resident stays busy.
const HOLD_MOST: Duration = Duration::from_secs(4 * 3600);

/// A length from the environment in ms, for tests; else `d`.
fn env_ms(var: &str, d: Duration) -> Duration {
    std::env::var(var).ok().and_then(|v| v.parse().ok()).map_or(d, Duration::from_millis)
}

/// What the daemon keeps about rituals between ticks. None of it outlives the daemon: the
/// stamps and the journal are on disk, and a queued fire or a complaint is worth no more than
/// this run of it.
#[derive(Default)]
pub(super) struct Rites {
    /// A fire held behind a run still going: the minute it was for.
    queued: HashMap<String, i64>,
    /// The last thing said about a ritual that cannot fire, so it is said once.
    complained: HashMap<String, String>,
    /// Headless runs in flight. A daemon that stops forgets them (they carry on without it).
    pub(super) headless: HashMap<String, u32>,
    /// Ritual runs being asked to leave.
    pub(super) reaping: HashSet<String>,
    /// The last "not sent" notice per ritual: a missing resident is news once, not every fire.
    undelivered: HashMap<String, String>,
    /// Probed fires under way, one at most per ritual, from the probe until its prompt is
    /// delivered or not: the probe's process group while it runs.
    probing: HashMap<String, i32>,
    /// `.claude.json`, as of its mtime: it is large, and the listing wants it every tick.
    trust: Option<(SystemTime, Rc<Trust>)>,
    /// Fires waiting for their resident to be idle (`deliver: idle`), by ritual and resident:
    /// the newest one's number. An older one that finds it is not the newest gives up.
    pub(super) waiting: HashMap<(String, String), Waiter>,
    held: u64,
    /// Rituals whose worktree git is making for a run.
    making: HashSet<String>,
    /// What a branch ritual's probe printed that was left out, said once per daemon.
    warned: HashSet<(String, String)>,
    /// The listing last sent to watchers.
    listing: Vec<RitualInfo>,
    /// When the clock last ticked. Here rather than in the clock: one started again after a
    /// panic carries on from it, instead of taking its first tick for a start and making up a
    /// fire.
    ticked: Option<i64>,
}

/// The ritual clock: `$GENSOKYO_NOW_FILE`'s epoch seconds when set, so tests move time by
/// hand; else the wall clock. Stamps, the journal, queue life and `keep` all read this.
pub(super) fn now() -> i64 {
    static FILE: OnceLock<Option<PathBuf>> = OnceLock::new();
    FILE.get_or_init(|| std::env::var_os("GENSOKYO_NOW_FILE").map(PathBuf::from))
        .as_ref()
        .and_then(|f| std::fs::read_to_string(f).ok()?.trim().parse().ok())
        .unwrap_or_else(super::store::now)
}

fn load(sh: &Shrine) -> (Vec<Ritual>, Vec<PathBuf>) {
    let share = sh.share.as_deref();
    ritual::load(&ritual::dirs(share), share)
}

/// The trust file, read again only when it has changed.
pub(super) fn trust(shrine: &Shared) -> Rc<Trust> {
    let mtime = std::fs::metadata(Trust::path()).and_then(|m| m.modified()).ok();
    let mut sh = shrine.borrow_mut();
    match (&sh.rites.trust, mtime) {
        (Some((at, t)), Some(m)) if *at == m => t.clone(),
        _ => {
            let t = Rc::new(Trust::load());
            sh.rites.trust = mtime.map(|m| (m, t.clone()));
            t
        }
    }
}

/// Every `TICK`: whatever has come round. The daemon's first tick, and one after a gap in the
/// ticking (a lid closed), may also make up the most recent fire missed.
pub(super) async fn clock(shrine: Shared) {
    let every = std::env::var("GENSOKYO_TICK_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(TICK, |ms: u64| Duration::from_millis(ms.max(10)));
    let gap = (3 * every.as_secs() as i64).max(60);
    loop {
        let t = now();
        let last = shrine.borrow_mut().rites.ticked.replace(t);
        let catch = last.is_none_or(|l| t - l > gap);
        if let Some(l) = last.filter(|_| catch) {
            log(json!({"ev": "ritual", "gap_s": t - l}));
        }
        tick(&shrine, t, catch);
        tokio::time::sleep(every).await;
    }
}

fn tick(shrine: &Shared, now: i64, catch: bool) {
    if shrine.borrow().quitting {
        return;
    }
    let tz = TimeZone::system();
    let (rs, bad) = load(&shrine.borrow());
    let mut trusted: Option<Rc<Trust>> = None;
    for r in rs.iter().filter(|r| r.enabled()) {
        let d = Dir::of(&r.slug);
        let s = match Schedule::parse(&r.schedule) {
            Ok(s) => s,
            Err(e) => {
                complain(shrine, r, &d, now, &format!("schedule: {e}"));
                continue;
            }
        };
        let (why, minute) = match cron::decide(&s, d.stamp(), now, catch && r.catch_up(), &tz) {
            Some(x) => x,
            None => {
                // From now on a missed minute is a miss: none before gensokyo saw the ritual.
                if d.stamp().is_none() {
                    let _ = d.set_stamp(now - now.rem_euclid(60));
                }
                // Nothing is due, which is the one moment a fire held behind a run gets its turn.
                match (r.overlap == "queue").then(|| queued(shrine, r, &d, now, &tz)).flatten() {
                    Some(m) => (Why::Queued, m),
                    None => continue,
                }
            }
        };
        let trusted = trusted.get_or_insert_with(|| trust(shrine));
        if let Some(p) = ritual::problem(r, now, &tz, trusted) {
            complain(shrine, r, &d, now, &p);
            continue;
        }
        // Before the run starts: a fire is once per minute, and launching takes long enough for
        // another tick to land in the same one. A queued fire already stamped its minute.
        if why != Why::Queued
            && let Err(e) = d.set_stamp(minute)
        {
            // Unstamped, the same minute would fire again on every tick inside it.
            complain(shrine, r, &d, now, &format!("could not write its stamp: {e}"));
            continue;
        }
        let label = format!("{why} {}", when(minute, &tz));
        let mut alongside = false;
        // Only a run of its own can collide with itself: a prompt typed into a session that is
        // mid-turn is Claude Code's to queue.
        if r.target() == Target::New && running(&shrine.borrow(), &r.slug) {
            match r.overlap.as_str() {
                "parallel" => alongside = true,
                // A probe's fire is held back quietly: the probe has not said whether it
                // has anything, and a change waits for a later fire anyway.
                "queue" => {
                    shrine.borrow_mut().rites.queued.insert(r.slug.clone(), minute);
                    if r.when.is_none() {
                        let text = format!("queued ({label}): the last run is still going");
                        d.note(now, "queued", &text);
                    }
                    continue;
                }
                _ => {
                    if r.when.is_none() {
                        let text = format!("skipped ({label}): the last run is still going");
                        d.note(now, "skipped", &text);
                    }
                    continue;
                }
            }
        }
        shrine.borrow_mut().rites.queued.remove(&r.slug);
        if r.when.is_some() {
            let _ = probed(shrine, r, label, alongside, false);
            continue;
        }
        // A complaint stands until a fire gets through, so a failing start is said once too.
        match fire(shrine, r, &d, &label, alongside, now, None) {
            Ok(_) => _ = shrine.borrow_mut().rites.complained.remove(&r.slug),
            Err(e) => complain(shrine, r, &d, now, &e),
        }
    }
    super::keep::reap(shrine, now);
    super::keep::orphans(shrine, now);
    push(shrine, &rs, &bad);
}

/// A fire waiting behind a run whose turn has come; one too old is dropped, and says so.
fn queued(shrine: &Shared, r: &Ritual, d: &Dir, now: i64, tz: &TimeZone) -> Option<i64> {
    let at = *shrine.borrow().rites.queued.get(&r.slug)?;
    if now - at > QUEUE_LIFE {
        shrine.borrow_mut().rites.queued.remove(&r.slug);
        let took = tele::age(QUEUE_LIFE as u64);
        let text = format!(
            "dropped the fire queued for {}: the run in its way took over {took}",
            when(at, tz)
        );
        d.note(now, "dropped", &text);
        return None;
    }
    (!running(&shrine.borrow(), &r.slug)).then_some(at)
}

/// A fire of a ritual with a probe: the probe first, in a task of its own, and the fire only if
/// it said something new since the last fire that launched (by hand, whatever it said). Its
/// minute is already stamped, so a fire the probe holds back is no miss for `catch_up`; and
/// it is not journalled: at `every 5m` that would be 288 lines a day.
fn probed(
    shrine: &Shared,
    r: &Ritual,
    label: String,
    alongside: bool,
    by_hand: bool,
) -> Result<(), String> {
    let argv = ritual::probe(r)?.ok_or("it has no probe")?;
    if shrine.borrow_mut().rites.probing.insert(r.slug.clone(), 0).is_some() {
        return Err("its probe, or the fire it set off, is under way; try again soon".into());
    }
    let (shrine, slug) = (shrine.clone(), r.slug.clone());
    let hold = Hold(shrine.clone(), slug.clone());
    tokio::task::spawn_local(async move {
        let d = Dir::of(&slug);
        let Some((r, said)) = probe_and_read(&shrine, &slug, &argv, &d, by_hand).await else {
            return;
        };
        let now = now();
        log(
            json!({"ev": "ritual", "slug": slug, "probe": said.failed.as_deref().unwrap_or("new")}),
        );
        if r.target() == Target::Branch {
            drop(hold);
            return nudge(&shrine, &r, &d, &said, &label, by_hand);
        }
        // The probe took its time: a run may have started meanwhile. The change waits for the
        // next fire, since nothing was marked fired.
        if r.target() == Target::New && !alongside && running(&shrine.borrow(), &slug) {
            let text = format!("skipped ({label}): the last run is still going");
            return d.note(now, "skipped", &text);
        }
        let mut run = r.clone();
        run.prompt = format!("{}\n\n{}", r.prompt, super::probe::sentence(&r, &d, &said));
        let why = match &said.failed {
            None if by_hand => "",
            None => ", its probe said something new",
            Some(_) => ", its probe failed",
        };
        let key = Some(Fired { key: said.key, _hold: Some(hold) });
        match fire(&shrine, &run, &d, &format!("{label}{why}"), alongside, now, key) {
            Ok(_) => _ = shrine.borrow_mut().rites.complained.remove(&slug),
            Err(e) => complain(&shrine, &r, &d, now, &e),
        }
    });
    Ok(())
}

/// The probe run, and the ritual read again after it: what fires is the ritual as it is now,
/// which may have been removed, paused or changed while the probe ran. None when nothing is to
/// fire: the daemon is stopping, the output is unchanged, or the ritual is no longer the one
/// the probe was run for.
async fn probe_and_read(
    shrine: &Shared,
    slug: &str,
    argv: &[String],
    d: &Dir,
    by_hand: bool,
) -> Option<(Ritual, super::probe::Said)> {
    let (cwd, branch) = {
        let sh = shrine.borrow();
        let r = load(&sh).0.into_iter().find(|r| r.slug == slug);
        if sh.quitting {
            return None;
        }
        let r = r?;
        (r.cwd.clone().unwrap_or_else(crate::paths::home), r.target() == Target::Branch)
    };
    let set = |pid| _ = shrine.borrow_mut().rites.probing.insert(slug.into(), pid);
    let said = super::probe::run(argv, &cwd, d, set).await;
    set(0);
    // A branch ritual measures each branch against what it sent, so unchanged output still
    // reaches a resident that has just moved onto a branch.
    if shrine.borrow().quitting || (!by_hand && !branch && super::probe::same(d, &said)) {
        return None;
    }
    let r = load(&shrine.borrow()).0.into_iter().find(|r| r.slug == slug)?;
    let same_probe = ritual::probe(&r).ok().flatten().as_deref() == Some(argv);
    if !same_probe || !(by_hand || r.enabled()) {
        return None;
    }
    if let Some(p) = ritual::problem(&r, now(), &TimeZone::system(), &trust(shrine)) {
        complain(shrine, &r, d, now(), &p);
        return None;
    }
    Some((r, said))
}

/// What a probe said that set a fire off. While it is held, the ritual's next fire is not
/// probed: measured against a change still on its way, that one would send it again. A fire
/// that waits for an idle resident holds nothing: a newer one takes its place instead.
struct Fired {
    key: Vec<u8>,
    _hold: Option<Hold>,
}

/// A ritual's place in `probing`, given up when the fire it led to is delivered or not.
struct Hold(Shared, String);

impl Drop for Hold {
    fn drop(&mut self) {
        match self.0.try_borrow_mut() {
            Ok(mut sh) => _ = sh.rites.probing.remove(&self.1),
            Err(_) => log(json!({"ev": "ritual", "slug": self.1, "error": "left probing"})),
        }
    }
}

/// A fire's prompt has reached its run: what its probe said is what the next fire is measured
/// against.
fn delivered(d: &Dir, key: Option<Fired>) {
    if let Some(f) = key {
        super::probe::commit(d, &f.key);
    }
}

/// Every probe running is killed: the daemon is stopping, and nothing would read what it says.
pub(super) fn stop_probes(sh: &Shrine) {
    sh.rites.probing.values().for_each(|g| super::probe::kill(*g));
}

/// Said once: a ritual that cannot fire is read again every tick, and so would its complaint be.
/// The stamp is left alone: a ritual that could not run has not run.
fn complain(shrine: &Shared, r: &Ritual, d: &Dir, now: i64, why: &str) {
    let mut sh = shrine.borrow_mut();
    if sh.rites.complained.get(&r.slug).is_some_and(|w| w == why) {
        return;
    }
    sh.rites.complained.insert(r.slug.clone(), why.into());
    d.note(now, "not-run", &format!("not run: {why}"));
    notice(&sh, &r.slug, why);
}

pub(super) fn notice(sh: &Shrine, slug: &str, text: &str) {
    log(json!({"ev": "ritual", "slug": slug, "notice": text}));
    let _ = sh.notices.send(Reply::Notice { text: format!("⏲ {slug}: {text}") });
}

/// A run of it still going: a resident it started that has not finished, or a headless run.
fn running(sh: &Shrine, slug: &str) -> bool {
    sh.rites.making.contains(slug)
        || sh.rites.headless.get(slug).is_some_and(|n| *n > 0)
        || sh.entries.iter().any(|e| {
            e.rec.ritual.as_deref() == Some(slug) && e.handle.is_some() && !e.aware.finished()
        })
}

/// Fires it the way it says; `label` says why ("due 2026-09-28 09:05", "by hand"). What to
/// tell someone who asked for it by hand.
fn fire(
    shrine: &Shared,
    r: &Ritual,
    d: &Dir,
    label: &str,
    alongside: bool,
    now: i64,
    key: Option<Fired>,
) -> Result<String, String> {
    if let (Some(slug), Target::New) = (r.worktree.clone(), r.target()) {
        return in_worktree(shrine, r, label, alongside, key, slug);
    }
    let cwd = r.cwd.as_deref().map(crate::paths::short).unwrap_or_default();
    log(json!({"ev": "ritual", "slug": r.slug, "ran": label, "alongside": alongside}));
    let said = match r.target() {
        _ if r.headless() => {
            super::headless::start(shrine, r, d, label)?;
            format!("{} is running headless in {cwd}: no pane, and nothing to watch", r.slug)
        }
        Target::New => {
            fresh(shrine, r, d, r.keep_secs())?;
            format!("{} is running in {cwd}", r.slug)
        }
        Target::Branch => {
            return Err("a branch ritual sends through its probe, which it has not run".into());
        }
        // Typed in later, by a task of its own: the journal says `sent` or `not sent` then.
        t => {
            let to = match &t {
                Target::Resident(who) => who.clone(),
                _ => "the session it keeps".into(),
            };
            let said = format!("{}'s prompt is on its way to {to}", r.slug);
            // Held for an idle resident, the probe runs on meanwhile: from now, not from when
            // the task below gets its turn, or the next minute's probe may find it still held.
            let key = match key {
                Some(Fired { key, .. }) if r.deliver_idle() => Some(Fired { key, _hold: None }),
                k => k,
            };
            let (shrine, r, label) = (shrine.clone(), r.clone(), label.to_string());
            tokio::task::spawn_local(async move {
                match t {
                    Target::Resident(who) => send(&shrine, &r, &who, &label, key).await,
                    Target::Branch => {}
                    _ => persistent(&shrine, &r, &label, key).await,
                }
            });
            return Ok(said);
        }
    };
    delivered(d, key);
    let beside = if alongside { ", alongside the run that was still going" } else { "" };
    d.note(now, "ran", &format!("ran ({label}){beside}"));
    Ok(said)
}

/// A run in the ritual's worktree: found or made first, by a task of its own (a checkout takes
/// seconds), then fired there. Meanwhile the ritual counts as running.
fn in_worktree(
    shrine: &Shared,
    r: &Ritual,
    label: &str,
    alongside: bool,
    key: Option<Fired>,
    slug: String,
) -> Result<String, String> {
    let cwd = r.cwd.clone().ok_or("cwd: missing")?;
    shrine.borrow_mut().rites.making.insert(r.slug.clone());
    let said = format!("{} is making its worktree {slug}, then runs there", r.slug);
    let (shrine, mut run, label) = (shrine.clone(), r.clone(), label.to_string());
    tokio::task::spawn_local(async move {
        let ask = proto::WorktreeAsk { slug, ..Default::default() };
        let prefix = crate::paths::config("BRANCH_PREFIX").unwrap_or_default();
        let made = super::worktree::make(std::path::Path::new(&cwd), &ask, &prefix).await;
        shrine.borrow_mut().rites.making.remove(&run.slug);
        let d = Dir::of(&run.slug);
        let made = match made {
            Ok(m) => m,
            Err(e) => return complain(&shrine, &run, &d, now(), &format!("worktree: {e}")),
        };
        if let Some(n) = &made.note {
            d.note(now(), "worktree", n);
        }
        // The repository's top may be above the trusted cwd, and a run at the trust dialog
        // would be alive and stuck.
        if !run.headless() && !trust(&shrine).trusted(std::path::Path::new(&made.path)) {
            let why = format!(
                "worktree: nothing has answered Claude Code's trust prompt for {} (open Claude \
                 Code there once and accept)",
                crate::paths::short(&made.path)
            );
            return complain(&shrine, &run, &d, now(), &why);
        }
        (run.cwd, run.worktree) = (Some(made.path), None);
        match fire(&shrine, &run, &d, &label, alongside, now(), key) {
            Ok(_) => _ = shrine.borrow_mut().rites.complained.remove(&run.slug),
            Err(e) => complain(&shrine, &run, &d, now(), &e),
        }
    });
    Ok(said)
}

/// A resident of its own for the run, with the prompt as its first argument. Its tab is named
/// after the ritual when that name is free.
fn fresh(shrine: &Shared, r: &Ritual, d: &Dir, keep: Option<u64>) -> Result<String, String> {
    let mut extra = ritual::args(r, &d.path);
    let mut take = |f: &str| {
        let i = extra.iter().position(|a| a == f)?;
        extra.remove(i);
        Some(extra.remove(i))
    };
    let (model, effort, mode) = (take("--model"), take("--effort"), take("--permission-mode"));
    let name = {
        let sh = shrine.borrow();
        (valid_name(&r.slug) && !taken(&sh, &r.slug)).then(|| r.slug.clone())
    };
    let s = Start {
        cwd: r.cwd.clone().unwrap_or_default(),
        name,
        model,
        effort,
        mode,
        prompt: Some(ritual::prompt_text(r, &d.memory())),
        ritual: Some(r.slug.clone()),
        keep,
        extra,
        owner: None,
        quiet: r.quiet(),
    };
    start(shrine, s).map(|res| res.id)
}

/// The session a persistent ritual keeps: typed into when it is here, recalled when it has
/// left, started when there has never been one.
async fn persistent(shrine: &Shared, r: &Ritual, label: &str, key: Option<Fired>) {
    let d = Dir::of(&r.slug);
    let id = d.session().filter(|id| {
        let sh = shrine.borrow();
        sh.entries.iter().any(|e| &e.rec.id == id) || sh.store.load_departed_id(id).is_some()
    });
    let Some(id) = id else {
        match fresh(shrine, r, &d, None) {
            Ok(id) => {
                delivered(&d, key);
                if let Err(e) = d.set_session(&id) {
                    // The next fire finds no session and starts yet another.
                    log(
                        json!({"ev": "ritual", "slug": r.slug, "session": id, "error": e.to_string()}),
                    );
                }
            }
            Err(e) => undelivered(shrine, r, &format!("could not start the session it keeps: {e}")),
        }
        return;
    };
    let here = shrine.borrow().entries.iter().any(|e| e.rec.id == id && e.handle.is_some());
    if !here && let Err(e) = recall(shrine, &id, None) {
        let why = format!("could not recall the session this ritual keeps ({id}): {e}");
        return undelivered(shrine, r, &why);
    }
    type_prompt(shrine, r, &id, label, key.map_or(Sent::Nothing, Sent::Probe)).await;
}

/// A resident the user manages, by name: the prompt alone. Typed in as a spell card is, so a
/// fire into a resident on screen can land after something the user had half typed there.
async fn send(shrine: &Shared, r: &Ritual, who: &str, label: &str, key: Option<Fired>) {
    let found = {
        let sh = shrine.borrow();
        let e = sh.entries.iter().find(|e| e.rec.name.eq_ignore_ascii_case(who));
        e.map(|e| (e.rec.id.clone(), e.handle.is_some()))
    };
    match found {
        None => undelivered(shrine, r, &format!("there is no resident called {who}")),
        Some((_, false)) => undelivered(
            shrine,
            r,
            &format!("{who} has left, and a departed screen has no prompt to type into"),
        ),
        Some((id, true)) => {
            type_prompt(shrine, r, &id, label, key.map_or(Sent::Nothing, Sent::Probe)).await
        }
    }
}

/// Journal and notice for a fire that did not get to its resident.
/// Every one is journaled; the notice only when it says something new.
fn undelivered(shrine: &Shared, r: &Ritual, why: &str) {
    Dir::of(&r.slug).note(now(), "not-sent", &format!("not sent: {why}"));
    let mut sh = shrine.borrow_mut();
    if sh.rites.undelivered.get(&r.slug).is_some_and(|w| w == why) {
        return;
    }
    sh.rites.undelivered.insert(r.slug.clone(), why.into());
    notice(&sh, &r.slug, &format!("{why}, so the fire was not delivered"));
}

/// The prompt into resident `id`'s input line, as a spell card goes: never into a dialog. One
/// that is still starting (just recalled) is waited for, up to `READY_WAIT`, and so is a card or
/// another prompt on its way in; a dialog is reported instead. With `deliver: idle` (the
/// default) it also waits, up to `HOLD_MOST` on the ritual clock, for the resident to be idle:
/// no turn running, no dialog, nothing half typed, and `IDLE_HOLD` since its last hook. Focus
/// plays no part. A newer fire for the same resident replaces a waiting one, and the probe goes
/// on meanwhile, so what is typed is the newest.
async fn type_prompt(shrine: &Shared, r: &Ritual, id: &str, label: &str, sent: Sent) {
    let idle = r.deliver_idle();
    let hold = env_ms("GENSOKYO_IDLE_HOLD_MS", IDLE_HOLD).as_millis() as i64;
    let ready_wait = env_ms("GENSOKYO_READY_WAIT_MS", READY_WAIT);
    let d = Dir::of(&r.slug);
    // Waiting, the probe's next fire may run: it replaces this one if it says something new.
    let (key, _hold, nudge) = match sent {
        Sent::Probe(f) if idle => (Some(f.key), None, None),
        Sent::Probe(f) => (Some(f.key), f._hold, None),
        Sent::Branch(n) => (None, None, Some(n)),
        Sent::Nothing => (None, None, None),
    };
    // Already sent as this fire began: a fire by hand, which sends what is there regardless.
    let sent_before = key.as_deref().is_some_and(|k| super::probe::same_key(&d, k));
    let slot = (r.slug.clone(), id.to_string());
    let (mine, replaced) = {
        let mut sh = shrine.borrow_mut();
        sh.rites.held += 1;
        let n = sh.rites.held;
        let fresh = Waiter {
            n,
            since: Instant::now(),
            first: now(),
            unlisted: None,
            held: false,
            told: false,
        };
        let was = sh.rites.waiting.insert(slot.clone(), fresh);
        if let Some(w) = was {
            sh.rites.waiting.insert(slot.clone(), Waiter { n, ..w });
        }
        (n, was.is_some())
    };
    let gone = |shrine: &Shared| {
        let mut sh = shrine.borrow_mut();
        if sh.rites.waiting.get(&slot).is_some_and(|w| w.n == mine) {
            sh.rites.waiting.remove(&slot);
        }
    };
    // Fresh once, for the dialog guard; while it starts up the poll keeps it current. A fire
    // taking a waiting one's place finds it fresh already.
    let claude = shrine.borrow().claude("");
    if let Some((claude, env)) = claude.filter(|_| !replaced) {
        let at = hooks::now_ms();
        if let Some(list) = registry::fetch(&claude, &env).await {
            registry::seen(shrine, &list, at);
        }
    }
    let (name, handle) = loop {
        let state = {
            let sh = shrine.borrow();
            if sh.quitting {
                drop(sh);
                gone(shrine);
                return undelivered(shrine, r, "the daemon is stopping");
            }
            let Some(w) = sh.rites.waiting.get(&slot).filter(|w| w.n == mine).copied() else {
                log(json!({"ev": "ritual", "slug": r.slug, "replaced": label}));
                return;
            };
            sh.entries.iter().find(|e| e.rec.id == id).map(|e| {
                let quiet = hooks::now_ms() - e.aware.heard() >= hold;
                (
                    e.rec.name.clone(),
                    e.handle.clone().filter(|h| h.exit().is_none()),
                    e.aware.blocked(),
                    e.aware.idle() && quiet,
                    w,
                )
            })
        };
        // Since when it has gone unlisted, kept across fires that take each other's place.
        let unlisted = {
            let starting = matches!(&state, Some((_, _, Some(why), ..)) if *why == STARTING);
            let mut sh = shrine.borrow_mut();
            sh.rites.waiting.get_mut(&slot).filter(|w| w.n == mine).and_then(|w| match starting {
                true => Some(*w.unlisted.get_or_insert_with(Instant::now)),
                false => w.unlisted.take().and(None),
            })
        };
        match state {
            None | Some((_, None, ..)) => {
                gone(shrine);
                return undelivered(shrine, r, "its resident has left");
            }
            Some((name, Some(h), None, ready, _)) if ready || !idle => break (name, h),
            // `deliver: now` waits only for what passes by itself.
            Some((name, _, Some(why), ..))
                if why == STARTING && unlisted.is_some_and(|t| t.elapsed() > ready_wait)
                    || !idle && why != STARTING && why != TYPING =>
            {
                gone(shrine);
                return undelivered(shrine, r, &format!("{name} {why}"));
            }
            Some((name, _, why, _, w)) if now() - w.first > HOLD_MOST.as_secs() as i64 => {
                gone(shrine);
                let waited = tele::age(HOLD_MOST.as_secs());
                // A prompt on its way in is a moment's state, not what held it for hours.
                let why = why.filter(|w| *w != TYPING).unwrap_or("is busy");
                return undelivered(
                    shrine,
                    r,
                    &format!("{name} was not idle for {waited}: it {why}"),
                );
            }
            Some((name, _, why, ..)) => {
                if idle {
                    held(shrine, r, &slot, &name, label, why);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    };
    gone(shrine);
    // A fire replaced by this one may have been typed while this one waited for the turn it
    // started to end: what it said is not said twice.
    let already = match &nudge {
        Some(n) => !super::branch::still(shrine, &d, id, n),
        None => !sent_before && key.as_deref().is_some_and(|k| super::probe::same_key(&d, k)),
    };
    if already {
        return log(json!({"ev": "ritual", "slug": r.slug, "already": label}));
    }
    let text =
        nudge.as_ref().map_or_else(|| ritual::prompt_text(r, &d.memory()), |n| n.text.clone());
    match deliver(shrine, id, &handle, &text, "prompt").await {
        Ok(()) => {
            if let Some(k) = &key {
                super::probe::commit(&d, k);
            }
            if let Some(n) = &nudge {
                super::branch::record(&d, n);
            }
            d.note(now(), "sent", &format!("sent to {name} ({label})"));
            shrine.borrow_mut().rites.undelivered.remove(&r.slug);
        }
        Err(why) => undelivered(shrine, r, &format!("{name} {why}")),
    }
}

/// What a typed prompt carries, to be recorded once it is in.
enum Sent {
    Nothing,
    /// A probe's output, committed so the next fire is measured against it.
    Probe(Fired),
    /// A branch's news, from `target: branch`; the prompt is its own.
    Branch(super::branch::Nudge),
}

/// A branch ritual's fire: each branch's news to the resident working on it, typed in as any
/// prompt for a resident is. A failed probe, or output of the wrong shape, is said once.
fn nudge(
    shrine: &Shared,
    r: &Ritual,
    d: &Dir,
    said: &super::probe::Said,
    label: &str,
    by_hand: bool,
) {
    if let Some(how) = &said.failed {
        return complain(shrine, r, d, now(), &format!("its probe failed ({how})"));
    }
    let out = said.key.strip_prefix(b"ok\n".as_slice()).unwrap_or(&said.key);
    let (sends, bad) = match super::branch::route(shrine, r, d, out, by_hand) {
        Ok(x) => x,
        Err(e) => return complain(shrine, r, d, now(), &e),
    };
    shrine.borrow_mut().rites.complained.remove(&r.slug);
    for b in bad {
        if shrine.borrow_mut().rites.warned.insert((r.slug.clone(), b.clone())) {
            d.note(now(), "left-out", &b);
        }
    }
    if by_hand && sends.is_empty() {
        let why = "no resident works on a branch its probe named with facts";
        return undelivered(shrine, r, why);
    }
    for (id, n) in sends {
        let (shrine, r, label) = (shrine.clone(), r.clone(), format!("{label}, {}", n.key));
        tokio::task::spawn_local(async move {
            type_prompt(&shrine, &r, &id, &label, Sent::Branch(n)).await;
        });
    }
}

/// A fire waiting for its resident to be idle (`deliver: idle`), by ritual and resident.
#[derive(Clone, Copy)]
pub(super) struct Waiter {
    /// The newest fire's number: an older one that finds another gives up.
    n: u64,
    /// When the first of the fires that took each other's place began to wait.
    since: Instant,
    /// The same, on the ritual clock.
    first: i64,
    /// Since when its resident has not been listed by the registry, while that lasts.
    unlisted: Option<Instant>,
    /// `held` is journaled once for a run of fires that take each other's place.
    held: bool,
    /// Told the user once that it waits on them: a dialog, or something typed and not sent.
    told: bool,
}

/// Journals that a fire is held, once, and says so to the user when what holds it is theirs to
/// clear: a dialog, or text typed and not sent, which counts as half typed until a prompt goes
/// in, Ctrl-C, `/clear`, or the Enter that runs a `/` or `!` command.
fn held(
    shrine: &Shared,
    r: &Ritual,
    slot: &(String, String),
    name: &str,
    label: &str,
    why: Option<&str>,
) {
    let mut sh = shrine.borrow_mut();
    // A moment's wait (the hold after a turn, a registry read) is not worth a line.
    let Some(w) = sh.rites.waiting.get_mut(slot).filter(|w| w.since.elapsed().as_secs() >= 2)
    else {
        return;
    };
    let theirs = why.is_some_and(|w| w != STARTING && w != TYPING);
    let (journal, tell) = (!w.held, !w.told && theirs);
    w.held = true;
    w.told |= tell;
    if journal {
        let why = why.unwrap_or("is busy");
        let text = format!("held for {name} ({label}), which {why}; sent once it is idle");
        Dir::of(&r.slug).note(now(), "held", &text);
    }
    if tell {
        notice(&sh, &r.slug, &format!("waiting for {name}, which {}", why.unwrap_or_default()));
    }
}

fn infos(shrine: &Shared, rs: &[Ritual]) -> Vec<RitualInfo> {
    let (now, tz, trust) = (now(), TimeZone::system(), trust(shrine));
    let sh = shrine.borrow();
    rs.iter()
        .map(|r| RitualInfo {
            running: running(&sh, &r.slug),
            ..ritual::info(r, now, &tz, &trust, &Dir::of(&r.slug))
        })
        .collect()
}

fn names(bad: &[PathBuf]) -> Vec<String> {
    bad.iter().map(|p| p.to_string_lossy().into_owned()).collect()
}

/// The timetable, for `rituals` (id 0: for `watch`).
pub(super) fn listing(shrine: &Shared, id: u64) -> Reply {
    let (rs, bad) = load(&shrine.borrow());
    Reply::Rituals { id, rituals: infos(shrine, &rs), unusable: names(&bad) }
}

/// The timetable to every watcher, when it has changed since they were last sent it.
fn push(shrine: &Shared, rs: &[Ritual], bad: &[PathBuf]) {
    let list = infos(shrine, rs);
    let mut sh = shrine.borrow_mut();
    if list != sh.rites.listing {
        sh.rites.listing = list.clone();
        let _ = sh.notices.send(Reply::Rituals { id: 0, rituals: list, unusable: names(bad) });
    }
}

/// `ritual {verb, name}`.
pub(super) fn verb(shrine: &Shared, verb: RitualVerb, name: &str) -> Result<String, String> {
    let (rs, _) = load(&shrine.borrow());
    let r = ritual::find(&rs, name)?.clone();
    let said = match verb {
        RitualVerb::Run => run(shrine, &r),
        RitualVerb::Enable | RitualVerb::Disable => {
            let on = verb == RitualVerb::Enable;
            Ok(ritual::toggle(&r, on, now(), &ritual::mine_dir())?.join("\n"))
        }
        RitualVerb::Remove => remove(shrine, &r, name),
    };
    repush(shrine);
    said
}

/// `push` after something other than a tick changed the listing.
pub(super) fn repush(shrine: &Shared) {
    let (rs, bad) = load(&shrine.borrow());
    push(shrine, &rs, &bad);
}

/// Fired now by hand, whatever the schedule says: how the user sees what it stops to ask.
fn run(shrine: &Shared, r: &Ritual) -> Result<String, String> {
    let (now, tz) = (now(), TimeZone::system());
    if let Some(p) = ritual::problem(r, now, &tz, &trust(shrine)) {
        return Err(format!("{}: {p}", r.slug));
    }
    let slug = &r.slug;
    {
        let sh = shrine.borrow();
        if sh.quitting {
            return Err("the daemon is stopping".into());
        }
        // Headless is a run of its own too, and `running` counts it.
        if (r.headless() || r.target() == Target::New) && running(&sh, slug) {
            return Err(match sh.rites.headless.get(slug).is_some_and(|n| *n > 0) {
                true => format!(
                    "{slug} is running headless right now, and both runs would write its notes; \
                     wait for it (gensokyo ritual log {slug} says when it started)"
                ),
                false => format!(
                    "{slug} is still running from last time (gensokyo list), and both runs \
                     would write its notes"
                ),
            });
        }
    }
    shrine.borrow_mut().rites.queued.remove(slug);
    let said = match &r.when {
        Some(w) => {
            probed(shrine, r, Why::ByHand.to_string(), false, true)
                .map_err(|e| format!("{slug}: {e}"))?;
            format!("{slug}: its probe ({w}) runs first, and it fires whatever that says")
        }
        None => fire(shrine, r, &Dir::of(slug), &Why::ByHand.to_string(), false, now, None)?,
    };
    Ok(match r.enabled() {
        true => said,
        false => {
            format!("{said}\n{slug} is disabled: this run is by hand, and the schedule stays off")
        }
    })
}

fn remove(shrine: &Shared, r: &Ritual, asked: &str) -> Result<String, String> {
    let slug = &r.slug;
    if !slug.eq_ignore_ascii_case(asked) {
        return Err(format!(
            "no ritual called '{asked}' - did you mean {slug}? (remove wants the whole name)"
        ));
    }
    let open = {
        let sh = shrine.borrow();
        if running(&sh, slug) {
            return Err(format!(
                "{slug} is running now, and that run writes its notes as it finishes: close it \
                 first, or wait for it"
            ));
        }
        if sh.rites.probing.contains_key(slug) {
            return Err(format!(
                "{slug}'s probe, or the fire it set off, is under way; try again in a minute"
            ));
        }
        let e = sh.entries.iter().find(|e| e.rec.ritual.as_deref() == Some(slug.as_str()));
        e.map(|e| e.rec.name.clone())
    };
    let share = shrine.borrow().share.clone();
    let mut out = ritual::remove(r, &Dir::of(slug), share.as_deref())?;
    shrine.borrow_mut().rites.complained.remove(slug);
    if let Some(n) = open {
        out.push(format!("  a run of it is still in the shrine: close {n}, or what gets typed there writes its notes back"));
    }
    Ok(out.join("\n"))
}
