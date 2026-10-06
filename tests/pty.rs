//! PTY supervision against real children: readiness on the master, job control, exec errors,
//! exit status, reaping, fd hygiene, setsid'd grandchildren, paste backpressure, resize, and
//! the HUP -> TERM -> KILL sweep.
// Each test runs on its own runtime and thread; the lock only orders tests against each other.
#![allow(clippy::await_holding_lock)]

use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::pin::Pin;
use std::sync::{RwLock, RwLockReadGuard};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use gensokyo::daemon::pty::{self, OwnedReadPty, OwnedWritePty, Size, Spawn};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio::process::Child;

/// Every test holds this shared; the fd count test holds it exclusively, since the tests run
/// as threads of one process and would otherwise see each other's masters.
static FDS: RwLock<()> = RwLock::new(());

fn shared() -> RwLockReadGuard<'static, ()> {
    FDS.read().unwrap_or_else(|e| e.into_inner())
}

fn try_spawn(program: &Path, args: &[&str]) -> std::io::Result<(pty::Pty, Child)> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let env = [("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into())];
    let cwd = Path::new(env!("CARGO_TARGET_TMPDIR"));
    pty::spawn(Spawn { program, args: &args, env: &env, cwd, cols: 80, rows: 24 })
        .map_err(std::io::Error::other)
}

/// The split master, the child, and the master's raw fd (for tcgetpgrp).
fn bash(script: &str) -> (OwnedReadPty, OwnedWritePty, Child, i32) {
    let (pty, child) = try_spawn(Path::new("/bin/bash"), &["-c", script]).expect("spawn");
    let fd = std::os::fd::AsRawFd::as_raw_fd(&pty);
    let (r, w) = pty.into_split();
    (r, w, child, fd)
}

/// Reads until `needle` shows up; returns the number of reads that returned data.
async fn read_until(r: &mut OwnedReadPty, buf: &mut Vec<u8>, needle: &str, secs: u64) -> usize {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut chunk = vec![0u8; 65536];
    let mut reads = 0;
    while !text(buf).contains(needle) {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, r.read(&mut chunk)).await {
            Ok(Ok(0)) => panic!("EOF before {needle:?}; got {:?}", text(buf)),
            Ok(Ok(n)) => {
                reads += 1;
                buf.extend_from_slice(&chunk[..n]);
            }
            Ok(Err(e)) => panic!("read error {e} before {needle:?}"),
            Err(_) => panic!("timeout waiting for {needle:?}; got {:?}", text(buf)),
        }
    }
    reads
}

/// Reads to EOF (or an error, which is how a macOS master reports a hung-up slave).
async fn drain(r: &mut OwnedReadPty) -> Vec<u8> {
    let (mut out, mut chunk) = (vec![], vec![0u8; 65536]);
    while let Ok(k @ 1..) = r.read(&mut chunk).await {
        out.extend_from_slice(&chunk[..k]);
    }
    out
}

fn text(buf: &[u8]) -> String {
    String::from_utf8_lossy(buf).into_owned()
}

/// The pid printed after `tag ` in `out`.
fn tagged(out: &str, tag: &str) -> i32 {
    out.split(&format!("{tag} "))
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// Counts how often the read half is polled, and how often it had nothing (Pending).
struct Counting {
    inner: OwnedReadPty,
    polls: usize,
    pending: usize,
}

impl AsyncRead for Counting {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.polls += 1;
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        if r.is_pending() {
            self.pending += 1;
        }
        r
    }
}

#[tokio::test]
async fn asyncfd_on_a_macos_pty_master() {
    let _g = shared();
    for (name, script) in [
        ("slow 1 B / 10 ms x 200", "for i in $(jot 200); do printf x; sleep 0.01; done"),
        ("idle 2 s then 1 B", "sleep 2; printf x"),
        ("burst 8 MiB", "head -c 8388608 /dev/zero | tr '\\0' x"),
        ("exit with a byte still buffered", "printf LAST"),
    ] {
        let (r, _w, mut child, _) = bash(script);
        let mut c = Counting { inner: r, polls: 0, pending: 0 };
        let t = Instant::now();
        let (mut bytes, mut reads) = (0usize, 0usize);
        let mut chunk = vec![0u8; 65536];
        let res = tokio::time::timeout(Duration::from_secs(20), async {
            while let Ok(k @ 1..) = c.read(&mut chunk).await {
                reads += 1;
                bytes += k;
            }
        })
        .await;
        let st = child.wait().await.unwrap();
        println!(
            "{name}: {bytes} bytes in {reads} reads, {} polls ({} pending), EOF after {:?}, {st:?}",
            c.polls,
            c.pending,
            t.elapsed()
        );
        assert!(res.is_ok(), "{name}: no EOF");
        assert!(bytes > 0, "{name}: output lost");
        // A spurious wakeup or two is fine; a Pending for every poll is the bug this looks for.
        assert!(c.pending <= 2 * reads + 2, "{name}: more Pending polls than data reads");
    }
}

#[tokio::test]
async fn ctrl_c_byte_sigints_the_foreground_pgrp() {
    let _g = shared();
    // bash -c keeps one pgrp, so the trap fires in the session leader.
    let (mut r, mut w, mut child, fd) =
        bash("trap 'echo GOT-INT; exit 7' INT; echo READY; while :; do sleep 0.05; done");
    let pid = child.id().unwrap() as i32;
    let mut buf = vec![];
    read_until(&mut r, &mut buf, "READY", 5).await;
    // SAFETY: fd is the live master.
    let fg = unsafe { libc::tcgetpgrp(fd) };
    w.write_all(b"\x03").await.unwrap();
    read_until(&mut r, &mut buf, "GOT-INT", 5).await;
    let st = child.wait().await.unwrap();
    assert_eq!(fg, pid);
    assert_eq!(st.code(), Some(7));
}

#[tokio::test]
async fn exec_errors_come_back_from_spawn() {
    let _g = shared();
    use std::os::unix::fs::PermissionsExt;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("exec");
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, body: &str, mode| {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        p
    };
    let noexec = write("noexec", "#!/bin/sh\necho hi\n", 0o644);
    let badshebang = write("badshebang", "#!/nonexistent/interpreter\necho hi\n", 0o755);
    let cases = [
        Path::new("/nonexistent/claude"),
        Path::new("no-such-claude-binary"),
        &noexec,
        &badshebang,
        &dir,
    ];
    for path in cases {
        let r = try_spawn(path, &[]);
        println!("{}: {:?}", path.display(), r.as_ref().map(|(_, c)| c.id()));
        assert!(r.is_err(), "spawn returned Ok for {}", path.display());
    }
    let (_pty, mut child) = try_spawn(Path::new("/usr/bin/true"), &[]).unwrap();
    assert!(child.wait().await.unwrap().success());
}

#[tokio::test]
async fn exit_code_and_signal_are_distinguished() {
    let _g = shared();
    for (script, want) in [
        ("exit 3", (Some(3), None)),
        ("kill -TERM $$", (None, Some(15))),
        ("kill -KILL $$", (None, Some(9))),
        ("exit 0", (Some(0), None)),
    ] {
        let (_r, _w, mut child, _) = bash(script);
        let st = child.wait().await.unwrap();
        assert_eq!((st.code(), st.signal()), want, "{script}");
    }
}

/// A leader that exits with any output unread is not reaped until the master is read: the
/// daemon must keep reading until wait returns.
#[tokio::test]
async fn exit_waits_for_unread_output() {
    let _g = shared();
    for (name, script, bytes) in [
        ("0 bytes", "exit 0", 0),
        ("2 bytes", "printf 'x\\n'; exit 0", 2),
        ("100 KB", "head -c 100000 /dev/zero | tr '\\0' x; exit 0", 100000),
    ] {
        let (mut r, _w, mut child, _) = bash(script);
        let t = Instant::now();
        // A child with nothing to say is reaped at once, however slow it was to start under a
        // loaded runner; one with output blocks, which a second shows.
        let limit = Duration::from_secs(if bytes == 0 { 10 } else { 1 });
        let unread = tokio::time::timeout(limit, child.wait()).await;
        let blocked = unread.is_err();
        let n = tokio::time::timeout(Duration::from_secs(5), drain(&mut r)).await.unwrap().len();
        let st = child.wait().await.unwrap();
        println!("{name}: wait blocked without reading: {blocked}; read {n}; {:?}", t.elapsed());
        assert!(st.success());
        assert_eq!(blocked, bytes > 0, "{name}");
        // The tty turns \n into \r\n.
        assert!(n == bytes || n == bytes + 1, "{name}: read {n}");
    }
}

#[tokio::test]
async fn twelve_concurrent_spawns_then_reaps_return_to_baseline() {
    let _g = FDS.write().unwrap_or_else(|e| e.into_inner());
    fn lsof(pid: u32) -> Vec<String> {
        let out = std::process::Command::new("lsof")
            .args(["-n", "-P", "-p", &pid.to_string()])
            .output()
            .unwrap();
        // Numbered fds only (not cwd, txt or mappings).
        text(&out.stdout)
            .lines()
            .skip(1)
            .filter(|l| {
                l.split_whitespace().nth(3).is_some_and(|f| f.starts_with(char::is_numeric))
            })
            .map(str::to_owned)
            .collect()
    }
    let me = std::process::id();
    // Warm-up: the first spawn creates tokio's SIGCHLD driver fds. The pause lets runtimes of
    // tests that just finished drop theirs.
    let (_r, _w, mut c, _) = bash("exit 0");
    c.wait().await.unwrap();
    drop((_r, _w));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let base = lsof(me);

    // Twelve spawns from twelve threads at once, racing the master's close-on-exec.
    let tasks: Vec<_> = (0..12)
        .map(|_| tokio::task::spawn_blocking(|| bash("echo READY; read x; exit 0")))
        .collect();
    let mut live = vec![];
    for t in tasks {
        let (mut r, w, child, _) = t.await.unwrap();
        read_until(&mut r, &mut vec![], "READY", 10).await;
        live.push((r, w, child));
    }
    println!("baseline {} fds, with 12 live {}", base.len(), lsof(me).len());
    // Each child holds its own pts only: no /dev/ptmx and no other tty.
    for (_, _, child) in &live {
        let mut ttys: Vec<_> = lsof(child.id().unwrap())
            .into_iter()
            .filter(|l| l.contains("/dev/tty") || l.contains("ptmx"))
            .map(|l| l.split_whitespace().last().unwrap().to_owned())
            .collect();
        ttys.dedup();
        assert!(ttys.len() == 1 && !ttys[0].contains("ptmx"), "child holds {ttys:?}");
    }
    for (mut r, mut w, mut child) in live {
        w.write_all(b"\n").await.unwrap();
        drain(&mut r).await;
        child.wait().await.unwrap();
    }
    let after = lsof(me);
    let new: Vec<_> = after.iter().filter(|l| !base.contains(l)).collect();
    assert_eq!(after.len(), base.len(), "new fds: {new:#?}");
}

#[tokio::test]
async fn wait_returns_while_a_setsid_grandchild_holds_the_slave() {
    let _g = shared();
    // macOS has no setsid(1); perl's POSIX::setsid stands in. The grandchild keeps the slave
    // as stdout and tries to write a line a second after it has said it is there, by which time
    // the leader, which waits for that, is long gone.
    let there = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("gc-{}", std::process::id()));
    let _ = std::fs::remove_file(&there);
    let (mut r, _w, mut child, _) = bash(&format!(
        "perl -e 'use POSIX; setsid(); $|=1; print \"GC $$\\n\"; open(F, \">{t}\"); close(F); \
         select(undef,undef,undef,1); print \"LATE\\n\"; exec \"sleep\", \"303\"' & \
         while [ ! -e {t} ]; do sleep 0.01; done; echo EXITING; exit 0",
        t = there.display()
    ));
    let pid = child.id().unwrap() as i32;
    let mut buf = vec![];
    read_until(&mut r, &mut buf, "GC ", 5).await;
    read_until(&mut r, &mut buf, "EXITING", 5).await;
    let t = Instant::now();
    child.wait().await.unwrap();
    let waited = t.elapsed();
    let gc = tagged(&text(&buf), "GC");
    // The leader's exit revokes the slave: EOF comes at once and the late write is lost.
    let second = Duration::from_secs(1);
    let eof = tokio::time::timeout(second, drain(&mut r)).await;
    let eof_at = t.elapsed();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let later = tokio::time::timeout(second, drain(&mut r)).await;
    let _ = std::fs::remove_file(&there);
    buf.extend(eof.iter().chain(later.iter()).flatten());
    // Reparented to launchd in a session of its own, it is out of reach of a later snapshot.
    let (reach, survived) = (pty::snapshot(pid).contains(&gc), !pty::gone(gc));
    // SAFETY: plain kill(2) on our own grandchild.
    unsafe { libc::kill(gc, libc::SIGKILL) };
    println!("wait returned in {waited:?}, EOF at {eof_at:?}; grandchild alive {survived}");
    assert!(waited < Duration::from_millis(250), "{waited:?}");
    assert!(eof.is_ok() && later.is_ok());
    assert!(!text(&buf).contains("LATE"), "{:?}", text(&buf));
    assert!(survived && !reach);
}

/// 256 KiB into a child that reads 4 KiB per ~2 ms with echo on, so it writes back as much as
/// it is sent: if the host stopped reading while writing, both sides would block.
#[tokio::test]
async fn paste_256k_into_a_slow_reader() {
    let _g = shared();
    const N: usize = 262144;
    let (mut r, mut w, mut child, _) = bash(
        "stty raw echo; echo READY; perl -e '$n=0; $p=0; while ($n < 262144) { \
         $k = sysread(STDIN, $b, 4096) or last; $n += $k; \
         if (int($n/16384) > $p) { $p = int($n/16384); print STDERR \"[P$n]\" } \
         select(undef,undef,undef,0.002) } print \"\\r\\nWC $n\\r\\n\"' | cat; stty sane",
    );
    read_until(&mut r, &mut vec![], "READY", 5).await;
    let payload: Vec<u8> = (0..N).map(|i| b'a' + (i % 26) as u8).collect();
    let t = Instant::now();
    let writer = tokio::spawn(async move {
        w.write_all(&payload).await.unwrap();
        (t.elapsed(), w)
    });
    let (mut out, mut chunk, mut marks_while_writing) = (vec![], vec![0u8; 65536], 0);
    while !text(&out).contains("WC ") || !text(&out).ends_with("\r\n") {
        let k = tokio::time::timeout(Duration::from_secs(30), r.read(&mut chunk))
            .await
            .expect("no output for 30 s")
            .unwrap();
        if !writer.is_finished() {
            marks_while_writing += text(&chunk[..k]).matches("[P").count();
        }
        out.extend_from_slice(&chunk[..k]);
    }
    let (write_time, _w) = writer.await.unwrap();
    println!("written in {write_time:?}, done in {:?}", t.elapsed());
    assert_eq!(tagged(&text(&out), "WC"), N as i32);
    assert!(marks_while_writing > 0);
    drain(&mut r).await;
    child.wait().await.unwrap();
}

#[tokio::test]
async fn resize_reaches_the_child() {
    let _g = shared();
    let (mut r, w, mut child, _) = bash(
        "trap 'echo WINCH $(stty size)' WINCH; echo INITIAL $(stty size); echo READY; \
         while :; do sleep 0.02; done",
    );
    let mut buf = vec![];
    read_until(&mut r, &mut buf, "READY", 5).await;
    assert!(text(&buf).contains("INITIAL 24 80"));
    for (rows, cols) in [(40, 120), (10, 50), (60, 200)] {
        w.resize(Size::new(rows, cols)).unwrap();
        read_until(&mut r, &mut buf, &format!("WINCH {rows} {cols}"), 5).await;
    }
    child.start_kill().unwrap();
    child.wait().await.unwrap();
}

#[tokio::test]
async fn sweep_leaves_no_descendant() {
    let _g = shared();
    // One descendant in the leader's pgrp, one in its own pgrp, one setsid'd, and one that
    // ignores HUP and TERM so the KILL stage has work. The leader waits on them.
    let (mut r, _w, mut child, _) = bash(
        "sleep 304 & echo SAME $!; \
         perl -e 'setpgrp(0,0); exec \"sleep\", \"305\"' & echo OWNPGRP $!; \
         perl -e 'use POSIX; setsid(); exec \"sleep\", \"306\"' & echo SETSID $!; \
         bash -c 'trap \"\" HUP TERM; exec sleep 307' & echo STUBBORN $!; \
         sleep 0.2; echo READY; wait",
    );
    let leader = child.id().unwrap() as i32;
    let mut buf = vec![];
    read_until(&mut r, &mut buf, "READY", 5).await;
    let out = text(&buf);
    let mut pids = vec![leader];
    pids.extend(["SAME", "OWNPGRP", "SETSID", "STUBBORN"].map(|tag| tagged(&out, tag)));
    let stubborn = pids[4];

    let t = Instant::now();
    // Reaped alongside, as the daemon does: an unreaped zombie leader counts as alive.
    let ((set, log), _) =
        tokio::join!(pty::sweep(leader, Duration::from_millis(250)), child.wait());
    let took = t.elapsed();
    let deadline = Instant::now() + Duration::from_secs(1);
    while pids.iter().any(|&p| !pty::gone(p)) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    println!("sweep took {took:?}; {log:?}");
    let left: Vec<_> = pids.iter().filter(|&&p| !pty::gone(p)).collect();
    assert!(left.is_empty(), "alive after sweep: {left:?}");
    assert!(pids.iter().all(|p| set.contains(p)));
    let (sig, killed) = log.last().unwrap();
    assert_eq!((*sig, killed.as_slice()), (libc::SIGKILL, &[stubborn][..]));
}

#[tokio::test]
async fn sweep_ends_early_when_the_leader_leaves_on_hup() {
    let _g = shared();
    let (mut r, _w, mut child, _) = bash("sleep 308 & echo CHILD $!; echo READY; wait");
    let leader = child.id().unwrap() as i32;
    let mut buf = vec![];
    read_until(&mut r, &mut buf, "READY", 5).await;
    let sleeper = tagged(&text(&buf), "CHILD");
    let t = Instant::now();
    let ((_, log), _) = tokio::join!(pty::sweep(leader, Duration::from_secs(5)), child.wait());
    let took = t.elapsed();
    println!("sweep took {took:?}; {log:?}");
    assert!(took < Duration::from_secs(1), "{took:?}");
    assert!(pty::gone(sleeper), "{log:?}");
}
