//! `relocal destroy [session-name]` — removes a session's remote state.
//!
//! Deletes the remote working directory after prompting for confirmation.
//! Uses the [`CommandRunner`] trait for all SSH operations.

use tracing::info;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::runner::CommandRunner;
use crate::ssh;

/// Removes a session's remote working directory.
///
/// If `confirm` is true, prompts the user for confirmation before proceeding.
/// Pass `false` in tests to skip the interactive prompt.
pub fn run(
    runner: &dyn CommandRunner,
    config: &Config,
    session_name: &str,
    confirm: bool,
) -> Result<()> {
    // Check the session exists (use run_status_check to distinguish SSH
    // transport errors from "directory not found")
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
            eprintln!("Aborted.");
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

    eprintln!("Session '{session_name}' destroyed.");
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
        // dir check (wrapped by run_status_check) -> exists
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));
        // rm work dir
        mock.add_response(MockResponse::Ok(String::new()));
        // rm lock file
        mock.add_response(MockResponse::Ok(String::new()));

        run(&mock, &test_config(), "my-session", false).unwrap();

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
        // dir check -> exists
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));
        // rm work dir
        mock.add_response(MockResponse::Ok(String::new()));
        // rm lock file
        mock.add_response(MockResponse::Ok(String::new()));

        let config = Config::parse("remote = \"deploy@prod\"").unwrap();
        run(&mock, &config, "s1", false).unwrap();

        let inv = mock.invocations();
        for i in &inv {
            match i {
                Invocation::Ssh { remote, .. } => assert_eq!(remote, "deploy@prod"),
                _ => panic!("expected Ssh"),
            }
        }
    }

    #[test]
    fn nonexistent_session_returns_error() {
        let mock = MockRunner::new();
        // dir check -> not found
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));

        let result = run(&mock, &test_config(), "no-such-session", false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
