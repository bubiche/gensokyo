//! The screen, drawn as a pure function of the model: the shrine sidebar, the focused
//! resident's grid in a box, and a modal over it. `render` also returns what every cell does
//! when clicked, so clicks are resolved against exactly what was drawn.

use super::modal::{Modal, when_short};
use crate::paths::tilde;
use crate::proto::{CloudWatch, Resident, RitualInfo, State};
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
pub(super) const CHORDS: [(&str, &str); 21] = [
    ("n", "summon"),
    ("c", "cast"),
    ("b", "banish"),
    ("r", "recall"),
    ("t", "timetable"),
    ("x", "close"),
    ("u", "renew"),
    ("j", "next"),
    ("k", "previous"),
    ("a", "needs you"),
    ("[", "scroll"),
    ("/", "search"),
    ("y", "copy last"),
    ("1-9", "slot"),
    ("m", "mouse"),
    ("w", "awake"),
    ("d", "detach"),
    ("q", "quit"),
    ("h", "history"),
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
    /// The leader was pressed and the next key is ours.
    pub leader: bool,
    /// A search through the scrollback: from its prompt until the view goes home.
    pub find: Option<Find>,
    /// A line for the user, above the sidebar's buttons, until it expires.
    pub message: Option<Message>,
    /// What was said since the client came, and the daemon's notices from before, newest
    /// first: when (epoch seconds), and what.
    pub history: Vec<(i64, Message)>,
    /// Drawn when the shrine is empty.
    pub banner: Vec<String>,
    /// Shown as `~` in paths.
    pub home: String,
    /// The config's `BRANCH_PREFIX`, left off branches in the sidebar.
    pub prefix: String,
    /// Epoch seconds, for "5m ago".
    pub now: i64,
    /// This machine's date, `YYYY-MM-DD`: a fire today shows its time alone.
    pub today: String,
    /// The timetable, once the daemon has sent it.
    pub rituals: Option<Vec<RitualInfo>>,
    /// The routines on claude.ai, below the rituals, while the cloud watch is on.
    pub cloud: Option<CloudWatch>,
    /// Keeping the Mac awake, once the daemon has said.
    pub awake: Option<Awake>,
    /// The last answer of the cloud session on screen: its id, its turns then, and the text, or
    /// why it could not be read.
    pub answer: Option<(String, u64, Result<String, String>)>,
}

/// Whether the daemon keeps the Mac from idle-sleeping while work runs, and whether it holds it
/// awake now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Awake {
    pub on: bool,
    pub held: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Find {
    /// What was last looked for.
    pub needle: String,
    /// Toward older output: `/`, and `?` the other way.
    pub back: bool,
    /// The prompt is open, with what is typed so far.
    pub typing: Option<String>,
}

impl Find {
    /// The line on the box: the prompt, or what is being looked for and the keys that go on.
    pub fn line(&self) -> String {
        let mark = if self.back { '/' } else { '?' };
        match &self.typing {
            Some(t) => format!(" {mark}{t}"),
            None => format!(" {mark}{} · n N for more · esc returns ", self.needle),
        }
    }
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
    /// Keeping the Mac awake, on or off.
    Awake,
    /// The open ritual's run now, pause or resume, and remove.
    RunRitual,
    ToggleRitual,
    RemoveRitual,
    /// Everyone behind the installed claude, renewed.
    Renew,
    /// The departed screen's recall and close, for the resident on it.
    RecallFocused,
    CloseFocused,
    /// The cloud session on screen, in the browser, and said something to.
    OpenFocused,
    SayFocused,
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
        let (x, y) = (inner.x + width(&fit_field(s, inner.width)), inner.y + i as u16);
        return (x < inner.right() && y < inner.bottom()).then_some((x, y));
    }
    // The search prompt's, at the end of what is typed on the box's bottom edge.
    if let Some(f) = m.find.as_ref().filter(|f| f.typing.is_some()) {
        let main = main_rect(area);
        let x = (main.x + 1).saturating_add(width(&f.line()));
        return (x < main.right().saturating_sub(1)).then(|| (x, main.bottom() - 1));
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
            // Its tags ahead of the place, which may be cut: short, and cut in the sidebar.
            let t = match r.tags.is_empty() {
                true => String::new(),
                false => format!(" · {}", tags(&r.tags)),
            };
            let f = tele::fields(r.telemetry.as_ref(), r.mode.as_deref(), None, false, m.now);
            let f = match f.is_empty() || r.departed.is_some() {
                true => String::new(),
                false => format!(" · {f}"),
            };
            let at = tilde(r.here.as_ref().unwrap_or(&r.cwd), &m.home);
            format!(" {slot}{}{t} · {at}{branch}{f} ", r.name)
        }
        None => " the shrine is empty ".into(),
    };
    let mut block = Block::bordered().title(title).border_style(DIM);
    // Scrolled back, the way home is on the box, in the colour of a resident that needs you.
    if let Some(fr) = m.screen.as_ref().filter(|fr| fr.back > 0 && f.is_some()) {
        let s = format!(" ↑ {} of {} · esc returns ", fr.back, fr.history);
        block = block.title_top(Line::styled(s, GOLD).right_aligned());
    }
    if let Some(find) = m.find.as_ref().filter(|_| f.is_some()) {
        let mut l = find.line();
        if find.typing.is_some() {
            l.push(' ');
        }
        block = block.title_bottom(Line::styled(l, GOLD));
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
        Some(r) if r.in_cloud() => cloud_pane(m, r, g, buf, &mut hits),
        Some(_) => {
            if let Some(fr) = &m.screen {
                grid(fr, g, buf);
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

/// A cloud session has no screen here: what is known of it, its last answer, and what can be
/// done with it.
fn cloud_pane(m: &Model, r: &Resident, g: Rect, buf: &mut Buffer, hits: &mut HitMap) {
    let Some(c) = &r.cloud else { return };
    let w = g.width.saturating_sub(2) as usize;
    let state = r.detail.as_deref().unwrap_or("starting");
    let turns = match r.turns {
        1 => " · 1 turn".to_string(),
        n if n > 1 => format!(" · {n} turns"),
        _ => String::new(),
    };
    let mut rows = vec![(format!("☁ on claude.ai · {state}{turns}"), Style::new())];
    // As it is copied or clicked: a narrow pane wraps it.
    rows.extend(wrap(&c.url, w, 2).into_iter().map(|l| (l, BUTTON)));
    let branch = c.branch.as_deref().unwrap_or("?");
    rows.push((format!("{} · {} → {branch}", c.repo, c.base), Style::new()));
    if let Some(e) = &c.env {
        rows.push((format!("environment {e}"), DIM));
    }
    rows.push((String::new(), Style::new()));
    let (said, st) = match m.answer.as_ref().filter(|a| a.0 == r.id).map(|a| &a.2) {
        Some(Ok(text)) => (text.as_str(), Style::new()),
        Some(Err(e)) => (e.as_str(), ERROR),
        None => ("no answer yet", DIM),
    };
    // The buttons and a hint below, whatever is cut.
    let room = (g.height as usize).saturating_sub(2);
    let left = room.saturating_sub(rows.len() + 1).max(1);
    rows.extend(wrap(said, w, left).into_iter().map(|l| (l, st)));
    rows.push((String::new(), Style::new()));
    rows.truncate(room);
    let x = g.x + 1;
    for (y, (l, st)) in (g.y..).zip(&rows) {
        buf.set_stringn(x, y, l, w, *st);
        // The link opens it, as its button does.
        if *st == BUTTON {
            hits.push(Rect::new(x, y, width(l), 1), Hit::Button(Button::OpenFocused));
        }
    }
    let y = g.y + rows.len() as u16;
    let row = [
        ("[open ⏎]", Button::OpenFocused),
        ("[say s]", Button::SayFocused),
        ("[close x]", Button::CloseFocused),
    ];
    let width = g.width.saturating_sub(2);
    flow(
        buf,
        Rect { x, y, width, height: g.bottom().saturating_sub(y).min(1) },
        &row,
        BUTTON,
        hits,
    );
    let hint = format!("gensokyo cloud log {0} · new --teleport {0} takes it over", r.name);
    let n = g.bottom().saturating_sub(y + 1).clamp(1, 2) as usize;
    for (y, l) in (y + 1..g.bottom()).zip(wrap(&hint, w, n)) {
        buf.set_stringn(x, y, &l, w, DIM);
    }
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
    // Lit while it holds the Mac awake.
    if let Some(a) = m.awake {
        let (label, style) = match (a.on, a.held) {
            (false, _) => ("[awake: off w]", BUTTON),
            (true, false) => ("[awake: on w]", BUTTON),
            (true, true) => ("[awake: on, holding w]", Style::new().fg(Color::Yellow)),
        };
        let y = up(1);
        flow(buf, Rect { y, height: 1, ..inner }, &[(label, Button::Awake)], style, hits);
    }
    let mut buttons = vec![
        ("[summon n]", Button::Summon),
        ("[cast c]", Button::Cast),
        ("[banish b]", Button::Banish),
        ("[recall r]", Button::Recall),
        ("[timetable t]", Button::Timetable),
        ("[quit q]", Button::Quit),
        ("[?]", Button::Help),
    ];
    // Only while someone is behind the installed claude and not yet on the way.
    if m.residents.iter().any(|r| r.outdated.is_some() && !r.renewing) {
        buttons.insert(0, ("[renew u]", Button::Renew));
    }
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
    // The account's usage, the bar warm or hot as it presses.
    let reports = || m.residents.iter().filter_map(|r| r.telemetry.as_ref());
    let wk = tele::freshest(reports().filter_map(|t| t.seven_day.as_ref()), m.now);
    let five = tele::freshest(reports().filter_map(|t| t.five_hour.as_ref()), m.now);
    for (label, l, span) in [("wk", wk, 7 * 86400), ("5h", five, 5 * 3600)] {
        let Some(l) = l else { continue };
        let y = up(1);
        let heat = heated(tele::usage_heat(l, span, m.now), DIM);
        let parts = [
            (format!("{label} "), DIM),
            (tele::bar(l.used, 8), heat),
            (format!(" {}", tele::usage_tail(l, m.now)), DIM),
        ];
        let mut x = inner.x;
        for (text, style) in parts {
            x = buf.set_stringn(x, y, text, inner.right().saturating_sub(x) as usize, style).0;
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
        // Wrapped, not cut: a cast's reply names who was left out at its end. What does not fit
        // ends in the way to the history, which has it whole.
        let mut lines = wrap(&msg.text, w, usize::MAX);
        if lines.len() > 5 {
            const MORE: &str = "… ^] h";
            lines.truncate(5);
            let last = &mut lines[4];
            while !last.is_empty() && width(last) + width(MORE) > w as u16 {
                last.pop();
            }
            last.push_str(MORE);
        }
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
    let mut order = drawn(&m.residents);
    // Those with a helper below them: a lead with helpers, and each helper but the last.
    let held: Vec<usize> = order.windows(2).filter(|p| p[1].1).map(|p| p[0].0).collect();
    // Each one's model at its right only while they are not all on the same one: the one on
    // screen has its own on the box.
    let mut models = m
        .residents
        .iter()
        .filter(|r| r.state != State::Departed)
        .filter_map(|r| r.telemetry.as_ref()?.model.as_deref())
        .map(tele::model_short);
    let first = models.next();
    let model_col = models.any(|s| Some(&s) != first.as_ref());
    // Model and context at the right, while it runs and has reported.
    let right = |r: &Resident| {
        let t = r.telemetry.as_ref().filter(|_| r.state != State::Departed);
        t.map_or(String::new(), |t| {
            let ctx = t.ctx.map_or(String::new(), |c| format!("{c}%"));
            match (model_col, ctx.is_empty()) {
                (true, _) => {
                    let model = t.model.as_deref().map(tele::model_short).unwrap_or_default();
                    format!(" {model:<3} {ctx:>4}")
                }
                (false, true) => String::new(),
                (false, false) => format!(" {ctx:>4}"),
            }
        })
    };
    let lead = |i: usize, helper: bool| {
        let r = &m.residents[i];
        let slot = r.slot.map_or(" ".into(), |s| s.to_string());
        let tee = match (helper, held.contains(&i)) {
            (false, _) => "",
            (true, true) => "├",
            (true, false) => "└",
        };
        // A cloud session: on claude.ai, with nothing here to be in a state.
        let glyph = if r.in_cloud() { "☁" } else { r.state.glyph() };
        format!("{slot} {tee}{glyph} ")
    };
    // Behind the claude installed now, or on its way to it.
    let renew = |r: &Resident| match (r.renewing, r.outdated.is_some()) {
        (true, _) => Some(" ↻"),
        (false, true) => Some(" ⇡"),
        _ => None,
    };
    // Each one's branch and tags on a dim row below it, while every row fits.
    let has_row = |r: &Resident| shown_branch(r, &m.prefix).is_some() || !r.tags.is_empty();
    let on = |(i, _): &(usize, bool)| Some(&m.residents[*i].id) == m.focused.as_ref();
    // The one on screen whole, while there is room: its name over as many rows as it takes,
    // and its dim row uncut.
    let whole = order.iter().find(|p| on(p)).map(|&(i, helper)| {
        let r = &m.residents[i];
        let pad = width(&lead(i, helper)) as usize;
        let mark = renew(r).map_or(0, |_| 2);
        let most = w.saturating_sub(pad + width(&right(r)) as usize + mark).max(1);
        let name = match width(&r.name) as usize > most {
            true => wrap(&r.name, most, 3),
            false => vec![r.name.clone()],
        };
        let dim = match has_row(r) {
            true => whole_row(r, &m.prefix, w.saturating_sub(pad).max(1)),
            false => Vec::new(),
        };
        (i, name, dim)
    });
    let branches = order.iter().filter(|(i, _)| has_row(&m.residents[*i])).count();
    let room = bottom.saturating_sub(inner.y + 1) as usize;
    let (names, dims) = whole.as_ref().map_or((0, 0), |(_, n, d)| (n.len() - 1, d.len()));
    // Best first: every dim row and the one on screen whole, every dim row, a row each but the
    // one on screen whole, a row each.
    let n = order.len();
    let (two, grow) = if n + branches + names + dims.saturating_sub(1) <= room {
        (true, true)
    } else if n + branches <= room {
        (true, false)
    } else {
        (false, n + names + dims <= room)
    };
    // More than fit: as many as do around the one on screen, and a line for those above and
    // one for those below.
    let (mut above, mut below) = (Vec::new(), Vec::new());
    if n > room {
        let f = order.iter().position(on).unwrap_or(0);
        let mut fit = room.saturating_sub(1).max(1);
        if window(n, f, fit).start > 0 && window(n, f, fit).end < n {
            fit = room.saturating_sub(2).max(1);
        }
        let shown = window(n, f, fit);
        below = order.split_off(shown.end);
        above = order.drain(..shown.start).collect();
    }
    let mut y = inner.y;
    if !above.is_empty() && y + 1 < bottom {
        more(buf, Rect { y, height: 1, ..inner }, "↑", &above, m, hits);
        y += 1;
    }
    for (i, helper) in order {
        let r = &m.residents[i];
        if y + 1 >= bottom {
            break;
        }
        let mut style = match r.state {
            State::Departed => DIM,
            s if s.needs_you() => GOLD,
            _ => Style::new(),
        };
        let focused = Some(&r.id) == m.focused.as_ref();
        if focused {
            if !r.state.needs_you() {
                style = style.bg(Color::Indexed(237));
            }
            style = style.add_modifier(Modifier::BOLD);
        }
        let line = Rect { y, height: 1, ..inner };
        buf.set_style(line, style);
        let lead = lead(i, helper);
        let pad = width(&lead) as usize;
        let (name, dim) = match whole.as_ref().filter(|wh| grow && wh.0 == i) {
            Some((_, n, d)) => (n.clone(), d.clone()),
            None if two && has_row(r) => {
                (vec![r.name.clone()], vec![branch_row(r, &m.prefix, w.saturating_sub(pad))])
            }
            None => (vec![r.name.clone()], Vec::new()),
        };
        let t = r.telemetry.as_ref().filter(|_| r.state != State::Departed);
        let right = right(r);
        let rw = (width(&right) as usize).min(w);
        let (x, _) = buf.set_stringn(inner.x, y, format!("{lead}{}", name[0]), w - rw, style);
        let end = inner.x + (w - rw) as u16;
        if let Some(g) = renew(r).filter(|_| x + 2 <= end) {
            let mark = if r.state.needs_you() { style } else { style.fg(Color::Yellow) };
            buf.set_stringn(x, y, g, 2, mark);
        }
        buf.set_stringn(inner.x + (w - rw) as u16, y, right, rw, style);
        // The context, warm or hot as it fills, unless the line is already gold.
        if let Some(c) = t.and_then(|t| t.ctx).filter(|_| !r.state.needs_you()) {
            let pct = format!("{c}%");
            let x = inner.x + w as u16 - width(&pct);
            if rw >= pct.len() {
                buf.set_stringn(x, y, &pct, pct.len(), heated(tele::heat(c, 0), style));
            }
        }
        hits.push(line, Hit::Resident(i));
        y += 1;
        let under = match focused && !r.state.needs_you() {
            true => DIM.bg(Color::Indexed(237)),
            false => DIM,
        };
        // The rest of its name, then its dim rows, under the name, a lead's `│` down beside them.
        let rest = name[1..].iter().map(|s| (s, style)).chain(dim.iter().map(|s| (s, under)));
        for (s, st) in rest {
            if y + 1 >= bottom {
                break;
            }
            let line = Rect { y, height: 1, ..inner };
            buf.set_style(line, st);
            buf.set_stringn(inner.x + pad as u16, y, s, w.saturating_sub(pad), st);
            // Under the tee, one past the slot.
            let bar = r.slot.map_or(1, |s| width(&s.to_string())) + 1;
            if held.contains(&i) && (bar as usize) < w {
                buf.set_stringn(inner.x + bar, y, "│", 1, st);
            }
            hits.push(line, Hit::Resident(i));
            y += 1;
        }
    }
    if !below.is_empty() && y + 1 < bottom {
        more(buf, Rect { y, height: 1, ..inner }, "↓", &below, m, hits);
    }
}

/// `key=value` for each tag, in order.
fn tags(t: &[(String, String)]) -> String {
    t.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")
}

/// `s` in `most` columns, its end cut to `…` when it is wider.
fn cut(s: &str, most: usize) -> String {
    match width(s) as usize > most {
        true => take(s.chars(), most.saturating_sub(1)).into_iter().chain(['…']).collect(),
        false => s.into(),
    }
}

/// The first of `cs` in `room` columns, taken by width, so a wide letter is never counted as one
/// column.
fn take(cs: impl Iterator<Item = char>, mut room: usize) -> Vec<char> {
    let mut v = Vec::new();
    for c in cs {
        let cw = width(c.encode_utf8(&mut [0; 4])) as usize;
        if cw > room {
            break;
        }
        room -= cw;
        v.push(c);
    }
    v
}

/// `s` in `most` columns, its middle cut to `…` when it is wider: a branch's end is what tells
/// it from its siblings.
fn cut_mid(s: &str, most: usize) -> String {
    if width(s) as usize <= most {
        return s.into();
    }
    let head = take(s.chars(), most.saturating_sub(1) / 2);
    let tail = take(s.chars().rev(), most.saturating_sub(1) - most.saturating_sub(1) / 2);
    head.into_iter().chain(['…']).chain(tail.into_iter().rev()).collect()
}

/// The branch on a resident's dim row, `BRANCH_PREFIX` left off: none when its last part only
/// says the name again, `_`, `-` and a space alike.
fn shown_branch<'a>(r: &'a Resident, prefix: &str) -> Option<&'a str> {
    let b = r.branch.as_deref()?;
    let b = b.strip_prefix(prefix).filter(|b| !b.is_empty()).unwrap_or(b);
    let same = |s: &str| s.to_lowercase().replace(['_', ' '], "-");
    (same(b.rsplit('/').next().unwrap_or(b)) != same(&r.name)).then_some(b)
}

/// A resident's dim row in `room` columns: `⎇ branch`, then its tags. The branch is cut first,
/// in its middle, to no less than half the row, then the tags at their end.
fn branch_row(r: &Resident, prefix: &str, room: usize) -> String {
    let t = tags(&r.tags);
    let Some(b) = shown_branch(r, prefix) else { return cut(&t, room) };
    let after = if t.is_empty() { 0 } else { width(&t) as usize + 1 };
    let most = room.saturating_sub(2 + after).max(room.saturating_sub(2) / 2).max(2);
    let b = format!("⎇ {}", cut_mid(b, most));
    cut(&if t.is_empty() { b } else { format!("{b} {t}") }, room)
}

/// The dim row uncut, over as many rows of `room` as it takes, at most three: a branch that
/// would leave no room on them for its first tag leaves the last row to its tags.
fn whole_row(r: &Resident, prefix: &str, room: usize) -> Vec<String> {
    let t = tags(&r.tags);
    // Held to its branch by a no-break space, so `⎇` is not left on a row of its own.
    let b = shown_branch(r, prefix).map(|b| format!("⎇\u{a0}{b}"));
    let crowds = |b: &str| {
        let rows = wrap(b, room, usize::MAX);
        let first = width(&tags(&r.tags[..1])) as usize;
        rows.len() > 3 || rows.len() == 3 && width(&rows[2]) as usize + 1 + first > room
    };
    let mut v = match b {
        Some(b) if t.is_empty() => wrap(&b, room, 3),
        Some(b) if crowds(&b) => {
            let mut v = wrap(&b, room, 2);
            v.push(cut(&t, room));
            v
        }
        Some(b) => wrap(&format!("{b} {t}"), room, 3),
        None => wrap(&t, room, 3),
    };
    v[0] = v[0].replacen('\u{a0}', " ", 1);
    v
}

/// A word in pieces, each ending in a `-` or a `/` but the last; a run of them stays with the
/// letters after it, so `--with` is one.
fn pieces(word: &str) -> Vec<&str> {
    let mut v = Vec::new();
    let (mut start, mut letters) = (0, false);
    let mut cs = word.char_indices().peekable();
    while let Some((at, c)) = cs.next() {
        let sep = matches!(c, '-' | '/');
        letters |= !sep;
        let next_sep = cs.peek().map(|&(_, c)| matches!(c, '-' | '/'));
        if sep && letters && next_sep == Some(false) {
            v.push(&word[start..at + 1]);
            (start, letters) = (at + 1, false);
        }
    }
    v.push(&word[start..]);
    v
}

/// `calm` while a number is calm, else yellow or red over it.
fn heated(h: tele::Heat, calm: Style) -> Style {
    match h {
        tele::Heat::Calm => calm,
        tele::Heat::Warn => calm.fg(Color::Yellow),
        tele::Heat::Hot => calm.fg(Color::LightRed),
    }
}

/// The line for residents the sidebar has no room for: how many, gold with how many of them
/// need you. A click shows the first that does, else the nearest.
fn more(
    buf: &mut Buffer,
    line: Rect,
    arrow: &str,
    hidden: &[(usize, bool)],
    m: &Model,
    hits: &mut HitMap,
) {
    let needs: Vec<usize> =
        hidden.iter().map(|h| h.0).filter(|&i| m.residents[i].state.needs_you()).collect();
    let (text, style) = match needs.len() {
        0 => (format!("{arrow} {} more", hidden.len()), DIM),
        1 => (format!("{arrow} {} more, 1 needs you", hidden.len()), GOLD),
        n => (format!("{arrow} {} more, {n} need you", hidden.len()), GOLD),
    };
    buf.set_stringn(line.x, line.y, text, line.width as usize, style);
    let near = if arrow == "↑" { hidden.last() } else { hidden.first() };
    if let Some(i) = needs.first().copied().or(near.map(|h| h.0)) {
        hits.push(line, Hit::Resident(i));
    }
}

/// The sidebar's order: each resident, and below it the helpers it leads. A helper whose lead
/// is not in the list stands on its own. Each is its index into `residents`, and whether it is
/// drawn as a helper.
pub fn drawn(residents: &[Resident]) -> Vec<(usize, bool)> {
    let lead = |r: &Resident| {
        let o = r.owner.as_deref()?;
        residents.iter().position(|l| l.id == o && l.id != r.id && lead_free(residents, l))
    };
    let (all, mut v) = (0..residents.len(), Vec::new());
    for i in all.clone().filter(|&i| lead(&residents[i]).is_none()) {
        v.push((i, false));
        v.extend(all.clone().filter(|&j| lead(&residents[j]) == Some(i)).map(|j| (j, true)));
    }
    v
}

/// Not drawn under anyone: its lead, if it names one, is not in the list.
fn lead_free(residents: &[Resident], r: &Resident) -> bool {
    r.owner.as_deref().is_none_or(|o| residents.iter().all(|l| l.id != o || l.id == r.id))
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
    let (title, rows) = md.view(m, g.width.min(72).saturating_sub(4) as usize, g.height as usize);
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
            Text(s, st) => {
                buf.set_stringn(line.x, y, s, line.width as usize, *st);
            }
            Field(s, st) => {
                buf.set_stringn(line.x, y, fit_field(s, line.width), line.width as usize, *st);
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

/// `s` in lines of at most `w` columns, broken at spaces where it can be, else a word too wide
/// for a row after a `-` or a `/` as in a branch or a path, at most `n` of them; the last ends in
/// `…` when some was left over.
pub(super) fn wrap(s: &str, w: usize, n: usize) -> Vec<String> {
    let mut lines: Vec<String> = vec![String::new()];
    for word in s.split(' ') {
        let pieces = match width(word) as usize > w {
            true => pieces(word),
            false => vec![word],
        };
        for (i, piece) in pieces.into_iter().enumerate() {
            let cur = lines.last_mut().expect("never empty");
            let sep = usize::from(i == 0 && !cur.is_empty());
            if width(cur) as usize + sep + width(piece) as usize <= w {
                if sep == 1 {
                    cur.push(' ');
                }
                cur.push_str(piece);
                continue;
            }
            // Too long for a row of its own too: cut where it has to be.
            if lines.last().is_some_and(String::is_empty) {
                lines.pop();
            }
            let mut next = String::new();
            for ch in piece.chars() {
                let cw = width(ch.encode_utf8(&mut [0; 4])) as usize;
                if !next.is_empty() && width(&next) as usize + cw > w {
                    lines.push(std::mem::take(&mut next));
                }
                next.push(ch);
            }
            lines.push(next);
        }
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

/// A text field as it fits `w` columns with the cursor after it: its end, which is where the
/// typing is, with `…` for what is cut from its start.
fn fit_field(s: &str, w: u16) -> String {
    if width(s) < w {
        return s.to_string();
    }
    let mut rest = s;
    while !rest.is_empty() && width(rest) + 2 > w {
        let mut cs = rest.chars();
        cs.next();
        rest = cs.as_str();
    }
    format!("…{rest}")
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
