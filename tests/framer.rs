//! The host-input framer against byte fixtures. Each case is `fixtures/c1/<case>.jsonl` (`in`
//! is one read from the host, `out` a query the client wrote to it, bytes base64 in `b`) plus
//! `<case>.expected`, one rendered chunk per line.

mod common;

use common::base64;
use std::path::{Path, PathBuf};

use gensokyo::client::framer::Framer;

enum Event {
    In(f64, Vec<u8>),
    Out(f64, Vec<u8>),
    Other(f64),
}

fn read(path: &Path) -> Vec<Event> {
    let text = std::fs::read_to_string(path).unwrap();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            let t = v["t"].as_f64().unwrap();
            let b = || base64(v["b"].as_str().unwrap());
            match v["ev"].as_str().unwrap() {
                "in" => Event::In(t, b()),
                "out" => Event::Out(t, b()),
                _ => Event::Other(t),
            }
        })
        .collect()
}

/// Every read is preceded by a tick at its time, as the event loop does; a last tick far past
/// the end releases whatever is still held.
fn run(events: &[Event]) -> Vec<String> {
    let mut f = Framer::new();
    let mut out = Vec::new();
    let mut last = 0.0;
    for ev in events {
        last = match ev {
            Event::Out(t, b) => {
                f.note_sent(b, *t);
                *t
            }
            Event::In(t, b) => {
                out.extend(f.tick(*t));
                out.extend(f.feed(b, *t));
                *t
            }
            Event::Other(t) => *t,
        };
    }
    out.extend(f.tick(last + 10_000.0));
    out.iter().map(|c| c.to_string()).collect()
}

#[test]
fn fixtures_match() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/c1");
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 13);
    let mut failed = Vec::new();
    for p in &names {
        let got = run(&read(p));
        let want: Vec<String> = std::fs::read_to_string(p.with_extension("expected"))
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect();
        if got != want {
            failed.push(format!("{}:\n  got:  {got:#?}\n  want: {want:#?}", p.display()));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
