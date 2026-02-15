//! Sync sidecar — background mediator between remote hooks and local rsync.
//!
//! The sidecar reads sync requests from a remote FIFO via SSH, runs the
//! appropriate rsync (push or pull), and writes an ack back to the remote.
//! It runs on a background thread managed by [`Sidecar`], which provides
//! a [`Sidecar::shutdown`] method for clean termination.
//!
//! The request-handling logic is in [`handle_request`], a pure orchestration
//! function testable with [`MockRunner`](crate::test_support::MockRunner).

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use tracing::warn;

use crate::commands::sync::{sync_pull, sync_push};
use crate::config::Config;
use crate::error::Result;
use crate::runner::CommandRunner;
use crate::ssh;

/// Manages a background thread that reads sync requests from the remote FIFO
/// and dispatches rsync + ack operations.
pub struct Sidecar {
    thread: Option<JoinHandle<()>>,
    ssh_child: Option<Child>,
    shutdown_flag: Arc<AtomicBool>,
}

impl Sidecar {
    /// Starts the sidecar background thread.
    ///
    /// Opens an SSH connection that reads from the session's request FIFO in a
    /// loop. Each line triggers an rsync operation and ack write.
    pub fn start(
        runner: Arc<dyn CommandRunner + Send + Sync>,
        config: Config,
        session_name: String,
        repo_root: PathBuf,
        verbose: bool,
    ) -> Result<Self> {
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let flag_clone = shutdown_flag.clone();

        let fifo_cmd = ssh::read_request_fifo(&session_name);
        let mut child = Command::new("ssh")
            .args([&config.remote, &fifo_cmd])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()?;

        let stdout = child.stdout.take().expect("stdout was piped");

        let thread = thread::spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                if flag_clone.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(request) = line else {
                    break;
                };
                let trimmed = request.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }

                let result = handle_request(
                    runner.as_ref(),
                    &config,
                    &session_name,
                    &repo_root,
                    verbose,
                    &trimmed,
                );

                // Write ack regardless of whether we're shutting down — the
                // remote hook is blocking on it.
                let ack_msg = match &result {
                    Ok(()) => "ok".to_string(),
                    Err(e) => format!("error:{e}"),
                };
                let _ = runner.run_ssh(&config.remote, &ssh::write_ack(&session_name, &ack_msg));
            }
        });

        Ok(Sidecar {
            thread: Some(thread),
            ssh_child: Some(child),
            shutdown_flag,
        })
    }

    /// Signals the sidecar to stop and waits for the background thread to exit.
    ///
    /// Kills the SSH process reading the FIFO (which unblocks the reader thread),
    /// then joins the thread.
    pub fn shutdown(&mut self) {
        self.shutdown_flag.store(true, Ordering::Relaxed);

        if let Some(ref mut child) = self.ssh_child {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.ssh_child = None;

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Handles a single sync request by running rsync in the appropriate direction.
///
/// This is the core logic, separated from the threading/SSH-process concerns
/// so it can be tested with [`MockRunner`](crate::test_support::MockRunner).
pub fn handle_request(
    runner: &dyn CommandRunner,
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
    request: &str,
) -> Result<()> {
    match request {
        "push" => sync_push(runner, config, session_name, repo_root, verbose),
        "pull" => sync_pull(runner, config, session_name, repo_root, verbose),
        other => {
            warn!("Sidecar: unknown request: {other}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Invocation, MockResponse, MockRunner};

    fn test_config() -> Config {
        Config::parse("remote = \"user@host\"").unwrap()
    }

    fn repo_root() -> PathBuf {
        PathBuf::from("/home/user/my-project")
    }

    #[test]
    fn push_request_triggers_rsync() {
        let mock = MockRunner::new();
        // rsync (push)
        mock.add_response(MockResponse::Ok(String::new()));

        handle_request(&mock, &test_config(), "s1", &repo_root(), false, "push").unwrap();

        let inv = mock.invocations();
        // Just rsync, no hook reinjection
        assert_eq!(inv.len(), 1);

        match &inv[0] {
            Invocation::Rsync { args, .. } => {
                let last = args.last().unwrap();
                assert!(last.contains("user@host:"));
            }
            _ => panic!("expected Rsync, got {:?}", inv[0]),
        }
    }

    #[test]
    fn pull_request_triggers_fsck_then_rsync() {
        let mock = MockRunner::new();
        // git fsck
        mock.add_response(MockResponse::Ok(String::new()));
        // rsync (pull)
        mock.add_response(MockResponse::Ok(String::new()));

        handle_request(&mock, &test_config(), "s1", &repo_root(), false, "pull").unwrap();

        let inv = mock.invocations();
        // git fsck (1) + rsync (1), no hook reinjection
        assert_eq!(inv.len(), 2);
        match &inv[0] {
            Invocation::Ssh { command, .. } => {
                assert!(command.contains("git fsck"));
            }
            _ => panic!("expected Ssh for git fsck"),
        }
        match &inv[1] {
            Invocation::Rsync { args, .. } => {
                let last = args.last().unwrap();
                assert!(last.starts_with("/home/user/my-project/"));
            }
            _ => panic!("expected Rsync"),
        }
    }

    #[test]
    fn rsync_failure_returns_error() {
        let mock = MockRunner::new();
        // rsync fails
        mock.add_response(MockResponse::Err("rsync failed".into()));

        let result = handle_request(&mock, &test_config(), "s1", &repo_root(), false, "push");
        assert!(result.is_err());
    }

    #[test]
    fn unknown_request_is_ignored() {
        let mock = MockRunner::new();
        // No responses needed — unknown request doesn't trigger any commands

        handle_request(&mock, &test_config(), "s1", &repo_root(), false, "bogus").unwrap();

        let inv = mock.invocations();
        assert!(inv.is_empty());
    }

    #[test]
    fn multiple_sequential_requests() {
        let mock = MockRunner::new();
        // First request: push
        mock.add_response(MockResponse::Ok(String::new())); // rsync

        // Second request: pull
        mock.add_response(MockResponse::Ok(String::new())); // git fsck
        mock.add_response(MockResponse::Ok(String::new())); // rsync

        // Third request: push
        mock.add_response(MockResponse::Ok(String::new())); // rsync

        handle_request(&mock, &test_config(), "s1", &repo_root(), false, "push").unwrap();
        handle_request(&mock, &test_config(), "s1", &repo_root(), false, "pull").unwrap();
        handle_request(&mock, &test_config(), "s1", &repo_root(), false, "push").unwrap();

        let inv = mock.invocations();
        // push(1) + pull(2: fsck+rsync) + push(1) = 4
        assert_eq!(inv.len(), 4);

        // Verify pattern
        assert!(matches!(&inv[0], Invocation::Rsync { .. })); // push rsync
        assert!(matches!(&inv[1], Invocation::Ssh { .. })); // pull fsck
        assert!(matches!(&inv[2], Invocation::Rsync { .. })); // pull rsync
        assert!(matches!(&inv[3], Invocation::Rsync { .. })); // push rsync
    }

    #[test]
    fn push_with_verbose_flag() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new())); // rsync

        handle_request(&mock, &test_config(), "s1", &repo_root(), true, "push").unwrap();

        let inv = mock.invocations();
        match &inv[0] {
            Invocation::Rsync { args, .. } => {
                assert!(args.contains(&"--progress".to_string()));
            }
            _ => panic!("expected Rsync"),
        }
    }

    #[test]
    fn correct_session_name_in_operations() {
        let mock = MockRunner::new();
        mock.add_response(MockResponse::Ok(String::new())); // rsync

        handle_request(
            &mock,
            &test_config(),
            "my-project",
            &repo_root(),
            false,
            "push",
        )
        .unwrap();

        let inv = mock.invocations();
        assert_eq!(inv.len(), 1);
        // Rsync uses session name in remote path
        match &inv[0] {
            Invocation::Rsync { args, .. } => {
                let last = args.last().unwrap();
                assert!(last.contains("my-project"));
            }
            _ => panic!("expected Rsync"),
        }
    }
}
