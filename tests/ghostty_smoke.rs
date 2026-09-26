//! Runtime check of the libghostty-vt binding against the vendored Ghostty source.
//!
//! The crate's bindings are checked in, not generated, so a Ghostty C API change links cleanly
//! and only misbehaves at runtime (a struct passed by value lands in the wrong registers and
//! `Terminal::new` rejects the size, or gets a garbage one). Every call the emulator relies on is
//! exercised here once.

use std::cell::RefCell;
use std::rc::Rc;

use libghostty_vt::key::{self, Encoder, Mods};
use libghostty_vt::render::{CellIterator, RowIterator};
use libghostty_vt::screen::CellWide;
use libghostty_vt::style::RgbColor;
use libghostty_vt::terminal::{
    ColorScheme, ConformanceLevel, DeviceAttributeFeature, DeviceAttributes, DeviceType, Mode,
    ModeKind, Options, PrimaryDeviceAttributes, SecondaryDeviceAttributes,
    TertiaryDeviceAttributes,
};
use libghostty_vt::{RenderState, Terminal};

fn terminal(cols: u16, rows: u16) -> (Terminal<'static, 'static>, Rc<RefCell<Vec<u8>>>) {
    let mut term = Terminal::new(Options { cols, rows, max_scrollback: 10_000 }).expect("terminal");
    let replies = Rc::new(RefCell::new(Vec::new()));
    let r = replies.clone();
    term.on_pty_write(move |_, b| r.borrow_mut().extend_from_slice(b)).unwrap();
    // Without this callback DA1/DA2 go unanswered.
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
    term.set_default_fg_color(Some(RgbColor { r: 0xc7, g: 0xc7, b: 0xc7 })).unwrap();
    term.set_default_bg_color(Some(RgbColor { r: 0, g: 0, b: 0 })).unwrap();
    (term, replies)
}

/// The visible screen, one string per row, trailing blanks trimmed, wide-char spacers skipped.
fn screen(term: &Terminal<'static, 'static>) -> Vec<String> {
    let mut render = RenderState::new().unwrap();
    let mut rows = RowIterator::new().unwrap();
    let mut cells = CellIterator::new().unwrap();
    let snap = render.update(term).unwrap();
    let mut out = Vec::new();
    let mut rows = rows.update(&snap).unwrap();
    while let Some(row) = rows.next() {
        let mut line = String::new();
        let mut it = cells.update(row).unwrap();
        while let Some(cell) = it.next() {
            let wide = cell.raw_cell().unwrap().wide().unwrap();
            if matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead) {
                continue;
            }
            let g: String = cell.graphemes().unwrap().into_iter().collect();
            line.push_str(if g.is_empty() { " " } else { &g });
        }
        out.push(line.trim_end().to_string());
    }
    out
}

fn mode(term: &Terminal<'static, 'static>, n: u16) -> bool {
    term.mode(Mode::new(n, ModeKind::Dec)).unwrap()
}

fn shift_enter(term: &Terminal<'static, 'static>) -> Vec<u8> {
    let mut enc = Encoder::new().unwrap();
    enc.set_options_from_terminal(term);
    enc.set_macos_option_as_alt(key::OptionAsAlt::True);
    let mut ev = key::Event::new().unwrap();
    ev.set_action(key::Action::Press)
        .set_key(key::Key::Enter)
        .set_mods(Mods::SHIFT)
        .set_utf8(Some("\r"));
    let mut out = Vec::new();
    enc.encode_to_vec(&ev, &mut out).unwrap();
    out
}

#[test]
fn grid_modes_encoder_and_replies() {
    let (mut term, replies) = terminal(20, 5);
    term.vt_write("ab\x1b[1mc\x1b[0m\r\n日本\x1b[?2026h\x1b[>1u\x1b[?2004h".as_bytes());

    assert_eq!(screen(&term), ["abc", "日本", "", "", ""]);
    assert_eq!(term.kitty_keyboard_flags().unwrap().bits(), 1);
    assert!(mode(&term, 2026));
    assert!(mode(&term, 2004));
    assert_eq!(shift_enter(&term), b"\x1b[13;2u");

    replies.borrow_mut().clear();
    // DA1, XTVERSION, kitty flags query, OSC 11 background query, DECRQM 2026.
    term.vt_write(b"\x1b[c\x1b[>q\x1b[?u\x1b]11;?\x1b\\\x1b[?2026$p");
    assert_eq!(
        String::from_utf8_lossy(&replies.borrow()),
        "\x1b[?62;22c\x1bP>|libghostty\x1b\\\x1b[?1u\x1b]11;rgb:0000/0000/0000\x1b\\\x1b[?2026;1$y"
    );
}

#[test]
fn new_and_resize_keep_their_size() {
    let (mut term, _) = terminal(20, 5);
    term.vt_write(&[b'x'; 21]);
    assert_eq!(screen(&term), ["x".repeat(20), "x".into(), "".into(), "".into(), "".into()]);
    term.resize(30, 7, 0, 0).unwrap();
    term.vt_write(b"\x1b[H\x1b[2J");
    term.vt_write(&[b'y'; 31]);
    let s = screen(&term);
    assert_eq!(s.len(), 7);
    assert_eq!((s[0].as_str(), s[1].as_str()), ("y".repeat(30).as_str(), "y"));
}
