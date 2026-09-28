//! The frontmatter both rituals and spell cards are written in, and the files that ship.

use gensokyo::card;
use gensokyo::cron::Schedule;
use gensokyo::frontmatter::{items, name_ok, quote, read, value};
use gensokyo::ritual::{self, Trust};
use std::path::Path;

#[test]
fn a_list_splits_only_on_commas_outside_quotes_and_brackets() {
    let cases: &[(&str, &[&str])] = &[
        (r#"["Bash(ls [ab])", "Read"]"#, &["Bash(ls [ab])", "Read"]),
        (r#"["Bash(git commit -m \"a, b\")", "Read"]"#, &[r#"Bash(git commit -m "a, b")"#, "Read"]),
        (r#"Read, "Bash(x, y)""#, &["Read", "Bash(x, y)"]),
        ("Bash(x, y), Read # the two", &["Bash(x, y)", "Read"]),
        ("[Bash(ls [ab]), Read]   # unquoted", &["Bash(ls [ab])", "Read"]),
        ("Bash(don't), 'it''s', Read", &["Bash(don't)", "it's", "Read"]),
        (r#""Bash(git log:*)", Read"#, &["Bash(git log:*)", "Read"]),
        ("mcp__claude_ai_Slack__*#x", &["mcp__claude_ai_Slack__*#x"]),
        ("[]", &[]),
        ("  ", &[]),
        ("a,, b,", &["a", "b"]),
    ];
    for (v, want) in cases {
        assert_eq!(items(v), *want, "{v}");
    }
}

#[test]
fn a_value_is_what_is_inside_its_quotes_or_before_a_comment() {
    let cases = [
        (r#" "say \"hi\"""#, r#"say "hi""#),
        (" 'it''s'", "it's"),
        (r#" "C:\x\\y""#, r"C:\x\y"),
        (r#" "3 9 * * 1-5"   # weekdays"#, "3 9 * * 1-5"),
        (" the morning's mail # sorted", "the morning's mail"),
        (" last: by title", "last: by title"),
        (" # only a comment", ""),
        (r#" "never closed"#, "never closed"),
        (" a#b", "a#b"),
    ];
    for (v, want) in cases {
        assert_eq!(value(v), want, "{v}");
    }
    for v in [r#"a "b" it's C:\x # not a comment"#, "plain", "'", r#"\""#, ""] {
        assert_eq!(value(&quote(v).unwrap()), v, "{v}");
    }
    assert!(quote("two\nlines").is_err());
}

#[test]
fn the_block_reads_crlf_blocks_repeats_and_strays() {
    let f = read(
        "---\r\n# c\r\ntitle: A\r\nlist:\r\n  - one\r\n\r\n  - \"two, three\"\r\ntitle: B\r\n\
         schedul: x\r\npeer required\r\n: nothing\r\n---\r\n\r\nbody\r\nline\r\n\r\n",
        &["title", "list"],
    );
    let keys: Vec<&str> = f.fields.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, ["title", "list", "title", "schedul"]);
    assert_eq!(f.fields[1].list(), ["one", "two, three"]);
    assert_eq!(f.fields[2].text(), "B");
    assert_eq!(f.unknown, ["schedul", "peer required", ": nothing"]);
    assert_eq!(f.body, "body\nline");
    // A `- ` line under a value is not an item of it.
    let f = read("---\nlist: a\n- b\n---\n", &["list"]);
    assert_eq!((f.fields[0].list(), f.unknown), (vec!["a".to_string()], vec!["- b".to_string()]));
    let f = read("no fence\n---\nx: y\n", &[]);
    assert_eq!((f.fields.len(), f.body.as_str()), (0, "no fence\n---\nx: y"));
}

#[test]
fn a_name_starts_with_a_letter_or_digit() {
    for ok in ["a", "9am", "slack-morning", "a.b_c-d"] {
        assert!(name_ok(ok), "{ok}");
    }
    for bad in ["", "-x", "_x", ".x", "a b", "a/b", "é"] {
        assert!(!name_ok(bad), "{bad}");
    }
}

#[test]
fn a_card_with_a_line_that_means_nothing_is_listed_as_such_and_not_cast() {
    let c = card::parse("pair", "---\ntitle: \"Pair\"   # quoted\npear: required\n---\nhi {peer}");
    assert_eq!((c.title.as_str(), c.pair), ("Pair", false));
    assert_eq!(c.problem.as_deref(), Some("not a card setting: pear"));
    assert_eq!(c.listed().summary, "not cast: not a card setting: pear");
    let c = card::parse("pair", "---\r\ntitle: Pair\r\npeer: \"required\"\r\n---\r\nhi {peer}\r\n");
    assert_eq!((c.pair, c.problem.clone(), c.body.as_str()), (true, None, "hi {peer}"));
    let c = card::parse("pair", "---\npeer: yes\n---\nx");
    assert!(c.problem.unwrap().starts_with("peer: yes"));
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("fm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("_x.md"), "x").unwrap();
    let (cs, bad) = card::load(std::slice::from_ref(&d));
    assert_eq!((cs.len(), bad), (0, vec![d.join("_x.md")]));
}

#[test]
fn every_shipped_card_and_ritual_reads_cleanly() {
    let share = Path::new(env!("CARGO_MANIFEST_DIR")).join("share");
    let (cs, bad) = card::load(&[share.join("spellcards")]);
    assert!(bad.is_empty() && cs.len() >= 4, "{bad:?}");
    for c in &cs {
        assert_eq!(c.problem, None, "{}", c.slug);
        assert!(!c.summary.is_empty() && !c.body.is_empty(), "{}", c.slug);
    }
    let (rs, bad) = ritual::load(&[share.join("rituals")], Some(&share));
    assert!(bad.is_empty() && rs.len() >= 3, "{bad:?}");
    let trust = Trust::from_json(None);
    for r in &rs {
        assert!(r.shipped && r.unknown.is_empty(), "{}: {:?}", r.slug, r.unknown);
        assert!(!r.enabled(), "{} ships paused", r.slug);
        Schedule::parse(&r.schedule).unwrap();
        assert!(r.allowed_tools.iter().all(|t| !t.contains('#')), "{:?}", r.allowed_tools);
        // Only the placeholder directory is wrong with one: it is the user's to change.
        let p = ritual::problem(r, 1_790_000_000, &jiff::tz::TimeZone::UTC, &trust);
        assert!(p.as_deref().is_none_or(|p| p.starts_with("cwd: ")), "{}: {p:?}", r.slug);
    }
}
