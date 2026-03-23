//! Session name validation and default derivation.
//!
//! Each session maps to a remote working directory at `~/relocal/<session-name>/`.
//! The name is embedded in filesystem paths, so it must be restricted to safe
//! characters.

use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Validates that a session name contains only alphanumeric characters, hyphens,
/// and underscores. This prevents path traversal and shell injection issues since
/// the name is used in remote paths and SSH commands.
pub fn validate_session_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::InvalidSessionName {
            name: name.to_string(),
            reason: "must not be empty".to_string(),
        });
    }

    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::InvalidSessionName {
            name: name.to_string(),
            reason: "must contain only alphanumeric characters, hyphens, and underscores"
                .to_string(),
        });
    }

    Ok(())
}

/// Derives a default session name with a hash suffix to prevent collisions
/// between different repos or checkouts.
///
/// Format: `<dirname>-<8-hex-chars>`. The hash is derived from the canonical
/// path and the git origin URL. The dirname prefix keeps it human-readable;
/// the hash prevents collisions. Returns an error if the directory name
/// contains characters invalid for session names.
pub fn hashed_session_name(repo_root: &Path) -> Result<String> {
    let dirname = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::InvalidSessionName {
            name: repo_root.display().to_string(),
            reason: "cannot derive session name from directory path".to_string(),
        })?;

    validate_session_name(dirname)?;

    let canonical = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let origin = git_origin_url(repo_root);

    let hash = compute_hash(canonical.as_os_str().as_encoded_bytes(), origin.as_bytes());
    Ok(format!("{dirname}-{hash}"))
}

/// Reads the git origin URL for a repo, returning an empty string if
/// no origin is configured or git is not available.
fn git_origin_url(repo_root: &Path) -> String {
    Command::new("git")
        .current_dir(repo_root)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// First 8 hex chars of SHA-256(path + \0 + origin).
fn compute_hash(path_bytes: &[u8], origin_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path_bytes);
    hasher.update(b"\0");
    hasher.update(origin_bytes);
    let digest = hasher.finalize();
    format!(
        "{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Runs a git command and asserts it succeeds.
    fn git(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {:?} failed", args);
    }

    #[test]
    fn valid_names() {
        for name in ["my-session", "session_1", "foo", "A-B_C-123"] {
            assert!(
                validate_session_name(name).is_ok(),
                "expected valid: {name}"
            );
        }
    }

    #[test]
    fn invalid_space() {
        assert!(validate_session_name("my session").is_err());
    }

    #[test]
    fn invalid_slash() {
        assert!(validate_session_name("a/b").is_err());
    }

    #[test]
    fn invalid_dot() {
        assert!(validate_session_name("a.b").is_err());
    }

    #[test]
    fn invalid_traversal() {
        assert!(validate_session_name("../escape").is_err());
    }

    #[test]
    fn invalid_empty() {
        assert!(validate_session_name("").is_err());
    }

    #[test]
    fn hashed_name_is_deterministic() {
        let path = Path::new("/home/user/my-project");
        let a = hashed_session_name(path).unwrap();
        let b = hashed_session_name(path).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn hashed_name_has_dirname_prefix() {
        let path = Path::new("/home/user/my-project");
        let name = hashed_session_name(path).unwrap();
        assert!(name.starts_with("my-project-"), "got: {name}");
    }

    #[test]
    fn hashed_name_format_is_dirname_dash_8hex() {
        let path = Path::new("/home/user/my-project");
        let name = hashed_session_name(path).unwrap();
        let parts: Vec<&str> = name.rsplitn(2, '-').collect();
        assert_eq!(parts[0].len(), 8, "suffix should be 8 hex chars: {name}");
        assert!(parts[0].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(parts[1], "my-project");
    }

    #[test]
    fn hash_is_8_hex_chars() {
        let h = compute_hash(b"/some/path", b"git@example.com:repo.git");
        assert_eq!(h.len(), 8);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hashed_name_different_paths_differ() {
        let a = compute_hash(b"/home/alice/repo", b"");
        let b = compute_hash(b"/home/bob/repo", b"");
        assert_ne!(a, b);
    }

    #[test]
    fn hashed_name_different_origins_differ() {
        let a = compute_hash(b"/same/path", b"git@github.com:alice/repo.git");
        let b = compute_hash(b"/same/path", b"git@github.com:bob/repo.git");
        assert_ne!(a, b);
    }

    #[test]
    fn hashed_name_non_existent_path_still_works() {
        // canonicalize falls back to the raw path for non-existent dirs
        let path = Path::new("/nonexistent/test-repo");
        let name = hashed_session_name(path).unwrap();
        assert!(name.starts_with("test-repo-"), "got: {name}");
    }

    #[test]
    fn hashed_name_rejects_invalid_dirname() {
        let path = Path::new("/home/user/my.project");
        assert!(hashed_session_name(path).is_err());
    }

    #[test]
    fn hashed_name_root_path_errors() {
        let path = Path::new("/");
        assert!(hashed_session_name(path).is_err());
    }

    #[test]
    fn hashed_name_reads_origin_from_git_repo() {
        let tmp = TempDir::new().unwrap();
        // Use a subdirectory with a valid session-name-compatible name,
        // since tempdir names contain dots which fail validation.
        let repo = tmp.path().join("test-repo");
        fs::create_dir(&repo).unwrap();

        git(&repo, &["init"]);
        git(
            &repo,
            &["remote", "add", "origin", "git@github.com:test/repo.git"],
        );

        let with_origin = hashed_session_name(&repo).unwrap();

        git(&repo, &["remote", "remove", "origin"]);

        let without_origin = hashed_session_name(&repo).unwrap();

        assert!(with_origin.starts_with("test-repo-"));
        assert!(without_origin.starts_with("test-repo-"));
        assert_ne!(with_origin, without_origin, "origin should affect the hash");
    }

    #[test]
    fn git_origin_url_returns_empty_for_non_repo() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(git_origin_url(tmp.path()), "");
    }

    #[test]
    fn git_origin_url_reads_configured_origin() {
        let tmp = TempDir::new().unwrap();
        git(tmp.path(), &["init"]);
        git(
            tmp.path(),
            &["remote", "add", "origin", "https://example.com/repo.git"],
        );

        assert_eq!(git_origin_url(tmp.path()), "https://example.com/repo.git");
    }
}
