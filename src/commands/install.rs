//! `relocal remote install` — installs the full environment on the remote host.
//!
//! Performs six idempotent steps: APT packages, Rust, Claude Code, Claude auth,
//! hook script, and FIFO directory. Safe to re-run at any time.

use tracing::info;

use crate::config::Config;
use crate::error::Result;
use crate::hooks::hook_script_content;
use crate::runner::CommandRunner;
use crate::ssh;

/// Runs all remote installation steps in order.
pub fn run(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    install_apt_packages(runner, config)?;
    install_rust(runner, config)?;
    install_claude_code(runner, config)?;
    authenticate_claude(runner, config)?;
    install_hook_script(runner, config)?;
    create_fifo_dir(runner, config)?;
    create_logs_dir(runner, config)?;

    info!("Remote installation complete.");
    Ok(())
}

fn install_apt_packages(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    info!("Installing APT packages...");
    let mut packages = vec![
        "build-essential".to_string(),
        "nodejs".to_string(),
        "npm".to_string(),
    ];
    packages.extend(config.apt_packages.clone());

    let pkg_list = packages.join(" ");
    let cmd = format!("sudo apt-get update && sudo apt-get install -y {pkg_list}");
    runner.run_ssh(&config.remote, &cmd)?;
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
    runner.run_ssh(
        &config.remote,
        "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y",
    )?;
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
    runner.run_ssh(&config.remote, "npm install -g @anthropic-ai/claude-code")?;
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
    runner.run_ssh_interactive(&config.remote, "claude login")?;
    Ok(())
}

fn install_hook_script(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    info!("Installing hook script...");
    runner.run_ssh(&config.remote, &ssh::mkdir_bin_dir())?;

    let script = hook_script_content();
    let write_cmd = format!(
        "cat > {} << 'RELOCAL_HOOK_EOF'\n{}\nRELOCAL_HOOK_EOF\nchmod +x {}",
        ssh::hook_script_path(),
        script,
        ssh::hook_script_path()
    );
    runner.run_ssh(&config.remote, &write_cmd)?;
    Ok(())
}

fn create_fifo_dir(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    info!("Creating FIFO directory...");
    runner.run_ssh(&config.remote, &ssh::mkdir_fifos_dir())?;
    Ok(())
}

fn create_logs_dir(runner: &dyn CommandRunner, config: &Config) -> Result<()> {
    info!("Creating logs directory...");
    runner.run_ssh(&config.remote, &ssh::mkdir_logs_dir())?;
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
    fn hook_script_installed() {
        let mock = MockRunner::new();
        // mkdir .bin
        mock.add_response(MockResponse::Ok(String::new()));
        // write script
        mock.add_response(MockResponse::Ok(String::new()));

        install_hook_script(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 2);
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("mkdir -p"));
                assert!(command.contains(".bin"));
            }
            _ => panic!("expected Ssh"),
        }
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("relocal-hook.sh"));
                assert!(command.contains("chmod +x"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn fifo_dir_created() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));

        create_fifo_dir(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("mkdir -p"));
                assert!(command.contains(".fifos"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn logs_dir_created() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new()));

        create_logs_dir(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("mkdir -p"));
                assert!(command.contains(".logs"));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn full_run_issues_all_steps() {
        let mock = MockRunner::new();
        // 1. APT
        mock.add_response(MockResponse::Ok(String::new()));
        // 2. rustup check -> present
        mock.add_response(MockResponse::Ok("rustup".into()));
        // 3. claude check -> present
        mock.add_response(MockResponse::Ok("claude".into()));
        // 4. auth check -> authenticated
        mock.add_response(MockResponse::Ok("ok".into()));
        // 5. hook: mkdir .bin
        mock.add_response(MockResponse::Ok(String::new()));
        // 5. hook: write script
        mock.add_response(MockResponse::Ok(String::new()));
        // 6. mkdir .fifos
        mock.add_response(MockResponse::Ok(String::new()));
        // 7. mkdir .logs
        mock.add_response(MockResponse::Ok(String::new()));

        run(&mock, &test_config()).unwrap();

        let inv = mock.invocations();
        // APT(1) + rustup check(1) + claude check(1) + auth check(1) + hook(2) + fifos(1) + logs(1) = 8
        assert_eq!(inv.len(), 8);

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
