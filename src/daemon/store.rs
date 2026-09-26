//! Records on disk: `residents/<id>.json` for everyone in the shrine, `departed/<id>.json` for
//! those who left it. Every write is a whole file, renamed into place.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// The resident's id and its first session id.
    pub id: String,
    /// The current session id; `/clear` and `/compact` rotate it.
    pub session: String,
    pub name: String,
    pub slot: Option<u8>,
    pub cwd: String,
    pub program: String,
    /// The full launch argv after the program, `--settings` json included.
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ritual: Option<String>,
    pub launched: i64,
    #[serde(default)]
    pub departed: Option<i64>,
    #[serde(default)]
    pub exit: Option<i32>,
    #[serde(default)]
    pub signal: Option<i32>,
}

pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn residents(&self) -> PathBuf {
        self.root.join("residents")
    }

    pub fn departed(&self) -> PathBuf {
        self.root.join("departed")
    }

    pub fn save(&self, r: &Record) -> std::io::Result<()> {
        write_atomic(&self.residents().join(format!("{}.json", r.id)), &to_json(r))
    }

    /// Out of the shrine, kept for recall.
    pub fn retire(&self, r: &Record) -> std::io::Result<()> {
        write_atomic(&self.departed().join(format!("{}.json", r.id)), &to_json(r))?;
        remove(&self.residents().join(format!("{}.json", r.id)))
    }

    /// Every record in `residents/`, oldest launch first; unreadable files are skipped.
    pub fn load(&self) -> Vec<Record> {
        let mut v: Vec<Record> = std::fs::read_dir(self.residents())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
            .collect();
        v.sort_by_key(|r| r.launched);
        v
    }
}

fn to_json(r: &Record) -> Vec<u8> {
    let mut b = serde_json::to_vec_pretty(r).unwrap_or_default();
    b.push(b'\n');
    b
}

fn remove(p: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// Write beside the target, then rename over it: a reader sees the old file or the new one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// A random (v4) UUID, which Claude Code takes as `--session-id`.
pub fn uuid() -> String {
    let mut b = [0u8; 16];
    // SAFETY: arc4random_buf fills exactly the given length.
    unsafe { libc::arc4random_buf(b.as_mut_ptr().cast(), b.len()) };
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

pub fn random(n: usize) -> usize {
    // SAFETY: plain libc call.
    (unsafe { libc::arc4random_uniform(n.max(1) as u32) }) as usize
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
