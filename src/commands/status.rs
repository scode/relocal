//! `relocal status [session-name]` — shows information about a session.
//!
//! Checks the remote for: working directory existence and Claude installation.
//! All checks are done via SSH through the [`CommandRunner`] trait.

use crate::config::Config;
use crate::error::Result;
use crate::runner::CommandRunner;
use crate::ssh;

/// Prints session status to stderr.
pub fn run(runner: &dyn CommandRunner, config: &Config, session_name: &str) -> Result<()> {
    eprintln!("Session:    {session_name}");
    eprintln!("Remote:     {}", config.remote);
    eprintln!("Remote dir: {}", ssh::remote_work_dir(session_name));

    let dir_exists = ssh::run_status_check(
        runner,
        &config.remote,
        &ssh::check_work_dir_exists(session_name),
    )?;
    eprintln!(
        "Directory:  {}",
        if dir_exists { "exists" } else { "not found" }
    );

    let claude_installed =
        ssh::run_status_check(runner, &config.remote, &ssh::check_claude_installed())?;
    eprintln!(
        "Claude:     {}",
        if claude_installed {
            "installed"
        } else {
            "not installed"
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
    fn checks_both_conditions() {
        let mock = MockRunner::new();
        // check_work_dir_exists
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_TRUE.into()));
        // check_claude_installed
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_TRUE.into()));

        run(&mock, &test_config(), "my-session").unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 2);

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
    }

    #[test]
    fn reports_when_everything_exists() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_TRUE.into()));
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_TRUE.into()));

        run(&mock, &test_config(), "s1").unwrap();
    }

    #[test]
    fn reports_when_nothing_exists() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_FALSE.into()));

        run(&mock, &test_config(), "s1").unwrap();
    }
}
