//! Roles: a paragraph appended to a resident's system prompt, which `/clear` and compaction keep
//! where a first prompt is gone. A role is a markdown file, its words what follows an optional
//! `---` block whose one setting is `ledger: project` (the `ledger` module). They ship in
//! `share/roles/`; the user's own in the config dir's `roles/` shadow those of the same file
//! name. A role with a `/` in it is a file of the user's anywhere, by its full path.

use crate::frontmatter::{self, name_ok};
use crate::paths;
use std::path::{Path, PathBuf};

fn dirs(share: Option<&Path>) -> Vec<PathBuf> {
    let mut d = vec![paths::config_dir().join("roles")];
    d.extend(share.map(|s| s.join("roles")));
    d
}

/// Every role by name, the user's and the shipped ones, sorted.
pub fn names(share: Option<&Path>) -> Vec<String> {
    let mut v: Vec<String> = dirs(share)
        .iter()
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".md").map(String::from))
        .filter(|n| name_ok(n))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The file `role` names: a full path, or a name looked up in the user's `roles/`, then the
/// shipped ones.
pub fn find(role: &str, share: Option<&Path>) -> Result<PathBuf, String> {
    if role.contains('/') {
        let p = PathBuf::from(role);
        return match (p.is_absolute(), p.is_file()) {
            (false, _) => Err(format!("{role} is a file, and a file is named by its full path")),
            (true, false) => Err(format!("{role}: no such file")),
            (true, true) => Ok(p),
        };
    }
    if name_ok(role) {
        let file = format!("{role}.md");
        if let Some(p) = dirs(share).into_iter().map(|d| d.join(&file)).find(|p| p.is_file()) {
            return Ok(p);
        }
    }
    let mine = paths::short(&paths::config_dir().join("roles").to_string_lossy());
    let file = match role.ends_with(".md") {
        true => format!("; a file is named by its path, ./{role}"),
        false => String::new(),
    };
    let there = names(share).join(", ");
    Err(format!("no role {role} (there are {there}; yours go in {mine}/{file})"))
}

/// A role's file, read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Role {
    /// What goes into the system prompt: the file under its `---` block.
    pub words: String,
    /// `ledger: project`.
    pub ledger: bool,
}

/// The role in `file`, found by `find`: a setting it does not know refuses it, as a ledger
/// quietly not kept would be found out only after a compaction.
pub fn read(file: &Path) -> Result<Role, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let front = frontmatter::read(&text, &["ledger"]);
    let bad = |why: String| Err(format!("role {}: {why}", file.display()));
    if !front.unknown.is_empty() {
        return bad(format!("not a role setting: {}", front.unknown.join(", ")));
    }
    let mut role = Role { words: front.body, ledger: false };
    for f in front.fields.iter().filter(|f| f.key == "ledger") {
        match f.text().as_str() {
            "project" => role.ledger = true,
            v => return bad(format!("ledger: {v} (ledger: project, or no ledger line)")),
        }
    }
    Ok(role)
}
