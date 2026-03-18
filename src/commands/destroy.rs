//! `relocal destroy [session-name]` — removes a session's remote state.
//!
//! Deletes the remote working directory after prompting for confirmation.
//! Refuses to proceed if a daemon is running for the session.

use tracing::{info, warn};

use crate::config::Config;
use crate::daemon_client;
use crate::error::{Error, Result};
use crate::runner::CommandRunner;
use crate::ssh;

/// Removes a session's remote working directory.
///
/// If `confirm` is true, prompts the user for confirmation before proceeding.
/// Pass `false` in tests to skip the interactive prompt.
///
/// If `check_daemon` is true, refuses to proceed when a daemon is running
/// for this session. Pass `false` in tests to skip the daemon check.
pub fn run(
    runner: &dyn CommandRunner,
    config: &Config,
    session_name: &str,
    confirm: bool,
    check_daemon: bool,
) -> Result<()> {
    if check_daemon && daemon_client::is_daemon_running(session_name, &config.remote) {
        return Err(Error::Remote {
            remote: config.remote.clone(),
            message: format!(
                "session '{session_name}' has a running daemon. \
                 Exit all relocal claude/codex/ssh sessions for this project first, \
                 then retry."
            ),
        });
    }

    let dir_exists = ssh::run_status_check(
        runner,
        &config.remote,
        &ssh::check_work_dir_exists(session_name),
    )?;
    if !dir_exists {
        return Err(Error::Remote {
            remote: config.remote.clone(),
            message: format!("session '{session_name}' not found. No working directory exists."),
        });
    }

    if confirm {
        let prompt = format!(
            "Remove session '{session_name}' on {}? This deletes {}.",
            config.remote,
            ssh::remote_work_dir(session_name)
        );
        let confirmed = dialoguer::Confirm::new()
            .with_prompt(prompt)
            .default(false)
            .interact()
            .map_err(std::io::Error::other)?;

        if !confirmed {
            info!("Aborted.");
            return Ok(());
        }
    }

    info!("Removing remote working directory...");
    runner
        .run_ssh(&config.remote, &ssh::rm_work_dir(session_name))?
        .check("rm work dir")?;

    info!("Removing lock file...");
    runner
        .run_ssh(&config.remote, &ssh::remove_lock_file(session_name))?
        .check("rm lock file")?;

    let mut local_cleanup_failed = false;
    for path in [
        ssh::daemon_socket_path(session_name, &config.remote),
        ssh::daemon_flock_path(session_name, &config.remote),
        ssh::daemon_log_path(session_name, &config.remote),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                warn!("failed to remove {}: {e}", path.display());
                local_cleanup_failed = true;
            }
        }
    }

    if local_cleanup_failed {
        return Err(Error::CommandFailed {
            command: "destroy".to_string(),
            message: "remote session destroyed but some local daemon files could not be removed"
                .to_string(),
        });
    }

    info!("Session '{session_name}' destroyed.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::{STATUS_CHECK_FALSE, STATUS_CHECK_TRUE};
    use crate::test_support::{Invocation, MockResponse, MockRunner};

    fn test_config() -> Config {
        Config::parse("remote = \"user@host\"").unwrap()
    }

    #[test]
    fn removes_working_dir_and_lock() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));
        mock.add_response(MockResponse::Ok(String::new()));
        mock.add_response(MockResponse::Ok(String::new()));

        run(&mock, &test_config(), "my-session", false, false).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 3);

        match &inv[1] {
            Invocation::Ssh { remote, command } => {
                assert_eq!(remote, "user@host");
                assert!(command.contains("rm -rf"));
                assert!(command.contains("my-session"));
            }
            _ => panic!("expected Ssh for rm work dir"),
        }

        match &inv[2] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("rm -f"));
                assert!(command.contains(".locks/my-session.lock"));
            }
            _ => panic!("expected Ssh for rm lock"),
        }
    }

    #[test]
    fn targets_correct_remote() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));
        mock.add_response(MockResponse::Ok(String::new()));
        mock.add_response(MockResponse::Ok(String::new()));

        let config = Config::parse("remote = \"deploy@prod\"").unwrap();
        run(&mock, &config, "s1", false, false).unwrap();

        let inv = mock.invocations();
        for i in &inv {
            match i {
                Invocation::Ssh { remote, .. } => assert_eq!(remote, "deploy@prod"),
                _ => panic!("expected Ssh"),
            }
        }
    }

    #[test]
    fn rm_work_dir_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));
        mock.add_response(MockResponse::Fail("permission denied".into()));

        let result = run(&mock, &test_config(), "s1", false, false);
        assert!(result.is_err());
    }

    #[test]
    fn rm_lock_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));
        mock.add_response(MockResponse::Ok(String::new()));
        mock.add_response(MockResponse::Fail("permission denied".into()));

        let result = run(&mock, &test_config(), "s1", false, false);
        assert!(result.is_err());
    }

    #[test]
    fn nonexistent_session_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));

        let result = run(&mock, &test_config(), "no-such-session", false, false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
