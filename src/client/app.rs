//! The client's event loop: the host terminal in raw mode on one side, the daemon's socket on the
//! other. A thread does the blocking host reads; the rest runs on a current-thread runtime.
//!
//! Frames are drawn at most every `FRAME`, each inside a synchronized update (2026) on a
//! buffered writer. The host is set to the focused resident's kitty flags, so keys usually go
//! through as they came.

use super::framer::{Chunk, Esc, Framer, Mouse, Reply as HostReply};
use super::keys::{self, Chord, Forward, Scrollback};
use super::modal::{self, Act, Cast, History, Modal, Recall, Summon, Timetable};
use super::render::{self, Button, Find, Hit, HitMap, Message, Model, Say};
use crate::cli;
use crate::paths;
use crate::proto::{self, Envelope, Reply, Request, Resident};
use crate::tele;
use crate::vt::{self, Pointer};
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::{Buffer, CellDiffOption, CellWidth};
use ratatui::crossterm::terminal;
use ratatui::layout::{Position, Rect};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Read, Stdout, Write};
use std::ops::Range;
use std::os::unix::fs::OpenOptionsExt;
use std::process::ExitCode;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// At most one frame this often: about 60 fps while a resident streams output.
const FRAME: Duration = Duration::from_millis(16);

/// Rows back that are surely past the top of any scrollback: the daemon stops at the top.
const TOP: i32 = -(1 << 30);

/// How long a message stays: long enough to read, and an error longer.
const INFO_LIFE: Duration = Duration::from_secs(5);
const ERROR_LIFE: Duration = Duration::from_secs(10);

/// How many messages the history keeps.
const HISTORY: usize = 50;

/// A key this soon after one typed into a resident is more of that typing, not a command: the
/// rest of a sentence meant for a resident that has just left must not act on the shrine (`a` moving on to another
/// resident, then the rest typed into its dialog).
const TYPING: Duration = Duration::from_secs(1);

/// A yes this soon after the key before is part of a burst, not an answer (`q` then Enter).
const ANSWER: Duration = Duration::from_millis(250);

/// A second press on the same cell this soon after the first is a double click.
const DOUBLE: Duration = Duration::from_millis(500);

/// Held past the grid's top or bottom edge, a drag moves the view a row this often.
pub const EDGE: Duration = Duration::from_millis(30);

/// The host's title kept, alternate screen, cursor hidden, focus reports, bracketed paste, a kitty
/// entry of our own.
const ENTER: &[u8] = b"\x1b[22;0t\x1b[?1049h\x1b[?25l\x1b[?1004h\x1b[?2004h\x1b[>0u\x1b[?u";
const LEAVE: &[u8] =
    b"\x1b[?2026l\x1b[<u\x1b[?2004l\x1b[?1004l\x1b[?1003l\x1b[?1006l\x1b[?1002l\x1b[?1000l\x1b[?25h\x1b[?1049l\x1b[23;0t";
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
    let pid = cli::peer_pid(&sock);
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
    let mut app = App::new(Config::from_env());
    app.send(proto::hello("client"));
    app.send(Request::Watch);
    app.list();
    let mut input = reader();
    let mut framer = Framer::new();
    let start = Instant::now();
    let ms = || start.elapsed().as_secs_f64() * 1000.0;
    let mut drawn = Instant::now() - FRAME;
    let mut dirty = true;
    loop {
        let mut wrote = Ok(());
        for l in app.out.drain(..) {
            wrote = w.write_all(&l).await;
            if wrote.is_err() {
                break;
            }
        }
        if wrote.is_err() {
            break app.ended(pid);
        }
        if let Some(why) = app.done.take() {
            break Ok(why);
        }
        if dirty && drawn.elapsed() >= FRAME {
            app.draw(&mut term).map_err(err)?;
            (drawn, dirty) = (Instant::now(), false);
        }
        let held = framer.deadline().map(|d| start + Duration::from_secs_f64(d / 1000.0));
        let next_draw = dirty.then_some(drawn + FRAME);
        let expires = app.expires();
        let edge = app.edge();
        // Countdowns and times go on while nothing happens: a frame on each minute.
        let minute = Instant::now() + Duration::from_secs(60 - now().rem_euclid(60) as u64);
        tokio::select! {
            b = input.recv() => match b {
                Some(b) => {
                    for c in framer.feed(&b, ms()) {
                        app.chunk(c);
                    }
                }
                None => break Ok("the terminal closed".into()),
            },
            _ = tokio::time::sleep_until(held.unwrap_or(start).into()), if held.is_some() => {
                for c in framer.tick(ms()) {
                    app.chunk(c);
                }
            }
            l = lines.next_line() => match l {
                Ok(Some(l)) => app.reply(&l),
                _ => break app.ended(pid),
            },
            _ = tokio::time::sleep_until(next_draw.unwrap_or(drawn).into()), if next_draw.is_some() => {}
            _ = tokio::time::sleep_until(expires.unwrap_or(start).into()), if expires.is_some() => {
                app.expire(Instant::now());
            }
            _ = tokio::time::sleep_until(edge.unwrap_or(start).into()), if edge.is_some() => app.tick(),
            _ = tokio::time::sleep_until(minute.into()) => {}
            _ = winch.recv() => term.resize(host_area()).map_err(err)?,
            _ = hup.recv() => break Ok("the terminal hung up".into()),
            _ = sigterm.recv() => break Ok("terminated".into()),
        }
        dirty = true;
    }
}

/// What the list replies were for.
enum Want {
    Shrine,
    All,
}

/// What the client takes from its surroundings as it starts, given as values so a test can
/// build an App without a terminal.
pub struct Config {
    /// Shown as `~` in paths.
    pub home: String,
    /// Drawn when the shrine is empty.
    pub banner: Vec<String>,
    /// The config's `NOTIFY_BELL` and `NOTIFY_DESKTOP`.
    pub bell: bool,
    pub desktop: bool,
    /// `$GENSOKYO_CLIENT_LOG`, opened.
    pub log: Option<File>,
    /// `$GENSOKYO_COPY`, a shell command the selection is piped to; `pbcopy` by default.
    pub copy: String,
    /// The config's `BRANCH_PREFIX`, left off branches in the sidebar.
    pub prefix: String,
}

impl Config {
    pub fn from_env() -> Config {
        let exe = std::env::current_exe().unwrap_or_default();
        let banner = paths::share_dir(&exe)
            .and_then(|s| std::fs::read_to_string(s.join("banner.txt")).ok())
            .map(|b| b.lines().map(String::from).collect())
            .unwrap_or_default();
        Config {
            home: paths::home(),
            banner,
            bell: paths::config("NOTIFY_BELL").as_deref() != Some("off"),
            desktop: paths::config("NOTIFY_DESKTOP").as_deref() != Some("off"),
            log: std::env::var_os("GENSOKYO_CLIENT_LOG")
                .and_then(|p| File::options().create(true).append(true).mode(0o600).open(p).ok()),
            copy: std::env::var("GENSOKYO_COPY").unwrap_or_else(|_| "pbcopy".into()),
            prefix: paths::config("BRANCH_PREFIX").unwrap_or_default(),
        }
    }
}

pub struct App {
    pub m: Model,
    hits: HitMap,
    /// The host's kitty flags as it last reported them; 0 until it answers, and forever on a
    /// terminal without kitty support.
    host_kitty: u8,
    /// What the host was last set to, and whether 1003 is on.
    set_kitty: u8,
    any_motion: bool,
    /// The focused resident's frame number, for damage.
    rev: u64,
    /// A left press in the grid, held.
    held: Option<Held>,
    /// When a drag held past the grid's edge next moves the view.
    edge: Option<Instant>,
    /// The last left press in the grid that was not a double click's second: when, and where.
    pressed: Option<(Instant, (u16, u16))>,
    /// The requests whose `done` carries text to copy, and what each text is.
    copying: Vec<(u64, String)>,
    /// The grid size last sent.
    pub(super) size: (u16, u16),
    /// Everyone, departed included, from the last `list --all`.
    pub(super) all: Vec<Resident>,
    lists: HashMap<u64, Want>,
    next: u64,
    /// Socket lines to write, and host bytes to write before the next frame.
    pub out: Vec<Vec<u8>>,
    pub host: Vec<u8>,
    /// Set to leave the loop, with what to print.
    done: Option<String>,
    /// Printed if the daemon hangs up next (after a quit).
    pub(super) gone: Option<String>,
    /// The daemon's refusal of our hello: another build's.
    refused: Option<String>,
    /// The quit request, whose error means the daemon stays.
    pub(super) quit: Option<u64>,
    /// The summon the summon modal waits on: its reply closes the modal, its error shows there.
    pub(super) summoning: Option<u64>,
    /// When the message goes.
    said: Option<Instant>,
    /// When the last key came, and the one before it.
    typed: Option<Instant>,
    pub(super) before: Option<Instant>,
    /// When a key last went to a resident, or was dropped as the rest of such typing.
    talked: Option<Instant>,
    /// Sent back to the live screen, and not yet told it is there: a screen the daemon sent
    /// before it saw that is still scrolled back, and must not take the next key for itself.
    homing: bool,
    /// Added to the clock: tests move time on with `later`.
    skew: Duration,
    log: Option<File>,
    t0: Instant,
    /// The config's `NOTIFY_BELL` and `NOTIFY_DESKTOP`: a bell and an OSC 9 notification to the
    /// host when a resident nobody watches comes to need the user.
    bell: bool,
    desktop: bool,
    /// The shell command a selection is piped to.
    copy: String,
    /// The host cells drawn as links in the last frame: row, columns, and where to.
    linked: Vec<(u16, Range<u16>, String)>,
    /// The host's title as last set.
    titled: Option<String>,
}

impl App {
    /// The daemon hung up: after a quit that is the end, otherwise it is a failure to say.
    fn ended(&mut self, pid: Option<i32>) -> Result<String, String> {
        if let Some(e) = self.refused.take() {
            return Err(cli::refusal(&e, pid).map_or(e, |r| r.to_string()));
        }
        self.gone.take().ok_or_else(|| "the daemon went away".into())
    }

    pub fn new(c: Config) -> App {
        App {
            m: Model {
                capture: true,
                home: c.home,
                banner: c.banner,
                prefix: c.prefix,
                now: now(),
                ..Model::default()
            },
            hits: HitMap::default(),
            host_kitty: 0,
            set_kitty: 0,
            any_motion: false,
            rev: 0,
            held: None,
            edge: None,
            pressed: None,
            copying: Vec::new(),
            size: (0, 0),
            all: Vec::new(),
            lists: HashMap::new(),
            next: 1,
            out: Vec::new(),
            host: Vec::new(),
            done: None,
            gone: None,
            refused: None,
            quit: None,
            summoning: None,
            said: None,
            typed: None,
            before: None,
            talked: None,
            homing: false,
            skew: Duration::ZERO,
            log: c.log,
            t0: Instant::now(),
            bell: c.bell,
            desktop: c.desktop,
            copy: c.copy,
            linked: Vec::new(),
            titled: None,
        }
    }

    /// The clock, as `later` has moved it.
    pub(super) fn clock(&self) -> Instant {
        Instant::now() + self.skew
    }

    /// Time moves on by `d`, for tests of what depends on the pace of typing.
    pub fn later(&mut self, d: Duration) {
        self.skew += d;
    }

    /// Whether the key being handled came within `d` of the one before it.
    pub(super) fn burst(&self, d: Duration) -> bool {
        self.before.zip(self.typed).is_some_and(|(b, t)| t.duration_since(b) < d)
    }

    /// A yes to a confirm, unless it came in a burst of typing.
    pub(super) fn answer(&self) -> bool {
        !self.burst(ANSWER)
    }

    /// A message for the user, in place of the last one, and kept in the history.
    pub(super) fn say(&mut self, kind: Say, text: impl Into<String>) {
        let msg = Message::new(kind, text);
        self.m.history.insert(0, (now(), msg.clone()));
        self.m.history.truncate(HISTORY);
        let life = if kind == Say::Error { ERROR_LIFE } else { INFO_LIFE };
        self.show(msg, life);
    }

    fn show(&mut self, msg: Message, life: Duration) {
        self.m.message = Some(msg);
        self.said = Some(Instant::now() + life);
    }

    /// The daemon's notices from before this client came, into the history in their places.
    /// One nobody has heard yet is said once; more, how many.
    fn notices(&mut self, notices: Vec<proto::Notice>) {
        let missed: Vec<&proto::Notice> = notices.iter().filter(|n| n.missed).collect();
        let text = match missed[..] {
            [] => None,
            [one] => Some(tele::clean(&one.text, proto::NOTICE_MOST)),
            _ => Some(format!("{} notices while you were away: ^] h", missed.len())),
        };
        for n in notices {
            let text = tele::clean(&n.text, proto::NOTICE_MOST);
            self.m.history.push((n.at, Message::new(Say::Notice, text)));
        }
        self.m.history.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        self.m.history.truncate(HISTORY);
        if let Some(text) = text {
            self.show(Message::new(Say::Notice, text), ERROR_LIFE);
        }
    }

    /// When the message on show runs out.
    pub fn expires(&self) -> Option<Instant> {
        self.said.filter(|_| self.m.message.is_some())
    }

    /// The message gone once its time is up.
    pub fn expire(&mut self, now: Instant) {
        if self.said.is_some_and(|t| t <= now) {
            (self.m.message, self.said) = (None, None);
        }
    }

    /// One line in `$GENSOKYO_CLIENT_LOG`, when it is set.
    fn trace(&mut self, what: std::fmt::Arguments) {
        if let Some(f) = self.log.as_mut() {
            let _ = writeln!(f, "{:>9.1} {what}", self.t0.elapsed().as_secs_f64() * 1000.0);
        }
    }

    pub(super) fn send(&mut self, req: Request) -> u64 {
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
        (self.m.focused, self.homing) = (id, false);
        (self.m.screen, self.m.find) = (None, None);
        (self.held, self.edge, self.pressed) = (None, None, None);
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
        if self.m.rituals.as_ref().is_some_and(|r| !r.is_empty()) {
            self.m.today = jiff::Zoned::now().date().to_string();
        }
        self.retitle();
        let b = term.backend_mut();
        b.write_all(&std::mem::take(&mut self.host))?;
        b.write_all(b"\x1b[?2026h")?;
        let mut links = Vec::new();
        term.draw(|f| {
            self.paint(f.area(), f.buffer_mut());
            links = self.links(f.area(), f.buffer_mut());
            if let Some(c) = render::cursor(&self.m, f.area()) {
                f.set_cursor_position(c);
            }
        })?;
        let b = term.backend_mut();
        b.write_all(&links)?;
        b.write_all(b"\x1b[?2026l")?;
        b.flush()?;
        Ok(())
    }

    /// The model drawn into `buf`: what each cell does when clicked is kept for the next
    /// clicks, and a new grid size goes to the daemon.
    pub fn paint(&mut self, area: Rect, buf: &mut Buffer) {
        self.hits = render::render(&self.m, area, buf);
        let g = render::grid_rect(area);
        if (g.width, g.height) != self.size && g.width > 0 && g.height > 0 {
            self.size = (g.width, g.height);
            self.send(Request::Resize { cols: g.width, rows: g.height });
        }
    }

    /// The links in the resident's grid, to write once ratatui has drawn `buf`: each run the
    /// child marked a link to somewhere `vt::link` lets through is printed again inside OSC 8,
    /// as `buf` holds it, where nothing is drawn over the grid. The host keeps a link on a cell
    /// until the cell is written, so one that held a link last time and holds none now is
    /// printed again bare. A cursor that shows goes back to where ratatui put it.
    pub fn links(&mut self, area: Rect, buf: &Buffer) -> Vec<u8> {
        let g = render::grid_rect(area);
        let open = |x, y| matches!(self.hits.at(x, y), Some((_, Hit::Grid)));
        let mut spans = Vec::new();
        let rows = self.m.screen.iter().flat_map(|fr| fr.rows.iter().take(g.height as usize));
        for (y, row) in (g.y..).zip(rows) {
            for run in row {
                let Some(uri) = run.link.as_deref().and_then(vt::link) else { continue };
                let x0 = g.x.saturating_add(run.col);
                let x1 = x0.saturating_add(render::width(&run.text)).min(g.right());
                let mut x = x0;
                while x < x1 {
                    let from = x;
                    while x < x1 && open(x, y) {
                        x += 1;
                    }
                    if x > from {
                        spans.push((y, from..x, uri.to_string()));
                    }
                    x += 1;
                }
            }
        }
        let mut out = Vec::new();
        for (y, xs, _) in self.linked.iter().filter(|s| !spans.contains(s)) {
            reprint(&mut out, buf, *y, xs.clone());
        }
        for (y, xs, uri) in &spans {
            out.extend(format!("\x1b]8;;{uri}\x1b\\").bytes());
            reprint(&mut out, buf, *y, xs.clone());
            out.extend(b"\x1b]8;;\x1b\\");
        }
        self.linked = spans;
        if let Some((x, y)) = render::cursor(&self.m, area).filter(|_| !out.is_empty()) {
            out.extend(format!("\x1b[{};{}H", y + 1, x + 1).bytes());
        }
        out
    }

    /// The host's title follows the resident on screen: its name, then the title it set itself
    /// while its screen shows. Set only when it changes.
    pub fn retitle(&mut self) {
        let own = match self.live().is_some() && self.m.screen.is_some() {
            true => tele::clean(&self.m.modes.title, vt::TITLE_MOST),
            false => String::new(),
        };
        let title = match self.focused() {
            Some(r) if own.is_empty() => r.name.clone(),
            Some(r) => format!("{} · {own}", r.name),
            None => "gensokyo".into(),
        };
        let title = tele::clean(&title, 2 * vt::TITLE_MOST);
        if self.titled.as_ref() != Some(&title) {
            self.host.extend(format!("\x1b]0;{title}\x1b\\").bytes());
            self.titled = Some(title);
        }
    }

    /// One line from the daemon.
    pub fn reply(&mut self, line: &str) {
        let Ok(r) = serde_json::from_str::<Reply>(line) else {
            self.trace(format_args!("bad line {line}"));
            return;
        };
        match r {
            Reply::Welcome { .. } | Reply::Waited { .. } => {}
            Reply::Rituals { rituals, .. } => self.rituals(rituals),
            Reply::Notice { text } => {
                let text = tele::clean(&text, proto::NOTICE_MOST);
                if self.desktop {
                    let short = tele::clean(&text, 200);
                    self.host.extend(format!("\x1b]9;{short}\x07").bytes());
                }
                self.say(Say::Notice, text);
            }
            Reply::Notices { notices } => self.notices(notices),
            Reply::Cards { cards, unusable, .. } => {
                if let Some(Modal::Cast(c)) = &mut self.m.modal {
                    (c.cards, c.unusable) = (Some(cards), unusable);
                }
            }
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
                    self.say(Say::Notice, format!("{} {text}", state.glyph()));
                }
            }
            Reply::Summoned { id, resident, note } => {
                // How its worktree came to be: reused, or from a base that could not be fetched.
                if let Some(n) = note {
                    self.say(Say::Notice, n);
                }
                // Only the modal waiting on it: one opened since is another summon.
                if self.summoning.take_if(|s| *s == id).is_some()
                    && matches!(&self.m.modal, Some(Modal::Summon(s)) if s.waiting)
                {
                    self.m.modal = None;
                }
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
            Reply::Done { id, message } => match self.copying.iter().position(|c| c.0 == id) {
                Some(i) => {
                    let (_, what) = self.copying.remove(i);
                    if !message.is_empty() {
                        self.copy(message, &what);
                    }
                }
                None => self.say(Say::Info, message),
            },
            Reply::Error { id, error } => {
                self.copying.retain(|c| c.0 != id);
                if Some(id) == self.quit {
                    self.gone = None;
                }
                // The hello (the first request) turned away: why the daemon hangs up.
                if id == 1 {
                    self.refused = Some(error.clone());
                }
                // A summon's error belongs in the modal still waiting on it.
                if self.summoning.take_if(|s| *s == id).is_some()
                    && let Some(Modal::Summon(s)) = &mut self.m.modal
                    && s.waiting
                {
                    (s.waiting, s.error) = (false, Some(error));
                    return;
                }
                self.say(Say::Error, error);
            }
            Reply::Frame { who, rev, frame, modes } => {
                if Some(&who) == self.m.focused.as_ref() {
                    let mut frame = frame;
                    frame.back = self.landed(frame.back);
                    (self.m.screen, self.rev, self.m.modes) = (Some(frame), rev, modes);
                    self.modes();
                }
            }
            Reply::Damage { who, base, rev, rows, cursor, modes, back, history } => {
                // Until the first frame, damage belongs to a stream this client left.
                if Some(&who) != self.m.focused.as_ref() || self.m.screen.is_none() {
                    return;
                }
                let back = self.landed(back);
                match self.m.screen.as_mut().filter(|_| base == self.rev) {
                    Some(s) => {
                        for (y, runs) in rows {
                            if let Some(row) = s.rows.get_mut(y as usize) {
                                *row = runs;
                            }
                        }
                        (s.cursor, s.back, s.history) = (cursor, back, history);
                        (self.rev, self.m.modes) = (rev, modes);
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
            (self.m.screen, self.m.find) = (None, None);
            (self.held, self.edge) = (None, None);
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

    /// One chunk of host input.
    pub fn chunk(&mut self, c: Chunk) {
        self.trace(format_args!("in {c}"));
        match &c {
            Chunk::Reply { kind: HostReply::KittyFlags(n), .. } => {
                self.host_kitty = *n as u8;
                return;
            }
            Chunk::Reply { .. } | Chunk::Dropped(_) => return,
            Chunk::PasteRejected { len, why } => {
                self.say(Say::Error, format!("paste of {len} bytes not sent: {why}"));
                return;
            }
            Chunk::Mouse { m, .. } => return self.mouse(*m),
            Chunk::Focus { gained, .. } => {
                // The daemon keeps quiet about a resident on screen in a focused terminal.
                self.send(Request::Focus { on: *gained });
                // A release away from the terminal is never seen.
                if !gained {
                    (self.held, self.edge) = (None, None);
                }
                if self.m.modal.is_none() {
                    self.forward(&c);
                }
                return;
            }
            _ => {
                let now = self.clock();
                self.before = self.typed.replace(now);
            }
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
        } else if self.m.find.as_ref().is_some_and(|f| f.typing.is_some()) {
            self.find_key(&c);
        } else if self.live().is_some() {
            // A search keeps the keys even when what it found is on the live screen.
            if self.scrolled() || self.m.find.is_some() {
                // Keys of ours that came in one read are taken one by one, as typed.
                if let Chunk::Text(t) = &c
                    && t.chars().nth(1).is_some()
                    && self.ours(t)
                {
                    for ch in t.chars() {
                        self.chunk(Chunk::Text(ch.into()));
                    }
                    return;
                }
                match keys::scrollback(&c) {
                    Some(Scrollback::Find(back)) => return self.find(back),
                    Some(Scrollback::Again(other)) if self.m.find.is_some() => {
                        return self.search(other);
                    }
                    Some(Scrollback::Again(_)) | None => self.scroll(Scrollback::Live),
                    Some(s) => return self.scroll(s),
                }
            }
            self.talked = Some(self.clock());
            self.forward(&c);
        } else if let Some(ch) = keys::chord(&c) {
            // Nothing on screen takes keys, so the letters work without the leader, but only
            // after a pause: typing that outlived its resident goes nowhere.
            let now = self.clock();
            if self.talked.is_some_and(|t| now.duration_since(t) < TYPING) {
                self.talked = Some(now);
                if let Some(r) = self.focused() {
                    let say = format!("{} has left; what was typed went nowhere", r.name);
                    self.say(Say::Info, say);
                }
                return;
            }
            // On a departed screen `r` is its own `[recall r]`, and `Ctrl-] r` the whole list.
            match ch {
                Chord::Recall if self.focused().is_some() => self.recall_focused(),
                ch => self.chord(ch),
            }
        }
    }

    fn forward(&mut self, c: &Chunk) {
        let Some(who) = self.live() else { return };
        let f = keys::forward(c, self.host_kitty, &self.m.modes);
        self.input(who, f);
    }

    /// How far back a screen from the daemon is, as far as keys go: a screen sent before it
    /// saw the way home counts as home.
    fn landed(&mut self, back: u32) -> u32 {
        if back == 0 {
            self.homing = false;
        }
        if self.homing { 0 } else { back }
    }

    /// The resident on screen is showing its scrollback, not its live screen.
    fn scrolled(&self) -> bool {
        self.m.screen.as_ref().is_some_and(|s| s.back > 0)
    }

    /// The scrollback moved. The live screen is taken as reached at once, so the next key
    /// goes to the resident even before the daemon's damage says so.
    fn scroll(&mut self, s: Scrollback) {
        let Some(who) = self.live() else { return };
        let page = i32::from(self.size.1.max(2) - 1);
        let rows = match s {
            Scrollback::By(n) => Some(n),
            Scrollback::Pages(n) => Some(n * page),
            Scrollback::Top => Some(TOP),
            Scrollback::Live => None,
            Scrollback::Stay | Scrollback::Find(_) | Scrollback::Again(_) => return,
        };
        self.homing = rows.is_none();
        if self.homing {
            self.m.find = None;
        }
        if self.homing
            && let Some(fr) = &mut self.m.screen
        {
            fr.back = 0;
        }
        self.send(Request::Scroll { who, rows });
    }

    /// Whether text that came in one read is keys the scrollback or a search takes, and not
    /// typing for the resident: all of it, up to a `/` or `?` whose prompt takes the rest, and
    /// up to a key that goes home only as the last.
    fn ours(&self, t: &str) -> bool {
        let n = t.chars().count();
        for (i, ch) in t.chars().enumerate() {
            match keys::scrollback(&Chunk::Text(ch.into())) {
                Some(Scrollback::Find(_)) => return true,
                Some(Scrollback::Again(_)) if self.m.find.is_some() => {}
                Some(Scrollback::Live) if i + 1 == n => {}
                Some(Scrollback::By(_) | Scrollback::Pages(_) | Scrollback::Top) => {}
                _ => return false,
            }
        }
        true
    }

    /// The search prompt opens on the box's edge, `back` toward older output; what was last
    /// looked for stays, for an Enter with nothing typed.
    fn find(&mut self, back: bool) {
        let needle = self.m.find.take().map(|f| f.needle).unwrap_or_default();
        self.m.find = Some(Find { needle, back, typing: Some(String::new()) });
    }

    /// A key at the search prompt: Enter looks, Esc (or Backspace with nothing typed) leaves
    /// it, and with nothing looked for yet, the search.
    fn find_key(&mut self, c: &Chunk) {
        let Some(f) = self.m.find.as_mut() else { return };
        let Some(t) = f.typing.as_mut() else { return };
        let done = match modal::Key::of(c) {
            modal::Key::Text(ch) => {
                t.push(ch);
                false
            }
            modal::Key::Paste(p) => {
                t.extend(p.chars().map(|ch| if ch.is_control() { ' ' } else { ch }));
                false
            }
            modal::Key::Back => t.pop().is_none(),
            modal::Key::Esc => true,
            modal::Key::Enter => {
                let t = std::mem::take(t);
                if !t.is_empty() {
                    f.needle = t;
                }
                f.typing = None;
                if !f.needle.is_empty() {
                    return self.search(false);
                }
                true
            }
            _ => false,
        };
        if done {
            f.typing = None;
            if f.needle.is_empty() {
                self.m.find = None;
            }
        }
    }

    /// Looks for what the search is on, its own way or with `other` the other.
    fn search(&mut self, other: bool) {
        let (Some(who), Some(f)) = (self.live(), &self.m.find) else { return };
        let req = Request::Search { who, needle: f.needle.clone(), back: f.back != other };
        self.homing = false;
        self.send(req);
    }

    fn input(&mut self, who: String, f: Forward) {
        let req = match f {
            Forward::Bytes(bytes) => Request::Input { who, bytes, key: None },
            Forward::Key(k) => Request::Input { who, bytes: Vec::new(), key: Some(k) },
            Forward::Scroll(rows) => {
                self.homing = false;
                Request::Scroll { who, rows: Some(rows.into()) }
            }
            Forward::Drop => return,
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
        let wheel = ev.button & 64 != 0;
        // A press while one is held: its release was lost (outside the window, say).
        if left && ev.press {
            (self.held, self.edge) = (None, None);
        }
        if let Some(h) = self.held.as_mut().filter(|_| !wheel) {
            // Another button's release.
            if !ev.press && !left {
                return;
            }
            // The drag stays in the grid wherever the pointer goes; past its top or bottom
            // edge, the view moves a row every `EDGE` while it is held there.
            let g = h.grid;
            h.at = (col.clamp(g.x, g.right() - 1) - g.x, row.clamp(g.y, g.bottom() - 1) - g.y);
            let past = match row {
                r if r < g.y => Some(true),
                r if r >= g.bottom() => Some(false),
                _ => None,
            };
            let (at, moved) = (h.at, std::mem::replace(&mut h.sent, h.at) != h.at);
            h.past = past;
            if !ev.press {
                (self.held, self.edge) = (None, None);
                // The motion to it may not have been reported.
                if moved {
                    self.select(Pointer::Drag, at);
                }
                if let Some(id) = self.select(Pointer::Release, at) {
                    self.copying.push((id, String::new()));
                }
                return;
            }
            self.edge = match past {
                Some(_) => self.edge.or(Some(self.clock() + EDGE)),
                None => None,
            };
            if moved {
                // A press here soon after is a new selection, not a double click.
                self.pressed = None;
                self.select(Pointer::Drag, at);
            }
            return;
        }
        // On the other screen the wheel goes in as arrow keys, and typing drops the selection.
        if wheel && self.held.is_some() && self.m.modes.alt {
            return;
        }
        match self.hits.at(col, row) {
            Some((g, Hit::Grid)) if left && ev.press && self.m.modes.mouse == 0 => {
                let (at, now) = ((col - g.x, row - g.y), self.clock());
                let double = self.pressed.is_some_and(|(t, c)| c == at && now - t < DOUBLE);
                // A third press is a single click again.
                self.pressed = (!double).then_some((now, at));
                self.select(if double { Pointer::Double } else { Pointer::Press }, at);
                self.held = Some(Held { grid: g, at, sent: at, past: None });
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
        // The wheel while a drag is held: the selection follows the pointer onto the rows it
        // brought.
        if let Some(at) = self.held.as_ref().filter(|_| wheel).map(|h| h.at) {
            self.select(Pointer::Drag, at);
        }
    }

    /// What the pointer did in the resident's grid, for the daemon to select by.
    fn select(&mut self, how: Pointer, (x, y): (u16, u16)) -> Option<u64> {
        let who = self.live()?;
        Some(self.send(Request::Select { who, how, x, y }))
    }

    /// When a drag held past the grid's edge next moves the view, on the clock.
    pub fn edge(&self) -> Option<Instant> {
        self.edge
    }

    /// The view a row further, back past the top edge or on past the bottom, once it is time.
    pub fn tick(&mut self) {
        // No release reaches a drag once capture is off or a modal is up: it is over.
        if !self.m.capture || self.m.modal.is_some() {
            (self.held, self.edge) = (None, None);
            return;
        }
        let Some(h) = self.held.as_ref().filter(|_| self.edge.is_some_and(|t| t <= self.clock()))
        else {
            return;
        };
        let (how, at) = (if h.past == Some(true) { Pointer::Back } else { Pointer::On }, h.at);
        self.edge = Some(self.clock() + EDGE);
        self.select(how, at);
    }

    /// `text` to `$GENSOKYO_COPY` (a shell command; `pbcopy` by default): a selection, or
    /// `what` it is.
    fn copy(&mut self, text: String, what: &str) {
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", &self.copy])
            // pbcopy reads bytes in the locale's encoding, and a bare environment has none.
            .env("LC_CTYPE", "UTF-8")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match child {
            Ok(mut child) => {
                let n = text.chars().count();
                std::thread::spawn(move || {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(text.as_bytes());
                    }
                    let _ = child.wait();
                });
                let say = match what {
                    "" => format!("copied {n} characters"),
                    w => format!("copied {w}, {n} characters"),
                };
                self.say(Say::Info, say);
            }
            Err(e) => self.say(Say::Error, format!("copy: {e}")),
        }
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
                Button::Cast => self.chord(Chord::Cast),
                Button::Timetable => self.chord(Chord::Timetable),
                Button::RunRitual => self.ritual(Act::Run),
                Button::ToggleRitual => self.ritual(Act::Toggle),
                Button::RemoveRitual => self.ritual(Act::Remove),
                Button::Quit => self.chord(Chord::Quit),
                Button::Help => self.chord(Chord::Help),
                Button::Capture => self.chord(Chord::Capture),
                Button::RecallFocused => self.recall_focused(),
                Button::CloseFocused => self.chord(Chord::Close),
                Button::Yes => self.confirm(),
                Button::No => self.go_back(),
            },
            Hit::Item(i) => self.click_item(i),
            Hit::Modal | Hit::Grid => {}
        }
    }

    /// The first resident after the one on screen (before it, going back) that `pick` takes,
    /// round from the other end and the one on screen last; false when there is none. Taken in
    /// the sidebar's order, a lead's helpers right after it.
    fn round(&mut self, by: isize, pick: impl Fn(&Resident) -> bool) -> bool {
        let order: Vec<&Resident> =
            render::drawn(&self.m.residents).iter().map(|&(i, _)| &self.m.residents[i]).collect();
        let n = order.len() as isize;
        let at = order.iter().position(|r| Some(&r.id) == self.m.focused.as_ref());
        let at = at.map_or(if by > 0 { -1 } else { 0 }, |i| i as isize);
        let found = (1..=n)
            .map(|i| order[(at + by * i).rem_euclid(n.max(1)) as usize])
            .find(|r| pick(r))
            .map(|r| r.id.clone());
        let found_any = found.is_some();
        if found_any {
            self.focus(found);
        }
        found_any
    }

    fn recall_focused(&mut self) {
        if let Some(r) = self.focused().filter(|r| r.departed.is_some()) {
            let who = r.id.clone();
            self.send(Request::Recall { who });
        }
    }

    fn chord(&mut self, ch: Chord) {
        (self.m.message, self.said) = (None, None);
        match ch {
            Chord::Summon => {
                self.m.modal = Some(Modal::Summon(Summon::default()));
                self.refresh_modal();
            }
            Chord::Banish => {
                if let Some(r) = self.focused().filter(|r| r.departed.is_none()) {
                    self.m.modal = Some(Modal::Banish { id: r.id.clone(), name: r.name.clone() });
                }
            }
            Chord::Recall => {
                self.m.modal = Some(Modal::Recall(Recall::default()));
                self.refresh_modal();
            }
            Chord::Cast => {
                self.m.modal = Some(Modal::Cast(Cast::default()));
                self.send(Request::Cards);
            }
            Chord::Timetable => {
                self.m.modal = Some(Modal::Timetable(Timetable::default()));
                self.send(Request::Rituals);
            }
            Chord::Quit => self.m.modal = Some(Modal::Quit),
            Chord::Help => self.m.modal = Some(Modal::Help),
            Chord::History => self.m.modal = Some(Modal::History(History::default())),
            Chord::Capture => {
                self.m.capture = !self.m.capture;
                (self.held, self.edge) = (None, None);
                self.host.extend_from_slice(if self.m.capture { CAPTURE_ON } else { CAPTURE_OFF });
                self.any_motion = false;
                self.modes();
            }
            Chord::Detach => self.done = Some("detached; the residents keep running".into()),
            // A live one is asked first: its turn and its input line go with it. A departed
            // one just leaves the sidebar.
            Chord::Close => match self.focused() {
                Some(r) if r.departed.is_none() => {
                    self.m.modal = Some(Modal::Close { id: r.id.clone(), name: r.name.clone() });
                }
                Some(r) => {
                    let who = r.id.clone();
                    self.send(Request::Close { who });
                }
                None => {}
            },
            Chord::Focus(n) => match self.m.residents.iter().find(|r| r.slot == Some(n)) {
                Some(r) => self.focus(Some(r.id.clone())),
                None => self.say(Say::Info, format!("nobody is in slot {n}")),
            },
            Chord::Next => {
                self.round(1, |_| true);
            }
            Chord::Prev => {
                self.round(-1, |_| true);
            }
            Chord::Awaiting => {
                if !self.round(1, |r| r.state.needs_you()) {
                    self.say(Say::Info, "nobody needs you");
                }
            }
            Chord::Leader => {
                if let Some(who) = self.live() {
                    self.input(who, Forward::Key(keys::LEADER));
                }
            }
            Chord::ScrollBack if self.m.modes.alt => {
                self.say(Say::Info, "a full-screen program has no scrollback");
            }
            Chord::ScrollBack => {
                let half = i32::from(self.size.1.max(2) / 2);
                self.scroll(Scrollback::By(-half));
            }
            Chord::Find if self.m.modes.alt => {
                self.say(Say::Info, "a full-screen program has no scrollback");
            }
            Chord::Find if self.live().is_some() => self.find(true),
            Chord::Find => {}
            Chord::Copy => match self.focused() {
                Some(r) => {
                    let (who, what) = (r.id.clone(), format!("{}'s last answer", r.name));
                    let id = self.send(Request::Read { who, screen: false, bare: true });
                    self.copying.push((id, what));
                }
                None => self.say(Say::Info, "nobody is on screen"),
            },
            Chord::Cancel | Chord::Unbound => {}
        }
    }
}

/// A left press in the resident's grid, while the button is held.
struct Held {
    /// The grid when it was pressed.
    grid: Rect,
    /// The cell under the pointer, held to the grid, and the last one the daemon was sent.
    at: (u16, u16),
    sent: (u16, u16),
    /// Past the grid's top edge (true) or its bottom.
    past: Option<bool>,
}

/// Cells `xs` of row `y` printed again as ratatui printed them from `buf`: a wide character
/// over its two cells, and none that would run past `xs`.
fn reprint(out: &mut Vec<u8>, buf: &Buffer, y: u16, xs: Range<u16>) {
    use ratatui::backend::Backend;
    let mut cells = Vec::new();
    let mut skip = 0u16;
    for x in xs.clone().filter(|&x| buf.area.contains(Position::new(x, y))) {
        let c = &buf[(x, y)];
        if skip > 0 || c.diff_option == CellDiffOption::Skip {
            skip = skip.saturating_sub(1);
            continue;
        }
        let w = c.cell_width().max(1);
        if x + w > xs.end {
            break;
        }
        skip = w - 1;
        cells.push((x, y, c));
    }
    let _ = CrosstermBackend::new(out).draw(cells.into_iter());
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
