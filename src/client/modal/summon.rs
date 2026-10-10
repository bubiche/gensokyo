//! Summon: a directory (typed with Tab completion, or one of the recent ones), then a name, then
//! a role and words of the user's own for its system prompt, and in a git repository a worktree
//! to work in, which is none unless one is named.

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
    /// The directory chosen is in a git repository: the worktree stage follows the name.
    pub repo: bool,
    /// The roles there are, by name (`role.rs`).
    pub roles: Vec<String>,
    /// The one picked: 0 is none, else one past its index in `roles`.
    pub role: usize,
    /// Words of the user's own for its system prompt, with the role.
    pub words: String,
    /// A worktree to make, or find, by its name; empty works in the directory itself.
    pub worktree: String,
    pub error: Option<String>,
    /// Sent, and waiting for the daemon to say it came or why not.
    pub waiting: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Dir,
    Name,
    Role,
    Worktree,
}

impl Summon {
    pub(super) fn view(&self, m: &Model) -> (String, Vec<Row>) {
        let mut rows = match self.stage {
            Stage::Dir => {
                let typed = if self.selected.is_none() { PICK } else { Style::new() };
                let mut rows = vec![
                    Row::text("Where?"),
                    Row::Field(format!("{}{}", mark(self.selected.is_none()), self.path), typed),
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
                Row::Field(format!("name: {}", self.name), PICK),
                Row::dim(match self.waiting {
                    true => "summoning…",
                    false => "Enter picks a random name when this is empty",
                }),
            ],
            Stage::Role => {
                let names = std::iter::once("none").chain(self.roles.iter().map(String::as_str));
                let names: Vec<&str> = names.collect();
                let mut rows = vec![Row::text(&format!("In {}", tilde(&self.path, &m.home)))];
                rows.push(Row::dim("role"));
                rows.extend(window(names.len(), self.role, 8).map(|i| {
                    let sel = self.role == i;
                    Row::Item(i, format!("{}{}", mark(sel), names[i]), sel)
                }));
                rows.push(Row::Field(format!("your words: {}", self.words), PICK));
                rows.push(Row::dim(match self.waiting {
                    true => "summoning…",
                    false => "↑↓ a role, and what you type goes with it into its system prompt",
                }));
                rows
            }
            Stage::Worktree => vec![
                Row::text(&format!("In {}", tilde(&self.path, &m.home))),
                Row::Field(format!("worktree: {}", self.worktree), PICK),
                Row::dim(match (self.waiting, self.worktree.trim().is_empty()) {
                    (true, true) => "summoning…",
                    (true, false) => "making the worktree…",
                    (false, _) => "Enter works right here when this is empty",
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
                self.error = None;
            }
            (Stage::Name, Key::Text(ch)) => {
                self.name.push(ch);
                self.error = None;
            }
            (Stage::Name, Key::Paste(t)) => {
                self.name.push_str(t.trim());
                self.error = None;
            }
            (Stage::Role, Key::Up) => self.role = self.role.saturating_sub(1),
            (Stage::Role, Key::Down) => self.role = (self.role + 1).min(self.roles.len()),
            (Stage::Role, Key::Back) => {
                self.words.pop();
            }
            (Stage::Role, Key::Text(ch)) => self.words.push(ch),
            // One line: it is shown, and sent, as typed.
            (Stage::Role, Key::Paste(t)) => {
                self.words.push_str(&t.trim_end_matches(['\r', '\n']).replace(['\r', '\n'], " "))
            }
            (Stage::Worktree, Key::Back) => {
                self.worktree.pop();
            }
            (Stage::Worktree, Key::Text(ch)) => self.worktree.push(ch),
            (Stage::Worktree, Key::Paste(t)) => self.worktree.push_str(t.trim()),
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
    /// The directory, once it is one, moves on to the name, the name to the role; the role
    /// summons, or in a repository the worktree does, and the modal stays until the daemon says
    /// how that went.
    pub(super) fn summon_confirm(&mut self, mut s: Summon) {
        let home = self.m.home.clone();
        if s.waiting {
            self.m.modal = Some(super::Modal::Summon(s));
            return;
        }
        if s.stage == Stage::Name {
            let name = s.name.trim();
            if !name.is_empty() && !proto::valid_name(name) {
                s.error = Some(proto::NAME_RULE.into());
                self.m.modal = Some(super::Modal::Summon(s));
                return;
            }
            let share = std::env::current_exe().ok().and_then(|e| paths::share_dir(&e));
            s.roles = crate::role::names(share.as_deref());
            s.role = s.role.min(s.roles.len());
            (s.stage, s.error) = (Stage::Role, None);
            self.m.modal = Some(super::Modal::Summon(s));
            return;
        }
        if s.stage == Stage::Role && s.repo {
            (s.stage, s.error) = (Stage::Worktree, None);
            self.m.modal = Some(super::Modal::Summon(s));
            return;
        }
        if s.stage != Stage::Dir {
            let cwd = expand(&s.path, &home).to_string_lossy().into_owned();
            let name = Some(s.name.trim().to_string()).filter(|n| !n.is_empty());
            let role = s.role.checked_sub(1).and_then(|i| s.roles.get(i)).cloned();
            let system_prompt = Some(s.words.trim().to_string()).filter(|w| !w.is_empty());
            let slug = s.worktree.trim().to_string();
            let worktree = (s.stage == Stage::Worktree && !slug.is_empty())
                .then(|| proto::WorktreeAsk { slug, ..Default::default() });
            let ask =
                proto::Summon { cwd, name, role, system_prompt, worktree, ..Default::default() };
            let id = self.send(Request::Summon(ask));
            (self.summoning, s.waiting, s.error) = (Some(id), true, None);
            self.m.modal = Some(super::Modal::Summon(s));
            return;
        }
        let chosen = s.selected.and_then(|i| s.recent.get(i).cloned());
        let dir = expand(chosen.as_deref().unwrap_or(&s.path), &home);
        if dir.is_dir() {
            (s.stage, s.path, s.error) = (Stage::Name, tilde(&dir.to_string_lossy(), &home), None);
            s.repo = dir.ancestors().any(|d| d.join(".git").exists());
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
