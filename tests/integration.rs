//! Integration tests for relocal.
//!
//! These tests exercise real SSH, rsync, and filesystem operations against a
//! remote host (which may be localhost). They are gated on the
//! `RELOCAL_TEST_REMOTE` environment variable — when unset, all tests
//! are `#[ignore]`d.
//!
//! Tests share remote state (`~/relocal/`) and must run sequentially:
//!
//! ```sh
//! RELOCAL_TEST_REMOTE=$USER@localhost cargo test -- --ignored --test-threads=1
//! ```

use std::sync::Arc;

use relocal::commands::{destroy, nuke, sync};
use relocal::config::Config;
use relocal::daemon;
use relocal::daemon_client;
use relocal::runner::{CommandRunner, ProcessRunner};
use relocal::sidecar::Sidecar;
use relocal::ssh;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

/// Returns the test remote from the environment, or None if not set.
fn test_remote() -> Option<String> {
    std::env::var("RELOCAL_TEST_REMOTE").ok()
}

/// Generates a unique session name for a test to avoid collisions.
fn unique_session(test_name: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("test-{test_name}-{ts}")
}

/// Creates a local temp directory with a `relocal.toml` file.
fn make_local_repo(remote: &str) -> (tempfile::TempDir, Config) {
    make_local_repo_with_excludes(remote, &[])
}

fn make_local_repo_with_excludes(remote: &str, excludes: &[&str]) -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().expect("create temp dir");

    // Initialize a git repo so the remote will pass git fsck on pull.
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .expect("git init");

    let mut toml = format!("remote = \"{remote}\"\n");
    if !excludes.is_empty() {
        let list = excludes
            .iter()
            .map(|e| format!("\"{e}\""))
            .collect::<Vec<_>>()
            .join(", ");
        toml.push_str(&format!("exclude = [{list}]\n"));
    }
    std::fs::write(dir.path().join("relocal.toml"), &toml).unwrap();
    let config = Config::parse(&toml).unwrap();
    (dir, config)
}

/// Creates a local temp directory with only a `.git` repo (no `relocal.toml`).
/// Used to test config-free workflows where config comes from user config alone.
fn make_local_repo_git_only(remote: &str) -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().expect("create temp dir");

    let git_init = std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .expect("git init");
    assert!(git_init.status.success(), "git init failed");

    // No relocal.toml — config must come from user config or be passed directly.
    let toml = format!("remote = \"{remote}\"\n");
    let config = Config::parse(&toml).unwrap();
    (dir, config)
}

/// Returns the path to the compiled `relocal` binary for integration tests.
fn relocal_bin() -> &'static str {
    env!("CARGO_BIN_EXE_relocal")
}

/// RAII guard that cleans up remote state on drop (even on panic).
struct RemoteCleanup {
    remote: String,
    session: String,
}

impl Drop for RemoteCleanup {
    fn drop(&mut self) {
        let runner = ProcessRunner::default();
        // Best-effort cleanup
        let _ = runner.run_ssh(&self.remote, &ssh::rm_work_dir(&self.session));
    }
}

/// Reads a file from the remote via SSH.
fn read_remote_file(remote: &str, path: &str) -> Option<String> {
    let runner = ProcessRunner::default();
    let out = runner.run_ssh(remote, &format!("cat {path}")).ok()?;
    if out.status.success() {
        Some(out.stdout)
    } else {
        None
    }
}

/// Writes a file on the remote via SSH.
fn write_remote_file(remote: &str, path: &str, content: &str) {
    let runner = ProcessRunner::default();
    let cmd = format!("mkdir -p $(dirname {path}) && printf '%s' '{content}' > {path}");
    runner.run_ssh(remote, &cmd).expect("write remote file");
}

/// Checks if a remote file exists.
fn remote_file_exists(remote: &str, path: &str) -> bool {
    let runner = ProcessRunner::default();
    runner
        .run_ssh(remote, &format!("test -e {path}"))
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Returns the remote working directory path for a session.
fn remote_dir(session: &str) -> String {
    ssh::remote_work_dir(session)
}

/// Ensures the remote session directory exists (for tests that call sync directly
/// without going through `daemon::daemon_setup`).
fn ensure_remote_session_dir(remote: &str, session: &str) {
    let runner = ProcessRunner::default();
    runner
        .run_ssh(remote, &ssh::mkdir_work_dir(session))
        .expect("create remote session dir");
}

// ---------------------------------------------------------------------------
// Sync push tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn push_files_appear_on_remote() {
    let remote = test_remote().unwrap();
    let session = unique_session("push-appear");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();
    ensure_remote_session_dir(&remote, &session);

    // Create a local file
    std::fs::write(dir.path().join("hello.txt"), "world").unwrap();

    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    let content = read_remote_file(&remote, &format!("{}/hello.txt", remote_dir(&session)));
    assert_eq!(content.as_deref(), Some("world"));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn push_deletes_propagate() {
    let remote = test_remote().unwrap();
    let session = unique_session("push-delete");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();
    ensure_remote_session_dir(&remote, &session);

    // Push a file
    std::fs::write(dir.path().join("delete-me.txt"), "temp").unwrap();
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();
    assert!(remote_file_exists(
        &remote,
        &format!("{}/delete-me.txt", remote_dir(&session))
    ));

    // Delete locally and push again
    std::fs::remove_file(dir.path().join("delete-me.txt")).unwrap();
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();
    assert!(!remote_file_exists(
        &remote,
        &format!("{}/delete-me.txt", remote_dir(&session))
    ));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn push_respects_gitignore() {
    let remote = test_remote().unwrap();
    let session = unique_session("push-gitignore");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();
    ensure_remote_session_dir(&remote, &session);

    std::fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(dir.path().join("app.log"), "log data").unwrap();
    std::fs::write(dir.path().join("keep.txt"), "keep").unwrap();

    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    assert!(!remote_file_exists(
        &remote,
        &format!("{}/app.log", remote_dir(&session))
    ));
    assert!(remote_file_exists(
        &remote,
        &format!("{}/keep.txt", remote_dir(&session))
    ));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn push_respects_config_excludes() {
    let remote = test_remote().unwrap();
    let session = unique_session("push-exclude");
    let (dir, config) = make_local_repo_with_excludes(&remote, &[".env", "secrets/"]);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();
    ensure_remote_session_dir(&remote, &session);

    std::fs::write(dir.path().join(".env"), "SECRET=x").unwrap();
    std::fs::create_dir(dir.path().join("secrets")).unwrap();
    std::fs::write(dir.path().join("secrets/key.pem"), "key").unwrap();
    std::fs::write(dir.path().join("normal.txt"), "ok").unwrap();

    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    assert!(!remote_file_exists(
        &remote,
        &format!("{}/.env", remote_dir(&session))
    ));
    assert!(!remote_file_exists(
        &remote,
        &format!("{}/secrets/key.pem", remote_dir(&session))
    ));
    assert!(remote_file_exists(
        &remote,
        &format!("{}/normal.txt", remote_dir(&session))
    ));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn push_excludes_claude_dir() {
    let remote = test_remote().unwrap();
    let session = unique_session("push-no-claude");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();
    ensure_remote_session_dir(&remote, &session);

    // Create .claude/ content locally
    std::fs::create_dir_all(dir.path().join(".claude/skills")).unwrap();
    std::fs::write(dir.path().join(".claude/skills/my-skill.md"), "skill").unwrap();
    std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
    std::fs::write(dir.path().join(".claude/settings.json"), "{}").unwrap();

    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    // Nothing under .claude/ should be synced
    assert!(!remote_file_exists(
        &remote,
        &format!("{}/.claude/skills/my-skill.md", remote_dir(&session))
    ));
    assert!(!remote_file_exists(
        &remote,
        &format!("{}/.claude/settings.json", remote_dir(&session))
    ));
}

// ---------------------------------------------------------------------------
// Config-free (git-only) workflow tests
// ---------------------------------------------------------------------------

/// Exercises the actual binary in a .git-only repo (no relocal.toml).
///
/// Runs `relocal status <session>` from a directory that has `.git` but no
/// `relocal.toml`. This exercises repo root discovery (the .git fallback)
/// and merged config loading through the real binary, not just library calls.
///
/// The user's `~/.relocal/config.toml` must provide `remote` for this test
/// to pass. If it doesn't exist, the test verifies the error is about config
/// (missing `remote`), not about discovery (missing `relocal.toml`).
#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn git_only_repo_discovery_through_binary() {
    let remote = test_remote().unwrap();
    let session = unique_session("git-only-bin");
    let (dir, _config) = make_local_repo_git_only(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    ensure_remote_session_dir(&remote, &session);

    let output = std::process::Command::new(relocal_bin())
        .args(["status", &session])
        .current_dir(dir.path())
        .output()
        .expect("run relocal status in git-only repo");

    let stderr = String::from_utf8_lossy(&output.stderr);
    // The binary should succeed: discovery via .git, config from
    // ~/.relocal/config.toml (which the test runner is expected to have).
    assert!(
        output.status.success(),
        "relocal status should succeed in git-only repo: {stderr}"
    );
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn git_only_push_pull_round_trip() {
    let remote = test_remote().unwrap();
    let session = unique_session("git-only");
    let (dir, config) = make_local_repo_git_only(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();
    ensure_remote_session_dir(&remote, &session);

    std::fs::write(dir.path().join("hello.txt"), "from git-only").unwrap();
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    let content = read_remote_file(&remote, &format!("{}/hello.txt", remote_dir(&session)));
    assert_eq!(content.as_deref(), Some("from git-only"));

    write_remote_file(
        &remote,
        &format!("{}/hello.txt", remote_dir(&session)),
        "modified-remote",
    );
    sync::sync_pull(&runner, &config, &session, dir.path(), false).unwrap();
    let local = std::fs::read_to_string(dir.path().join("hello.txt")).unwrap();
    assert_eq!(local, "modified-remote");
}

// ---------------------------------------------------------------------------
// Sync pull tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn pull_files_appear_locally() {
    let remote = test_remote().unwrap();
    let session = unique_session("pull-appear");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();
    ensure_remote_session_dir(&remote, &session);

    // Push first to create remote dir
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    // Create a file on the remote
    write_remote_file(
        &remote,
        &format!("{}/remote-file.txt", remote_dir(&session)),
        "from remote",
    );

    sync::sync_pull(&runner, &config, &session, dir.path(), false).unwrap();

    let content = std::fs::read_to_string(dir.path().join("remote-file.txt")).unwrap();
    assert_eq!(content, "from remote");
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn pull_deletes_propagate() {
    let remote = test_remote().unwrap();
    let session = unique_session("pull-delete");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();
    ensure_remote_session_dir(&remote, &session);

    // Push two files
    std::fs::write(dir.path().join("keep.txt"), "keep").unwrap();
    std::fs::write(dir.path().join("remove.txt"), "remove").unwrap();
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    // Delete one on remote
    runner
        .run_ssh(&remote, &format!("rm {}/remove.txt", remote_dir(&session)))
        .unwrap();

    sync::sync_pull(&runner, &config, &session, dir.path(), false).unwrap();

    assert!(dir.path().join("keep.txt").exists());
    assert!(!dir.path().join("remove.txt").exists());
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn pull_keeps_gitignored_relocal_toml_across_repeated_pulls() {
    let remote = test_remote().unwrap();
    let session = unique_session("pull-keep-relocal-toml");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();
    ensure_remote_session_dir(&remote, &session);

    std::fs::write(dir.path().join(".gitignore"), "relocal.toml\n").unwrap();
    assert!(dir.path().join("relocal.toml").exists());

    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();
    assert!(!remote_file_exists(
        &remote,
        &format!("{}/relocal.toml", remote_dir(&session))
    ));

    sync::sync_pull(&runner, &config, &session, dir.path(), false).unwrap();
    assert!(
        dir.path().join("relocal.toml").exists(),
        "first pull must not delete local relocal.toml"
    );

    sync::sync_pull(&runner, &config, &session, dir.path(), false).unwrap();
    assert!(
        dir.path().join("relocal.toml").exists(),
        "second pull must also preserve local relocal.toml"
    );
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn pull_excludes_claude_dir() {
    let remote = test_remote().unwrap();
    let session = unique_session("pull-no-claude");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();
    ensure_remote_session_dir(&remote, &session);

    // Push to create remote dir
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    // Create .claude/ content on remote
    write_remote_file(
        &remote,
        &format!("{}/.claude/settings.json", remote_dir(&session)),
        "{\"hooks\":{}}",
    );

    sync::sync_pull(&runner, &config, &session, dir.path(), false).unwrap();

    // .claude/ content should NOT be pulled
    assert!(!dir.path().join(".claude/settings.json").exists());
}

// ---------------------------------------------------------------------------
// Session lifecycle tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn setup_creates_dir_and_pushes() {
    let remote = test_remote().unwrap();
    let session = unique_session("lifecycle-setup");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();

    std::fs::write(dir.path().join("data.txt"), "hello").unwrap();

    daemon::daemon_setup(&runner, &config, &session, dir.path(), false).unwrap();

    // Remote dir exists with pushed data
    assert!(remote_file_exists(
        &remote,
        &format!("{}/data.txt", remote_dir(&session))
    ));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn destroy_removes_dir() {
    let remote = test_remote().unwrap();
    let session = unique_session("lifecycle-destroy");
    let (dir, config) = make_local_repo(&remote);
    // No RemoteCleanup needed — destroy does the cleanup
    let runner = ProcessRunner::default();

    // Setup first
    daemon::daemon_setup(&runner, &config, &session, dir.path(), false).unwrap();

    // Destroy (no confirm in test)
    destroy::run(&runner, &config, &session, false, false).unwrap();

    assert!(!remote_file_exists(&remote, &remote_dir(&session)));
}

// ---------------------------------------------------------------------------
// Background sync loop tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn background_sync_pulls_remote_changes() {
    let remote = test_remote().unwrap();
    let session = unique_session("bg-sync-pull");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();

    // Setup: push initial state
    daemon::daemon_setup(&runner, &config, &session, dir.path(), false).unwrap();

    // Start background sync loop
    let sidecar_runner: Arc<dyn CommandRunner + Send + Sync> = Arc::new(ProcessRunner::default());
    let mut sidecar = Sidecar::start(
        sidecar_runner,
        config.clone(),
        session.clone(),
        dir.path().to_path_buf(),
        false,
    )
    .unwrap();

    // Create a file on the remote while the loop is running
    write_remote_file(
        &remote,
        &format!("{}/bg-test.txt", remote_dir(&session)),
        "from background",
    );

    // Wait for at least one poll cycle (interval is 3s, give it some margin)
    std::thread::sleep(std::time::Duration::from_secs(5));

    sidecar.shutdown();

    // The background loop should have pulled the file
    assert!(
        dir.path().join("bg-test.txt").exists(),
        "background sync did not pull remote file"
    );
    let content = std::fs::read_to_string(dir.path().join("bg-test.txt")).unwrap();
    assert_eq!(content, "from background");
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn background_sync_shutdown_is_prompt() {
    let remote = test_remote().unwrap();
    let session = unique_session("bg-sync-shutdown");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();

    daemon::daemon_setup(&runner, &config, &session, dir.path(), false).unwrap();

    let sidecar_runner: Arc<dyn CommandRunner + Send + Sync> = Arc::new(ProcessRunner::default());
    let mut sidecar = Sidecar::start(
        sidecar_runner,
        config.clone(),
        session.clone(),
        dir.path().to_path_buf(),
        false,
    )
    .unwrap();

    // Shutdown should complete quickly, not wait for the full poll interval
    let start = std::time::Instant::now();
    sidecar.shutdown();
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "shutdown took {:?}, expected < 2s",
        start.elapsed()
    );
}

// ---------------------------------------------------------------------------
// Daemon lifecycle tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn daemon_spawns_and_client_connects() {
    let remote = test_remote().unwrap();
    let session = unique_session("daemon-spawn");
    let (dir, _config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };

    std::fs::write(dir.path().join("data.txt"), "hello").unwrap();

    // connect_or_spawn should start a daemon and return a connection.
    let conn = daemon_client::connect_or_spawn_with_exe(
        &session,
        &remote,
        dir.path(),
        0,
        Some(relocal_bin().as_ref()),
    )
    .unwrap();

    // The connection should provide a valid ControlMaster path.
    assert!(
        conn.control_master_path()
            .to_str()
            .unwrap()
            .contains("rlc-"),
        "control master path should contain rlc- prefix"
    );

    // The daemon should have pushed our file to the remote.
    assert!(
        remote_file_exists(&remote, &format!("{}/data.txt", remote_dir(&session))),
        "daemon should have pushed initial state"
    );

    drop(conn);

    // Give daemon time to shut down and clean up.
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Daemon socket should be gone.
    assert!(
        !ssh::daemon_socket_path(&session, &remote).exists(),
        "daemon socket should be removed after last client disconnects"
    );
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn daemon_second_client_reuses_existing() {
    let remote = test_remote().unwrap();
    let session = unique_session("daemon-reuse");
    let (dir, _config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };

    std::fs::write(dir.path().join("data.txt"), "hello").unwrap();

    let conn1 = daemon_client::connect_or_spawn_with_exe(
        &session,
        &remote,
        dir.path(),
        0,
        Some(relocal_bin().as_ref()),
    )
    .unwrap();
    let control_path_1 = conn1.control_master_path().to_path_buf();

    // Second client should connect to the same daemon.
    let conn2 = daemon_client::connect_or_spawn_with_exe(
        &session,
        &remote,
        dir.path(),
        0,
        Some(relocal_bin().as_ref()),
    )
    .unwrap();
    let control_path_2 = conn2.control_master_path().to_path_buf();

    assert_eq!(
        control_path_1, control_path_2,
        "both clients should share the same ControlMaster"
    );

    // Drop first client — daemon should stay alive.
    drop(conn1);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        ssh::daemon_socket_path(&session, &remote).exists(),
        "daemon should stay alive while second client is connected"
    );

    // Drop second client — daemon should shut down.
    drop(conn2);
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert!(
        !ssh::daemon_socket_path(&session, &remote).exists(),
        "daemon should shut down after last client disconnects"
    );
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn daemon_does_final_pull_on_last_disconnect() {
    let remote = test_remote().unwrap();
    let session = unique_session("daemon-finalpull");
    let (dir, _config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };

    let conn = daemon_client::connect_or_spawn_with_exe(
        &session,
        &remote,
        dir.path(),
        0,
        Some(relocal_bin().as_ref()),
    )
    .unwrap();

    // Create a file on the remote while the daemon is running.
    write_remote_file(
        &remote,
        &format!("{}/remote-created.txt", remote_dir(&session)),
        "from remote",
    );

    // Wait for at least one background sync cycle.
    std::thread::sleep(std::time::Duration::from_secs(5));

    // The background sync should have pulled it.
    assert!(
        dir.path().join("remote-created.txt").exists(),
        "daemon background sync should have pulled remote file"
    );

    // Now create another file just before disconnecting.
    write_remote_file(
        &remote,
        &format!("{}/final-file.txt", remote_dir(&session)),
        "final",
    );

    // Disconnect — daemon should do a final pull before exiting.
    drop(conn);
    std::thread::sleep(std::time::Duration::from_secs(2));

    assert!(
        dir.path().join("final-file.txt").exists(),
        "daemon should have done a final pull on last client disconnect"
    );
}

// ---------------------------------------------------------------------------
// List / status / nuke tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn list_shows_sessions() {
    let remote = test_remote().unwrap();
    let session1 = unique_session("list-a");
    let session2 = unique_session("list-b");
    let _cleanup1 = RemoteCleanup {
        remote: remote.clone(),
        session: session1.clone(),
    };
    let _cleanup2 = RemoteCleanup {
        remote: remote.clone(),
        session: session2.clone(),
    };
    let runner = ProcessRunner::default();

    // Create sessions
    runner
        .run_ssh(&remote, &ssh::mkdir_work_dir(&session1))
        .unwrap();
    runner
        .run_ssh(&remote, &ssh::mkdir_work_dir(&session2))
        .unwrap();

    // List sessions via SSH — output format is "name\tsize" per line
    let output = runner.run_ssh(&remote, &ssh::list_sessions()).unwrap();
    let session_names: Vec<&str> = output
        .stdout
        .lines()
        .filter_map(|line| line.split('\t').next())
        .collect();

    assert!(session_names.contains(&session1.as_str()));
    assert!(session_names.contains(&session2.as_str()));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn status_reports_correct_info() {
    let remote = test_remote().unwrap();
    let session = unique_session("status-info");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();

    // Before setup: dir should not exist
    let check = runner
        .run_ssh(&remote, &ssh::check_work_dir_exists(&session))
        .unwrap();
    assert!(!check.status.success());

    // After setup: dir should exist
    daemon::daemon_setup(&runner, &config, &session, dir.path(), false).unwrap();

    let check = runner
        .run_ssh(&remote, &ssh::check_work_dir_exists(&session))
        .unwrap();
    assert!(check.status.success());
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn status_command_reports_missing_then_existing_directory() {
    let remote = test_remote().unwrap();
    let session = unique_session("status-probe");
    let (dir, _config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };

    let missing_output = std::process::Command::new(relocal_bin())
        .args(["status", &session])
        .current_dir(dir.path())
        .output()
        .expect("run relocal status before remote dir exists");
    assert!(
        missing_output.status.success(),
        "status before mkdir should succeed: stderr={}",
        String::from_utf8_lossy(&missing_output.stderr)
    );
    let missing_stderr = String::from_utf8_lossy(&missing_output.stderr);
    assert!(missing_stderr.contains(&format!("Session:    {session}")));
    assert!(missing_stderr.contains("Directory:  not found"));
    // Tool installation checks appear regardless of directory state
    assert!(missing_stderr.contains("Claude:"));
    assert!(missing_stderr.contains("Codex:"));

    ensure_remote_session_dir(&remote, &session);

    let existing_output = std::process::Command::new(relocal_bin())
        .args(["status", &session])
        .current_dir(dir.path())
        .output()
        .expect("run relocal status after remote dir exists");
    assert!(
        existing_output.status.success(),
        "status after mkdir should succeed: stderr={}",
        String::from_utf8_lossy(&existing_output.stderr)
    );
    let existing_stderr = String::from_utf8_lossy(&existing_output.stderr);
    assert!(existing_stderr.contains("Directory:  exists"));
    assert!(existing_stderr.contains("Claude:"));
    assert!(existing_stderr.contains("Codex:"));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn nuke_removes_everything() {
    let remote = test_remote().unwrap();
    let session = unique_session("nuke-test");
    let config = Config::parse(&format!("remote = \"{remote}\"")).unwrap();
    let runner = ProcessRunner::default();

    // Create some state
    runner
        .run_ssh(&remote, &ssh::mkdir_work_dir(&session))
        .unwrap();

    // Nuke (no confirm)
    nuke::run(&runner, &config, false).unwrap();

    assert!(!remote_file_exists(&remote, "~/relocal"));
}

// ---------------------------------------------------------------------------
// Localhost-as-remote test
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn localhost_push_pull_roundtrip() {
    let remote = test_remote().unwrap();
    let session = unique_session("localhost-rt");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner::default();

    // Create local files
    std::fs::write(dir.path().join("local.txt"), "local content").unwrap();
    std::fs::create_dir_all(dir.path().join("subdir")).unwrap();
    std::fs::write(dir.path().join("subdir/nested.txt"), "nested").unwrap();

    ensure_remote_session_dir(&remote, &session);

    // Push
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    // Verify on remote
    let content =
        read_remote_file(&remote, &format!("{}/local.txt", remote_dir(&session))).unwrap();
    assert_eq!(content, "local content");
    let content = read_remote_file(
        &remote,
        &format!("{}/subdir/nested.txt", remote_dir(&session)),
    )
    .unwrap();
    assert_eq!(content, "nested");

    // Modify on remote
    write_remote_file(
        &remote,
        &format!("{}/remote-new.txt", remote_dir(&session)),
        "from remote",
    );

    // Pull
    sync::sync_pull(&runner, &config, &session, dir.path(), false).unwrap();

    // Verify locally
    assert_eq!(
        std::fs::read_to_string(dir.path().join("remote-new.txt")).unwrap(),
        "from remote"
    );
    // Original files still present
    assert_eq!(
        std::fs::read_to_string(dir.path().join("local.txt")).unwrap(),
        "local content"
    );
}
