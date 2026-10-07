//! The emulator adapter: query answers, keyboard-mode absorption, the key encoder, style runs,
//! and a replay of recorded Claude Code sessions compared with what the real terminal showed.

mod common;

use common::base64;
use std::path::{Path, PathBuf};

use gensokyo::vt::{Color, FOUND, Frame, Key, KeyEvent, Mods, Style, Vt};

fn shift_enter() -> KeyEvent {
    KeyEvent::press(Key::Enter, Mods::SHIFT)
}

fn show(b: &[u8]) -> String {
    b.iter()
        .map(|&c| match c {
            0x1b => "\\e".to_string(),
            0x20..=0x7e => (c as char).to_string(),
            _ => format!("\\x{c:02x}"),
        })
        .collect()
}

#[test]
fn answers_xcpr_in_order_and_across_reads() {
    let mut vt = Vt::new(40, 10);
    vt.feed(b"\x1b[3;7Hab\x1b[?6n\x1b[c");
    assert_eq!(show(&vt.take_replies()), show(b"\x1b[?3;9;1R\x1b[?62;22c"));
    // Split over three reads, with a partial match that falls through in between.
    vt.feed(b"\x1b[5;1H\x1b[");
    vt.feed(b"?");
    vt.feed(b"6");
    assert!(vt.take_replies().is_empty());
    vt.feed(b"n");
    assert_eq!(show(&vt.take_replies()), show(b"\x1b[?5;1;1R"));
    vt.feed(b"\x1b[?\x1b[?6n");
    assert_eq!(show(&vt.take_replies()), show(b"\x1b[?5;1;1R"));
}

#[test]
fn ghostty_answers_the_rest_once() {
    let mut vt = Vt::new(40, 10);
    // CPR, DA1, XTVERSION, kitty flags, OSC 11, DECRQM 2026.
    vt.feed(b"\x1b[2;4H\x1b[6n\x1b[c\x1b[>0q\x1b[?u\x1b]11;?\x1b\\\x1b[?2026$p");
    assert_eq!(
        show(&vt.take_replies()),
        show(b"\x1b[2;4R\x1b[?62;22c\x1bP>|libghostty\x1b\\\x1b[?0u\x1b]11;rgb:0000/0000/0000\x1b\\\x1b[?2026;2$y")
    );
}

#[test]
fn keyboard_modes_are_absorbed() {
    let mut vt = Vt::new(40, 10);
    // What Claude Code sends at startup, twice: pop, kitty 5, modifyOtherKeys 2.
    vt.feed(b"\x1b[<u\x1b[>5u\x1b[>4;2m\x1b[<u\x1b[>5u\x1b[>4;2m");
    // The host gets set to kitty 5 and nothing else; modifyOtherKeys did not downgrade it.
    assert_eq!(vt.kitty_flags(), 5);
    assert_eq!(show(&vt.encode(shift_enter())), "\\e[13;2u");
    assert!(vt.take_replies().is_empty());
    assert!(vt.frame().text().iter().all(|l| l.is_empty()));

    // modifyOtherKeys alone is held here too, and the encoder follows it.
    let mut vt = Vt::new(40, 10);
    vt.feed(b"\x1b[>4;2m");
    assert_eq!(vt.kitty_flags(), 0);
    assert_eq!(show(&vt.encode(shift_enter())), "\\e[27;2;13~");
}

#[test]
fn encoder() {
    let mut vt = Vt::new(40, 10);
    let mut enc = |key, mods, kitty| show(&vt.encode_as(KeyEvent::press(key, mods), kitty, false));
    // Legacy.
    assert_eq!(enc(Key::Enter, Mods::NONE, 0), "\\x0d");
    assert_eq!(enc(Key::Enter, Mods::ALT, 0), "\\e\\x0d");
    assert_eq!(enc(Key::Char('c'), Mods::CTRL, 0), "\\x03");
    assert_eq!(enc(Key::Char('x'), Mods::NONE, 0), "x");
    assert_eq!(enc(Key::Char('X'), Mods::SHIFT, 0), "X");
    assert_eq!(enc(Key::Char('日'), Mods::NONE, 0), "\\xe6\\x97\\xa5");
    assert_eq!(enc(Key::Backspace, Mods::NONE, 0), "\\x7f");
    assert_eq!(enc(Key::Tab, Mods::NONE, 0), "\\x09");
    assert_eq!(enc(Key::Up, Mods::NONE, 0), "\\e[A");
    assert_eq!(enc(Key::Up, Mods::SHIFT, 0), "\\e[1;2A");
    assert_eq!(enc(Key::F(1), Mods::NONE, 0), "\\eOP");
    // Kitty 5, as Claude Code asks.
    assert_eq!(enc(Key::Enter, Mods::NONE, 5), "\\x0d");
    assert_eq!(enc(Key::Enter, Mods::SHIFT, 5), "\\e[13;2u");
    assert_eq!(enc(Key::Enter, Mods::ALT, 5), "\\e[13;3u");
    assert_eq!(enc(Key::Char('c'), Mods::CTRL, 5), "\\e[99;5u");
    assert_eq!(enc(Key::Escape, Mods::NONE, 5), "\\e[27u");
    // What iTerm2 sends for Shift+Enter once Claude Code has turned kitty back off.
    assert_eq!(show(&vt.encode_as(shift_enter(), 0, true)), "\\e[27;2;13~");
}

#[test]
fn style_runs_and_cursor() {
    let mut vt = Vt::new(20, 3);
    vt.feed("ab\x1b[1;31mcd\x1b[0m \x1b[48;2;1;2;3m日\x1b[0m e  \r\n\x1b[4mu\x1b[0m".as_bytes());
    let f = vt.frame();
    let plain = Style::default();
    let bold_red = Style { fg: Color::Palette(1), attrs: Style::BOLD, ..plain };
    let on_rgb = Style { bg: Color::Rgb(1, 2, 3), ..plain };
    let under = Style { attrs: Style::UNDERLINE, ..plain };
    let runs = |r: &[gensokyo::vt::Run]| {
        r.iter().map(|r| (r.col, r.style, r.text.clone())).collect::<Vec<_>>()
    };
    assert_eq!(
        runs(&f.rows[0]),
        [
            (0, plain, "ab".into()),
            (2, bold_red, "cd".into()),
            (4, plain, " ".into()),
            (5, on_rgb, "日".into()),
            (7, plain, " e".into()),
        ]
    );
    assert_eq!(runs(&f.rows[1]), [(0, under, "u".into())]);
    assert!(f.rows[2].is_empty());
    assert_eq!(f.text(), ["abcd 日 e", "u", ""]);
    assert_eq!(f.cursor, Some((1, 1)));
    vt.feed(b"\x1b[?25l");
    assert_eq!(vt.frame().cursor, None);
}

#[test]
fn damage() {
    let mut vt = Vt::new(20, 4);
    vt.feed(b"one\r\ntwo\r\nthree");
    let a = vt.frame();
    assert_eq!(a.damage(&Frame::default()), [0, 1, 2, 3]);
    assert!(vt.frame().damage(&a).is_empty());
    vt.feed(b"\x1b[2;1H\x1b[1mtwo");
    let b = vt.frame();
    assert_eq!(b.damage(&a), [1]);
    vt.resize(20, 5);
    assert_eq!(vt.frame().damage(&b), [0, 1, 2, 3, 4]);
}

#[test]
fn odd_sizes_and_modes_do_not_panic() {
    let mut vt = Vt::new(0, 0);
    assert_eq!((vt.frame().cols, vt.frame().rows.len()), (1, 1));
    vt.resize(0, 10);
    assert_eq!((vt.frame().cols, vt.frame().rows.len()), (1, 10));
    assert!(!vt.mode(12345));
}

// Replay of recorded sessions.

enum Event {
    Start { cols: u16, rows: u16 },
    Out(Vec<u8>),
    In(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Mark(String),
    Other,
}

#[test]
fn the_live_screen_is_read_wherever_the_view_is_and_the_view_stays() {
    let mut vt = Vt::new(20, 5);
    for i in 0..30 {
        vt.feed(format!("line {i}\r\n").as_bytes());
    }
    let live = vt.live_text();
    vt.scroll(Some(-12));
    let shown = vt.frame().text();
    assert_eq!(vt.live_text(), live);
    assert_eq!((vt.scrolled().0, vt.frame().text()), (12, shown));
    assert_eq!(live[3], "line 29");
}

#[test]
fn scrollback_holds_its_place_while_output_comes_and_clamps_at_both_ends() {
    let mut vt = Vt::new(20, 5);
    for i in 0..30 {
        vt.feed(format!("line {i}\r\n").as_bytes());
    }
    let top = |vt: &mut Vt| vt.frame().text()[0].clone();
    let f = vt.frame();
    assert_eq!((f.back, f.history, f.cursor), (0, 26, Some((0, 4))));
    vt.scroll(Some(-10));
    let f = vt.frame();
    // Scrolled back, the cursor would be drawn over history.
    assert_eq!((f.back, f.history, f.cursor, top(&mut vt).as_str()), (10, 26, None, "line 16"));
    // Output while reading history leaves the view on the same rows.
    for i in 30..33 {
        vt.feed(format!("line {i}\r\n").as_bytes());
    }
    assert_eq!((vt.scrolled(), top(&mut vt).as_str()), ((13, 29), "line 16"));
    vt.scroll(Some(-1000));
    assert_eq!((vt.scrolled(), top(&mut vt).as_str()), ((29, 29), "line 0"));
    vt.scroll(Some(1000));
    assert_eq!(vt.scrolled(), (0, 29));
    // Past the bottom is live again: it follows the output.
    vt.feed(b"line 33\r\n");
    assert_eq!((vt.scrolled().0, top(&mut vt).as_str()), (0, "line 30"));
    // A resize keeps the view as far back as it was, as a new viewer's nudge of a row does.
    vt.scroll(Some(-5));
    vt.resize(20, 4);
    assert_eq!(vt.scrolled().0, 5);
    vt.resize(20, 5);
    assert_eq!(vt.scrolled().0, 5);
    vt.resize(30, 8);
    vt.resize(10, 3);
    assert_eq!(vt.scrolled().0, 5);
    // The ends are reached by asking for more than there is, however much.
    let all = vt.scrolled().1;
    for (n, back) in [(i32::MIN, all), (i32::MAX, 0), (-(1 << 30), all)] {
        vt.scroll(Some(n));
        assert_eq!(vt.scrolled().0, back, "{n}");
    }
    vt.scroll(None);
    let f = vt.frame();
    assert_eq!(f.back, 0);
    assert!(f.cursor.is_some());
}

#[test]
fn the_alternate_screen_has_no_scrollback_to_move_through() {
    let mut vt = Vt::new(20, 5);
    for i in 0..30 {
        vt.feed(format!("line {i}\r\n").as_bytes());
    }
    vt.feed(b"\x1b[?1049h\x1b[Hfull screen");
    assert_eq!(vt.scrolled(), (0, 0));
    vt.scroll(Some(-5));
    let f = vt.frame();
    assert_eq!((f.back, f.history, f.text()[0].as_str()), (0, 0, "full screen"));
    assert!(f.cursor.is_some());
    vt.feed(b"\x1b[?1049l");
    assert_eq!(vt.scrolled(), (0, 26));
}

/// What a search found, as drawn: row on screen, first column, text.
fn found(f: &Frame) -> Vec<(usize, u16, String)> {
    let runs = f.rows.iter().enumerate().flat_map(|(y, r)| r.iter().map(move |r| (y, r)));
    runs.filter(|(_, r)| r.style == FOUND).map(|(y, r)| (y, r.col, r.text.clone())).collect()
}

#[test]
fn the_scrollback_is_searched_back_and_on_and_what_was_found_is_drawn() {
    let mut vt = Vt::new(20, 5);
    for i in 0..30 {
        let s = match i % 10 {
            3 if i == 13 => format!("line {i} Needle\r\n"),
            3 => format!("line {i} needle\r\n"),
            _ => format!("line {i}\r\n"),
        };
        vt.feed(s.as_bytes());
    }
    // Back from the live screen: the newest first, a third of the way down the view.
    assert!(vt.find("needle", true));
    let f = vt.frame();
    assert_eq!(found(&f), [(1, 8, "needle".into())]);
    assert_eq!((f.text()[1].as_str(), f.back), ("line 23 needle", 4));
    // Lower case finds either case; a capital only its own.
    assert!(vt.find("needle", true));
    assert_eq!(vt.frame().text()[1], "line 13 Needle");
    assert!(vt.find("needle", true));
    assert_eq!(vt.frame().text()[1], "line 3 needle");
    // Nothing older: what was found stays, and so does the view.
    assert!(!vt.find("needle", true));
    let f = vt.frame();
    assert_eq!(
        (found(&f), f.text()[1].as_str()),
        ([(1, 7, "needle".into())].to_vec(), "line 3 needle")
    );
    assert!(vt.find("Needle", false));
    assert_eq!(vt.frame().text()[1], "line 13 Needle");
    assert!(!vt.find("Needle", false));
    assert!(!vt.find("Needle", true));
    // Output while it shows leaves it on its text.
    for i in 30..40 {
        vt.feed(format!("line {i}\r\n").as_bytes());
    }
    let f = vt.frame();
    assert_eq!(
        (found(&f), f.text()[1].as_str()),
        ([(1, 8, "Needle".into())].to_vec(), "line 13 Needle")
    );
    // Scrolled away from it, a search starts from the view instead.
    vt.scroll(Some(-1000));
    assert!(found(&vt.frame()).is_empty());
    assert!(vt.find("needle", false));
    assert_eq!(vt.frame().text()[3], "line 3 needle");
    // Home again, it is let go.
    vt.scroll(None);
    assert!(vt.find("line 39", true));
    let f = vt.frame();
    assert_eq!((found(&f), f.back), ([(3, 0, "line 39".into())].to_vec(), 0));
    vt.scroll(None);
    assert!(found(&vt.frame()).is_empty());
}

#[test]
fn a_search_goes_through_a_row_match_by_match_and_counts_wide_cells() {
    let mut vt = Vt::new(30, 4);
    vt.feed("two \u{4e2d} ab ab ab \u{4e2d}ab\r\n".as_bytes());
    for _ in 0..6 {
        vt.feed(b"filler\r\n");
    }
    let cols = |vt: &mut Vt| found(&vt.frame()).iter().map(|f| f.1).collect::<Vec<_>>();
    let mut seen = Vec::new();
    while vt.find("ab", true) {
        seen.extend(cols(&mut vt));
    }
    // The wide character before them takes two columns each.
    assert_eq!(seen, [18, 13, 10, 7]);
    assert!(vt.find("\u{4e2d}ab", false));
    assert_eq!(found(&vt.frame()), [(0, 16, "\u{4e2d}ab".into())]);
}

#[test]
fn what_was_found_keeps_to_its_text_when_the_oldest_rows_go() {
    let mut vt = Vt::new(200, 10);
    let pad = "x".repeat(150);
    let feed = |vt: &mut Vt, from: usize, to: usize| {
        let mut b = String::new();
        for i in from..to {
            let mark = if i % 1000 == 500 { " needle" } else { "" };
            b.push_str(&format!("line {i}{mark} {pad}\r\n"));
        }
        vt.feed(b.as_bytes());
    };
    feed(&mut vt, 0, 7000);
    assert!(vt.find("needle", true));
    let at = |vt: &mut Vt| {
        let f = vt.frame();
        let y = found(&f).first().map(|f| f.0);
        y.map(|y| f.text()[y].split(' ').take(2).collect::<Vec<_>>().join(" "))
    };
    assert_eq!(at(&mut vt).as_deref(), Some("line 6500"));
    let kept = vt.scrolled().1;
    // Enough more that the oldest rows go: every row's place from the top moves.
    feed(&mut vt, 7000, 10000);
    assert!(vt.scrolled().1 < kept + 1000, "the scrollback is full: old rows went");
    assert_eq!(at(&mut vt).as_deref(), Some("line 6500"));
    assert!(vt.find("needle", true));
    assert_eq!(at(&mut vt).as_deref(), Some("line 5500"));
}

#[test]
fn the_alternate_screen_is_not_searched() {
    let mut vt = Vt::new(20, 5);
    for i in 0..30 {
        vt.feed(format!("line {i}\r\n").as_bytes());
    }
    assert!(vt.find("line 2", true));
    vt.feed(b"\x1b[?1049h\x1b[Hline 2 full screen");
    assert!(!vt.find("line 2", true));
    assert!(found(&vt.frame()).is_empty());
    vt.feed(b"\x1b[?1049l");
    assert!(!found(&vt.frame()).is_empty());
}

#[test]
fn scrollback_is_measured_in_bytes_and_keeps_thousands_of_rows() {
    let mut vt = Vt::new(80, 24);
    let line = format!("{}\r\n", "x".repeat(76));
    vt.feed(line.repeat(3000).as_bytes());
    // 10_000 taken as lines kept 861 rows here; 10 MB keeps about 13,800.
    assert_eq!(vt.scrolled().1, 3000 - 24 + 1);
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// A recording: one JSON object per line, byte payloads base64 in `b`.
fn read(path: &Path) -> Vec<Event> {
    let text = std::fs::read_to_string(path).unwrap();
    let num = |v: &serde_json::Value, k: &str| v[k].as_u64().unwrap() as u16;
    let bytes = |v: &serde_json::Value| base64(v["b"].as_str().unwrap());
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            match v["ev"].as_str().unwrap() {
                "start" => Event::Start { cols: num(&v, "cols"), rows: num(&v, "rows") },
                "out" => Event::Out(bytes(&v)),
                "in" => Event::In(bytes(&v)),
                "resize" => Event::Resize { cols: num(&v, "cols"), rows: num(&v, "rows") },
                "mark" => Event::Mark(v["name"].as_str().unwrap().into()),
                _ => Event::Other,
            }
        })
        .collect()
}

/// What the terminal showed at a mark: the last `rows` lines of the saved screen. iTerm2 ends
/// every row with a newline and the capture adds one more, so trailing empty lines go first.
fn ground_truth(path: &Path, rows: usize) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut lines: Vec<&str> = text.split('\n').collect();
    while lines.last() == Some(&"") {
        lines.pop();
    }
    let mut lines: Vec<String> = lines.iter().map(|l| l.trim_end().to_string()).collect();
    let mut v = lines.split_off(lines.len().saturating_sub(rows));
    v.resize(rows, String::new());
    v
}

/// DECXCPR answers (`CSI ? r ; c ; p R`) in a byte stream.
fn xcpr_answers(b: &[u8]) -> Vec<String> {
    let s = String::from_utf8_lossy(b);
    s.split("\x1b[?")
        .skip(1)
        .filter_map(|t| {
            let end = t.find(|c: char| !c.is_ascii_digit() && c != ';')?;
            (t[end..].starts_with('R')).then(|| format!("?{}R", &t[..end]))
        })
        .collect()
}

#[test]
fn replay_matches_the_terminal_at_every_mark() {
    let mut names: Vec<String> = std::fs::read_dir(fixtures())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| (n.starts_with("a1-") || n.starts_with("a3-")) && n.ends_with(".jsonl"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 7, "{names:?}");

    let (mut marks, mut with_mok) = (0, 0);
    let mut bad = Vec::new();
    for name in &names {
        let path = fixtures().join(name);
        let evs = read(&path);
        let Some(&Event::Start { cols, rows }) = evs.first() else { panic!("{name}: no start") };
        let (mut vt, mut rows) = (Vt::new(cols, rows), rows);
        let (mut ours, mut theirs, mut last_key) = (Vec::new(), Vec::new(), Vec::new());
        let mut sent_mok = false;
        for ev in &evs {
            match ev {
                Event::Out(b) => {
                    sent_mok |= b.windows(7).any(|w| w == b"\x1b[>4;2m");
                    vt.feed(b);
                    let r = vt.take_replies();
                    assert!(
                        !r.windows(3).any(|w| w == b"\x1b[>"),
                        "{name}: a keyboard mode in the replies"
                    );
                    ours.extend(xcpr_answers(&r));
                }
                Event::In(b) => {
                    theirs.extend(xcpr_answers(b));
                    // Focus reports from clicking into the window are not keys.
                    if b.as_slice() != b"\x1b[I" && b.as_slice() != b"\x1b[O" {
                        last_key = b.clone();
                    }
                }
                Event::Resize { cols, rows: r } => {
                    vt.resize(*cols, *r);
                    rows = *r;
                    // Every recording is back at its first size by the next mark.
                    let f = vt.frame();
                    assert_eq!((f.cols, f.rows.len()), (*cols, rows as usize), "{name}: resize");
                }
                Event::Mark(mark) => {
                    marks += 1;
                    let frame = vt.frame();
                    let got = frame.text();
                    assert!(
                        got.iter().all(|l| !l.contains('\x1b')),
                        "{name}: an escape in the frame"
                    );
                    let mut dir = path.clone().into_os_string();
                    dir.push(".marks");
                    let want =
                        ground_truth(&Path::new(&dir).join(format!("{mark}.txt")), rows as usize);
                    for y in (0..rows as usize).filter(|&y| got.get(y) != want.get(y)) {
                        bad.push(format!(
                            "{name} {mark} row {y}\n  vt   |{}|\n  term |{}|",
                            got[y], want[y]
                        ));
                    }
                    if mark == "shift-enter" && name == "a1-iterm2.jsonl" {
                        // iTerm2 honoured Claude Code's modifyOtherKeys and not its kitty push;
                        // this emulator honours both, and kitty wins.
                        assert_eq!(show(&last_key), "\\e[27;2;13~");
                        assert_eq!(show(&vt.encode_as(shift_enter(), 0, true)), show(&last_key));
                        assert_eq!(vt.kitty_flags(), 5);
                        assert_eq!(show(&vt.encode(shift_enter())), "\\e[13;2u");
                    }
                }
                Event::Start { .. } | Event::Other => {}
            }
        }
        if sent_mok {
            with_mok += 1;
        }
        if name == "a1-iterm2.jsonl" {
            // The only recording whose terminal answered DECXCPR. Ours must give the same cursor.
            assert!(!theirs.is_empty());
            assert_eq!(ours, theirs, "{name}: DECXCPR answers");
        }
    }
    assert!(bad.is_empty(), "{} rows differ:\n{}", bad.len(), bad.join("\n"));
    assert_eq!(marks, 20);
    // So the replies were checked against Claude Code's modifyOtherKeys request at all.
    assert!(with_mok >= 3, "{with_mok} recordings send modifyOtherKeys");
}
