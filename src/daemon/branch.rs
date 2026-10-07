//! `target: branch`: a probe prints `{"<repo>:<branch>": {facts}}`, and when a branch has a new
//! fact, all of its facts, the new ones first, are typed into the resident working on it. This
//! is the one place a probe's output reaches a prompt, so only short tokens get through: a
//! fact's value is letters, digits and `_.:/#@-`, never prose. Tokens can still spell words, so
//! a probe passes only what it works out itself (states, counts, hashes, numbers). A fact going
//! away sends nothing; one that comes back is new again. The repo is matched ignoring ASCII
//! case, the branch exactly.

use super::shrine::Shared;
use crate::ritual::{Dir, Ritual};
use crate::tele;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

type Facts = BTreeMap<String, String>;

/// Each nudge by the id of the resident it goes to, and what was left out, each to say once.
pub(super) type Routed = (Vec<(String, Nudge)>, Vec<String>);

/// One branch's news, on its way to the resident there.
pub(super) struct Nudge {
    pub(super) key: String,
    repo: String,
    branch: String,
    /// Its facts as they are now: what is recorded as sent once it is typed.
    facts: Facts,
    /// From a fire by hand, which sends what is there whether or not it is news.
    by_hand: bool,
    pub(super) text: String,
}

fn file(d: &Dir) -> std::path::PathBuf {
    d.path.join("branches.json")
}

/// What was last sent for each key.
fn sent(d: &Dir) -> BTreeMap<String, Facts> {
    let b = std::fs::read(file(d)).unwrap_or_default();
    serde_json::from_slice(&b).unwrap_or_default()
}

fn save(d: &Dir, s: &BTreeMap<String, Facts>) {
    let _ = std::fs::create_dir_all(&d.path);
    let b = serde_json::to_vec(s).unwrap_or_default();
    let _ = super::store::write_atomic(&file(d), &b);
}

fn token(s: &str, extra: &str, most: usize) -> bool {
    let ok = |c: char| c.is_ascii_alphanumeric() || extra.contains(c);
    !s.is_empty() && s.len() <= most && s.chars().all(ok)
}

/// A key's repo and branch: `<repo path>:<branch>`.
fn split(key: &str) -> Option<(&str, &str)> {
    let (repo, branch) = key.split_once(':')?;
    let part = |p: &str| token(p, "._/-", 200) && !p.starts_with(['-', '/']);
    (part(repo) && part(branch)).then_some((repo, branch))
}

/// A fact's value as text, when it is a token.
fn value(v: &Value) -> Option<String> {
    let t = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    token(&t, "_.:/#@-", 200).then_some(t)
}

/// The facts of key `key`, and its context (`_name`), adding what was left out to `bad`.
fn facts(key: &str, v: &Value, bad: &mut Vec<String>) -> Option<(Facts, Facts)> {
    let (mut facts, mut context) = (Facts::new(), Facts::new());
    for (name, v) in v.as_object()? {
        let (plain, into) = match name.strip_prefix('_') {
            Some(n) => (n, &mut context),
            None => (name.as_str(), &mut facts),
        };
        let named = plain.len() <= 24 && plain.chars().all(|c| c.is_ascii_lowercase() || c == '_');
        match value(v).filter(|_| named && !plain.is_empty()) {
            Some(t) => _ = into.insert(name.clone(), t),
            None => bad.push(format!("left out {name} of {key}: not a name and a short token")),
        }
    }
    Some((facts, context))
}

/// Where a resident works now, and that place's repo and branch.
fn place(dir: &str) -> Option<(String, String)> {
    let p = Path::new(dir);
    Some((tele::git_repo(p)?, tele::git_head(p)?))
}

/// What a fire of branch ritual `r` sends, by resident id. Records what went away, and drops
/// each held fire of `r` for a resident that is sent nothing now. Err when the output is not one
/// JSON object; the second item is what was left out, each to say once.
pub(super) fn route(
    shrine: &Shared,
    r: &Ritual,
    d: &Dir,
    out: &[u8],
    by_hand: bool,
) -> Result<Routed, String> {
    let Ok(Value::Object(all)) = serde_json::from_slice::<Value>(out) else {
        return Err(
            "its probe printed something other than one JSON object of {\"<repo>:<branch>\": \
             {facts}}"
                .into(),
        );
    };
    let mut bad = Vec::new();
    let mut now = BTreeMap::new();
    for (key, v) in &all {
        match split(key).and_then(|_| facts(key, v, &mut bad)) {
            Some(f) => _ = now.insert(key.clone(), f),
            None => bad.push(format!("left out the key {key}: not <repo>:<branch> with facts")),
        }
    }
    // A fact gone is forgotten as sent, so its return is news.
    let mut was = sent(d);
    was.retain(|k, _| now.contains_key(k));
    for (k, f) in was.iter_mut() {
        f.retain(|n, v| now[k].0.get(n) == Some(v));
    }
    save(d, &was);
    let mut sh = shrine.borrow_mut();
    let places: Vec<(String, String, String, bool, i64)> = sh
        .entries
        .iter()
        .filter(|e| e.handle.is_some())
        .filter_map(|e| {
            let dir = e.here.as_ref().map_or(e.rec.cwd.clone(), |(_, d)| d.clone());
            let (repo, branch) = place(&dir)?;
            Some((e.rec.id.clone(), repo, branch, dir == e.rec.cwd, e.aware.heard()))
        })
        .collect();
    let mut out = Vec::new();
    for (key, (f, context)) in now {
        let (repo, branch) =
            split(&key).map(|(r, b)| (r.to_string(), b.to_string())).unwrap_or_default();
        let here = places.iter().filter(|p| p.1.eq_ignore_ascii_case(&repo) && p.2 == branch);
        let Some(who) = here.max_by_key(|p| (p.3, p.4)).map(|p| p.0.clone()) else { continue };
        let new = |n: &&String| by_hand || was.get(&key).and_then(|w| w.get(*n)) != f.get(*n);
        let fresh: Vec<&String> = f.keys().filter(new).collect();
        if fresh.is_empty() {
            continue;
        }
        let mut line: Vec<String> = fresh.iter().map(|n| format!("{n}: {}", f[*n])).collect();
        line.extend(f.iter().filter(|(n, _)| !fresh.contains(n)).map(|(n, v)| format!("{n}: {v}")));
        let text = fill(&r.prompt, &key, &branch, &line.join(", "), &context);
        out.push((who, Nudge { key, repo, branch, facts: f, by_hand, text }));
    }
    // A held fire for a resident sent nothing now gives up: its news went away (a merged PR's
    // key is gone), or another resident on that branch is told instead.
    sh.rites.waiting.retain(|(slug, id), _| *slug != r.slug || out.iter().any(|(to, _)| to == id));
    Ok((out, bad))
}

/// The ritual's body with `{key}`, `{branch}`, `{facts}` and each `{_name}` filled in; a
/// `{_name}` the probe did not give is left empty, and a `{_` that is no placeholder stays. A
/// body with no `{facts}` gets them at its end.
fn fill(body: &str, key: &str, branch: &str, facts: &str, context: &Facts) -> String {
    let mut t = body.replace("{key}", key).replace("{branch}", branch);
    for (n, v) in context {
        t = t.replace(&format!("{{{n}}}"), v);
    }
    let mut from = 0;
    while let Some(i) = t[from..].find("{_").map(|i| from + i) {
        match t[i..].find('}').map(|e| i + e).filter(|&end| token(&t[i + 2..end], "_", 24)) {
            Some(end) => t.replace_range(i..=end, ""),
            None => from = i + 2,
        }
    }
    match t.contains("{facts}") {
        true => t.replace("{facts}", facts),
        false => format!("{t}\n\n{key}: {facts}"),
    }
}

/// Just before typing: still on that branch, and still news. Else it is not typed, and not
/// recorded, so the next fire measures again.
pub(super) fn still(shrine: &Shared, d: &Dir, id: &str, n: &Nudge) -> bool {
    let sh = shrine.borrow();
    let Some(e) = sh.entries.iter().find(|e| e.rec.id == id) else { return false };
    let dir = e.here.as_ref().map_or(e.rec.cwd.clone(), |(_, d)| d.clone());
    let on = place(&dir).is_some_and(|(r, b)| r.eq_ignore_ascii_case(&n.repo) && b == n.branch);
    let was = sent(d);
    on && (n.by_hand
        || n.facts.iter().any(|(k, v)| was.get(&n.key).and_then(|w| w.get(k)) != Some(v)))
}

/// Typed in: these facts are what was sent.
pub(super) fn record(d: &Dir, n: &Nudge) {
    let mut was = sent(d);
    was.insert(n.key.clone(), n.facts.clone());
    save(d, &was);
}
