//! Trait abstraction for executing external commands (ssh, rsync, local processes).
//!
//! All orchestration code (sync loop, `claude`, `install`, `sync`) uses
//! [`CommandRunner`] rather than calling `std::process::Command` directly.
//! This enables unit-testing command sequences with a mock implementation
//! that records invocations and returns canned results, without needing
//! real SSH or rsync.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use shell_quote::{Bash, QuoteRefExt};

use crate::error::{Error, Result};
use crate::rsync::{Direction, RsyncParams};

/// Builds an rsync `-e` value for SSH ControlMaster args.
///
/// The args are `-o Key=Value` pairs with no shell metacharacters, so they
/// are joined directly. (Using shell_quote here would produce `$'...'`
/// ANSI-C quoting which rsync misinterprets as a variable reference.)
fn build_rsync_ssh_command(ssh_extra_args: &[String]) -> String {
    if ssh_extra_args.is_empty() {
        return "ssh".to_string();
    }
    format!("ssh {}", ssh_extra_args.join(" "))
}

/// Output captured from a non-interactive command.
#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
}

impl CommandOutput {
    /// Returns `Ok(self)` if the command exited successfully, or an error with
    /// the command name and stderr if it failed.
    ///
    /// Use this at call sites that must not silently ignore non-zero exit codes.
    pub fn check(self, command: &str) -> crate::error::Result<Self> {
        if self.status.success() {
            Ok(self)
        } else {
            Err(crate::error::Error::CommandFailed {
                command: command.to_string(),
                message: self.stderr,
            })
        }
    }
}

/// Abstraction over shelling out to ssh, rsync, and local processes.
///
/// Each method corresponds to a distinct invocation pattern:
/// - `run_ssh`: non-interactive `ssh user@host "command"`, captures output.
/// - `run_ssh_interactive`: `ssh -t user@host "command"`, inherits the terminal.
/// - `run_rsync`: runs rsync with the given argument list, captures output.
/// - `run_local`: runs an arbitrary local program, captures output.
pub trait CommandRunner {
    fn run_ssh(&self, remote: &str, command: &str) -> Result<CommandOutput>;
    fn run_ssh_interactive(&self, remote: &str, command: &str) -> Result<ExitStatus>;
    fn run_rsync(&self, params: &RsyncParams) -> Result<CommandOutput>;
    fn run_local(&self, program: &str, args: &[&str]) -> Result<CommandOutput>;
}

/// Production implementation that shells out via `std::process::Command`.
///
/// When a ControlMaster socket is configured, all SSH and rsync commands
/// reuse that persistent connection.
pub struct ProcessRunner {
    ssh: OsString,
    /// Extra args injected into all SSH invocations (e.g., ControlPath options).
    ssh_extra_args: Vec<String>,
}

impl ProcessRunner {
    /// Creates a runner that uses the system `ssh` binary.
    pub fn new() -> Self {
        Self {
            ssh: OsString::from("ssh"),
            ssh_extra_args: Vec::new(),
        }
    }

    /// Creates a runner that shells out through the given SSH client program.
    ///
    /// This keeps the injection point local to the runner instance, which makes
    /// transport-failure tests deterministic without leaking configuration into
    /// unrelated code running in the same process.
    pub fn with_ssh_program(ssh: impl Into<OsString>) -> Self {
        Self {
            ssh: ssh.into(),
            ssh_extra_args: Vec::new(),
        }
    }

    /// Creates a runner that routes all SSH/rsync through a ControlMaster socket.
    pub fn with_control_path(socket_path: &Path) -> Self {
        Self {
            ssh: OsString::from("ssh"),
            ssh_extra_args: vec![
                "-o".to_string(),
                format!("ControlPath={}", socket_path.display()),
                "-o".to_string(),
                "ControlMaster=auto".to_string(),
            ],
        }
    }
}

impl Default for ProcessRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Wraps a command in `bash -lc <quoted-command>` so it runs as a login shell.
///
/// Non-interactive SSH sessions don't source `~/.profile` or `~/.bash_profile`,
/// which means user-installed tools (e.g. `~/.local/bin/claude`) aren't on PATH.
/// Wrapping in a login shell ensures the full user environment is available.
fn login_shell_wrap(command: &str) -> String {
    let quoted: String = command.quoted(Bash);
    format!("bash -lc {quoted}")
}

/// Validates that the local pull target is a repo root.
///
/// Canonicalizes the path and delegates to [`find_repo_root`] for marker
/// validation. This prevents `rsync --delete` from wiping an unintended
/// directory if a bug in higher-level code passes the wrong `repo_root`.
fn validate_local_pull_target(local_path: &Path) -> Result<()> {
    let canonical = local_path
        .canonicalize()
        .map_err(|e| Error::CommandFailed {
            command: "rsync".to_string(),
            message: format!(
                "refusing to pull: local path {} cannot be resolved: {e}",
                local_path.display()
            ),
        })?;
    crate::discovery::find_repo_root(&canonical).map_err(|_| Error::CommandFailed {
        command: "rsync".to_string(),
        message: format!(
            "refusing to pull: {} does not contain relocal.toml or a valid .git",
            canonical.display()
        ),
    })?;
    Ok(())
}

impl CommandRunner for ProcessRunner {
    fn run_ssh(&self, remote: &str, command: &str) -> Result<CommandOutput> {
        let wrapped = login_shell_wrap(command);
        let output = Command::new(&self.ssh)
            .args(&self.ssh_extra_args)
            .args([remote, &wrapped])
            .output()?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        })
    }

    fn run_ssh_interactive(&self, remote: &str, command: &str) -> Result<ExitStatus> {
        let wrapped = login_shell_wrap(command);
        let status = Command::new(&self.ssh)
            .args(&self.ssh_extra_args)
            .args(["-t", remote, &wrapped])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        Ok(status)
    }

    fn run_rsync(&self, params: &RsyncParams) -> Result<CommandOutput> {
        if params.direction() == Direction::Pull {
            validate_local_pull_target(params.local_path())?;
        }
        let mut cmd = Command::new("rsync");
        // Inject SSH options for ControlMaster when configured.
        // Safe to join without shell quoting because ssh_extra_args are only
        // set by with_control_path(), which produces `-o Key=Value` pairs
        // with no spaces or shell metacharacters in the values (the socket
        // path is a short hash-based name we control).
        if !self.ssh_extra_args.is_empty() {
            let ssh_cmd = build_rsync_ssh_command(&self.ssh_extra_args);
            cmd.args(["-e", &ssh_cmd]);
        }
        let output = cmd.args(params.args()).output()?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        })
    }

    fn run_local(&self, program: &str, args: &[&str]) -> Result<CommandOutput> {
        let output = Command::new(program).args(args).output()?;
        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status: output.status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::process::ExitStatus;

    fn make_params(direction: Direction, local_path: PathBuf) -> RsyncParams {
        RsyncParams::for_test(vec!["--help".to_string()], direction, local_path)
    }

    #[test]
    fn check_success_returns_output() {
        let output = CommandOutput {
            stdout: "hello".to_string(),
            stderr: String::new(),
            status: ExitStatus::from_raw(0),
        };
        let checked = output.check("test-cmd").unwrap();
        assert_eq!(checked.stdout, "hello");
    }

    #[test]
    fn check_failure_returns_error_with_stderr() {
        let output = CommandOutput {
            stdout: String::new(),
            stderr: "something broke".to_string(),
            status: ExitStatus::from_raw(256), // exit code 1
        };
        let err = output.check("my-command").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("my-command"),
            "error should name the command: {msg}"
        );
        assert!(
            msg.contains("something broke"),
            "error should include stderr: {msg}"
        );
    }

    #[test]
    fn pull_refused_without_markers() {
        let dir = tempfile::tempdir().unwrap();
        let result = validate_local_pull_target(dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("refusing to pull"));
    }

    #[test]
    fn pull_allowed_with_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("relocal.toml"), "remote = \"u@h\"").unwrap();
        let result = validate_local_pull_target(dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn pull_allowed_with_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let result = validate_local_pull_target(dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn pull_allowed_with_git_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".git"), "gitdir: /some/path").unwrap();
        let result = validate_local_pull_target(dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn pull_refused_with_stray_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        // .git dir without HEAD — not a real git repo
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let result = validate_local_pull_target(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn push_skips_validation() {
        let runner = ProcessRunner::default();
        let params = make_params(Direction::Push, PathBuf::from("/nonexistent/path"));
        let result = runner.run_rsync(&params);
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(
                !msg.contains("refusing to pull"),
                "push should not validate local path"
            );
        }
    }

    #[test]
    fn run_rsync_pull_rejects_invalid_destination() {
        let runner = ProcessRunner::default();
        let dir = tempfile::tempdir().unwrap();
        let params = make_params(Direction::Pull, dir.path().to_path_buf());
        let err = runner.run_rsync(&params).unwrap_err().to_string();
        assert!(err.contains("refusing to pull"));
    }

    #[test]
    fn pull_refused_nonexistent_path() {
        let result = validate_local_pull_target(&PathBuf::from("/nonexistent/path/xyz"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cannot be resolved"));
    }

    #[test]
    fn run_local_captures_stdout() {
        let runner = ProcessRunner::default();
        let out = runner.run_local("echo", &["hello world"]).unwrap();
        assert_eq!(out.stdout.trim(), "hello world");
        assert!(out.status.success());
    }

    #[test]
    fn run_local_captures_stderr() {
        let runner = ProcessRunner::default();
        let out = runner.run_local("sh", &["-c", "echo oops >&2"]).unwrap();
        assert_eq!(out.stderr.trim(), "oops");
    }

    #[test]
    fn run_local_failing_command() {
        let runner = ProcessRunner::default();
        let out = runner.run_local("false", &[]).unwrap();
        assert!(!out.status.success());
    }

    #[test]
    fn run_local_nonexistent_program() {
        let runner = ProcessRunner::default();
        let result = runner.run_local("this-program-does-not-exist-xyz", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn login_shell_wrap_simple_command() {
        let wrapped = login_shell_wrap("command -v claude");
        assert!(wrapped.starts_with("bash -lc "));
        assert!(wrapped.contains("command -v claude"));
    }

    #[test]
    fn login_shell_wrap_with_single_quotes() {
        let wrapped = login_shell_wrap("echo 'hello' > /tmp/out");
        assert!(wrapped.starts_with("bash -lc "));
        // The command should be properly quoted so bash -lc receives the full command
        assert!(wrapped.contains("hello"));
    }

    #[test]
    fn login_shell_wrap_with_heredoc() {
        let cmd = "cat > /tmp/test << 'EOF'\n{\"key\": \"value\"}\nEOF";
        let wrapped = login_shell_wrap(cmd);
        assert!(wrapped.starts_with("bash -lc "));
    }

    #[test]
    fn injected_ssh_program_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-ssh");
        std::fs::write(
            &script,
            "#!/bin/sh\necho 'ssh: injected failure from runner test' >&2\nexit 255\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();

        let runner = ProcessRunner::with_ssh_program(&script);
        let out = runner.run_ssh("user@host", "echo hi").unwrap();
        assert!(!out.status.success());
        assert!(out.stderr.contains("injected failure from runner test"));
    }

    #[test]
    fn with_control_path_sets_extra_args() {
        let runner = ProcessRunner::with_control_path(Path::new("/tmp/test.sock"));
        assert_eq!(runner.ssh_extra_args.len(), 4);
        assert!(runner
            .ssh_extra_args
            .contains(&"ControlPath=/tmp/test.sock".to_string()));
        assert!(runner
            .ssh_extra_args
            .contains(&"ControlMaster=auto".to_string()));
    }

    #[test]
    fn build_rsync_ssh_command_no_args() {
        let cmd = build_rsync_ssh_command(&[]);
        assert_eq!(cmd, "ssh");
    }

    #[test]
    fn build_rsync_ssh_command_typical_control_path() {
        let args = vec![
            "-o".to_string(),
            "ControlPath=/var/folders/f7/abc/T/rlc-test-12345678".to_string(),
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
        ];
        let cmd = build_rsync_ssh_command(&args);
        // Must produce plain args, no $'...' quoting that rsync misinterprets
        assert_eq!(
            cmd,
            "ssh -o ControlPath=/var/folders/f7/abc/T/rlc-test-12345678 -o ControlMaster=auto"
        );
    }

    #[test]
    fn build_rsync_ssh_command_no_ansi_c_quoting() {
        // Regression: shell_quote's $'...' ANSI-C quoting causes rsync to
        // misinterpret `-o $'ControlPath=...'` as a shell variable reference,
        // producing "Bad configuration option: $controlpath".
        let args = vec![
            "-o".to_string(),
            "ControlPath=/tmp/rlc-test-abcd1234".to_string(),
        ];
        let cmd = build_rsync_ssh_command(&args);
        assert!(
            !cmd.contains("$'"),
            "must not use ANSI-C quoting ($'...'), got: {cmd}"
        );
        assert!(
            !cmd.contains("$\""),
            "must not use dollar-quoting ($\"...\"), got: {cmd}"
        );
    }
}
