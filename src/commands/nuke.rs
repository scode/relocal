//! `relocal remote nuke` — deletes everything under `~/relocal/` on the remote.
//!
//! This is a development/upgrade escape hatch for when you want a clean slate.
//! It does NOT uninstall APT packages, Rust, or Claude Code.

use tracing::info;

use crate::config::Config;
use crate::error::Result;
use crate::runner::CommandRunner;
use crate::ssh;

/// Removes the entire `~/relocal/` directory on the remote.
///
/// If `confirm` is true, prompts the user for confirmation before proceeding.
/// Pass `false` in tests to skip the interactive prompt.
pub fn run(runner: &dyn CommandRunner, config: &Config, confirm: bool) -> Result<()> {
    if confirm {
        let prompt = format!(
            "Delete ALL relocal data on {}? This removes ~/relocal/ entirely \
             (all sessions, FIFOs, and the hook script).",
            config.remote
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

    info!("Nuking ~/relocal/ on {}...", config.remote);
    runner.run_ssh(&config.remote, &ssh::rm_relocal_dir())?;

    eprintln!("Done. Run `relocal remote install` to set up again.");
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
    fn removes_entire_relocal_dir() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));

        run(&mock, &test_config(), false).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Ssh { remote, command } => {
                assert_eq!(remote, "user@host");
                assert!(command.contains("rm -rf"));
                assert!(command.contains("~/relocal"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn targets_correct_remote() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));

        let config = Config::parse("remote = \"deploy@prod\"").unwrap();
        run(&mock, &config, false).unwrap();

        let inv = mock.invocations();
        match &inv[0] {
            Invocation::Ssh { remote, .. } => assert_eq!(remote, "deploy@prod"),
            _ => panic!("expected Ssh"),
        }
    }
}
