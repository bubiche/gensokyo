//! Ledgers: a file per project that a resident in a role with `ledger: project` keeps its running
//! picture in, so a compaction, a `/clear` or a new session takes nothing it wrote down. It lives
//! in the state dir, never the project, and every worktree of a repository shares the one of its
//! main checkout.

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

/// The ledger of the project at `root`: its directory's name, for a person reading the list,
/// and a hash of its whole path, so that `/a-b` and `/a/b` keep two.
pub fn file(root: &Path) -> PathBuf {
    let path = root.to_string_lossy();
    // FNV-1a: the same in every build, which a std hasher does not promise.
    let hash = path.bytes().fold(0x811c_9dc5_u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    let name = root.file_name().map_or("root".into(), |n| n.to_string_lossy().into_owned());
    paths::state_dir().join("ledgers").join(format!("{name}-{hash:08x}.md"))
}

/// The ledger for a resident in `cwd`, made with a heading naming its project when there is
/// none: whoever reads it next should find a file, and know whose it is.
pub fn make(cwd: &Path) -> Result<PathBuf, String> {
    let root = root(cwd);
    let f = file(&root);
    let dir = f.parent().unwrap_or(Path::new("/"));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // Its real path, which is what a permission rule matches.
    let real = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let f = real.join(f.file_name().unwrap_or_default());
    if !f.exists() {
        let head = format!("# Ledger for {}\n", root.display());
        std::fs::write(&f, head).map_err(|e| format!("{}: {e}", f.display()))?;
    }
    Ok(f)
}
