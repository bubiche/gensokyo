//! The client's App without a terminal: host bytes and daemon lines in, the requests it queues
//! for the socket, the bytes it has for the host, and its model out.

use gensokyo::client::app::{App, Config, EDGE};
use gensokyo::client::framer::Framer;
use gensokyo::client::modal::{Modal, Stage};
use gensokyo::client::render::grid_rect;
use gensokyo::proto::{Card, Notice, Reply, Resident, RitualInfo, State};
use gensokyo::vt::{Frame, Modes, Run, Style, Vt};
use ratatui::backend::{Backend, CrosstermBackend};
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
        prefix: String::new(),
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
        ..Default::default()
    }
}

/// Reimu and Marisa in the shrine, Reimu on screen with her first frame.
fn shrine() -> App {
    let mut a = app();
    daemon(
        &mut a,
        Reply::Residents { residents: vec![resident(1, "Reimu"), resident(2, "Marisa")] },
    );
    let rows =
        vec![vec![Run { col: 0, style: Style::default(), text: "hello".into(), link: None }]];
    let frame = Frame { cols: 80, rows, cursor: None, ..Frame::default() };
    daemon(&mut a, Reply::Frame { who: "id-Reimu".into(), rev: 1, frame, modes: Modes::default() });
    sent(&mut a);
    a
}

#[test]
fn close_asks_before_a_live_resident_leaves_and_not_after() {
    let mut a = shrine();
    let close = |a: &mut App| sent(a).into_iter().filter(|r| r["t"] == "close").count();
    // The chord asks; n keeps it, and a y in the same breath as the chord is not a yes.
    host(&mut a, b"\x1dx");
    assert!(screen(&mut a).iter().any(|l| l.contains("Close Reimu?")));
    host(&mut a, b"n");
    assert!(a.m.modal.is_none());
    host(&mut a, b"\x1dx");
    host(&mut a, b"y");
    assert!(matches!(a.m.modal, Some(Modal::Close { .. })));
    assert_eq!(close(&mut a), 0);
    a.later(Duration::from_millis(400));
    host(&mut a, b"y");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["close"]);
    assert_eq!(out[0]["who"], "id-Reimu");
    // A departed one just leaves the sidebar.
    let mut gone = resident(1, "Reimu");
    gone.departed = Some(1);
    daemon(&mut a, Reply::Residents { residents: vec![gone, resident(2, "Marisa")] });
    sent(&mut a);
    host(&mut a, b"\x1dx");
    assert!(a.m.modal.is_none());
    assert_eq!(close(&mut a), 1);
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
#[test]
fn renew_shows_only_while_someone_is_behind_asks_and_a_new_pid_brings_its_new_screen() {
    let mut a = shrine();
    assert!(!screen(&mut a).iter().any(|l| l.contains("[renew u]")));
    host(&mut a, b"\x1du");
    assert!(a.m.modal.is_none());
    let out = sent(&mut a);
    assert!(!kinds(&out).contains(&"renew"), "{out:?}");

    let mut residents = a.m.residents.clone();
    residents[1].outdated = Some("2.1.300".into());
    daemon(&mut a, Reply::Residents { residents: residents.clone() });
    let rows = screen(&mut a);
    assert!(rows.iter().any(|l| l.contains("Marisa ⇡")), "{rows:#?}");
    click(&mut a, "[renew u]");
    assert!(screen(&mut a).iter().any(|l| l.contains("Renew Marisa?")));
    a.later(Duration::from_millis(400));
    host(&mut a, b"y");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["renew"]);
    assert_eq!(out[0].get("who"), None, "everyone behind");

    // On its way: the glyph turns and the button goes.
    residents[1].renewing = true;
    daemon(&mut a, Reply::Residents { residents: residents.clone() });
    let rows = screen(&mut a);
    assert!(rows.iter().any(|l| l.contains("Marisa ↻")), "{rows:#?}");
    assert!(!rows.iter().any(|l| l.contains("[renew u]")));

    // Reimu, on screen, renewed: the same id under a new pid, so her new screen is asked for.
    residents[0].pid = Some(999);
    daemon(&mut a, Reply::Residents { residents });
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["view"]);
    assert_eq!(out[0]["who"], "id-Reimu");
}

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
fn a_search_has_the_keys_until_esc_even_on_the_live_screen() {
    let mut a = shrine();
    let area = Rect::new(0, 0, 120, 40);
    a.paint(area, &mut Buffer::empty(area));
    sent(&mut a);
    let search = |out: &[Value]| -> Vec<(String, bool)> {
        let s = out.iter().filter(|r| r["t"] == "search");
        s.map(|r| (r["needle"].as_str().unwrap().into(), r["back"] == json!(true))).collect()
    };
    // The chord opens the prompt; what is typed is the prompt's, not the resident's.
    host(&mut a, b"\x1d/");
    host(&mut a, b"x");
    host(&mut a, b"\x7f");
    for ch in "needle".chars() {
        host(&mut a, ch.to_string().as_bytes());
    }
    assert!(sent(&mut a).is_empty());
    assert_eq!(a.m.find.as_ref().unwrap().typing.as_deref(), Some("needle"));
    host(&mut a, b"\r");
    assert_eq!(search(&sent(&mut a)), [("needle".into(), true)]);
    // Found on the live screen, the view is home, and n and N are still the search's.
    host(&mut a, b"n");
    host(&mut a, b"N");
    host(&mut a, b"\x1b[110;2u");
    let want = [("needle".into(), true), ("needle".into(), false), ("needle".into(), false)];
    assert_eq!(search(&sent(&mut a)), want);
    daemon(&mut a, Reply::Error { id: 9, error: "no “needle” further back".into() });
    assert_eq!(said(&a), Some("no “needle” further back"));
    // Keys of the search's that came in one read are taken one by one, as typed.
    host(&mut a, b"nnN");
    let want = [("needle".into(), true), ("needle".into(), true), ("needle".into(), false)];
    assert_eq!(search(&sent(&mut a)), want);
    host(&mut a, b"?abc");
    assert!(sent(&mut a).is_empty());
    assert_eq!(a.m.find.as_ref().unwrap().typing.as_deref(), Some("abc"));
    host(&mut a, b"\x1b");
    // ? the other way, and Enter with nothing typed looks for the same again.
    host(&mut a, b"?");
    host(&mut a, b"\r");
    assert_eq!(search(&sent(&mut a)), [("needle".into(), false)]);
    // Rows and pages still move; Esc goes home and ends it, and n is the resident's again.
    host(&mut a, b"k");
    assert_eq!(kinds(&sent(&mut a)), ["scroll"]);
    host(&mut a, b"\x1b");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["scroll"]);
    assert!(out[0].get("rows").is_none() && a.m.find.is_none());
    host(&mut a, b"n");
    assert_eq!(kinds(&sent(&mut a)), ["input"]);
    // A prompt left with nothing looked for leaves no search; with something, it stays.
    host(&mut a, b"\x1d/");
    host(&mut a, b"\x1b");
    assert!(a.m.find.is_none());
    host(&mut a, b"\x1d/");
    host(&mut a, b"ab\r");
    sent(&mut a);
    host(&mut a, b"/");
    host(&mut a, b"\x7f");
    assert_eq!(
        a.m.find.as_ref().map(|f| (f.needle.as_str(), f.typing.is_none())),
        Some(("ab", true))
    );
    // Another key goes home and on to the resident, and so does typing that only starts
    // with keys of ours.
    host(&mut a, b"x");
    assert_eq!(kinds(&sent(&mut a)), ["scroll", "input"]);
    assert!(a.m.find.is_none());
    host(&mut a, b"\x1d/");
    host(&mut a, b"ab\r");
    sent(&mut a);
    host(&mut a, b"nope");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["scroll", "input"]);
    assert_eq!(out[1]["bytes"], json!(b"nope"));
    // Home, then scrolled back without a search: / opens one, and n before one is any key.
    for back in [0, 9] {
        let frame = Frame { cols: 80, rows: vec![], back, history: 400, ..Frame::default() };
        let modes = Modes::default();
        daemon(&mut a, Reply::Frame { who: "id-Reimu".into(), rev: 2, frame, modes });
    }
    host(&mut a, b"/");
    assert!(a.m.find.as_ref().is_some_and(|f| f.back && f.typing.is_some()));
    host(&mut a, b"\x1b");
    host(&mut a, b"n");
    assert_eq!(kinds(&sent(&mut a)), ["scroll", "input"]);
    // On the alternate screen there is nothing to look through.
    let modes = Modes { alt: true, ..Modes::default() };
    let frame = Frame { cols: 80, rows: vec![], ..Frame::default() };
    daemon(&mut a, Reply::Frame { who: "id-Reimu".into(), rev: 3, frame, modes });
    host(&mut a, b"\x1d/");
    assert!(a.m.find.is_none());
    assert_eq!(said(&a), Some("a full-screen program has no scrollback"));
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
fn j_k_and_a_take_a_lead_s_helpers_right_after_it_as_the_sidebar_draws_them() {
    let mut a = shrine();
    // Sakuya is Reimu's helper, summoned after Marisa: the sidebar has her under Reimu.
    let sakuya = Resident { owner: Some("id-Reimu".into()), ..resident(3, "Sakuya") };
    let mut residents = a.m.residents.clone();
    residents.push(sakuya);
    daemon(&mut a, Reply::Residents { residents });
    let on = |a: &App| a.m.focused.clone().unwrap_or_default();
    for (keys, who) in [
        (&b"\x1dj"[..], "id-Sakuya"),
        (b"\x1dj", "id-Marisa"),
        (b"\x1dj", "id-Reimu"),
        (b"\x1dk", "id-Marisa"),
        (b"\x1dk", "id-Sakuya"),
    ] {
        host(&mut a, keys);
        assert_eq!(on(&a), who, "{keys:?}");
    }
    let mut residents = a.m.residents.clone();
    residents[1].state = State::Asked;
    residents[2].state = State::Asked;
    daemon(&mut a, Reply::Residents { residents });
    host(&mut a, b"\x1d1");
    host(&mut a, b"\x1da");
    assert_eq!(on(&a), "id-Sakuya");
}

#[test]
fn a_cast_starts_on_the_resident_on_screen_and_a_double_enter_casts_nothing() {
    let mut a = shrine();
    host(&mut a, b"\x1d2");
    host(&mut a, b"\x1dc");
    let card =
        Card { slug: "wrap".into(), title: "Wrap Up".into(), summary: String::new(), pair: false };
    daemon(&mut a, Reply::Cards { id: 1, cards: vec![card], unusable: vec![] });
    sent(&mut a);
    host(&mut a, b"\r");
    assert!(screen(&mut a).iter().any(|l| l.contains("› 2 ○ Marisa")));
    // The second Enter of a double one is not taken; one a moment later casts at Marisa.
    host(&mut a, b"\r");
    assert!(matches!(a.m.modal, Some(Modal::Cast(_))));
    assert!(!sent(&mut a).iter().any(|r| r["t"] == "cast"));
    a.later(Duration::from_millis(400));
    host(&mut a, b"\r");
    let out: Vec<Value> = sent(&mut a).into_iter().filter(|r| r["t"] == "cast").collect();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["targets"], serde_json::json!(["id-Marisa"]));
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
    host(&mut a, b"\r");
    assert_eq!(stage(&a), Some(Stage::Role));
    host(&mut a, b"\x1b");
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

/// The history's texts, newest first.
fn history(a: &App) -> Vec<&str> {
    a.m.history.iter().map(|(_, m)| m.text.as_str()).collect()
}

#[test]
fn what_was_said_is_kept_newest_first_and_the_oldest_go_past_fifty() {
    let mut a = shrine();
    for i in 0..60 {
        daemon(&mut a, Reply::Done { id: 9, message: format!("cast {i}") });
    }
    daemon(&mut a, Reply::Error { id: 9, error: "no resident 7".into() });
    let h = history(&a);
    assert_eq!((h.len(), h[0], h[1], h[49]), (50, "no resident 7", "cast 59", "cast 11"));
    // A chord clears the sidebar's message, not the history.
    host(&mut a, b"\x1dj");
    assert_eq!((said(&a), history(&a).len()), (None, 50));
}

#[test]
fn notices_from_before_take_their_places_and_those_nobody_heard_are_said_once() {
    let notice = |at, text: &str, missed| Notice { at, text: text.into(), missed };
    let mut a = shrine();
    daemon(&mut a, Reply::Done { id: 9, message: "now".into() });
    let notices = vec![
        notice(i64::MAX, "\x1b]9;from the future\x07", false),
        notice(30, "⏲ tea: not delivered", true),
        notice(20, "⏲ tea: waiting for Reimu", true),
        notice(10, "the daemon crashed", false),
    ];
    a.host.clear();
    daemon(&mut a, Reply::Notices { notices });
    let want = [" ]9;from the future ", "now", "⏲ tea: not delivered", "⏲ tea: waiting for Reimu"];
    assert_eq!(history(&a)[..4], want);
    assert_eq!(history(&a)[4], "the daemon crashed");
    assert_eq!(said(&a), Some("2 notices while you were away: ^] h"));
    // Old news: no bell, nothing to the desktop.
    assert!(a.host.is_empty(), "{:?}", String::from_utf8_lossy(&a.host));
    // One is said as it is; none, not at all.
    let mut a = shrine();
    daemon(&mut a, Reply::Notices { notices: vec![notice(30, "⏲ tea: not delivered", true)] });
    assert_eq!(said(&a), Some("⏲ tea: not delivered"));
    let mut a = shrine();
    daemon(&mut a, Reply::Notices { notices: vec![notice(30, "⏲ tea: not delivered", false)] });
    assert_eq!((said(&a), history(&a)), (None, vec!["⏲ tea: not delivered"]));
}

#[test]
fn leader_h_opens_the_history_which_scrolls_like_the_scrollback_and_closes() {
    let mut a = shrine();
    for i in 0..30 {
        daemon(&mut a, Reply::Done { id: 9, message: format!("cast {i}") });
    }
    let top = |a: &App| match &a.m.modal {
        Some(Modal::History(h)) => Some(h.top),
        _ => None,
    };
    // At 120x40, 20 rows of the 30 show at once.
    screen(&mut a);
    host(&mut a, b"\x1dh");
    assert_eq!(top(&a), Some(0));
    let rows = screen(&mut a);
    assert!(rows.iter().any(|l| l.contains("history 1–20 of 30")), "{rows:#?}");
    for (keys, want) in [
        (&b"j"[..], 1),
        (b"\x1b[B", 2),
        (b"k", 1),
        (b"\x1b[6~", 10),
        (b"\x1b[6~", 10),
        (b"\x1b[5~", 0),
        (b"\x1b[5~", 0),
        (b"G", 10),
        (b"g", 0),
        (b"x", 0),
    ] {
        host(&mut a, keys);
        assert_eq!(top(&a), Some(want), "{}", String::from_utf8_lossy(keys));
    }
    let rows = screen(&mut a);
    assert!(rows.iter().any(|l| l.contains("ago cast 29")), "{rows:#?}");
    for close in [&b"q"[..], b"h", b"\x1b"] {
        host(&mut a, b"\x1dh");
        assert_eq!(top(&a), Some(0));
        host(&mut a, close);
        assert_eq!(top(&a), None, "{}", String::from_utf8_lossy(close));
    }
    // Nothing went to the resident.
    assert!(sent(&mut a).iter().all(|r| r["t"] != "input"));
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
    assert_eq!(summoning(&a), Some((Stage::Role, false, None)));
    assert!(sent(&mut a).is_empty(), "the name alone summons nothing: the role comes next");
    host(&mut a, b"\r");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["summon"]);
    assert!(out[0].get("role").is_none() && out[0].get("system_prompt").is_none(), "{}", out[0]);
    let id = out[0]["id"].as_u64().unwrap();
    assert_eq!(summoning(&a), Some((Stage::Role, true, None)));
    // A second Enter while it waits sends nothing more.
    host(&mut a, b"\r");
    assert!(sent(&mut a).is_empty());
    daemon(&mut a, Reply::Error { id, error: "no claude on PATH".into() });
    assert_eq!(summoning(&a), Some((Stage::Role, false, Some("no claude on PATH".into()))));
    assert_eq!(said(&a), None, "the error is the modal's, not the sidebar's");
    host(&mut a, b"\r");
    let id = sent(&mut a)[0]["id"].as_u64().unwrap();
    daemon(&mut a, Reply::Summoned { id, resident: resident(3, "Sakuya"), note: None });
    assert!(a.m.modal.is_none());
    assert_eq!(a.m.focused.as_deref(), Some("id-Sakuya"));
    // Esc while it waits, and a new summon opened: the old reply leaves the new one alone.
    host(&mut a, b"\x1dn");
    host(&mut a, b"/\r\r\r");
    let id = sent(&mut a).last().unwrap()["id"].as_u64().unwrap();
    host(&mut a, b"\x1b");
    assert!(a.m.modal.is_none());
    host(&mut a, b"\x1dn");
    daemon(&mut a, Reply::Error { id, error: "late".into() });
    daemon(&mut a, Reply::Summoned { id, resident: resident(4, "Youmu"), note: None });
    assert_eq!(summoning(&a), Some((Stage::Dir, false, None)));
}

#[test]
fn the_role_stage_sends_the_role_picked_and_the_words_typed() {
    let mut a = shrine();
    host(&mut a, b"\x1dn");
    sent(&mut a);
    host(&mut a, b"/\r\r");
    let Some(Modal::Summon(s)) = &mut a.m.modal else { panic!("no summon modal") };
    assert_eq!((s.stage, s.role), (Stage::Role, 0), "none is picked first");
    s.roles = vec!["debugger".into(), "reviewer".into()];
    // Down past the end stays on the last; Up comes back one.
    host(&mut a, b"\x1b[B\x1b[B\x1b[B\x1b[A");
    host(&mut a, b"mind the SQL");
    host(&mut a, b"\x1b[200~ layer\nonly\n\x1b[201~");
    host(&mut a, b"\r");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["summon"]);
    assert_eq!(out[0]["role"], "debugger", "{}", out[0]);
    assert_eq!(out[0]["system_prompt"], "mind the SQL layer only", "{}", out[0]);
}

#[test]
fn damage_updates_the_screen_and_a_missed_frame_asks_for_the_whole_again() {
    let mut a = shrine();
    let row = |t: &str| vec![Run { col: 0, style: Style::default(), text: t.into(), link: None }];
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

#[test]
fn in_a_git_repository_summon_asks_for_a_worktree_and_enter_works_right_there() {
    let repo = std::env::temp_dir().join(format!("gsk-app-repo-{}", std::process::id()));
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let mut a = shrine();
    let stage = |a: &App| match &a.m.modal {
        Some(Modal::Summon(s)) => Some(s.stage),
        _ => None,
    };
    host(&mut a, b"\x1dn");
    sent(&mut a);
    host(&mut a, format!("{}\r", repo.display()).as_bytes());
    host(&mut a, b"\r\r");
    assert_eq!(stage(&a), Some(Stage::Worktree));
    assert!(sent(&mut a).is_empty(), "the name and role summon nothing in a repository");
    host(&mut a, b"\x1b");
    assert_eq!(stage(&a), Some(Stage::Role), "Esc goes back to the role");
    host(&mut a, b"\r\r");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["summon"]);
    assert!(out[0].get("worktree").is_none(), "empty is right here: {}", out[0]);

    host(&mut a, b"\x1b");
    host(&mut a, b"\x1dn");
    sent(&mut a);
    host(&mut a, format!("{}\r\r\rfix-login\r", repo.display()).as_bytes());
    let out = sent(&mut a);
    assert_eq!(out[0]["worktree"], serde_json::json!({"slug": "fix-login"}), "{}", out[0]);
    let (id, note) = (out[0]["id"].as_u64().unwrap(), "reused ~/x/.claude/worktrees/fix-login");
    daemon(
        &mut a,
        Reply::Summoned { id, resident: resident(3, "Sakuya"), note: Some(note.into()) },
    );
    assert_eq!(said(&a), Some(note), "the note is the user's to see");
    std::fs::remove_dir_all(&repo).unwrap();
}

/// A mouse report in SGR form: button, column and row (1-based), and press or release.
fn sgr(a: &mut App, b: u8, x: u16, y: u16, press: bool) {
    let end = if press { 'M' } else { 'm' };
    host(a, format!("\x1b[<{b};{x};{y}{end}").as_bytes());
}

/// Reimu on screen at 120x40: her grid's first cell is column 27, row 2 in SGR terms.
fn gridded() -> App {
    let mut a = shrine();
    let area = Rect::new(0, 0, 120, 40);
    a.paint(area, &mut Buffer::empty(area));
    sent(&mut a);
    a
}

/// The pointer requests sent: what, and the grid cell.
fn picks(out: &[Value]) -> Vec<(&str, u64, u64)> {
    let selects = out.iter().filter(|r| r["t"] == "select");
    selects
        .map(|r| (r["how"].as_str().unwrap(), r["x"].as_u64().unwrap(), r["y"].as_u64().unwrap()))
        .collect()
}

#[test]
fn a_drag_in_the_grid_is_the_daemons_to_select_and_what_it_answers_is_copied() {
    let mut a = gridded();
    sgr(&mut a, 0, 27, 2, true);
    sgr(&mut a, 32, 30, 3, true);
    // The same cell again says nothing new; over the sidebar it is held to the grid.
    sgr(&mut a, 32, 30, 3, true);
    sgr(&mut a, 32, 10, 3, true);
    sgr(&mut a, 0, 10, 3, false);
    let out = sent(&mut a);
    assert_eq!(picks(&out), [("press", 0, 0), ("drag", 3, 1), ("drag", 0, 1), ("release", 0, 1)]);
    assert!(out.iter().all(|r| r["t"] == "select" && r["who"] == "id-Reimu"), "{out:?}");
    // The release's answer is copied; any other `done` is said.
    let id = out[3]["id"].as_u64().unwrap();
    daemon(&mut a, Reply::Done { id, message: "two\nlines".into() });
    assert_eq!(said(&a), Some("copied 9 characters"));
    daemon(&mut a, Reply::Done { id, message: "news".into() });
    assert_eq!(said(&a), Some("news"));
    // With the resident's own mouse mode on, the press is the resident's.
    let modes = Modes { mouse: 1000, sgr: true, ..Modes::default() };
    let frame = Frame { cols: 80, rows: vec![], ..Frame::default() };
    daemon(&mut a, Reply::Frame { who: "id-Reimu".into(), rev: 2, frame, modes });
    sgr(&mut a, 0, 27, 2, true);
    assert_eq!(kinds(&sent(&mut a)), ["input"]);
}

#[test]
fn a_second_press_on_the_same_cell_soon_after_is_a_double_click_and_a_third_a_single() {
    let mut a = gridded();
    let click = |a: &mut App, x: u16| {
        sgr(a, 0, x, 5, true);
        sgr(a, 0, x, 5, false);
    };
    click(&mut a, 30);
    click(&mut a, 30);
    click(&mut a, 30);
    a.later(Duration::from_millis(600));
    click(&mut a, 30);
    a.later(Duration::from_millis(400));
    click(&mut a, 30);
    click(&mut a, 31);
    let out = sent(&mut a);
    let presses: Vec<&str> =
        picks(&out).into_iter().map(|p| p.0).filter(|h| *h != "release").collect();
    assert_eq!(presses, ["press", "double", "press", "press", "double", "press"]);
}

#[test]
fn held_past_the_grids_edge_a_drag_moves_the_view_on_the_clock_until_it_comes_back() {
    let mut a = gridded();
    sgr(&mut a, 0, 40, 20, true);
    // Over the box's top border, past the grid's top edge: held to its first row.
    sgr(&mut a, 32, 40, 1, true);
    assert_eq!(picks(&sent(&mut a)), [("press", 13, 18), ("drag", 13, 0)]);
    assert!(a.edge().is_some());
    a.tick();
    assert_eq!(sent(&mut a), Vec::<Value>::new(), "not yet");
    a.later(EDGE);
    a.tick();
    a.later(EDGE);
    a.tick();
    assert_eq!(picks(&sent(&mut a)), [("back", 13, 0), ("back", 13, 0)]);
    // Past the bottom edge, on toward the live screen.
    sgr(&mut a, 32, 41, 40, true);
    a.later(EDGE);
    a.tick();
    assert_eq!(picks(&sent(&mut a)), [("drag", 14, 37), ("on", 14, 37)]);
    // Back inside, it stops.
    sgr(&mut a, 32, 41, 20, true);
    assert!(a.edge().is_none());
    a.later(EDGE);
    a.tick();
    assert_eq!(picks(&sent(&mut a)), [("drag", 14, 18)]);
    // So does a release past the edge.
    sgr(&mut a, 32, 41, 1, true);
    sgr(&mut a, 0, 41, 1, false);
    assert!(a.edge().is_none());
    a.later(EDGE);
    a.tick();
    assert_eq!(picks(&sent(&mut a)), [("drag", 14, 0), ("release", 14, 0)]);
}

#[test]
fn a_drag_whose_release_cannot_come_stops_moving_the_view() {
    let mut a = gridded();
    // Another button's release is not the drag's.
    sgr(&mut a, 0, 40, 20, true);
    sgr(&mut a, 2, 41, 20, false);
    assert_eq!(picks(&sent(&mut a)), [("press", 13, 18)]);
    // A press soon after a drag is a new selection, not a double click.
    sgr(&mut a, 32, 42, 20, true);
    sgr(&mut a, 0, 42, 20, false);
    sgr(&mut a, 0, 40, 20, true);
    sgr(&mut a, 0, 40, 20, false);
    let want = [("drag", 15, 18), ("release", 15, 18), ("press", 13, 18), ("release", 13, 18)];
    assert_eq!(picks(&sent(&mut a)), want);
    // A release away from the last cell reported: the motion to it was not.
    a.later(EDGE * 20);
    sgr(&mut a, 0, 40, 20, true);
    sgr(&mut a, 0, 46, 20, false);
    let want = [("press", 13, 18), ("drag", 19, 18), ("release", 19, 18)];
    assert_eq!(picks(&sent(&mut a)), want);
    // Capture turned off mid-drag past the edge: the release goes to the host now.
    sgr(&mut a, 0, 50, 20, true);
    sgr(&mut a, 32, 50, 1, true);
    host(&mut a, b"\x1dm");
    sent(&mut a);
    a.later(EDGE);
    a.tick();
    assert!(a.edge().is_none());
    assert_eq!(picks(&sent(&mut a)), []);
    // A modal opened mid-drag.
    host(&mut a, b"\x1dm");
    sgr(&mut a, 0, 50, 20, true);
    sgr(&mut a, 32, 50, 1, true);
    host(&mut a, b"\x1dt");
    sent(&mut a);
    a.later(EDGE);
    a.tick();
    assert!(a.edge().is_none());
    assert_eq!(picks(&sent(&mut a)), []);
}

#[test]
fn the_wheel_while_a_drag_is_held_scrolls_and_the_selection_follows_the_pointer() {
    let mut a = gridded();
    sgr(&mut a, 0, 40, 20, true);
    sgr(&mut a, 64, 40, 20, true);
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["select", "scroll", "select"]);
    assert_eq!(picks(&out), [("press", 13, 18), ("drag", 13, 18)]);
}

#[test]
fn leader_y_copies_the_last_answer_or_says_why_it_cannot() {
    let mut a = shrine();
    host(&mut a, b"\x1dy");
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["read"]);
    assert_eq!((&out[0]["who"], &out[0]["bare"]), (&json!("id-Reimu"), &json!(true)));
    let id = out[0]["id"].as_u64().unwrap();
    daemon(&mut a, Reply::Done { id, message: "the answer".into() });
    assert_eq!(said(&a), Some("copied Reimu's last answer, 10 characters"));
    host(&mut a, b"\x1dy");
    let id = sent(&mut a)[0]["id"].as_u64().unwrap();
    let error = "Reimu has not finished a turn yet".to_string();
    daemon(&mut a, Reply::Error { id, error: error.clone() });
    assert_eq!(said(&a), Some(error.as_str()));
}

#[test]
fn a_click_while_an_answer_is_on_its_way_leaves_it_bound_for_the_clipboard() {
    let mut a = gridded();
    host(&mut a, b"\x1dy");
    sgr(&mut a, 0, 30, 5, true);
    sgr(&mut a, 0, 30, 5, false);
    let out = sent(&mut a);
    assert_eq!(kinds(&out), ["read", "select", "select"]);
    let (read, release) = (out[0]["id"].as_u64().unwrap(), out[2]["id"].as_u64().unwrap());
    // A click selects nothing: its answer is empty, and says nothing.
    daemon(&mut a, Reply::Done { id: release, message: String::new() });
    assert_eq!(said(&a), None);
    daemon(&mut a, Reply::Done { id: read, message: "the whole answer".into() });
    assert_eq!(said(&a), Some("copied Reimu's last answer, 16 characters"));
}

/// A host terminal fed what the client writes for each frame: ratatui's changes, then the links.
struct Host {
    vt: Vt,
    buf: Buffer,
}

impl Host {
    fn new() -> Host {
        Host { vt: Vt::new(120, 40), buf: Buffer::empty(Rect::new(0, 0, 120, 40)) }
    }

    /// One frame drawn: the bytes `links` gave.
    fn draw(&mut self, a: &mut App) -> Vec<u8> {
        let area = self.buf.area;
        let mut buf = Buffer::empty(area);
        a.paint(area, &mut buf);
        let links = a.links(area, &buf);
        let mut out = Vec::new();
        CrosstermBackend::new(&mut out).draw(self.buf.diff(&buf).into_iter()).unwrap();
        out.extend(&links);
        self.vt.feed(&out);
        self.buf = buf;
        links
    }

    /// The host's linked runs: row, column, text and link.
    fn links(&mut self) -> Vec<(usize, u16, String, String)> {
        let f = self.vt.frame();
        let runs = f.rows.iter().enumerate().flat_map(|(y, r)| r.iter().map(move |r| (y, r)));
        runs.filter_map(|(y, r)| Some((y, r.col, r.text.clone(), r.link.clone()?))).collect()
    }
}

/// Reimu's screen: `runs` on row 2, each a text and maybe a link, and `title` set.
fn screen_of(a: &mut App, runs: &[(&str, Option<&str>)], style: Style, title: &str) {
    let mut col = 0;
    let row = runs.iter().map(|(text, link)| {
        let r = Run { col, style, text: text.to_string(), link: link.map(String::from) };
        col += text.chars().count() as u16;
        r
    });
    let rows = vec![vec![], vec![], row.collect()];
    let frame = Frame { cols: 80, rows, cursor: Some((0, 0)), ..Frame::default() };
    let modes = Modes { title: title.into(), ..Modes::default() };
    daemon(a, Reply::Frame { who: "id-Reimu".into(), rev: 1, frame, modes });
}

#[test]
fn a_link_is_drawn_again_inside_osc_8_over_its_own_cells_and_the_cursor_put_back() {
    let mut a = shrine();
    let mut h = Host::new();
    let url = "https://example.com";
    let ex = Some(url);
    screen_of(&mut a, &[("see ", None), ("example", ex), (" end", None)], Style::default(), "");
    let out = h.draw(&mut a);
    let g = grid_rect(Rect::new(0, 0, 120, 40));
    let at = (g.y as usize + 2, g.x + 4);
    assert_eq!(h.links(), [(at.0, at.1, "example".into(), url.into())]);
    assert!(out.starts_with(b"\x1b]8;;https://example.com\x1b\\"), "{out:?}");
    let home = format!("\x1b]8;;\x1b\\\x1b[{};{}H", g.y + 1, g.x + 1);
    assert!(out.ends_with(home.as_bytes()), "{out:?}");
    assert_eq!(h.vt.frame().cursor, Some((g.x, g.y)));
    assert!(h.vt.frame().text()[at.0].contains("see example end"));
    // Every frame, as the selection draws it.
    let picked = Style { attrs: Style::INVERSE, ..Style::default() };
    screen_of(&mut a, &[("see ", None), ("example", ex), (" end", None)], picked, "");
    let out = h.draw(&mut a);
    assert!(!out.is_empty());
    assert_eq!(h.links(), [(at.0, at.1, "example".into(), url.into())]);
    let row = h.vt.frame().rows[at.0].clone();
    assert!(row.iter().any(|r| r.text == "example" && r.style.attrs & Style::INVERSE != 0));
}

#[test]
fn a_link_that_goes_away_is_drawn_again_bare_and_none_shows_under_a_modal() {
    let mut a = shrine();
    let mut h = Host::new();
    let ex = Some("https://example.com");
    screen_of(&mut a, &[("see ", None), ("example", ex)], Style::default(), "");
    h.draw(&mut a);
    assert_eq!(h.links().len(), 1);
    // The same text, no longer a link: ratatui writes nothing, the links do.
    screen_of(&mut a, &[("see example", None)], Style::default(), "");
    let out = h.draw(&mut a);
    assert_eq!(h.links(), []);
    assert!(String::from_utf8_lossy(&out).contains("example"), "{out:?}");
    assert!(!String::from_utf8_lossy(&out).contains("\x1b]8;;h"));
    // Nothing more to say then.
    assert_eq!(h.draw(&mut a), b"");
    // A link the whole width of the grid's middle row, under the quit modal.
    let x = "x".repeat(90);
    let mut rows = vec![vec![]; 19];
    let link = Some("https://x".to_string());
    rows.push(vec![Run { col: 0, style: Style::default(), text: x, link }]);
    let frame = Frame { cols: 93, rows, ..Frame::default() };
    daemon(&mut a, Reply::Frame { who: "id-Reimu".into(), rev: 2, frame, modes: Modes::default() });
    let linked = |h: &mut Host| h.links().iter().map(|l| l.2.len()).sum::<usize>();
    h.draw(&mut a);
    assert_eq!(linked(&mut h), 90);
    host(&mut a, b"\x1dq");
    assert!(matches!(a.m.modal, Some(Modal::Quit)));
    h.draw(&mut a);
    let links = h.links();
    assert_eq!(links.len(), 2, "{links:?}");
    assert!(links.iter().all(|l| l.2.chars().all(|c| c == 'x')));
    assert!(linked(&mut h) < 90);
    host(&mut a, b"\x1b");
    h.draw(&mut a);
    assert_eq!(linked(&mut h), 90);
}

#[test]
fn a_link_that_could_break_out_of_its_escape_is_not_drawn() {
    for bad in [
        "https://x\x1b]0;pwned\x07",
        "https://x\x1b\\\x1b]0;pwned\x07",
        "https://x\x07",
        "https://x\u{9c}",
        "javascript:alert(1)",
        "ftp://example.com",
    ] {
        let mut a = shrine();
        screen_of(&mut a, &[("click", Some(bad))], Style::default(), "");
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        a.paint(area, &mut buf);
        assert_eq!(a.links(area, &buf), b"", "{bad:?}");
    }
}

/// The host bytes queued since the last call.
fn hosted(a: &mut App) -> String {
    String::from_utf8(std::mem::take(&mut a.host)).unwrap()
}

#[test]
fn the_tab_title_is_the_resident_on_screen_and_its_own_title_set_only_on_a_change() {
    let mut a = app();
    a.retitle();
    assert_eq!(hosted(&mut a), "\x1b]0;gensokyo\x1b\\");
    let mut a = shrine();
    a.retitle();
    assert_eq!(hosted(&mut a), "\x1b]0;Reimu\x1b\\");
    a.retitle();
    assert_eq!(hosted(&mut a), "");
    screen_of(&mut a, &[("hi", None)], Style::default(), "✳ Fix the bug");
    hosted(&mut a);
    a.retitle();
    assert_eq!(hosted(&mut a), "\x1b]0;Reimu · ✳ Fix the bug\x1b\\");
    a.retitle();
    assert_eq!(hosted(&mut a), "");
    // Another resident, before its screen comes: its name alone.
    host(&mut a, b"\x1d2");
    hosted(&mut a);
    a.retitle();
    assert_eq!(hosted(&mut a), "\x1b]0;Marisa\x1b\\");
    // A title cannot end the sequence that carries it.
    host(&mut a, b"\x1d1");
    screen_of(&mut a, &[("hi", None)], Style::default(), "a\x1b]0;pwned\x07\u{9c}b");
    hosted(&mut a);
    a.retitle();
    assert_eq!(hosted(&mut a), "\x1b]0;Reimu · a ]0;pwned  b\x1b\\");
}

#[test]
fn a_sidebar_too_short_for_everyone_keeps_the_one_on_screen_and_counts_the_rest() {
    let names = ["Reimu", "Marisa", "Sakuya", "Youmu", "Sanae", "Remilia"];
    let names = names.iter().chain(&["Alice", "Patchouli", "Cirno", "Aya", "Suika", "Yukari"]);
    let mut all: Vec<Resident> = names.enumerate().map(|(i, n)| resident(i as u8 + 1, n)).collect();
    all.iter_mut().skip(9).for_each(|r| r.slot = None);
    all[1].state = State::Awaits;
    let mut a = app();
    daemon(&mut a, Reply::Residents { residents: all });
    let area = Rect::new(0, 0, 120, 16);
    let side = |a: &mut App| {
        let mut buf = Buffer::empty(area);
        a.paint(area, &mut buf);
        let cells: Vec<&str> = buf.content.iter().map(|c| c.symbol()).collect();
        cells.chunks(120).map(|r| r[..25].concat()).collect::<Vec<_>>()
    };
    let has = |rows: &[String], s: &str| rows.iter().position(|l| l.contains(s));
    // Reimu on screen: the top of the list, and a count of the rest below.
    let rows = side(&mut a);
    assert!(has(&rows, "1 ○ Reimu").is_some(), "{rows:#?}");
    assert!(has(&rows, "↑").is_none());
    assert!(has(&rows, "↓ ").is_some_and(|y| !rows[y].contains("needs you")), "{rows:#?}");
    // Cirno on screen: in view, with Marisa, who needs you, counted among those above.
    host(&mut a, b"\x1d9");
    let rows = side(&mut a);
    assert!(has(&rows, "9 ○ Cirno").is_some(), "{rows:#?}");
    assert!(has(&rows, "1 ○ Reimu").is_none());
    let y = has(&rows, "more, 1 needs you").expect("the line above");
    assert!(rows[y].contains('↑'), "{rows:#?}");
    assert!(has(&rows, "↓ 3 more").is_some(), "{rows:#?}");
    // A click on it shows her.
    sent(&mut a);
    host(&mut a, format!("\x1b[<0;3;{}M\x1b[<0;3;{}m", y + 1, y + 1).as_bytes());
    assert_eq!(a.m.focused.as_deref(), Some("id-Marisa"));
    assert!(has(&side(&mut a), "Marisa").is_some());
}
