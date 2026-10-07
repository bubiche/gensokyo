//! Host input to resident input: raw when host and child agree, re-encoded by the child's own
//! encoder when they don't, releases dropped, the leader chord, the host's kitty bytes, and
//! mouse reports for the grid.

use gensokyo::client::framer::{Chunk, Framer, Key, Mouse};
use gensokyo::client::keys::{self, Chord, Forward, LEADER, Scrollback};
use gensokyo::vt::{KeyEvent, Modes, Vt};

/// The chunks one host read gives, with any ESC held back released.
fn chunks(b: &[u8]) -> Vec<Chunk> {
    let mut f = Framer::new();
    let mut out = f.feed(b, 0.0);
    out.extend(f.tick(10_000.0));
    out
}

fn one(b: &[u8]) -> Chunk {
    let mut c = chunks(b);
    assert_eq!(c.len(), 1, "{c:?}");
    c.remove(0)
}

fn kitty(flags: u8) -> Modes {
    Modes { kitty: flags, ..Modes::default() }
}

fn fwd(b: &[u8], host: u8, child: u8) -> Forward {
    keys::forward(&one(b), host, &kitty(child))
}

fn raw(b: &[u8]) -> Forward {
    Forward::Bytes(b.to_vec())
}

fn key(code: u32, mods: u8, event: u8) -> Forward {
    Forward::Key(Key { code, mods, event })
}

/// What the daemon writes for a `Forward::Key`: the child's encoder at kitty `flags`.
fn encoded(f: Forward, flags: u8) -> Vec<u8> {
    let Forward::Key(k) = f else { panic!("not a key: {f:?}") };
    let mut vt = Vt::new(80, 24);
    vt.feed(format!("\x1b[>{flags}u").as_bytes());
    vt.encode(KeyEvent::from_kitty(k.code, k.mods, k.event).unwrap())
}

#[test]
fn agreeing_flags_forward_raw() {
    // iTerm2 set to Claude's 5.
    assert_eq!(fwd(b"\x1b[13;2u", 5, 5), raw(b"\x1b[13;2u"));
    assert_eq!(fwd(b"\x1b[99;5u", 5, 5), raw(b"\x1b[99;5u"));
    assert_eq!(fwd(b"\r", 0, 0), raw(b"\r"));
    assert_eq!(fwd(b"\x1b[13;2:3u", 7, 7), raw(b"\x1b[13;2:3u"));
}

#[test]
fn legacy_host_to_kitty_child_re_encodes() {
    // Terminal.app with Option as Meta: Option+Enter is ESC CR, Alt+Enter to Claude.
    let f = fwd(b"\x1b\r", 0, 5);
    assert_eq!(f, key(13, 2, 1));
    assert_eq!(encoded(f, 5), b"\x1b[13;3u");
    assert_eq!(encoded(fwd(b"\x03", 0, 5), 5), b"\x1b[99;5u");
    assert_eq!(encoded(fwd(b"\x1bx", 0, 5), 5), b"\x1b[120;3u");
    // Enter, Tab and Backspace mean the same either way.
    for b in [&b"\r"[..], b"\t", b"\x7f"] {
        assert_eq!(fwd(b, 0, 5), raw(b));
    }
    // Keys the framer leaves unparsed go as they came.
    assert_eq!(fwd(b"\x1b[A", 0, 5), raw(b"\x1b[A"));
    assert_eq!(fwd(b"\x1bOP", 0, 5), raw(b"\x1bOP"));
}

#[test]
fn kitty_host_to_other_child_re_encodes() {
    // Host at 7 (reports events), child at 5.
    assert_eq!(encoded(fwd(b"\x1b[13;2u", 7, 5), 5), b"\x1b[13;2u");
    assert_eq!(fwd(b"\x1b[13;2:3u", 7, 5), Forward::Drop);
    assert_eq!(fwd(b"\x1b[120;1:3u", 7, 5), Forward::Drop, "a text key's release");
    assert_eq!(fwd(b"\x1b[13;2:2u", 7, 5), key(13, 1, 1), "a repeat is a press");
    // To a legacy child (one that never pushed flags, or not yet).
    assert_eq!(fwd(b"\x1b[99;5u", 5, 0), key(99, 4, 1));
    assert_eq!(encoded(fwd(b"\x1b[99;5u", 5, 0), 0), b"\x03");
    assert_eq!(fwd(b"\x1b[99;69u", 5, 0), key(99, 4 | 64, 1), "caps lock rides along");
    // A child that asked for events keeps releases and repeats.
    assert_eq!(fwd(b"\x1b[13;1:3u", 7, 3), key(13, 0, 3));
    assert_eq!(fwd(b"\x1b[13;1:2u", 7, 3), key(13, 0, 2));
}

#[test]
fn all_as_escapes_child_gets_enter_encoded() {
    assert_eq!(fwd(b"\r", 0, 8), key(13, 0, 1));
    assert_eq!(encoded(fwd(b"\r", 0, 8), 8), b"\x1b[13u");
}

#[test]
fn text_paste_and_focus() {
    let child = Modes { paste: true, focus: true, ..kitty(5) };
    let plain = kitty(5);
    assert_eq!(keys::forward(&Chunk::Text("héllo".into()), 0, &plain), raw("héllo".as_bytes()));
    let p = Chunk::Paste(b"a\rb".to_vec());
    assert_eq!(keys::forward(&p, 5, &child), raw(b"\x1b[200~a\rb\x1b[201~"));
    assert_eq!(keys::forward(&p, 5, &plain), raw(b"a\rb"));
    let focus = one(b"\x1b[I");
    assert_eq!(keys::forward(&focus, 5, &child), raw(b"\x1b[I"));
    assert_eq!(keys::forward(&focus, 5, &plain), Forward::Drop);
    let rejected = Chunk::PasteRejected { len: 9, why: "oversized" };
    assert_eq!(keys::forward(&rejected, 5, &child), Forward::Drop);
    assert_eq!(keys::forward(&one(b"\x1b[?5u"), 5, &child), Forward::Drop, "a reply");
}

#[test]
fn leader_is_ctrl_bracket_however_encoded() {
    assert!(keys::is_leader(&one(b"\x1d")));
    assert!(keys::is_leader(&one(b"\x1b[93;5u")));
    assert!(keys::is_leader(&one(b"\x1b[93;69u")), "caps lock");
    assert!(!keys::is_leader(&one(b"\x1b[93;5:3u")), "its release");
    assert!(!keys::is_leader(&one(b"\x1b\x1d")), "Ctrl-Alt-]");
    assert!(!keys::is_leader(&one(b"]")));
    // The leader twice sends one, encoded for the child.
    assert_eq!(encoded(Forward::Key(LEADER), 5), b"\x1b[93;5u");
    assert_eq!(encoded(Forward::Key(LEADER), 0), b"\x1d");
}

#[test]
fn chord_after_the_leader() {
    let c = |b: &[u8]| keys::chord(&one(b));
    let text = |t: &str| keys::chord(&Chunk::Text(t.into()));
    for (t, want) in [
        ("n", Chord::Summon),
        ("b", Chord::Banish),
        ("r", Chord::Recall),
        ("c", Chord::Cast),
        ("t", Chord::Timetable),
        ("q", Chord::Quit),
        ("?", Chord::Help),
        ("m", Chord::Capture),
        ("d", Chord::Detach),
        ("x", Chord::Close),
        ("[", Chord::ScrollBack),
        ("/", Chord::Find),
        ("y", Chord::Copy),
        ("j", Chord::Next),
        ("k", Chord::Prev),
        ("a", Chord::Awaiting),
        ("1", Chord::Focus(1)),
        ("9", Chord::Focus(9)),
        ("0", Chord::Unbound),
        ("z", Chord::Unbound),
        ("nb", Chord::Unbound),
    ] {
        assert_eq!(text(t), Some(want), "{t}");
    }
    // Kitty's all-as-escapes form of the same keys.
    assert_eq!(c(b"\x1b[110u"), Some(Chord::Summon));
    assert_eq!(c(b"\x1b[51u"), Some(Chord::Focus(3)));
    assert_eq!(c(b"\x1b[47;2u"), Some(Chord::Help));
    assert_eq!(c(b"\x1b[110;5u"), Some(Chord::Unbound), "Ctrl-n");
    assert_eq!(c(b"\x1d"), Some(Chord::Leader));
    assert_eq!(c(b"\x1b[93;5u"), Some(Chord::Leader));
    assert_eq!(c(b"\x1b"), Some(Chord::Cancel));
    assert_eq!(c(b"\x1b[27u"), Some(Chord::Cancel));
    assert_eq!(c(b"\x1b[A"), Some(Chord::Unbound));
    // Nothing that ends the wait: the leader's own release, focus, a reply.
    assert_eq!(c(b"\x1b[93;5:3u"), None);
    assert_eq!(c(b"\x1b[I"), None);
    assert_eq!(c(b"\x1b[?5u"), None);
}

#[test]
fn host_kitty_bytes() {
    assert_eq!(keys::kitty_push(5), b"\x1b[>5u");
    assert_eq!(keys::kitty_set(0), b"\x1b[=0;1u");
    assert_eq!(keys::KITTY_QUERY, b"\x1b[?u");
    assert_eq!(keys::KITTY_POP, b"\x1b[<u");
}

fn m(button: u16, press: bool) -> Mouse {
    Mouse { button, col: 0, row: 0, press }
}

#[test]
fn mouse_follows_the_child_mode() {
    let sgr = |mouse| Modes { mouse, sgr: true, ..Modes::default() };
    let at = |ev: Mouse, modes: Modes| keys::mouse(&ev, 6, 9, &modes);
    assert_eq!(at(m(0, true), sgr(1000)), raw(b"\x1b[<0;6;9M"));
    assert_eq!(at(m(0, false), sgr(1000)), raw(b"\x1b[<0;6;9m"));
    assert_eq!(at(m(65, true), sgr(1000)), raw(b"\x1b[<65;6;9M"), "wheel");
    // Motion: a drag (button held) from 1002, any motion only from 1003.
    assert_eq!(at(m(32, true), sgr(1000)), Forward::Drop);
    assert_eq!(at(m(32, true), sgr(1002)), raw(b"\x1b[<32;6;9M"));
    assert_eq!(at(m(35, true), sgr(1002)), Forward::Drop);
    assert_eq!(at(m(35, true), sgr(1003)), raw(b"\x1b[<35;6;9M"));
    // X10 encoding without 1006: a release names no button.
    let x10 = Modes { mouse: 1000, ..Modes::default() };
    assert_eq!(at(m(0, true), x10.clone()), raw(b"\x1b[M &)"));
    assert_eq!(at(m(16, false), x10.clone()), raw(b"\x1b[M3&)"), "Ctrl kept on release");
    assert_eq!(keys::mouse(&m(0, true), 224, 1, &x10), Forward::Drop);
    assert_eq!(keys::mouse(&m(0, true), 223, 1, &x10), raw(&[0x1b, b'[', b'M', 32, 255, 33]));
}

#[test]
fn wheel_without_child_mouse_scrolls() {
    let alt = Modes { alt: true, ..Modes::default() };
    let main = Modes::default();
    assert_eq!(keys::mouse(&m(64, true), 1, 1, &alt), raw(b"\x1b[A\x1b[A\x1b[A"));
    assert_eq!(keys::mouse(&m(65, true), 1, 1, &alt), raw(b"\x1b[B\x1b[B\x1b[B"));
    assert_eq!(keys::mouse(&m(64, true), 1, 1, &main), Forward::Scroll(-3));
    assert_eq!(keys::mouse(&m(65, true), 1, 1, &main), Forward::Scroll(3));
    assert_eq!(keys::mouse(&m(0, true), 1, 1, &main), Forward::Drop, "a click");
}

#[test]
fn keys_that_move_through_the_scrollback() {
    use Scrollback::*;
    let s = |b: &[u8]| keys::scrollback(&one(b));
    for (b, want) in [
        (&b"k"[..], By(-1)),
        (b"j", By(1)),
        (b"\x1b[A", By(-1)),
        (b"\x1b[B", By(1)),
        (b"\x1b[5~", Pages(-1)),
        (b"\x1b[6~", Pages(1)),
        (b"b", Pages(-1)),
        (b" ", Pages(1)),
        (b"g", Top),
        (b"\x1b[H", Top),
        (b"G", Live),
        (b"\x1b[F", Live),
        (b"q", Live),
        (b"\x1b", Live),
        (b"\x1b[27u", Live),
        // Kitty's all-as-escapes form of a letter, and its release.
        (b"\x1b[107u", By(-1)),
        (b"\x1b[107;1:3u", Stay),
        // An arrow's release, which the framer leaves unparsed.
        (b"\x1b[1;1:3A", Stay),
        (b"/", Find(true)),
        (b"?", Find(false)),
        (b"n", Again(false)),
        (b"N", Again(true)),
        // Shifted, in kitty's all-as-escapes form: Shift and `/`, Shift and a letter.
        (b"\x1b[47;2u", Find(false)),
        (b"\x1b[110;2u", Again(true)),
        (b"\x1b[103;2u", Live),
    ] {
        assert_eq!(s(b), Some(want), "{b:?}");
    }
    // Anything else is the resident's: typing, Enter, a chord, a paste.
    for b in [&b"x"[..], b"\r", b"\x1b[107;5u", b"\x1b[200~k\x1b[201~", b"hello"] {
        assert_eq!(s(b), None, "{b:?}");
    }
}
