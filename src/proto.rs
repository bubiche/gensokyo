//! The wire protocol: one JSON object per line over a unix socket, both ways. A client says
//! `hello` first and gets `welcome`; every other request carries an `id` its reply echoes.
//! `watch`, `view`, `unview`, `input`, `scroll`, `search`, `select`, `resize`, `focus`, `hook`
//! and `statusline` are answered only when they fail, except a `select` release, answered with
//! `done` and the text selected; `wait` when its residents have news. `watch` also brings `rituals` at once and whenever the timetable changes, and the `notices` kept. Events carry no `id` and go only to connections that asked for them
//! (`watch`, `view`).

use crate::vt::{Frame, Modes, Pointer, Run};
use serde::{Deserialize, Serialize};

pub const PROTO: u32 = 15;

/// The longest a notice is, in characters: long enough that a crash's, naming everyone it cut
/// off, is never cut.
pub const NOTICE_MOST: usize = 1000;

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(default)]
    pub id: u64,
    #[serde(flatten)]
    pub req: Request,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Request {
    Hello {
        proto: u32,
        who: String,
        /// The resident this runs inside (`GENSOKYO_RESIDENT`), by id: its connection has a
        /// resident's rights, not the user's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resident: Option<String>,
    },
    List {
        /// Also everyone in `departed/`, from earlier runs.
        #[serde(default)]
        all: bool,
    },
    Summon(Summon),
    /// HUP, the grace, TERM, KILL. The resident stays in the shrine as departed.
    Banish {
        who: String,
    },
    /// A live resident is asked to `/exit`; a departed one leaves the shrine.
    Close {
        who: String,
    },
    /// A departed resident comes back: `claude --resume` with its old settings and name.
    Recall {
        who: String,
    },
    /// Live residents started again on the claude installed now, each into its own
    /// conversation once it rests; with nobody named, everyone behind it. Answered with `done`
    /// at once, saying who goes now and who once they rest.
    Renew {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        who: Vec<String>,
    },
    /// `residents` events from now on, whenever the shrine changes.
    Watch,
    /// This connection shows `who`: a `frame` now, then `damage` as the screen changes. A new
    /// `view` replaces the old one.
    View {
        who: String,
    },
    Unview,
    /// Bytes for the resident's tty as they are, or a key for its own encoder to encode.
    Input {
        who: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bytes: Vec<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<Key>,
    },
    /// The resident's scrollback: `rows` back (negative) or forward, or with none, back to the
    /// live screen. It is the resident's, so every client viewing it scrolls with it.
    Scroll {
        who: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rows: Option<i32>,
    },
    /// Looks through the resident's scrollback and screen for `needle`, `back` toward older
    /// output, from what the last search found or the view's far edge: the view moves to it and
    /// every client viewing it sees it drawn, until the view is back on the live screen.
    Search {
        who: String,
        needle: String,
        #[serde(default)]
        back: bool,
    },
    /// What the pointer did at cell `x`, `y` of the resident's screen as shown: this
    /// connection's own selection, drawn in its frames alone and kept on its text as output
    /// comes. A release is answered with `done` and the text selected, empty when none is.
    Select {
        who: String,
        how: Pointer,
        x: u16,
        y: u16,
    },
    /// The size of the client's grid: every live resident and every later summon takes it.
    /// The last client to say wins.
    Resize {
        cols: u16,
        rows: u16,
    },
    /// Every spell card, by title, and the card files whose names are not usable.
    Cards,
    /// A card typed into each target as a prompt; answered with `done` saying who got it and
    /// who was left out, or an error when nobody did.
    Cast(Cast),
    /// Every ritual with its next fire, and the ritual files whose names are not usable.
    Rituals,
    /// A ritual fired now, paused, resumed or deleted; answered with `done`.
    Ritual {
        verb: RitualVerb,
        name: String,
    },
    /// Resident `who`'s tags changed: each of `set` is `key=value`, or `key=` to remove one;
    /// with `clear`, all go first. With neither, only asked. Answered with `done`: its tags.
    Tag {
        who: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        set: Vec<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        clear: bool,
    },
    /// Everyone is asked to `/exit`, then the daemon stops.
    Quit,
    /// Keeping the Mac from idle-sleeping while work runs turned on or off, and kept in the
    /// config; with neither, only asked. Answered with `done`: the setting, and whom it holds
    /// the Mac for.
    Awake {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on: Option<bool>,
    },
    /// Held until the residents named have news: a turn ended, a dialog or question opened, or
    /// they departed. Answered with `waited`, whether that came or the timeout did.
    Wait(Wait),
    /// A resident's last answer, kept past its departure; with `screen`, its live screen as text.
    /// With `bare`, the answer's text alone, and an error when its last turn left none.
    Read {
        who: String,
        #[serde(default)]
        screen: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        bare: bool,
    },
    /// The client's host terminal gained or lost focus. Until it first says, it has not.
    Focus {
        on: bool,
    },
    /// From `gensokyo _hook` inside a resident: one Claude Code hook, reduced.
    Hook {
        resident: String,
        hook: Hook,
    },
    /// From `gensokyo _statusline` inside a resident: its latest status line report.
    Statusline {
        resident: String,
        telemetry: Telemetry,
    },
}

/// The hello a client or the CLI says, naming the resident it runs inside, if any.
pub fn hello(who: &str) -> Request {
    let resident = std::env::var("GENSOKYO_RESIDENT").ok().filter(|r| !r.is_empty());
    Request::Hello { proto: PROTO, who: who.into(), resident }
}

/// What a hook said, as much of it as the shrine needs: never the prompt itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hook {
    /// `hook_event_name`.
    pub event: String,
    pub session: Option<String>,
    /// Epoch milliseconds, taken when the hook ran.
    pub at: i64,
    /// `notification_type` for a Notification, `source` for a SessionStart.
    pub kind: Option<String>,
    pub mode: Option<String>,
    pub tool: Option<String>,
    /// One line: the question asked, the notification's message, or the reply's first line.
    pub text: Option<String>,
    /// The reply itself, on `Stop` and `StopFailure`: what `read` gives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Where Claude works when the hook ran: it follows a `cd`, the launch dir does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

impl Hook {
    /// A session started after a compact or a `/clear`, which took the conversation: the
    /// daemon answers it with `done`, what a lead should know of its helpers, or nothing.
    pub fn briefs(&self) -> bool {
        self.event == "SessionStart" && matches!(self.kind.as_deref(), Some("compact" | "clear"))
    }
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ended {
    /// With a `Stop`, and its answer.
    Stop,
    /// On an API error (`StopFailure`).
    Failed,
    /// At a dialog (Esc, a denial) or cut short by a restart: no answer.
    Interrupted,
    /// With no `Stop` and no dialog: Claude Code's idle notice stood in for it, so its answer
    /// is not known.
    Unreported,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Wait {
    /// Residents by name, slot or id, read once as the wait begins.
    pub who: Vec<String>,
    /// Back when any one of them has news, rather than each.
    #[serde(default)]
    pub any: bool,
    /// Only this kind of news counts; every kind when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Until>,
    /// Seconds.
    pub timeout: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Until {
    /// A turn ended.
    Done,
    /// A dialog or a question opened.
    Needs,
    /// It departed.
    Gone,
}

/// One resident as `wait` reports it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub id: String,
    pub name: String,
    pub state: State,
    /// Its turns ended and dialogs opened, ever; and whether either is news to this wait.
    pub turns: u64,
    pub needs: u64,
    pub news: bool,
    /// How its last turn ended, and that turn's answer, first line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended: Option<Ended>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// A resident's last status line report. Every field is missing until Claude Code sends it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Telemetry {
    pub model: Option<String>,
    /// `advisorModel` from the settings chain.
    pub advisor: Option<String>,
    pub effort: Option<String>,
    /// Context window used, percent, and its size in tokens.
    pub ctx: Option<u32>,
    pub window: Option<u64>,
    /// Cache hit rate, percent: the session's, and the last request's.
    pub cache: Option<u32>,
    pub turn_cache: Option<u32>,
    /// Whether the cached prefix was warm at the report, and when it goes cold, epoch seconds.
    pub cache_warm: Option<bool>,
    pub cache_until: Option<i64>,
    pub cost: Option<f64>,
    pub added: Option<u64>,
    pub removed: Option<u64>,
    /// Seconds the session has run.
    pub age: Option<u64>,
    /// The account's usage windows, Pro and Max only.
    pub five_hour: Option<Limit>,
    pub seven_day: Option<Limit>,
    /// Epoch seconds, set by the daemon when the report came.
    pub at: i64,
    /// `workspace.current_dir`, which the daemon takes out as the resident's `here`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// The Claude Code it runs: an update on disk waits for the session to start again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Limit {
    pub used: u32,
    pub resets: Option<i64>,
}

/// What a resident is doing, as its glyph shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Busy,
    /// A permission prompt, or a finished turn nobody has answered.
    Awaits,
    /// A question it asked.
    Asked,
    #[default]
    Resting,
    Departed,
}

impl State {
    pub fn glyph(self) -> &'static str {
        match self {
            State::Busy => "●",
            State::Awaits => "✦",
            State::Asked => "✧",
            State::Resting => "○",
            State::Departed => "·",
        }
    }

    /// Waiting on the user: drawn in gold.
    pub fn needs_you(self) -> bool {
        matches!(self, State::Awaits | State::Asked)
    }
}

/// A key in kitty's model: `code` is a Unicode codepoint or a kitty functional-key number,
/// `mods` the kitty modifier parameter minus one, `event` 1 press, 2 repeat, 3 release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub code: u32,
    pub mods: u8,
    pub event: u8,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Summon {
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Any number of lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// `claude --allowedTools`, kept for a recall too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// A role's name, or the full path of a file: appended to its system prompt (`role.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Words of the user's own appended to its system prompt, beside the role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// A checkout of its own, `<repo>/.claude/worktrees/<slug>`, made (or found) first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeAsk>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct WorktreeAsk {
    pub slug: String,
    /// Else `BRANCH_PREFIX` and the slug.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// What a new branch starts from; else the remote's default branch, fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Cast {
    /// A card's slug or title, or part of one that picks out a single card.
    pub card: String,
    /// `all`, `awaiting`, `idle`, or residents by name, slot or id.
    pub targets: Vec<String>,
    /// For a pair card: the resident the target talks to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
}

/// A spell card as a picker lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    /// The file name without `.md`.
    pub slug: String,
    pub title: String,
    pub summary: String,
    /// `peer: required`: cast at one resident, with a peer.
    pub pair: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RitualVerb {
    /// Fire it now, whatever the schedule says (and whether or not it is enabled).
    Run,
    Enable,
    Disable,
    /// The file, its notes and its journal. Never a shipped one.
    Remove,
}

/// A ritual as the timetable and `ritual list --json` show it. The keys are the ones the
/// `gensokyo-ritual` skill reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RitualInfo {
    /// The file name without `.md`.
    pub name: String,
    pub enabled: bool,
    pub schedule: String,
    /// Epoch seconds; none while disabled or when the schedule never comes round.
    pub next_fire: Option<i64>,
    /// The same minute on this machine's clock, `YYYY-MM-DD HH:MM`.
    pub next_fire_local: Option<String>,
    /// When it last ran or was sent, epoch seconds, from its journal.
    pub last_run: Option<i64>,
    /// The newest journal line.
    pub last: Option<String>,
    /// `new`, `persistent`, or a resident's name.
    pub target: String,
    pub headless: bool,
    pub keep: String,
    pub overlap: String,
    /// `idle` or `now`: when a prompt is typed into a resident.
    pub deliver: String,
    pub cwd: Option<String>,
    pub description: Option<String>,
    /// What each run may do without asking: its permission mode, its allowed tools, its MCP
    /// config, and the probe the daemon runs before each fire, as written.
    pub mode: Option<String>,
    pub allowed_tools: Vec<String>,
    pub mcp_config: Option<String>,
    /// The role appended to each run's system prompt, as written.
    pub role: Option<String>,
    pub when: Option<String>,
    /// Why it cannot fire, when something is wrong with it.
    pub problem: Option<String>,
    pub path: String,
    /// One of the examples in `share/rituals/`, not the user's own file.
    pub shipped: bool,
    /// A run of it is still going (daemon only).
    pub running: bool,
}

// A frame's rows dwarf the rest, and every reply is written out at once.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Reply {
    Welcome {
        proto: u32,
        pid: u32,
    },
    List {
        id: u64,
        residents: Vec<Resident>,
    },
    Summoned {
        id: u64,
        resident: Resident,
        /// What the user should know about how it was made: a worktree reused, a base that
        /// could not be fetched.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Done {
        id: u64,
        message: String,
    },
    Cards {
        id: u64,
        cards: Vec<Card>,
        /// Paths of card files left out for their names.
        unusable: Vec<String>,
    },
    Rituals {
        id: u64,
        rituals: Vec<RitualInfo>,
        /// Paths of ritual files left out for their names.
        unusable: Vec<String>,
    },
    Error {
        id: u64,
        error: String,
    },
    /// A `wait`'s end: `met` false when its timeout came first.
    Waited {
        id: u64,
        met: bool,
        residents: Vec<Report>,
    },
    /// Event: the shrine changed.
    Residents {
        residents: Vec<Resident>,
    },
    /// Event: the whole screen of the resident this connection views. `rev` numbers it.
    Frame {
        who: String,
        rev: u64,
        frame: Frame,
        modes: Modes,
    },
    /// Event: the rows that changed since `base`, same size. A `base` the client doesn't hold
    /// means it missed one: it asks for a `view` again.
    Damage {
        who: String,
        base: u64,
        rev: u64,
        rows: Vec<(u16, Vec<Run>)>,
        cursor: Option<(u16, u16)>,
        modes: Modes,
        /// As in `Frame`: how far it is scrolled back, of how much scrollback.
        #[serde(default)]
        back: u32,
        #[serde(default)]
        history: u32,
    },
    /// Event: a resident has just come to need the user. `watched` says some client has it on
    /// screen in a focused terminal, and nothing should ring.
    Notify {
        who: String,
        name: String,
        state: State,
        text: String,
        watched: bool,
    },
    /// Event: news about a ritual rather than a resident (a run that could not start, one that
    /// finished headless), or a crash's. Shown and sent to the desktop; no bell. The daemon
    /// keeps the last few for clients that come later.
    Notice {
        text: String,
    },
    /// Event, once as a `watch` begins: the notices kept, newest first.
    Notices {
        notices: Vec<Notice>,
    },
    /// Event, as a `watch` begins and whenever it changes: whether the daemon keeps the Mac
    /// from idle-sleeping while work runs, and whether it holds it awake now.
    Awake {
        on: bool,
        held: bool,
    },
}

/// A notice as the daemon keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    /// Epoch seconds.
    pub at: i64,
    pub text: String,
    /// No client was watching when it came, nor has one started to since.
    #[serde(default)]
    pub missed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Resident {
    pub id: String,
    pub name: String,
    pub slot: Option<u8>,
    pub cwd: String,
    /// Where it works now, when that is not `cwd`: Claude moved into a worktree or a subdir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub here: Option<String>,
    /// The leader's pid while it runs.
    pub pid: Option<i32>,
    /// Epoch seconds.
    pub departed: Option<i64>,
    pub exit: Option<i32>,
    pub signal: Option<i32>,
    #[serde(default)]
    pub state: State,
    /// One line on what it waits for.
    #[serde(default)]
    pub detail: Option<String>,
    /// The permission mode its hooks last reported, else the one it was started with.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub telemetry: Option<Telemetry>,
    /// Why nothing may be typed into it now (a dialog, a question, a prompt the user has half
    /// typed), which `awaits` and `asked` alone do not tell apart from a finished turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    /// Done with the last prompt it was given, and nothing waits on the user.
    #[serde(default)]
    pub finished: bool,
    /// The resident that summoned it, by id: its lead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Turns ended and dialogs or questions opened, over its life.
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub needs: u64,
    /// The Claude Code installed now, when it runs an older one (or any other): a renew starts
    /// it again on this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outdated: Option<String>,
    /// Asked to renew: it goes once it rests.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub renewing: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "pairs")]
    pub tags: Tags,
}

/// A resident's tags, `key` and `value`, in the order each key was first set.
pub type Tags = Vec<(String, String)>;

/// Tags as a JSON object, its keys in their order.
pub mod pairs {
    use super::Tags;
    use serde::de::{MapAccess, Visitor};
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &Tags, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(t.iter().map(|(k, v)| (k, v)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Tags, D::Error> {
        struct InOrder;
        impl<'de> Visitor<'de> for InOrder {
            type Value = Tags;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of tags")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Tags, A::Error> {
                let mut t = Tags::new();
                // A key twice (a hand-edited record) is one tag, the last value in its place.
                while let Some((k, v)) = m.next_entry::<String, String>()? {
                    match t.iter_mut().find(|(have, _)| *have == k) {
                        Some(have) => have.1 = v,
                        None => t.push((k, v)),
                    }
                }
                Ok(t)
            }
        }
        d.deserialize_map(InOrder)
    }
}
