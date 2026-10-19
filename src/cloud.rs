//! Claude Code routines (cloud sessions on claude.ai's own schedule), as gensokyo watches them.
//! No token touches gensokyo: every call is a courier, a `claude -p` stripped to the one tool
//! that reaches claude.ai (`RemoteTrigger`) and sent for one call. A gate hook allows that call
//! and nothing else, and the reply is read from the tool's own result in the stream, never from
//! what the model says: a run's title can quote whatever a run read.

use serde_json::{Value, json};

/// Whether the user has the watch on: it spends their plan's usage, on an API Claude Code does
/// not document, so it is off until they say.
pub fn on() -> bool {
    crate::paths::config("CLOUD_WATCH").as_deref() == Some("on")
}

/// Where a run is opened, and the only links gensokyo passes on.
pub const WEB: &str = "https://claude.ai/code/";

/// The most a courier may spend, in dollars. One call costs a tenth of a cent.
const BUDGET: &str = "0.10";

/// An id as the API gives one (`trig_…`, `cse_…`): letters, digits and `_` only, so it is safe
/// in a prompt, a hook's command line and a link.
pub fn valid_id(id: &str, prefix: &str) -> bool {
    id.len() > prefix.len()
        && id.len() <= 64
        && id.starts_with(prefix)
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The call as the tool's input, a field a line. Given the whole call on one line, haiku
/// wrote `{"action": "create", "body": ` itself, copied the line from the body on, the call's
/// own last brace too, and closed it again (2.1.296): most creates came out a brace too long.
pub fn prompt(call: &Value) -> String {
    let all = call.as_object().into_iter().flatten();
    let fields: String = all.map(|(k, v)| format!("\n{k}: {v}")).collect();
    format!(
        "Call the RemoteTrigger tool once, with an input of exactly these fields, each value \
         exactly as given (a JSON value; nothing added), then reply OK and stop. Only if the tool \
         says the input could not be parsed, send it again; whatever else the result says, make \
         no other call.{fields}"
    )
}

/// The courier's flags, after `claude`: one tool, no MCP servers, no skills, no transcript
/// left behind, a budget, a few turns, nobody to ask, and the gate. `exe` is gensokyo, which
/// the gate runs; `ticket` a file the gate takes away with the one call it allows, so the call
/// is made once: a run fired twice could post twice.
pub fn args(call: &Value, exe: &str, ticket: &std::path::Path) -> Vec<String> {
    // Claude Code lets a call through when its hook fails any way but exit 2 (2.1.296: a gate
    // that could not be found let `list` run), so a gate that cannot run refuses.
    let gate = format!(
        "{} _gate {} {} || exit 2",
        sh_quote(exe),
        sh_quote(&call.to_string()),
        sh_quote(&ticket.to_string_lossy())
    );
    let settings = json!({"hooks": {"PreToolUse": [
        {"matcher": "*", "hooks": [{"type": "command", "command": gate}]}
    ]}});
    let flags = [
        "-p",
        "--model",
        "haiku",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--setting-sources",
        "local",
        "--disable-slash-commands",
        "--strict-mcp-config",
        "--tools",
        "RemoteTrigger",
        "--allowedTools",
        "RemoteTrigger",
        "--permission-prompts",
        "none",
        "--max-turns",
        "4",
        "--max-budget-usd",
        BUDGET,
        "--settings",
    ];
    let mut v: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
    v.extend([settings.to_string(), "--".into(), prompt(call)]);
    v
}

fn sh_quote(s: &str) -> String {
    crate::daemon::launch::sh_quote(s)
}

/// Why the gate turns a PreToolUse payload down: anything but `RemoteTrigger` with exactly the
/// courier's call. None allows it.
pub fn gate(payload: &Value, call: &Value) -> Option<String> {
    let tool = payload["tool_name"].as_str().unwrap_or("");
    if tool != "RemoteTrigger" {
        return Some(format!("gate: only RemoteTrigger, not {tool}"));
    }
    let action = payload["tool_input"]["action"].as_str().unwrap_or("?");
    (payload["tool_input"] != *call).then(|| format!("gate: only {call}, not this {action}"))
}

/// `gensokyo _gate <call> <ticket>`: Claude Code's PreToolUse payload on stdin; a denial on
/// stdout, or nothing to let the call through, once: it takes the ticket away.
pub fn gate_main(call: &str, ticket: &str) {
    let mut b = Vec::new();
    let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut b);
    let payload: Value = serde_json::from_slice(&b).unwrap_or(Value::Null);
    let why = match serde_json::from_str::<Value>(call) {
        Ok(call) => gate(&payload, &call),
        Err(_) => Some("gate: no call to allow".into()),
    };
    let once = || ticket.is_empty() || std::fs::remove_file(ticket).is_err();
    let why = why.or_else(|| once().then(|| "gate: that call was made already".into()));
    if let Some(why) = why {
        let out = json!({"hookSpecificOutput": {"hookEventName": "PreToolUse",
            "permissionDecision": "deny", "permissionDecisionReason": why}});
        println!("{out}");
    }
}

/// From a courier's stream-json: the API's reply to `call`, its HTTP status and its body as the
/// tool gave it. From the first `tool_use` whose input is the call: the gate let that one through
/// and no other, so one refused before it (a slip the model then mended) ran nothing. One Claude
/// Code could not read as JSON (haiku copying a long body may slip a brace) never ran either: it
/// was refused before any hook.
pub fn reply(stream: &str, call: &Value) -> Result<(u16, String), String> {
    let lines: Vec<Value> = stream.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let blocks = |kind: &'static str| {
        lines
            .iter()
            .filter(move |l| l["type"] == kind)
            .flat_map(|l| l["message"]["content"].as_array().into_iter().flatten())
    };
    let read =
        |b: &&Value| b["type"] == "tool_use" && b["input"].get("__unparsedToolInput").is_none();
    let uses: Vec<&Value> = blocks("assistant").filter(read).collect();
    let end = lines.iter().rev().find(|l| l["type"] == "result");
    let said = || end.and_then(|e| e["result"].as_str()).map(|s| crate::tele::clean(s, 200));
    let Some(first) = uses.first() else {
        let why = said().filter(|s| !s.is_empty()).unwrap_or("no reply at all".into());
        return Err(format!("the courier made no call: {why}"));
    };
    let asked = |u: &&Value| u["name"] == "RemoteTrigger" && u["input"] == *call;
    let Some(use_) = uses.iter().copied().find(asked) else {
        // Its words are whatever it read: the action alone, as an id is.
        let action = first["input"]["action"].as_str().unwrap_or("");
        let action = if valid_id(&format!("a_{action}"), "a_") { action } else { "?" };
        return Err(format!("the courier tried another call ({action}), and the gate refused it"));
    };
    let result = blocks("user")
        .find(|b| b["type"] == "tool_result" && b["tool_use_id"] == use_["id"])
        .ok_or("the courier's call has no result")?;
    let text = match &result["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect(),
        _ => String::new(),
    };
    if result["is_error"] == true {
        return Err(format!("the call failed: {}", crate::tele::clean(&text, 200)));
    }
    let (head, body) = text.split_once('\n').unwrap_or((&text, ""));
    let code = head.strip_prefix("HTTP ").and_then(|c| c.trim().parse().ok());
    let code =
        code.ok_or_else(|| format!("not an HTTP reply: {}", crate::tele::clean(head, 80)))?;
    Ok((code, body.to_string()))
}

/// The JSON a reply's body starts with. The tool may follow it with a line of its own ("→ no
/// run sessions recorded…", 2.1.296).
pub fn json(body: &str) -> Result<Value, String> {
    let mut values = serde_json::Deserializer::from_str(body).into_iter::<Value>();
    match values.next() {
        Some(Ok(v)) => Ok(v),
        Some(Err(e)) => Err(format!("not JSON: {e}")),
        None => Err("no JSON".into()),
    }
}

/// The message of an API error body, or the start of whatever came.
pub fn error_of(body: &str) -> String {
    let v: Value = json(body).unwrap_or(Value::Null);
    match v["error"]["message"].as_str() {
        Some(m) => plain(m, 200),
        None => plain(body, 200),
    }
}

/// Routines are listed at least this often, for fires no schedule foretells: by hand, from the
/// web.
pub const HOURLY: i64 = 3600;

/// A fire is looked for this long after its time: a scheduled run may start late.
pub const MARGIN: i64 = 60;

/// While something is pending (a run going, a fire not seen yet), it is looked at again after
/// half the time it has been pending, within these: a run of seconds is told within a minute,
/// and one of an hour costs a dozen couriers.
const POLL_LEAST: i64 = 60;
const POLL_MOST: i64 = 600;

/// When the watch last looked and how that went, which say when it looks next. Times are on
/// the ritual clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pace {
    /// When the last round began.
    pub listed: Option<i64>,
    /// Since when something has been pending.
    pub pending: Option<i64>,
    /// Rounds in a row that could not list the routines.
    pub fails: u32,
}

impl Pace {
    /// Whether a round is due: the first, one after a gap in the ticking (a lid closed), the
    /// hourly one, a fire's time come round, something pending, or a retry after a failure,
    /// later with each.
    pub fn due(&self, routines: &[Routine], now: i64, catch: bool) -> bool {
        let Some(at) = self.listed else { return true };
        let since = now - at;
        // Failing, the retry's own back-off says when.
        let poll = self.pending.filter(|_| self.fails == 0);
        let poll = poll.map(|p| ((now - p) / 2).clamp(POLL_LEAST, POLL_MOST));
        let retry = (self.fails > 0).then(|| (POLL_LEAST << self.fails.min(6)).min(HOURLY));
        let mut fires = routines.iter().filter(|r| r.enabled).filter_map(|r| r.next_run);
        catch
            || since >= HOURLY
            || poll.is_some_and(|p| since >= p)
            || retry.is_some_and(|r| since >= r)
            || fires.any(|t| t + MARGIN > at && t + MARGIN <= now)
    }
}

/// A routine, as `list` gives it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Five fields, in UTC; none for a routine that fires once, or only by hand.
    pub cron: Option<String>,
    /// Epoch seconds.
    pub next_run: Option<i64>,
    pub last_fired: Option<i64>,
}

/// A run of a routine: one cloud session, as `list_runs` gives it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Run {
    pub id: String,
    pub title: String,
    /// `active` while the session lives.
    pub status: String,
    /// `running` while a turn goes, `idle` once it has ended.
    pub worker: String,
    pub created: i64,
    pub last_event: i64,
    pub url: String,
}

/// Epoch seconds from the API's RFC 3339 times.
fn epoch(v: &Value) -> Option<i64> {
    v.as_str()?.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
}

/// Bidi controls and invisible marks: they could reorder or hide what is shown next to them.
fn hidden(c: &char) -> bool {
    matches!(
        *c as u32,
        0x061C | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2069 | 0xFEFF
    )
}

/// One line of what claude.ai said, with nothing that could reorder or hide what is shown
/// next to it.
fn plain(s: &str, max: usize) -> String {
    let s: String = s.chars().filter(|c| !hidden(c)).collect();
    crate::tele::clean(&s, max)
}

/// A name or title, as `plain`.
fn text(v: &Value, max: usize) -> String {
    plain(v.as_str().unwrap_or(""), max)
}

/// A run's id from itself, from its link, or from the id `claude --cloud` prints: the API pairs
/// `session_X` (and `…/code/session_X`) with `cse_X`.
pub fn run_id(who: &str) -> Option<String> {
    if valid_id(who, "cse_") {
        return Some(who.into());
    }
    let page = link(who).and_then(|l| l.strip_prefix(WEB));
    let id = format!("cse_{}", page.unwrap_or(who).strip_prefix("session_")?);
    valid_id(&id, "cse_").then_some(id)
}

/// A run's link from its id.
pub fn run_link(id: &str) -> String {
    format!("{WEB}session_{}", id.strip_prefix("cse_").unwrap_or(id))
}

/// `claude -p --cloud`, which queues the user's words as the run's next turn and returns at once
/// with `{ok, session_id, url}`.
pub fn say_args(id: &str, text: &str) -> Vec<String> {
    let a = ["-p", "--cloud", id, "--output-format", "json", "--", text];
    a.map(String::from).to_vec()
}

/// Whether claude queued them for that run.
pub fn said(stdout: &str, id: &str) -> Result<(), String> {
    let v = json(stdout).map_err(|_| "claude did not say whether the run got it".to_string())?;
    match (&v["ok"], v["session_id"].as_str()) {
        (Value::Bool(true), Some(s)) if s == id => Ok(()),
        (Value::Bool(true), _) => Err(format!("claude sent it to another session than {id}")),
        _ => {
            let why = v["error"].as_str().map_or_else(|| v.to_string(), String::from);
            Err(format!("claude said: {}", plain(&why, 200)))
        }
    }
}

/// `claude --cloud`, which makes a session on claude.ai with the task and returns at once:
/// `env` picks its environment for this launch alone.
pub fn new_args(task: &str, env: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut a = Vec::new();
    if let Some(e) = env {
        let s = json!({"remote": {"defaultEnvironmentId": e}});
        a.extend(["--settings".into(), s.to_string()]);
    }
    if let Some(m) = model {
        a.extend(["--model".into(), m.into()]);
    }
    a.extend(["--cloud".into(), task.into()]);
    a
}

/// The session claude says it made, as `View: <link>` and `Resume with: claude --teleport
/// session_X` name it, the two agreeing: its id (`cse_…`) and link.
pub fn created(screen: &str) -> Result<(String, String), String> {
    let lines = lines(screen);
    let after = |p: &str| lines.iter().find_map(|l| l.trim().strip_prefix(p).map(str::trim));
    let view = after("View:").and_then(link);
    let resume = after("Resume with: claude --teleport");
    match (view.and_then(run_id), resume.and_then(run_id)) {
        (Some(a), Some(b)) if a == b => Ok((a.clone(), run_link(&a))),
        _ if view.is_some() || resume.is_some() => {
            let seen = view.or(resume).unwrap_or_default();
            Err(format!("claude's answer did not name one session plainly ({seen})"))
        }
        _ if after("Created cloud session:").is_some() => {
            Err("claude made a session, and its link could not be read".into())
        }
        _ => Err(format!("claude did not say it made a session: {}", plain(screen.trim(), 200))),
    }
}

/// A repository's `owner/name` from origin's URL, when it is on GitHub as a cloud session can
/// clone it: behind an SSH host alias, `claude --cloud` sends a copy with no remote instead,
/// from which the session can neither push nor be followed.
pub fn github(url: &str) -> Result<String, String> {
    let path = ["git@github.com:", "ssh://git@github.com/", "https://github.com/"]
        .iter()
        .find_map(|p| url.strip_prefix(p));
    let ok = |s: &str| {
        !s.is_empty()
            && !s.starts_with('-')
            && s != "."
            && s != ".."
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    if let Some(p) = path {
        let p = p.trim_end_matches('/');
        let p = p.strip_suffix(".git").unwrap_or(p);
        if let Some((o, r)) = p.split_once('/').filter(|(o, r)| ok(o) && ok(r)) {
            return Ok(format!("{o}/{r}"));
        }
    }
    let host = url.strip_prefix("git@").and_then(|h| h.split_once(':'));
    if let Some((h, p)) = host.filter(|(h, _)| h.starts_with("github.com") && *h != "github.com") {
        return Err(format!(
            "origin, {url}, goes through an SSH host alias ({h}): a cloud session would get a \
             copy with no remote and could not push. `git remote set-url origin \
             git@github.com:{p}` fixes it, and `git remote set-url --push origin {url}` keeps \
             pushes on the alias"
        ));
    }
    // Not echoed: an https URL may carry a token.
    Err("origin is not a GitHub repository (git@github.com:, ssh://git@github.com/ or \
         https://github.com/), which a cloud session clones"
        .into())
}

/// Text of many lines as a terminal may be shown it: no control or hidden character left in a
/// line.
fn lines(s: &str) -> Vec<String> {
    let shown = |c: &char| !c.is_control() && !hidden(c);
    s.trim_matches('\n')
        .lines()
        .map(|l| l.replace('\t', " ").chars().filter(shown).collect())
        .collect()
}

/// A page of a `get_run_log` body, as a terminal may be shown it: its lines, then where the
/// rest is when there is more.
pub fn run_log(body: &str, id: &str) -> String {
    let (head, rest) = body.split_once('\n').unwrap_or((body, ""));
    let head: Value = serde_json::from_str(head).unwrap_or(Value::Null);
    let mut out = lines(rest);
    if !head["next_cursor"].is_null() {
        out.push(format!("(the log goes on past this page: {})", run_link(id)));
    }
    out.join("\n")
}

/// Where a session is, by its log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    /// Nothing of Claude Code's on the page yet: a sandbox being made.
    #[default]
    Starting,
    Working,
    /// Its newest turn has ended, and nothing was said to it since.
    Idle,
}

impl Phase {
    pub fn word(self) -> &'static str {
        match self {
            Phase::Starting => "starting",
            Phase::Working => "working",
            Phase::Idle => "idle",
        }
    }
}

/// A turn that ended: when (epoch µs), whether on an error, and its answer as the log has it,
/// cut there (`… [+N chars]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub at: i64,
    pub failed: bool,
    pub answer: String,
}

/// What a page of a session's log (`get_run_log`, its newest events) says: where it is, the
/// turns that ended on it, oldest first, and when (epoch µs) the newest words to it came and the
/// newest event of any kind. Only a line that starts with a time and a kind counts: the log puts
/// each event on one line, so a quote of one inside another's text cannot begin a line.
#[derive(Debug, Default)]
pub struct Page {
    pub phase: Phase,
    pub turns: Vec<Turn>,
    pub words: Option<i64>,
    pub newest: Option<i64>,
}

pub fn session(body: &str) -> Page {
    let (mut p, mut said) = (Page::default(), None);
    for l in lines(body.split_once('\n').map_or("", |(_, rest)| rest)) {
        let Some((at, rest)) = l.strip_prefix('[').and_then(|l| l.split_once("] ")) else {
            continue;
        };
        let Ok(at) = at.parse::<jiff::Timestamp>() else { continue };
        let at = at.as_microsecond();
        p.newest = Some(at);
        let (kind, text) = rest.split_once(':').unwrap_or((rest, ""));
        let text = text.trim();
        match kind.split(' ').next().unwrap_or("") {
            "user" => {
                (p.words, said) = (Some(at), None);
                if p.phase == Phase::Idle {
                    p.phase = Phase::Working;
                }
            }
            "init" | "tool_use" | "tool_result" => p.phase = Phase::Working,
            "assistant" => {
                p.phase = Phase::Working;
                if text != "[thinking]" {
                    said = Some(text.to_string());
                }
            }
            "result" => {
                let (how, end) = text.split_once(" — ").unwrap_or((text, ""));
                let answer = said.take().unwrap_or_else(|| end.to_string());
                let failed = how.contains("is_error=true");
                p.turns.push(Turn { at, failed, answer: plain(&answer, ANSWER_MOST) });
                p.phase = Phase::Idle;
            }
            _ => {}
        }
    }
    p
}

/// The most of a session's answer kept: the log cuts one at about 2000.
const ANSWER_MOST: usize = 2400;

/// The routines in a `list` body, and whether there are more than it holds. One whose id is not
/// an id is left out.
pub fn routines(body: &str) -> Result<(Vec<Routine>, bool), String> {
    let v = json(body).map_err(|e| format!("list: {e}"))?;
    let data = v["data"].as_array().ok_or("list: no data")?;
    Ok((data.iter().filter_map(routine).collect(), v["has_more"] == true))
}

/// A routine from the API's own form of one, as `list` and every write's reply hold it.
pub fn routine(r: &Value) -> Option<Routine> {
    let id = r["id"].as_str().filter(|id| valid_id(id, "trig_"))?;
    Some(Routine {
        id: id.into(),
        name: text(&r["name"], 80),
        enabled: r["enabled"] == true,
        cron: r["cron_expression"].as_str().filter(|c| !c.is_empty()).map(|c| plain(c, 80)),
        next_run: epoch(&r["next_run_at"]),
        last_fired: epoch(&r["last_fired_at"]),
    })
}

/// The runs in a `list_runs` body, newest first as it gives them. One whose id or link is not
/// what the API gives is left out.
pub fn runs(body: &str) -> Result<Vec<Run>, String> {
    let v = json(body).map_err(|e| format!("list_runs: {e}"))?;
    let data = v["data"].as_array().ok_or("list_runs: no data")?;
    Ok(data
        .iter()
        .filter(|r| r["id"].as_str().is_some_and(|id| valid_id(id, "cse_")))
        .filter_map(|r| Some((r, link(r["url"].as_str()?)?)))
        .map(|(r, url)| Run {
            id: r["id"].as_str().unwrap_or_default().into(),
            title: text(&r["title"], 80),
            status: text(&r["status"], 20),
            worker: text(&r["worker_status"], 20),
            created: epoch(&r["created_at"]).unwrap_or(0),
            last_event: epoch(&r["last_event_at"]).unwrap_or(0),
            url: url.into(),
        })
        .collect())
}

/// A link gensokyo will show or open: on claude.ai/code, and nothing in it a terminal or a
/// shell could take for more than a link. Its query (`?from=cli&m=0`, as claude prints one) is
/// left off.
pub fn link(url: &str) -> Option<&str> {
    let (page, query) = url.split_once('?').unwrap_or((url, ""));
    let rest = page.strip_prefix(WEB)?;
    let ok =
        |s: &str, more: &[u8]| s.bytes().all(|b| b.is_ascii_alphanumeric() || more.contains(&b));
    (ok(rest, b"_-/") && ok(query, b"_-=&")).then_some(page)
}

/// A claude.ai link opened in the browser, by `open` with no shell between
/// (`GENSOKYO_OPEN` names another opener). It says nothing: the client's screen is the
/// terminal's.
pub fn browse(url: &str) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let url = link(url).ok_or("not a claude.ai link")?;
    let opener = std::env::var_os("GENSOKYO_OPEN").unwrap_or("open".into());
    let mut cmd = Command::new(opener);
    cmd.arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("open {url}: {s}")),
        Err(e) => Err(format!("open {url}: {e}")),
    }
}

/// A routine's page.
pub fn routine_link(id: &str) -> String {
    format!("{WEB}routines/{id}")
}

/// The model a routine runs on when none is named.
pub const MODEL: &str = "claude-sonnet-5-5";

/// The tools a routine's run is given. They do not hold back its connectors' tools: every one
/// of those runs unasked, writes too.
const TOOLS: [&str; 6] = ["Bash", "Read", "Write", "Edit", "Glob", "Grep"];

/// The longest prompt a routine is made with: the courier's model copies the whole call out,
/// and its gate allows that call only to the letter.
pub const PROMPT_MOST: usize = 4000;

/// A claude.ai connector a routine may be given, as the config names it:
/// `CLOUD_CONNECTOR_<name>=<uuid> <url>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Connector {
    pub name: String,
    pub uuid: String,
    pub url: String,
}

/// A connector from its config line, or why it is not one.
pub fn connector(name: &str, value: &str) -> Result<Connector, String> {
    let bad = |what: &str| {
        format!("CLOUD_CONNECTOR_{name} in the config: {what} (it is `<uuid> <https url>`)")
    };
    let named = !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b));
    if !named {
        return Err(bad("a name is letters, digits, _ and - only"));
    }
    let mut parts = value.split_whitespace();
    let (Some(uuid), Some(url), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad("not two words"));
    };
    let hex = uuid.len() == 36 && uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    if !hex {
        return Err(bad("not a uuid"));
    }
    if !url.starts_with("https://") || url.chars().any(|c| c.is_control() || hidden(&c)) {
        return Err(bad("not an https url"));
    }
    Ok(Connector { name: name.into(), uuid: uuid.into(), url: url.into() })
}

/// A fresh v4 uuid, for the routine's first message.
pub fn uuid4() -> std::io::Result<String> {
    let mut b = [0u8; 16];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut b)?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
}

/// `org/repo`, from that or its GitHub link.
fn repo(r: &str) -> Result<String, String> {
    let r = r.trim().trim_end_matches('/').trim_end_matches(".git");
    let r = r.strip_prefix("https://").unwrap_or(r);
    let r = r.strip_prefix("github.com/").unwrap_or(r);
    let ok = |p: &str| {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    };
    match r.split_once('/') {
        Some((o, n)) if ok(o) && ok(n) => Ok(format!("{o}/{n}")),
        _ => Err(format!("{r} is not a GitHub repository (org/repo)")),
    }
}

/// The `create` call for a routine, paused: claude.ai makes it as gensokyo writes it here and
/// nowhere else, with exactly the connectors named. None named, claude.ai gives it every one
/// the account has (as it does with the list left out), which `add` then clears.
pub fn create(
    a: &crate::proto::CloudAdd,
    cron: &str,
    env: &str,
    conns: &[Connector],
    uuid: &str,
) -> Result<Value, String> {
    let name = plain(&a.name, 200);
    if name.is_empty() || name != a.name || name.chars().count() > 80 {
        return Err("a routine's name is one line of at most 80 characters".into());
    }
    if a.prompt.trim().is_empty() || a.prompt.chars().count() > PROMPT_MOST {
        return Err(format!("a routine's prompt is 1 to {PROMPT_MOST} characters"));
    }
    if !valid_id(env, "env_") {
        return Err(format!("{env} is not an environment's id (env_…)"));
    }
    let model = a.model.as_deref().unwrap_or(MODEL);
    if model.is_empty() || !model.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
    {
        return Err(format!("{model} is not a model's id"));
    }
    let repos = a.repos.iter().map(|r| repo(r)).collect::<Result<Vec<_>, _>>()?;
    let sources: Vec<Value> = repos
        .iter()
        .map(|r| json!({"git_repository": {"url": format!("https://github.com/{r}")}}))
        .collect();
    let conns: Vec<Value> = conns
        .iter()
        .map(|c| json!({"connector_uuid": c.uuid, "name": c.name, "url": c.url}))
        .collect();
    let first = json!({"uuid": uuid, "session_id": "", "type": "user", "parent_tool_use_id": null,
        "message": {"content": a.prompt, "role": "user"}});
    let body = json!({
        "name": a.name,
        "cron_expression": cron,
        "enabled": false,
        "mcp_connections": conns,
        "job_config": {"ccr": {
            "environment_id": env,
            "session_context": {"model": model, "sources": sources, "allowed_tools": TOOLS},
            "events": [{"data": first}],
        }},
    });
    Ok(json!({"action": "create", "body": body}))
}

/// The routine in a write's or `get`'s reply: `{trigger: …}`, or the routine itself.
pub fn trigger(body: &str) -> Result<Value, String> {
    let v = json(body)?;
    let t = if v["trigger"].is_object() { v["trigger"].clone() } else { v };
    routine(&t).ok_or("no routine in the reply")?;
    Ok(t)
}

/// The connectors a routine has, by uuid.
pub fn connected(t: &Value) -> Vec<String> {
    let all = t["mcp_connections"].as_array().into_iter().flatten();
    let mut v: Vec<String> =
        all.filter_map(|c| c["connector_uuid"].as_str()).map(|u| plain(u, 64)).collect();
    v.sort();
    v
}

/// What a routine is and does, as a terminal may be shown it: before the user runs or resumes
/// one, and after one is made.
pub fn about(t: &Value, tz: &jiff::tz::TimeZone) -> String {
    let Some(r) = routine(t) else { return "no routine".into() };
    let ctx = &t["job_config"]["ccr"]["session_context"];
    let list = |v: &Value, f: &dyn Fn(&Value) -> &Value| {
        let all = v.as_array().into_iter().flatten();
        let names: Vec<String> = all.map(|x| text(f(x), 120)).filter(|s| !s.is_empty()).collect();
        if names.is_empty() { "none".into() } else { names.join(", ") }
    };
    let repos = list(&ctx["sources"], &|s| &s["git_repository"]["url"]);
    let conns = list(&t["mcp_connections"], &|c| &c["name"]);
    let model = text(&ctx["model"], 80);
    let env = text(&t["job_config"]["ccr"]["environment_id"], 80);
    let schedule = match (&r.cron, r.next_run) {
        (Some(c), Some(n)) => format!("{c} UTC, next {}", crate::ritual::when(n, tz)),
        (Some(c), None) => format!("{c} UTC"),
        (None, Some(n)) => format!("once, {}", crate::ritual::when(n, tz)),
        (None, None) => "by hand".into(),
    };
    let mut out = vec![
        format!("  routine     {}  ({})", r.name, if r.enabled { "on" } else { "paused" }),
        format!("  schedule    {schedule}"),
        format!("  model       {}", if model.is_empty() { "?".into() } else { model }),
        format!("  repos       {repos}"),
        format!("  connectors  {conns}"),
        format!("  environment {}", if env.is_empty() { "?" } else { &env }),
        format!("  page        {}", routine_link(&r.id)),
        "  prompt".into(),
    ];
    // The message a run is started with, as it is sent; the summary only without one.
    let first = &t["job_config"]["ccr"]["events"][0]["data"]["message"]["content"];
    let prompt = first.as_str().or(t["derived_state"]["prompt"].as_str()).unwrap_or("");
    out.extend(lines(prompt).into_iter().map(|l| format!("    {l}")));
    if conns != "none" {
        out.push(format!(
            "  every tool of {conns} runs unasked, writes too: allowed tools do not hold them back"
        ));
    }
    out.join("\n")
}

/// A cron in local time as claude.ai keeps routines: in UTC (claude.ai then fires within
/// minutes of it). The days are carried over as they are when no fire changes date in UTC.
/// When every fire moves a day the same way, only a weekly schedule moves with it (a daily one
/// whatever the moves): a date or a month would land on other days in another year. Then the result must fire at the same
/// moments as the local cron for a year, which a zone that changes its clocks does not; and at
/// most hourly, as claude.ai runs them.
pub fn utc_cron(spec: &str, tz: &jiff::tz::TimeZone, now: i64) -> Result<String, String> {
    use crate::cron::Schedule;
    use jiff::tz::TimeZone;
    use std::collections::BTreeSet;
    let year = now + 366 * 86400;
    let fires = Schedule::parse(spec)?.between(now, year, tz);
    let five = crate::cron::desugar(spec.trim()).unwrap_or_default();
    let local: Vec<&str> = five.split_whitespace().collect();
    if fires.is_empty() || local.len() != 5 {
        return Err(format!("{spec} does not come round within a year"));
    }
    if fires.windows(2).any(|w| w[1] - w[0] < HOURLY) {
        return Err(format!("{spec} comes round more often than claude.ai runs a routine: hourly"));
    }
    let none = || {
        format!(
            "{spec} has no UTC cron that keeps its local times, and claude.ai keeps a routine's \
             schedule in UTC: a zone that changes its clocks has none, and a time that falls on \
             another day in UTC keeps only days of the week, in every month"
        )
    };
    let stamp = |t: &i64| jiff::Timestamp::from_second(*t).ok();
    let at: Vec<jiff::Zoned> =
        fires.iter().filter_map(stamp).map(|t| t.to_zoned(TimeZone::UTC)).collect();
    let moved: BTreeSet<i32> = fires
        .iter()
        .filter_map(stamp)
        .zip(&at)
        .map(|(t, u)| (u.date() - t.to_zoned(tz.clone()).date()).get_days())
        .collect();
    let field = |set: BTreeSet<i8>, lo: i8, hi: i8| {
        if set.len() == (hi - lo + 1) as usize {
            return "*".to_string();
        }
        // Runs of three or more as a range.
        let v: Vec<i8> = set.into_iter().collect();
        let (mut out, mut i) = (Vec::new(), 0);
        while i < v.len() {
            let mut j = i;
            while j + 1 < v.len() && v[j + 1] == v[j] + 1 {
                j += 1;
            }
            match j - i {
                0 => out.push(v[i].to_string()),
                1 => out.extend([v[i].to_string(), v[j].to_string()]),
                _ => out.push(format!("{}-{}", v[i], v[j])),
            }
            i = j + 1;
        }
        out.join(",")
    };
    let min = field(at.iter().map(|z| z.minute()).collect(), 0, 59);
    let hour = field(at.iter().map(|z| z.hour()).collect(), 0, 23);
    let any = |f: &str| matches!(f, "*" | "?");
    let days = match moved.iter().collect::<Vec<_>>()[..] {
        [0] => format!("{} {} {}", local[2], local[3], local[4]),
        _ if any(local[2]) && local[3] == "*" && any(local[4]) => "* * *".into(),
        [_] if any(local[2]) && local[3] == "*" => {
            let week = at.iter().map(|z| z.weekday().to_sunday_zero_offset()).collect();
            format!("* * {}", field(week, 0, 6))
        }
        _ => return Err(none()),
    };
    let utc = format!("{min} {hour} {days}");
    let same = Schedule::parse(&utc).is_ok_and(|s| s.between(now, year, &TimeZone::UTC) == fires);
    same.then_some(utc).ok_or_else(none)
}
