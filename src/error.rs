//! Shared error types used across all relocal modules.
//!
//! Every module returns [`Result<T>`] which uses [`Error`] as the error type.
//! Error messages are user-facing and include actionable recovery suggestions
//! (e.g., "run `relocal init`").

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

/// All errors that relocal can produce.
///
/// Variants carry enough context to produce helpful user-facing messages:
/// paths, session names, remote hosts, and recovery instructions.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "relocal.toml not found in {start_dir}. Run relocal from the project root, or run `relocal init` to create one."
    )]
    ConfigNotFound { start_dir: PathBuf },

    #[error("failed to parse relocal.toml: {reason}")]
    ConfigParse { reason: String },

    #[error("invalid session name {name:?}: {reason}")]
    InvalidSessionName { name: String, reason: String },

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("command failed: {command}: {message}")]
    CommandFailed { command: String, message: String },

    #[error("remote error ({remote}): {message}")]
    Remote { remote: String, message: String },

    #[error("stale session {session}: FIFOs already exist. Another session may be running. Use `relocal destroy {session}` if the previous session crashed.")]
    StaleSession { session: String },

    #[error("refusing to pull: remote session {session} failed git fsck (not a git repo or repository is corrupted).\nStderr: {stderr}")]
    RemoteGitFsckFailed { session: String, stderr: String },
}
