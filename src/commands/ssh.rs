//! `relocal ssh [session-name]` — open an interactive shell in the remote session directory.

use crate::config::Config;
use crate::error::{Error, Result};
use crate::runner::CommandRunner;
use crate::ssh;

pub fn run(runner: &dyn CommandRunner, config: &Config, session_name: &str) -> Result<()> {
    let status =
        runner.run_ssh_interactive(&config.remote, &ssh::start_ssh_session(session_name))?;
    if !status.success() {
        return Err(Error::CommandFailed {
            command: "ssh".to_string(),
            message: format!(
                "SSH session exited with {}",
                status
                    .code()
                    .map_or("signal".to_string(), |c: i32| c.to_string())
            ),
        });
    }
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
    fn runs_interactive_ssh_to_session_dir() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));

        run(&mock, &test_config(), "my-session").unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::SshInteractive { remote, command } => {
                assert_eq!(remote, "user@host");
                assert!(command.contains("cd ~/relocal/my-session"));
                assert!(command.contains("exec $SHELL -l"));
            }
            _ => panic!("expected SshInteractive"),
        }
    }

    #[test]
    fn nonzero_exit_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new()));

        let result = run(&mock, &test_config(), "s1");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("SSH session exited"));
    }

    #[test]
    fn targets_correct_remote() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));

        let config = Config::parse("remote = \"deploy@prod\"").unwrap();
        run(&mock, &config, "s1").unwrap();

        let inv = mock.invocations();
        match &inv[0] {
            Invocation::SshInteractive { remote, .. } => assert_eq!(remote, "deploy@prod"),
            _ => panic!("expected SshInteractive"),
        }
    }
}
