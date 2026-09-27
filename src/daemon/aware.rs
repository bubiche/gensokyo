//! What a resident is doing, from two sources: its hooks, which say what it waits for, and
//! Claude Code's registry (`claude agents --json`), which says whether it is busy or has a
//! dialog open. Nothing here looks at the screen.
//!
//! Precedence: a question, then a dialog the registry sees, then busy, then a flag the hooks
//! raised (a permission prompt, a finished turn), then resting. The registry counts only when
//! its snapshot began after the newest hook: one taken before a `Stop` knows nothing of it.

use crate::proto::{Hook, State};
use crate::tele;

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
    /// The newest hook applied, epoch ms.
    hook_at: i64,
    /// The last snapshot and when it began.
    registry: Option<(Registry, i64)>,
    /// The permission mode, from UserPromptSubmit and Stop.
    pub mode: Option<String>,
    /// The flag is a dialog the registry shows as `waiting` while it is open (a permission
    /// prompt, a question), so an idle snapshot means it was answered.
    shown: bool,
}

impl Aware {
    /// Applies one hook; false, and nothing changes, for one older than the newest applied.
    pub fn hook(&mut self, h: &Hook) -> bool {
        if h.at < self.hook_at {
            return false;
        }
        self.hook_at = h.at;
        if let Some(m) = &h.mode {
            self.mode = Some(tele::clean(m, 20));
        }
        let text = h.text.as_deref().map(|t| tele::clean(t, 80)).filter(|t| !t.is_empty());
        let ask = h.tool.as_deref() == Some("AskUserQuestion");
        match (h.event.as_str(), h.kind.as_deref()) {
            ("UserPromptSubmit", _) => {
                self.set(None, None);
                self.running = true;
            }
            ("Stop", _) => {
                self.set(Some(Pending::Stopped), text);
                self.running = false;
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
                }
                self.running = false;
            }
            // Sent a minute after a turn: a late stand-in for a Stop that went missing. Not
            // after one that came, or a dialog dismissed: that turn has already been told of.
            ("Notification", Some("idle_prompt")) => {
                if self.pending.is_none() && self.running {
                    self.set(Some(Pending::Stopped), None);
                }
                self.running = false;
            }
            ("PreToolUse", _) if ask => {
                self.set(Some(Pending::Asked), text);
                self.shown = true;
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
                self.set(None, None)
            }
            _ => {}
        }
    }

    /// Why nothing may be typed into it now, or nothing. A dialog open would take the Enter
    /// after a card and answer it. So would one the registry cannot see yet: a session it does
    /// not list may be on the workspace trust dialog, where no hook fires either. A finished
    /// turn is not in the way: typing into that is what a card is for.
    pub fn blocked(&self) -> Option<&'static str> {
        match (self.pending, self.registry) {
            (Some(Pending::Asked), _) => Some("is asking you a question"),
            (_, None) => Some("is still starting up"),
            (Some(Pending::Awaits), _) | (_, Some((Registry::Waiting, _))) => {
                Some("has a dialog waiting for you")
            }
            _ => None,
        }
    }

    /// In the last registry snapshot: other sessions can message it.
    pub fn listed(&self) -> bool {
        self.registry.is_some()
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
