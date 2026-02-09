//! `relocal destroy [session-name]` — removes a session's remote state.
//!
//! Deletes the remote working directory and associated FIFOs after prompting
//! for confirmation. Uses the [`CommandRunner`] trait for all SSH operations.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::runner::CommandRunner;
use crate::ssh;

/// Removes a session's remote working directory and FIFOs.
///
/// If `confirm` is true, prompts the user for confirmation before proceeding.
/// Pass `false` in tests to skip the interactive prompt.
pub fn run(
    runner: &dyn CommandRunner,
    config: &Config,
    session_name: &str,
    confirm: bool,
) -> Result<()> {
    // Check the session exists
    let dir_check = runner.run_ssh(&config.remote, &ssh::check_work_dir_exists(session_name))?;
    let fifos_check = runner.run_ssh(&config.remote, &ssh::check_fifos_exist(session_name))?;
    if !dir_check.status.success() && !fifos_check.status.success() {
        return Err(Error::Remote {
            remote: config.remote.clone(),
            message: format!(
                "session '{session_name}' not found. No working directory or FIFOs exist."
            ),
        });
    }

    if confirm {
        let prompt = format!(
            "Remove session '{session_name}' on {}? This deletes {} and its FIFOs.",
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

    eprintln!("Removing remote working directory...");
    runner.run_ssh(&config.remote, &ssh::rm_work_dir(session_name))?;

    eprintln!("Removing FIFOs...");
    runner.run_ssh(&config.remote, &ssh::remove_fifos(session_name))?;

    eprintln!("Session '{session_name}' destroyed.");
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
    fn removes_working_dir_and_fifos() {
        let mock = MockRunner::new();
        // dir check -> exists
        mock.add_response(MockResponse::Ok(String::new()));
        // fifos check -> exists
        mock.add_response(MockResponse::Ok(String::new()));
        // rm work dir
        mock.add_response(MockResponse::Ok(String::new()));
        // rm fifos
        mock.add_response(MockResponse::Ok(String::new()));

        run(&mock, &test_config(), "my-session", false).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 4);

        match &inv[2] {
            Invocation::Ssh { remote, command } => {
                assert_eq!(remote, "user@host");
                assert!(command.contains("rm -rf"));
                assert!(command.contains("my-session"));
            }
            _ => panic!("expected Ssh"),
        }

        match &inv[3] {
            Invocation::Ssh { remote, command } => {
                assert_eq!(remote, "user@host");
                assert!(command.contains("rm -f"));
                assert!(command.contains("my-session-request"));
                assert!(command.contains("my-session-ack"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn targets_correct_remote() {
        let mock = MockRunner::new();
        // dir check -> exists
        mock.add_response(MockResponse::Ok(String::new()));
        // fifos check -> exists
        mock.add_response(MockResponse::Ok(String::new()));
        mock.add_response(MockResponse::Ok(String::new()));
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
        mock.add_response(MockResponse::Fail(String::new()));
        // fifos check -> not found
        mock.add_response(MockResponse::Fail(String::new()));

        let result = run(&mock, &test_config(), "no-such-session", false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
