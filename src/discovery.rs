//! Repo root discovery — checks the current directory for `relocal.toml` or `.git`.
//!
//! Unlike tools that walk up the directory tree (git, cargo), relocal intentionally
//! only checks the given directory. This prevents accidentally syncing an
//! unexpectedly large directory with `rsync --delete`.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Checks whether `dir` contains a valid `.git` marker.
///
/// Accepts either a `.git` directory containing a `HEAD` file (normal repo)
/// or a `.git` file starting with `gitdir:` (worktree). Rejects stray files
/// or directories named `.git` that aren't actual git repos.
pub fn is_git_root(dir: &Path) -> bool {
    let git_path = dir.join(".git");
    if git_path.is_dir() {
        return git_path.join("HEAD").is_file();
    }
    if git_path.is_file() {
        return std::fs::read_to_string(&git_path)
            .map(|s| s.starts_with("gitdir:"))
            .unwrap_or(false);
    }
    false
}

/// Finds the repo root by checking `start` for known markers.
///
/// Checks for `relocal.toml` first, then a valid `.git` marker. Does NOT
/// walk up the directory tree — only checks the given directory.
pub fn find_repo_root(start: &Path) -> Result<PathBuf> {
    if start.join("relocal.toml").is_file() || is_git_root(start) {
        return Ok(start.to_path_buf());
    }
    Err(Error::ConfigNotFound {
        start_dir: start.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn found_via_relocal_toml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("relocal.toml"), "remote = \"u@h\"").unwrap();
        assert_eq!(find_repo_root(tmp.path()).unwrap(), tmp.path());
    }

    #[test]
    fn found_via_git_dir_with_head() {
        let tmp = TempDir::new().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(find_repo_root(tmp.path()).unwrap(), tmp.path());
    }

    #[test]
    fn found_via_git_worktree_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join(".git"),
            "gitdir: /some/path/.git/worktrees/foo",
        )
        .unwrap();
        assert_eq!(find_repo_root(tmp.path()).unwrap(), tmp.path());
    }

    #[test]
    fn rejects_stray_git_dir_without_head() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        // No HEAD file — not a real git repo
        let err = find_repo_root(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound { .. }));
    }

    #[test]
    fn rejects_stray_git_file_without_gitdir() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".git"), "not a worktree").unwrap();
        let err = find_repo_root(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound { .. }));
    }

    #[test]
    fn relocal_toml_sufficient_without_git() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("relocal.toml"), "remote = \"u@h\"").unwrap();
        // No .git at all — still found via relocal.toml
        assert_eq!(find_repo_root(tmp.path()).unwrap(), tmp.path());
    }

    #[test]
    fn not_found() {
        let tmp = TempDir::new().unwrap();
        let err = find_repo_root(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound { .. }));
    }

    #[test]
    fn does_not_walk_up_to_parent_relocal_toml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("relocal.toml"), "remote = \"u@h\"").unwrap();
        let child = tmp.path().join("subdir");
        fs::create_dir(&child).unwrap();
        let err = find_repo_root(&child).unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound { .. }));
    }

    #[test]
    fn does_not_walk_up_to_parent_git() {
        let tmp = TempDir::new().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let child = tmp.path().join("subdir");
        fs::create_dir(&child).unwrap();
        let err = find_repo_root(&child).unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound { .. }));
    }

    // --- is_git_root tests ---

    #[test]
    fn is_git_root_real_repo() {
        let tmp = TempDir::new().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert!(is_git_root(tmp.path()));
    }

    #[test]
    fn is_git_root_worktree() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".git"), "gitdir: /some/path").unwrap();
        assert!(is_git_root(tmp.path()));
    }

    #[test]
    fn is_git_root_empty_dir() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        assert!(!is_git_root(tmp.path()));
    }

    #[test]
    fn is_git_root_stray_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".git"), "random content").unwrap();
        assert!(!is_git_root(tmp.path()));
    }

    #[test]
    fn is_git_root_no_git() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_git_root(tmp.path()));
    }
}
