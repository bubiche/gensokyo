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
    /// When `session` was last set by a hook, epoch ms: an older SessionStart never undoes it.
    #[serde(default)]
    pub session_at: i64,
    pub name: String,
    pub slot: Option<u8>,
    pub cwd: String,
    pub program: String,
    /// The full launch argv after the program, `--settings` json included.
    pub argv: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ritual: Option<String>,
    pub launched: i64,
    pub departed: Option<i64>,
    pub exit: Option<i32>,
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

    /// A record that stays in `departed/`, changed.
    pub fn save_departed(&self, r: &Record) -> std::io::Result<()> {
        write_atomic(&self.departed().join(format!("{}.json", r.id)), &to_json(r))
    }

    /// Back in the shrine: recalled.
    pub fn restore(&self, r: &Record) -> std::io::Result<()> {
        self.save(r)?;
        remove(&self.departed().join(format!("{}.json", r.id)))
    }

    /// Every record in `residents/`, or why one could not be read.
    pub fn load(&self) -> Vec<Result<Record, String>> {
        load(&self.residents())
    }

    /// One record in `departed/`, by id.
    pub fn load_departed_id(&self, id: &str) -> Option<Record> {
        let b = std::fs::read(self.departed().join(format!("{id}.json"))).ok()?;
        serde_json::from_slice(&b).ok()
    }

    /// The records in `departed/` that can be read.
    pub fn load_departed(&self) -> Vec<Record> {
        load(&self.departed()).into_iter().flatten().collect()
    }
}

fn load(dir: &Path) -> Vec<Result<Record, String>> {
    let read = |p: &Path| {
        let b = std::fs::read(p).map_err(|e| e.to_string())?;
        serde_json::from_slice(&b).map_err(|e| e.to_string())
    };
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| read(&p).map_err(|e| format!("{}: {e}", p.display())))
        .collect()
}

fn to_json(r: &Record) -> Vec<u8> {
    let mut b = serde_json::to_vec_pretty(r).expect("a record serializes");
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
    std::fs::rename(&tmp, path)?;
    // The rename is durable only once the directory is.
    std::fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()
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
