//! The modals over the grid: each one's state, what it draws, and what it does with a key, a
//! click on one of its rows, and Enter. One file each; the yes/no ones and help are here.

mod cast;
mod recall;
mod summon;
mod timetable;

pub use cast::{Cast, cast_choices};
pub use recall::Recall;
pub use summon::{Stage, Summon};
pub(super) use timetable::Act;
pub use timetable::{Timetable, opened, timetable_order, when_short};

use super::app::App;
use super::framer::Chunk;
use super::render::{CHORDS, Model, Row};
use crate::proto::{Request, Resident, State};

#[derive(Clone, Debug)]
pub enum Modal {
    Summon(Summon),
    Banish {
        id: String,
        name: String,
    },
    /// The departed, newest first.
    Recall(Recall),
    /// A spell card, then who gets it, then whom a pair card's target talks to.
    Cast(Cast),
    Timetable(Timetable),
    Quit,
    Help,
}

impl Modal {
    /// Its title and its rows, buttons last. `w` is the widest a line can be; long ones are
    /// wrapped to it.
    pub(super) fn view(&self, m: &Model, w: usize) -> (String, Vec<Row>) {
        match self {
            Modal::Summon(s) => s.view(m),
            Modal::Banish { name, .. } => (
                " banish ".into(),
                vec![
                    Row::text(&format!("Banish {name}?")),
                    Row::dim("It gets HUP, then TERM, then KILL."),
                    Row::yes_no("[banish y]", "[cancel n]"),
                ],
            ),
            Modal::Recall(r) => r.view(m),
            Modal::Cast(c) => c.view(m),
            Modal::Timetable(tt) => tt.view(m, w),
            Modal::Quit => (
                " quit ".into(),
                vec![
                    Row::text("Quit the shrine?"),
                    Row::dim("Everyone gets /exit, then the daemon stops."),
                    Row::yes_no("[quit y]", "[cancel n]"),
                ],
            ),
            Modal::Help => help(),
        }
    }
}

/// The chords, what a scrolled-back resident takes, and what the sidebar's glyphs mean.
fn help() -> (String, Vec<Row>) {
    let mut rows = vec![Row::text("Ctrl-] then a key:")];
    rows.extend(CHORDS.chunks(2).map(|p| {
        let right = p.get(1).map_or(String::new(), |(k, v)| format!("{k:>3}  {v}"));
        Row::text(&format!("{:>5}  {:<14}{right}", p[0].0, p[0].1))
    }));
    rows.push(Row::text("Nobody on screen, or a departed one: the keys alone."));
    rows.push(Row::text("Scrolled back: j k a row, b f a screen, g the top, q home."));
    let legend: Vec<String> = [
        (State::Busy, "busy"),
        (State::Awaits, "needs you"),
        (State::Asked, "asked you"),
        (State::Resting, "resting"),
        (State::Departed, "departed"),
    ]
    .iter()
    .map(|(s, what)| format!("{} {what}", s.glyph()))
    .collect();
    rows.push(Row::dim(&legend.join("  ")));
    rows.push(Row::close());
    (" help ".into(), rows)
}

/// A host chunk as a modal reads it.
pub(super) enum Key {
    Text(char),
    Paste(String),
    Enter,
    Esc,
    Tab,
    Back,
    Up,
    Down,
    Other,
}

impl Key {
    fn of(c: &Chunk) -> Key {
        match c {
            // One character per read is typing; more is a burst, taken as a paste.
            Chunk::Text(t) if t.chars().count() == 1 => Key::Text(t.chars().next().unwrap_or(' ')),
            Chunk::Text(t) => Key::Paste(t.clone()),
            Chunk::Paste(b) => Key::Paste(String::from_utf8_lossy(b).into_owned()),
            Chunk::Key { key: Some(k), .. } if k.event != 3 => match (k.code, k.mods & 0x0f) {
                (13, _) => Key::Enter,
                (27, _) => Key::Esc,
                (9, 0) => Key::Tab,
                (127 | 8, _) => Key::Back,
                (code, 0 | 1) => char::from_u32(code).map_or(Key::Other, Key::Text),
                _ => Key::Other,
            },
            Chunk::Key { raw, key: None } => match raw.as_slice() {
                b"\x1b[A" | b"\x1bOA" => Key::Up,
                b"\x1b[B" | b"\x1bOB" => Key::Down,
                _ => Key::Other,
            },
            _ => Key::Other,
        }
    }
}

impl App {
    pub(super) fn modal_key(&mut self, c: &Chunk) {
        let k = Key::of(c);
        if matches!(k, Key::Esc) {
            return self.go_back();
        }
        match &mut self.m.modal {
            Some(Modal::Help) => self.m.modal = None,
            Some(Modal::Quit | Modal::Banish { .. }) => match k {
                Key::Enter | Key::Text('y') if self.answer() => self.confirm(),
                Key::Text('n') => self.m.modal = None,
                _ => {}
            },
            Some(Modal::Summon(s)) => {
                let home = self.m.home.clone();
                if s.key(k, &home) {
                    self.confirm();
                }
            }
            Some(Modal::Recall(r)) => {
                if r.key(k) {
                    self.confirm();
                }
            }
            Some(Modal::Cast(_)) => self.cast_key(k),
            Some(Modal::Timetable(_)) => self.timetable_key(k),
            None => {}
        }
    }

    /// Esc or a modal's cancel: back one stage, and from the first, closed. A summon already
    /// sent just closes; it still comes.
    pub(super) fn go_back(&mut self) {
        match &mut self.m.modal {
            Some(Modal::Timetable(tt)) if tt.confirm => tt.confirm = false,
            Some(Modal::Timetable(tt)) if tt.open.is_some() => tt.open = None,
            Some(Modal::Summon(s)) if s.stage == Stage::Name && !s.waiting => {
                (s.stage, s.error) = (Stage::Dir, None);
                self.refresh_modal();
            }
            Some(Modal::Cast(c)) if c.target.is_some() => (c.target, c.selected) = (None, 0),
            Some(Modal::Cast(c)) if c.card.is_some() => (c.card, c.selected) = (None, 0),
            _ => self.m.modal = None,
        }
    }

    /// A click on list row `i`: the first picks it, a second on the same row confirms. A
    /// timetable row opens at once.
    pub(super) fn click_item(&mut self, i: usize) {
        match &mut self.m.modal {
            Some(Modal::Summon(s)) if s.stage == Stage::Dir => {
                if s.selected == Some(i) {
                    self.confirm();
                } else {
                    s.selected = Some(i);
                }
            }
            Some(Modal::Timetable(tt)) if tt.open.is_none() => {
                tt.selected = i;
                self.confirm();
            }
            Some(Modal::Recall(Recall { selected, .. }) | Modal::Cast(Cast { selected, .. })) => {
                if *selected == i {
                    self.confirm();
                } else {
                    *selected = i;
                }
            }
            _ => {}
        }
    }

    /// Enter, `y`, or the modal's confirm button.
    pub(super) fn confirm(&mut self) {
        let Some(modal) = self.m.modal.take() else { return };
        match modal {
            Modal::Summon(s) => self.summon_confirm(s),
            Modal::Banish { id, .. } => {
                self.send(Request::Banish { who: id });
            }
            Modal::Recall(r) => {
                if let Some(r) = r.list.get(r.selected) {
                    self.send(Request::Recall { who: r.id.clone() });
                }
            }
            Modal::Cast(c) => self.cast_confirm(c),
            Modal::Timetable(tt) => self.timetable_confirm(tt),
            Modal::Quit => {
                self.quit = Some(self.send(Request::Quit));
                self.gone = Some("the shrine is empty; the daemon stopped".into());
            }
            Modal::Help => {}
        }
    }

    /// The departed for recall, newest first, and the recent directories for summon: the
    /// client's own first, then the shrine's, then the departed's, newest first.
    pub(super) fn refresh_modal(&mut self) {
        let mut departed: Vec<Resident> =
            self.all.iter().filter(|r| r.departed.is_some()).cloned().collect();
        departed.sort_by_key(|r| std::cmp::Reverse(r.departed));
        match &mut self.m.modal {
            Some(Modal::Recall(r)) => r.refresh(departed),
            Some(Modal::Summon(s)) => s.refresh(&self.all, &departed, &self.m.home),
            _ => {}
        }
    }
}
