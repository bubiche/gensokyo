//! Casting spell cards: whom a card reaches, and typing it into each as if the user had. The
//! cards themselves are `card.rs`.
use super::log::log;
use super::notify::after;
use super::registry;
use super::resident::Handle;
use super::shrine::{Shared, Shrine, find};
use crate::card::{Card, clean, fill, find_card, load, needle, shown, typed};
use crate::hooks;
use crate::paths;
use crate::proto::{self, Cast, State};
use serde_json::json;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long a card may take to show in a resident's input line before it is called lost. One
/// that took the paste shows it within a tenth of a second, so only a lost card waits this long.
const SHOW_WAIT: Duration = Duration::from_secs(5);

/// From the paste to the Enter, at the least.
const ENTER_GAP: Duration = Duration::from_millis(300);

fn dirs(sh: &Shrine) -> Vec<PathBuf> {
    let mut d = vec![paths::config_dir().join("spellcards")];
    d.extend(sh.share.as_ref().map(|s| s.join("spellcards")));
    d
}

pub(super) fn cards(sh: &Shrine) -> (Vec<proto::Card>, Vec<String>) {
    let (cards, bad) = load(&dirs(sh));
    let bad = bad.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    (cards.iter().map(Card::listed).collect(), bad)
}

/// One resident a card is typed into: its id, name, screen and the text filled in for it.
struct Target {
    id: String,
    name: String,
    handle: Rc<Handle>,
    text: String,
}

/// "A", "A and B", "A, B and C".
fn names(v: &[String]) -> String {
    match v {
        [] => String::new(),
        [a] => a.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// A card typed into each target, the registry asked first: gensokyo never types into a
/// resident with a dialog open, where the Enter would answer it. Who got it, who was left out and
/// why, in one line; an error when nobody did. A resident casting (`caller`) is never typed
/// into itself: it is mid-turn, in the Bash call that cast.
pub(super) async fn cast(shrine: &Shared, c: Cast, caller: Option<&str>) -> Result<String, String> {
    let (card, (claude, env)) = {
        let sh = shrine.borrow();
        if sh.quitting {
            return Err("the daemon is stopping".into());
        }
        let (cards, _) = load(&dirs(&sh));
        let card = find_card(&cards, &c.card)?.clone();
        if let Some(p) = &card.problem {
            return Err(format!("{} is not cast: {p}", card.title));
        }
        if card.body.is_empty() {
            return Err(format!("{} has no prompt in it, only frontmatter", card.title));
        }
        if c.targets.is_empty() {
            return Err("name who gets it: all, awaiting, idle, or a resident".into());
        }
        (card, sh.claude("").ok_or("claude not found on PATH")?)
    };
    // Fresh, for the dialog guard: the last poll may be seconds old.
    let at = hooks::now_ms();
    let list = registry::fetch(&claude, &env)
        .await
        .ok_or("the registry could not be read, so a dialog cannot be ruled out")?;
    registry::seen(shrine, &list, at);
    let (targets, mut notes) = aim(&shrine.borrow(), &card, &c, caller)?;
    let runs: Vec<_> = targets
        .into_iter()
        .map(|t| {
            let shrine = shrine.clone();
            tokio::task::spawn_local(async move {
                let r = deliver(&shrine, &t.id, &t.handle, &t.text, "card").await;
                (t.name, r)
            })
        })
        .collect();
    let mut sent = Vec::new();
    for r in runs {
        match r.await {
            Ok((name, Ok(()))) => sent.push(name),
            Ok((name, Err(why))) => notes.push(format!("{name} {why}")),
            Err(_) => {}
        }
    }
    log(json!({"ev": "cast", "card": card.slug, "sent": sent, "notes": notes}));
    let notes = notes.iter().map(|n| format!("; {n}")).collect::<String>();
    match sent.len() {
        0 => Err(format!("{} reached nobody{notes}", card.title)),
        1 | 2 => Ok(format!("cast {} on {}{notes}", card.title, names(&sent))),
        n => Ok(format!("cast {} on {n} residents{notes}", card.title)),
    }
}

/// Who gets the card and what it says to each, and a note on everyone left out, all before
/// anything is typed.
fn aim(
    sh: &Shrine,
    card: &Card,
    c: &Cast,
    caller: Option<&str>,
) -> Result<(Vec<Target>, Vec<String>), String> {
    let live = |i: usize| sh.entries[i].handle.as_ref().filter(|h| h.exit().is_none());
    let me = |i: usize| caller == Some(sh.entries[i].rec.id.as_str());
    // A resident's group cast leaves out itself and the helpers other leads have briefed.
    let other = |i: usize| {
        let owner = sh.entries[i].rec.owner.as_deref();
        me(i) || caller.is_some_and(|c| owner.is_some_and(|o| o != c))
    };
    let mut ids: Vec<usize> = Vec::new();
    let mut notes = Vec::new();
    let group = c.targets.iter().any(|t| ["all", "awaiting", "idle"].contains(&t.as_str()));
    for t in &c.targets {
        let hits: Vec<usize> = match t.as_str() {
            "all" | "awaiting" | "idle" => (0..sh.entries.len())
                .filter(|&i| {
                    !other(i) && live(i).is_some() && sh.entries[i].aware.blocked().is_none()
                })
                .filter(|&i| {
                    let st = sh.entries[i].aware.state();
                    match t.as_str() {
                        "awaiting" => st.needs_you(),
                        "idle" => st == State::Resting,
                        _ => true,
                    }
                })
                .collect(),
            who => vec![find(sh, who).ok_or_else(|| format!("no resident {who} (gensokyo list)"))?],
        };
        for i in hits {
            if !ids.contains(&i) {
                ids.push(i);
            }
        }
    }
    if group {
        for (i, e) in sh.entries.iter().enumerate() {
            if let (Some(_), Some(why), false) = (live(i), e.aware.blocked(), other(i)) {
                notes.push(format!("{} {why}; left out", e.rec.name));
            }
        }
    }
    if ids.is_empty() {
        return Err(format!("nobody to cast {} at{}", card.title, {
            notes.iter().map(|n| format!("; {n}")).collect::<String>()
        }));
    }
    if card.pair && (ids.len() != 1 || c.peer.is_none()) {
        return Err(match ids.len() {
            1 => format!("{} needs a peer: --with <name>", card.title),
            n => format!("{} is cast at one resident, with a peer; {n} were named", card.title),
        });
    }
    let mut peer = String::new();
    if let Some(p) = &c.peer {
        if !card.body.contains("{peer}") {
            return Err(format!("{} names no peer, so --with has nowhere to go", card.title));
        }
        let i = find(sh, p).ok_or_else(|| format!("no resident {p} to be the peer"))?;
        let e = &sh.entries[i];
        peer = e.rec.name.clone();
        if live(i).is_none() {
            return Err(format!("{peer} has departed and cannot be the peer"));
        }
        // SendMessage reaches only a session the registry lists.
        if !e.aware.listed() {
            return Err(format!("{peer} is still starting, so nothing can message it yet"));
        }
        if ids.contains(&i) {
            return Err(format!("{peer} cannot be its own peer"));
        }
        if let Some(why) = e.aware.blocked() {
            notes.push(format!("{peer} {why}, so it may not answer until you have seen to that"));
        }
    }
    let mut targets = Vec::new();
    for i in ids {
        let e = &sh.entries[i];
        if me(i) {
            notes.push(format!("{} is you; not cast at", e.rec.name));
            continue;
        }
        let Some(h) = live(i) else {
            notes.push(format!("{} has departed; not cast at", e.rec.name));
            continue;
        };
        // Named by hand, a resident comes straight past the groups' guard.
        if let Some(why) = e.aware.blocked() {
            let would = if e.aware.dialog().is_some() { "answer it" } else { "go in with it" };
            notes.push(format!("{} {why}; not cast at, or the card would {would}", e.rec.name));
            continue;
        }
        let others: Vec<&str> = (0..sh.entries.len())
            .filter(|&j| j != i && live(j).is_some() && sh.entries[j].aware.listed())
            .map(|j| sh.entries[j].rec.name.as_str())
            .collect();
        let others = if others.is_empty() { "nobody".to_string() } else { others.join(", ") };
        let vals = [
            ("self", e.rec.name.as_str()),
            ("peer", peer.as_str()),
            ("cwd", e.rec.cwd.as_str()),
            ("residents", others.as_str()),
        ];
        targets.push(Target {
            id: e.rec.id.clone(),
            name: e.rec.name.clone(),
            handle: h.clone(),
            text: fill(&card.body, &vals),
        });
    }
    Ok((targets, notes))
}

/// Text into the input line, then Enter once it shows there (`what` names it in the errors: a
/// card, a ritual's prompt). It must be the text that shows, not merely a screen that changed:
/// whatever swallows a paste redraws doing it, and an Enter into an unknown dialog answers it.
/// Text that never shows gets no Enter: left in an input line it is visible and recoverable.
/// One at a time: the resident is marked as typed into, from before the paste until a hook says
/// the Enter was read, and another card or prompt waits or is turned away meanwhile.
pub(super) async fn deliver(
    shrine: &Shared,
    id: &str,
    h: &Rc<Handle>,
    text: &str,
    what: &str,
) -> Result<(), String> {
    // Looked at and taken in one go: nothing else runs in between.
    let mark = {
        let mut sh = shrine.borrow_mut();
        // Everyone is being asked to /exit: typing now would land between the Ctrl-C and it.
        if sh.quitting {
            return Err("was not typed into: the daemon is stopping".into());
        }
        let e = sh.entries.iter_mut().find(|e| e.rec.id == id).ok_or("has left")?;
        if let Some(why) = e.aware.blocked() {
            return Err(why.into());
        }
        e.aware.mark()
    };
    let sent = type_in(shrine, id, h, text, what, mark).await;
    if sent.is_err() {
        let mut sh = shrine.borrow_mut();
        sh.entries.iter_mut().filter(|e| e.rec.id == id).for_each(|e| e.aware.unmark(mark));
    }
    sent
}

async fn type_in(
    shrine: &Shared,
    id: &str,
    h: &Rc<Handle>,
    text: &str,
    what: &str,
    mark: u64,
) -> Result<(), String> {
    let needle = needle(&clean(text));
    // The paste is looked for on the live screen, where it lands, even if a client scrolls
    // back while it is on its way.
    h.to_live();
    let before = h.live_text();
    // More rows than before: an echo of the last cast still on screen does not count.
    let was = shown(&before, &needle);
    let mut changes = h.changes();
    changes.borrow_and_update();
    let pasted = Instant::now();
    if !h.input(&typed(text, h.modes().paste)).await {
        return Err("is not reading its input".into());
    }
    // Pasted and not sent: the next card or ritual prompt would go in with it.
    let unsent = || {
        let mut sh = shrine.borrow_mut();
        sh.entries.iter_mut().filter(|e| e.rec.id == id).for_each(|e| e.aware.unsent());
    };
    let until = tokio::time::Instant::from_std(pasted + SHOW_WAIT);
    loop {
        let now = h.live_text();
        if now != before && shown(&now, &needle) > was {
            break;
        }
        // The screen is looked at again only when it has changed; the changes end with the child.
        tokio::select! {
            c = changes.changed() => if c.is_err() {
                return Err(format!("left before the {what} showed"));
            },
            () = tokio::time::sleep_until(until) => {
                unsent();
                return Err(format!(
                    "never showed the {what} in its input line; nothing was submitted"
                ));
            }
        }
    }
    tokio::time::sleep(ENTER_GAP.saturating_sub(pasted.elapsed())).await;
    // A dialog that came up meanwhile would take the Enter.
    let why = {
        let mut sh = shrine.borrow_mut();
        let quitting = sh.quitting;
        let e = sh.entries.iter_mut().find(|e| e.rec.id == id);
        match e {
            _ if quitting => Some("is being asked to leave"),
            None => Some("has left"),
            Some(e) => e.aware.in_way().or_else(|| {
                e.aware.entered(mark, hooks::now_ms());
                None
            }),
        }
    };
    if let Some(why) = why {
        unsent();
        return Err(format!("{why}; the {what} waits in its input line, not submitted"));
    }
    if !h.input(b"\r").await {
        return Err("stopped reading before the Enter".into());
    }
    // Typed to for the user: whatever it waited to be told, it has been.
    let mut sh = shrine.borrow_mut();
    if let Some(i) = sh.entries.iter().position(|e| e.rec.id == id) {
        let before = sh.entries[i].aware.state();
        sh.entries[i].aware.clear();
        after(&mut sh, i, before);
    }
    Ok(())
}
