//! The PR watcher's probe, `share/probes/gh-prs`, against a stand-in `gh` that answers with
//! recorded GraphQL replies (`tests/pr-watch/`): the same PRs must print the same bytes, a real
//! change must not, and a failure prints the last list with a sentence.

mod common;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

fn here() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).into()
}

/// gh-prs with the stand-in first on PATH, `last` as its previous output, and `env` on top.
fn probe(answer: &str, last: Option<&Path>, env: &[(&str, &str)]) -> String {
    let dir = here().join("tests/pr-watch");
    let mut c = Command::new(here().join("share/probes/gh-prs"));
    c.env_clear()
        .env("PATH", format!("{}:{}", dir.display(), std::env::var("PATH").unwrap()))
        .env("HOME", std::env::var("HOME").unwrap())
        .env("STUB_GH_ANSWER", dir.join(format!("answer-{answer}.json")))
        .env("GENSOKYO_PROBE_LAST", last.map(|p| p.display().to_string()).unwrap_or_default())
        .envs(env.iter().copied());
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}

fn saved(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

#[test]
fn the_same_prs_print_the_same_bytes_and_a_change_does_not() {
    let dir = common::fresh("gh-prs");
    let a = probe("a", None, &[]);
    let last = saved(&dir, "a.out", &a);
    // Reordered, a bot comment later, and one mergeable gone UNKNOWN while GitHub recomputes.
    assert_eq!(probe("b", Some(&last), &[]), a);
    // Checks that now pass are a change.
    assert_ne!(probe("c", Some(&last), &[]), a);

    assert!(a.ends_with("}\n") && a.lines().count() == 1, "{a}");
    let v: Value = serde_json::from_str(&a).unwrap();
    let today =
        String::from_utf8(Command::new("date").arg("+%F").output().unwrap().stdout).unwrap();
    assert_eq!(v["as_of"], today.trim());
    assert!(v.get("failure").is_none());
    let order: Vec<String> = v["prs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| format!("{}#{}", p["repo"].as_str().unwrap(), p["number"]))
        .collect();
    assert_eq!(
        order,
        ["example/alpha#7", "example/alpha#12", "example/beta#3", "example/gamma#40"]
    );
    assert_eq!(
        v["prs"][1],
        json!({"repo": "example/alpha", "number": 12, "title": "Add retry to the uploader",
               "url": "https://github.com/example/alpha/pull/12", "ci": "FAILURE",
               "review": "CHANGES_REQUESTED", "mergeable": "MERGEABLE", "draft": false})
    );
    assert_eq!((&v["prs"][0]["draft"], &v["prs"][3]["ci"]), (&json!(true), &Value::Null));
    assert!(!a.contains("updatedAt") && !a.contains("2026-10-06T"), "{a}");

    // UNKNOWN with nothing to carry forward stays UNKNOWN.
    let b = probe("b", None, &[]);
    assert!(b.contains("\"mergeable\":\"UNKNOWN\""), "{b}");
}

#[test]
fn a_failure_prints_the_last_list_with_a_sentence_and_the_same_one_twice() {
    let dir = common::fresh("gh-prs-fail");
    let a = probe("a", None, &[]);
    let last = saved(&dir, "a.out", &a);
    let down = probe("a", Some(&last), &[("STUB_GH_DOWN", "1")]);
    let v: Value = serde_json::from_str(&down).unwrap();
    assert_eq!(v["failure"], "GitHub could not be reached");
    let mut was: Value = serde_json::from_str(&a).unwrap();
    was["failure"] = v["failure"].clone();
    assert_eq!(v, was, "the last list, kept as it was");
    // Failing the same way prints the same bytes, so it fires once.
    let again = saved(&dir, "down.out", &down);
    assert_eq!(probe("a", Some(&again), &[("STUB_GH_DOWN", "1")]), down);
    // Back up: the failure goes.
    assert_eq!(probe("a", Some(&again), &[]), a);

    let out = probe("a", Some(&last), &[("STUB_GH_LOGGED_OUT", "1")]);
    assert!(out.contains("\"failure\":\"gh is not logged in (gh auth login)\""), "{out}");
    // Nothing to fall back on: no PRs, and the reason.
    let v: Value = serde_json::from_str(&probe("a", None, &[("STUB_GH_DOWN", "1")])).unwrap();
    assert_eq!((&v["prs"], &v["failure"]), (&json!([]), &json!("GitHub could not be reached")));
    // A last output that is not a list is no fallback either.
    let junk = saved(&dir, "junk.out", "not json\n");
    let v: Value = serde_json::from_str(&probe("a", Some(&junk), &[])).unwrap();
    assert_eq!(v["prs"].as_array().unwrap().len(), 4);
}
