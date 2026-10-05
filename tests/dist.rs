//! The release as a user gets it: scripts/release.sh packages this build, install.sh fetches it
//! over file:// into a home of the test's own, `--update` swaps it, and uninstall.sh takes it
//! all away again. Nothing here touches the real home, launchd or the network.

mod common;

use common::{BIN, err, fresh, out};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const LABEL: &str = "dev.gensokyo.test.dist";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A releases tree as GitHub serves one (`download/v<ver>/…`, `latest/download/VERSION`),
/// built once per run: this build as the latest release, the same binary labelled 9.9.9 to
/// update to, and 6.6.6 whose SHA256SUMS does not match its tarball.
fn releases() -> &'static Path {
    static BASE: OnceLock<PathBuf> = OnceLock::new();
    BASE.get_or_init(|| {
        let base = fresh("dist-releases");
        for v in [VERSION, "9.9.9", "6.6.6"] {
            let to = base.join(format!("download/v{v}"));
            let o = Command::new(format!("{ROOT}/scripts/release.sh"))
                .args(["--bin", BIN, "--version", v, "--out"])
                .arg(&to)
                .env("GENSOKYO_RELEASE_ANY_VERSION", "1")
                .output()
                .unwrap();
            assert!(o.status.success(), "release.sh {v}: {}{}", out(&o), err(&o));
        }
        let sums = base.join("download/v6.6.6/SHA256SUMS");
        let text = std::fs::read_to_string(&sums).unwrap();
        std::fs::write(&sums, format!("{}{}", "0".repeat(64), &text[64..])).unwrap();
        std::fs::create_dir_all(base.join("latest/download")).unwrap();
        std::fs::write(base.join("latest/download/VERSION"), format!("{VERSION}\n")).unwrap();
        base
    })
}

/// A home of the test's own and a stand-in `launchctl` that writes its arguments down.
struct Home {
    dir: PathBuf,
}

impl Home {
    fn new(name: &str) -> Home {
        let dir = fresh(&format!("dist-{name}"));
        std::fs::create_dir_all(dir.join("home")).unwrap();
        let stub = dir.join("launchctl");
        std::fs::write(
            &stub,
            format!("#!/bin/sh\necho \"$@\" >> '{}'\n", dir.join("launchctl.log").display()),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        Home { dir }
    }

    fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    /// `prog args…` with nothing of the caller's environment but TMPDIR: a system PATH (so no
    /// gensokyo, tmux or claude of the machine's is seen), this home, and the releases tree.
    fn command(&self, prog: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Command {
        let mut c = Command::new(prog);
        c.args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("TMPDIR", std::env::temp_dir())
            .env("GENSOKYO_RELEASE_BASE", format!("file://{}", releases().display()))
            .env("GENSOKYO_LAUNCHCTL", self.dir.join("launchctl"))
            .env("GENSOKYO_LAUNCH_LABEL", LABEL)
            .current_dir(&self.dir)
            .stdin(Stdio::null());
        c
    }

    fn run(&self, prog: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Output {
        self.command(prog, args).output().unwrap()
    }

    /// `curl … | sh -s -- args`: install.sh on stdin, as the README says.
    fn piped(&self, args: &[&str]) -> Output {
        let script =
            std::fs::File::open(releases().join(format!("download/v{VERSION}/install.sh")));
        let mut c = self.command("/bin/sh", &["-s", "--"]);
        c.args(args).stdin(script.unwrap()).output().unwrap()
    }

    fn link(&self) -> PathBuf {
        self.home().join(".local/bin/gensokyo")
    }

    fn tree(&self) -> PathBuf {
        self.home().join(".gensokyo")
    }

    fn version(&self) -> String {
        out(&self.run(self.link(), &["--version"])).trim().to_string()
    }

    fn launchctl(&self) -> String {
        std::fs::read_to_string(self.dir.join("launchctl.log")).unwrap_or_default()
    }

    /// Every file and link under the home, relative to it; directories alone are not counted.
    fn files(&self) -> Vec<String> {
        fn walk(d: &Path, base: &Path, acc: &mut Vec<String>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                if e.file_type().unwrap().is_dir() {
                    walk(&p, base, acc);
                } else {
                    acc.push(p.strip_prefix(base).unwrap().display().to_string());
                }
            }
        }
        let mut acc = vec![];
        walk(&self.home(), &self.home(), &mut acc);
        acc.sort();
        acc
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.home().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// A login agent at our label that runs `prog`.
    fn plist(&self, prog: &str) {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
             <key>Label</key><string>{LABEL}</string><key>ProgramArguments</key>\
             <array><string>{prog}</string><string>daemon</string></array></dict></plist>\n"
        );
        self.write(&format!("Library/LaunchAgents/{LABEL}.plist"), &xml);
    }
}

fn ok(o: &Output, what: &str) {
    assert!(o.status.success(), "{what}: {}{}", out(o), err(o));
}

#[test]
fn curl_into_sh_installs_the_latest_release_and_links_it() {
    let h = Home::new("curl");
    let o = h.piped(&[]);
    ok(&o, "install");
    assert_eq!(h.version(), format!("gensokyo {VERSION}"));
    assert_eq!(std::fs::read_link(h.link()).unwrap(), h.tree().join("bin/gensokyo"));
    assert!(h.tree().join("share/names.txt").is_file());
    assert!(out(&o).contains("is not on your PATH"), "{}", out(&o));

    let again = h.piped(&[]);
    assert!(!again.status.success());
    assert!(err(&again).contains("already holds gensokyo"), "{}", err(&again));
}

#[test]
fn a_tarball_that_does_not_match_its_sums_installs_nothing() {
    let h = Home::new("tampered");
    let o = h.piped(&["--version", "6.6.6"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("checksum mismatch"), "{}", err(&o));
    assert_eq!(h.files(), Vec::<String>::new());
    assert!(!h.tree().exists());
}

#[test]
fn an_unpacked_tarball_links_itself_and_no_fetch_wants_one() {
    let h = Home::new("unpacked");
    let tarball =
        releases().join(format!("download/v{VERSION}/gensokyo-{VERSION}-macos-arm64.tar.gz"));
    ok(
        &h.run("/usr/bin/tar", &["-xzf", tarball.to_str().unwrap(), "-C", h.dir.to_str().unwrap()]),
        "untar",
    );
    let tree = h.dir.join(format!("gensokyo-{VERSION}"));
    let bin = h.home().join("bin");
    ok(&h.run(tree.join("install.sh"), &["--bin-dir", bin.to_str().unwrap()]), "install.sh");
    assert_eq!(std::fs::read_link(bin.join("gensokyo")).unwrap(), tree.join("bin/gensokyo"));

    let o = h.piped(&["--no-fetch"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("--no-fetch"), "{}", err(&o));
}

#[test]
fn update_swaps_the_tree_and_a_download_that_fails_changes_nothing() {
    let h = Home::new("update");
    ok(&h.piped(&[]), "install");
    let script = h.tree().join("install.sh");

    let same = h.run(&script, &["--update"]);
    ok(&same, "update to the latest");
    assert!(out(&same).contains("already the release you asked for"), "{}", out(&same));

    let o = h.run(&script, &["--update", "--version", "9.9.9"]);
    ok(&o, "update");
    assert!(out(&o).contains(&format!("gensokyo {VERSION} -> 9.9.9")), "{}", out(&o));
    assert_eq!(std::fs::read_to_string(h.tree().join("VERSION")).unwrap(), "9.9.9\n");
    assert_eq!(h.version(), format!("gensokyo {VERSION}"), "the link still runs");
    let before = h.files();

    for (v, why) in [("6.6.6", "checksum mismatch"), ("7.7.7", "cannot download")] {
        let o = h.run(h.tree().join("install.sh"), &["--update", "--version", v]);
        assert!(!o.status.success(), "{v}");
        assert!(err(&o).contains(why), "{v}: {}", err(&o));
        assert_eq!(h.files(), before, "{v} left the install as it was");
    }
    let names: Vec<_> =
        std::fs::read_dir(h.home()).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(names.iter().all(|n| !n.to_string_lossy().contains(".gensokyo-update")), "{names:?}");
    assert!(names.iter().all(|n| !n.to_string_lossy().contains(".old.")), "{names:?}");
}

#[test]
fn uninstall_says_what_it_would_remove_then_leaves_the_home_as_it_was() {
    let h = Home::new("uninstall");
    h.write("notes.txt", "mine");
    h.write(".config/other/config", "not gensokyo's");
    h.write("Library/LaunchAgents/com.example.other.plist", "<plist/>");
    let before = h.files();
    ok(&h.piped(&[]), "install");
    h.write(".config/gensokyo/rituals/r.md", "---\nschedule: every 10m\n---\nhi\n");
    h.write(".local/state/gensokyo/daemon.log", "{}\n");
    h.plist(&h.tree().join("bin/gensokyo").display().to_string());
    let installed = h.files();
    let script = h.tree().join("uninstall.sh");

    let o = h.run(&script, &[]);
    ok(&o, "uninstall, listing");
    for what in ["login agent", "link", "install tree", "config and rituals", "records"] {
        assert!(out(&o).contains(&format!("found  {what}")), "{what}: {}", out(&o));
    }
    assert_eq!(h.files(), installed, "nothing deleted without --yes");
    assert_eq!(h.launchctl(), "");

    let o = h.run(&script, &["--yes"]);
    ok(&o, "uninstall --yes");
    assert_eq!(h.files(), before, "{}", out(&o));
    assert!(!h.tree().exists());
    let uid = out(&h.run("/usr/bin/id", &["-u"])).trim().to_string();
    assert_eq!(h.launchctl(), format!("bootout gui/{uid}/{LABEL}\n"));
}

#[test]
fn keep_data_keeps_config_and_state_and_an_agent_that_is_not_ours_stays() {
    let h = Home::new("keep");
    ok(&h.piped(&[]), "install");
    h.write(".config/gensokyo/config", "STATUSLINE=user\n");
    h.write(".local/state/gensokyo/daemon.log", "{}\n");
    h.plist("/usr/bin/true");
    let o = h.run(h.tree().join("uninstall.sh"), &["--yes", "--keep-data"]);
    ok(&o, "uninstall --yes --keep-data");
    assert!(out(&o).contains("was not written by gensokyo"), "{}", out(&o));
    assert_eq!(
        h.files(),
        [
            ".config/gensokyo/config".to_string(),
            ".local/state/gensokyo/daemon.log".into(),
            format!("Library/LaunchAgents/{LABEL}.plist"),
        ]
    );
    assert_eq!(h.launchctl(), "");
}

#[test]
fn uninstall_refuses_while_the_daemon_holds_its_lock() {
    let h = Home::new("running");
    ok(&h.piped(&[]), "install");
    h.write(".local/state/gensokyo/run/daemon.lock", "");
    let lock = std::fs::File::open(h.home().join(".local/state/gensokyo/run/daemon.lock")).unwrap();
    lock.lock().unwrap();
    let installed = h.files();
    let o = h.run(h.tree().join("uninstall.sh"), &["--yes"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("nothing was removed"), "{}", err(&o));
    assert!(out(&o).contains(&format!("pid {}", std::process::id())), "{}", out(&o));
    assert_eq!(h.files(), installed);
    drop(lock);
    ok(&h.run(h.tree().join("uninstall.sh"), &["--yes"]), "uninstall once it stopped");
}

#[test]
fn the_installed_binary_updates_sets_up_login_and_uninstalls_itself() {
    let h = Home::new("self");
    let before = h.files();
    assert!(h.piped(&[]).status.success());
    let me = |args: &[&str]| {
        let mut c = h.command(h.link(), args);
        // `login setup` wants a claude for the agent's PATH to find; any program will do.
        c.env("GENSOKYO_CLAUDE", "/usr/bin/true").output().unwrap()
    };
    let o = me(&["update", "--check"]);
    assert!(out(&o).contains("is already the release you asked for"), "{}", err(&o));
    let o = me(&["update", "--version", "9.9.9"]);
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains(&format!("gensokyo {VERSION} -> 9.9.9")), "{}", out(&o));
    assert_eq!(std::fs::read_to_string(h.tree().join("VERSION")).unwrap(), "9.9.9\n");
    let o = me(&["login", "setup"]);
    assert!(o.status.success(), "{}", err(&o));
    let plist = h.home().join(format!("Library/LaunchAgents/{LABEL}.plist"));
    assert!(plist.exists());
    assert!(out(&me(&["doctor"])).contains("(the release in ~/.gensokyo)"));

    // With no terminal to ask, only the list.
    let o = me(&["uninstall"]);
    assert!(out(&o).contains("no terminal to ask, so nothing was deleted"), "{}", out(&o));
    assert!(plist.exists() && h.link().exists());
    let o = me(&["uninstall", "--yes"]);
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    assert!(h.launchctl().contains("bootout gui/"), "{}", h.launchctl());
    assert_eq!(h.files(), before, "{}", out(&o));
}
