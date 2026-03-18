//! `relocal status [session-name]` — shows information about a session.
//!
//! Checks the remote for: working directory existence and tool installation.
//! All checks are done via SSH through the [`CommandRunner`] trait.

use tracing::info;

use crate::config::Config;
use crate::error::Result;
use crate::runner::CommandRunner;
use crate::ssh;

/// Prints session status.
pub fn run(runner: &dyn CommandRunner, config: &Config, session_name: &str) -> Result<()> {
    info!("Session:    {session_name}");
    info!("Remote:     {}", config.remote);
    info!("Remote dir: {}", ssh::remote_work_dir(session_name));

    let dir_exists = ssh::run_status_check(
        runner,
        &config.remote,
        &ssh::check_work_dir_exists(session_name),
    )?;
    info!(
        "Directory:  {}",
        if dir_exists { "exists" } else { "not found" }
    );

    let claude_installed =
        ssh::run_status_check(runner, &config.remote, &ssh::check_claude_installed())?;
    info!(
        "Claude:     {}",
        if claude_installed {
            "installed"
        } else {
            "not installed"
        }
    );

    let codex_installed =
        ssh::run_status_check(runner, &config.remote, &ssh::check_codex_installed())?;
    info!(
        "Codex:      {}",
        if codex_installed {
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
    fn checks_all_conditions() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_TRUE.into())); // dir
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_TRUE.into())); // claude
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_TRUE.into())); // codex

        run(&mock, &test_config(), "my-session").unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 3);

        for i in &inv {
            match i {
                Invocation::Ssh { remote, .. } => assert_eq!(remote, "user@host"),
                _ => panic!("expected Ssh"),
            }
        }

        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("test -d"));
                assert!(command.contains("my-session"));
            }
            _ => panic!("expected Ssh"),
        }

        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v claude"));
            }
            _ => panic!("expected Ssh"),
        }

        match &inv[2] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v codex"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn reports_when_everything_exists() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_TRUE.into()));
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_TRUE.into()));
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_TRUE.into()));

        run(&mock, &test_config(), "s1").unwrap();
    }

    #[test]
    fn reports_when_nothing_exists() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Ok(ssh::STATUS_CHECK_FALSE.into()));

        run(&mock, &test_config(), "s1").unwrap();
    }
}
