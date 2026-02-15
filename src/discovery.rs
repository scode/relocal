//! Repo root discovery — checks only the current directory for `relocal.toml`.
//!
//! Unlike tools that walk up the directory tree (git, cargo), relocal intentionally
//! only checks the given directory. This prevents accidentally discovering a
//! `relocal.toml` high in the tree and syncing an unexpectedly large directory.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Checks whether `start` contains a `relocal.toml` file.
///
/// Unlike tools that walk up the directory tree, this intentionally only
/// checks the given directory. This prevents accidentally discovering a
/// `relocal.toml` high up the tree (e.g. in `$HOME`) which would cause
/// the tool to sync an unexpectedly large directory with `--delete`.
pub fn find_repo_root(start: &Path) -> Result<PathBuf> {
    if start.join("relocal.toml").is_file() {
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
    fn found_in_current_dir() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("relocal.toml"), "remote = \"u@h\"").unwrap();
        assert_eq!(find_repo_root(tmp.path()).unwrap(), tmp.path());
    }

    #[test]
    fn not_found() {
        let tmp = TempDir::new().unwrap();
        let err = find_repo_root(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound { .. }));
    }

    #[test]
    fn does_not_walk_up_to_parent() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("relocal.toml"), "remote = \"u@h\"").unwrap();
        let child = tmp.path().join("subdir");
        fs::create_dir(&child).unwrap();

        // Must NOT find relocal.toml in parent — only checks the given directory
        let err = find_repo_root(&child).unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound { .. }));
    }
}
