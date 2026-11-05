//! Residents from gensokyo 0.7.1, recalled by this build as `restart` brings them back after an
//! update: their records as 0.7.1 wrote them (tests/fixtures/v0.7.1), a chief of staff's role in
//! the words 0.7.1 shipped, and no ledger made for anyone.

mod common;

use common::{Daemon, err, fresh, out};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Output, Stdio};

const COS: &str = "5b0c3e9a-7d21-4f6e-8a43-c1d2e3f4a5b6";
const HELPER: &str = "9e8d7c6b-5a49-4382-b1a0-f9e8d7c6b5a4";

fn fixture(f: &str) -> String {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v0.7.1");
    std::fs::read_to_string(format!("{dir}/{f}")).unwrap()
}

/// `gensokyo <args>` from inside resident `id`, as its Bash tool runs it, with `stdin`.
fn inside(d: &Daemon, id: &str, args: &[&str], stdin: &str) -> Output {
    let mut c = d.command(args);
    c.env("GENSOKYO_SOCKET", d.socket()).env("GENSOKYO_RESIDENT", id);
    c.current_dir(&d.dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut c = c.spawn().unwrap();
    c.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    c.wait_with_output().unwrap()
}

#[test]
fn a_chief_of_staff_from_0_7_1_comes_back_in_its_own_words_and_keeps_no_ledger() {
    // The role ships as 0.7.1 shipped it, byte for byte: a recall reads it again, and its words
    // reach the conversation at its next compaction.
    let words = fixture("chief-of-staff.md");
    let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/share/roles/chief-of-staff.md");
    assert_eq!(std::fs::read_to_string(shipped).unwrap(), words);

    // Its state dir as 0.7.1 left it at `restart`, in a repository of the test's own. Every pid
    // is null: a daemon ends whatever process a record's pid still names.
    let dir = fresh("up-071");
    let proj = dir.join("proj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    for (sub, file, id) in
        [("residents", "resident.json", COS), ("departed", "helper.json", HELPER)]
    {
        let mut r: Value = serde_json::from_str(&fixture(file)).unwrap();
        assert!(r["pid"].is_null() && r.get("ledger").is_none(), "{file}");
        r["cwd"] = json!(proj);
        std::fs::create_dir_all(dir.join(sub)).unwrap();
        std::fs::write(dir.join(format!("{sub}/{id}.json")), r.to_string()).unwrap();
    }
    std::fs::create_dir_all(dir.join("answers")).unwrap();
    for (file, id) in [("answer.json", COS), ("helper-answer.json", HELPER)] {
        std::fs::write(dir.join(format!("answers/{id}.json")), fixture(file)).unwrap();
    }
    // A conversation to resume, under the name Claude Code has it by, and `restart`'s list of
    // who comes back.
    std::fs::create_dir_all(dir.join("claude/projects/stub")).unwrap();
    std::fs::write(dir.join(format!("claude/projects/stub/{COS}.jsonl")), "").unwrap();
    std::fs::create_dir_all(dir.join("stub")).unwrap();
    std::fs::write(dir.join(format!("stub/{COS}.name")), "ops_cos\n").unwrap();
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::write(dir.join("run/comeback"), format!("{COS}\n")).unwrap();

    let d = Daemon::at(dir, vec![("STUB_HOOKS".into(), "1".into())]);
    d.cli(&["list"]);
    let args = d.stub(&json!(COS), "args");
    assert!(args.contains(&format!("--resume {COS}")), "{args}");

    // Back in its own conversation, with the flags it had and the role's words in the one
    // appended prompt, after gensokyo's paragraph: no file, and nothing of a ledger.
    let file = d.dir.join(format!("residents/{COS}.json"));
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let argv: Vec<&str> =
        rec["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    let flag = |f: &str| argv.iter().position(|a| *a == f).map(|i| argv[i + 1]);
    assert_eq!(flag("--resume"), Some(COS));
    assert_eq!(flag("--permission-mode"), Some("auto"));
    assert_eq!(flag("--append-system-prompt-file"), None);
    let system = flag("--append-system-prompt").unwrap();
    assert!(system.contains("your resident name is ops_cos."), "{system}");
    assert!(system.ends_with(&format!("\n\n{}", words.trim_end())), "{system}");
    assert!(!system.contains("ledger"), "{system}");
    assert!(!flag("--settings").unwrap().contains("ledger"));
    assert!(rec.get("ledger").is_none(), "{rec}");
    assert!(!d.dir.join("ledgers").exists());
    let kept =
        [("name", json!("ops_cos")), ("role", json!("chief-of-staff")), ("turns", json!(1402))];
    for (k, v) in kept {
        assert_eq!(rec[k], v, "{k}");
    }

    // `list --json` has what a lead reads of itself and of its helpers.
    let all: Vec<Value> = serde_json::from_str(&out(&d.cli(&["list", "--json", "--all"]))).unwrap();
    let me = |name: &str| all.iter().find(|r| r["name"] == name).unwrap().clone();
    let cos = me("ops_cos");
    assert_eq!((cos["id"].as_str(), cos["turns"].as_u64()), (Some(COS), Some(1402)), "{cos}");
    assert!(cos["owner"].is_null() && cos["state"] != "departed", "{cos}");
    let helper = me("flaky-ci");
    assert_eq!((helper["owner"].as_str(), helper["state"].as_str()), (Some(COS), Some("departed")));

    // `read` has its last answer, from the user's terminal, and its helper's, from inside it.
    let o = d.cli(&["read", "ops_cos"]);
    assert!(out(&o).contains("the release waits on your decision"), "{}", out(&o));
    let o = inside(&d, COS, &["read", "flaky-ci"], "");
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("LGTM: the retry is bounded"), "{}", out(&o));

    // After its next compaction it is pointed to no ledger.
    let hook = json!({"hook_event_name": "SessionStart", "session_id": COS, "source": "compact"});
    let o = inside(&d, COS, &["_hook"], &hook.to_string());
    assert!(o.status.success(), "{}", err(&o));
    assert!(!out(&o).contains("ledger"), "{}", out(&o));
    assert!(!d.dir.join("ledgers").exists());
}
