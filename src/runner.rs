//! Trait abstraction for executing external commands (ssh, rsync, local processes).
//!
//! All orchestration code (sidecar, `start`, `install`, `sync`) uses
//! [`CommandRunner`] rather than calling `std::process::Command` directly.
//! This enables unit-testing command sequences with a mock implementation
//! that records invocations and returns canned results, without needing
//! real SSH or rsync.

use std::process::ExitStatus;

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
