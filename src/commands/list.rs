//! `relocal list` — lists all sessions on the remote.
//!
//! Lists directories under `~/relocal/`, excluding `.bin/` and `.fifos/`,
//! and prints each session name.

use crate::config::Config;
use crate::error::Result;
use crate::runner::CommandRunner;
use crate::ssh;

/// Lists all sessions on the remote.
pub fn run(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    let output = runner.run_ssh(&config.remote, &ssh::list_sessions())?;

    if !output.status.success() || output.stdout.trim().is_empty() {
        eprintln!("No sessions found on {}.", config.remote);
        return Ok(());
    }

    for line in output.stdout.lines() {
        let name = line.trim();
        if !name.is_empty() {
            eprintln!("{name}");
        }
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
    fn lists_sessions_via_ssh() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok("project-a\nproject-b\n".into()));

        run(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Ssh { remote, command } => {
                assert_eq!(remote, "user@host");
                assert!(command.contains("ls"));
                assert!(command.contains("grep -v"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn handles_no_sessions() {
        let mock = MockRunner::new();
        // ls fails or returns empty (no ~/relocal/ dir yet)
        mock.add_response(MockResponse::Fail(String::new()));

        // Should not error
        run(&mock, &test_config()).unwrap();
    }

    #[test]
    fn handles_empty_output() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));

        run(&mock, &test_config()).unwrap();
    }
}
