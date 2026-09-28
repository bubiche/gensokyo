//! Summon: a directory (typed with Tab completion, or one of the recent ones), then a name.

use super::Key;
use crate::client::app::App;
use crate::client::render::{ERROR, Model, PICK, Row, mark, window};
use crate::paths::{self, tilde};
use crate::proto::{self, Request, Resident};
use ratatui::style::Style;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summon {
    pub stage: Stage,
    /// Recent directories, newest first.
    pub recent: Vec<String>,
    /// The highlighted recent directory; None is the typed path.
    pub selected: Option<usize>,
    pub path: String,
    /// Tab completions of `path`. In the name stage `path` is the directory chosen.
    pub completions: Vec<String>,
    pub name: String,
    pub error: Option<String>,
    /// Sent, and waiting for the daemon to say it came or why not.
    pub waiting: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Dir,
    Name,
}

impl Summon {
    pub(super) fn view(&self, m: &Model) -> (String, Vec<Row>) {
        let mut rows = match self.stage {
            Stage::Dir => {
                let typed = if self.selected.is_none() { PICK } else { Style::new() };
                let mut rows = vec![
                    Row::text("Where?"),
                    Row::Text(format!("{}{}█", mark(self.selected.is_none()), self.path), typed),
                ];
                if !self.completions.is_empty() {
                    rows.push(Row::dim(&self.completions.join("  ")));
                }
                if !self.recent.is_empty() {
                    rows.push(Row::dim("recent"));
                    rows.extend(window(self.recent.len(), self.selected.unwrap_or(0), 8).map(
                        |i| {
                            let sel = self.selected == Some(i);
                            let line = format!("{}{}", mark(sel), tilde(&self.recent[i], &m.home));
                            Row::Item(i, line, sel)
                        },
                    ));
                }
                rows
            }
            Stage::Name => vec![
                Row::text(&format!("In {}", tilde(&self.path, &m.home))),
                Row::Text(format!("name: {}█", self.name), PICK),
                Row::dim(match self.waiting {
                    true => "summoning…",
                    false => "Enter picks a random name when this is empty",
                }),
            ],
        };
        if let Some(e) = &self.error {
            rows.push(Row::Text(e.clone(), ERROR));
        }
        rows.push(match self.waiting {
            true => Row::close(),
            false => Row::yes_no("[summon ⏎]", "[cancel esc]"),
        });
        (" summon ".into(), rows)
    }

    /// A key typed into it; true for Enter.
    pub(super) fn key(&mut self, k: Key, home: &str) -> bool {
        if self.waiting {
            return false;
        }
        match (self.stage, k) {
            (_, Key::Enter) => return true,
            (Stage::Dir, Key::Up) => self.selected = self.selected.and_then(|i| i.checked_sub(1)),
            (Stage::Dir, Key::Down) => {
                let n = self.recent.len();
                self.selected = match self.selected {
                    None if n > 0 => Some(0),
                    Some(i) if i + 1 < n => Some(i + 1),
                    other => other,
                };
            }
            (Stage::Dir, Key::Tab) => complete(self, home),
            (Stage::Dir, Key::Back) => {
                self.path.pop();
                (self.selected, self.error) = (None, None);
            }
            (Stage::Dir, Key::Text(ch)) => {
                self.path.push(ch);
                (self.selected, self.error) = (None, None);
            }
            (Stage::Dir, Key::Paste(t)) => {
                self.path.push_str(t.trim_end_matches(['\r', '\n']));
                self.selected = None;
            }
            (Stage::Name, Key::Back) => {
                self.name.pop();
            }
            (Stage::Name, Key::Text(ch)) => self.name.push(ch),
            (Stage::Name, Key::Paste(t)) => self.name.push_str(t.trim()),
            _ => {}
        }
        false
    }

    /// The recent directories from everyone the daemon knows: the client's own first, then the
    /// shrine's, then the departed's (newest first).
    pub(super) fn refresh(&mut self, all: &[Resident], departed: &[Resident], home: &str) {
        let here = std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned());
        let live = all.iter().rev().filter(|r| r.departed.is_none());
        let mut recent = Vec::new();
        for d in here.into_iter().chain(live.chain(departed).map(|r| r.cwd.clone())) {
            let d = tilde(&d, home);
            if !recent.contains(&d) {
                recent.push(d);
            }
        }
        self.recent = recent;
        self.selected = self.selected.filter(|&i| i < self.recent.len());
    }
}

impl App {
    /// The directory, once it is one, moves on to the name; the name summons, and the modal
    /// stays until the daemon says how that went.
    pub(super) fn summon_confirm(&mut self, mut s: Summon) {
        let home = self.m.home.clone();
        if s.waiting {
            self.m.modal = Some(super::Modal::Summon(s));
            return;
        }
        if s.stage == Stage::Name {
            let cwd = expand(&s.path, &home).to_string_lossy().into_owned();
            let name = Some(s.name.trim().to_string()).filter(|n| !n.is_empty());
            let id = self.send(Request::Summon(proto::Summon { cwd, name, ..Default::default() }));
            (self.summoning, s.waiting, s.error) = (Some(id), true, None);
            self.m.modal = Some(super::Modal::Summon(s));
            return;
        }
        let chosen = s.selected.and_then(|i| s.recent.get(i).cloned());
        let dir = expand(chosen.as_deref().unwrap_or(&s.path), &home);
        if dir.is_dir() {
            (s.stage, s.path, s.error) = (Stage::Name, tilde(&dir.to_string_lossy(), &home), None);
            s.completions.clear();
        } else {
            s.error = Some(format!("not a directory: {}", dir.display()));
        }
        self.m.modal = Some(super::Modal::Summon(s));
    }
}

/// A typed path made absolute: `~` is home, and a relative one is under the client's cwd.
fn expand(p: &str, home: &str) -> PathBuf {
    let p = paths::untilde(p.trim(), home);
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    if p.is_empty() { here } else { here.join(p) }
}

/// Tab: the directories under the typed path's parent that start with its last part. One match
/// is filled in; several fill in what they share and are listed.
fn complete(s: &mut Summon, home: &str) {
    let (dir, prefix) = match s.path.rfind('/') {
        Some(i) => (&s.path[..=i], &s.path[i + 1..]),
        None => ("", s.path.as_str()),
    };
    let base = expand(if dir.is_empty() { "." } else { dir }, home);
    let mut names: Vec<String> = std::fs::read_dir(&base)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix) && (prefix.starts_with('.') || !n.starts_with('.')))
        .collect();
    names.sort();
    let shared = match names.as_slice() {
        [] => {
            s.completions.clear();
            return;
        }
        [one] => format!("{one}/"),
        [first, rest @ ..] => {
            let mut n = rest.iter().fold(first.len(), |n, r| {
                first.bytes().zip(r.bytes()).take(n).take_while(|(a, b)| a == b).count()
            });
            // `café` and `cafè` share a byte of the é.
            while !first.is_char_boundary(n) {
                n -= 1;
            }
            first[..n].to_string()
        }
    };
    s.path = format!("{dir}{shared}");
    s.completions = if names.len() > 1 { names } else { Vec::new() };
    s.selected = None;
}
