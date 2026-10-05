//! The command line itself: help, refusals before any daemon starts, a daemon from another
//! build, and `restart` bringing the residents back into a new daemon.

mod common;

use common::{BIN, err, fresh, out, stub_env, wait};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Env {
    dir: PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let dir = fresh(&format!("cli-{name}"));
        std::fs::create_dir_all(dir.join("work")).unwrap();
        assert!(dir.join("run/gensokyo.sock").as_os_str().len() < 104);
        Env { dir }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .args(args)
            .current_dir(&self.dir)
            .envs(stub_env(&self.dir))
            .env("CLAUDE_CODE_CHILD_SESSION", "1")
            .env("TERM_PROGRAM", "iTerm.app")
            .env("ITERM_SESSION_ID", "w0t0p0")
            .env_remove("GENSOKYO_SOCKET")
            .env_remove("GENSOKYO_RESIDENT")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "gensokyo {args:?}: {}", err(&o));
        out(&o)
    }

    /// No daemon was started: nothing is listening and it never wrote its log.
    fn untouched(&self) -> bool {
        !self.dir.join("run/gensokyo.sock").exists() && !self.dir.join("daemon.log").exists()
    }

    fn residents(&self) -> Vec<Value> {
        serde_json::from_str(&self.ok(&["list", "--json"])).unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.run(&["quit"]);
    }
}

#[test]
fn help_names_every_verb_and_each_verb_has_its_own() {
    let e = Env::new("help");
    for args in [&["--help"][..], &["-h"], &["help"]] {
        let text = e.ok(args);
        for verb in ["list", "new", "resume", "banish", "broadcast", "ritual", "quit", "restart"]
            .into_iter()
            .chain(["doctor", "login", "update", "uninstall"])
        {
            assert!(text.contains(&format!("  {verb} ")), "{verb} missing from {args:?}: {text}");
        }
    }
    let text = e.ok(&["help", "ritual", "add"]);
    assert!(text.contains("--prompt-file") && text.contains("--allowed-tools"), "{text}");
    assert!(e.ok(&["ritual", "log", "--help"]).contains("-n, --lines <N>"));
    let v = format!("gensokyo {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(e.ok(&["version"]), v);
    assert_eq!(e.ok(&["--version"]), v);
    assert!(e.untouched());
}

#[test]
fn a_symlink_on_path_still_finds_the_shipped_files() {
    let e = Env::new("link");
    let link = e.dir.join("bin/gensokyo");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(BIN, &link).unwrap();
    let o = Command::new(&link)
        .arg("broadcast")
        .envs(stub_env(&e.dir))
        .env_remove("GENSOKYO_SHARE")
        .output()
        .unwrap();
    assert!(o.status.success() && out(&o).contains("status-report"), "{}", err(&o));
}

#[test]
fn a_mistake_is_refused_before_any_daemon_starts() {
    let e = Env::new("refused");
    let o = e.run(&["lst"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(err(&o).contains("similar") && err(&o).contains("'list'"), "{}", err(&o));
    let o = e.run(&["list", "--bogus"]);
    assert!(!o.status.success() && err(&o).contains("--bogus"), "{}", err(&o));
    // The next flag is not a value: this would have named the resident `--model`.
    let o = e.run(&["new", "-n", "--model", "haiku"]);
    assert!(!o.status.success() && err(&o).contains("'--name <NAME>'"), "{}", err(&o));
    let o = e.run(&["broadcast", "--with", "Reimu"]);
    assert!(!o.status.success() && err(&o).contains("which card?"), "{}", err(&o));
    assert!(e.untouched(), "a refused command started a daemon");
}

#[test]
fn broadcast_alone_lists_the_cards_without_a_daemon() {
    let e = Env::new("cards");
    let cards = e.dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    std::fs::write(
        cards.join("roll-call.md"),
        "---\ntitle: Roll Call\nsummary: who is here\n---\nHi.",
    )
    .unwrap();
    let text = e.ok(&["broadcast"]);
    assert!(text.contains("roll-call") && text.contains("who is here"), "{text}");
    assert!(e.untouched());
}

#[test]
fn a_daemon_from_another_build_is_named_with_its_pid() {
    let e = Env::new("proto");
    std::fs::create_dir_all(e.dir.join("run")).unwrap();
    let l = UnixListener::bind(e.dir.join("run/gensokyo.sock")).unwrap();
    // What an older daemon answers a hello it does not speak, then it hangs up.
    let old = std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut hello = String::new();
        BufReader::new(&s).read_line(&mut hello).unwrap();
        let theirs = serde_json::from_str::<Value>(&hello).unwrap()["proto"].clone();
        let refusal = serde_json::json!({"t": "error", "id": 0, "error": format!("protocol {theirs}, want 3")});
        writeln!(&s, "{refusal}").unwrap();
    });
    let o = e.run(&["list"]);
    old.join().unwrap();
    let want = format!("the running daemon is protocol 3 (pid {})", std::process::id());
    assert!(!o.status.success() && err(&o).contains(&want), "{}", err(&o));
    assert!(err(&o).contains("`gensokyo restart` replaces it"), "{}", err(&o));
}

#[test]
fn restart_brings_every_resident_back_into_a_new_daemon() {
    let e = Env::new("restart");
    e.ok(&["new", "work", "-n", "Reimu"]);
    e.ok(&["new", "work", "-n", "Marisa"]);
    let before = e.residents();
    let ids: Vec<&str> = before.iter().map(|r| r["id"].as_str().unwrap()).collect();
    for id in &ids {
        wait(|| e.dir.join(format!("stub/{id}.ready")).exists(), "the stub to be ready");
    }
    let text = e.ok(&["restart"]);
    let pid =
        |after: &str| text.split(after).nth(1).and_then(|t| t.split(')').next()).map(String::from);
    let (old, new) = (pid("stopping the daemon (pid "), pid("the new daemon is up (pid "));
    assert!(old.is_some() && new.is_some() && old != new, "{text}");
    assert!(text.contains("stopping the daemon (pid ") && text.contains("2 residents"), "{text}");
    assert!(
        text.contains("recalled Reimu (slot 1)") && text.contains("recalled Marisa (slot 2)"),
        "{text}"
    );
    let after = e.residents();
    let names: Vec<&str> = after.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Reimu", "Marisa"]);
    // The same residents, each under its own id, live again.
    for (r, id) in after.iter().zip(&ids) {
        assert_eq!(r["id"], *id);
        assert!(r["departed"].is_null(), "{r}");
    }
    // With nothing running it just starts one.
    e.ok(&["quit"]);
    assert!(e.ok(&["restart"]).contains("no daemon was running; started one"));
}

#[test]
fn restart_after_a_daemon_was_killed_brings_back_whoever_it_had() {
    let e = Env::new("killed");
    e.ok(&["new", "work", "-n", "Reimu"]);
    e.ok(&["new", "work", "-n", "Marisa"]);
    let log = std::fs::read_to_string(e.dir.join("daemon.log")).unwrap();
    let started = log.lines().find(|l| l.contains(r#""ev":"started""#)).unwrap();
    let pid = serde_json::from_str::<Value>(started).unwrap()["pid"].as_i64().unwrap() as i32;
    // SAFETY: plain libc call, on the daemon this test started.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    wait(|| unsafe { libc::kill(pid, 0) } != 0, "the daemon to die");
    let text = e.ok(&["restart"]);
    assert!(text.contains("no daemon was running; started one"), "{text}");
    assert!(
        text.contains("recalled Reimu (slot 1)") && text.contains("recalled Marisa (slot 2)"),
        "{text}"
    );
}

#[test]
fn restart_is_refused_inside_a_resident() {
    let e = Env::new("inside");
    let o = Command::new(BIN)
        .arg("restart")
        .env("GENSOKYO_STATE_DIR", &e.dir)
        .env("GENSOKYO_RESIDENT", "Reimu")
        .output()
        .unwrap();
    assert!(!o.status.success() && err(&o).contains("outside gensokyo"), "{}", err(&o));
    assert!(e.untouched());
}

#[test]
fn a_daemon_that_dies_at_start_is_reported_at_once() {
    let e = Env::new("dies");
    // Where its records go is a file, so it cannot start.
    std::fs::write(e.dir.join("residents"), "").unwrap();
    let t = Instant::now();
    let o = e.run(&["list"]);
    assert!(
        !o.status.success() && err(&o).contains("the daemon stopped as it started"),
        "{}",
        err(&o)
    );
    assert!(err(&o).contains("daemon.log"), "{}", err(&o));
    assert!(t.elapsed() < Duration::from_secs(4), "waited for a daemon that had already exited");
}
