//! `relocal remote install` — installs the full environment on the remote host.
//!
//! Performs six idempotent steps: APT packages, Homebrew, gh, Rust, Claude Code,
//! and Claude auth. Safe to re-run at any time.

use tracing::info;

use crate::config::Config;
use crate::error::Result;
use crate::runner::CommandRunner;
use crate::ssh;

/// Runs all remote installation steps in order.
pub fn run(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    install_apt_packages(runner, config)?;
    install_homebrew(runner, config)?;
    install_github_cli(runner, config)?;
    install_rust(runner, config)?;
    install_claude_code(runner, config)?;
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
    let check = runner.run_ssh(&config.remote, "command -v brew")?;
    if check.status.success() {
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

fn install_github_cli(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    info!("Checking for GitHub CLI...");
    let check = runner.run_ssh(&config.remote, "command -v gh")?;
    if check.status.success() {
        info!("GitHub CLI already installed, skipping.");
        return Ok(());
    }

    info!("Installing GitHub CLI via Homebrew...");
    runner
        .run_ssh(&config.remote, "brew install gh")?
        .check("brew install gh")?;
    Ok(())
}

fn install_rust(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    info!("Checking for Rust...");
    let check = runner.run_ssh(&config.remote, "command -v rustup")?;
    if check.status.success() {
        info!("rustup already installed, skipping.");
        return Ok(());
    }

    info!("Installing Rust via rustup...");
    runner
        .run_ssh(
            &config.remote,
            "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y",
        )?
        .check("rustup install")?;
    Ok(())
}

fn install_claude_code(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    info!("Checking for Claude Code...");
    let check = runner.run_ssh(&config.remote, &ssh::check_claude_installed())?;
    if check.status.success() {
        info!("Claude Code already installed, skipping.");
        return Ok(());
    }

    info!("Installing Claude Code via npm...");
    runner
        .run_ssh(&config.remote, "npm install -g @anthropic-ai/claude-code")?
        .check("npm install claude-code")?;
    Ok(())
}

fn authenticate_claude(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    info!("Checking Claude authentication...");
    let check = runner.run_ssh(&config.remote, "claude auth status")?;
    if check.status.success() {
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
        mock.add_response(MockResponse::Ok(
            "/home/linuxbrew/.linuxbrew/bin/brew\n".into(),
        ));

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
        mock.add_response(MockResponse::Fail(String::new())); // check -> not found
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
        mock.add_response(MockResponse::Fail(String::new())); // check -> not found
        mock.add_response(MockResponse::Ok(String::new())); // install succeeds
        mock.add_response(MockResponse::Fail("permission denied".into())); // PATH setup fails

        let result = install_homebrew(&mock, &test_config());
        assert!(result.is_err());
    }

    #[test]
    fn github_cli_skipped_when_present() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(
            "/home/linuxbrew/.linuxbrew/bin/gh\n".into(),
        ));

        install_github_cli(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v gh"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn github_cli_installed_when_absent() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new()));
        mock.add_response(MockResponse::Ok(String::new()));

        install_github_cli(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 2);
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("brew install gh"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn rust_skipped_when_present() {
        let mock = MockRunner::new();
        // rustup check succeeds
        mock.add_response(MockResponse::Ok("/home/user/.cargo/bin/rustup\n".into()));

        install_rust(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1); // only the check, no install
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v rustup"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn rust_installed_when_absent() {
        let mock = MockRunner::new();
        // rustup check fails
        mock.add_response(MockResponse::Fail(String::new()));
        // install command
        mock.add_response(MockResponse::Ok(String::new()));

        install_rust(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 2);
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("rustup.rs"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn claude_code_skipped_when_present() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok("/usr/local/bin/claude\n".into()));

        install_claude_code(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("command -v claude"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn claude_code_installed_when_absent() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new()));
        mock.add_response(MockResponse::Ok(String::new()));

        install_claude_code(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 2);
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("npm install -g @anthropic-ai/claude-code"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn claude_auth_skipped_when_authenticated() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok("Authenticated".into()));

        authenticate_claude(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
    }

    #[test]
    fn claude_auth_runs_login_when_not_authenticated() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new()));
        mock.add_response(MockResponse::Ok(String::new()));

        authenticate_claude(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 2);
        assert!(
            matches!(&inv[1], Invocation::SshInteractive { command, .. } if command.contains("claude login"))
        );
    }

    #[test]
    fn claude_auth_login_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new())); // not authenticated
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
        mock.add_response(MockResponse::Fail(String::new())); // check -> not found
        mock.add_response(MockResponse::Fail("curl failed".into())); // install fails

        let result = install_homebrew(&mock, &test_config());
        assert!(result.is_err());
    }

    #[test]
    fn github_cli_install_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new())); // check -> not found
        mock.add_response(MockResponse::Fail("brew failed".into())); // install fails

        let result = install_github_cli(&mock, &test_config());
        assert!(result.is_err());
    }

    #[test]
    fn rust_install_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new())); // check -> not found
        mock.add_response(MockResponse::Fail("curl failed".into())); // install fails

        let result = install_rust(&mock, &test_config());
        assert!(result.is_err());
    }

    #[test]
    fn claude_code_install_failure_returns_error() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail(String::new())); // check -> not found
        mock.add_response(MockResponse::Fail("npm ERR!".into())); // install fails

        let result = install_claude_code(&mock, &test_config());
        assert!(result.is_err());
    }

    #[test]
    fn full_run_issues_all_steps() {
        let mock = MockRunner::new();
        // 1. APT
        mock.add_response(MockResponse::Ok(String::new()));
        // 2. brew check -> present
        mock.add_response(MockResponse::Ok("brew".into()));
        // 3. gh check -> present
        mock.add_response(MockResponse::Ok("gh".into()));
        // 4. rustup check -> present
        mock.add_response(MockResponse::Ok("rustup".into()));
        // 5. claude check -> present
        mock.add_response(MockResponse::Ok("claude".into()));
        // 6. auth check -> authenticated
        mock.add_response(MockResponse::Ok("ok".into()));

        run(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        // APT(1) + brew check(1) + gh check(1) + rustup check(1) + claude check(1) + auth check(1) = 6
        assert_eq!(inv.len(), 6);

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
