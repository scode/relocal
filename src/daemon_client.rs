//! Client-side daemon interaction for `relocal claude` and `relocal codex`.
//!
//! Connects to an existing session daemon or spawns one if none is running.
//! The returned [`DaemonConnection`] holds the Unix socket open — when it is
//! dropped (or the process exits/crashes), the daemon detects the disconnect.

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use tracing::info;

use crate::error::{Error, Result};
use crate::ssh;

/// A live connection to the session daemon.
///
/// Holds the Unix socket open for the daemon's client tracking. The
/// daemon sees this client as "connected" until the stream is closed,
/// which happens automatically on drop or process exit.
pub struct DaemonConnection {
    _stream: UnixStream,
    control_master_path: PathBuf,
}

impl DaemonConnection {
    /// Path to the shared ControlMaster socket managed by the daemon.
    pub fn control_master_path(&self) -> &Path {
        &self.control_master_path
    }
}

/// Connects to the session daemon, spawning one if necessary.
///
/// On success, returns a [`DaemonConnection`] whose lifetime controls
/// the client's registration with the daemon. If a new daemon is spawned,
/// this function blocks until the daemon signals readiness (meaning it has
/// finished ControlMaster setup, lock acquisition, and initial push).
///
/// This function holds the startup flock while spawning and connecting.
/// The flock is released when this function returns (the `flock_file` is
/// dropped). The daemon does NOT acquire the flock at startup — only at
/// shutdown — to avoid a deadlock where the daemon blocks on flock while
/// the client blocks waiting for the control path message. See the
/// shutdown comments in `daemon::run_daemon` for the full explanation.
pub fn connect_or_spawn(
    session_name: &str,
    remote: &str,
    repo_root: &Path,
    verbosity: u8,
) -> Result<DaemonConnection> {
    let socket_path = ssh::daemon_socket_path(session_name, remote);

    // Fast path: daemon is already running.
    if let Ok(conn) = try_connect(&socket_path) {
        info!("Connected to existing session daemon");
        return Ok(conn);
    }

    // Slow path: acquire flock and spawn daemon if needed.
    let flock_path = ssh::daemon_flock_path(session_name, remote);
    let flock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&flock_path)
        .map_err(|e| Error::DaemonSpawnFailed {
            message: format!("failed to open flock file: {e}"),
        })?;
    // Restrict to owner-only, consistent with the daemon socket (0600).
    let _ = std::fs::set_permissions(&flock_path, std::fs::Permissions::from_mode(0o600));

    // Acquire exclusive lock (blocking). This also blocks if the old daemon
    // is shutting down — it holds this flock through cleanup to prevent us
    // from spawning a replacement that would hit a stale remote lock.
    ssh::acquire_flock(&flock_file)?;

    // Double-check: another process may have started the daemon while we waited.
    if let Ok(conn) = try_connect(&socket_path) {
        info!("Connected to session daemon (started by another process)");
        // flock released on drop of flock_file.
        return Ok(conn);
    }

    // Remove any stale socket file before spawning.
    let _ = std::fs::remove_file(&socket_path);

    // Spawn the daemon.
    info!("Spawning session daemon...");
    let repo_root_str = repo_root.to_str().ok_or_else(|| Error::DaemonSpawnFailed {
        message: "repo root path is not valid UTF-8".to_string(),
    })?;

    // The daemon is the same binary invoked with the hidden `_daemon` subcommand.
    let exe = std::env::current_exe().map_err(|e| Error::DaemonSpawnFailed {
        message: format!("failed to determine current executable: {e}"),
    })?;

    let mut cmd = Command::new(&exe);
    // Propagate the full verbosity level so the daemon gets the same
    // log level as the client (e.g. -vv for DEBUG, -vvv for TRACE).
    for _ in 0..verbosity {
        cmd.arg("-v");
    }
    let mut child = cmd
        .args(["_daemon", session_name, repo_root_str])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| Error::DaemonSpawnFailed {
            message: format!("failed to spawn daemon process: {e}"),
        })?;

    // Wait for the daemon to signal readiness.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::DaemonSpawnFailed {
            message: "daemon stdout not captured".to_string(),
        })?;

    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| Error::DaemonSpawnFailed {
            message: format!("failed to read daemon readiness signal: {e}"),
        })?;

    if line.trim() != "READY" {
        // Daemon exited or wrote something unexpected. Try to get its exit status.
        let status = child.wait().ok();
        return Err(Error::DaemonSpawnFailed {
            message: format!(
                "daemon did not signal readiness (got {:?}, exit: {:?})",
                line.trim(),
                status
            ),
        });
    }

    info!("Session daemon is ready");

    // Reap the daemon process in a background thread to avoid zombies.
    // The daemon outlives this client, so we can't wait synchronously.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    // Connect to the now-running daemon.
    let conn = try_connect(&socket_path).map_err(|_| Error::DaemonSpawnFailed {
        message: "daemon signaled ready but socket connection failed".to_string(),
    })?;

    // flock released on drop of flock_file.
    Ok(conn)
}

/// Attempts to connect to an existing daemon socket and read the control path.
///
/// Uses `BufReader::read_line` rather than a raw `read()` to correctly
/// handle the newline-terminated framing — a single `read()` is not
/// guaranteed to return the complete message even on Unix domain sockets.
fn try_connect(socket_path: &Path) -> std::result::Result<DaemonConnection, ()> {
    let stream = UnixStream::connect(socket_path).map_err(|_| ())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|_| ())?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|_| ())?;

    let path_str = line.trim_end_matches('\n');
    if path_str.is_empty() {
        return Err(());
    }

    Ok(DaemonConnection {
        _stream: reader.into_inner(),
        control_master_path: PathBuf::from(path_str),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    #[test]
    fn try_connect_reads_control_path() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        // Spawn an acceptor that sends a fake control path.
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"/tmp/rlc-test-12345678\n").unwrap();
            // Keep stream alive until test reads.
            std::thread::sleep(Duration::from_secs(1));
        });

        let conn = try_connect(&sock_path).unwrap();
        assert_eq!(
            conn.control_master_path(),
            Path::new("/tmp/rlc-test-12345678")
        );

        drop(conn);
        let _ = handle.join();
    }

    #[test]
    fn try_connect_fails_on_missing_socket() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("nonexistent.sock");
        assert!(try_connect(&sock_path).is_err());
    }

    #[test]
    fn try_connect_fails_on_empty_response() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            // Close immediately — client gets EOF.
            drop(stream);
        });

        assert!(try_connect(&sock_path).is_err());
        let _ = handle.join();
    }

    #[test]
    fn daemon_connection_drop_closes_stream() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"/tmp/control\n").unwrap();
            // Wait for client disconnect.
            let mut buf = [0u8; 1];
            stream.read(&mut buf).unwrap() // Should be 0 (EOF).
        });

        let conn = try_connect(&sock_path).unwrap();
        drop(conn);

        let bytes_read = handle.join().unwrap();
        assert_eq!(bytes_read, 0);
    }

    #[test]
    fn flock_serializes_access() {
        let dir = tempfile::tempdir().unwrap();
        let flock_path = dir.path().join("test.flock");

        let file1 = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&flock_path)
            .unwrap();
        ssh::acquire_flock(&file1).unwrap();

        // Try non-blocking flock from another thread — should fail.
        let flock_path_clone = flock_path.clone();
        let handle = std::thread::spawn(move || {
            let file2 = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&flock_path_clone)
                .unwrap();
            use std::os::fd::AsRawFd;
            // -1 means EWOULDBLOCK (lock held by file1)
            unsafe { libc::flock(file2.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }
        });

        let ret = handle.join().unwrap();
        assert_eq!(ret, -1, "flock should fail while held");

        // Release and verify re-acquisition.
        drop(file1);
        let file3 = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&flock_path)
            .unwrap();
        ssh::acquire_flock(&file3).unwrap(); // Should succeed now.
    }
}
