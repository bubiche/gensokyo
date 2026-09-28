//! What a ritual keeps between runs, in `<state>/rituals/<name>/`: the minute it last fired
//! for, a persistent ritual's session, its notes, its journal and its headless runs' logs.

use crate::daemon::store::write_atomic;
use crate::paths;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// About a thousand lines.
const JOURNAL_MAX: u64 = 150_000;

/// One line of a ritual's journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub at: i64,
    /// ran, sent, skipped, queued, dropped, not-run, not-sent, done, failed, closed, kept.
    pub ev: String,
    pub text: String,
}

/// `<state>/rituals/<name>/`: what a ritual keeps between runs.
pub struct Dir {
    pub path: PathBuf,
}

impl Dir {
    pub fn of(slug: &str) -> Dir {
        Dir::at(&paths::state_dir(), slug)
    }

    pub fn at(state: &Path, slug: &str) -> Dir {
        Dir { path: state.join("rituals").join(slug) }
    }

    fn read(&self, f: &str) -> Option<String> {
        let s = std::fs::read_to_string(self.path.join(f)).ok()?;
        Some(s.trim().to_string()).filter(|s| !s.is_empty())
    }

    fn write(&self, f: &str, s: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.path)?;
        write_atomic(&self.path.join(f), format!("{s}\n").as_bytes())
    }

    /// The minute it last fired for, or was first seen in.
    pub fn stamp(&self) -> Option<i64> {
        self.read("stamp")?.parse().ok()
    }

    pub fn set_stamp(&self, t: i64) -> std::io::Result<()> {
        self.write("stamp", &t.to_string())
    }

    /// A persistent ritual's resident, by record id.
    pub fn session(&self) -> Option<String> {
        self.read("session")
    }

    pub fn set_session(&self, id: &str) -> std::io::Result<()> {
        self.write("session", id)
    }

    /// The notes file every prompt names, made if it is not there: a run should find a file.
    pub fn memory(&self) -> PathBuf {
        let f = self.path.join("memory.md");
        if !f.exists() {
            let slug = self.path.file_name().unwrap_or_default().to_string_lossy();
            let _ = std::fs::create_dir_all(&self.path);
            let _ =
                std::fs::write(&f, format!("# {slug}\n\nNotes kept across runs of this ritual.\n"));
        }
        f
    }

    /// One line in the journal, written whole with O_APPEND. Past `JOURNAL_MAX` bytes it is cut
    /// to its newer half: an `every 1m` ritual skipping itself would otherwise grow it for ever.
    pub fn note(&self, at: i64, ev: &str, text: &str) {
        let e = Entry { at, ev: ev.into(), text: text.into() };
        let mut line = serde_json::to_vec(&e).expect("an entry serializes");
        line.push(b'\n');
        let _ = std::fs::create_dir_all(&self.path);
        let file = self.path.join("journal.jsonl");
        let f = std::fs::OpenOptions::new().create(true).append(true).open(&file);
        if let Ok(mut f) = f {
            let _ = f.write_all(&line);
        }
        if std::fs::metadata(&file).is_ok_and(|m| m.len() > JOURNAL_MAX) {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            let keep = lines[lines.len() / 2..].join("\n") + "\n";
            let _ = write_atomic(&file, keep.as_bytes());
        }
    }

    /// The last `n` entries, oldest first; 0 for all of them.
    pub fn journal(&self, n: usize) -> Vec<Entry> {
        let text = std::fs::read_to_string(self.path.join("journal.jsonl")).unwrap_or_default();
        let all: Vec<Entry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let skip = if n == 0 { 0 } else { all.len().saturating_sub(n) };
        all.into_iter().skip(skip).collect()
    }

    /// When a run last started or a prompt was last sent. Not the stamp: first sight writes one.
    pub fn last_run(&self) -> Option<i64> {
        self.journal(0).iter().rev().find(|e| e.ev == "ran" || e.ev == "sent").map(|e| e.at)
    }

    /// Headless runs' logs.
    pub fn runs(&self) -> PathBuf {
        self.path.join("runs")
    }

    /// The newest `keep` runs, each a name like `<time>.<pid>` with its files beside it, and
    /// every run still going (its `.pid` is there), however old.
    pub fn trim_runs(&self, keep: usize) {
        let files: Vec<PathBuf> = std::fs::read_dir(self.runs())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        let run = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().into_owned());
        let mut runs: Vec<String> = files.iter().filter_map(|p| run(p)).collect();
        runs.sort();
        runs.dedup();
        let going = |r: &String| {
            files
                .iter()
                .any(|p| run(p).as_ref() == Some(r) && p.extension().is_some_and(|x| x == "pid"))
        };
        let drop: Vec<&String> =
            runs[..runs.len().saturating_sub(keep)].iter().filter(|r| !going(r)).collect();
        for f in files.iter().filter(|p| run(p).is_some_and(|r| drop.contains(&&r))) {
            let _ = std::fs::remove_file(f);
        }
    }
}
