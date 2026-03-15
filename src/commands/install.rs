//! `relocal remote install` — installs the full environment on the remote host.
//!
//! Performs seven idempotent steps: APT packages, Homebrew, gh, Rust, Claude Code,
//! Codex CLI, and Claude auth. Safe to re-run at any time.

use tracing::info;

use crate::config::Config;
use crate::error::Result;
use crate::runner::CommandRunner;
use crate::ssh;

/// Runs all remote installation steps in order.
pub fn run(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    install_apt_packages(runner, config)?;
    install_homebrew(runner, config)?;
    install_if_absent(
        runner,
        &config.remote,
        "GitHub CLI",
        "gh",
        "brew install gh",
    )?;
    install_if_absent(
        runner,
        &config.remote,
        "Rust",
        "rustup",
        "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y",
    )?;
    install_if_absent(
        runner,
        &config.remote,
        "Claude Code",
        "claude",
        "npm install -g @anthropic-ai/claude-code",
    )?;
    install_if_absent(
        runner,
        &config.remote,
        "Codex CLI",
        "codex",
        "npm install -g @openai/codex",
    )?;
    authenticate_claude(runner, config)?;

    info!("Remote installation complete.");
    Ok(())
}

fn install_apt_packages(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    info!("Installing APT packages...");
    let mut packages = vec![
        "build-essential".to_string(),
        "git".to_string(),
        "nodejs".to_string(),
        "npm".to_string(),
    ];
    packages.extend(config.apt_packages.clone());

    let pkg_list = packages.join(" ");
    let cmd = format!("sudo apt-get update && sudo apt-get install -y {pkg_list}");
    runner.run_ssh(&config.remote, &cmd)?.check("apt-get")?;
    Ok(())
}

fn install_homebrew(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    info!("Checking for Homebrew...");
    if ssh::run_status_check(runner, &config.remote, "command -v brew")? {
        info!("Homebrew already installed, skipping.");
        return Ok(());
    }

    info!("Installing Homebrew (Linuxbrew)...");
    runner.run_ssh(
        &config.remote,
        "NONINTERACTIVE=1 bash -c 'curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh | bash'",
    )?.check("homebrew install")?;

    // Add brew to PATH for future login shells (idempotent: skip if already present)
    info!("Adding Homebrew to PATH...");
    runner.run_ssh(
        &config.remote,
        "grep -q linuxbrew ~/.profile 2>/dev/null || echo 'eval \"$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)\"' >> ~/.profile",
    )?.check("homebrew PATH setup")?;
    Ok(())
}

/// Checks for a binary on PATH and installs it if absent.
fn install_if_absent(
    runner: &dyn CommandRunner,
    remote: &str,
    name: &str,
    binary: &str,
    install_cmd: &str,
) -> Result<()> {
    info!("Checking for {name}...");
    if ssh::run_status_check(runner, remote, &format!("command -v {binary}"))? {
        info!("{name} already installed, skipping.");
        return Ok(());
    }

    info!("Installing {name}...");
    runner
        .run_ssh(remote, install_cmd)?
        .check(&format!("{name} install"))?;
    Ok(())
}

fn authenticate_claude(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    info!("Checking Claude authentication...");
    if ssh::run_status_check(runner, &config.remote, "claude auth status")? {
        info!("Claude already authenticated, skipping.");
        return Ok(());
    }

    info!("Running claude login (interactive)...");
    let status = runner.run_ssh_interactive(&config.remote, "claude login")?;
    if !status.success() {
        return Err(crate::error::Error::CommandFailed {
            command: "claude login".to_string(),
            message: "interactive login failed".to_string(),
        });
    }
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

    fn config_with_packages() -> Config {
        Config::parse(
            r#"
remote = "user@host"
apt_packages = ["libssl-dev", "pkg-config"]
"#,
        )
        .unwrap()
    }

    #[test]
    fn apt_packages_includes_git() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));

        install_apt_packages(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("git"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn apt_packages_includes_baseline_and_user_packages() {
        let mock = MockRunner::new();
        // APT install
        mock.add_response(MockResponse::Ok(String::new()));

        install_apt_packages(&mock, &config_with_packages()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("build-essential"));
                assert!(command.contains("nodejs"));
                assert!(command.contains("npm"));
                assert!(command.contains("libssl-dev"));
                assert!(command.contains("pkg-config"));
                assert!(command.contains("sudo apt-get"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn homebrew_skipped_when_present() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));

        install_homebrew(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v brew"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn homebrew_installed_when_absent() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // check -> not found
        mock.add_response(MockResponse::Ok(String::new())); // install
        mock.add_response(MockResponse::Ok(String::new())); // PATH setup

        install_homebrew(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 3);
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("Homebrew"));
                assert!(command.contains("NONINTERACTIVE=1"));
            }
            _ => panic!("expected Ssh"),
        }
        match &inv[2] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("linuxbrew"));
                assert!(command.contains(".profile"));
            }
            _ => panic!("expected Ssh for PATH setup"),
        }
    }

    #[test]
    fn homebrew_path_setup_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // check -> not found
        mock.add_response(MockResponse::Ok(String::new())); // install succeeds
        mock.add_response(MockResponse::Fail("permission denied".into())); // PATH setup fails

        let result = install_homebrew(&mock, &test_config());
        assert!(result.is_err());
    }

    #[test]
    fn homebrew_ssh_transport_failure_propagates() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail("ssh: connect timeout".into()));

        let result = install_homebrew(&mock, &test_config());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("status probe"));
    }

    #[test]
    fn install_if_absent_skips_when_present() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));

        install_if_absent(
            &mock,
            "user@host",
            "MyTool",
            "mytool",
            "brew install mytool",
        )
        .unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v mytool"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn install_if_absent_installs_when_missing() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Ok(String::new()));

        install_if_absent(
            &mock,
            "user@host",
            "MyTool",
            "mytool",
            "brew install mytool",
        )
        .unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 2);
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("brew install mytool"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn install_if_absent_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Fail("install failed".into()));

        let result = install_if_absent(
            &mock,
            "user@host",
            "MyTool",
            "mytool",
            "brew install mytool",
        );
        assert!(result.is_err());
    }

    #[test]
    fn install_if_absent_ssh_transport_failure_propagates() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail("ssh: connect timeout".into()));

        let result = install_if_absent(
            &mock,
            "user@host",
            "MyTool",
            "mytool",
            "brew install mytool",
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("status probe"));
    }

    #[test]
    fn claude_auth_skipped_when_authenticated() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));

        authenticate_claude(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
    }

    #[test]
    fn claude_auth_runs_login_when_not_authenticated() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Ok(String::new()));

        authenticate_claude(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 2);
        assert!(
            matches!(&inv[1], Invocation::SshInteractive { command, .. } if command.contains("claude login"))
        );
    }

    #[test]
    fn claude_auth_ssh_transport_failure_propagates() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail("ssh: connect timeout".into()));

        let result = authenticate_claude(&mock, &test_config());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("status probe"));
    }

    #[test]
    fn claude_auth_login_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // not authenticated
        mock.add_response(MockResponse::Fail(String::new())); // login fails

        let result = authenticate_claude(&mock, &test_config());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("claude login"));
    }

    #[test]
    fn apt_install_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail("E: Unable to locate package".into()));

        let result = install_apt_packages(&mock, &test_config());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("apt-get"));
    }

    #[test]
    fn homebrew_install_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // check -> not found
        mock.add_response(MockResponse::Fail("curl failed".into())); // install fails

        let result = install_homebrew(&mock, &test_config());
        assert!(result.is_err());
    }

    #[test]
    fn full_run_installs_everything_when_absent() {
        let mock = MockRunner::new();
        // 1. APT
        mock.add_response(MockResponse::Ok(String::new()));
        // 2. brew check -> absent, install, PATH setup
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Ok(String::new()));
        mock.add_response(MockResponse::Ok(String::new()));
        // 3. gh check -> absent, install
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Ok(String::new()));
        // 4. rustup check -> absent, install
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Ok(String::new()));
        // 5. claude check -> absent, install
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Ok(String::new()));
        // 6. codex check -> absent, install
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Ok(String::new()));
        // 7. auth check -> not authenticated, login succeeds
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));
        mock.add_response(MockResponse::Ok(String::new()));

        run(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        let cmds: Vec<&str> = inv
            .iter()
            .filter_map(|i| match i {
                Invocation::Ssh { command, .. } => Some(command.as_str()),
                Invocation::SshInteractive { command, .. } => Some(command.as_str()),
                _ => None,
            })
            .collect();

        assert!(cmds.iter().any(|c| c.contains("brew install gh")));
        assert!(cmds.iter().any(|c| c.contains("rustup.rs")));
        assert!(cmds
            .iter()
            .any(|c| c.contains("npm install -g @anthropic-ai/claude-code")));
        assert!(cmds
            .iter()
            .any(|c| c.contains("npm install -g @openai/codex")));
    }

    #[test]
    fn full_run_issues_all_steps() {
        let mock = MockRunner::new();
        // 1. APT
        mock.add_response(MockResponse::Ok(String::new()));
        // 2. brew check -> present
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));
        // 3. gh check -> present
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));
        // 4. rustup check -> present
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));
        // 5. claude check -> present
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));
        // 6. codex check -> present
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));
        // 7. auth check -> authenticated
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));

        run(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        // APT(1) + brew check(1) + gh check(1) + rustup check(1) + claude check(1) + codex check(1) + auth check(1) = 7
        assert_eq!(inv.len(), 7);

        // All commands go to the right remote
        for i in &inv {
            match i {
                Invocation::Ssh { remote, .. } | Invocation::SshInteractive { remote, .. } => {
                    assert_eq!(remote, "user@host");
                }
                _ => panic!("expected SSH invocation"),
            }
        }
    }
}
