//! Host input framer: splits raw bytes read from the host terminal into keys, text, pastes,
//! mouse reports, focus events and terminal replies. Parsing is for classification only; every
//! chunk that may be forwarded keeps its exact raw bytes.
//!
//! Time is passed in as milliseconds (`now`) so tests can replay fixtures. The event loop calls
//! `feed` on every read and `tick` when `deadline` passes.

use std::fmt;

/// A parsed key, in kitty's model.
pub use crate::proto::Key;

/// A bare ESC (or ESC O) is released as a key after this long without more input.
pub const ESC_MS: f64 = 20.0;
/// An unfinished sequence (`ESC [ ...`, `ESC ] ...`), or a bare ESC while a query is out.
pub const SEQ_MS: f64 = 150.0;
/// A bracketed paste with no `201~` for this long is abandoned.
pub const PASTE_IDLE_MS: f64 = 1000.0;
/// A query that got no reply stops holding ESC back after this long.
pub const QUERY_MS: f64 = 1000.0;
pub const PASTE_MAX: usize = 1 << 20;
const SEQ_CAP: usize = 4096;
const PASTE_END: &[u8] = b"\x1b[201~";

pub const SHIFT: u8 = 1;
pub const ALT: u8 = 2;
pub const CTRL: u8 = 4;

/// An SGR mouse report: `button` as sent (motion 32, wheel 64, Shift 4, Alt 8, Ctrl 16), 1-based
/// host screen cell, and whether it was `M` (press or motion) rather than `m` (release).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mouse {
    pub button: u16,
    pub col: u16,
    pub row: u16,
    pub press: bool,
}

/// An answer to a query the client sent the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// OSC `n`, such as 10 and 11 for the colours.
    Osc(u32),
    /// `CSI ? flags u`: the host's kitty keyboard flags.
    KittyFlags(u32),
    /// Row and column, while a `CSI 6n` is out.
    Cpr(u16, u16),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chunk {
    /// `key` is None for keys forwarded raw without classification (arrows, F-keys, ...).
    Key {
        raw: Vec<u8>,
        key: Option<Key>,
    },
    Text(String),
    /// A bracketed paste's body, without the markers.
    Paste(Vec<u8>),
    /// Unterminated, or longer than `paste_max`; the body is not kept.
    PasteRejected {
        len: usize,
        why: &'static str,
    },
    Mouse {
        raw: Vec<u8>,
        m: Mouse,
    },
    Focus {
        raw: Vec<u8>,
        gained: bool,
    },
    Reply {
        raw: Vec<u8>,
        kind: Reply,
    },
    /// An unfinished sequence that timed out. Never forwarded: it would reach the child as text.
    Dropped(Vec<u8>),
}

struct Paste {
    body: Vec<u8>,
    len: usize,
}

#[derive(Default)]
struct Pending {
    n: u32,
    until: f64,
}

impl Pending {
    fn add(&mut self, now: f64) {
        self.n += 1;
        self.until = now + QUERY_MS;
    }
    fn live(&self, now: f64) -> bool {
        self.n > 0 && now < self.until
    }
    fn take(&mut self) {
        self.n = self.n.saturating_sub(1);
    }
}

enum Scan {
    Chunk(usize, Chunk),
    PasteStart(usize),
    Need,
}

pub struct Framer {
    buf: Vec<u8>,
    last: f64,
    paste: Option<Paste>,
    cpr: Pending,
    osc: Pending,
    /// Longer pastes are counted and rejected.
    pub paste_max: usize,
}

impl Default for Framer {
    fn default() -> Self {
        Self::new()
    }
}

impl Framer {
    pub fn new() -> Self {
        Framer {
            buf: Vec::new(),
            last: 0.0,
            paste: None,
            cpr: Pending::default(),
            osc: Pending::default(),
            paste_max: PASTE_MAX,
        }
    }

    /// Tell the framer what the client wrote to the host, so replies can be told from keys
    /// (`CSI r;c R` is a cursor report only while `CSI 6n` is out; otherwise it is Shift+F3).
    pub fn note_sent(&mut self, b: &[u8], now: f64) {
        for w in b.windows(4) {
            if w == b"\x1b[6n" {
                self.cpr.add(now);
            }
            if w[..2] == *b"\x1b]" && matches!(&w[2..], b"4;" | b"10" | b"11" | b"12") {
                self.osc.add(now);
            }
        }
    }

    /// One read from the host. What cannot be classified yet is held until more comes or
    /// `tick` releases it.
    pub fn feed(&mut self, bytes: &[u8], now: f64) -> Vec<Chunk> {
        self.buf.extend_from_slice(bytes);
        self.last = now;
        let mut out = Vec::new();
        self.drain(&mut out);
        out
    }

    /// When `tick` must be called next, if anything is being held.
    pub fn deadline(&self) -> Option<f64> {
        if self.paste.is_some() {
            return Some(self.last + PASTE_IDLE_MS);
        }
        match self.buf.as_slice() {
            [] => None,
            [0x1b] | [0x1b, b'O'] if !(self.osc.live(self.last) || self.cpr.live(self.last)) => {
                Some(self.last + ESC_MS)
            }
            _ => Some(self.last + SEQ_MS),
        }
    }

    /// Releases what has been held past its deadline.
    pub fn tick(&mut self, now: f64) -> Vec<Chunk> {
        match self.deadline() {
            Some(d) if now >= d => {}
            _ => return Vec::new(),
        }
        let buf = std::mem::take(&mut self.buf);
        if let Some(p) = self.paste.take() {
            return vec![Chunk::PasteRejected { len: p.len + buf.len(), why: "unterminated" }];
        }
        vec![match buf.as_slice() {
            [0x1b] => key(&buf, 27, 0),
            [0x1b, c] if *c >= 0x20 && *c < 0x7f => key(&buf, *c as u32, ALT),
            _ if buf[0] == 0x1b => Chunk::Dropped(buf),
            _ => Chunk::Key { raw: buf, key: None }, // a cut-off UTF-8 sequence
        }]
    }

    fn drain(&mut self, out: &mut Vec<Chunk>) {
        let mut i = 0;
        loop {
            if self.paste.is_some() {
                i = self.paste_body(i, out);
                if self.paste.is_some() {
                    break;
                }
                continue;
            }
            if i >= self.buf.len() {
                break;
            }
            match self.scan(i) {
                Scan::Chunk(n, c) => {
                    out.push(c);
                    i += n;
                }
                Scan::PasteStart(n) => {
                    self.paste = Some(Paste { body: Vec::new(), len: 0 });
                    i += n;
                }
                Scan::Need => break,
            }
        }
        self.buf.drain(..i);
    }

    /// Consume paste body from `buf[i..]`; returns the new index. Keeps up to 5 trailing bytes
    /// back in case they begin a split `ESC[201~`. Past `paste_max` the body is counted, not kept.
    fn paste_body(&mut self, i: usize, out: &mut Vec<Chunk>) -> usize {
        let b = &self.buf[i..];
        let end = b.windows(PASTE_END.len()).position(|w| w == PASTE_END);
        let take = end.unwrap_or(b.len().saturating_sub(PASTE_END.len() - 1));
        let p = self.paste.as_mut().unwrap();
        p.len += take;
        if p.len <= self.paste_max {
            p.body.extend_from_slice(&b[..take]);
        } else {
            p.body = Vec::new();
        }
        let Some(end) = end else { return i + take };
        let p = self.paste.take().unwrap();
        out.push(if p.len <= self.paste_max {
            Chunk::Paste(p.body)
        } else {
            Chunk::PasteRejected { len: p.len, why: "oversized" }
        });
        i + end + PASTE_END.len()
    }

    fn scan(&mut self, i: usize) -> Scan {
        let b = &self.buf[i..];
        let c = b[0];
        if c == 0x1b {
            return self.esc(i);
        }
        if c < 0x20 || c == 0x7f {
            let (code, mods) = c0(c);
            return Scan::Chunk(1, key(&b[..1], code, mods));
        }
        let mut n = 0;
        while n < b.len() && b[n] >= 0x20 && b[n] != 0x7f && b[n] != 0x1b {
            let w = utf8_len(b[n]);
            if w == 0 || n + w > b.len() || std::str::from_utf8(&b[n..n + w]).is_err() {
                break;
            }
            n += w;
        }
        if n > 0 {
            return Scan::Chunk(n, Chunk::Text(String::from_utf8(b[..n].to_vec()).unwrap()));
        }
        let w = utf8_len(c);
        if w > 1 && b.len() < w && b[1..].iter().all(|x| x & 0xc0 == 0x80) {
            return Scan::Need;
        }
        Scan::Chunk(1, Chunk::Key { raw: vec![c], key: None })
    }

    fn esc(&mut self, i: usize) -> Scan {
        let b = &self.buf[i..];
        let Some(&c) = b.get(1) else { return Scan::Need };
        match c {
            b'[' => self.csi(i),
            b']' | b'P' | b'_' | b'^' | b'X' => self.string(i),
            b'O' => match b.get(2) {
                None => Scan::Need,
                Some(x) if x.is_ascii_alphabetic() => {
                    Scan::Chunk(3, Chunk::Key { raw: b[..3].to_vec(), key: None })
                }
                _ => Scan::Chunk(2, key(&b[..2], b'O' as u32, ALT)),
            },
            0x1b => Scan::Chunk(1, key(&b[..1], 27, 0)),
            c if c < 0x20 || c == 0x7f => {
                let (code, mods) = c0(c);
                Scan::Chunk(2, key(&b[..2], code, mods | ALT))
            }
            c if c < 0x80 => Scan::Chunk(2, key(&b[..2], c as u32, ALT)),
            _ => Scan::Chunk(1, key(&b[..1], 27, 0)),
        }
    }

    fn csi(&mut self, i: usize) -> Scan {
        let b = &self.buf[i..];
        let Some(fin) = b.iter().skip(2).position(|&x| !(0x20..=0x3f).contains(&x)).map(|p| p + 2)
        else {
            return if b.len() > SEQ_CAP {
                Scan::Chunk(b.len(), Chunk::Dropped(b.to_vec()))
            } else {
                Scan::Need
            };
        };
        let f = b[fin];
        if !(0x40..=0x7e).contains(&f) {
            return Scan::Chunk(fin, Chunk::Dropped(b[..fin].to_vec()));
        }
        let n = fin + 1;
        let raw = b[..n].to_vec();
        let p = std::str::from_utf8(&b[2..fin]).unwrap_or("");
        let reply = |kind| Scan::Chunk(n, Chunk::Reply { raw: raw.clone(), kind });
        if let Some(m) = p.strip_prefix('<').filter(|_| f == b'M' || f == b'm') {
            if let [button, col, row] = nums(m, ';')[..] {
                let m = Mouse {
                    button: button as u16,
                    col: col as u16,
                    row: row as u16,
                    press: f == b'M',
                };
                return Scan::Chunk(n, Chunk::Mouse { raw, m });
            }
            return Scan::Chunk(n, Chunk::Dropped(raw));
        }
        if let Some(q) = p.strip_prefix('?') {
            return reply(if f == b'u' {
                Reply::KittyFlags(q.parse().unwrap_or(0))
            } else {
                Reply::Other
            });
        }
        if p.starts_with(['>', '=']) {
            return reply(Reply::Other);
        }
        if f == b'R'
            && self.cpr.live(self.last)
            && let [r, c] = nums(p, ';')[..]
        {
            self.cpr.take();
            return reply(Reply::Cpr(r as u16, c as u16));
        }
        match (p, f) {
            ("200", b'~') => return Scan::PasteStart(n),
            ("", b'I' | b'O') => return Scan::Chunk(n, Chunk::Focus { raw, gained: f == b'I' }),
            _ => {}
        }
        let fields: Vec<&str> = p.split(';').collect();
        let k = match (f, &fields[..]) {
            // CSI code[:alts] ; mods[:event] [; text] u
            (b'u', [code, rest @ ..]) => {
                let code = nums(code, ':')[0];
                let me = rest.first().map(|m| nums(m, ':')).unwrap_or_default();
                let mods = me.first().map_or(0, |m| m.saturating_sub(1) as u8);
                Some(Key { code, mods, event: me.get(1).map_or(1, |e| *e as u8) })
            }
            // modifyOtherKeys: CSI 27 ; mods ; code ~
            (b'~', ["27", m, code]) => Some(Key {
                code: code.parse().unwrap_or(0),
                mods: m.parse::<u8>().unwrap_or(1).saturating_sub(1),
                event: 1,
            }),
            _ => None,
        };
        Scan::Chunk(n, Chunk::Key { raw, key: k })
    }

    /// OSC, DCS, APC, PM, SOS: up to ST (`ESC \`), or BEL for OSC.
    fn string(&mut self, i: usize) -> Scan {
        let b = &self.buf[i..];
        let osc = b[1] == b']';
        let mut j = 2;
        let end = loop {
            match b.get(j) {
                None if b.len() > SEQ_CAP => {
                    return Scan::Chunk(b.len(), Chunk::Dropped(b.to_vec()));
                }
                None => return Scan::Need,
                Some(0x07) if osc => break j + 1,
                Some(0x1b) => match b.get(j + 1) {
                    None => return Scan::Need,
                    Some(b'\\') => break j + 2,
                    Some(_) => return Scan::Chunk(j, Chunk::Dropped(b[..j].to_vec())), // damaged reply
                },
                _ => j += 1,
            }
        };
        let raw = b[..end].to_vec();
        let num = std::str::from_utf8(&b[2..end])
            .ok()
            .and_then(|s| s.split(';').next()?.parse().ok())
            .filter(|_| osc);
        let kind = match num {
            Some(n) => {
                self.osc.take();
                Reply::Osc(n)
            }
            None => Reply::Other,
        };
        Scan::Chunk(end, Chunk::Reply { raw, kind })
    }
}

fn key(raw: &[u8], code: u32, mods: u8) -> Chunk {
    Chunk::Key { raw: raw.to_vec(), key: Some(Key { code, mods, event: 1 }) }
}

/// A C0 control byte or DEL as a key: Enter, Tab, Backspace, Ctrl+letter.
fn c0(c: u8) -> (u32, u8) {
    match c {
        b'\r' | b'\t' | 0x7f | 0x1b => (c as u32, 0),
        0 => (b' ' as u32, CTRL),
        1..=26 => ((c + b'a' - 1) as u32, CTRL),
        _ => ((c + b'@') as u32, CTRL),
    }
}

fn utf8_len(c: u8) -> usize {
    match c {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 0,
    }
}

fn nums(s: &str, sep: char) -> Vec<u32> {
    s.split(sep).map(|x| x.parse().unwrap_or(0)).collect()
}

/// Bytes as a quoted string: printable UTF-8 as is, `\e \r \n \t` and `\xNN` for the rest.
pub struct Esc<'a>(pub &'a [u8]);

impl fmt::Display for Esc<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("\"")?;
        for ch in self.0.utf8_chunks() {
            for c in ch.valid().chars() {
                match c {
                    '\x1b' => f.write_str("\\e")?,
                    '\r' => f.write_str("\\r")?,
                    '\n' => f.write_str("\\n")?,
                    '\t' => f.write_str("\\t")?,
                    '"' | '\\' => write!(f, "\\{c}")?,
                    c if (c as u32) < 0x20 || c == '\x7f' => write!(f, "\\x{:02x}", c as u32)?,
                    c => write!(f, "{c}")?,
                }
            }
            for b in ch.invalid() {
                write!(f, "\\x{b:02x}")?;
            }
        }
        f.write_str("\"")
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.code)?;
        for (bit, name) in [
            (1, "shift"),
            (2, "alt"),
            (4, "ctrl"),
            (8, "super"),
            (16, "hyper"),
            (32, "meta"),
            (64, "caps"),
            (128, "num"),
        ] {
            if self.mods & bit != 0 {
                write!(f, "+{name}")?;
            }
        }
        match self.event {
            2 => f.write_str(" repeat"),
            3 => f.write_str(" release"),
            _ => Ok(()),
        }
    }
}

impl fmt::Display for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Chunk::Key { raw, key: Some(k) } => write!(f, "key {} {k}", Esc(raw)),
            Chunk::Key { raw, key: None } => write!(f, "key {}", Esc(raw)),
            Chunk::Text(s) => write!(f, "text {}", Esc(s.as_bytes())),
            Chunk::Paste(b) if b.len() <= 32 => write!(f, "paste {}", Esc(b)),
            Chunk::Paste(b) => write!(
                f,
                "paste len={} head={} tail={}",
                b.len(),
                Esc(&b[..16]),
                Esc(&b[b.len() - 16..])
            ),
            Chunk::PasteRejected { len, why } => write!(f, "paste-rejected len={len} {why}"),
            Chunk::Mouse { raw, m } => {
                write!(
                    f,
                    "mouse {} b={} x={} y={} {}",
                    Esc(raw),
                    m.button,
                    m.col,
                    m.row,
                    if m.press { "press" } else { "release" }
                )
            }
            Chunk::Focus { raw, gained } => {
                write!(f, "focus {} {}", if *gained { "in" } else { "out" }, Esc(raw))
            }
            Chunk::Reply { raw, kind } => match kind {
                Reply::Osc(n) => write!(f, "reply osc{n} {}", Esc(raw)),
                Reply::KittyFlags(n) => write!(f, "reply kitty-flags={n} {}", Esc(raw)),
                Reply::Cpr(r, c) => write!(f, "reply cpr={r};{c} {}", Esc(raw)),
                Reply::Other => write!(f, "reply other {}", Esc(raw)),
            },
            Chunk::Dropped(b) => write!(f, "dropped {}", Esc(b)),
        }
    }
}
