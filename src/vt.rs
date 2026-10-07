//! The terminal emulator a resident runs behind: libghostty-vt, wrapped in the calls the daemon
//! makes. No libghostty type appears in this module's interface, so the emulator can be swapped
//! here and nowhere else.
//!
//! Nothing the child writes reaches the host terminal: the screen is kept here and the client
//! draws it. So a child's keyboard-mode requests (`CSI >u`, `CSI <u`, `CSI >4;Nm`) are absorbed
//! into this emulator's state. The encoder follows them, and the client sets the host from
//! `kitty_flags`. A forwarded `CSI >4;2m` would turn iTerm2's kitty mode back off.
//!
//! libghostty's calls only fail on arguments this module never passes, hence the `unwrap`s. Sizes
//! are clamped to at least 1x1, and a mode Ghostty doesn't know reads as unset.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::io::Write;
use std::ops::BitOr;
use std::rc::Rc;

use libghostty_vt::fmt::{Format, Formatter, FormatterOptions};
use libghostty_vt::key::{self, Encoder, KittyKeyFlags, OptionAsAlt};
use libghostty_vt::render::{CellIterator, RowIterator};
use libghostty_vt::screen::{CellWide, Screen, TrackedGridRef};
use libghostty_vt::style::{RgbColor, StyleColor, Underline};
use libghostty_vt::terminal::{
    ColorScheme, ConformanceLevel, DeviceAttributeFeature, DeviceAttributes, DeviceType, Mode,
    ModeKind, Options, Point, PointCoordinate, PointSpace, PrimaryDeviceAttributes, ScrollViewport,
    SecondaryDeviceAttributes, TertiaryDeviceAttributes,
};
use libghostty_vt::{RenderState, Terminal};

/// The colours reported for OSC 10/11: iTerm2's default dark profile.
const FG: RgbColor = RgbColor { r: 0xc7, g: 0xc7, b: 0xc7 };
const BG: RgbColor = RgbColor { r: 0, g: 0, b: 0 };

/// Scrollback per resident, in bytes, whatever the C header's "lines" says: measured, 10_000
/// kept a single page (861 rows at 80 columns), and this, Ghostty's own default, keeps about
/// 13,800 rows at 80 columns and 5,400 at 200.
const SCROLLBACK: usize = 10_000_000;

/// DECXCPR, which Claude Code's fullscreen renderer sends and Ghostty leaves unanswered.
const XCPR: &[u8] = b"\x1b[?6n";

/// What a search found is drawn in, over the child's own style: the client's gold.
pub const FOUND: Style = Style { fg: Color::Palette(0), bg: Color::Palette(3), attrs: 0 };

pub struct Vt {
    term: Terminal<'static, 'static>,
    replies: Rc<RefCell<Vec<u8>>>,
    render: RenderState<'static>,
    row_it: RowIterator<'static>,
    cell_it: CellIterator<'static>,
    encoder: Encoder<'static>,
    /// How much of `CSI ?6n` the stream has matched so far; a query can span two reads.
    xcpr: usize,
    /// What the last search found: its first cell, which moves with the text as output comes
    /// and old rows go, and how many columns it covers.
    found: Option<(TrackedGridRef, u16)>,
}

impl Vt {
    pub fn new(cols: u16, rows: u16) -> Vt {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let max_scrollback = SCROLLBACK;
        let mut term = Terminal::new(Options { cols, rows, max_scrollback }).unwrap();
        let replies = Rc::new(RefCell::new(Vec::new()));
        let r = replies.clone();
        term.on_pty_write(move |_, b| r.borrow_mut().extend_from_slice(b)).unwrap();
        // Without this callback DA1/DA2 go unanswered. These are Ghostty's own answers.
        term.on_device_attributes(|_| {
            Some(DeviceAttributes {
                primary: PrimaryDeviceAttributes::new(
                    ConformanceLevel::VT220,
                    &[DeviceAttributeFeature::ANSI_COLOR],
                ),
                secondary: SecondaryDeviceAttributes {
                    device_type: DeviceType::VT220,
                    firmware_version: 0,
                    rom_cartridge: 0,
                },
                tertiary: TertiaryDeviceAttributes { unit_id: 0 },
            })
        })
        .unwrap();
        term.on_color_scheme(|_| Some(ColorScheme::Dark)).unwrap();
        term.set_default_fg_color(Some(FG)).unwrap();
        term.set_default_bg_color(Some(BG)).unwrap();
        // XTVERSION keeps Ghostty's own answer, `libghostty`: Claude Code turns on synchronized
        // output (2026) only after a non-empty one, and this is the one it was seen to accept.
        Vt {
            term,
            replies,
            render: RenderState::new().unwrap(),
            row_it: RowIterator::new().unwrap(),
            cell_it: CellIterator::new().unwrap(),
            encoder: Encoder::new().unwrap(),
            xcpr: 0,
            found: None,
        }
    }

    /// Feeds what the child wrote. Answers to its queries collect for `take_replies`.
    pub fn feed(&mut self, mut bytes: &[u8]) {
        // DECXCPR is answered at its place in the stream, so the position is the one the child
        // asked about and the answer stays in order with Ghostty's own.
        while let Some(end) = self.find_xcpr(bytes) {
            self.term.vt_write(&bytes[..end]);
            let (x, y) = (self.term.cursor_x().unwrap(), self.term.cursor_y().unwrap());
            write!(self.replies.borrow_mut(), "\x1b[?{};{};1R", y + 1, x + 1).unwrap();
            bytes = &bytes[end..];
        }
        self.term.vt_write(bytes);
    }

    /// The end of the first `CSI ?6n` completed in `bytes`, if any.
    fn find_xcpr(&mut self, bytes: &[u8]) -> Option<usize> {
        for (i, &b) in bytes.iter().enumerate() {
            self.xcpr = match b {
                _ if b == XCPR[self.xcpr] => self.xcpr + 1,
                0x1b => 1,
                _ => 0,
            };
            if self.xcpr == XCPR.len() {
                self.xcpr = 0;
                return Some(i + 1);
            }
        }
        None
    }

    /// Bytes to write back to the child: the answers to its queries since the last call.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut *self.replies.borrow_mut())
    }

    /// Columns and rows.
    pub fn size(&self) -> (u16, u16) {
        (self.term.cols().unwrap(), self.term.rows().unwrap())
    }

    /// Scrolled back, the view stays as far from the live screen as it was: a new viewer's
    /// nudge resizes by a row and back, and would move everyone's view otherwise.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let back = self.scrolled().0;
        self.term.resize(cols.max(1), rows.max(1), 0, 0).unwrap();
        let now = self.scrolled().0;
        if back > 0 && now != back {
            self.term.scroll_viewport(ScrollViewport::Delta(now as isize - back as isize));
        }
    }

    /// Whether DEC private mode `n` is set (2004 bracketed paste, 2026 synchronized output, ...).
    pub fn mode(&self, n: u16) -> bool {
        self.term.mode(Mode::new(n, ModeKind::Dec)).unwrap_or(false)
    }

    /// The kitty keyboard flags the child asked for: what the host should be set to.
    pub fn kitty_flags(&self) -> u8 {
        self.term.kitty_keyboard_flags().unwrap().bits()
    }

    /// What the child asked of its terminal that the client has to act on.
    pub fn modes(&self) -> Modes {
        let mouse = [1003, 1002, 1000].into_iter().find(|&m| self.mode(m)).unwrap_or(0);
        Modes {
            kitty: self.kitty_flags(),
            mouse,
            sgr: self.mode(1006),
            paste: self.mode(2004),
            focus: self.mode(1004),
            alt: self.mode(1049) || self.mode(1047) || self.mode(47),
        }
    }

    /// Moves the view through the scrollback: `rows` back (negative) or forward, clamped at
    /// either end, or with none, back to the live screen, where what a search found is let go.
    /// Scrolled back, the view stays on the same rows while output comes in, and the cursor is
    /// not shown. The alternate screen has no scrollback, so there it does nothing.
    pub fn scroll(&mut self, rows: Option<i32>) {
        self.term.scroll_viewport(match rows {
            Some(n) => ScrollViewport::Delta(n as isize),
            None => {
                self.found = None;
                ScrollViewport::Bottom
            }
        });
    }

    /// Looks for `needle` one screen row at a time, `back` toward older output or on toward
    /// newer, from what the last search found while it shows, else from the view's far edge.
    /// Any capital makes case count. Found, it is drawn in every frame, and the view moves to
    /// it unless it shows already. The alternate screen has no scrollback to search.
    pub fn find(&mut self, needle: &str, back: bool) -> bool {
        if self.term.active_screen().unwrap() != Screen::Primary || needle.is_empty() {
            return false;
        }
        let exact = needle.chars().any(char::is_uppercase);
        // One char for one, so an index into the folded row is one into the row.
        let fold = |c: char| if exact { c } else { c.to_lowercase().next().unwrap_or(c) };
        let needle: Vec<char> = needle.chars().map(fold).collect();
        let rows = u32::from(self.size().1);
        let top = self.term.scrollbar().unwrap().offset as u32;
        let at = self.found_at().filter(|&(_, y)| (top..top + rows).contains(&y));
        let text = self.all_text();
        let total = text.len() as u32;
        let from = at.map_or(if back { top + rows - 1 } else { top }, |(_, y)| y);
        let ys: Box<dyn Iterator<Item = u32>> = match back {
            true => Box::new((0..=from.min(total.saturating_sub(1))).rev()),
            false => Box::new(from..total),
        };
        for y in ys {
            let row: Vec<char> = text[y as usize].chars().map(fold).collect();
            let starts = row.windows(needle.len()).enumerate().filter(|(_, w)| *w == needle);
            let mut spans = starts.map(|(i, _)| self.span(y, i, needle.len()));
            let span = match at.filter(|&(_, at_y)| at_y == y) {
                Some((x, _)) if back => spans.rfind(|s| s.0 < x),
                Some((x, _)) => spans.find(|s| s.0 > x),
                None if back => spans.next_back(),
                None => spans.next(),
            };
            if let Some((x, cols)) = span {
                let cell = self.term.track_grid_ref(Point::Screen(PointCoordinate { x, y }));
                self.found = Some((cell.unwrap(), cols));
                if !(top..top + rows).contains(&y) {
                    // A third of the way down, with what led up to it above.
                    let row = y.saturating_sub(rows / 3) as usize;
                    self.term.scroll_viewport(ScrollViewport::Row(row));
                }
                return true;
            }
        }
        false
    }

    /// Whether a search found something that is still drawn.
    pub fn finding(&self) -> bool {
        self.found.is_some()
    }

    /// Where what the last search found starts: column, and row counted from the oldest kept.
    fn found_at(&self) -> Option<(u16, u32)> {
        let p = self.found.as_ref()?.0.point(PointSpace::Screen).ok()??;
        Some((p.x, p.y))
    }

    /// Every row from the oldest kept to the live screen's last, as plain text, one per row.
    fn all_text(&self) -> Vec<String> {
        let opts = FormatterOptions::new().with_format(Format::Plain).with_trim(true);
        let mut f = Formatter::new(&self.term, opts).unwrap();
        let text = f.format_alloc(None).unwrap();
        String::from_utf8_lossy(&text).split('\n').map(str::to_owned).collect()
    }

    /// The first column and the width of `len` characters from character `at` of row `y`, as
    /// the plain text counts them: a wide character is one character over two cells, a cluster
    /// several characters in one.
    fn span(&self, y: u32, at: usize, len: usize) -> (u16, u16) {
        let cols = self.size().0;
        let (mut chars, mut start) = (0, None);
        let mut buf = ['\0'; 16];
        for x in 0..cols {
            let cell = self.term.grid_ref(Point::Screen(PointCoordinate { x, y })).unwrap();
            if matches!(cell.cell().unwrap().wide().unwrap(), CellWide::SpacerTail) {
                continue;
            }
            match start {
                None if chars >= at => start = Some(x),
                Some(s) if chars >= at + len => return (s, x - s),
                _ => {}
            }
            // An empty cell is a blank in the text.
            chars += cell.graphemes(&mut buf).map_or(buf.len(), |n| n.max(1));
        }
        let s = start.unwrap_or(cols);
        (s, cols - s)
    }

    /// Rows the view is scrolled back from the live screen, and rows of scrollback there are.
    /// A few nanoseconds, against a frame's couple of hundred microseconds.
    pub fn scrolled(&self) -> (u32, u32) {
        let bar = self.term.scrollbar().unwrap();
        let history = bar.total.saturating_sub(bar.len);
        (history.saturating_sub(bar.offset) as u32, history as u32)
    }

    /// The live screen's rows as text, wherever the view is: the view is put back as it was.
    pub fn live_text(&mut self) -> Vec<String> {
        let bar = self.term.scrollbar().unwrap();
        if self.scrolled().0 == 0 {
            return self.frame().text();
        }
        self.term.scroll_viewport(ScrollViewport::Bottom);
        let text = self.frame().text();
        self.term.scroll_viewport(ScrollViewport::Row(bar.offset as usize));
        text
    }

    /// The visible screen as style runs.
    pub fn frame(&mut self) -> Frame {
        // What a search found, where it shows: it belongs to the main screen, not the other.
        let found = match self.term.active_screen().unwrap() {
            Screen::Primary => self.found.as_ref(),
            Screen::Alternate => None,
        };
        let found = found.and_then(|(cell, cols)| {
            let p = cell.point(PointSpace::Viewport).ok()??;
            Some((p.y, p.x..p.x + cols))
        });
        let snap = self.render.update(&self.term).unwrap();
        let cursor = match snap.cursor_visible().unwrap() {
            true => snap.cursor_viewport().unwrap().map(|c| (c.x, c.y)),
            false => None,
        };
        let mut rows = Vec::new();
        let mut row_it = self.row_it.update(&snap).unwrap();
        while let Some(row) = row_it.next() {
            let found = found.clone().filter(|(y, _)| *y == rows.len() as u32).map(|f| f.1);
            let mut runs: Vec<Run> = Vec::new();
            let mut cell_it = self.cell_it.update(row).unwrap();
            let mut col = 0;
            while let Some(cell) = cell_it.next() {
                let x = col;
                col += 1;
                let wide = cell.raw_cell().unwrap().wide().unwrap();
                if matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead) {
                    continue;
                }
                let style = match cell.has_styling().unwrap() {
                    _ if found.as_ref().is_some_and(|f| f.contains(&x)) => FOUND,
                    true => Style::from_ghostty(&cell.style().unwrap()),
                    false => Style::default(),
                };
                let g: String = cell.graphemes().unwrap().into_iter().collect();
                let text = if g.is_empty() { " " } else { &g };
                match runs.last_mut() {
                    Some(r) if r.style == style => r.text.push_str(text),
                    _ => runs.push(Run { col: x, style, text: text.to_string() }),
                }
            }
            if let Some(r) = runs.last_mut().filter(|r| r.style == Style::default()) {
                r.text.truncate(r.text.trim_end_matches(' ').len());
            }
            runs.retain(|r| !r.text.is_empty());
            rows.push(runs);
        }
        let (back, history) = self.scrolled();
        Frame { cols: self.term.cols().unwrap(), rows, cursor, back, history }
    }

    /// Encodes a key the way the child asked for: its kitty flags, modifyOtherKeys and
    /// cursor-key mode.
    pub fn encode(&mut self, ev: KeyEvent) -> Vec<u8> {
        self.encoder.set_options_from_terminal(&self.term);
        self.encode_now(ev)
    }

    /// Encodes a key as a terminal holding these keyboard modes would send it.
    pub fn encode_as(&mut self, ev: KeyEvent, kitty: u8, modify_other_keys: bool) -> Vec<u8> {
        self.encoder
            .set_options_from_terminal(&self.term)
            .set_kitty_flags(KittyKeyFlags::from_bits_truncate(kitty))
            .set_modify_other_keys_state_2(modify_other_keys);
        self.encode_now(ev)
    }

    fn encode_now(&mut self, ev: KeyEvent) -> Vec<u8> {
        // Resetting the options resets this too.
        self.encoder.set_macos_option_as_alt(OptionAsAlt::True);
        let mut e = key::Event::new().unwrap();
        e.set_action(match ev.action {
            Action::Press => key::Action::Press,
            Action::Repeat => key::Action::Repeat,
            Action::Release => key::Action::Release,
        })
        .set_mods(ev.mods.to_ghostty());
        match ev.key {
            Key::Char(c) => {
                let unshifted = c.to_ascii_lowercase();
                e.set_key(char_key(unshifted)).set_utf8(Some(c.to_string()));
                e.set_unshifted_codepoint(unshifted);
            }
            k => {
                e.set_key(named_key(k)).set_utf8(None::<String>);
            }
        }
        let mut out = Vec::new();
        self.encoder.encode_to_vec(&e, &mut out).unwrap();
        out
    }
}

/// The child's terminal modes the client acts on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modes {
    /// Kitty keyboard flags.
    pub kitty: u8,
    /// Mouse tracking: 0 none, 1000 clicks, 1002 drags too, 1003 all motion.
    pub mouse: u16,
    /// SGR mouse encoding (1006).
    pub sgr: bool,
    /// Bracketed paste (2004).
    pub paste: bool,
    /// Focus in/out reports (1004).
    pub focus: bool,
    /// The alternate screen.
    pub alt: bool,
}

/// The screen at one moment, as rows of style runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub cols: u16,
    pub rows: Vec<Vec<Run>>,
    /// Column and row of the cursor, when it is visible.
    pub cursor: Option<(u16, u16)>,
    /// Rows scrolled back from the live screen (0 is live), and rows of scrollback there are.
    #[serde(default)]
    pub back: u32,
    #[serde(default)]
    pub history: u32,
}

impl Frame {
    /// One string per row, trailing blanks dropped.
    pub fn text(&self) -> Vec<String> {
        let line = |runs: &[Run]| runs.iter().map(|r| r.text.as_str()).collect::<String>();
        self.rows.iter().map(|r| line(r).trim_end().to_string()).collect()
    }

    /// The rows of `self` that differ from `prev`: every row when the size changed.
    pub fn damage(&self, prev: &Frame) -> Vec<usize> {
        let resized = self.cols != prev.cols || self.rows.len() != prev.rows.len();
        (0..self.rows.len()).filter(|&y| resized || self.rows[y] != prev.rows[y]).collect()
    }
}

/// Cells sharing one style, starting at column `col`. A wide character counts two columns and
/// appears once in `text`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub col: u16,
    pub style: Style,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    /// `Style::BOLD` and the rest, or-ed.
    pub attrs: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Color {
    #[default]
    Default,
    Palette(u8),
    Rgb(u8, u8, u8),
}

impl Style {
    pub const BOLD: u8 = 1;
    pub const ITALIC: u8 = 2;
    pub const FAINT: u8 = 4;
    /// Any underline: single, double, curly, dotted or dashed.
    pub const UNDERLINE: u8 = 8;
    pub const INVERSE: u8 = 16;
    pub const STRIKETHROUGH: u8 = 32;
    pub const INVISIBLE: u8 = 64;
    pub const BLINK: u8 = 128;

    fn from_ghostty(st: &libghostty_vt::style::Style) -> Style {
        let color = |c: &StyleColor| match c {
            StyleColor::None => Color::Default,
            StyleColor::Palette(p) => Color::Palette(p.0),
            StyleColor::Rgb(RgbColor { r, g, b }) => Color::Rgb(*r, *g, *b),
        };
        let attrs = [
            (st.bold, Style::BOLD),
            (st.italic, Style::ITALIC),
            (st.faint, Style::FAINT),
            (st.underline != Underline::None, Style::UNDERLINE),
            (st.inverse, Style::INVERSE),
            (st.strikethrough, Style::STRIKETHROUGH),
            (st.invisible, Style::INVISIBLE),
            (st.blink, Style::BLINK),
        ];
        let attrs = attrs.iter().filter(|(on, _)| *on).fold(0, |a, (_, bit)| a | bit);
        Style { fg: color(&st.fg_color), bg: color(&st.bg_color), attrs }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A printable character, as typed: `Char('A')` with `Mods::SHIFT` for Shift+A.
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1 to F12.
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods(pub u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const SHIFT: Mods = Mods(1);
    pub const ALT: Mods = Mods(2);
    pub const CTRL: Mods = Mods(4);
    pub const SUPER: Mods = Mods(8);

    fn to_ghostty(self) -> key::Mods {
        [
            (Mods::SHIFT, key::Mods::SHIFT),
            (Mods::ALT, key::Mods::ALT),
            (Mods::CTRL, key::Mods::CTRL),
            (Mods::SUPER, key::Mods::SUPER),
        ]
        .into_iter()
        .filter(|(m, _)| self.0 & m.0 != 0)
        .fold(key::Mods::empty(), |a, (_, g)| a | g)
    }
}

impl BitOr for Mods {
    type Output = Mods;
    fn bitor(self, o: Mods) -> Mods {
        Mods(self.0 | o.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Mods,
    pub action: Action,
}

impl KeyEvent {
    pub fn press(key: Key, mods: Mods) -> KeyEvent {
        KeyEvent { key, mods, action: Action::Press }
    }

    /// A key in kitty's numbering (the codepoint of the unshifted key, or Enter 13, Tab 9,
    /// Backspace 127, Escape 27; modifiers as the kitty parameter minus one; event 1 press, 2
    /// repeat, 3 release). None for keys this module has no name for.
    pub fn from_kitty(code: u32, mods: u8, event: u8) -> Option<KeyEvent> {
        let mods = Mods(mods & 0x0f);
        let key = match code {
            13 => Key::Enter,
            9 => Key::Tab,
            127 | 8 => Key::Backspace,
            27 => Key::Escape,
            _ => match char::from_u32(code).filter(|c| !c.is_control()) {
                Some(c) if mods.0 & Mods::SHIFT.0 != 0 => Key::Char(c.to_ascii_uppercase()),
                Some(c) => Key::Char(c),
                None => return None,
            },
        };
        let action = match event {
            2 => Action::Repeat,
            3 => Action::Release,
            _ => Action::Press,
        };
        Some(KeyEvent { key, mods, action })
    }
}

/// The key a character sits on, US layout; `Unidentified` for the rest, which the encoder then
/// sends by its text.
fn char_key(c: char) -> key::Key {
    use key::Key as K;
    const LETTERS: [K; 26] = [
        K::A,
        K::B,
        K::C,
        K::D,
        K::E,
        K::F,
        K::G,
        K::H,
        K::I,
        K::J,
        K::K,
        K::L,
        K::M,
        K::N,
        K::O,
        K::P,
        K::Q,
        K::R,
        K::S,
        K::T,
        K::U,
        K::V,
        K::W,
        K::X,
        K::Y,
        K::Z,
    ];
    const DIGITS: [K; 10] = [
        K::Digit0,
        K::Digit1,
        K::Digit2,
        K::Digit3,
        K::Digit4,
        K::Digit5,
        K::Digit6,
        K::Digit7,
        K::Digit8,
        K::Digit9,
    ];
    match c {
        'a'..='z' => LETTERS[c as usize - 'a' as usize],
        '0'..='9' => DIGITS[c as usize - '0' as usize],
        ' ' => K::Space,
        '`' => K::Backquote,
        '\\' => K::Backslash,
        '[' => K::BracketLeft,
        ']' => K::BracketRight,
        ',' => K::Comma,
        '=' => K::Equal,
        '-' => K::Minus,
        '.' => K::Period,
        '\'' => K::Quote,
        ';' => K::Semicolon,
        '/' => K::Slash,
        _ => K::Unidentified,
    }
}

fn named_key(k: Key) -> key::Key {
    use key::Key as K;
    const F: [K; 12] =
        [K::F1, K::F2, K::F3, K::F4, K::F5, K::F6, K::F7, K::F8, K::F9, K::F10, K::F11, K::F12];
    match k {
        Key::Char(_) => unreachable!("characters go through char_key"),
        Key::Enter => K::Enter,
        Key::Tab => K::Tab,
        Key::Backspace => K::Backspace,
        Key::Escape => K::Escape,
        Key::Up => K::ArrowUp,
        Key::Down => K::ArrowDown,
        Key::Left => K::ArrowLeft,
        Key::Right => K::ArrowRight,
        Key::Home => K::Home,
        Key::End => K::End,
        Key::PageUp => K::PageUp,
        Key::PageDown => K::PageDown,
        Key::Insert => K::Insert,
        Key::Delete => K::Delete,
        Key::F(n) => F.get(usize::from(n).wrapping_sub(1)).copied().unwrap_or(K::Unidentified),
    }
}
