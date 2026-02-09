//! `relocal status [session-name]` — shows information about a session.
//!
//! Checks the remote for: working directory existence, Claude installation,
//! and active FIFOs (indicating a running session). All checks are done via
//! SSH through the [`CommandRunner`] trait.

use crate::config::Config;
use crate::error::Result;
use crate::runner::CommandRunner;
use crate::ssh;

/// Prints session status to stderr.
pub fn run(runner: &dyn CommandRunner, config: &Config, session_name: &str) -> Result<()> {
    eprintln!("Session:    {session_name}");
    eprintln!("Remote:     {}", config.remote);
    eprintln!("Remote dir: {}", ssh::remote_work_dir(session_name));

    let dir_exists = runner
        .run_ssh(&config.remote, &ssh::check_work_dir_exists(session_name))?
        .status
        .success();
    eprintln!(
        "Directory:  {}",
        if dir_exists { "exists" } else { "not found" }
    );

    let claude_installed = runner
        .run_ssh(&config.remote, &ssh::check_claude_installed())?
        .status
        .success();
    eprintln!(
        "Claude:     {}",
        if claude_installed {
            "installed"
        } else {
            "not installed"
        }
    );

    let fifos_exist = runner
        .run_ssh(&config.remote, &ssh::check_fifos_exist(session_name))?
        .status
        .success();
    eprintln!(
        "FIFOs:      {}",
        if fifos_exist {
            "exist (session may be active)"
        } else {
            "not found"
        }
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Invocation, MockResponse, MockRunner};

    fn test_config() -> Config {
        Config::parse("remote = \"user@host\"").unwrap()
    }

    #[test]
    fn checks_all_three_conditions() {
        let mock = MockRunner::new();
        // check_work_dir_exists
        mock.add_response(MockResponse::Ok(String::new()));
        // check_claude_installed
        mock.add_response(MockResponse::Ok("/usr/local/bin/claude\n".into()));
        // check_fifos_exist
        mock.add_response(MockResponse::Fail(String::new()));

        run(&mock, &test_config(), "my-session").unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 3);

        // All commands go to the right remote
        for i in &inv {
            match i {
                Invocation::Ssh { remote, .. } => assert_eq!(remote, "user@host"),
                _ => panic!("expected Ssh"),
            }
        }

        // First: directory check
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("test -d"));
                assert!(command.contains("my-session"));
            }
            _ => panic!("expected Ssh"),
        }

        // Second: claude check
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v claude"));
            }
            _ => panic!("expected Ssh"),
        }

        // Third: fifos check
        match &inv[2] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("test -e"));
                assert!(command.contains("my-session"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn reports_when_everything_exists() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));
        mock.add_response(MockResponse::Ok(String::new()));
        mock.add_response(MockResponse::Ok(String::new()));

        // Should not error even when FIFOs exist
        run(&mock, &test_config(), "s1").unwrap();
    }

    #[test]
    fn reports_when_nothing_exists() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new()));
        mock.add_response(MockResponse::Fail(String::new()));
        mock.add_response(MockResponse::Fail(String::new()));

        // Should not error when nothing exists
        run(&mock, &test_config(), "s1").unwrap();
    }
}
