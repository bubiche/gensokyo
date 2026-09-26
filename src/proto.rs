//! The wire protocol: one JSON object per line over a unix socket, both ways. A client says
//! `hello` first and gets `welcome`; every other request carries an `id` its reply echoes.
//! `watch`, `view`, `unview`, `input` and `resize` are answered only when they fail. Events
//! carry no `id` and go only to connections that asked for them (`watch`, `view`).

use crate::vt::{Frame, Modes, Run};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const PROTO: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(default)]
    pub id: u64,
    #[serde(flatten)]
    pub req: Request,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Request {
    Hello {
        proto: u32,
        who: String,
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
    /// The size of the client's grid: every live resident and every later summon takes it.
    /// The last client to say wins.
    Resize {
        cols: u16,
        rows: u16,
    },
    /// Everyone is asked to `/exit`, then the daemon stops.
    Quit,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
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
    },
    Done {
        id: u64,
        message: String,
    },
    Error {
        id: u64,
        error: String,
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
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resident {
    pub id: String,
    pub name: String,
    pub slot: Option<u8>,
    pub cwd: String,
    /// The leader's pid while it runs.
    pub pid: Option<i32>,
    /// Epoch seconds.
    pub departed: Option<i64>,
    pub exit: Option<i32>,
    pub signal: Option<i32>,
}

/// `$GENSOKYO_STATE_DIR`, else `~/.local/state/gensokyo`.
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("GENSOKYO_STATE_DIR") {
        return d.into();
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/".into());
    PathBuf::from(home).join(".local/state/gensokyo")
}

/// `$GENSOKYO_SHARE`, else `share/` beside the binary or up to three levels above it (a build
/// tree).
pub fn share_dir(exe: &Path) -> Option<PathBuf> {
    if let Some(s) = std::env::var_os("GENSOKYO_SHARE") {
        return Some(s.into());
    }
    exe.ancestors().skip(1).take(4).map(|d| d.join("share")).find(|s| s.join("names.txt").is_file())
}

/// `$GENSOKYO_SOCKET`, else `run/gensokyo.sock` in the state dir, else, when that is past the
/// 104 bytes a socket path may hold, a name in `$TMPDIR` derived from the state dir.
pub fn socket_path() -> PathBuf {
    if let Some(s) = std::env::var_os("GENSOKYO_SOCKET") {
        return s.into();
    }
    let p = state_dir().join("run/gensokyo.sock");
    if p.as_os_str().len() < 104 {
        return p;
    }
    // FNV-1a: stable across builds, unlike std's hasher.
    let h = p
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    std::env::temp_dir().join(format!("gensokyo-{h:016x}.sock"))
}
