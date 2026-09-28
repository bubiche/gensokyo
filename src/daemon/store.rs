//! Records on disk: `residents/<id>.json` for everyone in the shrine, `departed/<id>.json` for
//! those who left it. Every write is a whole file, renamed into place, and a write that fails
//! is logged here, whoever asked for it.

use super::server::log;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Run records kept in `departed/` per ritual, newest first: one that fires every few minutes
/// leaves one a run.
pub const RUNS_KEPT: usize = 50;

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
    /// A ritual run: seconds idle before it is asked to leave. None stays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<u64>,
    /// A ritual run's own flags (`--add-dir`, `--allowedTools` …), kept for a recall.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<String>,
    pub launched: i64,
    pub departed: Option<i64>,
    pub exit: Option<i32>,
    pub signal: Option<i32>,
}

pub struct Store {
    pub root: PathBuf,
    /// `departed/`, read once and kept current here: every `list --all` and recall wants all
    /// of it, on the daemon's one thread.
    gone: RefCell<Option<HashMap<String, Record>>>,
}

impl Store {
    pub fn new(root: PathBuf) -> Store {
        Store { root, gone: RefCell::new(None) }
    }

    pub fn residents(&self) -> PathBuf {
        self.root.join("residents")
    }

    pub fn departed(&self) -> PathBuf {
        self.root.join("departed")
    }

    fn gone(&self) -> RefMut<'_, HashMap<String, Record>> {
        RefMut::map(self.gone.borrow_mut(), |g| {
            g.get_or_insert_with(|| {
                let all = load(&self.departed()).into_iter().flatten();
                all.map(|r| (r.id.clone(), r)).collect()
            })
        })
    }

    pub fn save(&self, r: &Record) -> std::io::Result<()> {
        logged(
            "save",
            r,
            write_atomic(&self.residents().join(format!("{}.json", r.id)), &to_json(r)),
        )
    }

    /// Out of the shrine, kept for recall. A ritual's run beyond its newest `RUNS_KEPT` goes.
    pub fn retire(&self, r: &Record) -> std::io::Result<()> {
        let w = write_atomic(&self.departed().join(format!("{}.json", r.id)), &to_json(r))
            .and_then(|()| remove(&self.residents().join(format!("{}.json", r.id))));
        self.gone().insert(r.id.clone(), r.clone());
        if let Some(slug) = &r.ritual {
            self.trim(slug);
        }
        logged("retire", r, w)
    }

    /// A record that stays in `departed/`, changed.
    pub fn save_departed(&self, r: &Record) -> std::io::Result<()> {
        let w = write_atomic(&self.departed().join(format!("{}.json", r.id)), &to_json(r));
        self.gone().insert(r.id.clone(), r.clone());
        logged("save departed", r, w)
    }

    /// Back in the shrine: recalled.
    pub fn restore(&self, r: &Record) -> std::io::Result<()> {
        let w = write_atomic(&self.residents().join(format!("{}.json", r.id)), &to_json(r))
            .and_then(|()| remove(&self.departed().join(format!("{}.json", r.id))));
        self.gone().remove(&r.id);
        logged("restore", r, w)
    }

    /// Every record in `residents/`, or why one could not be read.
    pub fn load(&self) -> Vec<Result<Record, String>> {
        load(&self.residents())
    }

    /// One departed record, by id.
    pub fn load_departed_id(&self, id: &str) -> Option<Record> {
        self.gone().get(id).cloned()
    }

    /// Every departed record that could be read.
    pub fn load_departed(&self) -> Vec<Record> {
        self.gone().values().cloned().collect()
    }

    /// The oldest runs of ritual `slug` past `RUNS_KEPT`, gone. Never the session a persistent
    /// ritual keeps, however old.
    fn trim(&self, slug: &str) {
        let kept = crate::ritual::Dir::at(&self.root, slug).session();
        let mut gone = self.gone();
        let mut runs: Vec<(Option<i64>, i64, String)> = gone
            .values()
            .filter(|r| r.ritual.as_deref() == Some(slug) && Some(&r.id) != kept.as_ref())
            .map(|r| (r.departed, r.launched, r.id.clone()))
            .collect();
        runs.sort_by(|a, b| b.cmp(a));
        for (.., id) in runs.into_iter().skip(RUNS_KEPT) {
            gone.remove(&id);
            if let Err(e) = remove(&self.departed().join(format!("{id}.json"))) {
                log(json!({"ev": "record", "id": id, "op": "trim", "error": e.to_string()}));
            }
        }
    }
}

/// A record write's result, logged when it failed: a stale record resumes the wrong session.
fn logged(op: &str, r: &Record, w: std::io::Result<()>) -> std::io::Result<()> {
    if let Err(e) = &w {
        log(json!({"ev": "record", "id": r.id, "op": op, "error": e.to_string()}));
    }
    w
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
