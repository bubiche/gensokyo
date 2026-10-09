//! Tags: short `key=value` tokens on a resident, set by the user or by the resident itself or
//! its lead, kept on its record through a banish, a resume and a restart, and shown in `list`.

mod common;

use common::{Daemon, err, fresh, out, wait};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Output, Stdio};

fn daemon(name: &str) -> Daemon {
    let d = Daemon::at(fresh(&format!("tg-{name}")), vec![("STUB_HOOKS".into(), "1".into())]);
    d.cli(&["list"]);
    d
}

fn summon(d: &Daemon, name: &str) -> Value {
    let r = d.req(json!({"t": "summon", "id": 7, "cwd": d.dir, "name": name}));
    assert_eq!(r["t"], "summoned", "{r}");
    d.stub(&r["resident"]["id"], "ready");
    r["resident"].clone()
}

/// `gensokyo tag <args>` as the user: its line, or why it failed.
fn tag(d: &Daemon, args: &[&str]) -> Result<String, String> {
    let o = d.command(&[&["tag"], args].concat()).stdin(Stdio::null()).output().unwrap();
    match o.status.success() {
        true => Ok(out(&o).trim().to_string()),
        false => Err(err(&o)),
    }
}

/// `gensokyo <args>` from inside resident `me`.
fn inside(d: &Daemon, me: &Value, args: &[&str]) -> Output {
    let mut c = d.command(args);
    c.env("GENSOKYO_SOCKET", d.socket()).env("GENSOKYO_RESIDENT", me["id"].as_str().unwrap());
    c.current_dir(&d.dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut c = c.spawn().unwrap();
    if args.contains(&"-") {
        c.stdin.take().unwrap().write_all(b"brief\n").unwrap();
    }
    drop(c.stdin.take());
    c.wait_with_output().unwrap()
}

/// Its tags as `list --json --all` gives them, in their order.
fn tags(d: &Daemon, name: &str) -> Vec<(String, String)> {
    let raw = out(&d.cli(&["list", "--json", "--all"]));
    let all: Vec<Value> = serde_json::from_str(&raw).unwrap();
    let r = all.iter().find(|r| r["name"] == name).unwrap_or_else(|| panic!("{name} in {all:?}"));
    let Some(obj) = r["tags"].as_object() else { return vec![] };
    // In the order the daemon wrote them, which `Value` sorts: from its own object in the text.
    let at = raw.find(&format!("\"id\":\"{}\"", r["id"].as_str().unwrap())).unwrap();
    let mine = &raw[at..];
    let mine = &mine[..mine[1..].find("\"id\":").map_or(mine.len(), |n| n + 1)];
    let body = mine.split("\"tags\":{").nth(1).unwrap().split('}').next().unwrap();
    let kv: Vec<(String, String)> = body
        .split(',')
        .map(|p| {
            let (k, v) = p.split_once("\":\"").unwrap();
            (k.trim_matches('"').to_string(), v.trim_matches('"').to_string())
        })
        .collect();
    assert_eq!(kv.len(), obj.len(), "{body}");
    kv
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn tags_are_set_in_order_changed_in_place_removed_and_cleared() {
    let d = daemon("set");
    summon(&d, "Reimu");
    assert_eq!(tag(&d, &["Reimu"]).unwrap(), "Reimu has no tags");
    let said = tag(&d, &["Reimu", "pr=#123", "ci=green"]).unwrap();
    assert_eq!(said, "Reimu: pr=#123 ci=green");
    assert_eq!(tags(&d, "Reimu"), pairs(&[("pr", "#123"), ("ci", "green")]));
    // Changed in place; a key that is not there removed is nothing.
    tag(&d, &["Reimu", "ci=red", "nope="]).unwrap();
    assert_eq!(tags(&d, "Reimu"), pairs(&[("pr", "#123"), ("ci", "red")]));
    // Removed, then set again: at the end.
    tag(&d, &["Reimu", "pr="]).unwrap();
    tag(&d, &["Reimu", "pr=v2.0", "phase=review"]).unwrap();
    assert_eq!(tags(&d, "Reimu"), pairs(&[("ci", "red"), ("pr", "v2.0"), ("phase", "review")]));
    // A slot names it too; --clear with new ones clears first.
    assert_eq!(tag(&d, &["1", "--clear", "x=1"]).unwrap(), "Reimu: x=1");
    assert_eq!(tag(&d, &["Reimu", "--clear"]).unwrap(), "Reimu has no tags");
    let o = d.cli(&["list", "--json"]);
    assert!(!out(&o).contains("\"tags\""), "no tags, no field: {}", out(&o));
    // Not a bell, not gold: a tag is not news.
    let r = &d.list()[0];
    assert_eq!((r["turns"].as_u64(), r["needs"].as_u64()), (Some(0), Some(0)), "{r}");
}

#[test]
fn only_short_tokens_are_tags_and_a_refused_change_changes_nothing() {
    let d = daemon("valid");
    summon(&d, "Reimu");
    tag(&d, &["Reimu", "a=1", "b=2", "c=3"]).unwrap();
    let bad: &[(&[&str], &str)] = &[
        (&["Reimu", "d=4", "e=5"], "at most 4"),
        (&["Reimu", "PR=1"], "PR"),
        (&["Reimu", "thirteen-char=1"], "thirteen-char"),
        (&["Reimu", "=1"], "a key"),
        (&["Reimu", "note=two words"], "note"),
        (&["Reimu", "note=0123456789012345678901234"], "note"),
        (&["Reimu", "note"], "key=value"),
        (&["Nobody", "a=1"], "Nobody"),
    ];
    for (args, says) in bad {
        let e = tag(&d, args).unwrap_err();
        assert!(e.contains(says), "{args:?}: {e}");
        assert_eq!(tags(&d, "Reimu"), pairs(&[("a", "1"), ("b", "2"), ("c", "3")]), "{args:?}");
    }
    // Four, and a key's change is no new one; a removal makes room in the same go.
    tag(&d, &["Reimu", "d=4", "a=9"]).unwrap();
    tag(&d, &["Reimu", "a=", "e_-0=x.y:z/#@-"]).unwrap();
    let four = pairs(&[("b", "2"), ("c", "3"), ("d", "4"), ("e_-0", "x.y:z/#@-")]);
    assert_eq!(tags(&d, "Reimu"), four);
    // The daemon checks too: a resident can send what the CLI never would.
    let r = d.req(json!({"t": "tag", "id": 3, "who": "Reimu", "set": ["b=has space"]}));
    assert_eq!(r["t"], "error", "{r}");
    assert_eq!(tags(&d, "Reimu"), four);
}

#[test]
fn a_resident_tags_itself_and_its_helpers_and_reads_anyone() {
    let d = daemon("rights");
    let reimu = summon(&d, "Reimu");
    let marisa = summon(&d, "Marisa");
    let o = inside(&d, &reimu, &["new", "--json", "--name", "Youmu", "--prompt-file", "-"]);
    assert!(o.status.success(), "{}", err(&o));
    let youmu: Value = serde_json::from_str(&out(&o)).unwrap();
    d.stub(&youmu["id"], "ready");
    for who in ["Reimu", "Youmu"] {
        let o = inside(&d, &reimu, &["tag", who, "by=reimu"]);
        assert!(o.status.success(), "{who}: {}", err(&o));
        assert_eq!(tags(&d, who), pairs(&[("by", "reimu")]));
    }
    tag(&d, &["Marisa", "mine=1"]).unwrap();
    for args in [&["tag", "Marisa", "by=reimu"][..], &["tag", "Marisa", "--clear"]] {
        let o = inside(&d, &reimu, args);
        assert!(!o.status.success());
        assert!(err(&o).contains("Marisa is not you or one of your helpers"), "{}", err(&o));
    }
    // The helper may not tag its lead; anyone may read.
    let o = inside(&d, &youmu, &["tag", "Reimu", "x=1"]);
    assert!(!o.status.success(), "{}", out(&o));
    let o = inside(&d, &reimu, &["tag", "Marisa"]);
    assert_eq!(out(&o).trim(), "Marisa: mine=1", "{}", err(&o));
    assert_eq!(tags(&d, "Marisa"), pairs(&[("mine", "1")]));
    // Its helper by slot, and departed, and cleared; Marisa by slot is still not its own.
    let slot = |n: &str| d.list().iter().find(|r| r["name"] == n).unwrap()["slot"].to_string();
    let o = inside(&d, &reimu, &["tag", &slot("Youmu"), "at=slot"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(!inside(&d, &reimu, &["tag", &slot("Marisa"), "x=1"]).status.success());
    d.cli(&["banish", "Youmu"]);
    wait(|| d.list().iter().any(|r| r["name"] == "Youmu" && r["state"] == "departed"), "Youmu");
    let o = inside(&d, &reimu, &["tag", "Youmu", "--clear", "gone=1"]);
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(tags(&d, "Youmu"), pairs(&[("gone", "1")]));
    let _ = marisa;
}

#[test]
fn a_key_twice_in_a_record_is_one_tag_with_the_last_value() {
    #[derive(serde::Deserialize)]
    struct R {
        #[serde(with = "gensokyo::proto::pairs")]
        tags: gensokyo::proto::Tags,
    }
    let r: R = serde_json::from_str(r#"{"tags": {"a": "1", "b": "2", "a": "3"}}"#).unwrap();
    assert_eq!(r.tags, pairs(&[("a", "3"), ("b", "2")]));
}

#[test]
fn tags_are_kept_through_a_banish_a_resume_and_a_restart() {
    let d = daemon("kept");
    summon(&d, "Reimu");
    tag(&d, &["Reimu", "pr=42", "ci=green"]).unwrap();
    d.cli(&["banish", "Reimu"]);
    wait(|| d.list()[0]["state"] == "departed", "Reimu to depart");
    let kept = pairs(&[("pr", "42"), ("ci", "green")]);
    assert_eq!(tags(&d, "Reimu"), kept);
    // Departed, it can still be tagged.
    tag(&d, &["Reimu", "ci=red"]).unwrap();
    d.cli(&["resume", "Reimu"]);
    let kept = pairs(&[("pr", "42"), ("ci", "red")]);
    assert_eq!(tags(&d, "Reimu"), kept);
    d.cli(&["restart"]);
    wait(|| d.list().first().is_some_and(|r| r["state"] != "departed"), "Reimu back");
    assert_eq!(tags(&d, "Reimu"), kept);
    // And in departed/, where a closed one's record goes.
    d.cli(&["banish", "Reimu"]);
    wait(|| d.list()[0]["state"] == "departed", "Reimu to depart");
    d.cli(&["close", "Reimu"]);
    assert!(d.list().is_empty());
    assert_eq!(tags(&d, "Reimu"), kept);
}
