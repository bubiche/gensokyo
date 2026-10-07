//! The record store on its own: what leaves `departed/` and `answers/` for good, and finding a
//! departed record.

mod common;

use common::fresh;
use gensokyo::daemon::store::{self, DEPARTED_KEPT, RUNS_KEPT, Record, Store};
use serde_json::{Value, json};

const DAY: i64 = 86400;

fn record(id: &str, departed: i64, extra: Value) -> Record {
    let mut r = json!({"id": id, "session": id, "name": id, "slot": null, "cwd": "/",
                       "program": "claude", "argv": [], "launched": departed - 10,
                       "departed": departed});
    r.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    serde_json::from_value(r).unwrap()
}

fn at(name: &str) -> Store {
    let root = fresh(name);
    for d in ["residents", "departed", "answers"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    Store::new(root)
}

/// `r` in `departed/` with an answer, as the daemon before this one left it.
fn left(s: &Store, r: &Record) {
    let p = s.departed().join(format!("{}.json", r.id));
    std::fs::write(p, serde_json::to_vec(r).unwrap()).unwrap();
    std::fs::write(s.answer(&r.id), "{}").unwrap();
}

/// Whether `id`'s record is kept, and its answer with it.
fn kept(s: &Store, id: &str) -> bool {
    let (rec, answer) = (s.departed().join(format!("{id}.json")).exists(), s.answer(id).exists());
    assert_eq!(rec, answer, "{id}: the record and its answer go together");
    rec
}

#[test]
fn departed_records_go_once_both_old_and_past_the_newest_kept() {
    let s = at("store-cap");
    let now = store::now();
    // The newest DEPARTED_KEPT stay however old.
    for i in 0..DEPARTED_KEPT {
        left(&s, &record(&format!("n{i}"), now - 40 * DAY + i as i64, json!({})));
    }
    let past = now - 60 * DAY;
    left(&s, &record("old", now - 50 * DAY, json!({})));
    left(&s, &record("session", past, json!({"ritual": "daily"})));
    let dir = gensokyo::ritual::Dir::at(&s.root, "daily");
    std::fs::create_dir_all(&dir.path).unwrap();
    dir.set_session("session").unwrap();
    left(&s, &record("run", past, json!({"ritual": "daily"})));
    left(&s, &record("untold", past, json!({"owner": "n0"})));
    left(&s, &record("told", past, json!({"owner": "n0", "told_gone": true})));
    left(&s, &record("leadless", past, json!({"owner": "nobody"})));
    s.sweep();
    assert!((0..DEPARTED_KEPT).all(|i| kept(&s, &format!("n{i}"))));
    let stays = [
        ("old", false),
        ("session", true),
        ("run", false),
        ("untold", true),
        ("told", false),
        ("leadless", false),
    ];
    for (id, want) in stays {
        assert_eq!(kept(&s, id), want, "{id}");
    }
    assert!(s.load_departed_id("old").is_none());
}

#[test]
fn a_recent_record_stays_past_the_newest_kept() {
    let s = at("store-recent");
    let now = store::now();
    for i in 0..DEPARTED_KEPT {
        left(&s, &record(&format!("n{i}"), now - 60 + i as i64, json!({})));
    }
    left(&s, &record("fresh", now - 2 * DAY, json!({})));
    s.sweep();
    assert!(kept(&s, "fresh"));
}

#[test]
fn answers_go_with_their_records_and_alone_at_a_sweep() {
    let s = at("store-answers");
    let now = store::now();
    // A ritual's oldest run past RUNS_KEPT goes when the next one retires, its answer too.
    for i in 0..=RUNS_KEPT {
        let r = record(&format!("run{i}"), now - 1000 + i as i64, json!({"ritual": "often"}));
        std::fs::write(s.answer(&r.id), "{}").unwrap();
        s.retire(&r).unwrap();
    }
    assert!(!kept(&s, "run0"));
    assert!(kept(&s, "run1"));
    // An answer whose record is nowhere goes at a sweep; one of a resident here stays.
    s.save(&record("here", now, json!({}))).unwrap();
    for id in ["here", "stray"] {
        std::fs::write(s.answer(id), "{}").unwrap();
    }
    s.sweep();
    assert!(s.answer("here").exists());
    assert!(!s.answer("stray").exists());
}

#[test]
fn a_departed_record_is_found_by_id_first_then_the_newest_by_name() {
    let s = at("store-find");
    let mut a = record("aaa", 100, json!({"name": "Marisa"}));
    left(&s, &a);
    left(&s, &record("bbb", 200, json!({"name": "Marisa"})));
    left(&s, &record("marisa", 50, json!({"name": "Alice"})));
    assert_eq!(s.find_departed("marisa").unwrap().id, "marisa");
    assert_eq!(s.find_departed("MARISA").unwrap().id, "bbb");
    assert_eq!(s.find_departed("aaa").unwrap().id, "aaa");
    // A change to one is seen at once.
    a.name = "Patchouli".into();
    s.save_departed(&a).unwrap();
    assert_eq!(s.find_departed("patchouli").unwrap().id, "aaa");
    assert!(s.find_departed("nobody").is_none());
}
