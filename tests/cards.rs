//! Spell cards on disk and as typed: loading, naming, placeholders and the bytes a resident gets.

mod common;

use gensokyo::card::{fill, find_card, load, needle, parse, shown, typed};
use std::path::PathBuf;

fn dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let d = common::fresh(&format!("cards-{name}"));
    for (f, text) in files {
        std::fs::write(d.join(f), text).unwrap();
    }
    d
}

#[test]
fn cards_load_by_title_with_the_users_shadowing_the_shipped_and_bad_names_named() {
    let mine =
        dir("mine", &[("wrap-up.md", "---\ntitle: Mine\n---\nmy wrap"), ("bad name.md", "x")]);
    let shipped = dir(
        "shipped",
        &[
            ("wrap-up.md", "---\ntitle: Theirs\n---\ntheir wrap"),
            (
                "a.md",
                "---\ntitle: Zed\nsummary: last: by title\npeer: required\n---\n\n\nhi {peer}\n\n",
            ),
            ("notes.txt", "not a card"),
            (".hidden.md", "not a card either"),
        ],
    );
    let (cards, bad) = load(&[mine.clone(), shipped.join("missing"), shipped]);
    let titles: Vec<&str> = cards.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["Mine", "Zed"]);
    assert_eq!(cards[0].body, "my wrap");
    let z = &cards[1];
    assert_eq!((z.slug.as_str(), z.summary.as_str(), z.pair), ("a", "last: by title", true));
    assert_eq!(z.body, "hi {peer}", "blank lines after the fence and at the end go");
    assert_eq!(bad, [mine.join("bad name.md")]);
}

#[test]
fn a_file_without_a_fence_is_all_body_and_is_titled_by_its_name() {
    let c = parse("tea", "Make tea.\n---\nnot frontmatter\n");
    assert_eq!(
        (c.title.as_str(), c.body.as_str(), c.pair),
        ("tea", "Make tea.\n---\nnot frontmatter", false)
    );
    let c = parse("empty", "---\ntitle: Only a title\n---\n");
    assert_eq!((c.title.as_str(), c.body.as_str()), ("Only a title", ""));
    let c = parse("open", "---\ntitle: Never closed\n");
    assert_eq!((c.title.as_str(), c.body.as_str()), ("Never closed", ""));
}

#[test]
fn a_card_is_found_by_slug_or_title_or_by_a_part_that_picks_out_one() {
    let cards = vec![
        parse("status-report", "---\ntitle: Spirit Sign \"Status Report\"\n---\nx"),
        parse("sync-up", "---\ntitle: Border Sign \"Sync Up\"\n---\nx"),
        parse("sync", "---\ntitle: Plain\n---\nx"),
    ];
    let slug = |w: &str| find_card(&cards, w).map(|c| c.slug.clone());
    assert_eq!(slug("STATUS-REPORT").unwrap(), "status-report");
    assert_eq!(slug("border sign \"sync up\"").unwrap(), "sync-up");
    assert_eq!(slug("report").unwrap(), "status-report");
    // Exact wins over a part that would also match another.
    assert_eq!(slug("sync").unwrap(), "sync");
    assert!(slug("sign").unwrap_err().contains("could be status-report, sync-up"));
    assert!(slug("nothing").unwrap_err().starts_with("no spell card 'nothing'"));
}

#[test]
fn placeholders_are_filled_once_and_unknown_braces_stay() {
    let vals =
        [("self", "Reimu"), ("peer", "{self}"), ("cwd", "/w/{residents}"), ("residents", "nobody")];
    assert_eq!(
        fill("{self} asks {peer} in {cwd}; others: {residents}. {other} {self", &vals),
        "Reimu asks {self} in /w/{residents}; others: nobody. {other} {self"
    );
    assert_eq!(fill("{}{{self}}", &vals), "{}{Reimu}");
}

#[test]
fn the_needle_is_the_start_of_the_last_line_and_counts_rows_that_show_it() {
    assert_eq!(
        needle("first\n\n  do not start anything new, and so on  \n\n"),
        "do not start anythin"
    );
    assert_eq!(needle("short"), "short");
    let rows = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(shown(&rows(&["> short", "[Pasted text #1 +3 lines]", "other"]), "short"), 2);
}

#[test]
fn a_card_is_typed_without_controls_and_bracketed_only_when_asked() {
    let t = "one\x1b[201~\x07\ntwo\tend";
    assert_eq!(typed(t, false), b"one[201~\rtwo\tend");
    assert_eq!(typed(t, true), b"\x1b[200~one[201~\rtwo\tend\x1b[201~");
}
