//! The client's App without a terminal: host bytes and daemon lines in, the requests it queues
//! for the socket, the bytes it has for the host, and its model out.

use gensokyo::client::app::{App, Config};
use gensokyo::client::framer::Framer;
use gensokyo::client::modal::Modal;
use gensokyo::proto::{Reply, Resident, RitualInfo, State};
use gensokyo::vt::{Frame, Modes, Run, Style};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::Value;

fn app() -> App {
    App::new(Config {
        home: "/Users/someone".into(),
        banner: Vec::new(),
        bell: true,
        desktop: true,
        log: None,
        copy: "true".into(),
    })
}

/// Host bytes as one read, with any ESC held back released.
fn host(a: &mut App, b: &[u8]) {
    let mut f = Framer::new();
    let mut cs = f.feed(b, 0.0);
    cs.extend(f.tick(10_000.0));
    for c in cs {
        a.chunk(c);
    }
}

fn daemon(a: &mut App, r: Reply) {
    a.reply(&serde_json::to_string(&r).unwrap());
}

/// The requests queued since the last call.
fn sent(a: &mut App) -> Vec<Value> {
    a.out.drain(..).map(|l| serde_json::from_slice(&l).unwrap()).collect()
}

fn kinds(v: &[Value]) -> Vec<&str> {
    v.iter().map(|r| r["t"].as_str().unwrap()).collect()
}

fn resident(slot: u8, name: &str) -> Resident {
    Resident {
        id: format!("id-{name}"),
        name: name.into(),
        slot: Some(slot),
        cwd: "/Users/someone/dev".into(),
        pid: Some(100 + slot as i32),
        departed: None,
        exit: None,
        signal: None,
        state: State::Resting,
        detail: None,
        mode: None,
        branch: None,
        telemetry: None,
    }
}

/// Reimu and Marisa in the shrine, Reimu on screen with her first frame.
fn shrine() -> App {
    let mut a = app();
    daemon(
        &mut a,
        Reply::Residents { residents: vec![resident(1, "Reimu"), resident(2, "Marisa")] },
    );
    let rows = vec![vec![Run { col: 0, style: Style::default(), text: "hello".into() }]];
    let frame = Frame { cols: 80, rows, cursor: None, ..Frame::default() };
    daemon(&mut a, Reply::Frame { who: "id-Reimu".into(), rev: 1, frame, modes: Modes::default() });
    sent(&mut a);
    a
}

#[test]
fn keys_go_to_the_resident_on_screen_and_the_leader_takes_the_next() {
    let mut a = shrine();
    host(&mut a, b"n");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["input"]);
    assert_eq!(out[0]["bytes"], serde_json::json!([b'n']));
    host(&mut a, b"\x1d");
    assert!(a.m.leader);
    host(&mut a, b"n");
    assert!(matches!(a.m.modal, Some(Modal::Summon(_))), "{:?}", a.m.modal);
    assert!(sent(&mut a).is_empty());
    // With nobody on screen the letters work alone.
    let mut a = app();
    host(&mut a, b"t");
    assert!(matches!(a.m.modal, Some(Modal::Timetable(_))));
    assert_eq!(kinds(&sent(&mut a)), ["rituals"]);
}

#[test]
fn esc_in_the_timetable_goes_back_one_stage_at_a_time() {
    let mut a = app();
    host(&mut a, b"t");
    let ritual = |name: &str| RitualInfo { name: name.into(), enabled: true, ..Default::default() };
    daemon(
        &mut a,
        Reply::Rituals { id: 1, rituals: vec![ritual("alpha"), ritual("beta")], unusable: vec![] },
    );
    let tt = |a: &App| match &a.m.modal {
        Some(Modal::Timetable(tt)) => Some((tt.open.clone(), tt.confirm)),
        _ => None,
    };
    host(&mut a, b"\r");
    assert_eq!(tt(&a), Some((Some("alpha".into()), false)));
    host(&mut a, b"x");
    assert_eq!(tt(&a), Some((Some("alpha".into()), true)));
    host(&mut a, b"\x1b");
    assert_eq!(tt(&a), Some((Some("alpha".into()), false)));
    host(&mut a, b"\x1b");
    assert_eq!(tt(&a), Some((None, false)));
    host(&mut a, b"\x1b");
    assert_eq!(tt(&a), None);
    // Nothing was removed on the way.
    assert!(!sent(&mut a).iter().any(|r| r["t"] == "ritual"));
}

#[test]
fn a_resident_nobody_watches_rings_and_says_why() {
    let mut a = shrine();
    let notify = |watched| Reply::Notify {
        who: "id-Marisa".into(),
        name: "Marisa".into(),
        state: State::Awaits,
        text: "Marisa awaits".into(),
        watched,
    };
    daemon(&mut a, notify(true));
    assert!(a.host.is_empty() && a.m.message.is_none());
    daemon(&mut a, notify(false));
    assert_eq!(a.host, b"\x07\x1b]9;Marisa awaits\x07");
    assert_eq!(a.m.message.as_deref(), Some("✦ Marisa awaits"));
}

#[test]
fn damage_updates_the_screen_and_a_missed_frame_asks_for_the_whole_again() {
    let mut a = shrine();
    let row = |t: &str| vec![Run { col: 0, style: Style::default(), text: t.into() }];
    let damage = |base, rev, t: &str| Reply::Damage {
        who: "id-Reimu".into(),
        base,
        rev,
        rows: vec![(0, row(t))],
        cursor: Some((5, 0)),
        modes: Modes::default(),
        back: 0,
        history: 0,
    };
    daemon(&mut a, damage(1, 2, "hello there"));
    let screen = a.m.screen.as_ref().unwrap();
    assert_eq!((screen.rows[0][0].text.as_str(), screen.cursor), ("hello there", Some((5, 0))));
    assert!(sent(&mut a).is_empty());
    daemon(&mut a, damage(7, 8, "lost"));
    assert_eq!(a.m.screen.as_ref().unwrap().rows[0][0].text, "hello there");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["view"]);
    assert_eq!(out[0]["who"], "id-Reimu");
    // Damage for someone else is not ours to draw.
    let mut other = damage(2, 3, "not hers");
    if let Reply::Damage { who, .. } = &mut other {
        *who = "id-Marisa".into();
    }
    daemon(&mut a, other);
    assert_eq!(a.m.screen.as_ref().unwrap().rows[0][0].text, "hello there");
}

#[test]
fn a_click_on_a_sidebar_line_puts_that_resident_on_screen() {
    let mut a = shrine();
    let area = Rect::new(0, 0, 120, 40);
    a.paint(area, &mut Buffer::empty(area));
    assert_eq!(kinds(&sent(&mut a)), ["resize"]);
    // Marisa's line is the sidebar's second row: SGR is 1-based.
    host(&mut a, b"\x1b[<0;4;3M\x1b[<0;4;3m");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["view"]);
    assert_eq!(out[0]["who"], "id-Marisa");
    assert_eq!(a.m.focused.as_deref(), Some("id-Marisa"));
    assert!(a.m.screen.is_none(), "Reimu's screen is not Marisa's");
}
