//! Test utilities shared across unit tests in multiple modules.
//!
//! This module is only compiled under `#[cfg(test)]`. It provides [`MockRunner`],
//! a configurable fake [`CommandRunner`] that records all invocations and returns
//! pre-configured responses, enabling orchestration tests without real SSH or rsync.

use std::cell::RefCell;
use std::process::ExitStatus;

use crate::error::{Error, Result};
use crate::runner::{CommandOutput, CommandRunner};

/// What kind of command was invoked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    Ssh { remote: String, command: String },
    SshInteractive { remote: String, command: String },
    Rsync { args: Vec<String> },
    Local { program: String, args: Vec<String> },
}

/// Pre-configured result for a single mock invocation.
pub enum MockResponse {
    /// Return a successful `CommandOutput` with the given stdout.
    Ok(String),
    /// Return a successful `CommandOutput` with given stdout and stderr.
    OkWithStderr(String, String),
    /// Return a `CommandOutput` with a non-zero exit status.
    Fail(String),
    /// Return an `Err(Error::CommandFailed { .. })`.
    Err(String),
}

/// Creates a successful (code 0) `ExitStatus` by running `true`.
fn success_status() -> ExitStatus {
    std::process::Command::new("true")
        .status()
        .expect("failed to run `true`")
}

/// Creates a failing (non-zero) `ExitStatus` by running `false`.
fn failure_status() -> ExitStatus {
    std::process::Command::new("false")
        .status()
        .expect("failed to run `false`")
}

/// A fake [`CommandRunner`] for unit tests.
///
/// Enqueue expected responses with [`MockRunner::add_response`]. Each call to
/// any `CommandRunner` method pops the next response from the front of the queue
/// and records the invocation. After the test, inspect [`MockRunner::invocations`]
/// to verify the correct commands were issued in the expected order.
///
/// Panics if a method is called with no responses remaining.
pub struct MockRunner {
    invocations: RefCell<Vec<Invocation>>,
    responses: RefCell<Vec<MockResponse>>,
}

impl Default for MockRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl MockRunner {
    pub fn new() -> Self {
        Self {
            invocations: RefCell::new(Vec::new()),
            responses: RefCell::new(Vec::new()),
        }
    }

    pub fn add_response(&self, response: MockResponse) {
        self.responses.borrow_mut().push(response);
    }

    pub fn invocations(&self) -> Vec<Invocation> {
        self.invocations.borrow().clone()
    }

    fn next_response(&self) -> MockResponse {
        let mut responses = self.responses.borrow_mut();
        assert!(
            !responses.is_empty(),
            "MockRunner: no more responses queued (add more with add_response)"
        );
        responses.remove(0)
    }

    fn respond(&self, response: MockResponse) -> Result<CommandOutput> {
        match response {
            MockResponse::Ok(stdout) => Ok(CommandOutput {
                stdout,
                stderr: String::new(),
                status: success_status(),
            }),
            MockResponse::OkWithStderr(stdout, stderr) => Ok(CommandOutput {
                stdout,
                stderr,
                status: success_status(),
            }),
            MockResponse::Fail(stderr) => Ok(CommandOutput {
                stdout: String::new(),
                stderr,
                status: failure_status(),
            }),
            MockResponse::Err(message) => Err(Error::CommandFailed {
                command: "mock".to_string(),
                message,
            }),
        }
    }
}

impl CommandRunner for MockRunner {
    fn run_ssh(&self, remote: &str, command: &str) -> Result<CommandOutput> {
        self.invocations.borrow_mut().push(Invocation::Ssh {
            remote: remote.to_string(),
            command: command.to_string(),
        });
        let response = self.next_response();
        self.respond(response)
    }

    fn run_ssh_interactive(&self, remote: &str, command: &str) -> Result<ExitStatus> {
        self.invocations
            .borrow_mut()
            .push(Invocation::SshInteractive {
                remote: remote.to_string(),
                command: command.to_string(),
            });
        let response = self.next_response();
        match response {
            MockResponse::Ok(_) | MockResponse::OkWithStderr(_, _) => Ok(success_status()),
            MockResponse::Fail(_) => Ok(failure_status()),
            MockResponse::Err(message) => Err(Error::CommandFailed {
                command: "mock".to_string(),
                message,
            }),
        }
    }

    fn run_rsync(&self, args: &[String]) -> Result<CommandOutput> {
        self.invocations.borrow_mut().push(Invocation::Rsync {
            args: args.to_vec(),
        });
        let response = self.next_response();
        self.respond(response)
    }

    fn run_local(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        self.invocations.borrow_mut().push(Invocation::Local {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        });
        let response = self.next_response();
        self.respond(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_invocations_in_order() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok("out1".into()));
        mock.add_response(MockResponse::Ok("out2".into()));

        mock.run_ssh("user@host", "ls").unwrap();
        mock.run_local("echo", &["hi"]).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 2);
        assert_eq!(
            inv[0],
            Invocation::Ssh {
                remote: "user@host".into(),
                command: "ls".into()
            }
        );
        assert_eq!(
            inv[1],
            Invocation::Local {
                program: "echo".into(),
                args: vec!["hi".into()]
            }
        );
    }

    #[test]
    fn ok_response_returns_stdout() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok("hello\n".into()));

        let out = mock.run_ssh("u@h", "echo hello").unwrap();
        assert_eq!(out.stdout, "hello\n");
        assert!(out.status.success());
    }

    #[test]
    fn fail_response_returns_nonzero() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail("bad".into()));

        let out = mock.run_rsync(&["--help".into()]).unwrap();
        assert!(!out.status.success());
        assert_eq!(out.stderr, "bad");
    }

    #[test]
    fn err_response_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Err("boom".into()));

        let result = mock.run_local("x", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn interactive_ssh_success() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));

        let status = mock.run_ssh_interactive("u@h", "claude").unwrap();
        assert!(status.success());

        let inv = mock.invocations();
        assert_eq!(
            inv[0],
            Invocation::SshInteractive {
                remote: "u@h".into(),
                command: "claude".into()
            }
        );
    }

    #[test]
    fn interactive_ssh_failure() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new()));

        let status = mock.run_ssh_interactive("u@h", "claude").unwrap();
        assert!(!status.success());
    }

    #[test]
    #[should_panic(expected = "no more responses queued")]
    fn panics_when_no_responses() {
        let mock = MockRunner::new();
        let _ = mock.run_ssh("u@h", "ls");
    }

    #[test]
    fn ok_with_stderr() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::OkWithStderr("out".into(), "warning".into()));

        let out = mock.run_local("cmd", &[]).unwrap();
        assert_eq!(out.stdout, "out");
        assert_eq!(out.stderr, "warning");
        assert!(out.status.success());
    }
}
