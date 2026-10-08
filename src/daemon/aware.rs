//! What a resident is doing, from two sources: its hooks, which say what it waits for, and
//! Claude Code's registry (`claude agents --json`), which says whether it is busy or has a
//! dialog open. Nothing here looks at the screen.
//!
//! Precedence: a question, then a dialog the registry sees, then busy, then a flag the hooks
//! raised (a permission prompt, a finished turn), then resting. The registry counts only when
//! its snapshot began after the newest hook: one taken before a `Stop` knows nothing of it.

use crate::proto::{Ended, Hook, State};
use crate::tele;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    /// A permission prompt or another dialog.
    Awaits,
    /// A question (AskUserQuestion).
    Asked,
    /// A finished turn.
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Registry {
    Idle,
    Busy,
    /// A permission or plan dialog is open.
    Waiting,
}

#[derive(Clone, Debug, Default)]
pub struct Aware {
    pub pending: Option<Pending>,
    pub detail: Option<String>,
    /// The hooks say a turn is running: a prompt went in, or a question was answered.
    running: bool,
    /// A prompt has gone in since it started.
    prompted: bool,
    /// The newest hook applied, epoch ms.
    hook_at: i64,
    /// The last snapshot and when it began.
    registry: Option<(Registry, i64)>,
    /// The permission mode, from UserPromptSubmit and Stop.
    pub mode: Option<String>,
    /// The flag is a dialog the registry shows as `waiting` while it is open (a permission
    /// prompt, a question), so an idle snapshot means it was answered.
    shown: bool,
    /// What the user has typed into its input line since its last prompt. A card or a
    /// ritual's prompt pasted into a draft would go in with it, as one prompt (seen on 2.1.289).
    line: Line,
    /// A card or a ritual's prompt being typed in (`mark`), and how many marks there have been.
    typing: Option<Typing>,
    marks: u64,
    news: News,
}

/// gensokyo typing into it, from before the paste until a hook after the Enter: another prompt
/// typed meanwhile would go in with this one, or into the turn it starts.
#[derive(Clone, Copy, Debug)]
struct Typing {
    n: u64,
    since: Instant,
    /// When the Enter went, epoch ms: a prompt hook from before it is some other prompt's.
    entered: Option<i64>,
}

/// The longest a mark holds: a prompt that fires no hook (a built-in command) leaves it.
const TYPING_MOST: Duration = Duration::from_secs(15);

/// What happened since `take` last asked: turns ended, how the last of them did, and dialogs
/// or questions opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct News {
    pub ends: u32,
    pub ended: Option<Ended>,
    pub opened: u32,
}

/// What a resident's input line has in it: something the user typed and has not sent.
const DRAFT: &str = "has a prompt half typed into it (Ctrl-C there clears it)";

/// Not listed by the registry yet, so maybe at the workspace trust dialog.
pub const STARTING: &str = "is still starting up";

/// What `mark` holds back.
pub const TYPING: &str = "has a card or a ritual's prompt being typed in";

/// What a resident's input line has, as far as the user's keys tell (nothing here sees the
/// line itself): a draft, or nothing. A backspaced line stays a draft.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub draft: bool,
    /// The draft began with `/` or `!`: a built-in command (`/context`, `/cost`) or a shell
    /// command, which Enter runs, leaving the line empty, where a built-in sends no prompt hook
    /// to say so. A panel one opens (`/cost`) the registry shows as `waiting` (2.1.29x), a
    /// dialog. Any other draft waits for its prompt hook, which says that it went in.
    command: bool,
    /// The last key typed `\`, which makes the next Enter a newline.
    backslash: bool,
    /// Inside a bracketed paste, where everything is text.
    paste: bool,
    /// An Enter went in after text, the user's or a card's: a prompt whose hook may not have
    /// come yet.
    sent: bool,
    /// Typed since that Enter: its prompt hook, coming late, leaves this in the line.
    after: bool,
}

/// One key's effect on the input line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Leaves this character there.
    Text(char),
    /// Up, which brings back an earlier prompt.
    Recall,
    /// Ctrl-C, which empties it.
    Clear,
    /// Enter alone; Shift-Enter and Alt-Enter make a newline.
    Enter,
}

impl Line {
    /// Keys as they are typed, legacy bytes and kitty's `CSI u` alike. Tab, Backspace, the
    /// other arrows, focus and mouse reports leave the line as it was.
    pub fn keys(&mut self, bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != 0x1b {
                match bytes[i] {
                    b if self.paste => self.key(Key::Text(b as char)),
                    0x03 => self.key(Key::Clear),
                    b'\r' => self.key(Key::Enter),
                    b @ (0x20..=0x7e | 0x80..) => self.key(Key::Text(b as char)),
                    _ => {}
                }
                i += 1;
                continue;
            }
            match bytes.get(i + 1) {
                // CSI: parameters up to a final byte. X10 mouse is `CSI M` and three bytes of its
                // own, which look like text.
                Some(b'[') => {
                    let body = &bytes[i + 2..];
                    let Some(end) = body.iter().position(|c| (0x40..=0x7e).contains(c)) else {
                        break;
                    };
                    let (params, fin) = (&body[..end], body[end]);
                    i += 3 + end;
                    match (fin, params) {
                        (b'M', b"") => i += 3,
                        (b'A', b"" | b"1") => self.key(Key::Recall),
                        (b'~', b"200") => self.paste = true,
                        (b'~', b"201") => self.paste = false,
                        (b'u', _) => csi_u(params).into_iter().for_each(|k| self.key(k)),
                        _ => {}
                    }
                }
                // SS3 (F1-F4, arrows in application mode), then Alt and a key (Alt-Enter too).
                Some(b'O') => {
                    if bytes.get(i + 2) == Some(&b'A') {
                        self.key(Key::Recall);
                    }
                    i += 3;
                }
                Some(_) => i += 2,
                None => i += 1,
            }
        }
    }

    pub fn key(&mut self, k: Key) {
        let newline = std::mem::take(&mut self.backslash);
        match k {
            Key::Text(c) => {
                // A new line's first key, or the first after an Enter that may have sent it.
                if !self.draft || (self.sent && !self.after) {
                    self.command = matches!(c, '/' | '!');
                }
                (self.draft, self.backslash, self.after) = (true, c == '\\', true);
            }
            // Whatever it brings back, only its own hook says it went.
            Key::Recall => (self.draft, self.command, self.after) = (true, false, true),
            Key::Clear => *self = Line { paste: self.paste, ..Line::default() },
            Key::Enter if self.command && !newline => (self.draft, self.command) = (false, false),
            Key::Enter if self.draft && !newline => (self.sent, self.after) = (true, false),
            Key::Enter => {}
        }
    }

    /// A prompt went in. Only what was typed after the Enter that sent it is still there.
    fn went_in(&mut self) {
        *self = match (self.sent, self.after) {
            (true, true) => Line { sent: false, ..*self },
            _ => Line::default(),
        };
    }
}

/// `code[:alternates];mods[:event]` of a kitty key.
fn csi_u(params: &[u8]) -> Option<Key> {
    let p = std::str::from_utf8(params).ok()?;
    let mut fields = p.split(';');
    let code = fields.next()?.split(':').next()?.parse().ok()?;
    let mut m = fields.next().unwrap_or("1").split(':');
    let mods: u8 = m.next()?.parse().ok()?;
    let event = m.next().map_or(Some(1), |e| e.parse().ok())?;
    kitty_key(code, mods.saturating_sub(1), event)
}

/// One kitty key: `code` a codepoint or a functional key's number, `mods` its modifier bits.
pub fn kitty_key(code: u32, mods: u8, event: u8) -> Option<Key> {
    // Shift, Caps Lock and Num Lock still type text; Alt, Ctrl and Super do not.
    let others = mods & !(1 | 64 | 128);
    match code {
        _ if event == 3 => None,
        99 | 67 if others == 4 => Some(Key::Clear),
        13 if mods & !(64 | 128) == 0 => Some(Key::Enter),
        // The functional keys start at 57344, in the private use area.
        _ if others == 0 && code >= 0x20 && code != 0x7f && code < 57344 => {
            char::from_u32(code).map(Key::Text)
        }
        _ => None,
    }
}

impl Aware {
    /// Applies one hook; false, and nothing changes, for one older than the newest applied.
    pub fn hook(&mut self, h: &Hook) -> bool {
        if h.at < self.hook_at {
            return false;
        }
        self.hook_at = h.at;
        self.typed_in(h.at);
        if let Some(m) = &h.mode {
            self.mode = Some(tele::clean(m, 20));
        }
        let text = h.text.as_deref().map(|t| tele::clean(t, 80)).filter(|t| !t.is_empty());
        let ask = h.tool.as_deref() == Some("AskUserQuestion");
        match (h.event.as_str(), h.kind.as_deref()) {
            ("UserPromptSubmit", _) => {
                self.set(None, None);
                self.running = true;
                self.prompted = true;
                self.line.went_in();
            }
            ("Stop", _) => {
                self.set(Some(Pending::Stopped), text);
                self.running = false;
                self.end(Ended::Stop);
            }
            // An API error ended the turn, in place of a Stop.
            ("StopFailure", _) => {
                let why = text.as_deref().unwrap_or("an API error");
                self.set(Some(Pending::Stopped), Some(format!("stopped on {why}")));
                self.running = false;
                self.end(Ended::Failed);
            }
            // The turn holds until the dialog is answered. A question's own dialog brings a
            // permission_prompt too, some seconds in (2.1.283): it stays a question.
            ("Notification", Some(k))
                if ["permission_prompt", "agent_needs_input"].contains(&k)
                    || k.starts_with("elicitation") =>
            {
                if self.pending != Some(Pending::Asked) {
                    self.set(Some(Pending::Awaits), text);
                    self.shown = k == "permission_prompt";
                    self.news.opened += 1;
                }
                self.running = false;
            }
            // Sent a minute after a turn: a late stand-in for a Stop that went missing. Not
            // after one that came, or a dialog dismissed: that turn has already been told of.
            ("Notification", Some("idle_prompt")) => {
                if self.pending.is_none() && self.running {
                    self.set(Some(Pending::Stopped), None);
                    self.end(Ended::Unreported);
                }
                self.running = false;
            }
            ("PreToolUse", _) if ask => {
                self.set(Some(Pending::Asked), text);
                self.shown = true;
                self.news.opened += 1;
            }
            ("PostToolUse", _) if ask => {
                if self.pending == Some(Pending::Asked) {
                    self.set(None, None);
                }
                self.running = true;
            }
            // A fresh conversation: nothing is waiting any more.
            ("SessionStart", Some("clear")) => {
                self.set(None, None);
                self.running = false;
                self.line = Line::default();
            }
            _ => {}
        }
        true
    }

    /// A registry snapshot that began at `at`; `None` when the session was not in it.
    pub fn registry(&mut self, st: Option<Registry>, at: i64) {
        self.registry = st.map(|s| (s, at));
        // Busy after the flag went up: the permission was granted, or a new turn began. Idle
        // after a dialog: Esc or a denial, which end the turn with no Stop, no PostToolUse and
        // no idle_prompt (2.1.283).
        let after = at > self.hook_at;
        match (st, self.pending) {
            (Some(Registry::Busy), Some(Pending::Awaits | Pending::Stopped)) if after => {
                self.set(None, None);
                self.running = true;
            }
            (Some(Registry::Idle), Some(Pending::Awaits | Pending::Asked))
                if after && self.shown =>
            {
                self.set(None, None);
                self.end(Ended::Interrupted);
            }
            _ => {}
        }
    }

    /// Why nothing may be typed into it now, or nothing: a dialog, what the user has typed
    /// into its input line, which a card would be sent along with, or another card or prompt
    /// on its way in. A finished turn is not in the way: typing into that is what a card is for.
    pub fn blocked(&self) -> Option<&'static str> {
        self.in_way().or(self.being_typed().then_some(TYPING))
    }

    /// A dialog, or a draft: what a prompt being typed in would answer or go in with.
    pub fn in_way(&self) -> Option<&'static str> {
        self.dialog().or(self.line.draft.then_some(DRAFT))
    }

    /// Taken for typing into until `unmark`, a hook after `entered`, or `TYPING_MOST`.
    /// The mark, to hand back.
    pub fn mark(&mut self) -> u64 {
        self.marks += 1;
        self.typing = Some(Typing { n: self.marks, since: Instant::now(), entered: None });
        self.marks
    }

    /// The Enter is going in, at epoch ms `at`.
    pub fn entered(&mut self, mark: u64, at: i64) {
        if let Some(t) = self.typing.as_mut().filter(|t| t.n == mark) {
            t.entered = Some(at);
            (self.line.sent, self.line.after) = (true, false);
        }
    }

    /// Done typing, with nothing sent.
    pub fn unmark(&mut self, mark: u64) {
        if self.typing.is_some_and(|t| t.n == mark) {
            self.typing = None;
        }
    }

    /// A hook at `at`, after the Enter: it was read, and the mark is done with. Any hook, not
    /// only the prompt's: hooks come from processes of their own, and one stamped later that
    /// lands first gets the prompt's dropped as older.
    fn typed_in(&mut self, at: i64) {
        if self.typing.is_some_and(|t| t.entered.is_some_and(|e| at >= e)) {
            self.typing = None;
        }
    }

    fn being_typed(&self) -> bool {
        self.typing.is_some_and(|t| t.since.elapsed() < TYPING_MOST)
    }

    /// A dialog open, which would take the Enter after a card and answer it. So would one the
    /// registry cannot see yet: a session it does not list may be on the workspace trust
    /// dialog, where no hook fires either.
    pub fn dialog(&self) -> Option<&'static str> {
        match (self.pending, self.registry) {
            (Some(Pending::Asked), _) => Some("is asking you a question"),
            (_, None) => Some(STARTING),
            (Some(Pending::Awaits), _) | (_, Some((Registry::Waiting, _))) => {
                Some("has a dialog waiting for you")
            }
            _ => None,
        }
    }

    /// Neither at work, nor at a dialog or question it opened, nor holding a draft or being
    /// typed into: resting, or done. One the registry has yet to list counts.
    pub fn idle(&self) -> bool {
        !self.open() && !self.line.draft && !self.being_typed() && self.state() != State::Busy
    }

    /// When it was last heard from by a hook.
    pub fn heard(&self) -> i64 {
        self.hook_at
    }

    /// A dialog the hooks or the registry show.
    pub fn open(&self) -> bool {
        matches!(self.pending, Some(Pending::Awaits | Pending::Asked))
            || matches!(self.registry, Some((Registry::Waiting, _)))
    }

    /// In the last registry snapshot: other sessions can message it.
    pub fn listed(&self) -> bool {
        self.registry.is_some()
    }

    /// Done with what it was given: a prompt went in, its turn is over and nothing waits on the
    /// user. One just started has not got as far as its prompt (SessionStart comes first).
    /// A draft does not count: a ritual run the user typed into and left would never finish.
    pub fn finished(&self) -> bool {
        let quiet = self.dialog().is_none() && !self.being_typed();
        self.prompted && !self.running && quiet && self.state() != State::Busy
    }

    /// Resumed with its conversation: its prompts went in before this start.
    pub fn resumed(&mut self) {
        self.prompted = true;
    }

    /// The new start a renew made of it: nothing known yet but a finished turn the user has not
    /// seen, which still waits on them.
    pub fn renewed(&self, resumed: bool) -> Aware {
        // A hook the old process sent late is older than anything the new one says.
        // Marks keep counting, so one a card still holds is never taken for a new one.
        let mut a = Aware { hook_at: self.hook_at, marks: self.marks, ..Aware::default() };
        if self.pending == Some(Pending::Stopped) {
            a.set(self.pending, self.detail.clone());
        }
        if resumed {
            a.resumed();
        }
        a
    }

    /// On screen in a focused terminal: a finished turn has been seen. A dialog still waits.
    /// True if that cleared it.
    pub fn seen(&mut self) -> bool {
        let stopped = self.pending == Some(Pending::Stopped);
        if stopped {
            self.set(None, None);
        }
        stopped
    }

    /// Something pasted into its input line was not sent: it may be there still, dialog or not,
    /// until a prompt goes in or Ctrl-C clears it.
    pub fn unsent(&mut self) {
        (self.line.draft, self.line.command) = (true, false);
    }

    /// The user typed these keys into it. Not while a dialog the hooks or the registry show is
    /// open, which is what the keys went to. One still starting up counts as the input line:
    /// keys at a trust dialog are safer called a draft.
    pub fn typed(&mut self, keys: &[u8]) {
        if !self.open() {
            self.line.keys(keys);
        }
    }

    /// A finished turn seen on its lead's behalf (`seen`) that nobody collected after all:
    /// waiting on the user again.
    pub fn unseen(&mut self) {
        if self.pending.is_none() && !self.running {
            self.pending = Some(Pending::Stopped);
        }
    }

    /// Its turn cut short from outside (everyone leaving), at work or at a dialog: it ends here,
    /// with no answer.
    pub fn cut(&mut self) {
        if self.state() == State::Busy || self.open() {
            self.running = false;
            self.set(None, None);
            self.end(Ended::Interrupted);
        }
    }

    /// It left mid-turn, at work or at a dialog, as its hooks tell it: the turn ends here, with
    /// no answer. Not the registry's word: a snapshot that lags a Stop would make up a turn.
    pub fn left(&mut self) {
        if self.running || matches!(self.pending, Some(Pending::Awaits | Pending::Asked)) {
            self.running = false;
            self.set(None, None);
            self.end(Ended::Interrupted);
        }
    }

    /// What happened since the last call.
    pub fn take(&mut self) -> News {
        std::mem::take(&mut self.news)
    }

    fn end(&mut self, how: Ended) {
        self.news.ends += 1;
        self.news.ended = Some(how);
    }

    /// Typed to by gensokyo (a spell card, a ritual): whatever it waited to be told, it was.
    pub fn clear(&mut self) {
        self.set(None, None);
    }

    pub fn state(&self) -> State {
        if self.pending == Some(Pending::Asked) {
            return State::Asked;
        }
        let fresh = self.registry.filter(|(_, at)| *at > self.hook_at).map(|(s, _)| s);
        let busy = match fresh {
            Some(Registry::Waiting) => return State::Awaits,
            Some(r) => r == Registry::Busy,
            None => self.running,
        };
        match self.pending {
            _ if busy => State::Busy,
            Some(_) => State::Awaits,
            None => State::Resting,
        }
    }

    /// What a notification says about `name` in its present state.
    pub fn notice(&self, name: &str) -> String {
        let detail = self.detail.as_deref();
        match (self.state(), self.pending) {
            (State::Asked, _) => format!("{name} asks: {}", detail.unwrap_or("a question")),
            (_, Some(Pending::Stopped)) => match detail {
                Some(d) => format!("{name} is done: {d}"),
                None => format!("{name} is done"),
            },
            _ => format!("{name} needs your permission"),
        }
    }

    fn set(&mut self, p: Option<Pending>, detail: Option<String>) {
        (self.pending, self.detail) = (p, detail);
    }
}
