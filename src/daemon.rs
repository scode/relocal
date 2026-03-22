//! Session daemon — owns shared infrastructure for concurrent tool invocations.
//!
//! The daemon manages the SSH ControlMaster, background sync loop, and remote
//! lock file for a session. It listens on a Unix domain socket for client
//! connections and tears down when the last client disconnects.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use tracing::{debug, info, warn};

use crate::commands::sync::{sync_pull, sync_push};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::runner::ProcessRunner;
use crate::ssh::{self, SshControlMaster};

const SYNC_INTERVAL: Duration = Duration::from_secs(3);

/// How long the daemon waits for the first client before giving up. If the
/// spawning process dies between READY and connect, this prevents the daemon
/// from running forever with zero clients.
const INITIAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs the session daemon. Called via the hidden `_daemon` subcommand.
///
/// Performs setup (ControlMaster, remote dir, lock, push), signals readiness
/// on stdout, then enters the poll loop until the last client disconnects.
/// After the loop exits: final sync pull, lock removal, socket cleanup,
/// ControlMaster teardown.
///
/// The startup flock is NOT acquired here — it is acquired at shutdown only.
/// Acquiring it at startup would deadlock: the spawning client holds the
/// flock while waiting for the control path message, but the daemon can't
/// send the control path without entering the poll loop, which it can't do
/// while blocked on the flock. See the shutdown section for details.
pub fn run_daemon(
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
) -> Result<()> {
    info!("Connecting to {}...", config.remote);
    debug!("Establishing SSH ControlMaster...");
    let control_master = SshControlMaster::start_shared(&config.remote, session_name)?;
    debug!(
        "ControlMaster established at {}",
        control_master.socket_path().display()
    );
    let runner = ProcessRunner::with_control_path(control_master.socket_path());

    daemon_setup(&runner, config, session_name, repo_root, verbose)?;

    let socket_path = ssh::daemon_socket_path(session_name, &config.remote);
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path).map_err(|e| Error::DaemonSpawnFailed {
        message: format!("failed to bind daemon socket: {e}"),
    })?;
    // Restrict socket to owner-only access.
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)).map_err(
        |e| Error::DaemonSpawnFailed {
            message: format!("failed to set socket permissions: {e}"),
        },
    )?;
    listener
        .set_nonblocking(true)
        .map_err(|e| Error::DaemonSpawnFailed {
            message: format!("failed to set socket nonblocking: {e}"),
        })?;

    debug!("Daemon socket bound at {}", socket_path.display());

    // Signal readiness to the spawning client.
    debug!("Signaling READY to client...");
    let _ = std::io::stdout().write_all(b"READY\n");
    let _ = std::io::stdout().flush();
    // The spawning client blocks on read_line() waiting for READY. Closing
    // our end of the stdout pipe unblocks it.
    drop(std::io::stdout());

    let control_path_msg = format!("{}\n", control_master.socket_path().display());

    let exit_result = poll_loop(
        &listener,
        &runner,
        config,
        session_name,
        repo_root,
        verbose,
        &control_path_msg,
    );

    // Stop accepting new connections.
    drop(listener);

    // Acquire the startup flock before cleanup. This prevents a race: a new
    // client that sees ECONNREFUSED (listener gone) would normally acquire
    // the flock, spawn a fresh daemon, and hit StaleSession because our
    // remote lock is still present. By holding the flock through cleanup,
    // new clients block until we finish and exit.
    //
    // We acquire the flock here (at shutdown) rather than at startup to avoid
    // a deadlock: at startup the spawning client holds the flock while waiting
    // for READY, and the daemon can't enter the poll loop until the client
    // releases it, but the client can't release it until it connects, which
    // requires the poll loop to be running to accept and send the control path.
    //
    // Best-effort: if we can't open or lock the file, proceed with cleanup
    // anyway — a stuck flock is worse than a brief StaleSession race.
    let flock_path = ssh::daemon_flock_path(session_name, &config.remote);
    let _shutdown_flock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&flock_path)
        .ok()
        .and_then(|f| ssh::acquire_flock(&f).ok().map(|()| f));
    // _shutdown_flock is held (not dropped) until run_daemon returns.

    info!("Pulling final changes from remote...");
    if let Err(e) = sync_pull(&runner, config, session_name, repo_root, verbose) {
        warn!("Final sync pull failed: {e}");
    }
    if let Err(e) = cleanup(&runner, config, session_name) {
        warn!("Lock file cleanup failed: {e}");
    }
    drop(control_master);

    // Now safe to remove the socket — remote lock is gone.
    let _ = std::fs::remove_file(&socket_path);

    exit_result
}

/// Daemon-specific setup: stale session check, remote dir, lock, initial push.
///
/// Does NOT check tool installation — the daemon is tool-agnostic. Tool
/// checks are the client's responsibility.
pub fn daemon_setup(
    runner: &dyn crate::runner::CommandRunner,
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
) -> Result<()> {
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
    debug!("No stale session found");

    info!("Creating remote working directory...");
    runner
        .run_ssh(&config.remote, &ssh::mkdir_work_dir(session_name))?
        .check("mkdir")?;
    debug!("Remote directory created");

    runner
        .run_ssh(&config.remote, &ssh::create_lock_file(session_name))?
        .check("create lock file")?;
    debug!("Lock file created");

    debug!("Starting initial rsync push...");
    sync_push(runner, config, session_name, repo_root, verbose)?;
    debug!("Initial rsync push complete");

    Ok(())
}

/// Post-session cleanup: remove lock file (best-effort).
fn cleanup(
    runner: &dyn crate::runner::CommandRunner,
    config: &Config,
    session_name: &str,
) -> Result<()> {
    info!("Removing lock file...");
    runner
        .run_ssh(&config.remote, &ssh::remove_lock_file(session_name))?
        .check("remove lock file")?;
    Ok(())
}

/// Main event loop: accept clients, detect disconnects, sync on timeout.
fn poll_loop(
    listener: &UnixListener,
    runner: &ProcessRunner,
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
    control_path_msg: &str,
) -> Result<()> {
    let mut clients: Vec<UnixStream> = Vec::new();
    let mut ever_had_client = false;
    let started = std::time::Instant::now();

    loop {
        // If no client has ever connected and the timeout has elapsed, the
        // spawning process likely died between READY and connect. Exit to
        // avoid holding the remote lock indefinitely.
        if !ever_had_client && started.elapsed() > INITIAL_CONNECT_TIMEOUT {
            warn!(
                "No client connected within {:?}, shutting down",
                INITIAL_CONNECT_TIMEOUT
            );
            return Ok(());
        }

        // Collect raw fds for polling. We can't hold BorrowedFds across
        // the mutation of `clients` below, so snapshot the raw fds first.
        let listener_fd = listener.as_raw_fd();
        let client_fds: Vec<i32> = clients.iter().map(|c| c.as_raw_fd()).collect();

        // SAFETY: the listener and client streams outlive this poll call.
        let mut poll_fds: Vec<PollFd> = Vec::with_capacity(1 + client_fds.len());
        poll_fds.push(PollFd::new(
            unsafe { BorrowedFd::borrow_raw(listener_fd) },
            PollFlags::POLLIN,
        ));
        for &fd in &client_fds {
            poll_fds.push(PollFd::new(
                unsafe { BorrowedFd::borrow_raw(fd) },
                PollFlags::POLLIN | PollFlags::POLLHUP,
            ));
        }

        let timeout_ms: u16 = SYNC_INTERVAL
            .as_millis()
            .try_into()
            .expect("SYNC_INTERVAL must fit in u16 milliseconds");
        let n = poll(&mut poll_fds, PollTimeout::from(timeout_ms)).map_err(|e| {
            Error::DaemonSpawnFailed {
                message: format!("poll error: {e}"),
            }
        })?;

        // Snapshot revents before dropping poll_fds (which hold BorrowedFds).
        let listener_events = poll_fds[0].revents();
        let client_events: Vec<Option<PollFlags>> =
            poll_fds[1..].iter().map(|pfd| pfd.revents()).collect();
        drop(poll_fds);

        if n == 0 {
            // Timeout — run sync.
            if !clients.is_empty() {
                if let Err(e) = sync_pull(runner, config, session_name, repo_root, verbose) {
                    warn!("background sync failed: {e}");
                }
            }
            continue;
        }

        // Check listener for new connections.
        if let Some(events) = listener_events {
            if events.contains(PollFlags::POLLIN) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Best-effort: send control path. If the client already
                        // disconnected, write_all fails but we still add the
                        // stream — the disconnect will be detected on the next
                        // poll iteration via the normal EOF path. We must NOT
                        // skip adding the stream on write failure, because that
                        // would leave `clients` empty, and the "last client
                        // disconnected" exit condition would never trigger.
                        if let Err(e) = stream.write_all(control_path_msg.as_bytes()) {
                            warn!("Failed to send control path to client: {e}");
                        }

                        // set_nonblocking is critical: a blocking stream in the
                        // poll set would freeze the entire event loop when
                        // read() is called in the disconnect-detection path.
                        // This is the only case where we must reject the client.
                        if stream.set_nonblocking(true).is_err() {
                            warn!("Failed to set client socket nonblocking, dropping");
                        } else {
                            clients.push(stream);
                            ever_had_client = true;
                            info!("Client connected (total: {})", clients.len());
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        warn!("accept error: {e}");
                    }
                }
            }
        }

        // Check clients for disconnect (EOF or hangup). Iterate in reverse
        // so that removals don't shift indices of unprocessed entries.
        for i in (0..client_events.len()).rev() {
            if let Some(events) = client_events[i] {
                if events.intersects(PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR) {
                    let mut buf = [0u8; 1];
                    match clients[i].read(&mut buf) {
                        Ok(0) | Err(_) => {
                            clients.remove(i);
                            info!("Client disconnected (remaining: {})", clients.len());
                        }
                        Ok(_) => {}
                    }
                }
            }
        }
        if clients.is_empty() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::ssh::{STATUS_CHECK_FALSE, STATUS_CHECK_TRUE};
    use crate::test_support::{Invocation, MockResponse, MockRunner};

    fn test_config() -> Config {
        Config::parse("remote = \"user@host\"").unwrap()
    }

    fn repo_root() -> PathBuf {
        PathBuf::from("/home/user/my-project")
    }

    #[test]
    fn daemon_setup_full_sequence() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // lock check
        mock.add_response(MockResponse::Ok(String::new())); // mkdir
        mock.add_response(MockResponse::Ok(String::new())); // lock create
        mock.add_response(MockResponse::Ok(String::new())); // rsync push

        daemon_setup(&mock, &test_config(), "my-session", &repo_root(), false).unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 4);

        // lock check (wrapped)
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("test -e"));
                assert!(command.contains(".locks"));
            }
            _ => panic!("expected Ssh for lock check"),
        }

        // mkdir work dir
        match &inv[1] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("mkdir -p"));
                assert!(command.contains("my-session"));
            }
            _ => panic!("expected Ssh for mkdir"),
        }

        // lock file creation
        match &inv[2] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("noclobber"));
                assert!(command.contains(".locks"));
            }
            _ => panic!("expected Ssh for lock creation"),
        }

        // rsync (push)
        assert!(matches!(&inv[3], Invocation::Rsync { .. }));
    }

    #[test]
    fn daemon_setup_no_tool_check() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // lock check
        mock.add_response(MockResponse::Ok(String::new())); // mkdir
        mock.add_response(MockResponse::Ok(String::new())); // lock create
        mock.add_response(MockResponse::Ok(String::new())); // rsync push

        daemon_setup(&mock, &test_config(), "s1", &repo_root(), false).unwrap();

        // Should be 4 invocations — no tool check (that's the client's job).
        let inv = mock.invocations();
        assert_eq!(inv.len(), 4);
        // Verify none of them check for a tool binary.
        for i in &inv {
            if let Invocation::Ssh { command, .. } = i {
                assert!(!command.contains("command -v claude"));
                assert!(!command.contains("command -v codex"));
            }
        }
    }

    #[test]
    fn daemon_setup_stale_session_detected() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_TRUE.into())); // lock exists

        let result = daemon_setup(&mock, &test_config(), "stale-session", &repo_root(), false);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::StaleSession { .. }));
        assert_eq!(mock.invocations().len(), 1);
    }

    #[test]
    fn daemon_setup_fails_if_mkdir_fails() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // lock check
        mock.add_response(MockResponse::Fail("permission denied".into())); // mkdir fails

        let result = daemon_setup(&mock, &test_config(), "s1", &repo_root(), false);
        assert!(result.is_err());
        assert_eq!(mock.invocations().len(), 2);
    }

    #[test]
    fn daemon_setup_fails_if_lock_creation_fails() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(STATUS_CHECK_FALSE.into())); // lock check
        mock.add_response(MockResponse::Ok(String::new())); // mkdir
        mock.add_response(MockResponse::Fail("noclobber: file exists".into())); // lock fails

        let result = daemon_setup(&mock, &test_config(), "s1", &repo_root(), false);
        assert!(result.is_err());
        assert_eq!(mock.invocations().len(), 3);
    }

    #[test]
    fn poll_loop_exits_when_last_client_disconnects() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        listener.set_nonblocking(true).unwrap();

        let control_path_msg = "/tmp/fake-control-path\n";

        // Connect a client, then immediately drop it.
        let client = UnixStream::connect(&sock_path).unwrap();
        drop(client);

        // Give the kernel a moment to propagate the close.
        std::thread::sleep(Duration::from_millis(50));

        // The poll loop should accept the client, detect disconnect, and return.
        let runner = ProcessRunner::default();
        let config = test_config();
        let result = poll_loop(
            &listener,
            &runner,
            &config,
            "s1",
            &repo_root(),
            false,
            control_path_msg,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn poll_loop_sends_control_path_to_client() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        listener.set_nonblocking(true).unwrap();

        let control_path_msg = "/tmp/rlc-my-session-abcdef01\n";

        // Connect client before moving listener to the poll loop thread.
        let mut client = UnixStream::connect(&sock_path).unwrap();

        let handle = std::thread::spawn(move || {
            let runner = ProcessRunner::default();
            let config = Config::parse("remote = \"user@host\"").unwrap();
            poll_loop(
                &listener,
                &runner,
                &config,
                "s1",
                &PathBuf::from("/tmp/fake"),
                false,
                control_path_msg,
            )
        });

        // Read the control path sent by the daemon.
        let mut buf = [0u8; 256];
        std::thread::sleep(Duration::from_millis(100));
        let n = client.read(&mut buf).unwrap();
        let received = std::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(received, "/tmp/rlc-my-session-abcdef01\n");

        // Disconnect to let the poll loop exit.
        drop(client);
        let result = handle.join().unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn poll_loop_stays_alive_while_clients_remain() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        listener.set_nonblocking(true).unwrap();

        let control_path_msg = "/tmp/control\n";

        // Connect one client before moving the listener. This client will
        // stay connected to keep the loop alive while we connect and
        // disconnect a second client via the socket path.
        let _keeper = UnixStream::connect(&sock_path).unwrap();
        let sock_path_clone = sock_path.clone();

        let handle = std::thread::spawn(move || {
            let runner = ProcessRunner::default();
            let config = Config::parse("remote = \"user@host\"").unwrap();
            poll_loop(
                &listener,
                &runner,
                &config,
                "s1",
                &PathBuf::from("/tmp/fake"),
                false,
                control_path_msg,
            )
        });

        // Give the poll loop time to start and accept the keeper client.
        std::thread::sleep(Duration::from_millis(200));

        // Connect and disconnect a second client — the loop should survive.
        let transient = UnixStream::connect(&sock_path_clone).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        drop(transient);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!handle.is_finished(), "loop should still be running");

        // Drop the keeper — now the loop should exit.
        drop(_keeper);
        let result = handle.join().unwrap();
        assert!(result.is_ok());
    }
}
