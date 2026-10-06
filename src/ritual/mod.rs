//! Rituals as files: a markdown file with a schedule in its frontmatter and the prompt as its
//! body. They ship in `share/rituals/`; the user's own in the config dir's `rituals/` shadow
//! those of the same file name. The file name is the ritual's name. What a ritual keeps between
//! runs lives in `<state>/rituals/<name>/`: its stamp, its journal, its notes, its runs.
//!
//! Nothing here fires anything; the daemon does, and the CLI edits files, both through this.

mod edit;
mod journal;

pub use crate::frontmatter::name_ok;
pub use edit::{Add, add, mine, remove, set_enabled, template, toggle};
pub use journal::{Dir, Entry};

use crate::cron::Schedule;
use crate::frontmatter::{self, NAME_RULE};
use crate::paths;
use crate::proto::RitualInfo;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Every form `cron::Schedule` reads, as the messages and the template name them.
const SCHEDULES: &str = "five cron fields, @hourly @daily @weekly @monthly @yearly, or \
                         \"every 30m\" / \"every 2h\" (a length that divides the hour or the day)";
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

/// A length such as `90s`, `30m` or `2h`, in seconds.
pub fn seconds(w: &str) -> Option<u64> {
    keep_len(w).ok().flatten()
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

/// The user's rituals, then the shipped ones.
pub fn dirs(share: Option<&Path>) -> Vec<PathBuf> {
    let mut d = vec![mine_dir()];
    d.extend(share.map(|s| s.join("rituals")));
    d
}

/// Where the user's own rituals live.
pub fn mine_dir() -> PathBuf {
    paths::config_dir().join("rituals")
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

const KEYS: &[&str] = &[
    "name",
    "description",
    "schedule",
    "target",
    "cwd",
    "model",
    "effort",
    "mode",
    "permission_mode",
    "mcp_config",
    "keep",
    "overlap",
    "headless",
    "catch_up",
    "enabled",
    "allowed_tools",
    "allowedTools",
];

/// The frontmatter (`frontmatter::read`) and the prompt under it. A file with no fence is all
/// prompt, and has no schedule.
pub fn parse(slug: &str, path: &Path, shipped: bool, text: &str) -> Ritual {
    let front = frontmatter::read(text, KEYS);
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
        prompt: front.body,
        // A typo as far as a reader can tell: a `schedul:` line never fires.
        unknown: front.unknown,
    };
    for f in &front.fields {
        let v = f.text();
        let some = |v: String| (!v.is_empty()).then_some(v);
        match f.key.as_str() {
            "name" => r.name = some(v),
            "description" => r.description = some(v),
            "schedule" => r.schedule = v,
            "target" => r.target = v,
            "cwd" => r.cwd = some(crate::paths::expand(&v)),
            "model" => r.model = some(v),
            "effort" => r.effort = some(v),
            "mode" | "permission_mode" => r.mode = some(v),
            "mcp_config" => r.mcp_config = some(v),
            "keep" => r.keep = v,
            "overlap" => r.overlap = v,
            "headless" => r.headless = v,
            "catch_up" => r.catch_up = v,
            "enabled" => r.enabled = v,
            "allowed_tools" | "allowedTools" => r.allowed_tools = f.list(),
            _ => {}
        }
    }
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
        crate::paths::claude_json()
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
            return p(format!("cwd: {} is not a directory", crate::paths::short(cwd)));
        }
        // A run stalled at the trust dialog is alive, so every later fire would skip itself
        // as still going. `claude -p` never shows the dialog.
        if !r.headless() && !trust.trusted(Path::new(cwd)) {
            return p(format!(
                "cwd: nothing has answered Claude Code's trust prompt for {} (open Claude Code \
                 there once and accept; until then it does not fire)",
                crate::paths::short(cwd)
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
    let mcp = r.mcp_config.as_deref().map(crate::paths::expand);
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
