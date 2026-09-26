//! The wire protocol: one JSON object per line over a unix socket, both ways. A client says
//! `hello` first and gets `welcome`; every other request carries an `id` its reply echoes.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    List,
    Summon(Summon),
    /// HUP, the grace, TERM, KILL. The resident stays in the shrine as departed.
    Banish {
        who: String,
    },
    /// A live resident is asked to `/exit`; a departed one leaves the shrine.
    Close {
        who: String,
    },
    /// Everyone is asked to `/exit`, then the daemon stops.
    Quit,
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
    Welcome { proto: u32, pid: u32 },
    List { id: u64, residents: Vec<Resident> },
    Summoned { id: u64, resident: Resident },
    Done { id: u64, message: String },
    Error { id: u64, error: String },
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
