//! The modals over the grid: each one's state, what it draws, and what it does with a key, a
//! click on one of its rows, and Enter. One file each; the yes/no ones and help are here.

mod cast;
mod history;
mod recall;
mod summon;
mod timetable;

pub use cast::{Cast, cast_choices};
pub use history::History;
pub use recall::Recall;
pub use summon::{Stage, Summon};
pub(super) use timetable::Act;
pub use timetable::{Timetable, opened, timetable_order, when_short};

use super::app::App;
use super::framer::Chunk;
use super::keys::{self, Scrollback};
use super::render::{CHORDS, DIM, Model, Row, width, wrap};
use crate::proto::{Request, Resident, State};
use ratatui::style::Style;

#[derive(Clone, Debug)]
pub enum Modal {
    Summon(Summon),
    Banish {
        id: String,
        name: String,
    },
    /// A live resident asked to `/exit`, from the close chord.
    Close {
        id: String,
        name: String,
    },
    /// The departed, newest first.
    Recall(Recall),
    /// A spell card, then who gets it, then whom a pair card's target talks to.
    Cast(Cast),
    Timetable(Timetable),
    Quit,
    /// Everyone behind the installed claude, by name, to be renewed.
    Renew(Vec<String>),
    /// The keys and glyphs, from its row `top` while it is taller than the screen.
    Help {
        top: usize,
    },
    /// What was said, newest first.
    History(History),
    /// Words for a cloud session, its next turn.
    Say {
        id: String,
        name: String,
        text: String,
    },
}

impl Modal {
    /// Its title and its rows, buttons last. `w` is the widest a line can be; long ones are
    /// wrapped to it. `h` is the most rows there is room for, the box's included.
    pub(super) fn view(&self, m: &Model, w: usize, h: usize) -> (String, Vec<Row>) {
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
            Modal::Close { name, .. } => (
                " close ".into(),
                vec![
                    Row::text(&format!("Close {name}?")),
                    Row::dim("It gets Esc, Ctrl-C, then /exit: a turn under"),
                    Row::dim("way stops and what you typed is cleared."),
                    Row::dim("Recall brings the conversation back."),
                    Row::yes_no("[close y]", "[cancel n]"),
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
            Modal::Renew(names) => (
                " renew ".into(),
                vec![
                    Row::text(&format!("Renew {}?", names.join(", "))),
                    Row::dim("Each starts again on the claude installed now,"),
                    Row::dim("into its conversation, once it rests. Its"),
                    Row::dim("background tasks, Monitors and /loops end."),
                    Row::yes_no("[renew y]", "[cancel n]"),
                ],
            ),
            Modal::Help { top } => help_page(*top, w, h),
            Modal::History(hs) => hs.view(m, w, h),
            Modal::Say { name, text, .. } => (
                " say ".into(),
                vec![
                    Row::text(&format!("To {name}, on claude.ai:")),
                    Row::Field(text.clone(), Style::new()),
                    Row::dim("Its next turn; gensokyo tells when it answers."),
                    Row::yes_no("[send ⏎]", "[cancel esc]"),
                ],
            ),
        }
    }
}

impl Modal {
    /// Whether there is more of it above and below what fits in `h` rows, the box's included.
    pub(super) fn more(&self, w: usize, h: usize) -> (bool, bool) {
        match self {
            Modal::Help { top } => {
                let (n, page) = (help(w).1.len(), help_rows(h));
                let top = (*top).min(n.saturating_sub(page));
                (top > 0, top + page < n)
            }
            _ => (false, false),
        }
    }
}

/// Rows of the help that fit in `h`, under the box's edges.
fn help_rows(h: usize) -> usize {
    h.saturating_sub(2).max(1)
}

/// The help from its row `top`, as much as fits in `h`. Taller than that, each row is as wide
/// as its widest, so the box keeps its width as it scrolls.
fn help_page(top: usize, w: usize, h: usize) -> (String, Vec<Row>) {
    let (title, all) = help(w);
    let (n, page) = (all.len(), help_rows(h));
    if n <= page {
        return (title, all);
    }
    let widest = all.iter().filter_map(|r| match r {
        Row::Text(s, _) => Some(width(s) as usize),
        _ => None,
    });
    let widest = widest.max().unwrap_or(0);
    let top = top.min(n - page);
    let rows = all.into_iter().skip(top).take(page).map(|r| match r {
        Row::Text(s, st) => {
            let pad = " ".repeat(widest.saturating_sub(width(&s) as usize));
            Row::Text(format!("{s}{pad}"), st)
        }
        r => r,
    });
    (title, rows.collect())
}

impl App {
    /// Rows and pages as the scrollback takes them, while it is taller than the screen; `q`
    /// and any other key close it, as before.
    fn help_key(&mut self, c: &Chunk) {
        // As `render` lays it out for the grid last drawn.
        let w = self.size.0.min(72).saturating_sub(4) as usize;
        let (n, page) = (help(w).1.len(), help_rows(self.size.1 as usize));
        let last = n.saturating_sub(page);
        let Some(Modal::Help { top }) = &mut self.m.modal else { return };
        let by = |rows: i32| (*top).min(last).saturating_add_signed(rows as isize);
        *top = match keys::scrollback(c) {
            Some(Scrollback::By(r)) => by(r),
            Some(Scrollback::Pages(p)) => by(p * page as i32),
            Some(Scrollback::Top) => 0,
            Some(Scrollback::Live) if !matches!(Key::of(c), Key::Text('q')) => last,
            Some(Scrollback::Stay) => return,
            _ => {
                self.m.modal = None;
                return;
            }
        }
        .min(last);
    }
}

/// The chords, what a scrolled-back resident takes, and what the sidebar's glyphs mean, in
/// lines of at most `w`: a row too wide is wrapped, never cut.
fn help(w: usize) -> (String, Vec<Row>) {
    let w = w.max(1);
    let mut rows: Vec<Row> =
        wrap("Ctrl-] then a key:", w, usize::MAX).iter().map(|l| Row::text(l)).collect();
    let pairs: Vec<String> = CHORDS
        .chunks(2)
        .map(|p| {
            let right = p.get(1).map_or(String::new(), |(k, v)| format!("{k:>3}  {v}"));
            format!("{:>5}  {:<14}{right}", p[0].0, p[0].1)
        })
        .collect();
    // Two a row while every pair fits, so they stay in columns; else one each.
    if pairs.iter().all(|r| width(r) as usize <= w) {
        rows.extend(pairs.iter().map(|r| Row::text(r)));
    } else {
        for (k, v) in CHORDS {
            rows.extend(keyed(&format!("{k:>5}  "), v, w, Style::new()));
        }
    }
    for s in [
        "Nobody on screen, or a departed one: the keys alone.",
        "Scrolled back: j k a row, b f a screen, g the top, q home.",
        "Search: / back, ? on, then n more the same way, N the other.",
        "Drag to copy, past an edge to scroll; double-click a word.",
        "y copies the last answer of the resident on screen.",
        "A cloud session: ⏎ opens it, s says something to it.",
    ] {
        rows.extend(wrap(s, w, usize::MAX).iter().map(|l| Row::text(l)));
    }
    let legend = [
        (State::Busy, "busy"),
        (State::Awaits, "needs you"),
        (State::Asked, "asked you"),
        (State::Resting, "resting"),
        (State::Departed, "departed"),
    ]
    .map(|(s, what)| (s.glyph(), what));
    rows.extend(packed(&legend, w));
    rows.extend(packed(&[("⇡", "behind the installed claude (u renews)"), ("↻", "renewing")], w));
    rows.push(Row::close());
    (" help ".into(), rows)
}

/// `key` and then `what`, in dim rows of at most `w`, as many to a row as fit, two spaces apart.
fn packed(items: &[(&str, &str)], w: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut line = String::new();
    for (key, what) in items {
        let item = format!("{key} {what}");
        let sep = if line.is_empty() { 0 } else { 2 };
        if width(&line) as usize + sep + width(&item) as usize <= w {
            line = if line.is_empty() { item } else { format!("{line}  {item}") };
            continue;
        }
        if !line.is_empty() {
            rows.push(Row::dim(&std::mem::take(&mut line)));
        }
        match width(&item) as usize <= w {
            true => line = item,
            false => rows.extend(keyed(&format!("{key} "), what, w, DIM)),
        }
    }
    if !line.is_empty() {
        rows.push(Row::dim(&line));
    }
    rows
}

/// `key`, then `what` wrapped to the rest of `w`, each row it wraps onto starting under it.
/// With no room beside the key for `what`'s widest word, `what` goes under the key instead,
/// as wide as `w`.
fn keyed(key: &str, what: &str, w: usize, style: Style) -> Vec<Row> {
    let indent = width(key) as usize;
    let widest = what.split(' ').map(|word| width(word) as usize).max().unwrap_or(0);
    if indent + widest > w {
        let key = key.trim_end();
        let key = if width(key) as usize > w { key.trim_start() } else { key };
        let under = wrap(what, w, usize::MAX).into_iter().map(|l| Row::Text(l, style));
        return std::iter::once(Row::Text(key.into(), style)).chain(under).collect();
    }
    let lines = wrap(what, w.saturating_sub(indent).max(1), usize::MAX);
    let pad = " ".repeat(indent);
    let row = |(i, l): (usize, &String)| {
        Row::Text(format!("{}{l}", if i == 0 { key } else { &pad }), style)
    };
    lines.iter().enumerate().map(row).collect()
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
    pub(super) fn of(c: &Chunk) -> Key {
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
            Some(Modal::Help { .. }) => self.help_key(c),
            Some(Modal::Quit | Modal::Banish { .. } | Modal::Close { .. } | Modal::Renew(_)) => {
                match k {
                    Key::Enter | Key::Text('y') if self.answer() => self.confirm(),
                    Key::Text('n') => self.m.modal = None,
                    _ => {}
                }
            }
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
            Some(Modal::History(_)) => self.history_key(c),
            Some(Modal::Say { text, .. }) => match k {
                Key::Text(ch) => text.push(ch),
                Key::Paste(t) => text.push_str(&t.replace(['\r', '\n'], " ")),
                Key::Back => _ = text.pop(),
                Key::Enter => self.confirm(),
                _ => {}
            },
            None => {}
        }
    }

    /// Esc or a modal's cancel: back one stage, and from the first, closed. A summon already
    /// sent just closes; it still comes.
    pub(super) fn go_back(&mut self) {
        match &mut self.m.modal {
            Some(Modal::Timetable(tt)) if tt.confirm => tt.confirm = false,
            Some(Modal::Timetable(tt)) if tt.open.is_some() => tt.open = None,
            Some(Modal::Summon(s)) if s.stage == Stage::Worktree && !s.waiting => {
                (s.stage, s.error) = (Stage::Role, None);
            }
            Some(Modal::Summon(s)) if s.stage == Stage::Role && !s.waiting => {
                (s.stage, s.error) = (Stage::Name, None);
            }
            Some(Modal::Summon(s)) if s.stage == Stage::Name && !s.waiting => {
                (s.stage, s.error) = (Stage::Dir, None);
                self.refresh_modal();
            }
            Some(Modal::Cast(c)) if c.target.is_some() => (c.target, c.selected) = (None, 0),
            Some(Modal::Cast(c)) if c.card.is_some() => (c.card, c.selected) = (None, 0),
            Some(Modal::Summon(s)) if s.waiting => {
                (self.m.modal, self.summoning) = (None, None);
            }
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
            Some(Modal::Summon(s)) if s.stage == Stage::Role && !s.waiting => {
                if s.role == i {
                    self.confirm();
                } else {
                    s.role = i;
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
        // Nothing to say yet: it keeps asking, whether by Enter or its button.
        if matches!(&self.m.modal, Some(Modal::Say { text, .. }) if text.trim().is_empty()) {
            return;
        }
        let Some(modal) = self.m.modal.take() else { return };
        match modal {
            Modal::Summon(s) => self.summon_confirm(s),
            Modal::Banish { id, .. } => {
                self.send(Request::Banish { who: id });
            }
            Modal::Close { id, .. } => {
                self.send(Request::Close { who: id });
            }
            Modal::Renew(_) => {
                self.send(Request::Renew { who: Vec::new() });
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
            Modal::Say { id, text, .. } => {
                self.send(Request::CloudSay { who: id, text });
            }
            Modal::Help { .. } | Modal::History(_) => {}
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
