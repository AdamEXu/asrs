//! Where downloads live (Electron, emsdk, npm, public logs): `$ASRS_CACHE`, or else the cache
//! next to the main checkout's real `target` directory.

use std::fs;
use std::path::{Path, PathBuf};

use crate::vectors::workspace_root;

pub const CACHE_VARIABLE: &str = "ASRS_CACHE";

/// The cache directory and how it was chosen, for error messages.
pub fn cache_dir() -> (PathBuf, String) {
    if let Some(dir) = std::env::var_os(CACHE_VARIABLE) {
        return (PathBuf::from(dir), format!("${CACHE_VARIABLE}"));
    }
    let root = workspace_root();
    let main = main_checkout(&root).unwrap_or_else(|| root.clone());
    let target = main.join("target");
    if target.is_symlink()
        && let Ok(real) = fs::canonicalize(&target)
        && let Some(parent) = real.parent()
    {
        return (
            parent.join("cache"),
            format!(
                "next to {}, the real path of {}",
                real.display(),
                target.display()
            ),
        );
    }
    (target.join("cache"), format!("{}/cache", target.display()))
}

/// The checkout that owns the git directory, so a linked worktree (whose own `target` may point
/// anywhere) shares the main checkout's cache.
fn main_checkout(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(root.to_path_buf());
    }
    let text = fs::read_to_string(&dot_git).ok()?;
    let git_dir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        root.join(git_dir)
    };
    let common = fs::read_to_string(git_dir.join("commondir")).ok()?;
    let common = fs::canonicalize(git_dir.join(common.trim())).ok()?;
    if common.file_name()? != ".git" {
        return None;
    }
    common.parent().map(Path::to_path_buf)
}

/// `<cache>/public-logs/<file>`, or an error that says how to fetch the public corpus.
pub fn public_log(file: &str) -> Result<PathBuf, String> {
    let (cache, how) = cache_dir();
    let path = cache.join("public-logs").join(file);
    if path.is_file() {
        return Ok(path);
    }
    Err(format!(
        "public corpus log {file} is not in {} (cache chosen from {how}); \
         download the public corpus with `cargo xtask corpus fetch`, or set ${CACHE_VARIABLE} \
         to the cache that holds it",
        cache.join("public-logs").display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_resolves_to_its_main_checkout() {
        let dir = std::env::temp_dir().join(format!("asrs-cache-test-{}", std::process::id()));
        let main = dir.join("main");
        let worktree = dir.join("wt");
        fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        let found = main_checkout(&worktree).unwrap();
        assert_eq!(found, fs::canonicalize(&main).unwrap());
        assert_eq!(main_checkout(&main).unwrap(), main);
        fs::remove_dir_all(&dir).unwrap();
    }
}
