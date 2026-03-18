//! `relocal log` — tail the daemon's log file.

use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::ssh;

/// Execs `tail -f` on the daemon log file for the given session.
///
/// Does not return on success (the process is replaced by `tail`).
pub fn run(config: &Config, session_name: &str) -> Result<()> {
    let log_path = ssh::daemon_log_path(session_name, &config.remote);

    if !log_path.exists() {
        return Err(Error::CommandFailed {
            command: "log".to_string(),
            message: format!(
                "no daemon log found for session '{session_name}'. Is the session running?"
            ),
        });
    }

    let err = Command::new("tail")
        .args(["-f", &log_path.to_string_lossy()])
        .exec();
    Err(Error::CommandFailed {
        command: "tail".to_string(),
        message: format!("{err}"),
    })
}
