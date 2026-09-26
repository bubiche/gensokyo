//! A resident starts with default signal dispositions and an empty mask, whatever the daemon
//! ignored or blocked. Its own binary: it ignores SIGHUP and blocks SIGINT in the test process.

use std::ffi::OsString;
use std::path::Path;

use gensokyo::daemon::pty::{self, Size, Spawn};

/// Prints each signal's disposition as perl sees it (IGNORE or DEFAULT) and whether it is
/// blocked, into the file named by its argument.
const PROBE: &str = r#"use POSIX; open(STDOUT, ">", $ARGV[0]) or die;
my $o = POSIX::SigSet->new; sigprocmask(SIG_BLOCK, POSIX::SigSet->new, $o);
for (qw(HUP INT QUIT TERM TSTP PIPE)) {
    printf "%s %s%s\n", $_, $SIG{$_} // "DEFAULT", $o->ismember(eval "SIG$_") ? " blocked" : "" }"#;

fn out(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

#[tokio::test(flavor = "current_thread")]
async fn child_gets_default_dispositions_and_empty_mask() {
    // SAFETY: plain signal-state calls; the mask applies to this thread, which does the spawns.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }

    let control = out("sig-control");
    let st = std::process::Command::new("/usr/bin/perl")
        .args(["-e", PROBE])
        .arg(&control)
        .env_clear()
        .status()
        .unwrap();
    assert!(st.success());
    let control = std::fs::read_to_string(control).unwrap();
    println!("plain Command:\n{control}");

    let path = out("sig-pty");
    let args: Vec<OsString> = vec!["-e".into(), PROBE.into(), path.clone().into()];
    let (_pty, mut child) = pty::spawn(Spawn {
        program: Path::new("/usr/bin/perl"),
        args: &args,
        env: &[],
        cwd: Path::new(env!("CARGO_TARGET_TMPDIR")),
        size: Size::new(24, 80),
    })
    .unwrap();
    assert!(child.wait().await.unwrap().success());
    let reset = std::fs::read_to_string(path).unwrap();
    println!("pty::spawn:\n{reset}");

    assert!(control.contains("HUP IGNORE\n"), "the probe cannot see an inherited SIG_IGN");
    assert!(control.contains("INT DEFAULT blocked\n"), "the probe cannot see a blocked signal");
    for s in ["HUP", "INT", "QUIT", "TERM", "TSTP", "PIPE"] {
        assert!(reset.contains(&format!("{s} DEFAULT\n")), "SIG{s} not clean:\n{reset}");
    }
}
