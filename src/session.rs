//! Session name validation and default derivation.
//!
//! Each session maps to a remote working directory at `~/relocal/<session-name>/`
//! and a pair of FIFOs at `~/relocal/.fifos/<session-name>-{request,ack}`. The
//! name is embedded in filesystem paths, so it must be restricted to safe characters.

use std::path::Path;

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

/// Derives a default session name from a directory path by taking its final
/// component (e.g., `/home/user/my-project` → `my-project`). Returns an error
/// if the directory name contains invalid characters.
pub fn default_session_name(path: &Path) -> Result<String> {
    let name =
        path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::InvalidSessionName {
                name: path.display().to_string(),
                reason: "cannot derive session name from directory path".to_string(),
            })?;

    validate_session_name(name)?;
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn default_from_directory() {
        let path = Path::new("/home/user/my-project");
        assert_eq!(default_session_name(path).unwrap(), "my-project");
    }

    #[test]
    fn default_from_invalid_directory_name() {
        let path = Path::new("/home/user/my.project");
        assert!(default_session_name(path).is_err());
    }

    #[test]
    fn default_from_root_path() {
        let path = Path::new("/");
        assert!(default_session_name(path).is_err());
    }
}
