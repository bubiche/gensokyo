//! The client in a pseudo-terminal, against a daemon whose residents are tests/stub-claude. What
//! it draws is read back through the emulator, which also answers its queries as a terminal
//! would (kitty flags included), so the host side behaves like iTerm2's.

use gensokyo::vt::{Style, Vt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BIN: &str = env!("CARGO_BIN_EXE_gensokyo");

fn env(dir: &Path) -> Vec<(String, String)> {
    vec![
        ("GENSOKYO_STATE_DIR".into(), dir.display().to_string()),
        (
            "GENSOKYO_CLAUDE".into(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/stub-claude").into(),
        ),
        ("STUB_STATE".into(), dir.join("stub").display().to_string()),
        ("CLAUDE_CONFIG_DIR".into(), dir.join("claude").display().to_string()),
        ("GENSOKYO_CLIENT_LOG".into(), dir.join("client.log").display().to_string()),
        ("HOME".into(), dir.display().to_string()),
        ("STUB_WINCH".into(), "1".into()),
        ("GENSOKYO_COPY".into(), format!("cat > '{}'", dir.join("copied").display())),
    ]
}

fn state(name: &str) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("c-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("work")).unwrap();
    dir
}

struct Client {
    pty: pty_process::Pty,
    child: tokio::process::Child,
    vt: Vt,
    /// Everything the client wrote.
    seen: Vec<u8>,
}

impl Client {
    fn spawn(dir: &Path, cols: u16, rows: u16) -> Client {
        let (pty, pts) = pty_process::open().unwrap();
        pty.resize(pty_process::Size::new(rows, cols)).unwrap();
        let child = pty_process::Command::new(BIN)
            .envs(env(dir))
            .env_remove("GENSOKYO_SOCKET")
            .env("TERM", "xterm-256color")
            .current_dir(dir.join("work"))
            .spawn(pts)
            .unwrap();
        Client { pty, child, vt: Vt::new(cols, rows), seen: Vec::new() }
    }

    async fn send(&mut self, b: &[u8]) {
        self.pty.write_all(b).await.unwrap();
    }

    /// Reads and answers until the screen satisfies `ok`, or fails after 10 s. The screen is
    /// looked at only between frames, never halfway through one.
    async fn wait(&mut self, what: &str, ok: impl Fn(&str) -> bool) -> String {
        let t = Instant::now();
        let mut buf = vec![0u8; 65536];
        loop {
            let screen = self.vt.frame().text().join("\n");
            if !self.vt.mode(2026) && ok(&screen) {
                return screen;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "waiting for {what}; screen:\n{screen}");
            if let Ok(Ok(n @ 1..)) =
                tokio::time::timeout(Duration::from_millis(50), self.pty.read(&mut buf)).await
            {
                self.vt.feed(&buf[..n]);
                self.seen.extend_from_slice(&buf[..n]);
                let r = self.vt.take_replies();
                if !r.is_empty() {
                    self.send(&r).await;
                }
            }
        }
    }

    /// The text of every reversed run on screen, row by row.
    fn reversed(&mut self) -> Vec<(usize, String)> {
        let fr = self.vt.frame();
        let runs = fr.rows.iter().enumerate().flat_map(|(y, r)| r.iter().map(move |r| (y, r)));
        runs.filter(|(_, r)| r.style.attrs & Style::INVERSE != 0)
            .map(|(y, r)| (y + 1, r.text.clone()))
            .collect()
    }

    /// A left drag from one cell to another, 1-based, released at the second.
    async fn drag(&mut self, (x0, y0): (u16, u16), (x1, y1): (u16, u16)) {
        let (mx, my) = ((x0 + x1) / 2, (y0 + y1) / 2);
        let s =
            format!("\x1b[<0;{x0};{y0}M\x1b[<32;{mx};{my}M\x1b[<32;{x1};{y1}M\x1b[<0;{x1};{y1}m");
        self.send(s.as_bytes()).await;
    }

    /// Reads until the client exits; its status.
    async fn exit(&mut self) -> std::process::ExitStatus {
        let mut buf = vec![0u8; 65536];
        let t = Instant::now();
        loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                // A terminal that is slow to answer a cursor query must not matter: the client
                // never asks.
                assert!(!self.seen.windows(4).any(|w| w == b"\x1b[6n"), "the client sent CSI 6n");
                return st;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "the client did not exit");
            if let Ok(Ok(n @ 1..)) =
                tokio::time::timeout(Duration::from_millis(50), self.pty.read(&mut buf)).await
            {
                self.vt.feed(&buf[..n]);
            }
        }
    }
}

/// One of the stub's files (there is one resident): `input` holds every line it read, `size`
/// the tty's `rows cols` after the last SIGWINCH.
fn stub(dir: &Path, ext: &str) -> String {
    let f = std::fs::read_dir(dir.join("stub"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == ext));
    f.and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default()
}

/// Quits the test's daemon when dropped, so a failed test leaves nothing running.
struct Quit(PathBuf);

impl Drop for Quit {
    fn drop(&mut self) {
        let _ =
            Command::new(BIN).arg("quit").envs(env(&self.0)).env_remove("GENSOKYO_SOCKET").output();
    }
}

/// Where the row holding `text` is on screen, 1-based, and the column it starts at.
fn find(screen: &str, text: &str) -> (u16, u16) {
    for (y, line) in screen.lines().enumerate() {
        if let Some(x) = line.find(text) {
            let col = line[..x].chars().count();
            return (col as u16 + 1, y as u16 + 1);
        }
    }
    panic!("{text:?} not on screen:\n{screen}");
}

#[tokio::test(flavor = "current_thread")]
async fn summon_type_click_detach_reattach() {
    let dir = state("summon");
    let _quit = Quit(dir.clone());
    let mut c = Client::spawn(&dir, 120, 40);
    c.wait("the empty shrine", |s| s.contains("the shrine is empty")).await;

    // Leader, n: the summon modal; Enter takes the client's own directory, Enter again a name.
    c.send(b"\x1d").await;
    c.send(b"n").await;
    c.wait("the summon modal", |s| s.contains("Where?")).await;
    c.send(b"\r").await;
    c.wait("the name stage", |s| s.contains("random name")).await;
    for ch in "Reimu".bytes() {
        c.send(&[ch]).await;
    }
    c.send(b"\r").await;
    let s = c.wait("the stub on screen", |s| s.contains("stub-claude Reimu")).await;
    assert!(s.contains("1 ○ Reimu"), "{s}");
    // Typing waits until the stub reads, and past the reattach nudge's two resizes: a SIGWINCH
    // in the middle of the stub's `read` would lose the line.
    c.wait("the stub reading", |_| {
        dir.join("stub").read_dir().is_ok_and(|mut d| {
            d.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "ready")))
        })
    })
    .await;
    let t = Instant::now();
    c.wait("the nudge to pass", |_| t.elapsed() > Duration::from_millis(200)).await;

    // Typing reaches the resident.
    c.send(b"hello\r").await;
    let s = c.wait("the echo", |s| s.contains("> hello")).await;

    // A drag in the grid selects: highlighted, and copied on release.
    let copied = || std::fs::read_to_string(dir.join("copied")).unwrap_or_default();
    let (x, y) = find(&s, "> hello");
    c.drag((x, y), (x + 6, y)).await;
    c.wait("the copy", |s| s.contains("copied 7 characters")).await;
    c.wait("the copied text", |_| copied() == "> hello").await;
    assert_eq!(c.reversed(), [(y as usize, "> hello".to_string())]);
    // Dragged back up and out over the sidebar, it stops at the grid's edge: rows in reading
    // order, and nothing of the sidebar.
    let (_, top) = find(&s, "stub-claude Reimu");
    c.drag((x + 6, y), (1, top)).await;
    let grid = |l: &str| l.chars().skip(26).take(93).collect::<String>().trim_end().to_string();
    let rows: Vec<&str> = s.lines().skip(top as usize - 1).take((y - top + 1) as usize).collect();
    let want = rows.iter().map(|l| grid(l)).collect::<Vec<_>>().join("\n");
    c.wait("the second copy", |_| copied() == want).await;
    assert!(want.starts_with("stub-claude Reimu") && !want.contains(['○', '│']), "{want:?}");
    // A key clears it.
    c.send(b"x\x7f").await;
    let t = Instant::now();
    c.wait("a frame after the key", |_| t.elapsed() > Duration::from_millis(100)).await;
    assert_eq!(c.reversed(), []);

    // With the resident's mouse mode on, a grid click reaches it grid-relative...
    c.send(b"/mouse\r").await;
    let s = c.wait("mouse mode", |s| s.contains("mouse on")).await;
    let (x, y) = find(&s, "mouse on");
    c.send(format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m").as_bytes()).await;
    c.send(b"after\r").await;
    c.wait("the echo after the click", |s| s.contains("after")).await;
    let (gx, gy) = (x - 26, y - 1);
    let want = format!("\x1b[<0;{gx};{gy}M\x1b[<0;{gx};{gy}mafter");
    assert_eq!(stub(&dir, "input").lines().last(), Some(want.as_str()));

    // ...and a click on chrome opens the modal and never reaches it.
    let (x, y) = find(&s, "[summon n]");
    c.send(format!("\x1b[<0;{};{y}M\x1b[<0;{};{y}m", x + 1, x + 1).as_bytes()).await;
    c.wait("the summon modal again", |s| s.contains("Where?")).await;
    c.send(b"\x1b").await;
    c.wait("the modal closed", |s| !s.contains("Where?")).await;
    c.send(b"probe\r").await;
    c.wait("the probe", |s| s.contains("> probe")).await;
    assert_eq!(stub(&dir, "input").lines().last(), Some("probe"));

    // Detach: the client leaves, the resident stays; a new client draws it again, at its grid
    // size (the window less the 25-column sidebar and the main box).
    c.send(b"\x1dd").await;
    assert!(c.exit().await.success());
    let mut c = Client::spawn(&dir, 100, 30);
    c.wait("the new grid size", |_| stub(&dir, "size").trim() == "28 73").await;
    let s =
        c.wait("the resident again", |s| s.contains("1 ○ Reimu") && s.contains("> probe")).await;
    assert!(s.contains("stub-claude Reimu") && !s.contains("╱╨╲"), "{s}");
    // The host is at the child's kitty flags, so Shift+Enter goes through as it came. (The
    // stub's echo of it is a cursor restore, so it goes last.)
    c.send(b"one\x1b[13;2utwo\r").await;
    c.wait("the second echo", |s| s.contains("two")).await;
    assert!(stub(&dir, "input").contains("one\x1b[13;2utwo"), "{:?}", stub(&dir, "input"));
    c.send(b"\x1dd").await;
    assert!(c.exit().await.success());
}

/// `gensokyo <args>` from the test's state dir, with the stub's hooks on.
fn cli(dir: &Path, args: &[&str]) {
    let out = Command::new(BIN)
        .args(args)
        .envs(env(dir))
        .env("STUB_HOOKS", "1")
        .env_remove("GENSOKYO_SOCKET")
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[tokio::test(flavor = "current_thread")]
async fn a_resident_nobody_watches_rings_the_host() {
    let dir = state("ring");
    let _quit = Quit(dir.clone());
    cli(&dir, &["new", "work", "-n", "Reimu"]);
    cli(&dir, &["new", "work", "-n", "Marisa"]);
    let mut c = Client::spawn(&dir, 120, 40);
    c.wait("both residents", |s| s.contains("1 ○ Reimu") && s.contains("2 ○ Marisa")).await;
    let ready = || {
        dir.join("stub").read_dir().map_or(0, |d| {
            d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "ready")).count()
        })
    };
    c.wait("both stubs reading", |_| ready() == 2).await;
    // On Marisa, in a focused terminal: her finished turn is seen as it finishes, so she never
    // turns gold, and nothing rings.
    c.send(b"\x1b[I\x1d2").await;
    c.wait("Marisa on screen", |s| s.contains("stub-claude Marisa")).await;
    let t = Instant::now();
    c.wait("the nudge to pass", |_| t.elapsed() > Duration::from_millis(300)).await;
    c.send(b"hi\r").await;
    let stopped = |n: usize| {
        let log = std::fs::read_to_string(dir.join("daemon.log")).unwrap_or_default();
        log.lines().filter(|l| l.contains("\"event\":\"Stop\"")).count() == n
    };
    c.wait("Marisa done", |s| stopped(1) && s.contains("> hi")).await;
    let t = Instant::now();
    let s = c.wait("a frame after", |_| t.elapsed() > Duration::from_millis(200)).await;
    assert!(s.contains("2 ○ Marisa"), "{s}");
    let rang = |seen: &[u8], text: &str| {
        let want = format!("\x07\x1b]9;{text}\x07").into_bytes();
        seen.windows(want.len()).any(|w| w == want.as_slice())
    };
    assert!(!c.seen.windows(4).any(|w| w == b"\x1b]9;"), "a notification while watched");

    // The terminal loses focus: the next one is a bell and an OSC 9 notification.
    c.send(b"\x1b[O").await;
    c.send(b"again\r").await;
    c.wait("the notice", |s| s.contains("✦ Marisa is done")).await;
    assert!(rang(&c.seen, "Marisa is done: echo: again"), "{:?}", String::from_utf8_lossy(&c.seen));
    let s = c.wait("the gold", |s| s.contains("2 ✦ Marisa")).await;
    assert!(s.contains("2 ✦ Marisa"));
    // Back to the terminal, with her on screen: seen.
    c.send(b"\x1b[I").await;
    c.wait("Marisa seen", |s| s.contains("2 ○ Marisa")).await;
    c.send(b"\x1dd").await;
    assert!(c.exit().await.success());
}
