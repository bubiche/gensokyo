//! Spell cards: prompts typed into residents as if the user had typed them. A card is a markdown
//! file whose body is the prompt, with a little frontmatter and placeholders filled in per
//! resident. They ship in `share/spellcards/`; the user's own in the config dir's `spellcards/`
//! shadow those of the same file name. Writing a card is writing a file: nothing registers it.

use crate::frontmatter::{self, name_ok};
use crate::proto;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub slug: String,
    pub title: String,
    pub summary: String,
    /// `peer: required`.
    pub pair: bool,
    pub body: String,
    /// Why it is not cast: a line in its frontmatter that means nothing, which is most likely a
    /// typo of one that would have (`pear: required` casting a pair card alone).
    pub problem: Option<String>,
}

impl Card {
    /// The problem first, where the picker and the listing show the summary.
    pub fn listed(&self) -> proto::Card {
        let c = self.clone();
        let summary = match c.problem {
            Some(p) => format!("not cast: {p}"),
            None => c.summary,
        };
        proto::Card { slug: c.slug, title: c.title, summary, pair: c.pair }
    }
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
            if !name_ok(slug) {
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

/// The frontmatter (`frontmatter::read`) and the prompt under it. A file with no fence is all
/// body, titled by its name.
pub fn parse(slug: &str, text: &str) -> Card {
    let front = frontmatter::read(text, &["title", "summary", "peer"]);
    let mut c = Card {
        slug: slug.into(),
        title: String::new(),
        summary: String::new(),
        pair: false,
        body: front.body,
        problem: None,
    };
    let mut problems = Vec::new();
    if !front.unknown.is_empty() {
        problems.push(format!("not a card setting: {}", front.unknown.join(", ")));
    }
    for f in &front.fields {
        let v = f.text();
        match f.key.as_str() {
            "title" => c.title = v,
            "summary" => c.summary = v,
            "peer" => {
                c.pair = v == "required";
                if !c.pair && !v.is_empty() {
                    problems.push(format!("peer: {v} (peer: required, or no peer line)"));
                }
            }
            _ => {}
        }
    }
    if c.title.is_empty() {
        c.title = c.slug.clone();
    }
    c.problem = (!problems.is_empty()).then(|| problems.join("; "));
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
