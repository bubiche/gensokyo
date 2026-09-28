//! Rituals as files: a markdown file with a schedule in its frontmatter and the prompt as its
//! body. They ship in `share/rituals/`; the user's own in the config dir's `rituals/` shadow
//! those of the same file name. The file name is the ritual's name. What a ritual keeps between
//! runs lives in `<state>/rituals/<name>/`: its stamp, its journal, its notes, its runs.
//!
//! Nothing here fires anything; the daemon does, and the CLI edits files, both through this.

use crate::cron::Schedule;
use crate::daemon::store::write_atomic;
use crate::proto::{self, RitualInfo};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

const NAME_RULE: &str = "a letter or digit first, then letters, digits, . _ -";
const SCHEDULES: &str = "five cron fields, @hourly @daily @weekly @monthly, or \"every 30m\"";
const NEVER: &str = "never comes round (a date that does not exist, like 30 February)";

#[derive(Debug, Clone, PartialEq)]
pub struct Ritual {
    pub slug: String,
    pub path: PathBuf,
    /// From `share/rituals/`: an example, not the user's own file.
    pub shipped: bool,
    pub name: Option<String>,
    pub description: Option<String>,
    pub schedule: String,
    pub target: String,
    /// With `~` expanded.
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub allowed_tools: Vec<String>,
    pub mcp_config: Option<String>,
    // The words as written; `problem` says when one means nothing.
    pub keep: String,
    pub overlap: String,
    pub headless: String,
    pub catch_up: String,
    pub enabled: String,
    pub prompt: String,
    pub unknown: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A fresh resident per run.
    New,
    /// One session for the ritual, kept between fires.
    Persistent,
    /// A resident the user manages.
    Resident(String),
}

fn yes(w: &str) -> bool {
    ["true", "yes", "on", "1"].contains(&w.to_ascii_lowercase().as_str())
}

fn bool_word(w: &str) -> bool {
    yes(w) || ["false", "no", "off", "0"].contains(&w.to_ascii_lowercase().as_str())
}

/// `30m`, `2h`, `1d`: Ok(None) for a keep that never runs out, Err for no length at all.
fn keep_len(w: &str) -> Result<Option<u64>, ()> {
    if ["forever", "until_banished"].contains(&w) {
        return Ok(None);
    }
    let Some((i, _)) = w.char_indices().last() else { return Err(()) };
    let (n, unit) = w.split_at(i);
    // Nine digits of days is millions of years: anything longer is a typo, and would overflow.
    if n.is_empty() || n.len() > 9 || !n.bytes().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    let n: u64 = n.parse().map_err(|_| ())?;
    let per = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(()),
    };
    Ok(Some(n * per))
}

impl Ritual {
    pub fn enabled(&self) -> bool {
        yes(&self.enabled)
    }

    pub fn headless(&self) -> bool {
        yes(&self.headless)
    }

    pub fn catch_up(&self) -> bool {
        yes(&self.catch_up)
    }

    /// How long a finished run stays; None for `forever`.
    pub fn keep_secs(&self) -> Option<u64> {
        keep_len(&self.keep).ok().flatten()
    }

    pub fn target(&self) -> Target {
        match self.target.as_str() {
            "new" => Target::New,
            "persistent" => Target::Persistent,
            name => Target::Resident(name.into()),
        }
    }
}

/// A letter or digit first, then letters, digits, `.` `_` `-`: a ritual's name is a file name.
pub fn name_ok(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphanumeric())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// The user's rituals, then the shipped ones.
pub fn dirs(share: Option<&Path>) -> Vec<PathBuf> {
    let mut d = vec![mine_dir()];
    d.extend(share.map(|s| s.join("rituals")));
    d
}

/// Where the user's own rituals live.
pub fn mine_dir() -> PathBuf {
    proto::config_dir().join("rituals")
}

/// Every ritual in `dirs`, the first dir's shadowing the later ones' by file name, sorted by
/// name; and the `.md` files left out because their names are not usable.
pub fn load(dirs: &[PathBuf], share: Option<&Path>) -> (Vec<Ritual>, Vec<PathBuf>) {
    let (mut out, mut unusable) = (Vec::<Ritual>::new(), Vec::new());
    for d in dirs {
        let shipped = share.is_some_and(|s| d.starts_with(s));
        let mut files: Vec<PathBuf> =
            std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).collect();
        files.sort();
        for f in files {
            let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let Some(slug) = name.strip_suffix(".md").filter(|_| !name.starts_with('.')) else {
                continue;
            };
            if !name_ok(slug) {
                unusable.push(f);
                continue;
            }
            if out.iter().any(|r| r.slug == slug) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&f) {
                out.push(parse(slug, &f, shipped, &text));
            }
        }
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    (out, unusable)
}

/// YAML enough for a file a person types: a quoted value ends at its closing quote, so
/// `schedule: "3 9 * * 1-5"   # weekdays` means the cron; an unquoted one loses a ` #` comment.
fn value(v: &str) -> String {
    let v = v.trim_start();
    let v = match v.chars().next() {
        Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or(""),
        Some('#') => "",
        _ => v.split(" #").next().unwrap_or(""),
    };
    v.trim().to_string()
}

/// `["a", "b"]` or a bare `a, b`, each item a value of its own. A pattern holding a comma goes
/// on a `- ` line of its own.
fn list(v: &str) -> Vec<String> {
    let v = v.trim();
    let v = match v.strip_prefix('[') {
        Some(inner) => inner.split(']').next().unwrap_or(""),
        None => v.split(" #").next().unwrap_or(""),
    };
    v.split(',').map(value).filter(|s| !s.is_empty()).collect()
}

fn home(p: &str) -> String {
    let h = std::env::var("HOME").unwrap_or_default();
    match p.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{h}{rest}"),
        _ => p.into(),
    }
}

/// The frontmatter is the `---` fenced block at the top, one `key: value` a line, read by hand:
/// nothing in a ritual can run. A file with no fence is all prompt, and has no schedule.
pub fn parse(slug: &str, path: &Path, shipped: bool, text: &str) -> Ritual {
    let mut r = Ritual {
        slug: slug.into(),
        path: path.into(),
        shipped,
        name: None,
        description: None,
        schedule: String::new(),
        target: "new".into(),
        cwd: None,
        model: None,
        effort: None,
        mode: None,
        allowed_tools: Vec::new(),
        mcp_config: None,
        keep: "2h".into(),
        overlap: "skip".into(),
        headless: "false".into(),
        catch_up: "true".into(),
        enabled: "true".into(),
        prompt: String::new(),
        unknown: Vec::new(),
    };
    let mut lines = text.lines().peekable();
    if lines.peek() == Some(&"---") {
        lines.next();
        let mut block = false;
        for line in lines.by_ref() {
            if line == "---" {
                break;
            }
            let l = line.trim_start();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            if block {
                if let Some(item) = l.strip_prefix('-') {
                    let item = value(item);
                    if !item.is_empty() {
                        r.allowed_tools.push(item);
                    }
                    continue;
                }
                block = false;
            }
            let Some((key, rest)) = l.split_once(':') else { continue };
            let key = key.trim();
            let v = value(rest);
            let some = |v: String| (!v.is_empty()).then_some(v);
            match key {
                "name" => r.name = some(v),
                "description" => r.description = some(v),
                "schedule" => r.schedule = v,
                "target" => r.target = v,
                "cwd" => r.cwd = some(home(&v)),
                "model" => r.model = some(v),
                "effort" => r.effort = some(v),
                "mode" | "permission_mode" => r.mode = some(v),
                "mcp_config" => r.mcp_config = some(v),
                "keep" => r.keep = v,
                "overlap" => r.overlap = v,
                "headless" => r.headless = v,
                "catch_up" => r.catch_up = v,
                "enabled" => r.enabled = v,
                // An empty value opens the block form: the `- ` lines that follow.
                "allowed_tools" | "allowedTools" if v.is_empty() => block = true,
                "allowed_tools" | "allowedTools" => r.allowed_tools = list(rest),
                // A typo as far as a reader can tell: a `schedul:` line never fires.
                _ => r.unknown.push(key.into()),
            }
        }
    }
    // Leading blank lines belong to the fence, not to the prompt.
    let body: Vec<&str> = lines.skip_while(|l| l.trim().is_empty()).collect();
    r.prompt = body.join("\n").trim_end().to_string();
    r
}

/// By whole name (any case), else a part of one that picks out a single ritual.
pub fn find<'a>(rs: &'a [Ritual], want: &str) -> Result<&'a Ritual, String> {
    let w = want.to_ascii_lowercase();
    if let Some(r) = rs.iter().find(|r| r.slug.to_ascii_lowercase() == w) {
        return Ok(r);
    }
    let hits: Vec<&Ritual> =
        rs.iter().filter(|r| r.slug.to_ascii_lowercase().contains(&w)).collect();
    match hits.as_slice() {
        [r] => Ok(r),
        [] => Err(format!("no ritual called '{want}' (gensokyo ritual lists them)")),
        many => {
            let names: Vec<&str> = many.iter().map(|r| r.slug.as_str()).collect();
            Err(format!("'{want}' could be {}: which one?", names.join(", ")))
        }
    }
}

/// Which directories Claude Code has been trusted in, from its `.claude.json`.
pub struct Trust {
    projects: Option<serde_json::Map<String, Value>>,
}

impl Trust {
    /// `$CLAUDE_CONFIG_DIR/.claude.json`, else `~/.claude.json`.
    pub fn path() -> PathBuf {
        std::env::var_os("CLAUDE_CONFIG_DIR")
            .or_else(|| std::env::var_os("HOME"))
            .map_or_else(PathBuf::new, PathBuf::from)
            .join(".claude.json")
    }

    /// Read once per use: it is large.
    pub fn load() -> Trust {
        let v = std::fs::read(Trust::path()).ok().and_then(|b| serde_json::from_slice(&b).ok());
        Trust::from_json(v.as_ref())
    }

    /// None: no file gensokyo can read, which answers nothing and so counts as trusted.
    pub fn from_json(v: Option<&Value>) -> Trust {
        let projects = v.map(|v| v.get("projects").and_then(Value::as_object).cloned());
        Trust { projects: projects.map(Option::unwrap_or_default) }
    }

    /// Accepted for `dir` or any directory above it, which Claude Code (2.1.283) counts too.
    /// The real path, which is where the run finds itself: `a/trusted/../b` is not in `trusted`.
    pub fn trusted(&self, dir: &Path) -> bool {
        let Some(p) = &self.projects else { return true };
        let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.into());
        real.ancestors().any(|a| {
            let v = p.get(a.to_string_lossy().as_ref());
            v.and_then(|v| v.get("hasTrustDialogAccepted")) == Some(&Value::Bool(true))
        })
    }
}

/// What is wrong with a ritual, as a sentence for the user, or nothing. A ritual with one bad
/// line does not fire, and nothing else would say why.
pub fn problem(r: &Ritual, now: i64, tz: &TimeZone, trust: &Trust) -> Option<String> {
    check(r, now, tz, trust).err()
}

/// `problem`, and the schedule when there is none.
fn check(r: &Ritual, now: i64, tz: &TimeZone, trust: &Trust) -> Result<Schedule, String> {
    let p = |s: String| Err(s);
    if !name_ok(&r.slug) {
        return p(format!("the file name is not a usable ritual name ({NAME_RULE})"));
    }
    if let Some(n) = r.name.as_ref().filter(|n| **n != r.slug) {
        return p(format!(
            "name: {n} is not the file's name ({}.md is the ritual, and the file name wins)",
            r.slug
        ));
    }
    // Before the rest: a misspelt key is why whatever it meant to set looks missing.
    if !r.unknown.is_empty() {
        return p(format!("not a ritual setting: {}", r.unknown.join(", ")));
    }
    if r.schedule.is_empty() {
        return p(format!("schedule: missing ({SCHEDULES})"));
    }
    let s = Schedule::parse(&r.schedule).map_err(|e| format!("schedule: {e}"))?;
    if s.next(now, tz).is_none() {
        return p(format!("schedule: {} {NEVER}", r.schedule));
    }
    if r.prompt.is_empty() {
        return p(
            "no prompt: the lines under the frontmatter are what the resident is asked to do"
                .into(),
        );
    }
    let own = matches!(r.target(), Target::New | Target::Persistent);
    // A slot is handed out afresh by every start, so a ritual cannot name one.
    if !own && !r.target.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return p(format!(
            "target: {} is neither new, persistent, nor a resident's name (a name starts with a \
             letter; a slot number is given out afresh, so a ritual cannot name one)",
            r.target
        ));
    }
    if r.headless() && r.target() != Target::New {
        return p(format!(
            "headless: true is a claude -p of its own, which is always a fresh session, so it \
             goes with target: new and not with {}",
            r.target
        ));
    }
    // Only the targets that start a session have a directory: a prompt sent to a resident
    // lands wherever that resident was started.
    if own {
        let Some(cwd) = &r.cwd else {
            return p("cwd: missing (the directory the run works in)".into());
        };
        if !cwd.starts_with('/') {
            return p(format!(
                "cwd: {cwd} is not a full path (a run starts from the daemon, which is in no \
                 directory of yours: ~/... or /...)"
            ));
        }
        if !Path::new(cwd).is_dir() {
            return p(format!("cwd: {} is not a directory", tilde(cwd)));
        }
        // A run stalled at the trust dialog is alive, so every later fire would skip itself
        // as still going. `claude -p` never shows the dialog.
        if !r.headless() && !trust.trusted(Path::new(cwd)) {
            return p(format!(
                "cwd: nothing has answered Claude Code's trust prompt for {} (open Claude Code \
                 there once and accept; until then it does not fire)",
                tilde(cwd)
            ));
        }
    }
    if !["skip", "queue", "parallel"].contains(&r.overlap.as_str()) {
        return p(format!(
            "overlap: {} is not a thing to do about a run that is still going (skip, queue, \
             parallel)",
            r.overlap
        ));
    }
    if r.target() != Target::New && r.overlap != "skip" {
        return p(format!(
            "overlap: {} is only about a target of new, which is the target that starts a run of \
             its own (a prompt sent to a resident that is already there is queued by Claude Code \
             itself)",
            r.overlap
        ));
    }
    if keep_len(&r.keep).is_err() {
        return p(format!("keep: {} is not a length (30m, 2h, 1d, forever)", r.keep));
    }
    for (k, w) in [("enabled", &r.enabled), ("headless", &r.headless), ("catch_up", &r.catch_up)] {
        if !bool_word(w) {
            return p(format!("{k}: {w} is neither true nor false"));
        }
    }
    // The list is variadic on claude's command line, so an item there reads as a flag.
    if let Some(t) = r.allowed_tools.iter().find(|t| t.starts_with('-')) {
        return p(format!("allowed_tools: {t} starts with -, which claude would read as a flag"));
    }
    Ok(s)
}

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
        Dir::at(&proto::state_dir(), slug)
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

    /// The newest `keep` runs, each a name like `<time>.<pid>` with its files beside it.
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
        let drop = &runs[..runs.len().saturating_sub(keep)];
        for f in files.iter().filter(|p| run(p).is_some_and(|r| drop.contains(&r))) {
            let _ = std::fs::remove_file(f);
        }
    }
}

/// The ritual's prompt, and for a run of its own the sentence naming its notes: a fresh
/// session forgets everything, and the file is where the continuity lives. A resident the user
/// named gets the prompt alone: it was summoned by hand, and the file is outside its reach.
pub fn prompt_text(r: &Ritual, memory: &Path) -> String {
    match r.target() {
        Target::Resident(_) => r.prompt.clone(),
        _ => format!(
            "{}\n\nYour notes from previous runs are at `{}`. Read them first; update them before \
             you finish.",
            r.prompt,
            memory.display()
        ),
    }
}

/// The run's own flags. `--add-dir` is always the ritual's directory, so the notes read without
/// a permission prompt. `--allowedTools` is variadic and last: a flag or `--` must follow.
pub fn args(r: &Ritual, dir: &Path) -> Vec<String> {
    let mut a = Vec::new();
    let mcp = r.mcp_config.as_deref().map(home);
    let flags = [
        ("--model", r.model.clone()),
        ("--effort", r.effort.clone()),
        ("--permission-mode", r.mode.clone()),
        ("--mcp-config", mcp),
    ];
    for (f, v) in flags {
        if let Some(v) = v {
            a.extend([f.to_string(), v]);
        }
    }
    a.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
    if !r.allowed_tools.is_empty() {
        a.push("--allowedTools".into());
        a.extend(r.allowed_tools.iter().cloned());
    }
    a
}

/// A minute on this machine's clock, as the journal and the listings write it.
pub fn when(t: i64, tz: &TimeZone) -> String {
    match Timestamp::from_second(t) {
        Ok(ts) => ts.to_zoned(tz.clone()).strftime("%Y-%m-%d %H:%M").to_string(),
        Err(_) => t.to_string(),
    }
}

/// `$HOME/x` as `~/x`.
pub fn tilde(p: &str) -> String {
    crate::tele::tilde(p, &std::env::var("HOME").unwrap_or_default())
}

/// The ritual as the timetable shows it. The next fire only for one that will fire.
pub fn info(r: &Ritual, now: i64, tz: &TimeZone, trust: &Trust, dir: &Dir) -> RitualInfo {
    let checked = check(r, now, tz, trust);
    let next = checked.as_ref().ok().filter(|_| r.enabled()).and_then(|s| s.next(now, tz));
    let problem = checked.err();
    let last = dir.journal(1).pop().map(|e| format!("{}  {}", when(e.at, tz), e.text));
    RitualInfo {
        name: r.slug.clone(),
        enabled: r.enabled(),
        schedule: r.schedule.clone(),
        next_fire: next,
        next_fire_local: next.map(|t| when(t, tz)),
        last_run: dir.last_run(),
        last,
        target: r.target.clone(),
        headless: r.headless(),
        keep: r.keep.clone(),
        overlap: r.overlap.clone(),
        cwd: r.cwd.clone(),
        description: r.description.clone(),
        problem,
        path: r.path.to_string_lossy().into_owned(),
        shipped: r.shipped,
        running: false,
    }
}

/// What `ritual add` takes. `cwd` is absolute: the caller resolves it against its own directory.
#[derive(Debug, Clone, Default)]
pub struct Add {
    pub name: String,
    pub schedule: String,
    pub cwd: String,
    pub description: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub mcp_config: Option<String>,
    pub target: Option<String>,
    pub keep: Option<String>,
    pub overlap: Option<String>,
    pub headless: bool,
    pub catch_up: Option<String>,
    pub prompt: String,
    pub disabled: bool,
    pub allowed_tools: Vec<String>,
}

/// A value in the frontmatter, quoted so a `#` or a `:` in it stays in it.
fn quote(v: &str) -> Result<String, String> {
    if v.contains('\n') {
        return Err(format!("a value is one line: {v:?}"));
    }
    match (v.contains('"'), v.contains('\'')) {
        (true, true) => Err(format!("a value cannot hold both kinds of quote: {v}")),
        (true, false) => Ok(format!("'{v}'")),
        _ => Ok(format!("\"{v}\"")),
    }
}

/// A plain word: no quoting, and nothing that would read as something else.
fn word(k: &str, v: &str) -> Result<String, String> {
    if v.is_empty() || v.contains(|c: char| c.is_whitespace() || "#\"'".contains(c)) {
        return Err(format!("--{} {v:?} is not a single word", k.replace('_', "-")));
    }
    Ok(v.into())
}

/// Writes `<rituals>/<name>.md`, checked first as the daemon will read it: anything wrong
/// writes nothing, except a cwd Claude Code has not been trusted in, which is written with the
/// warning (the user can answer that dialog). Returns the file and what to say: lines starting
/// `warning: ` are warnings.
pub fn add(
    a: &Add,
    now: i64,
    tz: &TimeZone,
    trust: &Trust,
    rituals: &Path,
    share: Option<&Path>,
) -> Result<(PathBuf, Vec<String>), String> {
    let name = &a.name;
    if name.is_empty() {
        return Err("--name is the ritual's name, and the name of its file".into());
    }
    if !name_ok(name) {
        return Err(format!("'{name}' cannot be a ritual name ({NAME_RULE})"));
    }
    if a.prompt.trim().is_empty() {
        return Err("--prompt-file <file> (or --prompt \"one line\", or --prompt-file - for \
                    stdin) is what the run is asked to do"
            .into());
    }
    if a.schedule.is_empty() {
        return Err(format!("--schedule \"3 9 * * 1-5\" ({SCHEDULES})"));
    }
    // `keep` is how long a run of its own stays; a resident that is already there, or a run
    // with no pane, has none.
    if a.keep.is_some() && (a.headless || a.target.as_deref().is_some_and(|t| t != "new")) {
        return Err("--keep is only for a run of its own: not with --target or --headless".into());
    }
    let cwd = std::fs::canonicalize(&a.cwd)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| a.cwd.clone());
    let path = rituals.join(format!("{name}.md"));
    if path.exists() {
        return Err(format!(
            "{name} is already there: {} (edit that file, or gensokyo ritual edit {name})",
            tilde(&path.to_string_lossy())
        ));
    }
    let mut f = format!("---\nname: {name}\n");
    if let Some(d) = &a.description {
        f += &format!("description: {}\n", quote(d)?);
    }
    f += &format!("schedule: {}\n", quote(&a.schedule)?);
    if let Some(t) = &a.target {
        f += &format!("target: {}\n", word("target", t)?);
    }
    f += &format!("cwd: {}\n", quote(&cwd)?);
    for (k, v) in [("model", &a.model), ("effort", &a.effort), ("mode", &a.mode)] {
        if let Some(v) = v {
            f += &format!("{k}: {}\n", word(k, v)?);
        }
    }
    if !a.allowed_tools.is_empty() {
        f += "allowed_tools:\n";
        for t in &a.allowed_tools {
            f += &format!("  - {}\n", quote(t)?);
        }
    }
    if let Some(m) = &a.mcp_config {
        f += &format!("mcp_config: {}\n", quote(m)?);
    }
    for (k, v) in [("keep", &a.keep), ("overlap", &a.overlap), ("catch_up", &a.catch_up)] {
        if let Some(v) = v {
            f += &format!("{k}: {}\n", word(k, v)?);
        }
    }
    if a.headless {
        f += "headless: true\n";
    }
    if a.disabled {
        f += "enabled: false\n";
    }
    f += &format!("---\n{}\n", a.prompt.trim_end());
    let r = parse(name, &path, false, &f);
    // Every check but trust, which is the one the user can fix after the fact.
    let s = check(&r, now, tz, &Trust::from_json(None))
        .map_err(|p| format!("{p}\nnothing was written"))?;
    let own = matches!(r.target(), Target::New | Target::Persistent);
    let untrusted = own && !r.headless() && !trust.trusted(Path::new(&cwd));
    std::fs::create_dir_all(rituals).map_err(|e| format!("{}: {e}", rituals.display()))?;
    write_atomic(&path, f.as_bytes())
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    let mut out = vec![format!("wrote {}", tilde(&path.to_string_lossy()))];
    if share.is_some_and(|s| s.join("rituals").join(format!("{name}.md")).exists()) {
        out.push(format!(
            "warning: {name} is also one of the shipped examples; yours shadows it from now on"
        ));
    }
    if untrusted {
        out.push(format!("warning: {}", check(&r, now, tz, trust).err().unwrap_or_default()));
        out.push("  not firing until that is fixed".into());
    } else if !r.enabled() {
        out.push(format!("  disabled, so it will not fire: gensokyo ritual enable {name}"));
    } else if let Some(next) = s.next(now, tz) {
        let secs = (next - now).max(0) as u64;
        out.push(format!("  next fire  {} (in {})", when(next, tz), crate::tele::age(secs)));
    }
    let clock = Timestamp::from_second(now)
        .map(|t| t.to_zoned(tz.clone()).strftime("%H:%M %z").to_string())
        .unwrap_or_default();
    out.push(format!(
        "  {} is this machine's clock, now {clock} - a ritual is never in UTC",
        a.schedule
    ));
    if r.headless() {
        out.push(format!(
            "  headless: no pane to watch and nobody to answer a prompt, so what it needs goes in \
             allowed_tools; what each run says keeps in {}",
            tilde(&Dir::of(name).runs().to_string_lossy())
        ));
    }
    out.push(format!(
        "  gensokyo ritual run {name}   fires it now, so its prompts can be approved once"
    ));
    Ok((path, out))
}

/// The user's own copy of a ritual: a shipped one is copied into `rituals` first, so an
/// update cannot overwrite the user's version.
pub fn mine(r: &Ritual, rituals: &Path) -> Result<PathBuf, String> {
    if !r.shipped {
        return Ok(r.path.clone());
    }
    let dst = rituals.join(format!("{}.md", r.slug));
    if !dst.exists() {
        std::fs::create_dir_all(rituals)
            .and_then(|_| std::fs::copy(&r.path, &dst).map(|_| ()))
            .map_err(|e| format!("could not copy {} into {}: {e}", r.slug, rituals.display()))?;
    }
    Ok(dst)
}

/// `enabled: true|false` in the frontmatter, replacing the line that is there (and any repeat
/// of it) or added at the end of the block; the rest of the file as it was.
pub fn set_enabled(path: &Path, on: bool) -> Result<(), String> {
    let shown = tilde(&path.to_string_lossy());
    // Through a symlink (a dotfiles repo, say), not over it.
    let path = &std::fs::canonicalize(path).map_err(|e| format!("{shown}: {e}"))?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{shown}: {e}"))?;
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.first() != Some(&"---") {
        return Err(format!(
            "{shown} has no frontmatter, so there is no schedule to turn on or off"
        ));
    }
    let end = lines.iter().skip(1).position(|l| *l == "---").map(|i| i + 1);
    let Some(end) = end else {
        return Err(format!("{shown} has no end to its frontmatter (a --- line)"));
    };
    let line = format!("enabled: {on}");
    let is_key =
        |l: &str| l.trim_start().split_once(':').is_some_and(|(k, _)| k.trim() == "enabled");
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    for (i, l) in lines.drain(..).enumerate() {
        if i > 0 && i < end && is_key(l) {
            if !done {
                out.push(line.clone());
                done = true;
            }
            continue;
        }
        if i == end && !done {
            out.push(line.clone());
            done = true;
        }
        out.push(l.to_string());
    }
    let mut body = out.join("\n");
    if text.ends_with('\n') {
        body.push('\n');
    }
    write_atomic(path, body.as_bytes()).map_err(|e| format!("could not write {shown}: {e}"))
}

/// On or off, the user's own copy of a shipped one first. Turned on, it is stamped with this
/// minute: the fires it missed while off were not missed, and must not be caught up.
pub fn toggle(r: &Ritual, on: bool, now: i64, rituals: &Path) -> Result<Vec<String>, String> {
    let path = mine(r, rituals)?;
    let mut out = Vec::new();
    if path != r.path {
        out.push(format!(
            "the shipped {}.md is now yours, in {}",
            r.slug,
            tilde(&path.to_string_lossy())
        ));
    }
    set_enabled(&path, on)?;
    if on {
        let _ = Dir::of(&r.slug).set_stamp(now - now.rem_euclid(60));
    }
    out.push(format!("{} is {}", r.slug, if on { "enabled" } else { "disabled" }));
    Ok(out)
}

/// What `ritual new` opens in the editor. It arrives disabled.
pub fn template(name: &str, cwd: &str) -> String {
    format!(
        "---\n\
         name: {name}\n\
         description: what this ritual is for, in one line\n\
         schedule: \"0 9 * * 1-5\"     # five cron fields, local time; also @hourly @daily @weekly, \"every 30m\"\n\
         cwd: \"{cwd}\"                   # the directory the run works in\n\
         enabled: false              # true once you are happy with it\n\
         ---\n\
         What the resident should do, written for a session that has never seen this job\n\
         before: it starts fresh every time. It is told where its notes from previous\n\
         runs are - it reads them first, and updates them before it finishes.\n"
    )
}

/// The file, its notes and its journal: gone. Never a shipped example, which an update would
/// put back.
pub fn remove(r: &Ritual, dir: &Dir, share: Option<&Path>) -> Result<Vec<String>, String> {
    if r.shipped {
        return Err(format!(
            "{0} is one of the examples gensokyo ships, and an update would put it back: \
             'gensokyo ritual disable {0}' is how you stop it firing",
            r.slug
        ));
    }
    let shown = tilde(&r.path.to_string_lossy());
    std::fs::remove_file(&r.path).map_err(|e| format!("could not delete {shown}: {e}"))?;
    let mut out = vec![format!("{} is gone, and {shown} with it", r.slug)];
    if dir.path.is_dir() {
        std::fs::remove_dir_all(&dir.path)
            .map_err(|e| format!("could not delete {}: {e}", dir.path.display()))?;
        out.push(format!(
            "  its notes and its journal went too, from {}",
            tilde(&dir.path.to_string_lossy())
        ));
    }
    if share.is_some_and(|s| s.join("rituals").join(format!("{}.md", r.slug)).exists()) {
        out.push("  the example gensokyo ships with that name is in the listing again".into());
    }
    Ok(out)
}
