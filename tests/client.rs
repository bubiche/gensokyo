//! The client in a pseudo-terminal, against a daemon whose residents are tests/stub-claude. What
//! it draws is read back through the emulator, which also answers its queries as a terminal
//! would (kitty flags included), so the host side behaves like iTerm2's.

mod common;

use common::{BIN, fresh, stub_env};
use gensokyo::vt::{Style, Vt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn env(dir: &Path) -> Vec<(String, String)> {
    let mut env = stub_env(dir);
    env.extend([
        ("GENSOKYO_CLIENT_LOG".into(), dir.join("client.log").display().to_string()),
        ("HOME".into(), dir.display().to_string()),
        ("STUB_WINCH".into(), "1".into()),
        ("GENSOKYO_COPY".into(), format!("cat > '{}'", dir.join("copied").display())),
    ]);
    env
}

fn state(name: &str) -> PathBuf {
    let dir = fresh(&format!("c-{name}"));
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
                self.seen.extend_from_slice(&buf[..n]);
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

/// How many of the stubs are reading their tty.
fn ready(dir: &Path) -> usize {
    dir.join("stub").read_dir().map_or(0, |d| {
        d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "ready")).count()
    })
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

    // Leader, n: the summon modal; Enter takes the client's own directory, then a name.
    c.send(b"\x1d").await;
    c.send(b"n").await;
    c.wait("the summon modal", |s| s.contains("Where?")).await;
    c.send(b"\r").await;
    c.wait("the name stage", |s| s.contains("random name")).await;
    for ch in "Reimu".bytes() {
        c.send(&[ch]).await;
    }
    c.send(b"\r").await;
    // In a git repository (a build's target/ is in this one) a worktree is asked for next:
    // Enter leaves it empty, which works right there.
    let worktree = |s: &str| s.contains("Enter works right here");
    let s = c
        .wait("the stub, or the worktree stage", |s| worktree(s) || s.contains("stub-claude Reimu"))
        .await;
    if worktree(&s) {
        c.send(b"\r").await;
    }
    let s = c.wait("the stub on screen", |s| s.contains("stub-claude Reimu")).await;
    assert!(s.contains("1 ○ Reimu"), "{s}");
    // Typing waits until the stub reads, and past the reattach nudge's two resizes: a SIGWINCH
    // in the middle of the stub's `read` would lose the line.
    c.wait("the stub reading", |_| ready(&dir) == 1).await;
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
    let (_, top) = find(&s, "gensokyo on PATH");
    c.drag((x + 6, y), (1, top)).await;
    let grid = |l: &str| l.chars().skip(26).take(93).collect::<String>().trim_end().to_string();
    let rows: Vec<&str> = s.lines().skip(top as usize - 1).take((y - top + 1) as usize).collect();
    let want = rows.iter().map(|l| grid(l)).collect::<Vec<_>>().join("\n");
    c.wait("the second copy", |_| copied() == want).await;
    assert!(want.starts_with("gensokyo on PATH") && !want.contains(['○', '│']), "{want:?}");
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
    // The tty echoes it before the stub has read it: the stub's record is what counts.
    let (gx, gy) = (x - 26, y - 1);
    let want = format!("\x1b[<0;{gx};{gy}M\x1b[<0;{gx};{gy}mafter");
    let got = |_: &str| stub(&dir, "input").lines().last() == Some(want.as_str());
    c.wait("the click and the line after it", got).await;

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
    c.wait("Shift+Enter as it came", |_| stub(&dir, "input").contains("one\x1b[13;2utwo")).await;
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
    c.wait("both stubs reading", |_| ready(&dir) == 2).await;
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

    // Leader, c: a card (the user's own, first by title), then everyone (up from Marisa, on
    // screen, where the targets start): both get it, filled in for each.
    let cards = dir.join("conf/spellcards");
    std::fs::create_dir_all(&cards).unwrap();
    std::fs::write(cards.join("probe.md"), "---\ntitle: Aa Probe\n---\nprobe for {self}\n")
        .unwrap();
    c.send(b"\x1dc").await;
    c.wait("the cards", |s| s.contains("› Aa Probe")).await;
    c.send(b"\r").await;
    c.wait("the targets", |s| s.contains("› 2 ○ Marisa")).await;
    for row in ["› 1 ○ Reimu", "› everyone resting", "› everyone who needs you", "› everyone  "]
    {
        c.send(b"k").await;
        c.wait(row, |s| s.contains(row)).await;
    }
    let t = Instant::now();
    c.wait("a breath", |_| t.elapsed() > Duration::from_millis(400)).await;
    c.send(b"\r").await;
    // The reply wraps in the sidebar: its rows, read as one line.
    let side = |s: &str| {
        let rows = s.lines().map(|l| l.chars().skip(1).take(23).collect::<String>());
        rows.map(|r| r.trim().to_string()).collect::<Vec<_>>().join(" ")
    };
    c.wait("the cast", |s| side(s).contains("cast Aa Probe on Reimu and Marisa")).await;
    let inputs = std::fs::read_dir(dir.join("stub")).unwrap().flatten().map(|e| e.path());
    let inputs: String = inputs
        .filter(|p| p.extension().is_some_and(|x| x == "input"))
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    for who in ["Reimu", "Marisa"] {
        assert!(inputs.contains(&format!("\x1b[200~probe for {who}\x1b[201~")), "{inputs:?}");
    }

    // Leader, t: the timetable, as the daemon lists it; Esc closes it.
    c.send(b"\x1dt").await;
    c.wait("the timetable", |s| s.contains("┌ timetable") && !s.contains("loading")).await;
    c.send(b"\x1b").await;
    c.wait("the timetable closed", |s| !s.contains("┌ timetable")).await;

    // She leaves, and is recalled from another shell under the same id: her new screen comes.
    c.send(b"/exit\r").await;
    c.wait("the departed screen", |s| s.contains("[ recall r ]")).await;
    cli(&dir, &["resume", "Marisa"]);
    c.wait("Marisa back", |s| s.contains("stub-claude Marisa") && !s.contains("[ recall r ]"))
        .await;
    c.send(b"\x1dd").await;
    assert!(c.exit().await.success());
}

#[tokio::test(flavor = "current_thread")]
async fn the_timetable_runs_pauses_and_removes_a_ritual_and_keeps_the_examples() {
    let dir = state("timetable");
    let _quit = Quit(dir.clone());
    let rituals = dir.join("conf/rituals");
    std::fs::create_dir_all(&rituals).unwrap();
    let file = rituals.join("tt.md");
    let text = format!(
        "---\nschedule: \"@yearly\"\nheadless: true\ncwd: \"{}\"\n---\nSay the time.\n",
        dir.join("work").display()
    );
    std::fs::write(&file, text).unwrap();
    let mut c = Client::spawn(&dir, 120, 40);
    // The examples are paused, so the sidebar's next fire is this one; a click there opens the
    // timetable.
    let s = c.wait("the next fire", |s| s.contains("⏲ tt")).await;
    let (x, y) = find(&s, "⏲ tt");
    c.send(format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m").as_bytes()).await;
    let s =
        c.wait("the timetable", |s| s.contains("┌ timetable") && s.contains("slack-morning")).await;
    let (x, y) = find(&s, " tt ");
    c.send(format!("\x1b[<0;{};{y}M\x1b[<0;{};{y}m", x + 1, x + 1).as_bytes()).await;
    c.wait("its detail", |s| s.contains("┌ tt ") && s.contains("last ran  never")).await;

    // Run now: the run goes headless, and the detail says when it ran.
    c.send(b"r").await;
    c.wait("the run", |s| s.contains("last ran ") && !s.contains("last ran  never")).await;
    let journal =
        || std::fs::read_to_string(dir.join("rituals/tt/journal.jsonl")).unwrap_or_default();
    // Done, too: a ritual whose run is still going is not removed.
    let done =
        |s: &str| journal().contains("done (headless") && !s.contains("a run of it is going");
    c.wait("its run done", done).await;

    // Pause and resume: the file says so, and so do the buttons.
    c.send(b"p").await;
    c.wait("paused", |s| s.contains("[resume p]")).await;
    assert!(std::fs::read_to_string(&file).unwrap().contains("enabled: false"));
    c.send(b"p").await;
    c.wait("resumed", |s| s.contains("[pause p]")).await;
    assert!(std::fs::read_to_string(&file).unwrap().contains("enabled: true"));

    // Remove asks; n keeps it. A yes a moment later takes it, and the list comes back without it.
    c.send(b"x").await;
    c.wait("the question", |s| s.contains("Remove tt?")).await;
    c.send(b"n").await;
    c.wait("kept", |s| !s.contains("Remove tt?") && s.contains("[pause p]")).await;
    c.send(b"x").await;
    c.wait("the question again", |s| s.contains("Remove tt?")).await;
    let t = Instant::now();
    c.wait("a breath", |_| t.elapsed() > Duration::from_millis(400)).await;
    c.send(b"y").await;
    let s =
        c.wait("the list without it", |s| s.contains("┌ timetable") && !s.contains(" tt ")).await;
    assert!(!file.exists() && !dir.join("rituals/tt").exists());

    // An example has no remove.
    let (x, y) = find(&s, "slack-morning");
    c.send(format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m").as_bytes()).await;
    let s = c.wait("the example", |s| s.contains("┌ slack-morning ")).await;
    assert!(s.contains("[resume p]") && !s.contains("[remove x]"), "{s}");
    c.send(b"\x1b").await;
    c.wait("the list", |s| s.contains("┌ timetable")).await;
    c.send(b"\x1b").await;
    c.wait("closed", |s| !s.contains("┌ timetable")).await;
    c.send(b"\x1dd").await;
    assert!(c.exit().await.success());
}

#[tokio::test(flavor = "current_thread")]
async fn a_search_moves_the_view_to_each_match_and_its_keys_never_reach_the_resident() {
    let dir = state("search");
    let _quit = Quit(dir.clone());
    cli(&dir, &["new", "work", "-n", "Reimu"]);
    let mut c = Client::spawn(&dir, 120, 40);
    c.wait("Reimu", |s| s.contains("stub-claude Reimu")).await;
    c.wait("the stub reading", |_| ready(&dir) == 1).await;
    let t = Instant::now();
    c.wait("the nudge to pass", |_| t.elapsed() > Duration::from_millis(300)).await;
    // 120 lines, a mark every 30 from the 10th: the stub echoes them after the tty did.
    let lines: Vec<String> = (0..120)
        .map(|i| if i % 30 == 10 && i < 100 { format!("mark{i}") } else { format!("line{i}") })
        .collect();
    c.send(format!("\x1b[200~{}\x1b[201~\r", lines.join("\r")).as_bytes()).await;
    c.wait("the echo", |s| s.contains("> line119")).await;

    // Back from the live screen: the newest above it first, then older with n.
    c.send(b"\x1d/").await;
    c.wait("the prompt", |s| s.contains("└ / ")).await;
    c.send(b"mark").await;
    c.wait("what is typed", |s| s.contains("└ /mark ")).await;
    c.send(b"\r").await;
    let at = |n: &'static str, not: &'static str| {
        move |s: &str| s.contains("/mark · n N") && s.contains(n) && !s.contains(not)
    };
    c.wait("the newest", at("> mark70", "> line119")).await;
    c.send(b"n").await;
    c.wait("the one before", at("> mark40", "> mark70")).await;
    c.send(b"N").await;
    c.wait("forward again", at("> mark70", "> mark40")).await;
    // On toward the live screen: the view goes home, and the keys stay the search's.
    c.send(b"?").await;
    c.wait("the prompt, on", |s| s.contains("└ ? ")).await;
    c.send(b"line119\r").await;
    c.wait("on the live screen", |s| s.contains("?line119 · n N") && !s.contains("↑ ")).await;
    c.send(b"n").await;
    c.wait("nothing newer", |s| s.contains("no “line119” further on")).await;
    // Esc ends it; n then is the resident's.
    c.send(b"\x1b").await;
    c.wait("the search over", |s| !s.contains("n N for more")).await;
    c.send(b"n\r").await;
    c.wait("the n read", |_| stub(&dir, "input").lines().last() == Some("n")).await;
    assert!(!stub(&dir, "input").contains("mark\n"), "the search went to the resident");
    c.send(b"\x1dd").await;
    assert!(c.exit().await.success());
}

#[tokio::test(flavor = "current_thread")]
async fn the_wheel_scrolls_back_for_every_client_and_a_letter_goes_home_and_on() {
    let dir = state("scroll");
    let _quit = Quit(dir.clone());
    cli(&dir, &["new", "work", "-n", "Reimu"]);
    let mut c = Client::spawn(&dir, 120, 40);
    let mut c2 = Client::spawn(&dir, 120, 40);
    c.wait("Reimu", |s| s.contains("stub-claude Reimu")).await;
    c2.wait("Reimu, twice", |s| s.contains("stub-claude Reimu")).await;
    c.wait("the stub reading", |_| ready(&dir) == 1).await;
    let t = Instant::now();
    c.wait("the nudge to pass", |_| t.elapsed() > Duration::from_millis(300)).await;
    // One paste, which the stub echoes whole: 60 lines, more than the screen holds.
    let lines: Vec<String> = (0..60).map(|i| format!("line{i}")).collect();
    c.send(format!("\x1b[200~{}\x1b[201~\r", lines.join("\r")).as_bytes()).await;
    c.wait("the echo", |s| s.contains("> line59")).await;

    // The wheel over the grid: three rows back, on both clients' screens.
    let back =
        |n: u64| move |s: &str| s.contains(&format!("↑ {n} of ")) && s.contains("esc returns");
    c.send(b"\x1b[<64;60;20M").await;
    c.wait("three rows back", back(3)).await;
    c2.wait("three rows back, for the other client too", back(3)).await;
    // The chord, half a screen more; g the top; q home.
    c.send(b"\x1d[").await;
    c.wait("further back", |s| s.contains("↑ ") && !back(3)(s)).await;
    c.send(b"g").await;
    // The tty echoed the paste before the stub did: its first line is the top's.
    c.wait("the top", |s| s.contains("200~line0") && !s.contains("> line59")).await;
    c.send(b"q").await;
    c.wait("home", |s| !s.contains("esc returns") && s.contains("> line59")).await;
    c2.wait("home, for the other client too", |s| !s.contains("esc returns")).await;

    // A letter while scrolled back goes home and on to the resident.
    c.send(b"\x1b[<64;60;20M").await;
    c.wait("back again", back(3)).await;
    c.send(b"x").await;
    c.wait("home with the letter", |s| !s.contains("esc returns") && s.contains("││x ")).await;
    c.send(b"\r").await;
    c.wait("the letter read", |_| stub(&dir, "input").lines().last() == Some("x")).await;
    // Output that comes while scrolled back keeps the view on its rows, so the stub's answer
    // has to be in first.
    c.wait("the stub's answer", |s| s.contains("> x")).await;

    // Detached while scrolled back, the view is the resident's: it is still there on return.
    c.send(b"\x1b[<64;60;20M").await;
    c.wait("back once more", back(3)).await;
    c.send(b"\x1dd").await;
    assert!(c.exit().await.success());
    let mut c = Client::spawn(&dir, 120, 40);
    c.wait("still back", back(3)).await;
    c.send(b"\x1b").await;
    c.wait("home at last", |s| !s.contains("esc returns")).await;
    for mut c in [c, c2] {
        c.send(b"\x1dd").await;
        assert!(c.exit().await.success());
    }
}

/// The sidebar's rows read as one line, so a message that wraps there can be matched.
fn side(s: &str) -> String {
    let rows = s.lines().map(|l| l.chars().skip(1).take(23).collect::<String>());
    rows.map(|r| r.trim().to_string()).collect::<Vec<_>>().join(" ")
}

/// A grid row of a 120-column client's screen (1-based), trailing blanks dropped.
fn grid_row(s: &str, y: u16) -> String {
    let line = s.lines().nth(y as usize - 1).unwrap_or_default();
    line.chars().skip(26).take(93).collect::<String>().trim_end().to_string()
}

impl Client {
    /// One SGR mouse report: button, cell (1-based), press or release.
    async fn mouse(&mut self, b: u8, (x, y): (u16, u16), press: bool) {
        let end = if press { 'M' } else { 'm' };
        self.send(format!("\x1b[<{b};{x};{y}{end}").as_bytes()).await;
    }

    /// Reads until the reversed rows, trimmed, are `want`, or fails after 10 s.
    async fn picked(&mut self, want: &[(u16, &str)]) {
        let t = Instant::now();
        loop {
            let got =
                self.reversed().into_iter().map(|(y, s)| (y as u16, s.trim_end().to_string()));
            let got: Vec<(u16, String)> = got.filter(|(_, s)| !s.is_empty()).collect();
            if got.iter().map(|(y, s)| (*y, s.as_str())).eq(want.iter().copied()) {
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "selected {got:?}, not {want:?}");
            let t = Instant::now();
            self.wait("a frame", |_| t.elapsed() > Duration::from_millis(50)).await;
        }
    }
}

/// Reimu, summoned from the CLI, on screen in a 120x40 client, reading her tty.
async fn reimu(dir: &Path) -> Client {
    cli(dir, &["new", "work", "-n", "Reimu"]);
    let mut c = Client::spawn(dir, 120, 40);
    c.wait("Reimu", |s| s.contains("stub-claude Reimu")).await;
    // A focused terminal, watching her: her finished turns ring nothing over the messages.
    c.send(b"\x1b[I").await;
    c.wait("the stub reading", |_| ready(dir) == 1).await;
    let t = Instant::now();
    c.wait("the nudge to pass", |_| t.elapsed() > Duration::from_millis(300)).await;
    c
}

#[tokio::test(flavor = "current_thread")]
async fn a_drag_copies_soft_wraps_joined_line_breaks_kept_and_wide_characters_whole() {
    let dir = state("copy");
    let _quit = Quit(dir.clone());
    let mut c = reimu(&dir).await;
    let copied = || std::fs::read_to_string(dir.join("copied")).unwrap_or_default();

    // Before any turn, there is no answer to copy.
    c.send(b"\x1dy").await;
    c.wait("no answer", |s| side(s).contains("Reimu has not finished a turn yet")).await;

    // A line longer than the 93-column grid: the stub's echo of it wraps onto a second row.
    let long: Vec<String> = (0..30).map(|i| format!("w{i:02}")).collect();
    let long = long.join(" ");
    c.send(format!("{long}\r").as_bytes()).await;
    let s = c.wait("the long echo", |s| s.contains("> w00")).await;
    let (x, y) = find(&s, "> w00");
    assert!(grid_row(&s, y + 1).starts_with(" w23"), "{s}");
    c.drag((x, y), (119, y + 1)).await;
    let want = format!("> {long}");
    c.wait("the long line, joined", |_| copied() == want).await;
    c.wait("the message", |s| s.contains("copied 121 characters")).await;

    // A double click: the word.
    let w05 = (x + 2 + 5 * 4, y);
    for press in [true, false, true, false] {
        c.mouse(0, w05, press).await;
    }
    c.wait("the word", |_| copied() == "w05").await;
    c.picked(&[(y, "w05")]).await;

    // Two lines, each a line of its own.
    c.send(b"one\r").await;
    c.wait("one", |s| s.contains("> one")).await;
    c.send(b"two\r").await;
    let s = c.wait("two", |s| s.contains("> two")).await;
    let (x, y) = find(&s, "> one");
    c.drag((x, y), (x + 4, y + 2)).await;
    c.wait("the two lines", |_| copied() == "> one\ntwo\n> two").await;

    // Wide characters and a combining mark, whole, from either half of the first.
    c.send("日本語 🍣 cafe\u{301}\r".as_bytes()).await;
    let s = c.wait("the wide echo", |s| s.contains("> 日本語")).await;
    let (x, y) = find(&s, "> 日本語");
    c.drag((x, y), (119, y)).await;
    c.wait("the wide line", |_| copied() == "> 日本語 🍣 cafe\u{301}").await;
    c.drag((x + 3, y), (119, y)).await;
    c.wait("from the second half", |_| copied() == "日本語 🍣 cafe\u{301}").await;

    // Ctrl-] y: the last answer, as the stub gave it, once its turn is in.
    let log = || std::fs::read_to_string(dir.join("daemon.log")).unwrap_or_default();
    c.wait("four turns", |_| log().matches("\"event\":\"Stop\"").count() == 4).await;
    c.send(b"\x1dy").await;
    c.wait("the answer", |_| copied() == "echo: 日本語 🍣 cafe\u{301}").await;
    c.wait("its message", |s| side(s).contains("copied Reimu's last answer")).await;
    c.send(b"\x1dd").await;
    assert!(c.exit().await.success());
}

#[tokio::test(flavor = "current_thread")]
async fn a_selection_keeps_to_its_text_as_output_comes_and_past_the_top_it_scrolls_back() {
    let dir = state("drag");
    let _quit = Quit(dir.clone());
    let mut c = reimu(&dir).await;
    let mut c2 = Client::spawn(&dir, 120, 40);
    c2.wait("Reimu, twice", |s| s.contains("stub-claude Reimu")).await;
    let t = Instant::now();
    c2.wait("its nudge to pass", |_| t.elapsed() > Duration::from_millis(300)).await;
    let copied = || std::fs::read_to_string(dir.join("copied")).unwrap_or_default();
    let lines: Vec<String> = (0..60).map(|i| format!("line{i}")).collect();
    c.send(format!("\x1b[200~{}\x1b[201~\r", lines.join("\r")).as_bytes()).await;
    let s = c.wait("the echo", |s| s.contains("> line59")).await;

    // Held over three lines while the other client types: the rows move up, the selection
    // with them, on this client's screen only.
    let (x, y) = find(&s, "> line50");
    c.mouse(0, (x, y), true).await;
    c.mouse(32, (x + 7, y + 2), true).await;
    let held = |y: u16| [(y, "> line50"), (y + 1, "> line51"), (y + 2, "> line52")];
    c.picked(&held(y)).await;
    c2.send(b"more\r").await;
    c2.wait("the other client's line", |s| s.contains("> more")).await;
    c.picked(&held(y - 2)).await;
    c2.picked(&[]).await;
    c.mouse(0, (x + 7, y + 2), false).await;
    c.wait("the three lines", |_| copied() == "> line50\n> line51\n> line52").await;

    // From the end of the last line, up past the grid's top edge: back a row at a time until
    // the pointer comes back in, at the grid's first cell.
    let s = c.wait("the other line here", |s| s.contains("> more")).await;
    let (x, y) = find(&s, "> more");
    c.mouse(0, (x + 5, y), true).await;
    c.mouse(32, (27, 1), true).await;
    c.wait("scrolled back", |s| s.contains("↑ ") && !s.contains("↑ 1 ")).await;
    c.mouse(32, (27, 2), true).await;
    let mut last = String::new();
    let s = loop {
        let t = Instant::now();
        let s = c.wait("a moment", |_| t.elapsed() > Duration::from_millis(200)).await;
        if s == last {
            break s;
        }
        last = s;
    };
    let top = grid_row(&s, 2);
    let k: usize = top.strip_prefix("> line").and_then(|n| n.parse().ok()).expect(&top);
    c.mouse(0, (27, 2), false).await;
    let mut want: Vec<String> = (k..60).map(|i| format!("> line{i}")).collect();
    want.extend(["more".into(), "> more".into()]);
    let want = want.join("\n");
    c.wait("everything from there", |_| copied() == want).await;
    // Every client scrolled with it; only this one shows the selection.
    c2.wait("scrolled back there too", |s| grid_row(s, 2) == top).await;
    c2.picked(&[]).await;
    for mut c in [c, c2] {
        c.send(b"\x1dd").await;
        assert!(c.exit().await.success());
    }
}

/// Where `needle` first is in `hay`.
fn at(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[tokio::test(flavor = "current_thread")]
async fn a_link_the_resident_marks_is_a_link_on_the_host_and_the_title_names_her() {
    let dir = state("link");
    let _quit = Quit(dir.clone());
    let mut c = reimu(&dir).await;
    // The host's own title is kept first, then hers is set: her name, then the title she set.
    let push = at(&c.seen, b"\x1b[22;0t").expect("the title kept");
    assert!(push < at(&c.seen, b"\x1b]0;").expect("a title set"));
    assert_eq!(c.vt.modes().title, "Reimu · ✳ Reimu");

    c.send(b"/link https://example.com/x here\r").await;
    let t = Instant::now();
    let links = loop {
        let fr = c.vt.frame();
        let runs = fr.rows.iter().enumerate().flat_map(|(y, r)| r.iter().map(move |r| (y, r)));
        let links: Vec<_> =
            runs.filter_map(|(y, r)| Some((y, r.col, r.text.clone(), r.link.clone()?))).collect();
        if !links.is_empty() {
            break links;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "no link on the host");
        let t = Instant::now();
        c.wait("a frame", |_| t.elapsed() > Duration::from_millis(50)).await;
    };
    let s = c.vt.frame().text().join("\n");
    // Where the stub printed it: the grid's first column, the row after the line typed.
    let (_, y) = find(&s, "/link https://example.com/x here");
    assert_eq!(links, [(y as usize, 26, "here".into(), "https://example.com/x".into())], "{s}");
    assert_eq!(grid_row(&s, y + 1), "here");
    assert!(at(&c.seen, b"\x1b]8;;https://example.com/x\x1b\\").is_some());

    c.send(b"\x1dd").await;
    assert!(c.exit().await.success());
    // And given back as the client leaves.
    let pop = at(&c.seen, b"\x1b[23;0t").expect("the title given back");
    assert!(c.seen[pop..].windows(4).all(|w| w != b"\x1b]0;"));
}
