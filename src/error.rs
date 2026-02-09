use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "relocal.toml not found (walked up from {start_dir}). Run `relocal init` to create one."
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
}
