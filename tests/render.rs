//! The client's screen: snapshots of every screen and modal, and the hit map they return.

use gensokyo::client::render::{self, Button, Hit, HitMap, Modal, Model, Stage, Summon};
use gensokyo::proto::{Limit, Resident, State, Telemetry};
use gensokyo::vt::{Color, Frame, Run, Style};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

const HOME: &str = "/Users/someone";
const NOW: i64 = 1_790_000_000;

fn resident(n: u8, name: &str, cwd: &str, departed: Option<i64>) -> Resident {
    Resident {
        id: format!("id-{name}"),
        name: name.into(),
        slot: Some(n),
        cwd: format!("{HOME}/{cwd}"),
        pid: departed.is_none().then_some(100 + n as i32),
        departed,
        exit: departed.map(|_| 0),
        signal: None,
        state: if departed.is_some() { State::Departed } else { State::Resting },
        detail: None,
        mode: None,
        branch: None,
        telemetry: None,
    }
}

/// The shrine with what hooks and status lines tell: one busy with a full report, one waiting
/// on a permission, one asking a question.
fn aware() -> Model {
    let mut m = shrine();
    let t = Telemetry {
        model: Some("Haiku 4.5".into()),
        advisor: Some("opus".into()),
        effort: Some("high".into()),
        ctx: Some(12),
        cache: Some(91),
        cost: Some(0.4213),
        five_hour: Some(Limit { used: 37, resets: Some(NOW + 2 * 3600 + 11 * 60) }),
        seven_day: Some(Limit { used: 62, resets: Some(NOW + 3 * 86400 + 4 * 3600) }),
        at: NOW - 5,
        ..Telemetry::default()
    };
    let r = &mut m.residents;
    (r[0].state, r[0].mode, r[0].branch) =
        (State::Busy, Some("acceptEdits".into()), Some("main".into()));
    r[0].telemetry = Some(t.clone());
    r[1].state = State::Awaits;
    r[1].telemetry =
        Some(Telemetry { model: Some("Opus 5.5".into()), ctx: Some(8), at: NOW - 60, ..t });
    let mut asks = resident(4, "Sakuya", "work", None);
    asks.state = State::Asked;
    m.residents.push(asks);
    m
}

/// Rows of text in a few styles, with a gap between runs.
fn frame(cols: u16, rows: u16) -> Frame {
    let st = |fg| Style { fg, ..Style::default() };
    let rows = (0..rows)
        .map(|y| {
            vec![
                Run { col: 0, style: st(Color::Palette(2)), text: format!("line {y:02}") },
                Run { col: 12, style: st(Color::Default), text: "~".repeat((y as usize) % 20) },
            ]
        })
        .collect();
    Frame { cols, rows, cursor: Some((2, 3)) }
}

fn shrine() -> Model {
    let banner = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/share/banner.txt"));
    Model {
        residents: vec![
            resident(1, "Reimu", "dev/gensokyo", None),
            resident(2, "Marisa", "dev/scratch_repo", None),
            resident(3, "Cirno", "tmp", Some(NOW - 300)),
        ],
        focused: Some("id-Reimu".into()),
        screen: Some(frame(95, 38)),
        capture: true,
        banner: banner.unwrap().lines().map(String::from).collect(),
        home: HOME.into(),
        now: NOW,
        ..Model::default()
    }
}

fn summon(stage: Stage) -> Modal {
    Modal::Summon(Summon {
        stage,
        recent: ["dev/gensokyo", "dev/scratch_repo", "work/padlet"]
            .map(|d| format!("{HOME}/{d}"))
            .into(),
        selected: Some(1),
        path: format!("{HOME}/dev/{}", if stage == Stage::Dir { "sc" } else { "scratch_repo" }),
        completions: vec!["scratch_repo/".into(), "scripts/".into()],
        name: "Yuyu".into(),
        error: (stage == Stage::Name).then(|| "Yuyu is already here".into()),
    })
}

/// Every screen worth a snapshot, by name.
fn screens() -> Vec<(&'static str, Model)> {
    let with = |f: &dyn Fn(&mut Model)| {
        let mut m = shrine();
        f(&mut m);
        m
    };
    let departed: Vec<_> =
        shrine().residents.into_iter().filter(|r| r.departed.is_some()).collect();
    vec![
        ("residents", shrine()),
        ("aware", aware()),
        (
            "aware-focused-gold",
            with(&|m| *m = Model { focused: Some("id-Marisa".into()), ..aware() }),
        ),
        ("departed", with(&|m| m.focused = Some("id-Cirno".into()))),
        (
            "empty",
            with(&|m| {
                m.residents.clear();
                m.focused = None;
                m.screen = None;
            }),
        ),
        ("summon-dir", with(&|m| m.modal = Some(summon(Stage::Dir)))),
        ("summon-name", with(&|m| m.modal = Some(summon(Stage::Name)))),
        (
            "banish",
            with(&|m| {
                m.modal = Some(Modal::Banish { id: "id-Reimu".into(), name: "Reimu".into() })
            }),
        ),
        (
            "recall",
            with(&|m| m.modal = Some(Modal::Recall { list: departed.clone(), selected: 0 })),
        ),
        ("quit", with(&|m| m.modal = Some(Modal::Quit))),
        ("help", with(&|m| m.modal = Some(Modal::Help))),
        (
            "capture-off",
            with(&|m| {
                m.capture = false;
                m.message = Some("no resident 7".into());
            }),
        ),
        ("leader", with(&|m| m.leader = true)),
    ]
}

fn draw(m: &Model, w: u16, h: u16) -> (Terminal<TestBackend>, HitMap) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut map = None;
    t.draw(|f| map = Some(render::render(m, f.area(), f.buffer_mut()))).unwrap();
    (t, map.unwrap())
}

fn hits(m: &Model, area: Rect) -> HitMap {
    render::render(m, area, &mut Buffer::empty(area))
}

const AREA: Rect = Rect { x: 0, y: 0, width: 120, height: 40 };

#[test]
fn snapshots() {
    for (name, m) in screens() {
        for (w, h) in [(120, 40), (80, 24)] {
            let (t, _) = draw(&m, w, h);
            insta::assert_snapshot!(format!("{name}-{w}x{h}"), t.backend());
        }
    }
}

/// The centre cell of every hit of this kind resolves to it.
fn resolves(map: &HitMap, want: Hit) -> bool {
    map.0.iter().any(|(r, h)| *h == want && map.at(r.x + r.width / 2, r.y) == Some((*r, *h)))
}

#[test]
fn every_sidebar_button_and_line_resolves() {
    let map = hits(&shrine(), AREA);
    for b in [
        Button::Summon,
        Button::Banish,
        Button::Recall,
        Button::Quit,
        Button::Help,
        Button::Capture,
    ] {
        assert!(resolves(&map, Hit::Button(b)), "{b:?}");
    }
    for i in 0..3 {
        assert!(resolves(&map, Hit::Resident(i)), "resident {i}");
    }
    let g = render::grid_rect(AREA);
    assert_eq!(g, Rect::new(26, 1, 93, 38));
    assert_eq!(map.at(g.x, g.y), Some((g, Hit::Grid)));
    assert_eq!(map.at(g.right() - 1, g.bottom() - 1), Some((g, Hit::Grid)));
}

#[test]
fn chrome_never_resolves_to_the_grid() {
    for (name, m) in screens() {
        let map = hits(&m, AREA);
        let g = render::grid_rect(AREA);
        for y in 0..AREA.height {
            for x in 0..AREA.width {
                let inside = x >= g.x && x < g.right() && y >= g.y && y < g.bottom();
                if !inside {
                    assert_ne!(map.at(x, y).map(|h| h.1), Some(Hit::Grid), "{name} ({x},{y})");
                }
            }
        }
    }
}

#[test]
fn a_modal_covers_the_grid_beneath_it() {
    for (name, m) in screens().into_iter().filter(|(_, m)| m.modal.is_some()) {
        let map = hits(&m, AREA);
        let (r, _) = *map.0.iter().find(|(_, h)| *h == Hit::Modal).expect(name);
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                assert_ne!(map.at(x, y).map(|h| h.1), Some(Hit::Grid), "{name} ({x},{y})");
            }
        }
        assert!(resolves(&map, Hit::Button(Button::No)), "{name}");
        // Outside the modal the grid is still the grid.
        let g = render::grid_rect(AREA);
        assert_eq!(map.at(g.x, g.y).map(|h| h.1), Some(Hit::Grid), "{name}");
    }
}

#[test]
fn modal_items_resolve() {
    let mut m = shrine();
    m.modal = Some(summon(Stage::Dir));
    let map = hits(&m, AREA);
    for i in 0..3 {
        assert!(resolves(&map, Hit::Item(i)), "dir {i}");
    }
    assert!(resolves(&map, Hit::Button(Button::Yes)));
    let list = (0..20).map(|i| resident(1, &format!("R{i}"), "x", Some(NOW - i))).collect();
    m.modal = Some(Modal::Recall { list, selected: 15 });
    let map = hits(&m, AREA);
    // A long list scrolls to keep the selection in view.
    assert!(resolves(&map, Hit::Item(15)));
    assert!(!map.0.iter().any(|(_, h)| *h == Hit::Item(0)));
}

#[test]
fn departed_screen_offers_recall_and_close() {
    let mut m = shrine();
    m.focused = Some("id-Cirno".into());
    let map = hits(&m, AREA);
    assert!(resolves(&map, Hit::Button(Button::RecallFocused)));
    assert!(resolves(&map, Hit::Button(Button::CloseFocused)));
    assert!(!map.0.iter().any(|(_, h)| *h == Hit::Grid));
    assert_eq!(render::cursor(&m, AREA), None);
}

#[test]
fn runs_land_at_their_columns() {
    let mut m = shrine();
    let red = Style { fg: Color::Palette(1), attrs: Style::BOLD, ..Style::default() };
    let rgb = Style {
        bg: Color::Rgb(1, 2, 3),
        attrs: Style::UNDERLINE | Style::INVERSE,
        ..Style::default()
    };
    m.screen = Some(Frame {
        cols: 93,
        rows: vec![vec![
            Run { col: 0, style: Style::default(), text: "ab".into() },
            Run { col: 5, style: red, text: "日x".into() },
            Run { col: 9, style: rgb, text: "z".into() },
        ]],
        cursor: None,
    });
    let mut buf = Buffer::empty(AREA);
    render::render(&m, AREA, &mut buf);
    let g = render::grid_rect(AREA);
    let cell = |dx: u16| &buf[(g.x + dx, g.y)];
    let text: String = (0..10).map(|dx| cell(dx).symbol().to_string()).collect();
    // The cell after a wide character is its second half.
    assert_eq!(text, "ab   日 x z");
    assert_eq!(cell(7).symbol(), "x");
    assert_eq!(cell(5).fg, ratatui::style::Color::Indexed(1));
    assert!(cell(7).modifier.contains(Modifier::BOLD));
    assert_eq!(cell(9).bg, ratatui::style::Color::Rgb(1, 2, 3));
    assert!(cell(9).modifier.contains(Modifier::UNDERLINED | Modifier::REVERSED));
    assert_eq!(render::cursor(&m, AREA), None);
}

#[test]
fn the_cursor_is_the_residents_unless_a_modal_is_open() {
    let mut m = shrine();
    let g = render::grid_rect(AREA);
    assert_eq!(render::cursor(&m, AREA), Some((g.x + 2, g.y + 3)));
    m.modal = Some(Modal::Help);
    assert_eq!(render::cursor(&m, AREA), None);
}

#[test]
fn tiny_screens_do_not_panic() {
    for (_, m) in screens() {
        for (w, h) in [(40, 10), (26, 3), (25, 5), (10, 3), (1, 1), (0, 0), (200, 2)] {
            let area = Rect::new(0, 0, w, h);
            render::render(&m, area, &mut Buffer::empty(area));
            render::cursor(&m, area);
        }
    }
}

#[test]
fn a_selection_reads_in_order_and_trims_each_row() {
    let row = |runs: &[(u16, &str)]| -> Vec<Run> {
        let run = |&(col, t): &(u16, &str)| Run { col, style: Style::default(), text: t.into() };
        runs.iter().map(run).collect()
    };
    let fr = Frame {
        cols: 12,
        rows: vec![
            row(&[(0, "one two    ")]),
            // A wide character takes two cells, a combining mark none, and a gap is blank.
            row(&[(0, "日本 e\u{301}"), (8, "x   ")]),
            row(&[(0, "three       ")]),
        ],
        cursor: None,
    };
    let text = |a, b| render::selected_text(&fr, a, b);
    assert_eq!(text((4, 0), (2, 2)), "two\n日本 e\u{301}  x\nthr");
    // Backwards is the same selection.
    assert_eq!(text((2, 2), (4, 0)), text((4, 0), (2, 2)));
    // Either half of a wide character takes it; the mark goes with its letter.
    assert_eq!(text((1, 1), (2, 1)), "日本");
    assert_eq!(text((5, 1), (5, 1)), "e\u{301}");
    assert_eq!(text((9, 2), (11, 2)), "");
}

#[test]
fn a_selection_is_reversed_in_the_grid_only() {
    let mut m = shrine();
    m.selection = Some(((90, 0), (3, 1)));
    let mut buf = Buffer::empty(AREA);
    render::render(&m, AREA, &mut buf);
    let g = render::grid_rect(AREA);
    let reversed = |x: u16, y: u16| buf[(x, y)].modifier.contains(Modifier::REVERSED);
    let cells: Vec<(u16, u16)> = (0..AREA.height)
        .flat_map(|y| (0..AREA.width).map(move |x| (x, y)))
        .filter(|&(x, y)| reversed(x, y))
        .collect();
    let want: Vec<(u16, u16)> =
        (90..g.width).map(|x| (g.x + x, g.y)).chain((0..4).map(|x| (g.x + x, g.y + 1))).collect();
    assert_eq!(cells, want);
}

#[test]
fn whoever_needs_you_is_gold_across_the_sidebar() {
    let m = aware();
    let mut buf = Buffer::empty(AREA);
    render::render(&m, AREA, &mut buf);
    let gold = |y: u16| {
        (1..render::SIDEBAR_W - 1).all(|x| buf[(x, y)].bg == ratatui::style::Color::Yellow)
    };
    // Rows 1-4 inside the box: Reimu busy, Marisa awaits, Cirno departed, Sakuya asked.
    assert_eq!([1, 2, 3, 4].map(gold), [false, true, false, true]);
}
