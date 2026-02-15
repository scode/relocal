//! Trait abstraction for executing external commands (ssh, rsync, local processes).
//!
//! All orchestration code (sidecar, `start`, `install`, `sync`) uses
//! [`CommandRunner`] rather than calling `std::process::Command` directly.
//! This enables unit-testing command sequences with a mock implementation
//! that records invocations and returns canned results, without needing
//! real SSH or rsync.

use std::process::{Command, ExitStatus, Stdio};

use shell_quote::{Bash, QuoteRefExt};

use std::path::Path;

use crate::error::{Error, Result};
use crate::rsync::{Direction, RsyncParams};

/// Output captured from a non-interactive command.
#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
}

/// Abstraction over shelling out to ssh, rsync, and local processes.
///
/// Each method corresponds to a distinct invocation pattern:
/// - `run_ssh`: non-interactive `ssh user@host "command"`, captures output.
/// - `run_ssh_interactive`: `ssh -t user@host "command"`, inherits the terminal.
/// - `run_rsync`: runs rsync with the given argument list, captures output.
/// - `run_local`: runs an arbitrary local program, captures output.
pub trait CommandRunner {
    fn run_ssh(&self, remote: &str, command: &str) -> Result<CommandOutput>;
    fn run_ssh_interactive(&self, remote: &str, command: &str) -> Result<ExitStatus>;
    fn run_rsync(&self, params: &RsyncParams) -> Result<CommandOutput>;
    fn run_local(&self, program: &str, args: &[&str]) -> Result<CommandOutput>;
}

/// Production implementation that shells out via `std::process::Command`.
pub struct ProcessRunner;

/// Wraps a command in `bash -lc <quoted-command>` so it runs as a login shell.
///
/// Non-interactive SSH sessions don't source `~/.profile` or `~/.bash_profile`,
/// which means user-installed tools (e.g. `~/.local/bin/claude`) aren't on PATH.
/// Wrapping in a login shell ensures the full user environment is available.
fn login_shell_wrap(command: &str) -> String {
    let quoted: String = command.quoted(Bash);
    format!("bash -lc {quoted}")
}

/// Validates that the local pull target is a relocal repo root.
///
/// Canonicalizes the path and checks for `relocal.toml` — the same marker
/// [`find_repo_root`](crate::discovery::find_repo_root) uses. This prevents
/// `rsync --delete` from wiping an unintended directory if a bug in
/// higher-level code passes the wrong `repo_root`.
fn validate_local_pull_target(local_path: &Path) -> Result<()> {
    let canonical = local_path
        .canonicalize()
        .map_err(|e| Error::CommandFailed {
            command: "rsync".to_string(),
            message: format!(
                "refusing to pull: local path {} cannot be resolved: {e}",
                local_path.display()
            ),
        })?;
    if !canonical.join("relocal.toml").is_file() {
        return Err(Error::CommandFailed {
            command: "rsync".to_string(),
            message: format!(
                "refusing to pull: {} does not contain relocal.toml",
                canonical.display()
            ),
        });
    }
    Ok(())
}

impl CommandRunner for ProcessRunner {
    fn run_ssh(&self, remote: &str, command: &str) -> Result<CommandOutput> {
        let wrapped = login_shell_wrap(command);
        let output = Command::new("ssh").args([remote, &wrapped]).output()?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        })
    }

    fn run_ssh_interactive(&self, remote: &str, command: &str) -> Result<ExitStatus> {
        let wrapped = login_shell_wrap(command);
        let status = Command::new("ssh")
            .args(["-t", remote, &wrapped])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        Ok(status)
    }

    fn run_rsync(&self, params: &RsyncParams) -> Result<CommandOutput> {
        if params.direction() == Direction::Pull {
            validate_local_pull_target(params.local_path())?;
        }
        let output = Command::new("rsync").args(params.args()).output()?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        })
    }

    fn run_local(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        let output = Command::new(program).args(args).output()?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_params(direction: Direction, local_path: PathBuf) -> RsyncParams {
        RsyncParams::for_test(vec!["--help".to_string()], direction, local_path)
    }

    #[test]
    fn pull_refused_without_toml() {
        let dir = tempfile::tempdir().unwrap();
        let result = validate_local_pull_target(dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("relocal.toml"));
    }

    #[test]
    fn pull_allowed_with_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("relocal.toml"), "remote = \"u@h\"").unwrap();
        let result = validate_local_pull_target(dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn push_skips_validation() {
        let runner = ProcessRunner;
        // Use a nonexistent path — push should not validate it.
        // A pull with this path would fail validation, but push must not.
        let params = make_params(Direction::Push, PathBuf::from("/nonexistent/path"));
        let result = runner.run_rsync(&params);
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(
                !msg.contains("relocal.toml"),
                "push should not validate local path"
            );
        }
    }

    #[test]
    fn run_rsync_pull_rejects_invalid_destination() {
        let runner = ProcessRunner;
        let dir = tempfile::tempdir().unwrap();
        // No relocal.toml — ProcessRunner::run_rsync must refuse before invoking rsync.
        let params = make_params(Direction::Pull, dir.path().to_path_buf());
        let err = runner.run_rsync(&params).unwrap_err().to_string();
        assert!(err.contains("relocal.toml"));
    }

    #[test]
    fn pull_refused_nonexistent_path() {
        let result = validate_local_pull_target(&PathBuf::from("/nonexistent/path/xyz"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cannot be resolved"));
    }

    #[test]
    fn run_local_captures_stdout() {
        let runner = ProcessRunner;
        let out = runner.run_local("echo", &["hello world"]).unwrap();
        assert_eq!(out.stdout.trim(), "hello world");
        assert!(out.status.success());
    }

    #[test]
    fn run_local_captures_stderr() {
        let runner = ProcessRunner;
        let out = runner.run_local("sh", &["-c", "echo oops >&2"]).unwrap();
        assert_eq!(out.stderr.trim(), "oops");
    }

    #[test]
    fn run_local_failing_command() {
        let runner = ProcessRunner;
        let out = runner.run_local("false", &[]).unwrap();
        assert!(!out.status.success());
    }

    #[test]
    fn run_local_nonexistent_program() {
        let runner = ProcessRunner;
        let result = runner.run_local("this-program-does-not-exist-xyz", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn login_shell_wrap_simple_command() {
        let wrapped = login_shell_wrap("command -v claude");
        assert!(wrapped.starts_with("bash -lc "));
        assert!(wrapped.contains("command -v claude"));
    }

    #[test]
    fn login_shell_wrap_with_single_quotes() {
        let wrapped = login_shell_wrap("echo 'hello' > /tmp/out");
        assert!(wrapped.starts_with("bash -lc "));
        // The command should be properly quoted so bash -lc receives the full command
        assert!(wrapped.contains("hello"));
    }

    #[test]
    fn login_shell_wrap_with_heredoc() {
        let cmd = "cat > /tmp/test << 'EOF'\n{\"key\": \"value\"}\nEOF";
        let wrapped = login_shell_wrap(cmd);
        assert!(wrapped.starts_with("bash -lc "));
    }
}
