//! Background sync loop — continuously pulls remote changes to local.
//!
//! A background thread runs `sync_pull` on a fixed interval while the
//! interactive session is active. Uses `mpsc::recv_timeout` for clean,
//! fast shutdown.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tracing::warn;

use crate::commands::sync::sync_pull;
use crate::config::Config;
use crate::error::Result;
use crate::runner::CommandRunner;

/// How often the background loop runs sync_pull.
const SYNC_INTERVAL: Duration = Duration::from_secs(3);

/// Manages a background thread that periodically syncs remote changes to local.
pub struct Sidecar {
    thread: Option<JoinHandle<()>>,
    shutdown_sender: Option<mpsc::Sender<()>>,
}

impl Sidecar {
    /// Starts the background sync loop.
    ///
    /// The loop runs `sync_pull` every [`SYNC_INTERVAL`] seconds. Transient
    /// failures are logged as warnings and do not stop the loop.
    pub fn start(
        runner: Arc<dyn CommandRunner + Send + Sync>,
        config: Config,
        session_name: String,
        repo_root: PathBuf,
        verbose: bool,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::channel();

        let thread = thread::spawn(move || {
            while let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(SYNC_INTERVAL) {
                if let Err(e) =
                    sync_pull(runner.as_ref(), &config, &session_name, &repo_root, verbose)
                {
                    warn!("background sync failed: {e}");
                }
            }
        });

        Ok(Self {
            thread: Some(thread),
            shutdown_sender: Some(tx),
        })
    }

    /// Signals the background loop to stop and waits for it to exit.
    ///
    /// Dropping the channel sender unblocks `recv_timeout` immediately,
    /// giving sub-millisecond shutdown latency.
    pub fn shutdown(&mut self) {
        self.shutdown_sender.take();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::CommandOutput;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    /// Thread-safe mock runner for sidecar tests.
    struct ThreadSafeRunner;

    fn ok_output() -> CommandOutput {
        CommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            status: ExitStatus::from_raw(0),
        }
    }

    impl CommandRunner for ThreadSafeRunner {
        fn run_ssh(&self, _remote: &str, _command: &str) -> crate::error::Result<CommandOutput> {
            Ok(ok_output())
        }
        fn run_ssh_interactive(
            &self,
            _remote: &str,
            _command: &str,
        ) -> crate::error::Result<ExitStatus> {
            Ok(ExitStatus::from_raw(0))
        }
        fn run_rsync(
            &self,
            _params: &crate::rsync::RsyncParams,
        ) -> crate::error::Result<CommandOutput> {
            Ok(ok_output())
        }
        fn run_local(&self, _program: &str, _args: &[&str]) -> crate::error::Result<CommandOutput> {
            Ok(ok_output())
        }
    }

    fn test_config() -> Config {
        Config::parse("remote = \"user@host\"").unwrap()
    }

    fn repo_root() -> PathBuf {
        PathBuf::from("/home/user/my-project")
    }

    #[test]
    fn shutdown_is_immediate() {
        let runner = Arc::new(ThreadSafeRunner);

        let mut sidecar =
            Sidecar::start(runner, test_config(), "s1".into(), repo_root(), false).unwrap();

        // Shutdown should return quickly (well within 1 second, not waiting
        // for the full 3-second interval)
        let start = std::time::Instant::now();
        sidecar.shutdown();
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn drop_shuts_down_cleanly() {
        let runner = Arc::new(ThreadSafeRunner);

        let sidecar =
            Sidecar::start(runner, test_config(), "s1".into(), repo_root(), false).unwrap();

        // Dropping should not panic or hang
        drop(sidecar);
    }
}
