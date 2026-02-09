//! Repo root discovery by walking up the directory tree looking for `relocal.toml`.
//!
//! This mirrors the common pattern used by tools like git (`.git/`) and cargo
//! (`Cargo.toml`): start from the current directory and walk upward until the
//! marker file is found. The nearest match wins.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Walks up from `start` looking for a directory containing `relocal.toml`.
/// Returns the first (nearest) directory that contains it, or an error if
/// none is found all the way up to the filesystem root.
pub fn find_repo_root(start: &Path) -> Result<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join("relocal.toml").is_file() {
            return Ok(current);
        }
        if !current.pop() {
            return Err(Error::ConfigNotFound {
                start_dir: start.to_path_buf(),
            });
        }
    }
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
    fn found_in_parent() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("relocal.toml"), "remote = \"u@h\"").unwrap();
        let child = tmp.path().join("subdir");
        fs::create_dir(&child).unwrap();
        assert_eq!(find_repo_root(&child).unwrap(), tmp.path());
    }

    #[test]
    fn found_in_grandparent() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("relocal.toml"), "remote = \"u@h\"").unwrap();
        let grandchild = tmp.path().join("a").join("b");
        fs::create_dir_all(&grandchild).unwrap();
        assert_eq!(find_repo_root(&grandchild).unwrap(), tmp.path());
    }

    #[test]
    fn not_found() {
        let tmp = TempDir::new().unwrap();
        let err = find_repo_root(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::ConfigNotFound { .. }));
    }

    #[test]
    fn nearest_wins() {
        let tmp = TempDir::new().unwrap();
        // relocal.toml in root
        fs::write(tmp.path().join("relocal.toml"), "remote = \"outer@h\"").unwrap();
        // relocal.toml in child (nearer)
        let child = tmp.path().join("inner");
        fs::create_dir(&child).unwrap();
        fs::write(child.join("relocal.toml"), "remote = \"inner@h\"").unwrap();

        assert_eq!(find_repo_root(&child).unwrap(), child);
    }
}
