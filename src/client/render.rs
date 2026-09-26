//! The screen, drawn as a pure function of the model: the shrine sidebar, the focused
//! resident's grid in a box, and a modal over it. `render` also returns what every cell does
//! when clicked, so clicks are resolved against exactly what was drawn.

use crate::proto::Resident;
use crate::vt::{self, Frame, Modes};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Clear, Widget};

/// The sidebar's width, its box included.
pub const SIDEBAR_W: u16 = 25;

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
    /// One line: an error or a notice.
    pub message: Option<String>,
    /// Drawn when the shrine is empty.
    pub banner: Vec<String>,
    /// Shown as `~` in paths.
    pub home: String,
    /// Epoch seconds, for "5m ago".
    pub now: i64,
}

#[derive(Clone, Debug)]
pub enum Modal {
    Summon(Summon),
    Banish {
        id: String,
        name: String,
    },
    /// The departed, newest first.
    Recall {
        list: Vec<Resident>,
        selected: usize,
    },
    Quit,
    Help,
}

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
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Dir,
    Name,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Busy,
    Awaits,
    Asked,
    Resting,
    Departed,
}

impl Glyph {
    pub fn of(r: &Resident) -> Glyph {
        if r.departed.is_some() { Glyph::Departed } else { Glyph::Resting }
    }

    fn symbol(self) -> &'static str {
        match self {
            Glyph::Busy => "●",
            Glyph::Awaits => "✦",
            Glyph::Asked => "✧",
            Glyph::Resting => "○",
            Glyph::Departed => "·",
        }
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
    Quit,
    Help,
    Capture,
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

const DIM: Style = Style::new().fg(Color::DarkGray);
const BUTTON: Style = Style::new().fg(Color::Cyan);
const PICK: Style = Style::new().fg(Color::Black).bg(Color::Yellow);
const ERROR: Style = Style::new().fg(Color::LightRed);

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

/// Where the host cursor goes: the focused resident's, when it shows one and no modal is open.
pub fn cursor(m: &Model, area: Rect) -> Option<(u16, u16)> {
    let g = grid_rect(area);
    let (x, y) = m.screen.as_ref()?.cursor?;
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
            format!(" {slot}{} · {} ", r.name, tilde(&r.cwd, &m.home))
        }
        None => " the shrine is empty ".into(),
    };
    let block = Block::bordered().title(title).border_style(DIM);
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
    let border = if m.leader { Style::new().fg(Color::Yellow) } else { DIM };
    let block = Block::bordered().title(" gensokyo ").border_style(border);
    let inner = block.inner(side);
    block.render(side, buf);
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
        ("[banish b]", Button::Banish),
        ("[recall r]", Button::Recall),
        ("[quit q]", Button::Quit),
        ("[?]", Button::Help),
    ];
    let rows = flow_rows(&buttons, inner.width);
    let y = up(rows);
    flow(buf, Rect { y, height: rows, ..inner }, &buttons, BUTTON, hits);
    let y = up(1);
    if m.leader {
        buf.set_stringn(inner.x, y, "^] …", w, PICK.add_modifier(Modifier::BOLD));
    } else if let Some(msg) = &m.message {
        buf.set_stringn(inner.x, y, msg, w, ERROR);
    }
    for (i, r) in m.residents.iter().enumerate() {
        let y = inner.y + i as u16;
        if y + 1 >= bottom {
            break;
        }
        let g = Glyph::of(r);
        let slot = r.slot.map_or(" ".into(), |s| s.to_string());
        let mut style = if g == Glyph::Departed { DIM } else { Style::new() };
        if Some(&r.id) == m.focused.as_ref() {
            style = style.bg(Color::Indexed(237)).add_modifier(Modifier::BOLD);
            buf.set_style(Rect { y, height: 1, ..inner }, style);
        }
        buf.set_stringn(inner.x, y, format!("{slot} {} {}", g.symbol(), r.name), w, style);
        hits.push(Rect { y, height: 1, ..inner }, Hit::Resident(i));
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
enum Row {
    Text(String, Style),
    /// A clickable list row, highlighted when selected.
    Item(usize, String, bool),
    Buttons(Vec<(&'static str, Button)>),
}

fn modal(m: &Model, md: &Modal, g: Rect, buf: &mut Buffer, hits: &mut HitMap) {
    use Row::*;
    let t = |s: &str| Text(s.to_string(), Style::new());
    let dim = |s: &str| Text(s.to_string(), DIM);
    let yes_no =
        |yes: &'static str, no: &'static str| Buttons(vec![(yes, Button::Yes), (no, Button::No)]);
    let (title, mut rows): (&str, Vec<Row>) = match md {
        Modal::Summon(s) if s.stage == Stage::Dir => {
            let typed = if s.selected.is_none() { PICK } else { Style::new() };
            let mut rows = vec![
                t("Where?"),
                Text(format!("{}{}█", mark(s.selected.is_none()), s.path), typed),
            ];
            if !s.completions.is_empty() {
                rows.push(dim(&s.completions.join("  ")));
            }
            if !s.recent.is_empty() {
                rows.push(dim("recent"));
                rows.extend(window(s.recent.len(), s.selected.unwrap_or(0), 8).map(|i| {
                    let sel = s.selected == Some(i);
                    Item(i, format!("{}{}", mark(sel), tilde(&s.recent[i], &m.home)), sel)
                }));
            }
            (" summon ", rows)
        }
        Modal::Summon(s) => (
            " summon ",
            vec![
                t(&format!("In {}", tilde(&s.path, &m.home))),
                Text(format!("name: {}█", s.name), PICK),
                dim("Enter picks a random name when this is empty"),
            ],
        ),
        Modal::Banish { name, .. } => (
            " banish ",
            vec![t(&format!("Banish {name}?")), dim("It gets HUP, then TERM, then KILL.")],
        ),
        Modal::Recall { list, .. } if list.is_empty() => {
            (" recall ", vec![dim("Nobody has departed.")])
        }
        Modal::Recall { list, selected } => (
            " recall ",
            window(list.len(), *selected, 12)
                .map(|i| {
                    let r = &list[i];
                    let when = r.departed.map_or(String::new(), |d| ago(m.now - d) + " ago");
                    let home = tilde(&r.cwd, &m.home);
                    let line =
                        format!("{}{:<12} {:>7}  {home}", mark(i == *selected), r.name, when);
                    Item(i, line, i == *selected)
                })
                .collect(),
        ),
        Modal::Quit => (
            " quit ",
            vec![t("Quit the shrine?"), dim("Everyone gets /exit, then the daemon stops.")],
        ),
        Modal::Help => {
            const KEYS: [(&str, &str); 8] = [
                ("n", "summon"),
                ("b", "banish"),
                ("r", "recall"),
                ("x", "close"),
                ("q", "quit"),
                ("?", "help"),
                ("m", "mouse capture"),
                ("d", "detach"),
            ];
            let mut rows = vec![t("Ctrl-] then a key:")];
            rows.extend(
                KEYS.chunks(2)
                    .map(|p| t(&format!("  {}  {:<15} {}  {}", p[0].0, p[0].1, p[1].0, p[1].1))),
            );
            rows.extend(["  1-9  focus that slot", "  Ctrl-]  a literal Ctrl-]"].map(t));
            (" help ", rows)
        }
    };
    let error = match md {
        Modal::Summon(s) => s.error.as_deref(),
        _ => None,
    };
    if let Some(e) = error {
        rows.push(Text(e.to_string(), ERROR));
    }
    rows.push(match md {
        Modal::Summon(_) => yes_no("[summon ⏎]", "[cancel esc]"),
        Modal::Banish { .. } => yes_no("[banish y]", "[cancel n]"),
        Modal::Recall { list, .. } if list.is_empty() => Buttons(vec![("[close esc]", Button::No)]),
        Modal::Recall { .. } => yes_no("[recall ⏎]", "[cancel esc]"),
        Modal::Quit => yes_no("[quit y]", "[cancel n]"),
        Modal::Help => Buttons(vec![("[close esc]", Button::No)]),
    });
    let text_w = |r: &Row| match r {
        Text(s, _) | Item(_, s, _) => width(s),
        Buttons(b) => b.iter().map(|(l, _)| width(l) + 1).sum(),
    };
    let w = (rows.iter().map(text_w).max().unwrap_or(0) + 4).clamp(40, 72).min(g.width);
    let h = (rows.len() as u16 + 2).min(g.height);
    let r = Rect::new(g.x + (g.width - w) / 2, g.y + (g.height - h) / 2, w, h);
    Clear.render(r, buf);
    let block = Block::bordered().title(title).border_style(Style::new().fg(Color::Yellow));
    let inner = block.inner(r);
    block.render(r, buf);
    hits.push(r, Hit::Modal);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    for (i, row) in rows.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let line = Rect { y, height: 1, ..inner };
        match row {
            Text(s, st) => {
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

/// The selection marker, so it shows without colour too.
fn mark(selected: bool) -> &'static str {
    if selected { "› " } else { "  " }
}

/// Up to `n` indices around `selected`, so it stays in view.
fn window(len: usize, selected: usize, n: usize) -> std::ops::Range<usize> {
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

fn width(s: &str) -> u16 {
    ratatui::text::Line::raw(s).width() as u16
}

fn tilde(p: &str, home: &str) -> String {
    match p.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => p.to_string(),
    }
}

fn ago(s: i64) -> String {
    match s.max(0) {
        s @ 0..60 => format!("{s}s"),
        s @ 60..3600 => format!("{}m", s / 60),
        s @ 3600..86400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}
