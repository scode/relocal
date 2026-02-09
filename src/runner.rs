//! Trait abstraction for executing external commands (ssh, rsync, local processes).
//!
//! All orchestration code (sidecar, `start`, `install`, `sync`) uses
//! [`CommandRunner`] rather than calling `std::process::Command` directly.
//! This enables unit-testing command sequences with a mock implementation
//! that records invocations and returns canned results, without needing
//! real SSH or rsync.

use std::process::{Command, ExitStatus, Stdio};

use crate::error::Result;

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
    fn run_rsync(&self, args: &[String]) -> Result<CommandOutput>;
    fn run_local(&self, program: &str, args: &[&str]) -> Result<CommandOutput>;
}

/// Production implementation that shells out via `std::process::Command`.
pub struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run_ssh(&self, remote: &str, command: &str) -> Result<CommandOutput> {
        let output = Command::new("ssh").args([remote, command]).output()?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        })
    }

    fn run_ssh_interactive(&self, remote: &str, command: &str) -> Result<ExitStatus> {
        let status = Command::new("ssh")
            .args(["-t", remote, command])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        Ok(status)
    }

    fn run_rsync(&self, args: &[String]) -> Result<CommandOutput> {
        let output = Command::new("rsync").args(args).output()?;
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
}
