//! Rituals in the daemon: the clock that fires them, every target, `overlap`, `catch_up`, and
//! the timetable's requests; headless runs are `headless.rs` and `keep` is `keep.rs`. The files and the rules about them
//! are `crate::ritual`; the schedule is `crate::cron`. This decides and acts.

use super::cards::deliver;
use super::log::log;
use super::registry;
use super::shrine::{Shared, Shrine, Start, recall, start, taken, valid_name};
use crate::cron::{self, Schedule, Why};
use crate::hooks;
use crate::proto::{Reply, RitualInfo, RitualVerb};
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
/// before the fire is given up.
const READY_WAIT: Duration = Duration::from_secs(60);

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
    /// `.claude.json`, as of its mtime: it is large, and the listing wants it every tick.
    trust: Option<(SystemTime, Rc<Trust>)>,
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
fn trust(shrine: &Shared) -> Rc<Trust> {
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
                "queue" => {
                    shrine.borrow_mut().rites.queued.insert(r.slug.clone(), minute);
                    d.note(
                        now,
                        "queued",
                        &format!("queued ({label}): the last run is still going"),
                    );
                    continue;
                }
                _ => {
                    d.note(
                        now,
                        "skipped",
                        &format!("skipped ({label}): the last run is still going"),
                    );
                    continue;
                }
            }
        }
        shrine.borrow_mut().rites.queued.remove(&r.slug);
        // A complaint stands until a fire gets through, so a failing start is said once too.
        match fire(shrine, r, &d, &label, alongside, now) {
            Ok(_) => _ = shrine.borrow_mut().rites.complained.remove(&r.slug),
            Err(e) => complain(shrine, r, &d, now, &e),
        }
    }
    super::keep::reap(shrine, now);
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
    sh.rites.headless.get(slug).is_some_and(|n| *n > 0)
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
) -> Result<String, String> {
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
        // Typed in later, by a task of its own: the journal says `sent` or `not sent` then.
        t => {
            let to = match &t {
                Target::Resident(who) => who.clone(),
                _ => "the session it keeps".into(),
            };
            let said = format!("{}'s prompt is on its way to {to}", r.slug);
            let (shrine, r, label) = (shrine.clone(), r.clone(), label.to_string());
            tokio::task::spawn_local(async move {
                match t {
                    Target::Resident(who) => send(&shrine, &r, &who, &label).await,
                    _ => persistent(&shrine, &r, &label).await,
                }
            });
            return Ok(said);
        }
    };
    let beside = if alongside { ", alongside the run that was still going" } else { "" };
    d.note(now, "ran", &format!("ran ({label}){beside}"));
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
    };
    start(shrine, s).map(|res| res.id)
}

/// The session a persistent ritual keeps: typed into when it is here, recalled when it has
/// left, started when there has never been one.
async fn persistent(shrine: &Shared, r: &Ritual, label: &str) {
    let d = Dir::of(&r.slug);
    let id = d.session().filter(|id| {
        let sh = shrine.borrow();
        sh.entries.iter().any(|e| &e.rec.id == id) || sh.store.load_departed_id(id).is_some()
    });
    let Some(id) = id else {
        match fresh(shrine, r, &d, None) {
            Ok(id) => {
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
    if !here && let Err(e) = recall(shrine, &id) {
        let why = format!("could not recall the session this ritual keeps ({id}): {e}");
        return undelivered(shrine, r, &why);
    }
    type_prompt(shrine, r, &id, label).await;
}

/// A resident the user manages, by name: the prompt alone. Typed in as a spell card is, so a
/// fire into a resident on screen can land after something the user had half typed there.
async fn send(shrine: &Shared, r: &Ritual, who: &str, label: &str) {
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
        Some((id, true)) => type_prompt(shrine, r, &id, label).await,
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
/// that is still starting (just recalled) is waited for; a dialog is reported instead.
async fn type_prompt(shrine: &Shared, r: &Ritual, id: &str, label: &str) {
    let t = Instant::now();
    // Fresh once, for the dialog guard; while it starts up the poll keeps it current.
    let claude = shrine.borrow().claude("");
    if let Some((claude, env)) = claude {
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
                return undelivered(shrine, r, "the daemon is stopping");
            }
            sh.entries.iter().find(|e| e.rec.id == id).map(|e| {
                (
                    e.rec.name.clone(),
                    e.handle.clone().filter(|h| h.exit().is_none()),
                    e.aware.blocked(),
                )
            })
        };
        match state {
            None | Some((_, None, _)) => return undelivered(shrine, r, "its resident has left"),
            Some((name, Some(h), None)) => break (name, h),
            Some((name, _, Some(why)))
                if why != "is still starting up" || t.elapsed() > READY_WAIT =>
            {
                return undelivered(shrine, r, &format!("{name} {why}"));
            }
            Some(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    };
    let text = ritual::prompt_text(r, &Dir::of(&r.slug).memory());
    match deliver(shrine, id, &handle, &text, "prompt").await {
        Ok(()) => {
            Dir::of(&r.slug).note(now(), "sent", &format!("sent to {name} ({label})"));
            shrine.borrow_mut().rites.undelivered.remove(&r.slug);
        }
        Err(why) => undelivered(shrine, r, &format!("{name} {why}")),
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

/// Fired now by hand, whatever the schedule says: which is how its prompts get approved once.
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
    let said = fire(shrine, r, &Dir::of(slug), &Why::ByHand.to_string(), false, now)?;
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
