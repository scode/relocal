//! `relocal claude [session-name]` — main orchestration command.
//!
//! Syncs the repo to the remote, launches an interactive Claude session with
//! continuous background synchronization, and performs a final pull on clean exit.

use std::path::Path;
use std::sync::Arc;

use tracing::{error, info, warn};

use crate::commands::sync::{sync_pull, sync_push};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::runner::{CommandRunner, ProcessRunner};
use crate::sidecar::Sidecar;
use crate::ssh::{self, SshControlMaster};

/// Production entry point: runs the full session flow with ControlMaster,
/// background sync, and interactive SSH.
pub fn run(
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
    claude_args: &[String],
) -> Result<()> {
    // Establish persistent SSH connection
    info!("Establishing SSH ControlMaster...");
    let control_master = SshControlMaster::start(&config.remote, session_name)?;
    let runner = ProcessRunner::with_control_path(control_master.socket_path());

    // Pre-session setup
    setup(&runner, config, session_name, repo_root, verbose)?;

    // Start background sync loop
    let sidecar_runner: Arc<dyn CommandRunner + Send + Sync> = Arc::new(
        ProcessRunner::with_control_path(control_master.socket_path()),
    );
    let mut sidecar = Sidecar::start(
        sidecar_runner,
        config.clone(),
        session_name.to_string(),
        repo_root.to_path_buf(),
        verbose,
    )?;

    // Run interactive Claude session
    let ssh_result = runner.run_ssh_interactive(
        &config.remote,
        &ssh::start_claude_session(session_name, claude_args),
    );

    // Shutdown background sync
    sidecar.shutdown();

    // Report results
    match ssh_result {
        Ok(status) if status.success() => {
            // Final pull on clean exit
            info!("Performing final sync pull...");
            if let Err(e) = sync_pull(&runner, config, session_name, repo_root, verbose) {
                warn!("Final sync pull failed: {e}");
                warn!("Use `relocal sync pull {session_name}` to retry manually.");
            }
            print_summary(session_name, config);
        }
        Ok(_status) => {
            print_dirty_shutdown_message(session_name, config);
        }
        Err(e) => {
            error!("SSH session error: {e}");
            print_dirty_shutdown_message(session_name, config);
        }
    }

    // Clean up lock file (best-effort)
    if let Err(e) = cleanup(&runner, config, session_name) {
        warn!("Lock file cleanup failed: {e}");
        warn!("You may need to run: relocal destroy {session_name}");
    }

    // ControlMaster is torn down by Drop
    Ok(())
}

/// Pre-session setup: check for stale session, check Claude is installed,
/// create remote dir, acquire lock, initial push.
///
/// Separated from `run` so it can be tested with [`MockRunner`].
pub fn setup(
    runner: &dyn CommandRunner,
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
) -> Result<()> {
    // 1. Check for stale session
    info!("Checking for stale session...");
    let lock_check = runner.run_ssh(&config.remote, &ssh::check_lock_file_exists(session_name))?;
    if lock_check.status.success() {
        return Err(Error::StaleSession {
            session: session_name.to_string(),
        });
    }

    // 2. Check Claude is installed on remote
    info!("Checking Claude installation...");
    let claude_check = runner.run_ssh(&config.remote, &ssh::check_claude_installed())?;
    if !claude_check.status.success() {
        return Err(Error::Remote {
            remote: config.remote.clone(),
            message: "Claude Code is not installed. Run `relocal remote install` first."
                .to_string(),
        });
    }

    // 3. Create remote working directory and acquire lock
    info!("Creating remote working directory...");
    runner.run_ssh(&config.remote, &ssh::mkdir_work_dir(session_name))?;
    runner.run_ssh(&config.remote, &ssh::create_lock_file(session_name))?;

    // 4. Initial push
    sync_push(runner, config, session_name, repo_root, verbose)?;

    Ok(())
}

/// Post-session cleanup: remove lock file (best-effort).
pub fn cleanup(runner: &dyn CommandRunner, config: &Config, session_name: &str) -> Result<()> {
    info!("Removing lock file...");
    runner.run_ssh(&config.remote, &ssh::remove_lock_file(session_name))?;
    Ok(())
}

fn print_summary(session_name: &str, config: &Config) {
    eprintln!();
    eprintln!("Session ended: {session_name}");
    eprintln!("Remote dir:    {}", ssh::remote_work_dir(session_name));
    eprintln!("Remote host:   {}", config.remote);
    eprintln!();
    eprintln!("To pull latest changes: relocal sync pull {session_name}");
    eprintln!("To push local changes:  relocal sync push {session_name}");
}

fn print_dirty_shutdown_message(session_name: &str, config: &Config) {
    eprintln!();
    eprintln!("Session interrupted: {session_name}");
    eprintln!("Remote dir: {}", ssh::remote_work_dir(session_name));
    eprintln!("Remote host: {}", config.remote);
    eprintln!();
    eprintln!("There may be unsynchronized work on the remote.");
    eprintln!("Use `relocal sync pull {session_name}` to fetch remote changes,");
    eprintln!("or `relocal sync push {session_name}` to overwrite with local state.");
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
    fn setup_full_sequence() {
        let mock = MockRunner::new();
        // 1. lock check -> not found (good)
        mock.add_response(MockResponse::Fail(String::new()));
        // 2. check_claude_installed -> found
        mock.add_response(MockResponse::Ok("/usr/local/bin/claude\n".into()));
        // 3. mkdir_work_dir
        mock.add_response(MockResponse::Ok(String::new()));
        // 3b. create_lock_file
        mock.add_response(MockResponse::Ok(String::new()));
        // 4. sync_push: rsync
        mock.add_response(MockResponse::Ok(String::new()));

        setup(&mock, &test_config(), "my-session", &repo_root(), false).unwrap();

        let inv = mock.invocations();
        // lock_check(1) + claude_check(1) + mkdir(1) + lock_create(1) + rsync(1) = 5
        assert_eq!(inv.len(), 5);

        // lock check
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("test -e"));
                assert!(command.contains(".locks"));
            }
            _ => panic!("expected Ssh for lock check"),
        }

        // claude check
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v claude"));
            }
            _ => panic!("expected Ssh for claude check"),
        }

        // mkdir work dir
        match &inv[2] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("mkdir -p"));
                assert!(command.contains("my-session"));
            }
            _ => panic!("expected Ssh for mkdir"),
        }

        // lock file creation
        match &inv[3] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("noclobber"));
                assert!(command.contains(".locks"));
            }
            _ => panic!("expected Ssh for lock creation"),
        }

        // rsync (push)
        assert!(matches!(&inv[4], Invocation::Rsync { .. }));
    }

    #[test]
    fn setup_stale_session_detected() {
        let mock = MockRunner::new();
        // lock check -> found (stale session)
        mock.add_response(MockResponse::Ok(String::new()));

        let result = setup(&mock, &test_config(), "stale-session", &repo_root(), false);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::StaleSession { .. }));
        assert!(err.to_string().contains("stale-session"));
        assert!(err.to_string().contains("relocal destroy"));

        // Only the lock check was issued
        assert_eq!(mock.invocations().len(), 1);
    }

    #[test]
    fn setup_all_commands_target_correct_remote() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new())); // lock check
        mock.add_response(MockResponse::Ok(String::new())); // claude check
        mock.add_response(MockResponse::Ok(String::new())); // mkdir work dir
        mock.add_response(MockResponse::Ok(String::new())); // create lock
        mock.add_response(MockResponse::Ok(String::new())); // rsync (push)

        let config = Config::parse("remote = \"deploy@prod\"").unwrap();
        setup(&mock, &config, "s1", &repo_root(), false).unwrap();

        let inv = mock.invocations();
        for i in &inv {
            match i {
                Invocation::Ssh { remote, .. } => assert_eq!(remote, "deploy@prod"),
                Invocation::Rsync { args, .. } => {
                    let last = args.last().unwrap();
                    assert!(last.contains("deploy@prod"));
                }
                _ => panic!("unexpected invocation type"),
            }
        }
    }

    #[test]
    fn setup_verbose_passes_to_rsync() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new())); // lock check
        mock.add_response(MockResponse::Ok(String::new())); // claude check
        mock.add_response(MockResponse::Ok(String::new())); // mkdir work dir
        mock.add_response(MockResponse::Ok(String::new())); // create lock
        mock.add_response(MockResponse::Ok(String::new())); // rsync (push)

        setup(&mock, &test_config(), "s1", &repo_root(), true).unwrap();

        let inv = mock.invocations();
        match &inv[4] {
            Invocation::Rsync { args, .. } => {
                assert!(args.contains(&"--progress".to_string()));
            }
            _ => panic!("expected Rsync"),
        }
    }

    #[test]
    fn setup_fails_if_claude_not_installed() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new())); // lock check
        mock.add_response(MockResponse::Fail(String::new())); // claude check -> not found

        let result = setup(&mock, &test_config(), "s1", &repo_root(), false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Claude Code is not installed"));

        let inv = mock.invocations();
        assert_eq!(inv.len(), 2);
    }

    #[test]
    fn setup_fails_if_mkdir_fails() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new())); // lock check
        mock.add_response(MockResponse::Ok(String::new())); // claude check
        mock.add_response(MockResponse::Err("permission denied".into())); // mkdir fails

        let result = setup(&mock, &test_config(), "s1", &repo_root(), false);
        assert!(result.is_err());

        let inv = mock.invocations();
        assert_eq!(inv.len(), 3);
    }

    #[test]
    fn cleanup_removes_lock_file() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));

        cleanup(&mock, &test_config(), "s1").unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("rm -f"));
                assert!(command.contains(".locks/s1.lock"));
            }
            _ => panic!("expected Ssh for lock removal"),
        }
    }
}
