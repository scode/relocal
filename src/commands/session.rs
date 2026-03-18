//! Shared session orchestration for `relocal claude` and `relocal codex`.
//!
//! Both commands follow the same lifecycle: connect to (or spawn) the session
//! daemon, verify the tool is installed on the remote, launch an interactive
//! SSH session, and disconnect. The daemon owns the ControlMaster, background
//! sync loop, and remote lock file.

use std::path::Path;

use tracing::{error, info, warn};

use crate::config::Config;
use crate::daemon_client;
use crate::error::{Error, Result};
use crate::runner::{CommandRunner, ProcessRunner};
use crate::ssh;

/// Tool-specific configuration that varies between Claude and Codex sessions.
pub struct ToolConfig {
    /// Display name used in log messages and errors (e.g., "Claude Code", "Codex").
    pub display_name: &'static str,

    /// Shell command to check whether the tool is installed on the remote.
    pub check_installed: fn() -> String,

    /// Shell command to launch an interactive session in the remote working directory.
    pub start_session: fn(&str, &[String]) -> String,
}

/// Connects to the session daemon, checks the tool, and runs an interactive session.
pub fn run(
    tool: &ToolConfig,
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbosity: u8,
    extra_args: &[String],
) -> Result<()> {
    let daemon_conn =
        daemon_client::connect_or_spawn(session_name, &config.remote, repo_root, verbosity)
            .inspect_err(|_| {
                info!("Run `relocal log {session_name}` to see daemon logs.");
            })?;
    let runner = ProcessRunner::with_control_path(daemon_conn.control_master_path());

    check_tool_installed(tool, &runner, config)?;

    let remote_cmd = (tool.start_session)(session_name, extra_args);
    info!("Launching {} on {}...", tool.display_name, config.remote);

    let ssh_result = runner.run_ssh_interactive(&config.remote, &remote_cmd);

    // DaemonConnection is dropped here, signaling the daemon that this
    // client is done. The daemon handles final pull and cleanup when the
    // last client disconnects.
    drop(daemon_conn);

    match ssh_result {
        Ok(status) if status.success() => {
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

    Ok(())
}

/// Verifies the tool is installed on the remote, using the daemon's ControlMaster.
fn check_tool_installed(
    tool: &ToolConfig,
    runner: &dyn crate::runner::CommandRunner,
    config: &Config,
) -> Result<()> {
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
    Ok(())
}

fn print_summary(session_name: &str, config: &Config) {
    info!(
        "Session ended: {session_name} (remote: {}, dir: {})",
        config.remote,
        ssh::remote_work_dir(session_name)
    );
}

fn print_dirty_shutdown_message(session_name: &str, config: &Config) {
    warn!(
        "Session interrupted: {session_name} (remote: {}, dir: {})",
        config.remote,
        ssh::remote_work_dir(session_name)
    );
    warn!("There may be unsynchronized work on the remote.");
    warn!("Use `relocal sync pull {session_name}` to fetch remote changes,");
    warn!("or `relocal sync push {session_name}` to overwrite with local state.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::{STATUS_CHECK_FALSE, STATUS_CHECK_TRUE};
    use crate::test_support::{MockResponse, MockRunner};

    fn test_config() -> Config {
        Config::parse("remote = \"user@host\"").unwrap()
    }

    fn test_tool() -> ToolConfig {
        ToolConfig {
            display_name: "TestTool",
            check_installed: || "command -v testtool".to_string(),
            start_session: |_session, _args| "testtool".to_string(),
        }
    }

    #[test]
    fn check_tool_installed_succeeds_when_present() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));

        check_tool_installed(&test_tool(), &mock, &test_config()).unwrap();
        assert_eq!(mock.invocations().len(), 1);
    }

    #[test]
    fn check_tool_installed_fails_when_absent() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));

        let result = check_tool_installed(&test_tool(), &mock, &test_config());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn check_tool_installed_uses_claude_check() {
        let claude = ToolConfig {
            display_name: "Claude Code",
            check_installed: ssh::check_claude_installed,
            start_session: ssh::start_claude_session,
        };
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));

        check_tool_installed(&claude, &mock, &test_config()).unwrap();

        use crate::test_support::Invocation;
        match &mock.invocations()[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v claude"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn check_tool_installed_uses_codex_check() {
        let codex = ToolConfig {
            display_name: "Codex",
            check_installed: ssh::check_codex_installed,
            start_session: ssh::start_codex_session,
        };
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));

        check_tool_installed(&codex, &mock, &test_config()).unwrap();

        use crate::test_support::Invocation;
        match &mock.invocations()[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v codex"));
            }
            _ => panic!("expected Ssh"),
        }
    }
}
