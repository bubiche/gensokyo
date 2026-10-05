//! The client's App without a terminal: host bytes and daemon lines in, the requests it queues
//! for the socket, the bytes it has for the host, and its model out.

use gensokyo::client::app::{App, Config};
use gensokyo::client::framer::Framer;
use gensokyo::client::modal::{Modal, Stage};
use gensokyo::proto::{Card, Reply, Resident, RitualInfo, State};
use gensokyo::vt::{Frame, Modes, Run, Style};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::{Value, json};
use std::time::Duration;

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

/// What the app draws at 120x40, row by row.
fn screen(a: &mut App) -> Vec<String> {
    let area = Rect::new(0, 0, 120, 40);
    let mut buf = Buffer::empty(area);
    a.paint(area, &mut buf);
    let cells: Vec<&str> = buf.content.iter().map(|c| c.symbol()).collect();
    cells.chunks(120).map(|r| r.concat()).collect()
}

/// A left click on the first cell of `text`.
fn click(a: &mut App, text: &str) {
    let rows = screen(a);
    let (y, line) = rows.iter().enumerate().find(|(_, l)| l.contains(text)).expect(text);
    let x = line[..line.find(text).unwrap()].chars().count() + 1;
    host(a, format!("\x1b[<0;{x};{}M\x1b[<0;{x};{}m", y + 1, y + 1).as_bytes());
}

#[test]
fn the_timetable_picks_runs_pauses_and_removes_with_a_yes() {
    let mut a = shrine();
    host(&mut a, b"\x1dt");
    assert_eq!(kinds(&sent(&mut a)), ["rituals"]);
    let ritual = |name: &str, next: Option<i64>| RitualInfo {
        name: name.into(),
        enabled: true,
        next_fire: next,
        next_fire_local: next.map(|_| "2026-09-21 16:00".into()),
        ..Default::default()
    };
    let slack = RitualInfo { shipped: true, enabled: false, ..ritual("slack-morning", None) };
    let list = vec![ritual("alpha", Some(50)), ritual("beta", Some(100)), slack.clone()];
    daemon(&mut a, Reply::Rituals { id: 1, rituals: list, unusable: vec![] });
    let tt = |a: &App| match &a.m.modal {
        Some(Modal::Timetable(tt)) => (tt.selected, tt.open.clone(), tt.confirm),
        _ => panic!("the timetable closed"),
    };

    // Soonest first: alpha, beta, then the paused example. j, k and the arrows stop at the ends.
    let keys: [(&[u8], usize); 7] = [
        (b"j", 1),
        (b"k", 0),
        (b"k", 0),
        (b"\x1b[B", 1),
        (b"\x1b[B", 2),
        (b"\x1b[B", 2),
        (b"\x1b[A", 1),
    ];
    for (k, at) in keys {
        host(&mut a, k);
        assert_eq!(tt(&a).0, at, "after {k:?}");
    }
    host(&mut a, b"k");
    host(&mut a, b"\r");
    assert_eq!(tt(&a).1.as_deref(), Some("alpha"));

    // Run now, then pause; once the daemon says it is paused, p resumes it.
    // The first paint also asks for its size.
    let rituals = |a: &mut App| sent(a).into_iter().filter(|r| r["t"] == "ritual").collect();
    let verb = |a: &mut App| {
        let out: Vec<Value> = rituals(a);
        assert_eq!(out.len(), 1, "{out:?}");
        (out[0]["verb"].as_str().unwrap().to_string(), out[0]["name"].as_str().unwrap().into())
    };
    host(&mut a, b"r");
    assert_eq!(verb(&mut a), ("run".into(), "alpha".to_string()));
    assert_eq!(said(&a), Some("running alpha…"));
    host(&mut a, b"p");
    assert_eq!(verb(&mut a), ("disable".into(), "alpha".to_string()));
    let paused = RitualInfo { enabled: false, ..ritual("alpha", None) };
    let list = vec![paused, ritual("beta", Some(100)), slack.clone()];
    daemon(&mut a, Reply::Rituals { id: 0, rituals: list, unusable: vec![] });
    assert!(screen(&mut a).iter().any(|l| l.contains("[resume p]")));
    host(&mut a, b"p");
    assert_eq!(verb(&mut a), ("enable".into(), "alpha".to_string()));

    // Back on the list, the selection stayed on alpha, paused and so now after beta.
    host(&mut a, b"\x1b");
    assert_eq!(tt(&a), (1, None, false));
    // A click on a row opens it. An example has no remove, and x says what to do instead.
    click(&mut a, "slack-morning");
    assert_eq!(tt(&a).1.as_deref(), Some("slack-morning"));
    assert!(!screen(&mut a).iter().any(|l| l.contains("[remove x]")));
    host(&mut a, b"x");
    assert_eq!(said(&a), Some("slack-morning ships with gensokyo: pause it instead"));
    assert!(!tt(&a).2);
    assert!(rituals(&mut a).is_empty());

    // Remove asks; n backs out, and a y straight after the x is taken for typing, not a yes.
    host(&mut a, b"\x1b");
    click(&mut a, "beta");
    assert!(screen(&mut a).iter().any(|l| l.contains("[remove x]")));
    host(&mut a, b"x");
    assert_eq!(tt(&a), (0, Some("beta".into()), true));
    host(&mut a, b"n");
    assert_eq!(tt(&a), (0, Some("beta".into()), false));
    host(&mut a, b"x");
    host(&mut a, b"y");
    assert!(tt(&a).2, "a y in the same breath as the x");
    assert!(rituals(&mut a).is_empty());
    a.later(Duration::from_millis(400));
    host(&mut a, b"y");
    assert_eq!(verb(&mut a), ("remove".into(), "beta".to_string()));
    assert_eq!(tt(&a), (0, None, false));
    assert_eq!(said(&a), Some("removing beta…"));
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
    assert_eq!(said(&a), Some("✦ Marisa awaits"));
}

#[test]
fn the_wheel_and_the_chord_scroll_back_and_keys_move_or_leave() {
    let mut a = shrine();
    let area = Rect::new(0, 0, 120, 40);
    a.paint(area, &mut Buffer::empty(area));
    sent(&mut a);
    let rows = |out: &[Value]| -> Vec<Value> { out.iter().map(|r| r["rows"].clone()).collect() };
    // The wheel over the grid, up then down.
    host(&mut a, b"\x1b[<64;40;10M\x1b[<65;40;10M");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["scroll", "scroll"]);
    assert_eq!(rows(&out), [json!(-3), json!(3)]);
    // Half the 38-row grid back.
    host(&mut a, b"\x1d[");
    assert_eq!(rows(&sent(&mut a)), [json!(-19)]);
    // Until the daemon says it is scrolled back, keys are the resident's.
    host(&mut a, b"k");
    assert_eq!(kinds(&sent(&mut a)), ["input"]);
    let scrolled = |a: &mut App, back| {
        let frame = Frame { cols: 80, rows: vec![], back, history: 400, ..Frame::default() };
        let modes = Modes::default();
        daemon(a, Reply::Frame { who: "id-Reimu".into(), rev: 2, frame, modes });
    };
    scrolled(&mut a, 19);
    host(&mut a, b"k");
    host(&mut a, b"\x1b[6~");
    host(&mut a, b"g");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["scroll", "scroll", "scroll"]);
    assert_eq!(rows(&out)[..2], [json!(-1), json!(37)]);
    assert!(out[2]["rows"].as_i64().unwrap() < -10_000);
    // q is home again, and nothing for the resident; the next k is the resident's at once.
    host(&mut a, b"q");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["scroll"]);
    assert!(out[0].get("rows").is_none(), "{out:?}");
    host(&mut a, b"k");
    assert_eq!(kinds(&sent(&mut a)), ["input"]);
    // A screen the daemon sent before it saw the way home is still scrolled back: the next
    // key is the resident's all the same.
    scrolled(&mut a, 25);
    host(&mut a, b" ");
    assert_eq!(kinds(&sent(&mut a)), ["input"]);
    // Home, then scrolled back again (by another client, say): typing goes home, then on.
    scrolled(&mut a, 0);
    scrolled(&mut a, 5);
    host(&mut a, b"x");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["scroll", "input"]);
    assert_eq!(out[1]["bytes"], json!([b'x']));
}

#[test]
fn j_and_k_go_round_the_sidebar_and_a_finds_whoever_needs_you() {
    let mut a = shrine();
    let mut sakuya = resident(3, "Sakuya");
    sakuya.state = State::Asked;
    let mut residents = a.m.residents.clone();
    residents.push(sakuya);
    daemon(&mut a, Reply::Residents { residents });
    let on = |a: &App| a.m.focused.clone().unwrap_or_default();
    for (keys, who) in [
        (&b"\x1dj"[..], "id-Marisa"),
        (b"\x1dj", "id-Sakuya"),
        (b"\x1dj", "id-Reimu"),
        (b"\x1dk", "id-Sakuya"),
        (b"\x1dk", "id-Marisa"),
        (b"\x1da", "id-Sakuya"),
    ] {
        host(&mut a, keys);
        assert_eq!(on(&a), who, "{keys:?}");
    }
    let mut residents = a.m.residents.clone();
    residents[2].state = State::Resting;
    daemon(&mut a, Reply::Residents { residents });
    host(&mut a, b"\x1da");
    assert_eq!((on(&a).as_str(), said(&a)), ("id-Sakuya", Some("nobody needs you")));
    host(&mut a, b"\x1d7");
    assert_eq!((on(&a).as_str(), said(&a)), ("id-Sakuya", Some("nobody is in slot 7")));
}

#[test]
fn esc_goes_back_one_stage_in_summon_and_cast_too() {
    let mut a = shrine();
    host(&mut a, b"\x1dn");
    host(&mut a, b"/\r");
    let stage = |a: &App| match &a.m.modal {
        Some(Modal::Summon(s)) => Some(s.stage),
        _ => None,
    };
    assert_eq!(stage(&a), Some(Stage::Name));
    host(&mut a, b"\x1b");
    assert_eq!(stage(&a), Some(Stage::Dir));
    host(&mut a, b"\x1b");
    assert_eq!(stage(&a), None);
    host(&mut a, b"\x1dc");
    let card =
        Card { slug: "pair".into(), title: "Pair".into(), summary: String::new(), pair: true };
    daemon(&mut a, Reply::Cards { id: 1, cards: vec![card], unusable: vec![] });
    let cast = |a: &App| match &a.m.modal {
        Some(Modal::Cast(c)) => Some((c.card.is_some(), c.target.clone())),
        _ => None,
    };
    host(&mut a, b"\r");
    host(&mut a, b"\r");
    assert_eq!(cast(&a), Some((true, Some("id-Reimu".into()))));
    host(&mut a, b"\x1b");
    assert_eq!(cast(&a), Some((true, None)));
    host(&mut a, b"\x1b");
    assert_eq!(cast(&a), Some((false, None)));
    host(&mut a, b"\x1b");
    assert_eq!(cast(&a), None);
    assert!(!sent(&mut a).iter().any(|r| r["t"] == "cast"));
}

#[test]
fn on_a_departed_screen_r_recalls_it_and_the_chord_lists_them_all() {
    let mut a = shrine();
    let mut residents = a.m.residents.clone();
    residents[0].departed = Some(1);
    daemon(&mut a, Reply::Residents { residents });
    sent(&mut a);
    host(&mut a, b"r");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["recall"]);
    assert_eq!(out[0]["who"], "id-Reimu");
    assert!(a.m.modal.is_none());
    host(&mut a, b"\x1dr");
    assert!(matches!(a.m.modal, Some(Modal::Recall(_))));
}

/// Each byte as a read of its own, as a person types.
fn typing(a: &mut App, text: &[u8]) {
    for b in text {
        host(a, &[*b]);
        a.later(Duration::from_millis(120));
    }
}

#[test]
fn typing_that_outlives_its_resident_goes_nowhere_and_a_pause_gives_the_keys_back() {
    let mut a = shrine();
    let mut residents = a.m.residents.clone();
    residents[1].state = State::Awaits;
    typing(&mut a, b"looks ");
    assert_eq!(sent(&mut a).len(), 6);
    // Reimu leaves mid-sentence: `a` must not move on to Marisa and answer her dialog, nor
    // `x` close the departed screen and type the rest into whoever comes next.
    residents[0].departed = Some(1);
    daemon(&mut a, Reply::Residents { residents });
    sent(&mut a);
    typing(&mut a, b"all good, fix it\r");
    assert!(sent(&mut a).is_empty());
    assert_eq!(a.m.focused.as_deref(), Some("id-Reimu"));
    assert!(a.m.modal.is_none());
    assert_eq!(said(&a), Some("Reimu has left; what was typed went nowhere"));
    // After a pause the letters are the departed screen's own.
    a.later(Duration::from_secs(2));
    host(&mut a, b"r");
    assert_eq!(kinds(&sent(&mut a)), ["recall"]);
}

#[test]
fn keys_for_the_shrine_itself_come_as_fast_as_they_like() {
    // A dialog opened and closed, and the next straight after: nothing was typed to anyone.
    let mut a = app();
    host(&mut a, b"t");
    host(&mut a, b"\x1b");
    host(&mut a, b"n");
    assert!(matches!(a.m.modal, Some(Modal::Summon(_))), "{:?}", a.m.modal);
}

#[test]
fn a_yes_in_a_burst_of_typing_answers_nothing() {
    let mut a = app();
    // `q` then Enter at once, as the end of a word and a line would come.
    host(&mut a, b"q");
    host(&mut a, b"\r");
    assert!(matches!(a.m.modal, Some(Modal::Quit)));
    typing(&mut a, b"uickly");
    assert!(matches!(a.m.modal, Some(Modal::Quit)), "{:?}", a.m.modal);
    assert!(sent(&mut a).is_empty());
    a.later(Duration::from_millis(400));
    host(&mut a, b"y");
    assert_eq!(kinds(&sent(&mut a)), ["quit"]);
}

fn said(a: &App) -> Option<&str> {
    a.m.message.as_ref().map(|m| m.text.as_str())
}

#[test]
fn a_message_goes_when_its_time_is_up_without_a_key() {
    let mut a = shrine();
    daemon(&mut a, Reply::Done { id: 9, message: "cast it".into() });
    let at = a.expires().expect("a deadline");
    a.expire(at - Duration::from_millis(1));
    assert_eq!(said(&a), Some("cast it"));
    a.expire(at);
    assert_eq!(said(&a), None);
    assert_eq!(a.expires(), None);
    // An error stays longer than news of something done.
    daemon(&mut a, Reply::Error { id: 9, error: "no such resident".into() });
    assert!(a.expires().unwrap() > at + Duration::from_secs(3));
}

#[test]
fn summon_waits_for_its_reply_and_shows_its_error() {
    let mut a = shrine();
    host(&mut a, b"n");
    sent(&mut a);
    host(&mut a, b"/\r");
    let summoning = |a: &App| match &a.m.modal {
        Some(Modal::Summon(s)) => Some((s.stage, s.waiting, s.error.clone())),
        _ => None,
    };
    assert_eq!(summoning(&a), Some((Stage::Name, false, None)));
    host(&mut a, b"\r");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["summon"]);
    let id = out[0]["id"].as_u64().unwrap();
    assert_eq!(summoning(&a), Some((Stage::Name, true, None)));
    // A second Enter while it waits sends nothing more.
    host(&mut a, b"\r");
    assert!(sent(&mut a).is_empty());
    daemon(&mut a, Reply::Error { id, error: "no claude on PATH".into() });
    assert_eq!(summoning(&a), Some((Stage::Name, false, Some("no claude on PATH".into()))));
    assert_eq!(said(&a), None, "the error is the modal's, not the sidebar's");
    host(&mut a, b"\r");
    let id = sent(&mut a)[0]["id"].as_u64().unwrap();
    daemon(&mut a, Reply::Summoned { id, resident: resident(3, "Sakuya") });
    assert!(a.m.modal.is_none());
    assert_eq!(a.m.focused.as_deref(), Some("id-Sakuya"));
    // Esc while it waits, and a new summon opened: the old reply leaves the new one alone.
    host(&mut a, b"\x1dn");
    host(&mut a, b"/\r");
    host(&mut a, b"\r");
    let id = sent(&mut a).last().unwrap()["id"].as_u64().unwrap();
    host(&mut a, b"\x1b");
    assert!(a.m.modal.is_none());
    host(&mut a, b"\x1dn");
    daemon(&mut a, Reply::Error { id, error: "late".into() });
    daemon(&mut a, Reply::Summoned { id, resident: resident(4, "Youmu") });
    assert_eq!(summoning(&a), Some((Stage::Dir, false, None)));
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
