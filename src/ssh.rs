//! Shared SSH helpers for relocal.
//!
//! This module owns both the shell snippets used for remote operations and the
//! shared probe execution helper used by higher-level commands such as
//! `relocal status`. It is intentionally not "strings only": remote probe
//! semantics live here so callers do not each reimplement the same
//! exit-code-to-meaning translation.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use shell_quote::{Bash, QuoteRefExt};

use crate::error::{Error, Result};
use crate::runner::CommandRunner;

/// Remote base directory for all relocal state.
const RELOCAL_DIR: &str = "~/relocal";
pub const STATUS_CHECK_TRUE: &str = "__RELOCAL_STATUS_TRUE__";
pub const STATUS_CHECK_FALSE: &str = "__RELOCAL_STATUS_FALSE__";

/// Returns the remote working directory path for a session.
pub fn remote_work_dir(session: &str) -> String {
    format!("{RELOCAL_DIR}/{session}")
}

/// Command to create the remote working directory.
pub fn mkdir_work_dir(session: &str) -> String {
    format!("mkdir -p {}", remote_work_dir(session))
}

/// Command to remove the remote working directory.
pub fn rm_work_dir(session: &str) -> String {
    format!("rm -rf {}", remote_work_dir(session))
}

/// Command to remove the entire relocal directory (nuke).
pub fn rm_relocal_dir() -> String {
    format!("rm -rf {RELOCAL_DIR}")
}

/// Path to a session's lock file on the remote.
fn lock_file_path(session: &str) -> String {
    format!("{RELOCAL_DIR}/.locks/{session}.lock")
}

/// Command to create a lock file for a session (fails if it already exists).
///
/// Uses `set -o noclobber` so the redirect fails if the file exists, providing
/// atomic stale-session detection without requiring external tools.
pub fn create_lock_file(session: &str) -> String {
    format!(
        "mkdir -p {RELOCAL_DIR}/.locks && ( set -o noclobber; echo $$ > {} )",
        lock_file_path(session)
    )
}

/// Command to check whether a lock file exists for a session.
pub fn check_lock_file_exists(session: &str) -> String {
    format!("test -e {}", lock_file_path(session))
}

/// Command to remove a session's lock file.
pub fn remove_lock_file(session: &str) -> String {
    format!("rm -f {}", lock_file_path(session))
}

/// Command to list session directories with sizes.
///
/// Output format: `<name>\t<size>` per line, e.g. `my-session\t4.0K`.
pub fn list_sessions() -> String {
    format!(
        "cd {RELOCAL_DIR} 2>/dev/null && for d in $(ls -1); do size=$(du -sh \"$d\" 2>/dev/null | cut -f1); printf '%s\\t%s\\n' \"$d\" \"$size\"; done"
    )
}

/// Command to check whether the remote working directory exists.
pub fn check_work_dir_exists(session: &str) -> String {
    format!("test -d {}", remote_work_dir(session))
}

/// Wraps a shell probe so exit code `1` can be reported without looking like SSH failure.
///
/// Many status-style probes use shell exit code `1` to mean "absent" rather than
/// "broken". This wrapper turns `0` and `1` into explicit stdout markers while
/// preserving any other exit code as a real command failure.
fn wrap_status_check(command: &str) -> String {
    format!(
        "{{ {command}; }} >/dev/null 2>&1; status=$?; case \"$status\" in 0) printf '{STATUS_CHECK_TRUE}' ;; 1) printf '{STATUS_CHECK_FALSE}' ;; *) exit \"$status\" ;; esac"
    )
}

/// Runs a wrapped SSH probe and returns whether the remote check succeeded.
///
/// Many remote shell probes use exit code `1` to mean "absent" rather than
/// "broken". This helper preserves that distinction by translating `0` and `1`
/// into explicit markers and surfacing transport or shell-level failures as
/// [`Error::Remote`].
pub fn run_status_check(runner: &dyn CommandRunner, remote: &str, command: &str) -> Result<bool> {
    let output = runner.run_ssh(remote, &wrap_status_check(command))?;

    if !output.status.success() {
        let code = output.status.code().map_or_else(
            || "terminated by signal".to_string(),
            |code| format!("exit code {code}"),
        );
        let stderr = output.stderr.trim();
        let message = if stderr.is_empty() {
            format!("status probe failed with {code}")
        } else {
            format!("status probe failed with {code}: {stderr}")
        };
        return Err(Error::Remote {
            remote: remote.to_string(),
            message,
        });
    }

    match output.stdout.trim() {
        STATUS_CHECK_TRUE => Ok(true),
        STATUS_CHECK_FALSE => Ok(false),
        other => Err(Error::Remote {
            remote: remote.to_string(),
            message: format!("status probe returned unexpected output: {other:?}"),
        }),
    }
}

/// Command to verify the remote session directory is a valid git repository.
///
/// Runs `git fsck --strict --full --no-dangling` in the session's working
/// directory. This is used as a safety gate before pulling: if the remote
/// is not a git repo (or is corrupted), we refuse to rsync `--delete`
/// into the local tree.
pub fn git_fsck(session: &str) -> String {
    format!(
        "cd {} && git fsck --strict --full --no-dangling",
        remote_work_dir(session)
    )
}

/// Command to check whether `claude` is on PATH.
pub fn check_claude_installed() -> String {
    "command -v claude".to_string()
}

/// Command to launch an interactive login shell in the working directory.
///
/// The `exec $SHELL -l` looks redundant with `run_ssh_interactive`'s
/// `login_shell_wrap`, but is intentional: the outer `bash -lc` is transport
/// (ensures PATH is set up so `cd` resolves `~`), while `exec $SHELL -l`
/// replaces it with the user's preferred shell.
pub fn start_ssh_session(session: &str) -> String {
    format!("cd {} && exec $SHELL -l", remote_work_dir(session))
}

/// Command to launch an interactive Claude session in the working directory.
///
/// Any extra arguments are appended after `--dangerously-skip-permissions`,
/// allowing the caller to pass flags like `--debug` through to `claude`.
pub fn start_claude_session(session: &str, extra_args: &[String]) -> String {
    let mut cmd = format!(
        "cd {} && claude --dangerously-skip-permissions",
        remote_work_dir(session)
    );
    for arg in extra_args {
        cmd.push(' ');
        let quoted: String = arg.as_str().quoted(Bash);
        cmd.push_str(&quoted);
    }
    cmd
}

/// Command to check whether `codex` is on PATH.
pub fn check_codex_installed() -> String {
    "command -v codex".to_string()
}

/// Command to launch an interactive Codex session in the working directory.
///
/// Any extra arguments are appended after `--yolo`,
/// allowing the caller to pass flags through to `codex`.
pub fn start_codex_session(session: &str, extra_args: &[String]) -> String {
    let mut cmd = format!("cd {} && codex --yolo", remote_work_dir(session));
    for arg in extra_args {
        cmd.push(' ');
        let quoted: String = arg.as_str().quoted(Bash);
        cmd.push_str(&quoted);
    }
    cmd
}

/// Deterministic ControlMaster socket path for daemon-managed sessions.
///
/// Hashes session name + remote (no PID), so all clients and the daemon
/// for a given (session, remote) pair resolve to the same path. The remote
/// is included because two repos can have the same session name (derived
/// from directory name) but point at different remotes — without it, a
/// daemon for one remote would be reused by a client targeting another.
pub fn shared_control_socket_path(session: &str, remote: &str) -> PathBuf {
    let prefix: String = session.chars().take(20).collect();
    let mut hasher = std::hash::DefaultHasher::new();
    session.hash(&mut hasher);
    remote.hash(&mut hasher);
    let hash = hasher.finish() as u32;
    std::env::temp_dir().join(format!("rlc-{prefix}-{hash:08x}"))
}

/// Unix domain socket path where the session daemon listens for clients.
///
/// Keyed on (session, remote) — not session alone — so that two repos
/// with the same session name but different remotes get independent daemons.
/// Uses the same prefix+hash scheme as [`shared_control_socket_path`] to
/// keep the path under the 104-byte Unix socket limit on macOS.
pub fn daemon_socket_path(session: &str, remote: &str) -> PathBuf {
    let prefix: String = session.chars().take(20).collect();
    let mut hasher = std::hash::DefaultHasher::new();
    // Domain tag prevents hash collision with ControlMaster/flock paths.
    "daemon-sock".hash(&mut hasher);
    session.hash(&mut hasher);
    remote.hash(&mut hasher);
    let hash = hasher.finish() as u32;
    std::env::temp_dir().join(format!("rlc-{prefix}-{hash:08x}.sock"))
}

/// Flock path used to serialize daemon startup across concurrent invocations.
///
/// Keyed on (session, remote) like the other daemon paths.
pub fn daemon_flock_path(session: &str, remote: &str) -> PathBuf {
    let prefix: String = session.chars().take(20).collect();
    let mut hasher = std::hash::DefaultHasher::new();
    "daemon-flock".hash(&mut hasher);
    session.hash(&mut hasher);
    remote.hash(&mut hasher);
    let hash = hasher.finish() as u32;
    std::env::temp_dir().join(format!("rlc-{prefix}-{hash:08x}.flock"))
}

/// Log file path for the session daemon's tracing output.
///
/// Keyed on (session, remote) like the other daemon paths. The daemon writes
/// here instead of stderr so that interactive claude/codex sessions aren't
/// cluttered with background sync noise. Use `relocal log` to tail it.
pub fn daemon_log_path(session: &str, remote: &str) -> PathBuf {
    let prefix: String = session.chars().take(20).collect();
    let mut hasher = std::hash::DefaultHasher::new();
    "daemon-log".hash(&mut hasher);
    session.hash(&mut hasher);
    remote.hash(&mut hasher);
    let hash = hasher.finish() as u32;
    std::env::temp_dir().join(format!("rlc-{prefix}-{hash:08x}.log"))
}

/// Acquires an exclusive advisory lock on the given file, blocking until available.
///
/// Used by both the daemon client (to serialize daemon startup) and the daemon
/// itself (to hold the lock through shutdown, preventing new daemons from
/// starting before the old one finishes cleaning up the remote lock file).
///
/// The daemon acquires this at shutdown, not startup. Acquiring at startup
/// would deadlock — see the comments in `daemon::run_daemon` for why.
pub fn acquire_flock(file: &std::fs::File) -> Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: file is a valid open File; as_raw_fd returns a valid fd that
    // outlives this call. flock is safe to call on any valid fd.
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if ret != 0 {
        return Err(Error::DaemonSpawnFailed {
            message: format!(
                "failed to acquire flock: {}",
                std::io::Error::last_os_error()
            ),
        });
    }
    Ok(())
}

/// Manages a persistent SSH ControlMaster connection.
///
/// All SSH and rsync commands during a session can share this connection,
/// avoiding repeated TCP+SSH handshakes.
pub struct SshControlMaster {
    socket_path: PathBuf,
    remote: String,
}

impl SshControlMaster {
    /// Establishes a ControlMaster with a deterministic socket path (no PID).
    ///
    /// Used by the session daemon so that all clients resolve to the same
    /// ControlMaster. Standalone commands should use [`start`] instead to
    /// avoid colliding with a running daemon.
    pub fn start_shared(remote: &str, session: &str) -> Result<Self> {
        let socket_path = shared_control_socket_path(session, remote);
        Self::start_with_path(remote, socket_path)
    }

    /// Establishes a ControlMaster connection to the remote.
    ///
    /// Creates a background SSH process (`-N -f`) that holds the connection
    /// open. The socket path is kept short to stay under the 104-byte Unix
    /// socket limit on macOS: `rlc-<prefix>-<hash>` where prefix is up to 20
    /// chars of the session name and hash encodes session+PID.
    pub fn start(remote: &str, session: &str) -> Result<Self> {
        let socket_path = Self::socket_path_for(session);
        Self::start_with_path(remote, socket_path)
    }

    fn start_with_path(remote: &str, socket_path: PathBuf) -> Result<Self> {
        let status = Command::new("ssh")
            .args([
                "-o",
                "ControlMaster=yes",
                "-o",
                &format!("ControlPath={}", socket_path.display()),
                "-o",
                "ControlPersist=300",
                "-N",
                "-f",
                remote,
            ])
            .status()?;

        if !status.success() {
            return Err(Error::CommandFailed {
                command: "ssh ControlMaster".to_string(),
                message: format!("failed to establish ControlMaster to {remote}"),
            });
        }

        Ok(Self {
            socket_path,
            remote: remote.to_string(),
        })
    }

    /// Builds a short, collision-resistant socket path.
    ///
    /// Format: `$TMPDIR/rlc-<prefix>-<hash>` where prefix is up to 20 chars
    /// of the session name (for human readability) and hash is an 8-hex-digit
    /// digest of session+PID (for uniqueness). The fixed-length filename keeps
    /// the total path under the 104-byte Unix socket limit on macOS.
    fn socket_path_for(session: &str) -> PathBuf {
        let prefix: String = session.chars().take(20).collect();
        let pid = std::process::id();
        let mut hasher = std::hash::DefaultHasher::new();
        session.hash(&mut hasher);
        pid.hash(&mut hasher);
        let hash = hasher.finish() as u32;
        std::env::temp_dir().join(format!("rlc-{prefix}-{hash:08x}"))
    }

    /// Returns the socket path for injecting into other SSH/rsync commands.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Tears down the ControlMaster connection.
    pub fn stop(&self) {
        let _ = Command::new("ssh")
            .args([
                "-O",
                "exit",
                "-o",
                &format!("ControlPath={}", self.socket_path.display()),
                &self.remote,
            ])
            .output();
    }
}

impl Drop for SshControlMaster {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    use crate::runner::ProcessRunner;
    use crate::test_support::{Invocation, MockResponse, MockRunner};

    fn write_executable_script(name: &str, body: &str) -> std::path::PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        std::mem::forget(dir);
        path
    }

    #[test]
    fn control_socket_path_fits_unix_limit() {
        // Unix socket paths max out at 104 bytes on macOS.
        // Even with a long session name and a long $TMPDIR, the path must fit.
        let long_session = "a]".repeat(50); // 100-char session name
        let path = SshControlMaster::socket_path_for(&long_session);
        let path_str = path.to_str().unwrap();
        assert!(
            path_str.len() <= 104,
            "socket path too long ({} bytes): {path_str}",
            path_str.len()
        );
    }

    #[test]
    fn control_socket_path_includes_session_prefix() {
        let path = SshControlMaster::socket_path_for("my-cool-project");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("rlc-my-cool-project-"));
    }

    #[test]
    fn control_socket_path_truncates_long_session() {
        let path = SshControlMaster::socket_path_for("this-is-a-very-long-session-name-indeed");
        let filename = path.file_name().unwrap().to_str().unwrap();
        // 20-char prefix: "this-is-a-very-long-"
        assert!(
            filename.starts_with("rlc-this-is-a-very-long--"),
            "unexpected filename: {filename}"
        );
    }

    #[test]
    fn lock_file_path_format() {
        assert_eq!(lock_file_path("s1"), "~/relocal/.locks/s1.lock");
    }

    #[test]
    fn create_lock_file_format() {
        let cmd = create_lock_file("s1");
        assert!(cmd.contains("mkdir -p"));
        assert!(cmd.contains(".locks"));
        assert!(cmd.contains("noclobber"));
        assert!(cmd.contains("s1.lock"));
    }

    #[test]
    fn remove_lock_file_format() {
        let cmd = remove_lock_file("s1");
        assert!(cmd.contains("rm -f"));
        assert!(cmd.contains("s1.lock"));
    }

    #[test]
    fn remote_work_dir_format() {
        assert_eq!(remote_work_dir("my-proj"), "~/relocal/my-proj");
    }

    #[test]
    fn mkdir_work_dir_format() {
        assert_eq!(mkdir_work_dir("s1"), "mkdir -p ~/relocal/s1");
    }

    #[test]
    fn rm_work_dir_format() {
        assert_eq!(rm_work_dir("s1"), "rm -rf ~/relocal/s1");
    }

    #[test]
    fn list_sessions_format() {
        let cmd = list_sessions();
        assert!(cmd.contains("du -sh"));
        // No longer filters dot-dirs
        assert!(!cmd.contains("grep -v"));
    }

    #[test]
    fn start_ssh_session_format() {
        let cmd = start_ssh_session("s1");
        assert_eq!(cmd, "cd ~/relocal/s1 && exec $SHELL -l");
    }

    #[test]
    fn start_claude_session_format() {
        let cmd = start_claude_session("s1", &[]);
        assert!(cmd.contains("cd ~/relocal/s1"));
        assert!(cmd.contains("claude --dangerously-skip-permissions"));
    }

    #[test]
    fn start_claude_session_with_extra_args() {
        let args = vec!["--debug".to_string(), "--resume".to_string()];
        let cmd = start_claude_session("s1", &args);
        assert!(cmd.contains("claude --dangerously-skip-permissions"));
        assert!(cmd.ends_with(" --debug --resume"));
    }

    #[test]
    fn check_codex_installed_format() {
        assert_eq!(check_codex_installed(), "command -v codex");
    }

    #[test]
    fn start_codex_session_format() {
        let cmd = start_codex_session("s1", &[]);
        assert!(cmd.contains("cd ~/relocal/s1"));
        assert!(cmd.contains("codex --yolo"));
    }

    #[test]
    fn start_codex_session_with_extra_args() {
        let args = vec!["--model".to_string(), "o3".to_string()];
        let cmd = start_codex_session("s1", &args);
        assert!(cmd.contains("codex --yolo"));
        assert!(cmd.ends_with(" --model o3"));
    }

    #[test]
    fn git_fsck_format() {
        let cmd = git_fsck("s1");
        assert_eq!(
            cmd,
            "cd ~/relocal/s1 && git fsck --strict --full --no-dangling"
        );
    }

    #[test]
    fn check_claude_installed_format() {
        assert_eq!(check_claude_installed(), "command -v claude");
    }

    #[test]
    fn run_status_check_wraps_commands_and_reports_true() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into()));

        let result = run_status_check(&mock, "user@host", "test -d ~/relocal/s1").unwrap();
        assert!(result);

        let invocations = mock.invocations();
        assert_eq!(invocations.len(), 1);
        match &invocations[0] {
            Invocation::Ssh { remote, command } => {
                assert_eq!(remote, "user@host");
                assert!(command.contains("test -d ~/relocal/s1"));
                assert!(command.contains(STATUS_CHECK_TRUE));
                assert!(command.contains(STATUS_CHECK_FALSE));
            }
            _ => panic!("expected Ssh"),
        }
    }

    #[test]
    fn run_status_check_reports_false() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into()));

        let result = run_status_check(&mock, "user@host", "test -d ~/relocal/s1").unwrap();
        assert!(!result);
    }

    #[test]
    fn run_status_check_surfaces_transport_failures() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Fail("ssh: connect timeout".into()));

        let err = run_status_check(&mock, "user@host", "test -d ~/relocal/s1").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("remote error"));
        assert!(message.contains("status probe failed"));
        assert!(message.contains("timeout"));
    }

    #[test]
    fn run_status_check_rejects_unexpected_output() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok("maybe".into()));

        let err = run_status_check(&mock, "user@host", "test -d ~/relocal/s1").unwrap_err();
        assert!(err.to_string().contains("unexpected output"));
    }

    #[test]
    fn run_status_check_surfaces_injected_process_runner_failures() {
        let fake_ssh = write_executable_script(
            "fake-ssh.sh",
            "#!/bin/sh\necho 'ssh: injected failure from ssh helper test' >&2\nexit 255\n",
        );
        let runner = ProcessRunner::with_ssh_program(&fake_ssh);

        let err = run_status_check(&runner, "user@host", "test -d ~/relocal/s1").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("remote error"));
        assert!(message.contains("status probe failed"));
        assert!(message.contains("ssh helper test"));
    }

    #[test]
    fn shared_control_socket_path_is_deterministic() {
        let a = shared_control_socket_path("my-session", "user@host");
        let b = shared_control_socket_path("my-session", "user@host");
        assert_eq!(a, b);
    }

    #[test]
    fn shared_control_socket_path_differs_by_session() {
        let a = shared_control_socket_path("session-a", "user@host");
        let b = shared_control_socket_path("session-b", "user@host");
        assert_ne!(a, b);
    }

    #[test]
    fn shared_control_socket_path_differs_by_remote() {
        let a = shared_control_socket_path("my-session", "user@host-a");
        let b = shared_control_socket_path("my-session", "user@host-b");
        assert_ne!(a, b);
    }

    #[test]
    fn shared_control_socket_path_fits_unix_limit() {
        let long_session = "a".repeat(100);
        let path = shared_control_socket_path(&long_session, "user@very-long-hostname.example.com");
        let path_str = path.to_str().unwrap();
        assert!(
            path_str.len() <= 104,
            "socket path too long ({} bytes): {path_str}",
            path_str.len()
        );
    }

    #[test]
    fn shared_control_socket_path_includes_prefix() {
        let path = shared_control_socket_path("my-cool-project", "user@host");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("rlc-my-cool-project-"));
    }

    #[test]
    fn daemon_socket_path_format() {
        let path = daemon_socket_path("my-session", "user@host");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("rlc-my-session-"));
        assert!(filename.ends_with(".sock"));
    }

    #[test]
    fn daemon_socket_path_is_deterministic() {
        assert_eq!(
            daemon_socket_path("my-session", "user@host"),
            daemon_socket_path("my-session", "user@host"),
        );
    }

    #[test]
    fn daemon_socket_path_differs_by_session() {
        assert_ne!(
            daemon_socket_path("a", "user@host"),
            daemon_socket_path("b", "user@host"),
        );
    }

    #[test]
    fn daemon_socket_path_differs_by_remote() {
        assert_ne!(
            daemon_socket_path("my-session", "user@host-a"),
            daemon_socket_path("my-session", "user@host-b"),
        );
    }

    #[test]
    fn daemon_socket_path_fits_unix_limit() {
        let long_session = "a".repeat(100);
        let path = daemon_socket_path(&long_session, "user@very-long-hostname.example.com");
        let path_str = path.to_str().unwrap();
        assert!(
            path_str.len() <= 104,
            "socket path too long ({} bytes): {path_str}",
            path_str.len()
        );
    }

    #[test]
    fn daemon_socket_path_does_not_collide_with_control_path() {
        assert_ne!(
            daemon_socket_path("my-session", "user@host"),
            shared_control_socket_path("my-session", "user@host"),
        );
    }

    #[test]
    fn daemon_flock_path_format() {
        let path = daemon_flock_path("my-session", "user@host");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("rlc-my-session-"));
        assert!(filename.ends_with(".flock"));
    }

    #[test]
    fn daemon_log_path_format() {
        let path = daemon_log_path("my-session", "user@host");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("rlc-my-session-"));
        assert!(filename.ends_with(".log"));
    }

    #[test]
    fn daemon_log_path_is_deterministic() {
        assert_eq!(
            daemon_log_path("my-session", "user@host"),
            daemon_log_path("my-session", "user@host"),
        );
    }

    #[test]
    fn daemon_log_path_differs_by_session() {
        assert_ne!(
            daemon_log_path("a", "user@host"),
            daemon_log_path("b", "user@host"),
        );
    }

    #[test]
    fn daemon_log_path_differs_by_remote() {
        assert_ne!(
            daemon_log_path("my-session", "user@host-a"),
            daemon_log_path("my-session", "user@host-b"),
        );
    }

    #[test]
    fn daemon_log_path_does_not_collide_with_other_paths() {
        let session = "my-session";
        let remote = "user@host";
        let log = daemon_log_path(session, remote);
        assert_ne!(log, daemon_socket_path(session, remote));
        assert_ne!(log, daemon_flock_path(session, remote));
        assert_ne!(log, shared_control_socket_path(session, remote));
    }
}
