//! Shared session orchestration for `relocal claude` and `relocal codex`.
//!
//! Both commands follow the same lifecycle: establish ControlMaster, check for
//! stale sessions, verify the tool is installed, create the remote directory,
//! push, run a background sync loop, launch an interactive SSH session, and
//! clean up. This module captures that shared flow, parameterized by a
//! [`ToolConfig`] that describes the tool-specific differences.

use std::path::Path;
use std::sync::Arc;

use tracing::{error, info, warn};

use crate::commands::sync::{sync_pull, sync_push};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::runner::{CommandRunner, ProcessRunner};
use crate::sidecar::Sidecar;
use crate::ssh::{self, SshControlMaster};

/// Tool-specific configuration that varies between Claude and Codex sessions.
pub struct ToolConfig {
    /// Display name used in log messages and errors (e.g., "Claude Code", "Codex").
    pub display_name: &'static str,

    /// Shell command to check whether the tool is installed on the remote.
    pub check_installed: fn() -> String,

    /// Shell command to launch an interactive session in the remote working directory.
    pub start_session: fn(&str, &[String]) -> String,
}

/// Production entry point: runs the full session flow with ControlMaster,
/// background sync, and interactive SSH.
pub fn run(
    tool: &ToolConfig,
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
    extra_args: &[String],
) -> Result<()> {
    info!("Establishing SSH ControlMaster...");
    let control_master = SshControlMaster::start(&config.remote, session_name)?;
    let runner = ProcessRunner::with_control_path(control_master.socket_path());

    setup(tool, &runner, config, session_name, repo_root, verbose)?;

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

    let ssh_result = runner.run_ssh_interactive(
        &config.remote,
        &(tool.start_session)(session_name, extra_args),
    );

    sidecar.shutdown();

    match ssh_result {
        Ok(status) if status.success() => {
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

    if let Err(e) = cleanup(&runner, config, session_name) {
        warn!("Lock file cleanup failed: {e}");
        warn!("You may need to run: relocal destroy {session_name}");
    }

    Ok(())
}

/// Pre-session setup: check for stale session, verify tool is installed,
/// create remote dir, acquire lock, initial push.
///
/// Separated from `run` so it can be tested with [`MockRunner`].
pub fn setup(
    tool: &ToolConfig,
    runner: &dyn CommandRunner,
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
) -> Result<()> {
    // 1. Check for stale session
    info!("Checking for stale session...");
    let lock_exists = ssh::run_status_check(
        runner,
        &config.remote,
        &ssh::check_lock_file_exists(session_name),
    )?;
    if lock_exists {
        return Err(Error::StaleSession {
            session: session_name.to_string(),
        });
    }

    // 2. Check tool is installed on remote
    info!("Checking {} installation...", tool.display_name);
    let installed = ssh::run_status_check(runner, &config.remote, &(tool.check_installed)())?;
    if !installed {
        return Err(Error::Remote {
            remote: config.remote.clone(),
            message: format!(
                "{} is not installed. Run `relocal remote install` first.",
                tool.display_name
            ),
        });
    }

    // 3. Create remote working directory and acquire lock
    info!("Creating remote working directory...");
    runner
        .run_ssh(&config.remote, &ssh::mkdir_work_dir(session_name))?
        .check("mkdir")?;
    runner
        .run_ssh(&config.remote, &ssh::create_lock_file(session_name))?
        .check("create lock file")?;

    // 4. Initial push
    sync_push(runner, config, session_name, repo_root, verbose)?;

    Ok(())
}

/// Post-session cleanup: remove lock file (best-effort).
pub fn cleanup(runner: &dyn CommandRunner, config: &Config, session_name: &str) -> Result<()> {
    info!("Removing lock file...");
    runner
        .run_ssh(&config.remote, &ssh::remove_lock_file(session_name))?
        .check("remove lock file")?;
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
    use crate::ssh::{STATUS_CHECK_FALSE, STATUS_CHECK_TRUE};
    use crate::test_support::{Invocation, MockResponse, MockRunner};
    use std::path::PathBuf;

    fn test_config() -> Config {
        Config::parse("remote = \"user@host\"").unwrap()
    }

    fn repo_root() -> PathBuf {
        PathBuf::from("/home/user/my-project")
    }

    fn test_tool() -> ToolConfig {
        ToolConfig {
            display_name: "TestTool",
            check_installed: || "command -v testtool".to_string(),
            start_session: |_session, _args| "testtool".to_string(),
        }
    }

    /// Provides the standard mock responses for a successful setup.
    fn mock_successful_setup(mock: &MockRunner) {
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // lock check
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into())); // tool check
        mock.add_response(MockResponse::Ok(String::new())); // mkdir
        mock.add_response(MockResponse::Ok(String::new())); // lock create
        mock.add_response(MockResponse::Ok(String::new())); // rsync push
    }

    #[test]
    fn setup_full_sequence() {
        let mock = MockRunner::new();
        mock_successful_setup(&mock);

        setup(
            &test_tool(),
            &mock,
            &test_config(),
            "my-session",
            &repo_root(),
            false,
        )
        .unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 5);

        // lock check (wrapped)
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("test -e"));
                assert!(command.contains(".locks"));
            }
            _ => panic!("expected Ssh for lock check"),
        }

        // tool check (wrapped)
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v testtool"));
            }
            _ => panic!("expected Ssh for tool check"),
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
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into())); // lock exists

        let result = setup(
            &test_tool(),
            &mock,
            &test_config(),
            "stale-session",
            &repo_root(),
            false,
        );
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::StaleSession { .. }));
        assert_eq!(mock.invocations().len(), 1);
    }

    #[test]
    fn setup_all_commands_target_correct_remote() {
        let mock = MockRunner::new();
        mock_successful_setup(&mock);

        let config = Config::parse("remote = \"deploy@prod\"").unwrap();
        setup(&test_tool(), &mock, &config, "s1", &repo_root(), false).unwrap();

        for i in &mock.invocations() {
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
        mock_successful_setup(&mock);

        setup(
            &test_tool(),
            &mock,
            &test_config(),
            "s1",
            &repo_root(),
            true,
        )
        .unwrap();

        match &mock.invocations()[4] {
            Invocation::Rsync { args, .. } => {
                assert!(args.contains(&"--progress".to_string()));
            }
            _ => panic!("expected Rsync"),
        }
    }

    #[test]
    fn setup_fails_if_tool_not_installed() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // lock check
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // tool check -> not found

        let result = setup(
            &test_tool(),
            &mock,
            &test_config(),
            "s1",
            &repo_root(),
            false,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("TestTool is not installed"));
        assert_eq!(mock.invocations().len(), 2);
    }

    #[test]
    fn setup_fails_if_mkdir_fails() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // lock check
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into())); // tool check
        mock.add_response(MockResponse::Fail("permission denied".into())); // mkdir fails

        let result = setup(
            &test_tool(),
            &mock,
            &test_config(),
            "s1",
            &repo_root(),
            false,
        );
        assert!(result.is_err());
        assert_eq!(mock.invocations().len(), 3);
    }

    #[test]
    fn setup_fails_if_lock_creation_fails() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // lock check
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into())); // tool check
        mock.add_response(MockResponse::Ok(String::new())); // mkdir succeeds
        mock.add_response(MockResponse::Fail("noclobber: file exists".into())); // lock fails

        let result = setup(
            &test_tool(),
            &mock,
            &test_config(),
            "s1",
            &repo_root(),
            false,
        );
        assert!(result.is_err());
        assert_eq!(mock.invocations().len(), 4);
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

    #[test]
    fn setup_uses_claude_tool_config() {
        let claude = ToolConfig {
            display_name: "Claude Code",
            check_installed: ssh::check_claude_installed,
            start_session: ssh::start_claude_session,
        };
        let mock = MockRunner::new();
        mock_successful_setup(&mock);

        setup(&claude, &mock, &test_config(), "s1", &repo_root(), false).unwrap();

        match &mock.invocations()[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v claude"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn setup_uses_codex_tool_config() {
        let codex = ToolConfig {
            display_name: "Codex",
            check_installed: ssh::check_codex_installed,
            start_session: ssh::start_codex_session,
        };
        let mock = MockRunner::new();
        mock_successful_setup(&mock);

        setup(&codex, &mock, &test_config(), "s1", &repo_root(), false).unwrap();

        match &mock.invocations()[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v codex"));
            }
            _ => panic!("expected Ssh"),
        }
    }
}
