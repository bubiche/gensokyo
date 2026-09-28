//! Spell cards: prompts typed into residents as if the user had typed them. A card is a markdown
//! file whose body is the prompt, with a little frontmatter and placeholders filled in per
//! resident. They ship in `share/spellcards/`; the user's own in the config dir's `spellcards/`
//! shadow those of the same file name. Writing a card is writing a file: nothing registers it.

use super::notify::after;
use super::registry;
use super::resident::Handle;
use super::server::log;
use super::shrine::{Shared, Shrine, find};
use crate::hooks;
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

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub slug: String,
    pub title: String,
    pub summary: String,
    /// `peer: required`.
    pub pair: bool,
    pub body: String,
}

impl Card {
    pub fn listed(&self) -> proto::Card {
        let c = self.clone();
        proto::Card { slug: c.slug, title: c.title, summary: c.summary, pair: c.pair }
    }
}

/// A card's file name, without `.md`, is how it is named from the CLI.
fn slug_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// Every card in `dirs`, the first dir's shadowing the later ones' by file name, sorted by title;
/// and the `.md` files left out because their names are not usable.
pub fn load(dirs: &[PathBuf]) -> (Vec<Card>, Vec<PathBuf>) {
    let (mut cards, mut unusable) = (Vec::<Card>::new(), Vec::new());
    for d in dirs {
        let mut files: Vec<PathBuf> =
            std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).collect();
        files.sort();
        for f in files {
            let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let Some(slug) = name.strip_suffix(".md").filter(|_| !name.starts_with('.')) else {
                continue;
            };
            if !slug_ok(slug) {
                unusable.push(f);
                continue;
            }
            if cards.iter().any(|c| c.slug == slug) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&f) {
                cards.push(parse(slug, &text));
            }
        }
    }
    cards.sort_by(|a, b| a.title.cmp(&b.title));
    (cards, unusable)
}

/// The frontmatter is the `---` fenced block at the top, one `key: value` a line, read by hand:
/// nothing in a card can run. A file with no fence is all body.
pub fn parse(slug: &str, text: &str) -> Card {
    let mut c = Card {
        slug: slug.into(),
        title: String::new(),
        summary: String::new(),
        pair: false,
        body: String::new(),
    };
    let mut body = text;
    if let Some(rest) = text.strip_prefix("---\n") {
        let (front, after) = match rest.find("\n---\n") {
            Some(i) => (&rest[..i], &rest[i + 5..]),
            None => match rest.strip_suffix("\n---") {
                Some(f) => (f, ""),
                None => (rest, ""),
            },
        };
        for (k, v) in front.lines().filter_map(|l| l.split_once(':')) {
            let v = v.trim().to_string();
            match k.trim() {
                "title" => c.title = v,
                "summary" => c.summary = v,
                "peer" => c.pair = v == "required",
                _ => {}
            }
        }
        body = after;
    }
    // Blank lines after the fence belong to it, not to the prompt.
    let body = body.trim_start_matches(['\n', '\r']);
    c.body = body.trim_end().to_string();
    if c.title.is_empty() {
        c.title = c.slug.clone();
    }
    c
}

/// By slug or title in any case; else by part of either, when that picks out one card.
pub fn find_card<'a>(cards: &'a [Card], want: &str) -> Result<&'a Card, String> {
    let w = want.to_lowercase();
    if let Some(c) =
        cards.iter().find(|c| c.slug.to_lowercase() == w || c.title.to_lowercase() == w)
    {
        return Ok(c);
    }
    let part: Vec<&Card> = cards
        .iter()
        .filter(|c| format!("{} {}", c.slug, c.title).to_lowercase().contains(&w))
        .collect();
    match part[..] {
        [c] => Ok(c),
        [] => Err(format!("no spell card '{want}' (gensokyo broadcast lists them)")),
        _ => Err(format!(
            "'{want}' could be {} (gensokyo broadcast lists them)",
            part.iter().map(|c| c.slug.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// `{self}`, `{peer}`, `{cwd}` and `{residents}`, in one pass: a value that holds a placeholder
/// (a directory named `{peer}`) stays as it is.
pub fn fill(body: &str, vals: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let hit = vals.iter().find(|(k, _)| {
            rest.strip_prefix('{')
                .and_then(|r| r.strip_prefix(k))
                .is_some_and(|r| r.starts_with('}'))
        });
        match hit {
            Some((k, v)) => {
                out.push_str(v);
                rest = &rest[k.len() + 2..];
            }
            None => {
                out.push('{');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// What shows the card reached the input line: the start of its last line, short enough to stay
/// on one row. The last and not the first: by the time a long card's end is on screen its
/// opening has scrolled off.
pub fn needle(text: &str) -> String {
    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    last.trim().chars().take(20).collect::<String>().trim_end().to_string()
}

/// The rows showing a card: Claude Code collapses a multi-line paste to "[Pasted text #1 +3
/// lines]", and anything else shows the text, or echoes it.
pub fn shown(rows: &[String], needle: &str) -> usize {
    rows.iter().filter(|r| r.contains("Pasted text") || r.contains(needle)).count()
}

/// No control characters but newlines and tabs: a card cannot end the paste it goes in.
pub fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect()
}

/// The card as typed: newlines as the Return a terminal pastes, in bracketed paste when the
/// child asked for it, so a card of many lines arrives as one block and submits only on the
/// Enter after it.
pub fn typed(text: &str, bracketed: bool) -> Vec<u8> {
    let clean = clean(text).replace('\n', "\r");
    match bracketed {
        true => format!("\x1b[200~{clean}\x1b[201~").into_bytes(),
        false => clean.into_bytes(),
    }
}

fn dirs(sh: &Shrine) -> Vec<PathBuf> {
    let mut d = vec![proto::config_dir().join("spellcards")];
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
/// why, in one line; an error when nobody did.
pub(super) async fn cast(shrine: &Shared, c: Cast) -> Result<String, String> {
    let (card, (claude, env)) = {
        let sh = shrine.borrow();
        if sh.quitting {
            return Err("the daemon is stopping".into());
        }
        let (cards, _) = load(&dirs(&sh));
        let card = find_card(&cards, &c.card)?.clone();
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
    let (targets, mut notes) = aim(&shrine.borrow(), &card, &c)?;
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
fn aim(sh: &Shrine, card: &Card, c: &Cast) -> Result<(Vec<Target>, Vec<String>), String> {
    let live = |i: usize| sh.entries[i].handle.as_ref().filter(|h| h.exit().is_none());
    let mut ids: Vec<usize> = Vec::new();
    let mut notes = Vec::new();
    let group = c.targets.iter().any(|t| ["all", "awaiting", "idle"].contains(&t.as_str()));
    for t in &c.targets {
        let hits: Vec<usize> = match t.as_str() {
            "all" | "awaiting" | "idle" => (0..sh.entries.len())
                .filter(|&i| live(i).is_some() && sh.entries[i].aware.blocked().is_none())
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
            if let (Some(_), Some(why)) = (live(i), e.aware.blocked()) {
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
        let Some(h) = live(i) else {
            notes.push(format!("{} has departed; not cast at", e.rec.name));
            continue;
        };
        // Named by hand, a resident comes straight past the groups' guard.
        if let Some(why) = e.aware.blocked() {
            notes.push(format!("{} {why}; not cast at, or the card would answer it", e.rec.name));
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
pub(super) async fn deliver(
    shrine: &Shared,
    id: &str,
    h: &Rc<Handle>,
    text: &str,
    what: &str,
) -> Result<(), String> {
    // Everyone is being asked to /exit: typing now would land between the Ctrl-C and it.
    if shrine.borrow().quitting {
        return Err("was not typed into: the daemon is stopping".into());
    }
    let needle = needle(&clean(text));
    let before = h.frame().text();
    // More rows than before: an echo of the last cast still on screen does not count.
    let was = shown(&before, &needle);
    let pasted = Instant::now();
    if !h.input(&typed(text, h.modes().paste)).await {
        return Err("is not reading its input".into());
    }
    loop {
        let now = h.frame().text();
        if now != before && shown(&now, &needle) > was {
            break;
        }
        if h.exit().is_some() {
            return Err(format!("left before the {what} showed"));
        }
        if pasted.elapsed() > SHOW_WAIT {
            return Err(format!(
                "never showed the {what} in its input line; nothing was submitted"
            ));
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    tokio::time::sleep(ENTER_GAP.saturating_sub(pasted.elapsed())).await;
    // A dialog that came up meanwhile would take the Enter.
    let why = {
        let sh = shrine.borrow();
        let e = sh.entries.iter().find(|e| e.rec.id == id);
        match e {
            _ if sh.quitting => Some("is being asked to leave"),
            None => Some("has left"),
            Some(e) => e.aware.blocked(),
        }
    };
    if let Some(why) = why {
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
