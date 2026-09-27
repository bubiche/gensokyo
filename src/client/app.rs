//! The client's event loop: the host terminal in raw mode on one side, the daemon's socket on the
//! other. A thread does the blocking host reads; the rest runs on a current-thread runtime.
//!
//! Frames are drawn at most every `FRAME`, each inside a synchronized update (2026) on a
//! buffered writer. The host is set to the focused resident's kitty flags, so keys usually go
//! through as they came.

use super::framer::{Chunk, Esc, Framer, Mouse, Reply as HostReply};
use super::keys::{self, Chord, Forward};
use super::render::{self, Button, Hit, HitMap, Modal, Model, Stage};
use crate::cli;
use crate::proto::{self, Envelope, Reply, Request, Resident};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::terminal;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Read, Stdout, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// At most one frame this often: about 60 fps while a resident streams output.
const FRAME: Duration = Duration::from_millis(16);

/// Alternate screen, cursor hidden, focus reports, bracketed paste, a kitty entry of our own.
const ENTER: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?1004h\x1b[?2004h\x1b[>0u\x1b[?u";
const LEAVE: &[u8] =
    b"\x1b[?2026l\x1b[<u\x1b[?2004l\x1b[?1004l\x1b[?1003l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?25h\x1b[?1049l";
/// Clicks and drags in SGR form. Not 1003 (every hover) unless the resident on screen asked.
const CAPTURE_ON: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h";
const CAPTURE_OFF: &[u8] = b"\x1b[?1003l\x1b[?1006l\x1b[?1002l\x1b[?1000l";

type Term = ratatui::Terminal<CrosstermBackend<BufWriter<Stdout>>>;

/// Our modes off and raw mode undone, however the client ends.
fn restore() {
    let mut out = std::io::stdout();
    let _ = out.write_all(LEAVE);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

pub fn main() -> ExitCode {
    // SAFETY: isatty only inspects the descriptors.
    if unsafe { libc::isatty(0) == 0 || libc::isatty(1) == 0 } {
        eprintln!("gensokyo: the client needs a terminal");
        return ExitCode::FAILURE;
    }
    let sock = match cli::connect_or_start() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("gensokyo: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = terminal::enable_raw_mode() {
        eprintln!("gensokyo: raw mode: {e}");
        return ExitCode::FAILURE;
    }
    let hook = std::panic::take_hook();
    // Out at once: unwinding would flush what was queued for the host (mouse and kitty modes)
    // after the restore.
    std::panic::set_hook(Box::new(move |p| {
        restore();
        hook(p);
        std::process::exit(101);
    }));
    let r = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt.block_on(run(sock)),
        Err(e) => Err(e.to_string()),
    };
    restore();
    match r {
        Ok(msg) => {
            println!("gensokyo: {msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("gensokyo: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Host reads as they come. The descriptor stays blocking: it is the terminal's, and shared.
fn reader() -> mpsc::UnboundedReceiver<Vec<u8>> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = stdin.read(&mut buf) {
            if tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    rx
}

async fn run(sock: std::os::unix::net::UnixStream) -> Result<String, String> {
    let err = |e: std::io::Error| e.to_string();
    sock.set_nonblocking(true).map_err(err)?;
    let (r, mut w) = tokio::net::UnixStream::from_std(sock).map_err(err)?.into_split();
    let mut lines = BufReader::new(r).lines();
    // A fixed viewport: the default one asks the host where the cursor is, reading the answer
    // off stdin behind the reader thread's back.
    let backend = CrosstermBackend::new(BufWriter::new(std::io::stdout()));
    let viewport = ratatui::Viewport::Fixed(host_area());
    let mut term =
        Term::with_options(backend, ratatui::TerminalOptions { viewport }).map_err(err)?;
    term.backend_mut().write_all(ENTER).map_err(err)?;
    term.backend_mut().write_all(CAPTURE_ON).map_err(err)?;
    term.resize(host_area()).map_err(err)?;
    use tokio::signal::unix::{SignalKind, signal};
    let (mut winch, mut hup, mut sigterm) = (
        signal(SignalKind::window_change()).map_err(err)?,
        signal(SignalKind::hangup()).map_err(err)?,
        signal(SignalKind::terminate()).map_err(err)?,
    );
    let mut app = App::new();
    app.send(Request::Hello { proto: proto::PROTO, who: "client".into() });
    app.send(Request::Watch);
    app.list();
    let mut input = reader();
    let mut framer = Framer::new();
    let start = Instant::now();
    let ms = || start.elapsed().as_secs_f64() * 1000.0;
    let mut drawn = Instant::now() - FRAME;
    let mut dirty = true;
    let end = loop {
        let mut wrote = Ok(());
        for l in app.out.drain(..) {
            wrote = w.write_all(&l).await;
            if wrote.is_err() {
                break;
            }
        }
        if wrote.is_err() {
            break app.gone.take().unwrap_or_else(|| "the daemon went away".into());
        }
        if let Some(why) = app.done.take() {
            break why;
        }
        if dirty && drawn.elapsed() >= FRAME {
            app.draw(&mut term).map_err(err)?;
            (drawn, dirty) = (Instant::now(), false);
        }
        let held = framer.deadline().map(|d| start + Duration::from_secs_f64(d / 1000.0));
        let next_draw = dirty.then_some(drawn + FRAME);
        tokio::select! {
            b = input.recv() => match b {
                Some(b) => {
                    for c in framer.feed(&b, ms()) {
                        app.chunk(c);
                    }
                }
                None => break "the terminal closed".into(),
            },
            _ = tokio::time::sleep_until(held.unwrap_or(start).into()), if held.is_some() => {
                for c in framer.tick(ms()) {
                    app.chunk(c);
                }
            }
            l = lines.next_line() => match l {
                Ok(Some(l)) => app.reply(&l),
                _ => break app.gone.take().unwrap_or_else(|| "the daemon went away".into()),
            },
            _ = tokio::time::sleep_until(next_draw.unwrap_or(drawn).into()), if next_draw.is_some() => {}
            _ = winch.recv() => term.resize(host_area()).map_err(err)?,
            _ = hup.recv() => break "the terminal hung up".into(),
            _ = sigterm.recv() => break "terminated".into(),
        }
        dirty = true;
    };
    Ok(end)
}

/// What the list replies were for.
enum Want {
    Shrine,
    All,
}

struct App {
    m: Model,
    hits: HitMap,
    /// The host's kitty flags as it last reported them; 0 until it answers, and forever on a
    /// terminal without kitty support.
    host_kitty: u8,
    /// What the host was last set to, and whether 1003 is on.
    set_kitty: u8,
    any_motion: bool,
    /// The focused resident's frame number, for damage.
    rev: u64,
    /// A left press in the grid, which may become a selection: the grid then, and the cell.
    drag: Option<(ratatui::layout::Rect, (u16, u16))>,
    /// The grid size last sent.
    size: (u16, u16),
    /// Everyone, departed included, from the last `list --all`.
    all: Vec<Resident>,
    lists: HashMap<u64, Want>,
    next: u64,
    /// Socket lines to write, and host bytes to write before the next frame.
    out: Vec<Vec<u8>>,
    host: Vec<u8>,
    /// Set to leave the loop, with what to print.
    done: Option<String>,
    /// Printed if the daemon hangs up next (after a quit).
    gone: Option<String>,
    /// The quit request, whose error means the daemon stays.
    quit: Option<u64>,
    log: Option<File>,
    t0: Instant,
    /// The config's `NOTIFY_BELL` and `NOTIFY_DESKTOP`: a bell and an OSC 9 notification to the
    /// host when a resident nobody watches comes to need the user.
    bell: bool,
    desktop: bool,
}

impl App {
    fn new() -> App {
        let home = std::env::var("HOME").unwrap_or_default();
        let exe = std::env::current_exe().unwrap_or_default();
        let banner = proto::share_dir(&exe)
            .and_then(|s| std::fs::read_to_string(s.join("banner.txt")).ok())
            .map(|b| b.lines().map(String::from).collect())
            .unwrap_or_default();
        let log = std::env::var_os("GENSOKYO_CLIENT_LOG")
            .and_then(|p| File::options().create(true).append(true).mode(0o600).open(p).ok());
        App {
            m: Model { capture: true, home, banner, now: now(), ..Model::default() },
            hits: HitMap::default(),
            host_kitty: 0,
            set_kitty: 0,
            any_motion: false,
            rev: 0,
            drag: None,
            size: (0, 0),
            all: Vec::new(),
            lists: HashMap::new(),
            next: 1,
            out: Vec::new(),
            host: Vec::new(),
            done: None,
            gone: None,
            quit: None,
            log,
            t0: Instant::now(),
            bell: proto::config("NOTIFY_BELL").as_deref() != Some("off"),
            desktop: proto::config("NOTIFY_DESKTOP").as_deref() != Some("off"),
        }
    }

    /// One line in `$GENSOKYO_CLIENT_LOG`, when it is set.
    fn trace(&mut self, what: std::fmt::Arguments) {
        if let Some(f) = self.log.as_mut() {
            let _ = writeln!(f, "{:>9.1} {what}", self.t0.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn send(&mut self, req: Request) -> u64 {
        let id = self.next;
        self.next += 1;
        let mut l = serde_json::to_vec(&Envelope { id, req }).unwrap_or_default();
        l.push(b'\n');
        self.out.push(l);
        id
    }

    fn list(&mut self) {
        let id = self.send(Request::List { all: false });
        self.lists.insert(id, Want::Shrine);
        let id = self.send(Request::List { all: true });
        self.lists.insert(id, Want::All);
    }

    fn focused(&self) -> Option<&Resident> {
        self.m.residents.iter().find(|r| Some(&r.id) == self.m.focused.as_ref())
    }

    /// The resident on screen, if it is running.
    fn live(&self) -> Option<String> {
        self.focused().filter(|r| r.departed.is_none()).map(|r| r.id.clone())
    }

    fn focus(&mut self, id: Option<String>) {
        if id == self.m.focused {
            return;
        }
        self.m.focused = id;
        (self.m.screen, self.m.selection) = (None, None);
        match self.live() {
            Some(who) => self.send(Request::View { who }),
            None => self.send(Request::Unview),
        };
        self.modes();
    }

    /// The host follows the resident on screen: its kitty flags, and 1003 only if it asked.
    fn modes(&mut self) {
        let live = self.live().is_some() && self.m.screen.is_some();
        let kitty = if live { self.m.modes.kitty } else { 0 };
        if kitty != self.set_kitty {
            self.set_kitty = kitty;
            self.host.extend(keys::kitty_set(kitty));
            self.host.extend(keys::KITTY_QUERY);
        }
        let any = live && self.m.capture && self.m.modes.mouse == 1003;
        if any != self.any_motion {
            self.any_motion = any;
            self.host.extend_from_slice(if any { b"\x1b[?1003h" } else { b"\x1b[?1003l" });
        }
    }

    fn draw(&mut self, term: &mut Term) -> std::io::Result<()> {
        self.m.now = now();
        let b = term.backend_mut();
        b.write_all(&std::mem::take(&mut self.host))?;
        b.write_all(b"\x1b[?2026h")?;
        let mut hits = HitMap::default();
        let m = &self.m;
        let frame = term.draw(|f| {
            hits = render::render(m, f.area(), f.buffer_mut());
            if let Some(c) = render::cursor(m, f.area()) {
                f.set_cursor_position(c);
            }
        })?;
        let g = render::grid_rect(frame.area);
        self.hits = hits;
        let b = term.backend_mut();
        b.write_all(b"\x1b[?2026l")?;
        b.flush()?;
        if (g.width, g.height) != self.size && g.width > 0 && g.height > 0 {
            self.size = (g.width, g.height);
            self.send(Request::Resize { cols: g.width, rows: g.height });
        }
        Ok(())
    }

    fn reply(&mut self, line: &str) {
        let Ok(r) = serde_json::from_str::<Reply>(line) else {
            self.trace(format_args!("bad line {line}"));
            return;
        };
        match r {
            Reply::Welcome { .. } => {}
            Reply::List { id, residents } => match self.lists.remove(&id) {
                Some(Want::Shrine) => self.residents(residents),
                _ => {
                    self.all = residents;
                    self.refresh_modal();
                }
            },
            Reply::Residents { residents } => {
                // Recall and the recent dirs read the departed too, when someone came or went.
                let who = |l: &[Resident]| -> Vec<(String, bool)> {
                    l.iter().map(|r| (r.id.clone(), r.departed.is_some())).collect()
                };
                let moved = who(&residents) != who(&self.m.residents);
                self.residents(residents);
                if moved {
                    let id = self.send(Request::List { all: true });
                    self.lists.insert(id, Want::All);
                }
            }
            Reply::Notify { state, text, watched, .. } => {
                if !watched {
                    let text = crate::tele::clean(&text, 200);
                    if self.bell {
                        self.host.push(0x07);
                    }
                    if self.desktop {
                        // iTerm2 posts this as its own notification; terminals without it ignore it.
                        self.host.extend(format!("\x1b]9;{text}\x07").bytes());
                    }
                    self.m.message = Some(format!("{} {text}", state.glyph()));
                }
            }
            Reply::Summoned { resident, .. } => {
                let was = self.live();
                match self.m.residents.iter_mut().find(|r| r.id == resident.id) {
                    Some(r) => *r = resident.clone(),
                    None => self.m.residents.push(resident.clone()),
                }
                match self.m.focused == Some(resident.id.clone()) {
                    true => self.back(was),
                    false => self.focus(Some(resident.id)),
                }
            }
            Reply::Done { message, .. } => self.m.message = Some(message),
            Reply::Error { id, error } => {
                if Some(id) == self.quit {
                    self.gone = None;
                }
                // The hello (the first request) turned away: why the daemon hangs up.
                if id == 1 {
                    self.gone = Some(error.clone());
                }
                self.m.message = Some(error);
            }
            Reply::Frame { who, rev, frame, modes } => {
                if Some(&who) == self.m.focused.as_ref() {
                    (self.m.screen, self.rev, self.m.modes) = (Some(frame), rev, modes);
                    self.modes();
                }
            }
            Reply::Damage { who, base, rev, rows, cursor, modes } => {
                // Until the first frame, damage belongs to a stream this client left.
                if Some(&who) != self.m.focused.as_ref() || self.m.screen.is_none() {
                    return;
                }
                match self.m.screen.as_mut().filter(|_| base == self.rev) {
                    Some(s) => {
                        for (y, runs) in rows {
                            if let Some(row) = s.rows.get_mut(y as usize) {
                                *row = runs;
                            }
                        }
                        (s.cursor, self.rev, self.m.modes) = (cursor, rev, modes);
                        self.modes();
                    }
                    // A frame went missing: start over from a whole one.
                    None => {
                        self.send(Request::View { who });
                    }
                }
            }
        }
    }

    fn residents(&mut self, list: Vec<Resident>) {
        let was = self.live();
        self.m.residents = list;
        if self.focused().is_none() {
            let first = self.m.residents.iter().find(|r| r.departed.is_none());
            let id = first.or(self.m.residents.first()).map(|r| r.id.clone());
            self.focus(id);
        } else if self.live().is_none() {
            // Departed while on screen: the departed screen, and the host back to plain keys.
            self.m.screen = None;
            self.modes();
        } else {
            self.back(was);
        }
    }

    /// The resident on screen was recalled under its id: its new screen.
    fn back(&mut self, was: Option<String>) {
        if let Some(who) = self.live().filter(|l| was.as_ref() != Some(l)) {
            self.m.screen = None;
            self.send(Request::View { who });
            self.modes();
        }
    }

    fn chunk(&mut self, c: Chunk) {
        self.trace(format_args!("in {c}"));
        match &c {
            Chunk::Reply { kind: HostReply::KittyFlags(n), .. } => {
                self.host_kitty = *n as u8;
                return;
            }
            Chunk::Reply { .. } | Chunk::Dropped(_) => return,
            Chunk::PasteRejected { len, why } => {
                self.m.message = Some(format!("paste of {len} bytes not sent: {why}"));
                return;
            }
            Chunk::Mouse { m, .. } => return self.mouse(*m),
            Chunk::Focus { gained, .. } => {
                // The daemon keeps quiet about a resident on screen in a focused terminal.
                self.send(Request::Focus { on: *gained });
                if self.m.modal.is_none() {
                    self.forward(&c);
                }
                return;
            }
            _ => self.m.selection = None,
        }
        if self.m.leader {
            if let Some(ch) = keys::chord(&c) {
                self.m.leader = false;
                self.chord(ch);
            }
        } else if keys::is_leader(&c) {
            self.m.leader = true;
        } else if self.m.modal.is_some() {
            self.modal_key(&c);
        } else if self.live().is_some() {
            self.forward(&c);
        } else if let Some(ch) = keys::chord(&c) {
            // Nothing on screen takes keys, so the letters work without the leader.
            self.chord(ch);
        }
    }

    fn forward(&mut self, c: &Chunk) {
        let Some(who) = self.live() else { return };
        let f = keys::forward(c, self.host_kitty, &self.m.modes);
        self.input(who, f);
    }

    fn input(&mut self, who: String, f: Forward) {
        let req = match f {
            Forward::Bytes(bytes) => Request::Input { who, bytes, key: None },
            Forward::Key(k) => Request::Input { who, bytes: Vec::new(), key: Some(k) },
            // Daemon scrollback is not there yet.
            Forward::Scroll(_) | Forward::Drop => return,
        };
        if let Request::Input { bytes, key, .. } = &req {
            self.trace(format_args!("out {} {key:?}", Esc(bytes)));
        }
        self.send(req);
    }

    fn mouse(&mut self, ev: Mouse) {
        if !self.m.capture {
            return;
        }
        let (col, row) = (ev.col.saturating_sub(1), ev.row.saturating_sub(1));
        // Left, with no motion or wheel bits (modifiers aside).
        let left = ev.button & 0b0110_0011 == 0;
        if left && ev.press {
            (self.drag, self.m.selection) = (None, None);
        }
        if let Some((g, anchor)) = self.drag {
            // The drag stays in the grid wherever the pointer goes.
            let head = (col.clamp(g.x, g.right() - 1) - g.x, row.clamp(g.y, g.bottom() - 1) - g.y);
            if ev.press && (head != anchor || self.m.selection.is_some()) {
                self.m.selection = Some((anchor, head));
            } else if !ev.press {
                self.drag = None;
                match self.m.selection {
                    Some((a, b)) if a != b => self.copy(a, b),
                    _ => self.m.selection = None,
                }
            }
            return;
        }
        match self.hits.at(col, row) {
            Some((r, Hit::Grid)) if left && ev.press && self.m.modes.mouse == 0 => {
                self.drag = Some((r, (col - r.x, row - r.y)));
            }
            Some((r, Hit::Grid)) => {
                if let Some(who) = self.live() {
                    let f = keys::mouse(&ev, col - r.x + 1, row - r.y + 1, &self.m.modes);
                    self.input(who, f);
                }
            }
            // A left press with no motion or wheel bits: chrome acts on the press alone.
            Some((_, h)) if ev.press && ev.button & 0b1110_0011 == 0 => self.click(h),
            _ => {}
        }
    }

    /// The selection's text to `$GENSOKYO_COPY` (a shell command; `pbcopy` by default).
    fn copy(&mut self, a: (u16, u16), b: (u16, u16)) {
        let Some(fr) = &self.m.screen else { return };
        let text = render::selected_text(fr, a, b);
        let cmd = std::env::var("GENSOKYO_COPY").unwrap_or_else(|_| "pbcopy".into());
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", &cmd])
            // pbcopy reads bytes in the locale's encoding, and a bare environment has none.
            .env("LC_CTYPE", "UTF-8")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        self.m.message = Some(match child {
            Ok(mut child) => {
                let n = text.chars().count();
                std::thread::spawn(move || {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(text.as_bytes());
                    }
                    let _ = child.wait();
                });
                format!("copied {n} characters")
            }
            Err(e) => format!("copy: {e}"),
        });
    }

    fn click(&mut self, h: Hit) {
        match h {
            Hit::Resident(i) => {
                let id = self.m.residents.get(i).map(|r| r.id.clone());
                if id.is_some() {
                    self.focus(id);
                }
            }
            Hit::Button(b) => match b {
                Button::Summon => self.chord(Chord::Summon),
                Button::Banish => self.chord(Chord::Banish),
                Button::Recall => self.chord(Chord::Recall),
                Button::Quit => self.chord(Chord::Quit),
                Button::Help => self.chord(Chord::Help),
                Button::Capture => self.chord(Chord::Capture),
                Button::RecallFocused => self.recall_focused(),
                Button::CloseFocused => self.chord(Chord::Close),
                Button::Yes => self.confirm(),
                Button::No => self.m.modal = None,
            },
            Hit::Item(i) => match &mut self.m.modal {
                Some(Modal::Summon(s)) if s.stage == Stage::Dir => {
                    if s.selected == Some(i) {
                        self.confirm();
                    } else {
                        s.selected = Some(i);
                    }
                }
                Some(Modal::Recall { selected, .. }) => {
                    if *selected == i {
                        self.confirm();
                    } else {
                        *selected = i;
                    }
                }
                _ => {}
            },
            Hit::Modal | Hit::Grid => {}
        }
    }

    fn recall_focused(&mut self) {
        if let Some(r) = self.focused().filter(|r| r.departed.is_some()) {
            let who = r.id.clone();
            self.send(Request::Recall { who });
        }
    }

    fn chord(&mut self, ch: Chord) {
        self.m.message = None;
        match ch {
            Chord::Summon => {
                self.m.modal = Some(Modal::Summon(render::Summon::default()));
                self.refresh_modal();
            }
            Chord::Banish => {
                if let Some(r) = self.focused().filter(|r| r.departed.is_none()) {
                    self.m.modal = Some(Modal::Banish { id: r.id.clone(), name: r.name.clone() });
                }
            }
            Chord::Recall => {
                self.m.modal = Some(Modal::Recall { list: Vec::new(), selected: 0 });
                self.refresh_modal();
            }
            Chord::Quit => self.m.modal = Some(Modal::Quit),
            Chord::Help => self.m.modal = Some(Modal::Help),
            Chord::Capture => {
                self.m.capture = !self.m.capture;
                self.host.extend_from_slice(if self.m.capture { CAPTURE_ON } else { CAPTURE_OFF });
                self.any_motion = false;
                self.modes();
            }
            Chord::Detach => self.done = Some("detached; the residents keep running".into()),
            Chord::Close => {
                if let Some(who) = self.m.focused.clone() {
                    self.send(Request::Close { who });
                }
            }
            Chord::Focus(n) => {
                let id = self.m.residents.iter().find(|r| r.slot == Some(n)).map(|r| r.id.clone());
                if id.is_some() {
                    self.focus(id);
                }
            }
            Chord::Leader => {
                if let Some(who) = self.live() {
                    self.input(who, Forward::Key(keys::LEADER));
                }
            }
            Chord::Cancel | Chord::Unbound => {}
        }
    }

    /// The departed for recall, newest first, and the recent directories for summon: the
    /// client's own first, then the shrine's, then the departed's, newest first.
    fn refresh_modal(&mut self) {
        let mut departed: Vec<Resident> =
            self.all.iter().filter(|r| r.departed.is_some()).cloned().collect();
        departed.sort_by_key(|r| std::cmp::Reverse(r.departed));
        match &mut self.m.modal {
            Some(Modal::Recall { list, selected }) => {
                *selected = (*selected).min(departed.len().saturating_sub(1));
                *list = departed;
            }
            Some(Modal::Summon(s)) => {
                let here = std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned());
                let live = self.all.iter().rev().filter(|r| r.departed.is_none());
                let mut recent = Vec::new();
                for d in here.into_iter().chain(live.chain(&departed).map(|r| r.cwd.clone())) {
                    let d = tilde(&d, &self.m.home);
                    if !recent.contains(&d) {
                        recent.push(d);
                    }
                }
                s.recent = recent;
                s.selected = s.selected.filter(|&i| i < s.recent.len());
            }
            _ => {}
        }
    }

    /// Enter, `y`, or the modal's confirm button.
    fn confirm(&mut self) {
        let Some(modal) = self.m.modal.take() else { return };
        match modal {
            Modal::Summon(mut s) if s.stage == Stage::Dir => {
                let chosen = s.selected.and_then(|i| s.recent.get(i).cloned());
                let dir = expand(chosen.as_deref().unwrap_or(&s.path), &self.m.home);
                if dir.is_dir() {
                    (s.stage, s.path, s.error) =
                        (Stage::Name, tilde(&dir.to_string_lossy(), &self.m.home), None);
                    s.completions.clear();
                } else {
                    s.error = Some(format!("not a directory: {}", dir.display()));
                }
                self.m.modal = Some(Modal::Summon(s));
            }
            Modal::Summon(s) => {
                let cwd = expand(&s.path, &self.m.home).to_string_lossy().into_owned();
                let name = Some(s.name.trim().to_string()).filter(|n| !n.is_empty());
                self.send(Request::Summon(proto::Summon { cwd, name, ..Default::default() }));
            }
            Modal::Banish { id, .. } => {
                self.send(Request::Banish { who: id });
            }
            Modal::Recall { list, selected } => {
                if let Some(r) = list.get(selected) {
                    self.send(Request::Recall { who: r.id.clone() });
                }
            }
            Modal::Quit => {
                self.send(Request::Quit);
                self.gone = Some("the shrine is empty; the daemon stopped".into());
            }
            Modal::Help => {}
        }
    }

    fn modal_key(&mut self, c: &Chunk) {
        let k = ModalKey::of(c);
        let Some(modal) = self.m.modal.as_mut() else { return };
        match (modal, k) {
            (_, ModalKey::Esc) => self.m.modal = None,
            (Modal::Help, _) => self.m.modal = None,
            (Modal::Quit | Modal::Banish { .. }, ModalKey::Enter | ModalKey::Text('y')) => {
                self.confirm()
            }
            (Modal::Quit | Modal::Banish { .. }, ModalKey::Text('n')) => self.m.modal = None,
            (Modal::Recall { list, selected }, k) => match k {
                ModalKey::Up => *selected = selected.saturating_sub(1),
                ModalKey::Down => *selected = (*selected + 1).min(list.len().saturating_sub(1)),
                ModalKey::Enter => self.confirm(),
                _ => {}
            },
            (Modal::Summon(s), k) => match (s.stage, k) {
                (_, ModalKey::Enter) => self.confirm(),
                (Stage::Dir, ModalKey::Up) => {
                    s.selected = s.selected.and_then(|i| i.checked_sub(1))
                }
                (Stage::Dir, ModalKey::Down) => {
                    let n = s.recent.len();
                    s.selected = match s.selected {
                        None if n > 0 => Some(0),
                        Some(i) if i + 1 < n => Some(i + 1),
                        other => other,
                    };
                }
                (Stage::Dir, ModalKey::Tab) => {
                    let home = self.m.home.clone();
                    complete(s, &home);
                }
                (Stage::Dir, ModalKey::Back) => {
                    s.path.pop();
                    (s.selected, s.error) = (None, None);
                }
                (Stage::Dir, ModalKey::Text(ch)) => {
                    s.path.push(ch);
                    (s.selected, s.error) = (None, None);
                }
                (Stage::Dir, ModalKey::Paste(t)) => {
                    s.path.push_str(t.trim_end_matches(['\r', '\n']));
                    s.selected = None;
                }
                (Stage::Name, ModalKey::Back) => {
                    s.name.pop();
                }
                (Stage::Name, ModalKey::Text(ch)) => s.name.push(ch),
                (Stage::Name, ModalKey::Paste(t)) => s.name.push_str(t.trim()),
                _ => {}
            },
            _ => {}
        }
    }
}

/// A host chunk as a modal reads it.
enum ModalKey {
    Text(char),
    Paste(String),
    Enter,
    Esc,
    Tab,
    Back,
    Up,
    Down,
    Other,
}

impl ModalKey {
    fn of(c: &Chunk) -> ModalKey {
        match c {
            // One character per read is typing; more is a burst, taken as a paste.
            Chunk::Text(t) if t.chars().count() == 1 => {
                ModalKey::Text(t.chars().next().unwrap_or(' '))
            }
            Chunk::Text(t) => ModalKey::Paste(t.clone()),
            Chunk::Paste(b) => ModalKey::Paste(String::from_utf8_lossy(b).into_owned()),
            Chunk::Key { key: Some(k), .. } if k.event != 3 => match (k.code, k.mods & 0x0f) {
                (13, _) => ModalKey::Enter,
                (27, _) => ModalKey::Esc,
                (9, 0) => ModalKey::Tab,
                (127 | 8, _) => ModalKey::Back,
                (code, 0 | 1) => char::from_u32(code).map_or(ModalKey::Other, ModalKey::Text),
                _ => ModalKey::Other,
            },
            Chunk::Key { raw, key: None } => match raw.as_slice() {
                b"\x1b[A" | b"\x1bOA" => ModalKey::Up,
                b"\x1b[B" | b"\x1bOB" => ModalKey::Down,
                _ => ModalKey::Other,
            },
            _ => ModalKey::Other,
        }
    }
}

/// The host's size, from the tty itself.
fn host_area() -> ratatui::layout::Rect {
    let (w, h) = terminal::size().unwrap_or((80, 24));
    ratatui::layout::Rect::new(0, 0, w, h)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `~` for the home directory, as paths are shown.
fn tilde(p: &str, home: &str) -> String {
    match p.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => p.to_string(),
    }
}

/// A typed path made absolute: `~` is home, and a relative one is under the client's cwd.
fn expand(p: &str, home: &str) -> PathBuf {
    let p = p.trim();
    let p = match p.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => p.to_string(),
    };
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    if p.is_empty() { here } else { here.join(p) }
}

/// Tab: the directories under the typed path's parent that start with its last part. One match
/// is filled in; several fill in what they share and are listed.
fn complete(s: &mut render::Summon, home: &str) {
    let (dir, prefix) = match s.path.rfind('/') {
        Some(i) => (&s.path[..=i], &s.path[i + 1..]),
        None => ("", s.path.as_str()),
    };
    let base = expand(if dir.is_empty() { "." } else { dir }, home);
    let mut names: Vec<String> = std::fs::read_dir(&base)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix) && (prefix.starts_with('.') || !n.starts_with('.')))
        .collect();
    names.sort();
    let shared = match names.as_slice() {
        [] => {
            s.completions.clear();
            return;
        }
        [one] => format!("{one}/"),
        [first, rest @ ..] => {
            let mut n = rest.iter().fold(first.len(), |n, r| {
                first.bytes().zip(r.bytes()).take(n).take_while(|(a, b)| a == b).count()
            });
            // `café` and `cafè` share a byte of the é.
            while !first.is_char_boundary(n) {
                n -= 1;
            }
            first[..n].to_string()
        }
    };
    s.path = format!("{dir}{shared}");
    s.completions = if names.len() > 1 { names } else { Vec::new() };
    s.selected = None;
}
