//! The screen, drawn as a pure function of the model: the shrine sidebar, the focused
//! resident's grid in a box, and a modal over it. `render` also returns what every cell does
//! when clicked, so clicks are resolved against exactly what was drawn.

use super::modal::{Modal, when_short};
use crate::paths::tilde;
use crate::proto::{Resident, RitualInfo, State};
use crate::tele;
use crate::vt::{self, Frame, Modes};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Widget};

/// The sidebar's width, its box included.
pub const SIDEBAR_W: u16 = 25;

/// What each key after the leader does, in the order the sidebar and help list them.
pub(super) const CHORDS: [(&str, &str); 16] = [
    ("n", "summon"),
    ("c", "cast"),
    ("b", "banish"),
    ("r", "recall"),
    ("t", "timetable"),
    ("x", "close"),
    ("j", "next"),
    ("k", "previous"),
    ("a", "needs you"),
    ("[", "scroll"),
    ("1-9", "slot"),
    ("m", "mouse"),
    ("d", "detach"),
    ("q", "quit"),
    ("?", "help"),
    ("^]", "send ^]"),
];

#[derive(Clone, Debug, Default)]
pub struct Model {
    /// In shrine order.
    pub residents: Vec<Resident>,
    /// The id of the resident on screen.
    pub focused: Option<String>,
    /// Its screen, once the daemon has sent one.
    pub screen: Option<Frame>,
    pub modes: Modes,
    pub modal: Option<Modal>,
    /// Mouse capture: chrome is clickable. Off leaves selection to the host terminal.
    pub capture: bool,
    /// Our own selection in the grid: where the drag began and where it is, (column, row) in
    /// grid cells, both included.
    pub selection: Option<((u16, u16), (u16, u16))>,
    /// The leader was pressed and the next key is ours.
    pub leader: bool,
    /// A line for the user, above the sidebar's buttons, until it expires.
    pub message: Option<Message>,
    /// Drawn when the shrine is empty.
    pub banner: Vec<String>,
    /// Shown as `~` in paths.
    pub home: String,
    /// Epoch seconds, for "5m ago".
    pub now: i64,
    /// This machine's date, `YYYY-MM-DD`: a fire today shows its time alone.
    pub today: String,
    /// The timetable, once the daemon has sent it.
    pub rituals: Option<Vec<RitualInfo>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub kind: Say,
}

/// What a message is: its colour, and how long it stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Say {
    /// What was just done: `copied 12 characters`, a cast's reply.
    Info,
    /// A resident or a ritual has news, in gold.
    Notice,
    /// What could not be done.
    Error,
}

impl Message {
    pub fn new(kind: Say, text: impl Into<String>) -> Message {
        Message { text: text.into(), kind }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// A sidebar line: index into `Model::residents`.
    Resident(usize),
    Button(Button),
    /// A list row in the open modal: index into its recent dirs or its recall list.
    Item(usize),
    /// Elsewhere inside a modal: nothing, and nothing beneath it.
    Modal,
    /// The focused resident's grid.
    Grid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Summon,
    Banish,
    Recall,
    Cast,
    Timetable,
    Quit,
    Help,
    Capture,
    /// The open ritual's run now, pause or resume, and remove.
    RunRitual,
    ToggleRitual,
    RemoveRitual,
    /// The departed screen's recall and close, for the resident on it.
    RecallFocused,
    CloseFocused,
    /// A modal's confirm and cancel.
    Yes,
    No,
}

#[derive(Debug, Default, PartialEq)]
pub struct HitMap(pub Vec<(Rect, Hit)>);

impl HitMap {
    /// What was drawn on top at this cell (0-based), and its rect.
    pub fn at(&self, col: u16, row: u16) -> Option<(Rect, Hit)> {
        self.0.iter().rev().find(|(r, _)| r.contains(Position::new(col, row))).copied()
    }

    fn push(&mut self, r: Rect, h: Hit) {
        if !r.is_empty() {
            self.0.push((r, h));
        }
    }
}

pub(super) const DIM: Style = Style::new().fg(Color::DarkGray);
pub(super) const BUTTON: Style = Style::new().fg(Color::Cyan);
pub(super) const PICK: Style = Style::new().fg(Color::Black).bg(Color::Yellow);
pub(super) const ERROR: Style = Style::new().fg(Color::LightRed);
/// A resident that needs the user.
const GOLD: Style = Style::new().fg(Color::Black).bg(Color::Yellow);

fn sidebar_rect(area: Rect) -> Rect {
    Rect { width: SIDEBAR_W.min(area.width), ..area }
}

fn main_rect(area: Rect) -> Rect {
    let w = SIDEBAR_W.min(area.width);
    Rect { x: area.x + w, width: area.width - w, ..area }
}

/// Inside the main box: the size every resident is given.
pub fn grid_rect(area: Rect) -> Rect {
    Block::bordered().inner(main_rect(area))
}

fn focused(m: &Model) -> Option<&Resident> {
    m.residents.iter().find(|r| Some(&r.id) == m.focused.as_ref())
}

/// Where the host cursor goes: at the end of a modal's text field, or the focused resident's,
/// when it shows one on its live screen and no modal is open.
pub fn cursor(m: &Model, area: Rect) -> Option<(u16, u16)> {
    let g = grid_rect(area);
    if let Some(md) = &m.modal {
        let (_, inner, _, rows) = modal_layout(m, md, g);
        let i = rows.iter().position(|r| matches!(r, Row::Field(..)))?;
        let Row::Field(s, _) = &rows[i] else { return None };
        let (x, y) = (inner.x + width(s), inner.y + i as u16);
        return (x < inner.right() && y < inner.bottom()).then_some((x, y));
    }
    let fr = m.screen.as_ref().filter(|fr| fr.back == 0)?;
    let (x, y) = fr.cursor?;
    let live = focused(m)?.departed.is_none();
    (live && m.modal.is_none() && x < g.width && y < g.height).then(|| (g.x + x, g.y + y))
}

pub fn render(m: &Model, area: Rect, buf: &mut Buffer) -> HitMap {
    let mut hits = HitMap::default();
    sidebar(m, sidebar_rect(area), buf, &mut hits);
    let main = main_rect(area);
    let f = focused(m);
    let title = match f {
        Some(r) => {
            let slot = r.slot.map_or(String::new(), |s| format!("{s} "));
            let branch = r.branch.as_ref().map_or(String::new(), |b| format!(" ⎇ {b}"));
            let f = tele::fields(r.telemetry.as_ref(), r.mode.as_deref(), None, false);
            let f = match f.is_empty() || r.departed.is_some() {
                true => String::new(),
                false => format!(" · {f}"),
            };
            format!(" {slot}{} · {}{branch}{f} ", r.name, tilde(&r.cwd, &m.home))
        }
        None => " the shrine is empty ".into(),
    };
    let mut block = Block::bordered().title(title).border_style(DIM);
    // Scrolled back, the way home is on the box, in the colour of a resident that needs you.
    if let Some(fr) = m.screen.as_ref().filter(|fr| fr.back > 0 && f.is_some()) {
        let s = format!(" ↑ {} of {} · esc returns ", fr.back, fr.history);
        block = block.title_top(Line::styled(s, GOLD).right_aligned());
    }
    let g = block.inner(main);
    block.render(main, buf);
    match f {
        Some(r) if r.departed.is_some() => {
            let text = format!("{} has left the shrine", r.name);
            let y = g.y + g.height / 2;
            centered(buf, g, y.saturating_sub(1), &text, Style::new());
            let row =
                [("[ recall r ]", Button::RecallFocused), ("[ close x ]", Button::CloseFocused)];
            flow_centered(buf, g, y + 1, &row, &mut hits);
        }
        Some(_) => {
            if let Some(fr) = &m.screen {
                grid(fr, g, buf);
            }
            if let Some((a, b)) = m.selection {
                for (y, xs) in spans(a, b, g.width).take_while(|(y, _)| *y < g.height) {
                    for x in xs.start..xs.end.min(g.width) {
                        buf[(g.x + x, g.y + y)].modifier.toggle(Modifier::REVERSED);
                    }
                }
            }
            hits.push(g, Hit::Grid);
        }
        None => {
            let h = m.banner.len() as u16 + 2;
            let top = g.y + g.height.saturating_sub(h) / 2;
            let w = m.banner.iter().map(|l| width(l)).max().unwrap_or(0);
            let x = g.x + g.width.saturating_sub(w) / 2;
            for (i, l) in m.banner.iter().enumerate() {
                let y = top + i as u16;
                if y < g.bottom() && x < g.right() {
                    buf.set_stringn(x, y, l, (g.right() - x) as usize, Style::new().fg(Color::Red));
                }
            }
            flow_centered(buf, g, top + h - 1, &[("[summon n]", Button::Summon)], &mut hits);
        }
    }
    if let Some(md) = &m.modal {
        modal(m, md, g, buf, &mut hits);
    }
    hits
}

fn sidebar(m: &Model, side: Rect, buf: &mut Buffer, hits: &mut HitMap) {
    let (border, hint) = match m.leader {
        true => (Style::new().fg(Color::Yellow), " esc cancels "),
        false => (DIM, " ^] then a key "),
    };
    let block = Block::bordered()
        .title(" gensokyo ")
        .title_bottom(Line::from(hint).centered())
        .border_style(border);
    let inner = block.inner(side);
    block.render(side, buf);
    // A host one row high has no inside at all.
    if inner.is_empty() {
        return;
    }
    let w = inner.width as usize;
    // From the bottom up: capture, buttons, then the leader or the message.
    let mut bottom = inner.bottom();
    let mut up = |n: u16| {
        bottom = bottom.saturating_sub(n).max(inner.y);
        bottom
    };
    let capture: &[(&str, Button)] = match m.capture {
        true => &[("[mouse: shrine m]", Button::Capture)],
        false => &[("[mouse: native m]", Button::Capture)],
    };
    let y = up(1);
    let style = if m.capture { BUTTON } else { Style::new().fg(Color::Black).bg(Color::LightRed) };
    flow(buf, Rect { y, height: 1, ..inner }, capture, style, hits);
    if !m.capture {
        buf.set_stringn(inner.x, up(1), "clicks are off: ^] m", w, DIM);
    }
    let buttons = [
        ("[summon n]", Button::Summon),
        ("[cast c]", Button::Cast),
        ("[banish b]", Button::Banish),
        ("[recall r]", Button::Recall),
        ("[timetable t]", Button::Timetable),
        ("[quit q]", Button::Quit),
        ("[?]", Button::Help),
    ];
    if m.leader {
        // Every chord, two a line, where the buttons were: the next key is one of these.
        let lines: Vec<String> = CHORDS
            .chunks(2)
            .map(|p| {
                let right = p.get(1).map_or(String::new(), |(k, v)| format!("{k} {v}"));
                format!("{:<11} {right}", format!("{} {}", p[0].0, p[0].1))
            })
            .collect();
        for l in lines.iter().rev() {
            buf.set_stringn(inner.x, up(1), l, w, Style::new());
        }
        buf.set_stringn(inner.x, up(1), "^] then:", w, PICK.add_modifier(Modifier::BOLD));
    } else {
        let rows = flow_rows(&buttons, inner.width);
        let y = up(rows);
        flow(buf, Rect { y, height: rows, ..inner }, &buttons, BUTTON, hits);
    }
    // The account's usage, from the newest report that has it.
    let newest = m.residents.iter().filter_map(|r| r.telemetry.as_ref()).max_by_key(|t| t.at);
    if let Some(t) = newest {
        for (label, l) in [("wk", &t.seven_day), ("5h", &t.five_hour)] {
            if let Some(l) = l {
                buf.set_stringn(inner.x, up(1), tele::usage(label, l, m.now, 5), w, DIM);
            }
        }
    }
    // The ritual that fires next.
    let rituals = m.rituals.as_deref().unwrap_or_default();
    // A disabled ritual has no next fire.
    let next = rituals
        .iter()
        .filter_map(|r| Some((r.next_fire?, r.next_fire_local.as_deref()?, r.name.as_str())));
    let line = match next.min() {
        Some((_, local, name)) => Some((name.to_string(), when_short(local, &m.today))),
        None if !rituals.is_empty() => Some(("none on".into(), String::new())),
        None => None,
    };
    if let Some((name, at)) = line {
        let y = up(1);
        // The time only beside room for a few letters of the name, which is cut with a `…`.
        let aw = width(&at) as usize;
        let aw = if aw + 6 <= w { aw } else { 0 };
        let room = w.saturating_sub(if aw == 0 { 2 } else { aw + 3 });
        let name = match name.chars().count() > room {
            true => name.chars().take(room.saturating_sub(1)).chain(['…']).collect(),
            false => name,
        };
        buf.set_stringn(inner.x, y, format!("⏲ {name}"), w, Style::new());
        if aw > 0 {
            buf.set_stringn(inner.x + (w - aw) as u16, y, &at, aw, Style::new());
        }
        hits.push(Rect { y, height: 1, ..inner }, Hit::Button(Button::Timetable));
    }
    if let Some(msg) = m.message.as_ref().filter(|_| !m.leader) {
        // Wrapped, not cut: a cast's reply names who was left out at its end.
        let lines = wrap(&msg.text, w, 5);
        let y = up(lines.len() as u16);
        let style = match msg.kind {
            Say::Info => Style::new(),
            Say::Notice => Style::new().fg(Color::Yellow),
            Say::Error => ERROR,
        };
        for (i, l) in lines.iter().enumerate().take((inner.bottom() - y) as usize) {
            buf.set_stringn(inner.x, y + i as u16, l, w, style);
        }
    }
    for (i, r) in m.residents.iter().enumerate() {
        let y = inner.y + i as u16;
        if y + 1 >= bottom {
            break;
        }
        let slot = r.slot.map_or(" ".into(), |s| s.to_string());
        let mut style = match r.state {
            State::Departed => DIM,
            s if s.needs_you() => GOLD,
            _ => Style::new(),
        };
        if Some(&r.id) == m.focused.as_ref() {
            if !r.state.needs_you() {
                style = style.bg(Color::Indexed(237));
            }
            style = style.add_modifier(Modifier::BOLD);
        }
        let line = Rect { y, height: 1, ..inner };
        buf.set_style(line, style);
        // Model and context at the right, while it runs and has reported.
        let t = r.telemetry.as_ref().filter(|_| r.state != State::Departed);
        let right = t.map_or(String::new(), |t| {
            let ctx = t.ctx.map_or(String::new(), |c| format!("{c}%"));
            format!(
                " {:<3} {ctx:>4}",
                t.model.as_deref().map(tele::model_short).unwrap_or_default()
            )
        });
        let left = format!("{slot} {} {}", r.state.glyph(), r.name);
        let rw = (width(&right) as usize).min(w);
        buf.set_stringn(inner.x, y, left, w - rw, style);
        buf.set_stringn(inner.x + (w - rw) as u16, y, right, rw, style);
        hits.push(line, Hit::Resident(i));
    }
}

fn grid(fr: &Frame, g: Rect, buf: &mut Buffer) {
    for (dy, row) in fr.rows.iter().take(g.height as usize).enumerate() {
        for run in row {
            let x = g.x.saturating_add(run.col);
            if x >= g.right() {
                break;
            }
            buf.set_stringn(
                x,
                g.y + dy as u16,
                &run.text,
                (g.right() - x) as usize,
                style(run.style),
            );
        }
    }
}

/// A selection from `a` to `b` in reading order, as a terminal's is (not a rectangle): the
/// cells of each row it covers.
pub fn spans(
    a: (u16, u16),
    b: (u16, u16),
    cols: u16,
) -> impl Iterator<Item = (u16, std::ops::Range<u16>)> {
    let (s, e) = if (a.1, a.0) <= (b.1, b.0) { (a, b) } else { (b, a) };
    (s.1..=e.1).map(move |y| {
        let start = if y == s.1 { s.0 } else { 0 };
        (y, start..if y == e.1 { e.0 + 1 } else { cols })
    })
}

/// The text of a selection: trailing blanks dropped from each row, and one line per row (the
/// frame does not say which rows only wrapped). A wide character is in when either of its
/// cells is.
pub fn selected_text(fr: &Frame, a: (u16, u16), b: (u16, u16)) -> String {
    let mut lines = Vec::new();
    for (y, xs) in spans(a, b, fr.cols) {
        let (mut line, mut x) = (String::new(), 0);
        for run in fr.rows.get(y as usize).into_iter().flatten() {
            // Cells no run covers are blank.
            for gap in x..run.col {
                if xs.contains(&gap) {
                    line.push(' ');
                }
            }
            x = run.col;
            for ch in run.text.chars() {
                let w = ratatui::text::Span::raw(ch.encode_utf8(&mut [0; 4]) as &str).width();
                // A combining mark sits in the cell before it.
                let at = if w == 0 { x.saturating_sub(1) } else { x };
                if at < xs.end && at + w.max(1) as u16 > xs.start {
                    line.push(ch);
                }
                x += w as u16;
            }
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

/// A resident's style as ratatui's.
pub fn style(s: vt::Style) -> Style {
    let color = |c| match c {
        vt::Color::Default => Color::Reset,
        vt::Color::Palette(n) => Color::Indexed(n),
        vt::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    };
    const ATTRS: [(u8, Modifier); 8] = [
        (vt::Style::BOLD, Modifier::BOLD),
        (vt::Style::ITALIC, Modifier::ITALIC),
        (vt::Style::FAINT, Modifier::DIM),
        (vt::Style::UNDERLINE, Modifier::UNDERLINED),
        (vt::Style::INVERSE, Modifier::REVERSED),
        (vt::Style::STRIKETHROUGH, Modifier::CROSSED_OUT),
        (vt::Style::INVISIBLE, Modifier::HIDDEN),
        (vt::Style::BLINK, Modifier::SLOW_BLINK),
    ];
    let m =
        ATTRS.iter().filter(|(b, _)| s.attrs & b != 0).fold(Modifier::empty(), |a, (_, m)| a | *m);
    Style::new().fg(color(s.fg)).bg(color(s.bg)).add_modifier(m)
}

/// One line of a modal.
pub(super) enum Row {
    Text(String, Style),
    /// Text being typed, with the host cursor after it.
    Field(String, Style),
    /// A clickable list row, highlighted when selected.
    Item(usize, String, bool),
    Buttons(Vec<(&'static str, Button)>),
}

impl Row {
    pub(super) fn text(s: &str) -> Row {
        Row::Text(s.to_string(), Style::new())
    }

    pub(super) fn dim(s: &str) -> Row {
        Row::Text(s.to_string(), DIM)
    }

    pub(super) fn yes_no(yes: &'static str, no: &'static str) -> Row {
        Row::Buttons(vec![(yes, Button::Yes), (no, Button::No)])
    }

    pub(super) fn close() -> Row {
        Row::Buttons(vec![("[close esc]", Button::No)])
    }
}

/// Where the open modal goes, centred over the grid and sized to its widest line: its box, the
/// rect its rows are written in, its title and its rows.
fn modal_layout(m: &Model, md: &Modal, g: Rect) -> (Rect, Rect, String, Vec<Row>) {
    use Row::*;
    let (title, rows) = md.view(m, g.width.min(72).saturating_sub(4) as usize);
    let text_w = |r: &Row| match r {
        Text(s, _) | Item(_, s, _) => width(s),
        // Room for the cursor after it.
        Field(s, _) => width(s) + 1,
        Buttons(b) => b.iter().map(|(l, _)| width(l) + 1).sum(),
    };
    let w = (rows.iter().map(text_w).max().unwrap_or(0) + 4).clamp(40, 72).min(g.width);
    let h = (rows.len() as u16 + 2).min(g.height);
    let r = Rect::new(g.x + (g.width - w) / 2, g.y + (g.height - h) / 2, w, h);
    let inner = Block::bordered().inner(r);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    (r, inner, title, rows)
}

/// The open modal, drawn where `modal_layout` puts it.
fn modal(m: &Model, md: &Modal, g: Rect, buf: &mut Buffer, hits: &mut HitMap) {
    use Row::*;
    let (r, inner, title, rows) = modal_layout(m, md, g);
    Clear.render(r, buf);
    Block::bordered().title(title).border_style(Style::new().fg(Color::Yellow)).render(r, buf);
    hits.push(r, Hit::Modal);
    for (i, row) in rows.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let line = Rect { y, height: 1, ..inner };
        match row {
            Text(s, st) | Field(s, st) => {
                buf.set_stringn(line.x, y, s, line.width as usize, *st);
            }
            Item(n, s, sel) => {
                let st = if *sel { PICK } else { Style::new() };
                buf.set_style(line, st);
                buf.set_stringn(line.x, y, s, line.width as usize, st);
                hits.push(line, Hit::Item(*n));
            }
            Buttons(b) => flow(buf, line, b, BUTTON, hits),
        }
    }
}

/// `s` in lines of at most `w` columns, broken at spaces where it can be, at most `n` of them;
/// the last ends in `…` when some was left over.
pub(super) fn wrap(s: &str, w: usize, n: usize) -> Vec<String> {
    let mut lines: Vec<String> = vec![String::new()];
    for word in s.split(' ') {
        let cur = lines.last_mut().expect("never empty");
        let sep = usize::from(!cur.is_empty());
        if width(cur) as usize + sep + width(word) as usize <= w {
            if sep == 1 {
                cur.push(' ');
            }
            cur.push_str(word);
            continue;
        }
        // Too long for a row of its own too: cut where it has to be.
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        let mut next = String::new();
        for ch in word.chars() {
            let cw = width(ch.encode_utf8(&mut [0; 4])) as usize;
            if !next.is_empty() && width(&next) as usize + cw > w {
                lines.push(std::mem::take(&mut next));
            }
            next.push(ch);
        }
        lines.push(next);
    }
    if lines.len() > n {
        lines.truncate(n);
        let last = &mut lines[n - 1];
        while !last.is_empty() && width(last) as usize + 1 > w {
            last.pop();
        }
        last.push('…');
    }
    lines
}

/// The selection marker, so it shows without colour too.
pub(super) fn mark(selected: bool) -> &'static str {
    if selected { "› " } else { "  " }
}

/// Up to `n` indices around `selected`, so it stays in view.
pub(super) fn window(len: usize, selected: usize, n: usize) -> std::ops::Range<usize> {
    let start = (selected + 1).saturating_sub(n).min(len.saturating_sub(n));
    start..len.min(start + n)
}

/// How many rows `flow` needs for these buttons in `w` columns.
fn flow_rows(items: &[(&str, Button)], w: u16) -> u16 {
    let mut rows = 1;
    let mut x = 0;
    for (l, _) in items {
        let lw = width(l);
        if x > 0 && x + lw > w {
            rows += 1;
            x = 0;
        }
        x += lw + 1;
    }
    rows
}

/// Buttons left to right, one space apart, wrapping within `r`. Each is a hit target.
fn flow(buf: &mut Buffer, r: Rect, items: &[(&str, Button)], style: Style, hits: &mut HitMap) {
    let r = r.intersection(*buf.area());
    let (mut x, mut y) = (r.x, r.y);
    for (l, b) in items {
        let lw = width(l);
        if x > r.x && x + lw > r.right() {
            (x, y) = (r.x, y + 1);
        }
        if y >= r.bottom() {
            return;
        }
        let end = buf.set_stringn(x, y, l, r.right().saturating_sub(x) as usize, style).0;
        hits.push(Rect::new(x, y, end - x, 1), Hit::Button(*b));
        x = end + 1;
    }
}

fn flow_centered(buf: &mut Buffer, g: Rect, y: u16, items: &[(&str, Button)], hits: &mut HitMap) {
    let w: u16 = items.iter().map(|(l, _)| width(l) + 1).sum::<u16>().saturating_sub(1);
    if y < g.bottom() {
        let x = g.x + g.width.saturating_sub(w) / 2;
        flow(buf, Rect::new(x, y, g.right() - x, 1), items, BUTTON, hits);
    }
}

fn centered(buf: &mut Buffer, g: Rect, y: u16, s: &str, style: Style) {
    if y >= g.y && y < g.bottom() {
        let x = g.x + g.width.saturating_sub(width(s)) / 2;
        buf.set_stringn(x, y, s, (g.right() - x) as usize, style);
    }
}

pub(super) fn width(s: &str) -> u16 {
    ratatui::text::Line::raw(s).width() as u16
}

pub(super) fn ago(s: i64) -> String {
    match s.max(0) {
        s @ 0..60 => format!("{s}s"),
        s @ 60..3600 => format!("{}m", s / 60),
        s @ 3600..86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}
