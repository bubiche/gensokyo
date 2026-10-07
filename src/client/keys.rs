//! What the host's input becomes for the resident on screen: bytes as they came, a key for the
//! daemon to encode with the resident's own encoder, a scroll, or nothing. Also the leader chord
//! and the host's kitty keyboard bytes. All pure: the event loop holds the state.
//!
//! The client sets the host to the focused resident's kitty flags, so keys normally go through
//! raw. They differ when the host has no kitty support (Terminal.app never answers `CSI ?u`, so
//! its flags are 0) or has not caught up with a focus switch yet; a key parsed out of the host's
//! encoding is then re-encoded by the daemon for the child.

use super::framer::{CTRL, Chunk, Key, Mouse, SHIFT};
use crate::vt::{KeyEvent, Modes};

/// Kitty flag 2: report press, repeat and release. A child without it gets presses only.
const EVENTS: u8 = 2;
/// Kitty flag 8: every key as an escape code, Enter and Tab included.
const ALL_AS_ESCAPES: u8 = 8;
/// Caps Lock and Num Lock: they never change what a key means here.
const LOCKS: u8 = 64 | 128;

/// Ctrl-], which Claude Code doesn't bind. The next key is taken by `chord`.
pub const LEADER: Key = Key { code: b']' as u32, mods: CTRL, event: 1 };

pub const KITTY_QUERY: &[u8] = b"\x1b[?u";
pub const KITTY_POP: &[u8] = b"\x1b[<u";

/// Pushes `flags` onto the host's kitty stack, once at start; `KITTY_POP` undoes it at exit.
pub fn kitty_push(flags: u8) -> Vec<u8> {
    format!("\x1b[>{flags}u").into_bytes()
}

/// Replaces the pushed entry's flags, on a focus switch.
pub fn kitty_set(flags: u8) -> Vec<u8> {
    format!("\x1b[={flags};1u").into_bytes()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forward {
    /// Write these to the resident's tty.
    Bytes(Vec<u8>),
    /// Send `input {key}`: the daemon encodes it for the child.
    Key(Key),
    /// Scroll the resident's own scrollback by this many rows, negative up.
    Scroll(i16),
    Drop,
}

/// A host chunk the grid has the keyboard for. `host` is the host's kitty flags as it last
/// reported them (0 until it answers). Mouse reports go through the hit map and `mouse`
/// instead; replies, rejected pastes and dropped sequences are never forwarded.
pub fn forward(c: &Chunk, host: u8, child: &Modes) -> Forward {
    match c {
        Chunk::Key { raw, key } => forward_key(raw, *key, host, child),
        Chunk::Text(t) => Forward::Bytes(t.as_bytes().to_vec()),
        Chunk::Paste(body) => Forward::Bytes(paste(body, child)),
        Chunk::Focus { raw, .. } if child.focus => Forward::Bytes(raw.clone()),
        _ => Forward::Drop,
    }
}

/// A paste as the child takes it: bracketed only if it turned 2004 on.
pub fn paste(body: &[u8], child: &Modes) -> Vec<u8> {
    match child.paste {
        true => [b"\x1b[200~", body, b"\x1b[201~"].concat(),
        false => body.to_vec(),
    }
}

fn forward_key(raw: &[u8], key: Option<Key>, host: u8, child: &Modes) -> Forward {
    let events = child.kitty & EVENTS != 0;
    let Some(mut k) = key else { return Forward::Bytes(raw.to_vec()) };
    // A host at flag 2 reports releases of every key, text keys included.
    if k.event == 3 && !events {
        return Forward::Drop;
    }
    if host == child.kitty {
        return Forward::Bytes(raw.to_vec());
    }
    if !events {
        k.event = 1;
    }
    // Plain Enter, Tab and Backspace read the same in every mode short of all-as-escapes.
    let plain = k.event == 1 && k.mods & !LOCKS == 0 && child.kitty & ALL_AS_ESCAPES == 0;
    if plain && matches!(raw, b"\r" | b"\t" | b"\x7f") {
        return Forward::Bytes(raw.to_vec());
    }
    match KeyEvent::from_kitty(k.code, k.mods, k.event) {
        Some(_) => Forward::Key(k),
        None => Forward::Bytes(raw.to_vec()),
    }
}

/// The leader's press, however the host encoded it: 0x1d legacy, `CSI 93;5u` under kitty.
pub fn is_leader(c: &Chunk) -> bool {
    matches!(c, Chunk::Key { key: Some(k), .. }
        if k.code == LEADER.code && k.mods & !LOCKS == CTRL && k.event != 3)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    Summon,
    Banish,
    Recall,
    Cast,
    Timetable,
    Quit,
    Help,
    /// Mouse capture on or off (native text selection while off).
    Capture,
    Detach,
    Close,
    /// Back through the resident's scrollback, half a screen.
    ScrollBack,
    /// Look back through the resident's scrollback for something.
    Find,
    /// The next or the previous resident in the sidebar, round at the ends.
    Next,
    Prev,
    /// The next resident that needs the user.
    Awaiting,
    /// Slot 1 to 9.
    Focus(u8),
    /// The leader twice: send the resident one Ctrl-], as `Forward::Key(LEADER)`.
    Leader,
    Cancel,
    /// A key with no binding: the chord ends with nothing done.
    Unbound,
}

/// What the chunk after the leader means. None for chunks that don't end the chord: releases,
/// focus reports, replies, mouse.
pub fn chord(c: &Chunk) -> Option<Chord> {
    let ch = match c {
        Chunk::Text(t) => {
            let mut cs = t.chars();
            match (cs.next(), cs.next()) {
                (Some(ch), None) => ch,
                _ => return Some(Chord::Unbound),
            }
        }
        Chunk::Key { key: Some(k), .. } if k.event == 3 => return None,
        _ if is_leader(c) => return Some(Chord::Leader),
        Chunk::Key { key: Some(k), .. } => match (k.code, k.mods & !LOCKS) {
            (27, 0) => return Some(Chord::Cancel),
            // `?` from kitty's all-as-escapes mode: Shift and the `/` key, US layout.
            (47, SHIFT) => '?',
            (code, 0) => char::from_u32(code).unwrap_or('\0'),
            _ => return Some(Chord::Unbound),
        },
        Chunk::Key { key: None, .. } | Chunk::Paste(_) | Chunk::PasteRejected { .. } => {
            return Some(Chord::Unbound);
        }
        _ => return None,
    };
    Some(match ch {
        'n' => Chord::Summon,
        'b' => Chord::Banish,
        'r' => Chord::Recall,
        'c' => Chord::Cast,
        't' => Chord::Timetable,
        'q' => Chord::Quit,
        '?' => Chord::Help,
        'm' => Chord::Capture,
        'd' => Chord::Detach,
        'x' => Chord::Close,
        '[' => Chord::ScrollBack,
        '/' => Chord::Find,
        'j' => Chord::Next,
        'k' => Chord::Prev,
        'a' => Chord::Awaiting,
        '1'..='9' => Chord::Focus(ch as u8 - b'0'),
        _ => Chord::Unbound,
    })
}

/// What a key does while the resident on screen is scrolled back through its scrollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scrollback {
    /// Rows, negative back.
    By(i32),
    /// Screens, negative back.
    Pages(i32),
    Top,
    /// Back to the live screen, and the key goes no further.
    Live,
    /// Look for something: `back` toward older output.
    Find(bool),
    /// Look for it again, `true` the other way: what it is if a search is on, else any key.
    Again(bool),
    /// A release: nothing.
    Stay,
}

/// A key as the scrollback reads it: rows and pages as less and a terminal's scrollback have
/// them, a search, Esc and q to leave. None is any other key, which goes back to the live
/// screen and on to the resident.
pub fn scrollback(c: &Chunk) -> Option<Scrollback> {
    use Scrollback::*;
    let ch = match c {
        Chunk::Text(t) if t.chars().count() == 1 => t.chars().next(),
        Chunk::Key { key: Some(k), .. } if k.event == 3 => return Some(Stay),
        Chunk::Key { key: Some(k), .. } => match (k.code, k.mods & !LOCKS) {
            (27, 0) => return Some(Live),
            // Shifted, from kitty's all-as-escapes mode: `?` is Shift and `/`, US layout.
            (47, SHIFT) => Some('?'),
            (code, SHIFT) => char::from_u32(code)
                .filter(char::is_ascii_lowercase)
                .map(|c| c.to_ascii_uppercase()),
            (code, 0) => char::from_u32(code),
            _ => None,
        },
        Chunk::Key { raw, key: None } => {
            return match raw.as_slice() {
                b"\x1b[A" | b"\x1bOA" => Some(By(-1)),
                b"\x1b[B" | b"\x1bOB" => Some(By(1)),
                b"\x1b[5~" => Some(Pages(-1)),
                b"\x1b[6~" => Some(Pages(1)),
                b"\x1b[H" | b"\x1bOH" | b"\x1b[1~" | b"\x1b[7~" => Some(Top),
                b"\x1b[F" | b"\x1bOF" | b"\x1b[4~" | b"\x1b[8~" => Some(Live),
                // A kitty release (`CSI 1;1:3A`), when the resident asked for them: nothing.
                r if r.windows(2).any(|w| w == b":3") => Some(Stay),
                _ => None,
            };
        }
        _ => None,
    };
    match ch? {
        'k' => Some(By(-1)),
        'j' => Some(By(1)),
        'b' => Some(Pages(-1)),
        'f' | ' ' => Some(Pages(1)),
        'g' => Some(Top),
        'G' | 'q' => Some(Live),
        '/' => Some(Find(true)),
        '?' => Some(Find(false)),
        'n' => Some(Again(false)),
        'N' => Some(Again(true)),
        _ => None,
    }
}

/// A host mouse report over the grid, at grid cell `x`, `y` (1-based). With the child's mouse
/// mode it is encoded for the child if the mode covers it; without one, the wheel scrolls:
/// arrow keys on the alternate screen, the daemon's scrollback on the main one.
pub fn mouse(m: &Mouse, x: u16, y: u16, child: &Modes) -> Forward {
    let wheel = m.button & 64 != 0;
    if child.mouse == 0 {
        if !wheel || !m.press {
            return Forward::Drop;
        }
        let up = m.button & 1 == 0;
        return match (child.alt, up) {
            (true, true) => Forward::Bytes(b"\x1b[A".repeat(3)),
            (true, false) => Forward::Bytes(b"\x1b[B".repeat(3)),
            (false, true) => Forward::Scroll(-3),
            (false, false) => Forward::Scroll(3),
        };
    }
    let motion = m.button & 32 != 0;
    let held = m.button & 3 != 3;
    let covered = match child.mouse {
        1003 => true,
        1002 => !motion || held,
        _ => !motion,
    };
    if !covered {
        return Forward::Drop;
    }
    if child.sgr {
        let end = if m.press { 'M' } else { 'm' };
        return Forward::Bytes(format!("\x1b[<{};{x};{y}{end}", m.button).into_bytes());
    }
    // X10: a release names no button, and a cell past 223 cannot be written.
    let b = if m.press { m.button } else { m.button & 0b1_1100 | 3 };
    if x > 223 || y > 223 {
        return Forward::Drop;
    }
    Forward::Bytes(vec![0x1b, b'[', b'M', 32 + b as u8, 32 + x as u8, 32 + y as u8])
}
