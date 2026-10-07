//! The login agent, a client starting the daemon through launchd, `doctor`, and `update` and
//! `uninstall` refusing what is not theirs. launchctl is a stand-in that writes its calls down,
//! except in the one ignored test that loads a real agent under a test label.

mod common;

use common::{BIN, err, fresh, out, stub_env, wait, wait_for};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

/// The stand-in: `print` answers as launchd would for a job the test marked loaded (and running),
/// `bootstrap` and `bootout` mark it, and `kickstart` starts the daemon as launchd would, detached.
const LAUNCHCTL: &str = r#"#!/bin/sh
d=$(dirname "$0")
printf '%s\n' "$*" >> "$d/launchctl.calls"
case $1 in
  print) [ -f "$d/loaded" ] || exit 113; [ -f "$d/running" ] && printf '\tstate = running\n'; exit 0 ;;
  bootstrap) : > "$d/loaded" ;;
  bootout) rm -f "$d/loaded" ;;
  kickstart) [ -f "$d/loaded" ] || exit 113
    nohup "$TEST_BIN" daemon --foreground > /dev/null 2>&1 &
    echo kicked >> "$d/kicked" ;;
esac
exit 0
"#;

struct Env {
    dir: PathBuf,
    label: String,
}

impl Env {
    fn new(name: &str) -> Env {
        let dir = fresh(&format!("login-{name}"));
        let lc = dir.join("launchctl");
        std::fs::write(&lc, LAUNCHCTL).unwrap();
        chmod_x(&lc);
        let label = format!("dev.gensokyo.test.{name}-{}", std::process::id());
        Env { dir, label }
    }

    fn command(&self, bin: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(bin);
        c.args(args)
            .envs(stub_env(&self.dir))
            .env_remove("GENSOKYO_SOCKET")
            .env_remove("GENSOKYO_RESIDENT");
        c.env("GENSOKYO_LAUNCH_DIR", self.dir.join("agents"))
            .env("GENSOKYO_LAUNCHCTL", self.dir.join("launchctl"))
            .env("GENSOKYO_LAUNCH_LABEL", &self.label)
            .env("TEST_BIN", BIN)
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(Path::new(BIN), args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(o.status.success(), "gensokyo {args:?}: {}", err(&o));
        out(&o)
    }

    fn refused(&self, args: &[&str]) -> String {
        let o = self.run(args);
        assert!(!o.status.success(), "gensokyo {args:?} went through: {}", out(&o));
        err(&o)
    }

    fn plist(&self) -> PathBuf {
        self.dir.join("agents").join(format!("{}.plist", self.label))
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.join("launchctl.calls")).unwrap_or_default()
    }

    fn up(&self) -> bool {
        std::os::unix::net::UnixStream::connect(self.dir.join("run/gensokyo.sock")).is_ok()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.run(&["quit"]);
    }
}

fn chmod_x(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn uid() -> String {
    out(&Command::new("id").arg("-u").output().unwrap()).trim().to_string()
}

/// One key of a plist, as JSON or raw text, through the same plutil launchd's own tools use.
fn extract(plist: &Path, key: &str, fmt: &str) -> String {
    let o = Command::new("plutil").args(["-extract", key, fmt, "-o", "-"]).arg(plist).output();
    out(&o.unwrap()).trim().to_string()
}

#[test]
fn setup_writes_an_agent_launchd_can_load_and_remove_takes_it_out() {
    let e = Env::new("setup");
    assert!(e.ok(&["login"]).starts_with("at login: off"));
    let said = e.ok(&["login", "setup"]);
    assert!(said.contains("launchd runs"), "{said}");
    let p = e.plist();
    let lint = Command::new("plutil").arg("-lint").arg(&p).output().unwrap();
    assert!(lint.status.success(), "{}", out(&lint));
    let exe = std::fs::canonicalize(BIN).unwrap();
    for (i, a) in [&*exe.to_string_lossy(), "daemon", "--foreground"].iter().enumerate() {
        assert_eq!(extract(&p, &format!("ProgramArguments.{i}"), "raw"), *a);
    }
    assert_eq!(extract(&p, "KeepAlive.SuccessfulExit", "raw"), "false");
    assert_eq!(extract(&p, "RunAtLoad", "raw"), "true");
    assert_eq!(extract(&p, "ProcessType", "raw"), "Interactive");
    let path = std::env::var("PATH").unwrap();
    assert_eq!(extract(&p, "EnvironmentVariables.PATH", "raw"), path);
    let state = e.dir.display().to_string();
    assert_eq!(extract(&p, "EnvironmentVariables.GENSOKYO_STATE_DIR", "raw"), state);
    assert_eq!(extract(&p, "StandardErrorPath", "raw"), format!("{state}/login.log"));
    let target = format!("gui/{}/{}", uid(), e.label);
    let boot = format!("bootout {target}\nbootstrap gui/{} {}\n", uid(), p.display());
    assert!(e.calls().ends_with(&boot), "{}", e.calls());
    assert!(e.ok(&["login"]).starts_with("at login: on: launchd runs"));

    // The same agent again is left as it is; a new one is not loaded over a running daemon, whose
    // residents launchd's SIGTERM would send away.
    assert!(e.ok(&["login", "setup"]).starts_with("already on"));
    assert_eq!(e.calls().matches("bootstrap").count(), 1);
    let before = std::fs::read_to_string(&p).unwrap();
    std::fs::write(e.dir.join("running"), "").unwrap();
    let mut c = e.command(Path::new(BIN), &["login", "setup"]);
    let o = c.env("PATH", format!("/opt/new:{path}")).output().unwrap();
    assert!(!o.status.success() && err(&o).contains("`gensokyo quit` first"), "{}", err(&o));
    assert!(e.refused(&["login", "remove"]).contains("`gensokyo quit` first"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before);

    std::fs::remove_file(e.dir.join("running")).unwrap();
    assert!(e.ok(&["login", "remove"]).starts_with("removed"));
    assert!(!p.exists());
    assert!(e.calls().ends_with(&format!("bootout {target}\n")), "{}", e.calls());
    assert!(e.ok(&["login", "remove"]).starts_with("nothing to remove"));
    assert!(!e.up(), "no daemon was started by any of it");
}

#[test]
fn a_plist_gensokyo_did_not_write_is_left_alone() {
    let e = Env::new("foreign");
    std::fs::create_dir_all(e.dir.join("agents")).unwrap();
    let theirs = concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>"#,
        "<key>ProgramArguments</key><array><string>/usr/bin/true</string></array>",
        "</dict></plist>"
    );
    std::fs::write(e.plist(), theirs).unwrap();
    assert!(e.ok(&["login"]).contains("did not write"));
    assert!(e.refused(&["login", "setup"]).contains("was not written by gensokyo"));
    assert!(e.refused(&["login", "remove"]).contains("was not written by gensokyo"));
    assert_eq!(std::fs::read_to_string(e.plist()).unwrap(), theirs);
    assert_eq!(e.calls(), "");
}

#[test]
fn with_the_agent_a_client_has_launchd_start_the_daemon() {
    let e = Env::new("kick");
    e.ok(&["login", "setup"]);
    e.ok(&["list"]);
    assert!(e.dir.join("kicked").exists(), "launchd was not asked: {}", e.calls());
    assert!(e.calls().contains(&format!("kickstart gui/{}/{}", uid(), e.label)));
    assert!(e.up());
    e.ok(&["quit"]);
    wait(|| !e.up(), "the daemon to stop");

    // Written but not loaded (launchd refuses the kickstart): the client starts it itself.
    std::fs::remove_file(e.dir.join("loaded")).unwrap();
    e.ok(&["list"]);
    assert_eq!(std::fs::read_to_string(e.dir.join("kicked")).unwrap(), "kicked\n");
    assert_eq!(e.calls().matches("kickstart").count(), 2);
    assert!(e.up());
}

#[test]
fn restart_says_when_the_agent_runs_another_binary() {
    let e = Env::new("other");
    e.ok(&["login", "setup"]);
    let p = e.plist();
    let text = std::fs::read_to_string(&p).unwrap();
    let exe = std::fs::canonicalize(BIN).unwrap().display().to_string();
    std::fs::write(&p, text.replace(&exe, "/opt/elsewhere/bin/gensokyo")).unwrap();
    let o = e.run(&["restart"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("the login agent runs /opt/elsewhere/bin/gensokyo"), "{}", err(&o));
    assert!(!e.dir.join("kicked").exists(), "launchd would have started the other binary");
    assert!(e.ok(&["login"]).contains("STALE"));
}

#[test]
fn doctor_reads_without_starting_anything() {
    let e = Env::new("doctor");
    let text = e.ok(&["doctor"]);
    let has = |l: &str| text.lines().any(|t| t.trim_start().starts_with(l));
    for l in ["binary", "share", "claude", "registry", "daemon     not running", "at login   off"] {
        assert!(has(l), "no `{l}` in:\n{text}");
    }
    assert!(text.contains("(a build from source)"), "{text}");
    assert!(!e.up() && !e.dir.join("daemon.log").exists(), "doctor started a daemon");
    e.ok(&["list"]);
    let text = e.ok(&["doctor"]);
    assert!(text.contains("daemon     running (pid ") && text.contains(", 0 residents"), "{text}");
    std::fs::create_dir_all(e.dir.join("rituals/big/runs")).unwrap();
    std::fs::write(e.dir.join("rituals/big/runs/1.log"), vec![b'x'; 3 << 20]).unwrap();
    std::os::unix::fs::symlink("/", e.dir.join("rituals/big/root")).unwrap();
    let text = e.ok(&["doctor"]);
    let size = text.lines().find(|l| l.trim_start().starts_with("size")).unwrap_or_default();
    assert!(size.contains(" MB; largest rituals/big/runs/1.log 3.0 MB, "), "{text}");
    assert!(!text.contains("never reach residents"), "{text}");
    e.ok(&["login", "setup"]);
    let mut c = e.command(Path::new(BIN), &["doctor"]);
    let o =
        c.env("ANTHROPIC_BASE_URL", "http://x").env("https_proxy", "http://y").output().unwrap();
    let want = "ANTHROPIC_BASE_URL https_proxy set here never reach residents launchd starts";
    assert!(out(&o).contains(want), "{}", out(&o));
}

#[test]
fn update_and_uninstall_act_only_on_a_release_tree() {
    let e = Env::new("tree");
    assert!(e.refused(&["update"]).contains("is a build, not a release install"));
    assert!(e.refused(&["uninstall", "--yes"]).contains("is a build, not a release install"));
    e.ok(&["list"]);
    assert!(e.refused(&["uninstall", "--yes"]).contains("`gensokyo quit` first"));
    e.ok(&["quit"]);
    wait(|| !e.up(), "the daemon to stop");

    // A tree as a release unpacks: the binary in bin/, VERSION and install.sh beside it.
    let root = e.dir.join("rel");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::copy(BIN, root.join("bin/gensokyo")).unwrap();
    std::fs::write(root.join("VERSION"), format!("{}\n", env!("CARGO_PKG_VERSION"))).unwrap();
    std::fs::write(root.join("install.sh"), "exit 1\n").unwrap();
    let latest = e.dir.join("releases/latest/download");
    std::fs::create_dir_all(&latest).unwrap();
    std::fs::write(latest.join("VERSION"), "9.9.9\n").unwrap();
    let base = format!("file://{}", e.dir.join("releases").display());
    let mine = root.join("bin/gensokyo");
    let rel = |args: &[&str]| {
        let mut c = e.command(&mine, args);
        c.env("GENSOKYO_RELEASE_BASE", &base).output().unwrap()
    };
    let o = rel(&["update", "--check"]);
    let v = env!("CARGO_PKG_VERSION");
    assert_eq!(
        out(&o),
        format!("gensokyo {v} is installed; 9.9.9 is out (`gensokyo update` fetches it)\n")
    );
    let o = rel(&["update", "--version", v]);
    assert!(out(&o).contains("is already the release you asked for"), "{}", err(&o));
    // install.sh failing is reported, and nothing else is tried.
    let o = rel(&["update"]);
    assert!(!o.status.success() && err(&o).contains("nothing was changed"), "{}", err(&o));
    assert!(out(&rel(&["doctor"])).contains("(the release in "));
    std::fs::remove_dir_all(e.dir.join("releases")).unwrap();
    let o = rel(&["update", "--check"]);
    assert!(err(&o).contains("cannot tell which release is the latest"), "{}", err(&o));
}

/// A real agent under a test label, against the real launchd: the daemon comes up at load, finds
/// `claude` through the baked PATH alone, comes back after a crash, stays down after a quit, and a
/// client brings it back. `cargo test --test login -- --ignored`; the guard boots it out.
#[test]
#[ignore = "loads a real launchd agent"]
fn a_real_agent_starts_the_daemon_and_brings_it_back_after_a_crash() {
    struct Booted(String, PathBuf);
    impl Drop for Booted {
        fn drop(&mut self) {
            let _ = Command::new("launchctl").args(["bootout", &self.0]).output();
            let _ = std::fs::remove_file(&self.1);
        }
    }
    let dir = fresh("login-real");
    let label = format!("dev.gensokyo.test.real-{}", std::process::id());
    let plist = dir.join("agents").join(format!("{label}.plist"));
    let _guard = Booted(format!("gui/{}/{label}", uid()), plist.clone());
    // `claude` only on PATH: the stub, under its own name.
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let stub = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stub-claude");
    std::os::unix::fs::symlink(stub, bin.join("claude")).unwrap();
    let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", bin.display());
    let gensokyo = |args: &[&str]| {
        let mut c = Command::new(BIN);
        c.args(args).env_remove("GENSOKYO_SOCKET").env_remove("GENSOKYO_CLAUDE");
        c.env("GENSOKYO_STATE_DIR", &dir).env("GENSOKYO_CONFIG_DIR", dir.join("conf"));
        c.env("CLAUDE_CONFIG_DIR", dir.join("claude")).env("PATH", &path);
        c.env("GENSOKYO_LAUNCH_DIR", dir.join("agents")).env("GENSOKYO_LAUNCH_LABEL", &label);
        let o = c.stdin(Stdio::null()).output().unwrap();
        assert!(o.status.success(), "gensokyo {args:?}: {}", err(&o));
        out(&o)
    };
    let sock = dir.join("run/gensokyo.sock");
    let pid = || {
        let s = std::os::unix::net::UnixStream::connect(&sock).ok()?;
        gensokyo::cli::peer_pid(&s)
    };
    gensokyo(&["login", "setup"]);
    wait(|| pid().is_some(), "launchd to start the daemon at load");
    let first = pid().unwrap();
    std::fs::create_dir_all(dir.join("work")).unwrap();
    let said = gensokyo(&["new", &dir.join("work").display().to_string(), "-n", "Reimu"]);
    assert!(said.starts_with("summoned Reimu"), "{said}");

    // While launchd's daemon has a resident, neither setup nor remove may boot it out.
    let refused = |args: &[&str], path: &str| {
        let mut c = Command::new(BIN);
        c.args(args).env_remove("GENSOKYO_SOCKET").env_remove("GENSOKYO_CLAUDE");
        c.env("GENSOKYO_STATE_DIR", &dir).env("GENSOKYO_CONFIG_DIR", dir.join("conf"));
        c.env("CLAUDE_CONFIG_DIR", dir.join("claude")).env("PATH", path);
        c.env("GENSOKYO_LAUNCH_DIR", dir.join("agents")).env("GENSOKYO_LAUNCH_LABEL", &label);
        let o = c.stdin(Stdio::null()).output().unwrap();
        assert!(!o.status.success(), "gensokyo {args:?} went through: {}", out(&o));
        assert!(err(&o).contains("`gensokyo quit` first"), "{}", err(&o));
    };
    refused(&["login", "remove"], &path);
    refused(&["login", "setup"], &format!("/opt/new:{path}"));
    assert_eq!(pid(), Some(first));
    assert!(gensokyo(&["list"]).contains("Reimu"));

    // A crash: launchd starts it again (its throttle is 10 s).
    unsafe { libc::kill(first, libc::SIGKILL) };
    wait_for(Duration::from_secs(30), || pid().is_some_and(|p| p != first), "a new daemon");
    // A quit exits 0, which KeepAlive leaves down; the next client asks launchd for it.
    gensokyo(&["quit"]);
    wait(|| pid().is_none(), "the daemon to stop");
    std::thread::sleep(Duration::from_secs(12));
    assert!(pid().is_none(), "launchd brought back a daemon that quit");
    gensokyo(&["list"]);
    assert!(pid().is_some());
    assert!(gensokyo(&["login"]).contains("on: launchd runs"));
    gensokyo(&["quit"]);
    wait(|| pid().is_none(), "the daemon to stop");
    gensokyo(&["login", "remove"]);
}
