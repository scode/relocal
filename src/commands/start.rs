//! `relocal start [session-name]` — main orchestration command.
//!
//! Syncs the repo to the remote, launches an interactive Claude session, and
//! manages a background sidecar that handles hook-triggered syncs. On exit,
//! cleans up FIFOs and prints a summary.

use std::path::Path;
use std::sync::Arc;

use crate::commands::sync::sync_push;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::runner::{CommandRunner, ProcessRunner};
use crate::sidecar::Sidecar;
use crate::ssh;

/// Production entry point: runs the full start flow with real sidecar and SSH.
pub fn run(config: &Config, session_name: &str, repo_root: &Path, verbose: bool) -> Result<()> {
    let runner = ProcessRunner;

    // Pre-sidecar setup
    setup(&runner, config, session_name, repo_root, verbose)?;

    // Start sidecar
    let sidecar_runner: Arc<dyn CommandRunner + Send + Sync> = Arc::new(ProcessRunner);
    let mut sidecar = Sidecar::start(
        sidecar_runner,
        config.clone(),
        session_name.to_string(),
        repo_root.to_path_buf(),
        verbose,
    )?;

    // Run interactive Claude session
    let ssh_result =
        runner.run_ssh_interactive(&config.remote, &ssh::start_claude_session(session_name));

    // Cleanup always runs
    sidecar.shutdown();
    let cleanup_result = cleanup(&runner, config, session_name);

    // Report results
    match ssh_result {
        Ok(status) if status.success() => {
            print_summary(session_name, config);
        }
        Ok(_status) => {
            print_dirty_shutdown_message(session_name, config);
        }
        Err(e) => {
            eprintln!("SSH session error: {e}");
            print_dirty_shutdown_message(session_name, config);
        }
    }

    // Report cleanup failure but don't fail the command
    if let Err(e) = cleanup_result {
        eprintln!("Warning: FIFO cleanup failed: {e}");
        eprintln!("You may need to run: relocal destroy {session_name}");
    }

    Ok(())
}

/// Pre-sidecar setup: check stale FIFOs, create remote dir, create FIFOs,
/// initial push, and install hooks.
///
/// Separated from `run` so it can be tested with [`MockRunner`].
pub fn setup(
    runner: &dyn CommandRunner,
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
) -> Result<()> {
    // 1. Check for stale FIFOs
    eprintln!("Checking for stale session...");
    let fifo_check = runner.run_ssh(&config.remote, &ssh::check_fifos_exist(session_name))?;
    if fifo_check.status.success() {
        return Err(Error::StaleSession {
            session: session_name.to_string(),
        });
    }

    // 1b. Check Claude is installed on remote
    eprintln!("Checking Claude installation...");
    let claude_check = runner.run_ssh(&config.remote, &ssh::check_claude_installed())?;
    if !claude_check.status.success() {
        return Err(Error::Remote {
            remote: config.remote.clone(),
            message: "Claude Code is not installed. Run `relocal remote install` first."
                .to_string(),
        });
    }

    // 2. Create remote working directory
    eprintln!("Creating remote working directory...");
    runner.run_ssh(&config.remote, &ssh::mkdir_work_dir(session_name))?;

    // 3. Create FIFOs
    eprintln!("Creating FIFOs...");
    runner.run_ssh(&config.remote, &ssh::create_fifos(session_name))?;

    // 4. Initial push
    sync_push(runner, config, session_name, repo_root, verbose)?;

    // 5. Install hooks (reinject after push already does this, but the spec
    //    lists it as a separate step — sync_push handles both)
    // Hook injection already happened in sync_push via reinject_hooks.

    Ok(())
}

/// Post-session cleanup: remove FIFOs (best-effort).
pub fn cleanup(runner: &dyn CommandRunner, config: &Config, session_name: &str) -> Result<()> {
    eprintln!("Cleaning up FIFOs...");
    runner.run_ssh(&config.remote, &ssh::remove_fifos(session_name))?;
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
        // 1. check_fifos_exist -> not found (good)
        mock.add_response(MockResponse::Fail(String::new()));
        // 1b. check_claude_installed -> found
        mock.add_response(MockResponse::Ok("/usr/local/bin/claude\n".into()));
        // 2. mkdir_work_dir
        mock.add_response(MockResponse::Ok(String::new()));
        // 3. create_fifos
        mock.add_response(MockResponse::Ok(String::new()));
        // 4. sync_push: rsync
        mock.add_response(MockResponse::Ok(String::new()));
        // 4. sync_push: reinject_hooks read settings.json
        mock.add_response(MockResponse::Fail(String::new()));
        // 4. sync_push: reinject_hooks write settings.json
        mock.add_response(MockResponse::Ok(String::new()));

        setup(&mock, &test_config(), "my-session", &repo_root(), false).unwrap();

        let inv = mock.invocations();
        // check_fifos(1) + claude_check(1) + mkdir(1) + create_fifos(1) + rsync(1) + read_settings(1) + write_settings(1) = 7
        assert_eq!(inv.len(), 7);

        // Verify order: check fifos
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("test -e"));
                assert!(command.contains("my-session"));
            }
            _ => panic!("expected Ssh for fifo check"),
        }

        // claude check
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v claude"));
            }
            _ => panic!("expected Ssh for claude check"),
        }

        // mkdir
        match &inv[2] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("mkdir -p"));
                assert!(command.contains("my-session"));
            }
            _ => panic!("expected Ssh for mkdir"),
        }

        // create fifos
        match &inv[3] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("mkfifo"));
                assert!(command.contains("my-session-request"));
                assert!(command.contains("my-session-ack"));
            }
            _ => panic!("expected Ssh for create fifos"),
        }

        // rsync (push)
        assert!(matches!(&inv[4], Invocation::Rsync { .. }));

        // hook reinjection (read + write)
        match &inv[6] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("relocal-hook.sh"));
            }
            _ => panic!("expected Ssh for hook write"),
        }
    }

    #[test]
    fn setup_stale_fifos_detected() {
        let mock = MockRunner::new();
        // check_fifos_exist -> found (stale session)
        mock.add_response(MockResponse::Ok(String::new()));

        let result = setup(&mock, &test_config(), "stale-session", &repo_root(), false);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::StaleSession { .. }));
        assert!(err.to_string().contains("stale-session"));
        assert!(err.to_string().contains("relocal destroy"));

        // Only the fifo check was issued — nothing else
        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
    }

    #[test]
    fn cleanup_removes_fifos() {
        let mock = MockRunner::new();
        // remove_fifos
        mock.add_response(MockResponse::Ok(String::new()));

        cleanup(&mock, &test_config(), "s1").unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Ssh { remote, command } => {
                assert_eq!(remote, "user@host");
                assert!(command.contains("rm -f"));
                assert!(command.contains("s1-request"));
                assert!(command.contains("s1-ack"));
            }
            _ => panic!("expected Ssh for fifo removal"),
        }
    }

    #[test]
    fn cleanup_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Err("network down".into()));

        let result = cleanup(&mock, &test_config(), "s1");
        assert!(result.is_err());
    }

    #[test]
    fn setup_all_commands_target_correct_remote() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new())); // fifo check
        mock.add_response(MockResponse::Ok(String::new())); // claude check
        mock.add_response(MockResponse::Ok(String::new())); // mkdir
        mock.add_response(MockResponse::Ok(String::new())); // create fifos
        mock.add_response(MockResponse::Ok(String::new())); // rsync
        mock.add_response(MockResponse::Fail(String::new())); // read settings
        mock.add_response(MockResponse::Ok(String::new())); // write settings

        let config = Config::parse("remote = \"deploy@prod\"").unwrap();
        setup(&mock, &config, "s1", &repo_root(), false).unwrap();

        let inv = mock.invocations();
        for i in &inv {
            match i {
                Invocation::Ssh { remote, .. } => assert_eq!(remote, "deploy@prod"),
                Invocation::Rsync { args } => {
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
        mock.add_response(MockResponse::Fail(String::new())); // fifo check
        mock.add_response(MockResponse::Ok(String::new())); // claude check
        mock.add_response(MockResponse::Ok(String::new())); // mkdir
        mock.add_response(MockResponse::Ok(String::new())); // create fifos
        mock.add_response(MockResponse::Ok(String::new())); // rsync
        mock.add_response(MockResponse::Fail(String::new())); // read settings
        mock.add_response(MockResponse::Ok(String::new())); // write settings

        setup(&mock, &test_config(), "s1", &repo_root(), true).unwrap();

        let inv = mock.invocations();
        match &inv[4] {
            Invocation::Rsync { args } => {
                assert!(args.contains(&"--progress".to_string()));
            }
            _ => panic!("expected Rsync"),
        }
    }

    #[test]
    fn setup_fails_if_claude_not_installed() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new())); // fifo check (ok)
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
        mock.add_response(MockResponse::Fail(String::new())); // fifo check (ok)
        mock.add_response(MockResponse::Ok(String::new())); // claude check
        mock.add_response(MockResponse::Err("permission denied".into())); // mkdir fails

        let result = setup(&mock, &test_config(), "s1", &repo_root(), false);
        assert!(result.is_err());

        // fifo check + claude check + mkdir attempted
        let inv = mock.invocations();
        assert_eq!(inv.len(), 3);
    }

    #[test]
    fn setup_fails_if_create_fifos_fails() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new())); // fifo check
        mock.add_response(MockResponse::Ok(String::new())); // claude check
        mock.add_response(MockResponse::Ok(String::new())); // mkdir
        mock.add_response(MockResponse::Err("mkfifo failed".into())); // create fifos

        let result = setup(&mock, &test_config(), "s1", &repo_root(), false);
        assert!(result.is_err());

        let inv = mock.invocations();
        assert_eq!(inv.len(), 4);
    }
}
