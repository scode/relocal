//! `relocal sync push` / `relocal sync pull` — manual sync commands.
//!
//! Push runs rsync (local → remote) and then re-injects hooks into the remote
//! `.claude/settings.json` (since the push may have overwritten it).
//! Pull runs rsync (remote → local) with no hook re-injection.

use std::path::Path;

use crate::config::Config;
use crate::error::Result;
use crate::hooks::merge_hooks;
use crate::rsync::{build_rsync_args, Direction};
use crate::runner::CommandRunner;
use crate::ssh;

/// Pushes local files to the remote, then re-injects hooks.
pub fn sync_push(
    runner: &dyn CommandRunner,
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
) -> Result<()> {
    eprintln!("Pushing to remote...");
    let args = build_rsync_args(config, Direction::Push, session_name, repo_root, verbose);
    let rsync_result = runner.run_rsync(&args)?;
    if !rsync_result.status.success() {
        return Err(crate::error::Error::CommandFailed {
            command: "rsync".to_string(),
            message: rsync_result.stderr,
        });
    }

    reinject_hooks(runner, config, session_name)?;

    eprintln!("Push complete.");
    Ok(())
}

/// Pulls remote files to local.
pub fn sync_pull(
    runner: &dyn CommandRunner,
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
) -> Result<()> {
    eprintln!("Pulling from remote...");
    let args = build_rsync_args(config, Direction::Pull, session_name, repo_root, verbose);
    let rsync_result = runner.run_rsync(&args)?;
    if !rsync_result.status.success() {
        return Err(crate::error::Error::CommandFailed {
            command: "rsync".to_string(),
            message: rsync_result.stderr,
        });
    }

    eprintln!("Pull complete.");
    Ok(())
}

/// Reads the remote `.claude/settings.json`, merges relocal hooks, and writes
/// it back. Called after every push to ensure hooks survive being overwritten.
pub fn reinject_hooks(
    runner: &dyn CommandRunner,
    config: &Config,
    session_name: &str,
) -> Result<()> {
    eprintln!("Re-injecting hooks...");

    // Read existing settings.json (may not exist yet)
    let read_result = runner.run_ssh(&config.remote, &ssh::read_settings_json(session_name))?;

    let existing = if read_result.status.success() {
        serde_json::from_str(&read_result.stdout).ok()
    } else {
        None
    };

    let merged = merge_hooks(existing, session_name);
    let json_str = serde_json::to_string_pretty(&merged).expect("merged hooks must serialize");

    runner.run_ssh(
        &config.remote,
        &ssh::write_settings_json(session_name, &json_str),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Invocation, MockResponse, MockRunner};
    use std::path::PathBuf;

    fn test_config() -> Config {
        Config::parse("remote = \"user@host\"").unwrap()
    }

    fn repo_root() -> PathBuf {
        PathBuf::from("/home/user/my-project")
    }

    #[test]
    fn push_runs_rsync_with_push_direction() {
        let mock = MockRunner::new();
        // rsync
        mock.add_response(MockResponse::Ok(String::new()));
        // read settings.json (not found)
        mock.add_response(MockResponse::Fail(String::new()));
        // write settings.json
        mock.add_response(MockResponse::Ok(String::new()));

        sync_push(&mock, &test_config(), "s1", &repo_root(), false).unwrap();

        let inv = mock.invocations();
        // First invocation should be rsync
        match &inv[0] {
            Invocation::Rsync { args } => {
                // Verify push direction: local path first, remote path second
                let last = args.last().unwrap();
                assert!(last.contains("user@host:"));
                let second_last = &args[args.len() - 2];
                assert!(second_last.starts_with("/home/user/my-project/"));
                // Verify settings.json is included (push direction)
                assert!(args.contains(&"--include=.claude/settings.json".to_string()));
            }
            _ => panic!("expected Rsync, got {:?}", inv[0]),
        }
    }

    #[test]
    fn pull_runs_rsync_with_pull_direction() {
        let mock = MockRunner::new();
        // rsync
        mock.add_response(MockResponse::Ok(String::new()));

        sync_pull(&mock, &test_config(), "s1", &repo_root(), false).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Rsync { args } => {
                // Verify pull direction: remote path first, local path second
                let last = args.last().unwrap();
                assert!(last.starts_with("/home/user/my-project/"));
                let second_last = &args[args.len() - 2];
                assert!(second_last.contains("user@host:"));
                // Verify settings.json is NOT included (pull direction)
                assert!(!args.contains(&"--include=.claude/settings.json".to_string()));
            }
            _ => panic!("expected Rsync, got {:?}", inv[0]),
        }
    }

    #[test]
    fn push_reinjects_hooks_after_rsync() {
        let mock = MockRunner::new();
        // rsync
        mock.add_response(MockResponse::Ok(String::new()));
        // read settings.json — return existing content
        mock.add_response(MockResponse::Ok(
            r#"{"allowedTools": ["bash"]}"#.to_string(),
        ));
        // write settings.json
        mock.add_response(MockResponse::Ok(String::new()));

        sync_push(&mock, &test_config(), "my-session", &repo_root(), false).unwrap();

        let inv = mock.invocations();
        // rsync (1) + read settings.json (1) + write settings.json (1) = 3
        assert_eq!(inv.len(), 3);

        // Second invocation: read settings.json
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("settings.json"));
                assert!(command.contains("cat"));
            }
            _ => panic!("expected Ssh for read, got {:?}", inv[1]),
        }

        // Third invocation: write settings.json with merged hooks
        match &inv[2] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("settings.json"));
                assert!(command.contains("relocal-hook.sh"));
                assert!(command.contains("my-session"));
                // Verify original keys preserved
                assert!(command.contains("allowedTools"));
            }
            _ => panic!("expected Ssh for write, got {:?}", inv[2]),
        }
    }

    #[test]
    fn push_creates_hooks_when_no_settings_json() {
        let mock = MockRunner::new();
        // rsync
        mock.add_response(MockResponse::Ok(String::new()));
        // read settings.json — fails (file doesn't exist)
        mock.add_response(MockResponse::Fail(String::new()));
        // write settings.json
        mock.add_response(MockResponse::Ok(String::new()));

        sync_push(&mock, &test_config(), "s1", &repo_root(), false).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 3);

        // Write should still happen with fresh hooks
        match &inv[2] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("relocal-hook.sh push"));
                assert!(command.contains("relocal-hook.sh pull"));
            }
            _ => panic!("expected Ssh for write, got {:?}", inv[2]),
        }
    }

    #[test]
    fn pull_does_not_reinject_hooks() {
        let mock = MockRunner::new();
        // rsync only
        mock.add_response(MockResponse::Ok(String::new()));

        sync_pull(&mock, &test_config(), "s1", &repo_root(), false).unwrap();

        let inv = mock.invocations();
        // Only rsync — no SSH calls for settings.json
        assert_eq!(inv.len(), 1);
        assert!(matches!(&inv[0], Invocation::Rsync { .. }));
    }

    #[test]
    fn push_verbose_passes_through() {
        let mock = MockRunner::new();
        // rsync
        mock.add_response(MockResponse::Ok(String::new()));
        // read settings.json
        mock.add_response(MockResponse::Fail(String::new()));
        // write settings.json
        mock.add_response(MockResponse::Ok(String::new()));

        sync_push(&mock, &test_config(), "s1", &repo_root(), true).unwrap();

        let inv = mock.invocations();
        match &inv[0] {
            Invocation::Rsync { args } => {
                assert!(args.contains(&"--progress".to_string()));
            }
            _ => panic!("expected Rsync"),
        }
    }

    #[test]
    fn pull_verbose_passes_through() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));

        sync_pull(&mock, &test_config(), "s1", &repo_root(), true).unwrap();

        let inv = mock.invocations();
        match &inv[0] {
            Invocation::Rsync { args } => {
                assert!(args.contains(&"--progress".to_string()));
            }
            _ => panic!("expected Rsync"),
        }
    }
}
