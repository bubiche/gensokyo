//! Ledgers: a file per resident and project that a resident in a role with `ledger: resident`
//! keeps what it has in flight in, so a compaction or a `/clear` takes nothing it wrote down. It
//! lives in the state dir, never the project. Two residents in one repository keep two; one
//! summoned again under the same name in any worktree of it finds its own.

use crate::paths;
use std::path::{Path, PathBuf};

/// The project `cwd` is in: its repository's common git dir, which every worktree shares (a
/// worktree's `commondir`, else the dir a `.git` file names, as a submodule's does), taken as
/// the checkout above it when it is a `.git`; or `cwd` itself outside a repository.
pub fn root(cwd: &Path) -> PathBuf {
    let Some(top) = cwd.ancestors().find(|d| d.join(".git").exists()) else {
        return cwd.to_path_buf();
    };
    let common = std::fs::read_to_string(top.join(".git")).ok().and_then(|f| {
        let git = top.join(f.strip_prefix("gitdir:")?.trim());
        let common = std::fs::read_to_string(git.join("commondir")).ok();
        std::fs::canonicalize(common.map_or(git.clone(), |c| git.join(c.trim()))).ok()
    });
    let common = common.unwrap_or_else(|| top.join(".git"));
    match common.file_name().is_some_and(|n| n == ".git") {
        true => common.parent().unwrap_or(top).to_path_buf(),
        false => common,
    }
}

/// The ledger of resident `name` in the project at `root`: the project's directory name and the
/// resident's, for a person reading the list, and a hash of the project's whole path, so that
/// `/a-b` and `/a/b` keep two. A name is matched without case, as the shrine matches it.
pub fn file(root: &Path, name: &str) -> PathBuf {
    let path = root.to_string_lossy();
    // FNV-1a: the same in every build, which a std hasher does not promise.
    let hash = path.bytes().fold(0x811c_9dc5_u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    let dir = root.file_name().map_or("root".into(), |n| n.to_string_lossy().into_owned());
    let name = name.to_ascii_lowercase();
    paths::state_dir().join("ledgers").join(format!("{dir}-{hash:08x}-{name}.md"))
}

/// The ledger of resident `name` in `cwd`, made with a heading naming it and its project when
/// there is none: whoever reads it next should find a file, and know whose it is.
pub fn make(cwd: &Path, name: &str) -> Result<PathBuf, String> {
    let root = root(cwd);
    open(&file(&root, name), name, &root)
}

/// A ledger a resident was already given, made again the same way if it has gone: it stays the
/// resident's under a name it has taken since.
pub fn keep(f: &Path, cwd: &Path, name: &str) -> Result<PathBuf, String> {
    open(f, name, &root(cwd))
}

fn open(f: &Path, name: &str, root: &Path) -> Result<PathBuf, String> {
    let dir = f.parent().unwrap_or(Path::new("/"));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // Its real path, which is what a permission rule matches.
    let real = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let f = real.join(f.file_name().unwrap_or_default());
    if !f.exists() {
        let head = format!("# Ledger of {name} in {}\n", root.display());
        std::fs::write(&f, head).map_err(|e| format!("{}: {e}", f.display()))?;
    }
    Ok(f)
}
