//! The client's screen: snapshots of every screen and modal, and the hit map they return.

use gensokyo::client::modal::{self, Cast, History, Modal, Recall, Stage, Summon, Timetable};
use gensokyo::client::render::{self, Awake, Button, Find, Hit, HitMap, Message, Model, Say};
use gensokyo::proto::{Card, Limit, Resident, RitualInfo, State, Telemetry};
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
        ..Default::default()
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
                Run {
                    col: 0,
                    style: st(Color::Palette(2)),
                    text: format!("line {y:02}"),
                    link: None,
                },
                Run {
                    col: 12,
                    style: st(Color::Default),
                    text: "~".repeat((y as usize) % 20),
                    link: None,
                },
            ]
        })
        .collect();
    Frame { cols, rows, cursor: Some((2, 3)), ..Frame::default() }
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
        today: "2026-09-21".into(),
        ..Model::default()
    }
}

/// Reimu leads two helpers, summoned after Marisa and Cirno; Chen's lead is gone.
fn helpers() -> Model {
    let mut m = shrine();
    let helper = |n, name, lead: &str| Resident {
        owner: Some(lead.into()),
        ..resident(n, name, "dev/gensokyo", None)
    };
    m.residents.push(helper(4, "Youmu", "id-Reimu"));
    m.residents.push(helper(5, "Hieda-no-Akyuu-the-Ninth", "id-Reimu"));
    m.residents.push(helper(6, "Chen", "id-Ran"));
    m.residents[3].state = State::Busy;
    m
}

/// The helpers on their branches, `BRANCH_PREFIX` left off: one too long for the sidebar, a
/// detached HEAD, and a departed one with none known.
fn branches() -> Model {
    let mut m = Model { prefix: "nebel95/".into(), ..helpers() };
    let b = ["nebel95/padlet-ai-summary-for-every-board", "main", "", "nebel95/fix-ci", "a1b2c3d"];
    for (r, b) in m.residents.iter_mut().zip(b) {
        r.branch = Some(b.to_string()).filter(|b| !b.is_empty());
    }
    m
}

/// Tags beside the branches: Reimu's long branch cut to make room for its, Cirno's without a
/// branch, and Hieda's too many to fit, cut at the end.
fn tagged() -> Model {
    let mut m = branches();
    let tags = |t: &[(&str, &str)]| t.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    m.residents[0].tags = tags(&[("pr", "#123"), ("ci", "green")]);
    m.residents[2].tags = tags(&[("phase", "review")]);
    m.residents[4].tags = tags(&[("pr", "#4567"), ("ci", "failing"), ("owner", "reimu")]);
    m
}

/// Reimu on screen with a name and a branch too long for the sidebar, leading two helpers.
fn long() -> Model {
    let mut m = branches();
    m.residents[0].name = "Reimu Hakurei of the Hakurei Shrine".into();
    m
}

/// Everyone who reports a model on the same one.
fn uniform() -> Model {
    let mut m = aware();
    m.residents[1].telemetry.as_mut().unwrap().model = Some("Haiku 4.5".into());
    m
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
        repo: stage != Stage::Dir,
        roles: ["chief-of-staff", "debugger", "implementer", "researcher", "reviewer"]
            .map(String::from)
            .into(),
        role: 5,
        words: if stage == Stage::Role { "focus on the SQL layer".into() } else { String::new() },
        run: String::new(),
        task: String::new(),
        worktree: if stage == Stage::Worktree { "fix-login".into() } else { String::new() },
        error: (stage == Stage::Name).then(|| "Yuyu is already here".into()),
        waiting: false,
    })
}

fn cast(card: Option<usize>, target: Option<&str>) -> Modal {
    let one = |slug: &str, title: &str, summary: &str, pair| Card {
        slug: slug.into(),
        title: title.into(),
        summary: summary.into(),
        pair,
    };
    let cards = vec![
        one("second-opinion", "Review Sign \"Second Opinion\"", "another resident reviews", true),
        one(
            "status-report",
            "Spirit Sign \"Status Report\"",
            "three lines from every resident",
            false,
        ),
    ];
    Modal::Cast(Cast {
        card: card.map(|i| cards[i].clone()),
        target: target.map(String::from),
        cards: Some(cards),
        unusable: vec![format!("{HOME}/.config/gensokyo/spellcards/bad name.md")],
        selected: 1,
    })
}

/// A timetable: one firing today, one tomorrow, one headless in a fortnight, a shipped example
/// paused, and one paused with something wrong with it.
fn rituals() -> Vec<RitualInfo> {
    let one = |name: &str, next: Option<(i64, &str)>, desc: &str| RitualInfo {
        name: name.into(),
        enabled: next.is_some(),
        schedule: "5 9 * * 1-5".into(),
        next_fire: next.map(|n| n.0),
        next_fire_local: next.map(|n| n.1.to_string()),
        target: "new".into(),
        keep: "2h".into(),
        overlap: "skip".into(),
        cwd: Some(format!("{HOME}/dev/mozart")),
        description: Some(desc.into()),
        path: format!("{HOME}/.config/gensokyo/rituals/{name}.md"),
        ..RitualInfo::default()
    };
    let mut v = vec![
        one("slack-morning", Some((NOW + 39_100, "2026-09-22 09:05")), "overnight Slack"),
        one("evening-notes", Some((NOW + 2_800, "2026-09-21 23:00")), "the day, in notes"),
        one("inbox-zero", Some((NOW + 1_158_000, "2026-10-05 08:00")), "the morning's mail"),
        one("nightly-checks", None, "the tests and the linter overnight"),
        one("broken", None, "never comes round"),
    ];
    v[0].last_run = Some(NOW - 47_000);
    v[0].last = Some("ran (due 2026-09-21 09:05)".into());
    v[0].running = true;
    v[0].mode = Some("acceptEdits".into());
    v[0].allowed_tools = vec!["mcp__claude_ai_Slack__*".into(), "Read".into()];
    v[0].when = Some("gh-prs".into());
    v[2].headless = true;
    (v[3].shipped, v[3].schedule) = (true, "0 2 * * *".into());
    v[4].schedule = "0 0 30 2 *".into();
    v[4].problem = Some(
        "schedule: 0 0 30 2 * never comes round (a date that does not exist, like 30 February)"
            .into(),
    );
    v
}

fn timetable(open: Option<&str>, confirm: bool) -> Modal {
    Modal::Timetable(Timetable { selected: 1, open: open.map(String::from), confirm })
}

/// What was said, newest first: a notice too long for one row, an error, then a run of plain
/// messages, more than the history shows at once.
fn history() -> Vec<(i64, Message)> {
    let mut h = vec![
        (
            NOW - 40,
            Message::new(
                Say::Notice,
                "⏲ standup: there is no resident called Sakuya, so the fire was not delivered; \
                 it fires again tomorrow at 09:05 and will look for her then",
            ),
        ),
        (NOW - 600, Message::new(Say::Error, "no resident 7")),
    ];
    h.extend((0..24).map(|i| (NOW - 3600 * (i + 1), Message::new(Say::Info, format!("cast {i}")))));
    h
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
        ("helpers", helpers()),
        ("aware", aware()),
        ("branches", branches()),
        ("tags", tagged()),
        (
            "aware-focused-gold",
            with(&|m| *m = Model { focused: Some("id-Marisa".into()), ..aware() }),
        ),
        ("departed", with(&|m| m.focused = Some("id-Cirno".into()))),
        (
            "scrolled",
            with(&|m| {
                let fr = m.screen.as_mut().unwrap();
                (fr.back, fr.history) = (120, 3400);
            }),
        ),
        (
            "search-prompt",
            with(&|m| {
                m.find =
                    Some(Find { needle: String::new(), back: true, typing: Some("nee".into()) });
            }),
        ),
        (
            "searching",
            with(&|m| {
                let fr = m.screen.as_mut().unwrap();
                (fr.back, fr.history) = (120, 3400);
                m.find = Some(Find { needle: "needle".into(), back: false, typing: None });
            }),
        ),
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
        ("summon-role", with(&|m| m.modal = Some(summon(Stage::Role)))),
        ("summon-worktree", with(&|m| m.modal = Some(summon(Stage::Worktree)))),
        (
            "banish",
            with(&|m| {
                m.modal = Some(Modal::Banish { id: "id-Reimu".into(), name: "Reimu".into() })
            }),
        ),
        (
            "recall",
            with(&|m| {
                m.modal = Some(Modal::Recall(Recall { list: departed.clone(), selected: 0 }))
            }),
        ),
        ("cast-card", with(&|m| m.modal = Some(cast(None, None)))),
        ("cast-target", with(&|m| m.modal = Some(cast(Some(1), None)))),
        ("cast-peer", with(&|m| m.modal = Some(cast(Some(0), Some("id-Reimu"))))),
        ("cast-loading", with(&|m| m.modal = Some(Modal::Cast(Cast::default())))),
        ("sidebar-next-today", with(&|m| m.rituals = Some(rituals()))),
        (
            "sidebar-next-later",
            with(&|m| {
                let mut r = rituals();
                r.retain(|r| r.name != "evening-notes" && r.name != "slack-morning");
                m.rituals = Some(r);
            }),
        ),
        (
            "sidebar-none-on",
            with(&|m| {
                let mut r = rituals();
                r.retain(|r| !r.enabled);
                m.rituals = Some(r);
            }),
        ),
        ("timetable-loading", with(&|m| m.modal = Some(timetable(None, false)))),
        (
            "timetable",
            with(&|m| {
                m.rituals = Some(rituals());
                m.modal = Some(timetable(None, false));
            }),
        ),
        (
            "timetable-detail",
            with(&|m| {
                m.rituals = Some(rituals());
                m.modal = Some(timetable(Some("slack-morning"), false));
            }),
        ),
        (
            "timetable-detail-problem",
            with(&|m| {
                m.rituals = Some(rituals());
                m.modal = Some(timetable(Some("broken"), false));
            }),
        ),
        (
            "timetable-detail-shipped",
            with(&|m| {
                m.rituals = Some(rituals());
                m.modal = Some(timetable(Some("nightly-checks"), false));
            }),
        ),
        (
            "timetable-remove",
            with(&|m| {
                m.rituals = Some(rituals());
                m.modal = Some(timetable(Some("slack-morning"), true));
            }),
        ),
        ("quit", with(&|m| m.modal = Some(Modal::Quit))),
        ("help", with(&|m| m.modal = Some(Modal::Help))),
        (
            "capture-off",
            with(&|m| {
                m.capture = false;
                m.message = Some(Message::new(Say::Error, "no resident 7"));
            }),
        ),
        (
            "awake-held",
            with(&|m| {
                m.capture = false;
                m.awake = Some(Awake { on: true, held: true });
            }),
        ),
        ("leader", with(&|m| m.leader = true)),
        ("history-empty", with(&|m| m.modal = Some(Modal::History(History::default())))),
        (
            "history",
            with(&|m| {
                m.history = history();
                m.modal = Some(Modal::History(History { top: 1 }));
            }),
        ),
        (
            "message-cut",
            with(&|m| {
                m.message = Some(Message::new(
                    Say::Notice,
                    "the daemon crashed, cutting its residents' turns off; back in their \
                     conversations: Reimu, Marisa; left departed: Cirno (no conversation to resume)",
                ))
            }),
        ),
        (
            "cast-reply",
            with(&|m| {
                m.message = Some(Message::new(
                    Say::Info,
                    "cast Spirit Sign \"Status Report\" on Reimu; Marisa has a dialog waiting \
                     for you; left out",
                ))
            }),
        ),
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

/// The sidebar's inside, row by row as drawn in `area` (row 0 is its top edge), and the buffer.
fn side(m: &Model, area: Rect) -> (Vec<String>, Buffer) {
    let mut buf = Buffer::empty(area);
    render::render(m, area, &mut buf);
    let row = |y| (1..render::SIDEBAR_W - 1).map(|x| buf[(x, y)].symbol()).collect();
    ((0..area.height).map(row).collect(), buf)
}

/// What a resident's dim rows say, their `  │ ` lead and every space left out: a lead's
/// helper's rows have one column more.
fn joined(rows: &[String]) -> String {
    rows.iter().flat_map(|r| r.chars().skip(4)).filter(|c| !c.is_whitespace()).collect()
}

const AREA: Rect = Rect { x: 0, y: 0, width: 120, height: 40 };

/// Drawn whole at both sizes: the screens themselves, and two modals for where a modal sits.
const WHOLE: [&str; 2] = ["summon-dir", "timetable-detail"];

#[test]
fn snapshots() {
    for (name, m) in screens() {
        if m.modal.is_some() && !WHOLE.contains(&name) {
            insta::assert_snapshot!(name, modal_box(&m).backend());
            continue;
        }
        for (w, h) in [(120, 40), (80, 24)] {
            let (t, _) = draw(&m, w, h);
            insta::assert_snapshot!(format!("{name}-{w}x{h}"), t.backend());
        }
    }
}

/// Just the modal's box, as drawn at 120x40: a change to the sidebar or the grid leaves it be.
fn modal_box(m: &Model) -> Terminal<TestBackend> {
    let (t, map) = draw(m, AREA.width, AREA.height);
    let (r, _) = *map.0.iter().find(|(_, h)| *h == Hit::Modal).expect("a modal");
    let mut boxed = Terminal::new(TestBackend::new(r.width, r.height)).unwrap();
    let whole = t.backend().buffer();
    boxed
        .draw(|f| {
            for (y, x) in (0..r.height).flat_map(|y| (0..r.width).map(move |x| (y, x))) {
                f.buffer_mut()[(x, y)] = whole[(r.x + x, r.y + y)].clone();
            }
        })
        .unwrap();
    boxed
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
        Button::Cast,
        Button::Timetable,
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
fn a_helper_line_resolves_to_the_helper_beneath_its_lead() {
    let map = hits(&helpers(), AREA);
    // Drawn Reimu, Youmu, Hieda, Marisa, Cirno, Chen: rows 1 to 6.
    for (row, i) in [0, 3, 4, 1, 2, 5].into_iter().enumerate() {
        assert_eq!(map.at(5, 1 + row as u16).map(|(_, h)| h), Some(Hit::Resident(i)), "row {row}");
    }
}

#[test]
fn a_branch_row_is_its_residents_and_goes_when_the_sidebar_is_short() {
    // Chen on screen: nobody here is drawn in full.
    let m = Model { focused: Some("id-Chen".into()), ..branches() };
    let map = hits(&m, AREA);
    // Reimu and its branch, Youmu and its, Hieda with none, Marisa and its, Cirno, Chen.
    for (row, i) in [0, 0, 3, 3, 4, 4, 1, 1, 2, 5].into_iter().enumerate() {
        assert_eq!(map.at(5, 1 + row as u16).map(|(_, h)| h), Some(Hit::Resident(i)), "row {row}");
    }
    // Ten rows of residents with their branches, six without: at 16 rows high only the six fit.
    let (rows, _) = side(&m, Rect::new(0, 0, 120, 16));
    assert!(!rows.iter().any(|r| r.contains('⎇')), "{rows:#?}");
    assert!(rows[6].contains("Chen"));
}

#[test]
fn a_tag_row_is_its_residents_with_or_without_a_branch() {
    let m = Model { focused: Some("id-Chen".into()), ..tagged() };
    let map = hits(&m, AREA);
    // As with branches, and Cirno's tags under it.
    for (row, i) in [0, 0, 3, 3, 4, 4, 1, 1, 2, 2, 5].into_iter().enumerate() {
        assert_eq!(map.at(5, 1 + row as u16).map(|(_, h)| h), Some(Hit::Resident(i)), "row {row}");
    }
    let (rows, _) = side(&m, Rect::new(0, 0, 120, 16));
    assert!(!rows.iter().any(|r| r.contains('=')), "{rows:#?}");
    // Tags never squeeze a branch below half the row: they are cut instead, the branch at its
    // middle.
    let (rows, _) = side(&m, AREA);
    assert!(rows[2].contains("⎇ pad…oard pr=#123…"), "{rows:#?}");
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
    m.modal = Some(Modal::Recall(Recall { list, selected: 15 }));
    let map = hits(&m, AREA);
    // A long list scrolls to keep the selection in view.
    assert!(resolves(&map, Hit::Item(15)));
    assert!(!map.0.iter().any(|(_, h)| *h == Hit::Item(0)));
}

#[test]
fn cast_offers_groups_then_the_living_and_never_the_target_as_peer() {
    let mut m = shrine();
    let Some(Modal::Cast(c)) = Some(cast(Some(1), None)) else { unreachable!() };
    let picks: Vec<String> = modal::cast_choices(&m, &c).into_iter().map(|c| c.0).collect();
    assert_eq!(picks, ["all", "awaiting", "idle", "id-Reimu", "id-Marisa"]);
    let Some(Modal::Cast(c)) = Some(cast(Some(0), None)) else { unreachable!() };
    let picks: Vec<String> = modal::cast_choices(&m, &c).into_iter().map(|c| c.0).collect();
    assert_eq!(picks, ["id-Reimu", "id-Marisa"]);
    let Some(Modal::Cast(c)) = Some(cast(Some(0), Some("id-Reimu"))) else { unreachable!() };
    let picks: Vec<String> = modal::cast_choices(&m, &c).into_iter().map(|c| c.0).collect();
    assert_eq!(picks, ["id-Marisa"]);
    // Marisa leaves while the modal is open: she is no longer offered.
    m.residents[1].departed = Some(NOW);
    assert!(modal::cast_choices(&m, &c).is_empty());
    m.modal = Some(Modal::Cast(c));
    let map = hits(&m, AREA);
    assert!(!map.0.iter().any(|(_, h)| matches!(h, Hit::Item(_))));
}

#[test]
fn the_timetable_lists_soonest_first_and_offers_what_each_ritual_allows() {
    let list = rituals();
    let names: Vec<&str> =
        modal::timetable_order(&list).into_iter().map(|i| list[i].name.as_str()).collect();
    assert_eq!(names, ["evening-notes", "slack-morning", "inbox-zero", "broken", "nightly-checks"]);
    assert_eq!(modal::when_short("2026-09-21 23:00", "2026-09-21"), "23:00");
    assert_eq!(modal::when_short("2026-09-22 09:05", "2026-09-21"), "Tue 09:05");
    assert_eq!(modal::when_short("2026-10-05 08:00", "2026-09-21"), "10-05 08:00");
    let mut m = shrine();
    m.rituals = Some(list);
    m.modal = Some(timetable(None, false));
    let map = hits(&m, AREA);
    for i in 0..5 {
        assert!(resolves(&map, Hit::Item(i)), "row {i}");
    }
    // The sidebar's next fire opens the timetable too.
    m.modal = None;
    let map = hits(&m, AREA);
    let n = map.0.iter().filter(|(_, h)| *h == Hit::Button(Button::Timetable)).count();
    assert_eq!(n, 2);
    for (open, remove) in [("slack-morning", true), ("nightly-checks", false)] {
        m.modal = Some(timetable(Some(open), false));
        let map = hits(&m, AREA);
        assert!(resolves(&map, Hit::Button(Button::RunRitual)), "{open}");
        assert!(resolves(&map, Hit::Button(Button::ToggleRitual)), "{open}");
        assert_eq!(resolves(&map, Hit::Button(Button::RemoveRitual)), remove, "{open}");
    }
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
            Run { col: 0, style: Style::default(), text: "ab".into(), link: None },
            Run { col: 5, style: red, text: "日x".into(), link: None },
            Run { col: 9, style: rgb, text: "z".into(), link: None },
        ]],
        cursor: None,
        ..Frame::default()
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
    // Scrolled back, the resident's cursor is somewhere below what is shown.
    m.modal = None;
    m.screen.as_mut().unwrap().back = 4;
    assert_eq!(render::cursor(&m, AREA), None);
    // At the search prompt, it is after what is typed, on the box's bottom edge.
    m.find = Some(Find { needle: String::new(), back: true, typing: Some("nee".into()) });
    let (t, _) = draw(&m, AREA.width, AREA.height);
    let (x, y) = render::cursor(&m, AREA).expect("a cursor");
    let buf = t.backend().buffer();
    let row: String = (0..x).map(|x| buf[(x, y)].symbol()).collect();
    assert!(row.ends_with("└ /nee"), "{row:?}");
}

#[test]
fn a_text_field_has_the_cursor_after_what_is_typed() {
    let fields = [
        (Stage::Dir, "  /Users/someone/dev/sc"),
        (Stage::Name, "name: Yuyu"),
        (Stage::Role, "your words: focus on the SQL layer"),
        (Stage::Worktree, "worktree: fix-login"),
    ];
    for (stage, typed) in fields {
        let mut m = shrine();
        m.modal = Some(summon(stage));
        let (t, _) = draw(&m, AREA.width, AREA.height);
        let (x, y) = render::cursor(&m, AREA).expect("a cursor");
        let buf = t.backend().buffer();
        let row: String = (0..x).map(|x| buf[(x, y)].symbol()).collect();
        assert!(row.ends_with(typed), "{row:?}");
        assert_eq!(buf[(x, y)].symbol(), " ");
    }
}

#[test]
fn a_path_longer_than_its_field_shows_its_end_and_the_cursor_after_it() {
    let mut m = shrine();
    let long = format!("{HOME}/{}/deep/end", "a-very-long-directory-name".repeat(4));
    let mut s = summon(Stage::Dir);
    let Modal::Summon(sm) = &mut s else { unreachable!() };
    (sm.path, sm.selected) = (long, None);
    m.modal = Some(s);
    for (w, h) in [(120, 40), (80, 24)] {
        let (t, _) = draw(&m, w, h);
        let area = Rect::new(0, 0, w, h);
        let (x, y) = render::cursor(&m, area).expect("a cursor");
        let buf = t.backend().buffer();
        let row: String = (0..x).map(|x| buf[(x, y)].symbol()).collect();
        assert!(row.ends_with("/deep/end") && row.contains('…'), "{row:?}");
        assert_eq!(buf[(x, y)].symbol(), " ");
    }
}

#[test]
fn tiny_screens_do_not_panic() {
    for (_, m) in screens() {
        for (w, h) in
            [(40, 10), (26, 3), (25, 5), (10, 3), (6, 40), (1, 1), (0, 0), (200, 2), (80, 1)]
        {
            let area = Rect::new(0, 0, w, h);
            render::render(&m, area, &mut Buffer::empty(area));
            render::cursor(&m, area);
        }
    }
}

#[test]
fn the_sidebar_shows_the_freshest_usage_and_none_past_its_reset() {
    let mut m = aware();
    let sidebar = |m: &Model| -> Vec<String> {
        let mut buf = Buffer::empty(AREA);
        render::render(m, AREA, &mut buf);
        (0..AREA.height)
            .map(|y| (1..render::SIDEBAR_W - 1).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    };
    let has = |m: &Model, s: &str| sidebar(m).iter().any(|l| l.starts_with(s));
    // Marisa, idle, reported last but from before Reimu's turns: Reimu's higher use is fresher.
    let r = &mut m.residents;
    r[1].telemetry.as_mut().unwrap().at = NOW;
    r[1].telemetry.as_mut().unwrap().five_hour.as_mut().unwrap().used = 20;
    assert!(has(&m, "5h ██▉░░░░░ 37%"), "{:#?}", sidebar(&m));
    // A window past its reset is not shown at its old use.
    for t in m.residents.iter_mut().filter_map(|r| r.telemetry.as_mut()) {
        t.five_hour.as_mut().unwrap().resets = Some(NOW - 1);
    }
    assert!(!has(&m, "5h"), "{:#?}", sidebar(&m));
    assert!(has(&m, "wk"));
}

#[test]
fn numbers_that_press_turn_yellow_or_red_and_a_cold_cache_says_so() {
    use ratatui::style::Color;
    let mut m = aware();
    m.residents[0].telemetry.as_mut().unwrap().ctx = Some(93);
    m.residents[0].telemetry.as_mut().unwrap().cache_warm = Some(false);
    let mut buf = Buffer::empty(AREA);
    render::render(&m, AREA, &mut buf);
    let row = |y: u16| -> String { (0..AREA.width).map(|x| buf[(x, y)].symbol()).collect() };
    let at = |p: &str| (0..AREA.height).find(|&y| row(y)[3..].starts_with(p)).unwrap();
    let fg = |x: u16, y: u16| buf[(x, y)].fg;
    // The week is 62% used with 55% of it gone: ahead of the clock. Five hours at 37% is calm.
    let (wk, five) = (at("wk "), at("5h "));
    assert_eq!((fg(4, wk), fg(4, five)), (Color::Yellow, Color::DarkGray));
    assert_eq!((fg(1, wk), fg(13, wk)), (Color::DarkGray, Color::DarkGray));
    // Reimu's context is nearly full; the rest of its line keeps its colour.
    let reimu = at("1 ");
    let end = render::SIDEBAR_W - 2;
    assert_eq!((fg(end, reimu), fg(end - 2, reimu)), (Color::LightRed, Color::LightRed));
    assert_ne!(fg(1, reimu), Color::LightRed);
    assert!(row(0).contains("91% cold"), "{}", row(0));
}

#[test]
fn whoever_needs_you_is_gold_across_the_sidebar() {
    let m = aware();
    let mut buf = Buffer::empty(AREA);
    render::render(&m, AREA, &mut buf);
    let gold = |y: u16| {
        (1..render::SIDEBAR_W - 1).all(|x| buf[(x, y)].bg == ratatui::style::Color::Yellow)
    };
    // Inside the box: Reimu busy and its branch, Marisa awaits, Cirno departed, Sakuya asked.
    assert_eq!([1, 2, 3, 4, 5].map(gold), [false, false, true, false, true]);
}

#[test]
fn a_narrow_sidebar_drops_the_rituals_time_rather_than_draw_over_its_name() {
    let mut m = shrine();
    m.rituals = Some(rituals());
    let row = |w: u16| {
        let area = Rect::new(0, 0, w, 24);
        let mut buf = Buffer::empty(area);
        render::render(&m, area, &mut buf);
        let line = |y: u16| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        (0..24).map(line).find(|l| l.contains('⏲')).unwrap_or_default()
    };
    assert_eq!(row(25).trim_end_matches('│'), "│⏲ evening-notes   23:00", "{}", row(25));
    assert_eq!(row(14), "│⏲ eve… 23:00│");
    let narrow = row(10);
    assert!(narrow.starts_with("│⏲ eveni…") && !narrow.contains("23"), "{narrow}");
}

#[test]
fn a_long_ritual_name_is_cut_with_an_ellipsis_in_the_timetable() {
    let mut m = shrine();
    let mut rs = rituals();
    rs[0].name = "a-ritual-name-past-twenty-columns".into();
    (m.rituals, m.modal) = (Some(rs), Some(timetable(None, false)));
    let t = modal_box(&m);
    let text: String = t.backend().buffer().content().iter().map(|c| c.symbol()).collect();
    assert!(text.contains("a-ritual-name-past-…"), "{text}");
}

#[test]
fn the_model_column_shows_only_while_models_differ() {
    // Haiku beside Opus: each row says which.
    let (rows, _) = side(&aware(), AREA);
    assert!(rows[1].ends_with("Hai  12%") && rows[3].ends_with("Opu   8%"), "{rows:#?}");
    // One model among the living, Cirno's from before it left not counted: the name has its
    // columns.
    let mut m = uniform();
    m.residents[1].name = "Marisa-Kirisame-of-the-Forest".into();
    let t = Telemetry { model: Some("Sonnet 5".into()), at: NOW, ..Telemetry::default() };
    m.residents[2].telemetry = Some(t);
    let (rows, _) = side(&m, AREA);
    let models = ["Hai", "Opu", "Son"];
    assert!(!rows[1..].iter().any(|r| models.iter().any(|s| r.contains(s))), "{rows:#?}");
    assert!(rows[1].starts_with("1 ● Reimu") && rows[1].ends_with(" 12%"), "{rows:#?}");
    assert_eq!(rows[3], "2 ✦ Marisa-Kirisam   8%");
}

#[test]
fn a_branch_that_repeats_the_name_is_left_off() {
    let mut m = shrine();
    m.residents[0].branch = Some("main".into());
    (m.residents[1].name, m.residents[1].branch) =
        ("openai_eu_cos".into(), Some("nebel95/openai-eu-cos".into()));
    m.residents[2].branch = Some("x".into());
    let (rows, _) = side(&m, AREA);
    // Reimu and its branch, openai_eu_cos with no row below it, Cirno and its branch.
    assert!(rows[2].contains("⎇ main") && rows[3].starts_with("2 ○ openai_eu_cos"), "{rows:#?}");
    assert!(rows[4].starts_with("3 · Cirno") && rows[5].contains("⎇ x"), "{rows:#?}");
    assert!(!rows.iter().any(|r| r.contains("openai-eu")), "{rows:#?}");
    // It no longer needs a row: five rows are enough for every branch row.
    let (rows, _) = side(&m, Rect::new(0, 0, 120, 13));
    assert!(rows[2].contains("⎇ main") && rows[5].contains("⎇ x"), "{rows:#?}");
    // On screen too, and its tags still shown, alone.
    m.focused = Some("id-Marisa".into());
    let (rows, _) = side(&m, AREA);
    assert!(rows[4].starts_with("3 · Cirno"), "{rows:#?}");
    m.residents[1].tags = vec![("pr".into(), "#9".into())];
    let (rows, _) = side(&m, AREA);
    assert_eq!(rows[4].trim(), "pr=#9", "{rows:#?}");
    // Read without case, after the last `/`.
    m.residents[0].branch = Some("someone/REIMU".into());
    let (rows, _) = side(&m, AREA);
    assert!(rows[2].starts_with("2 ○ openai_eu_cos"), "{rows:#?}");
}

#[test]
fn the_focused_name_wraps_over_rows_that_are_all_its() {
    let mut m = shrine();
    m.residents[0].name = "Reimu Hakurei of the Hakurei Shrine".into();
    let t =
        Telemetry { model: Some("Opus 5.5".into()), ctx: Some(12), at: NOW, ..Default::default() };
    m.residents[0].telemetry = Some(t);
    let (rows, buf) = side(&m, AREA);
    // One model, so the name has 14 columns beside the context, which stays on its first row.
    let top: Vec<&str> = rows[1..4].iter().map(|r| r.trim_end()).collect();
    assert_eq!(top, ["1 ○ Reimu Hakurei   12%", "    of the Hakurei", "    Shrine"], "{rows:#?}");
    assert!(rows[4].starts_with("2 ○ Marisa"), "{rows:#?}");
    let map = hits(&m, AREA);
    for y in 1..4 {
        assert_eq!(map.at(5, y).map(|(_, h)| h), Some(Hit::Resident(0)), "row {y}");
        let lit = (1..render::SIDEBAR_W - 1)
            .all(|x| buf[(x, y)].bg == ratatui::style::Color::Indexed(237));
        assert!(lit, "row {y}");
    }
    assert_eq!(map.at(5, 4).map(|(_, h)| h), Some(Hit::Resident(1)));
    // Behind the installed claude: its mark keeps its place beside the first row.
    m.residents[0].outdated = Some("2.1.0".into());
    let (rows, _) = side(&m, AREA);
    assert!(rows[1].starts_with("1 ○ Reimu ⇡"), "{rows:#?}");
    // Past three rows, the third ends in `…`.
    m.residents[0].name = "x".repeat(50);
    let (rows, _) = side(&m, AREA);
    assert!(rows[3].trim_end().ends_with('…') && rows[4].starts_with("2 ○ Marisa"), "{rows:#?}");
}

#[test]
fn the_focused_branch_shows_whole_and_a_leads_bar_runs_down_its_rows() {
    let m = long();
    let (rows, _) = side(&m, AREA);
    // Reimu's name over two rows, then its branch over as many as it takes, down to Youmu.
    assert!(rows[1].starts_with("1 ○ Reimu Hakurei of"), "{rows:#?}");
    assert_eq!(rows[2].trim_end(), "  │ the Hakurei Shrine");
    let youmu = rows.iter().position(|r| r.starts_with("4 ├")).unwrap();
    assert!((5..=6).contains(&youmu), "{rows:#?}");
    let whole = joined(&rows[3..youmu]);
    assert!(whole.contains("⎇padlet-ai-summary-for-every-board"), "{rows:#?}");
    assert!(!whole.contains('…') && !whole.contains("nebel95"), "{rows:#?}");
    let map = hits(&m, AREA);
    for y in 1..youmu {
        assert_eq!(map.at(5, y as u16).map(|(_, h)| h), Some(Hit::Resident(0)), "row {y}");
        if y > 1 {
            assert_eq!(rows[y].chars().nth(2), Some('│'), "{rows:#?}");
        }
    }
}

#[test]
fn the_focused_resident_is_drawn_whole_only_while_it_fits() {
    let at = |h| side(&branches(), Rect::new(0, 0, 120, h)).0;
    let has = |rows: &[String], s: &str| rows.iter().any(|r| r.contains(s));
    // Every branch row fits but not Reimu's whole: as before, its branch cut in the middle.
    let rows = at(18);
    assert!(has(&rows, "⎇ padlet-a…ry-board") && has(&rows, "⎇ main"), "{rows:#?}");
    // Not every branch row: one row each, Reimu's whole below it.
    let rows = at(17);
    assert!(
        !has(&rows, "⎇ fix-ci") && !has(&rows, "⎇ main") && has(&rows, "6 ○ Chen"),
        "{rows:#?}"
    );
    let youmu = rows.iter().position(|r| r.starts_with("4 ├")).unwrap();
    assert!(joined(&rows[2..youmu]).contains("⎇padlet-ai-summary-for-every-board"), "{rows:#?}");
    // Not that either: one row each, as before.
    let rows = at(15);
    assert!(!has(&rows, "⎇") && has(&rows, "6 ○ Chen"), "{rows:#?}");
    // Not even one row each: Reimu on one row among those that fit.
    let rows = at(12);
    assert!(!has(&rows, "⎇") && has(&rows, "1 ○ Reimu") && has(&rows, "more"), "{rows:#?}");
}

#[test]
fn a_message_breaks_at_spaces_and_keeps_a_word_that_fits_whole() {
    let mut m = shrine();
    m.message = Some(Message::new(Say::Error, "Duel needs a peer: --with <name>"));
    let (rows, _) = side(&m, AREA);
    assert!(rows.iter().any(|r| r.trim() == "--with <name>"), "{rows:#?}");
    assert!(!rows.iter().any(|r| r.trim_end().ends_with("--")), "{rows:#?}");
}

#[test]
fn a_focused_branch_too_long_for_its_rows_leaves_the_last_to_its_tags() {
    let mut m = long();
    m.residents[0].branch = Some(format!("nebel95/{}", "summary-for-every-board-".repeat(4)));
    m.residents[0].tags = vec![("pr".into(), "#12".into())];
    let (rows, _) = side(&m, AREA);
    let youmu = rows.iter().position(|r| r.starts_with("4 ├")).unwrap();
    assert_eq!(rows[youmu - 1].trim_end(), "  │ pr=#12", "{rows:#?}");
    assert!(rows[youmu - 2].trim_end().ends_with('…'), "{rows:#?}");
}

#[test]
fn a_dim_row_of_wide_letters_is_cut_by_columns() {
    let mut m = branches();
    m.residents[1].branch = Some("機能追加の仕事をする".into());
    m.residents[1].tags = vec![("pr".into(), "#12".into()), ("ci".into(), "green".into())];
    let (rows, _) = side(&m, Rect::new(0, 0, 120, 30));
    let marisa = rows.iter().position(|r| r.starts_with("2 ○ Marisa")).unwrap();
    let dim = rows[marisa + 1].trim_end();
    assert!(dim.contains("pr=#12") && dim.ends_with('…'), "{rows:#?}");
}

#[test]
fn a_cloud_session_is_drawn_as_a_cloud_and_departed_as_anyone() {
    let mut m = shrine();
    let mut aya = resident(4, "Aya", "work", None);
    (aya.pid, aya.cloud) = (None, Some("https://claude.ai/code/session_01X".into()));
    m.residents.push(aya.clone());
    let (rows, _) = side(&m, AREA);
    assert!(rows.iter().any(|r| r.contains("4 ☁ Aya")), "{rows:#?}");
    m.residents[3] = Resident { departed: Some(NOW), state: State::Departed, ..aya };
    let (rows, _) = side(&m, AREA);
    assert!(rows.iter().any(|r| r.contains("4 · Aya")), "{rows:#?}");
}
