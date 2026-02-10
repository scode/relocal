//! Integration tests for relocal.
//!
//! These tests exercise real SSH, rsync, and FIFO operations against a
//! remote host (which may be localhost). They are gated on the
//! `RELOCAL_TEST_REMOTE` environment variable — when unset, all tests
//! are `#[ignore]`d.
//!
//! Run with: `RELOCAL_TEST_REMOTE=$USER@localhost cargo test -- --ignored`

use relocal::commands::{destroy, nuke, start, sync};
use relocal::config::Config;
use relocal::hooks;
use relocal::runner::{CommandRunner, ProcessRunner};
use relocal::ssh;

// ---------------------------------------------------------------------------
// Test infrastructure (step 13a)
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

/// RAII guard that cleans up remote state on drop (even on panic).
struct RemoteCleanup {
    remote: String,
    session: String,
}

impl Drop for RemoteCleanup {
    fn drop(&mut self) {
        let runner = ProcessRunner;
        // Best-effort cleanup
        let _ = runner.run_ssh(&self.remote, &ssh::rm_work_dir(&self.session));
        let _ = runner.run_ssh(&self.remote, &ssh::remove_fifos(&self.session));
    }
}

/// Reads a file from the remote via SSH.
fn read_remote_file(remote: &str, path: &str) -> Option<String> {
    let runner = ProcessRunner;
    let out = runner.run_ssh(remote, &format!("cat {path}")).ok()?;
    if out.status.success() {
        Some(out.stdout)
    } else {
        None
    }
}

/// Writes a file on the remote via SSH.
fn write_remote_file(remote: &str, path: &str, content: &str) {
    let runner = ProcessRunner;
    let cmd = format!("mkdir -p $(dirname {path}) && printf '%s' '{content}' > {path}");
    runner.run_ssh(remote, &cmd).expect("write remote file");
}

/// Checks if a remote file exists.
fn remote_file_exists(remote: &str, path: &str) -> bool {
    let runner = ProcessRunner;
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
/// without going through `start::setup`).
fn ensure_remote_session_dir(remote: &str, session: &str) {
    let runner = ProcessRunner;
    runner
        .run_ssh(remote, &ssh::mkdir_work_dir(session))
        .expect("create remote session dir");
}

// ---------------------------------------------------------------------------
// 13b. Sync push tests
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
    let runner = ProcessRunner;
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
    let runner = ProcessRunner;
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
    let runner = ProcessRunner;
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
    let runner = ProcessRunner;
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
fn push_syncs_claude_skills_but_not_conversations() {
    let remote = test_remote().unwrap();
    let session = unique_session("push-claude-dirs");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;
    ensure_remote_session_dir(&remote, &session);

    // Create .claude/skills/ (synced) and .claude/conversations/ (not synced)
    std::fs::create_dir_all(dir.path().join(".claude/skills")).unwrap();
    std::fs::write(dir.path().join(".claude/skills/my-skill.md"), "skill").unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/conversations")).unwrap();
    std::fs::write(dir.path().join(".claude/conversations/chat.json"), "chat").unwrap();

    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    assert!(remote_file_exists(
        &remote,
        &format!("{}/.claude/skills/my-skill.md", remote_dir(&session))
    ));
    assert!(!remote_file_exists(
        &remote,
        &format!("{}/.claude/conversations/chat.json", remote_dir(&session))
    ));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn push_syncs_settings_json_with_hooks() {
    let remote = test_remote().unwrap();
    let session = unique_session("push-settings");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;
    ensure_remote_session_dir(&remote, &session);

    std::fs::write(dir.path().join("file.txt"), "data").unwrap();
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    // settings.json should exist and contain hooks
    let settings = read_remote_file(
        &remote,
        &format!("{}/.claude/settings.json", remote_dir(&session)),
    );
    assert!(settings.is_some());
    let content = settings.unwrap();
    assert!(content.contains("relocal-hook.sh"));
    assert!(content.contains(&session));
}

// ---------------------------------------------------------------------------
// 13c. Sync pull tests
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
    let runner = ProcessRunner;
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
    let runner = ProcessRunner;
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
fn pull_excludes_settings_json() {
    let remote = test_remote().unwrap();
    let session = unique_session("pull-no-settings");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;
    ensure_remote_session_dir(&remote, &session);

    // Push to create remote dir + hooks
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    // Remove local settings.json if it exists
    let local_settings = dir.path().join(".claude/settings.json");
    if local_settings.exists() {
        std::fs::remove_file(&local_settings).unwrap();
    }

    // Pull should NOT bring back settings.json
    sync::sync_pull(&runner, &config, &session, dir.path(), false).unwrap();

    assert!(!local_settings.exists());
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn pull_syncs_claude_skills() {
    let remote = test_remote().unwrap();
    let session = unique_session("pull-skills");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;
    ensure_remote_session_dir(&remote, &session);

    // Push to create remote dir
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    // Create a skill on remote
    write_remote_file(
        &remote,
        &format!("{}/.claude/skills/remote-skill.md", remote_dir(&session)),
        "remote skill content",
    );

    sync::sync_pull(&runner, &config, &session, dir.path(), false).unwrap();

    let content =
        std::fs::read_to_string(dir.path().join(".claude/skills/remote-skill.md")).unwrap();
    assert_eq!(content, "remote skill content");
}

// ---------------------------------------------------------------------------
// 13d. Hook injection tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn push_reinjects_hooks_after_overwrite() {
    let remote = test_remote().unwrap();
    let session = unique_session("hook-reinject");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;
    ensure_remote_session_dir(&remote, &session);

    // First push installs hooks
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    // Create a local settings.json that overwrites hooks
    std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
    std::fs::write(
        dir.path().join(".claude/settings.json"),
        "{\"allowedTools\": [\"bash\"]}",
    )
    .unwrap();

    // Push again — should overwrite then re-inject
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    let settings = read_remote_file(
        &remote,
        &format!("{}/.claude/settings.json", remote_dir(&session)),
    )
    .unwrap();

    // Hooks present
    assert!(settings.contains("relocal-hook.sh"));
    assert!(settings.contains(&session));
    // Original keys preserved
    assert!(settings.contains("allowedTools"));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn hooks_reference_correct_session_name() {
    let remote = test_remote().unwrap();
    let session = unique_session("hook-session-name");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;
    ensure_remote_session_dir(&remote, &session);

    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    let settings = read_remote_file(
        &remote,
        &format!("{}/.claude/settings.json", remote_dir(&session)),
    )
    .unwrap();

    assert!(settings.contains(&format!("RELOCAL_SESSION={session}")));
}

// ---------------------------------------------------------------------------
// 13e. FIFO lifecycle tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn fifos_created_by_setup() {
    let remote = test_remote().unwrap();
    let session = unique_session("fifo-create");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;

    start::setup(&runner, &config, &session, dir.path(), false).unwrap();

    assert!(remote_file_exists(
        &remote,
        &ssh::fifo_request_path(&session)
    ));
    assert!(remote_file_exists(&remote, &ssh::fifo_ack_path(&session)));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn fifos_removed_by_cleanup() {
    let remote = test_remote().unwrap();
    let session = unique_session("fifo-cleanup");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;

    start::setup(&runner, &config, &session, dir.path(), false).unwrap();
    start::cleanup(&runner, &config, &session).unwrap();

    assert!(!remote_file_exists(
        &remote,
        &ssh::fifo_request_path(&session)
    ));
    assert!(!remote_file_exists(&remote, &ssh::fifo_ack_path(&session)));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn stale_fifos_prevent_setup() {
    let remote = test_remote().unwrap();
    let session = unique_session("fifo-stale");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;

    // Pre-create FIFOs
    runner.run_ssh(&remote, &ssh::mkdir_fifos_dir()).unwrap();
    runner
        .run_ssh(&remote, &ssh::create_fifos(&session))
        .unwrap();

    // Setup should fail with stale session error
    let result = start::setup(&runner, &config, &session, dir.path(), false);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("stale session") || err.contains("FIFOs already exist"));
}

// ---------------------------------------------------------------------------
// 13f. Sidecar tests (using real SSH + FIFO)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn sidecar_push_request_syncs_and_acks() {
    let remote = test_remote().unwrap();
    let session = unique_session("sidecar-push");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;

    // Setup creates remote dir + FIFOs + initial push
    start::setup(&runner, &config, &session, dir.path(), false).unwrap();

    // Start sidecar
    let sidecar_runner: std::sync::Arc<dyn CommandRunner + Send + Sync> =
        std::sync::Arc::new(ProcessRunner);
    let mut sidecar = relocal::sidecar::Sidecar::start(
        sidecar_runner,
        config.clone(),
        session.clone(),
        dir.path().to_path_buf(),
        false,
    )
    .unwrap();

    // Create a new local file
    std::fs::write(dir.path().join("sidecar-test.txt"), "sidecar").unwrap();

    // Write "push" to the request FIFO
    runner
        .run_ssh(
            &remote,
            &format!("echo push > {}", ssh::fifo_request_path(&session)),
        )
        .unwrap();

    // Read ack from the ack FIFO
    let ack = runner
        .run_ssh(&remote, &format!("cat {}", ssh::fifo_ack_path(&session)))
        .unwrap();

    assert_eq!(ack.stdout.trim(), "ok");

    // Verify file was synced
    assert!(remote_file_exists(
        &remote,
        &format!("{}/sidecar-test.txt", remote_dir(&session))
    ));

    sidecar.shutdown();
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn sidecar_pull_request_syncs_and_acks() {
    let remote = test_remote().unwrap();
    let session = unique_session("sidecar-pull");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;

    start::setup(&runner, &config, &session, dir.path(), false).unwrap();

    let sidecar_runner: std::sync::Arc<dyn CommandRunner + Send + Sync> =
        std::sync::Arc::new(ProcessRunner);
    let mut sidecar = relocal::sidecar::Sidecar::start(
        sidecar_runner,
        config.clone(),
        session.clone(),
        dir.path().to_path_buf(),
        false,
    )
    .unwrap();

    // Create a file on the remote
    write_remote_file(
        &remote,
        &format!("{}/remote-only.txt", remote_dir(&session)),
        "remote data",
    );

    // Write "pull" to the request FIFO
    runner
        .run_ssh(
            &remote,
            &format!("echo pull > {}", ssh::fifo_request_path(&session)),
        )
        .unwrap();

    // Read ack
    let ack = runner
        .run_ssh(&remote, &format!("cat {}", ssh::fifo_ack_path(&session)))
        .unwrap();

    assert_eq!(ack.stdout.trim(), "ok");

    // Verify file was pulled locally
    let content = std::fs::read_to_string(dir.path().join("remote-only.txt")).unwrap();
    assert_eq!(content, "remote data");

    sidecar.shutdown();
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn sidecar_clean_shutdown() {
    let remote = test_remote().unwrap();
    let session = unique_session("sidecar-shutdown");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;

    start::setup(&runner, &config, &session, dir.path(), false).unwrap();

    let sidecar_runner: std::sync::Arc<dyn CommandRunner + Send + Sync> =
        std::sync::Arc::new(ProcessRunner);
    let mut sidecar = relocal::sidecar::Sidecar::start(
        sidecar_runner,
        config.clone(),
        session.clone(),
        dir.path().to_path_buf(),
        false,
    )
    .unwrap();

    // Sidecar should shut down cleanly without hanging
    sidecar.shutdown();
}

// ---------------------------------------------------------------------------
// 13g. Hook script end-to-end tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn hook_script_ok_ack_exits_zero() {
    let remote = test_remote().unwrap();
    let session = unique_session("hook-ok");
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;

    // Create FIFOs and install hook script
    runner.run_ssh(&remote, &ssh::mkdir_fifos_dir()).unwrap();
    runner.run_ssh(&remote, &ssh::mkdir_bin_dir()).unwrap();
    runner
        .run_ssh(&remote, &ssh::create_fifos(&session))
        .unwrap();

    let script = hooks::hook_script_content();
    let write_cmd = format!(
        "cat > {} << 'RELOCAL_HOOK_EOF'\n{}\nRELOCAL_HOOK_EOF\nchmod +x {}",
        ssh::hook_script_path(),
        script,
        ssh::hook_script_path()
    );
    runner.run_ssh(&remote, &write_cmd).unwrap();

    // In a background process, write "ok" to the ack FIFO after a short delay
    // (simulating the sidecar). Meanwhile the hook script blocks on ack.
    let ack_cmd = format!("sleep 1 && echo ok > {}", ssh::fifo_ack_path(&session));
    runner
        .run_ssh(&remote, &format!("nohup bash -c '{ack_cmd}' &>/dev/null &"))
        .unwrap();

    // Run the hook script — it writes to request FIFO and reads from ack FIFO
    let result = runner.run_ssh(
        &remote,
        &format!("RELOCAL_SESSION={session} {} push", ssh::hook_script_path()),
    );

    // Drain the request FIFO (the hook wrote to it)
    let _ = runner.run_ssh(
        &remote,
        &format!("cat {} || true", ssh::fifo_request_path(&session)),
    );

    assert!(result.unwrap().status.success());
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn hook_script_error_ack_exits_nonzero() {
    let remote = test_remote().unwrap();
    let session = unique_session("hook-err");
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;

    runner.run_ssh(&remote, &ssh::mkdir_fifos_dir()).unwrap();
    runner.run_ssh(&remote, &ssh::mkdir_bin_dir()).unwrap();
    runner
        .run_ssh(&remote, &ssh::create_fifos(&session))
        .unwrap();

    let script = hooks::hook_script_content();
    let write_cmd = format!(
        "cat > {} << 'RELOCAL_HOOK_EOF'\n{}\nRELOCAL_HOOK_EOF\nchmod +x {}",
        ssh::hook_script_path(),
        script,
        ssh::hook_script_path()
    );
    runner.run_ssh(&remote, &write_cmd).unwrap();

    // Write error ack
    let ack_cmd = format!(
        "sleep 1 && echo 'error:sync failed' > {}",
        ssh::fifo_ack_path(&session)
    );
    runner
        .run_ssh(&remote, &format!("nohup bash -c '{ack_cmd}' &>/dev/null &"))
        .unwrap();

    let result = runner.run_ssh(
        &remote,
        &format!("RELOCAL_SESSION={session} {} pull", ssh::hook_script_path()),
    );

    let _ = runner.run_ssh(
        &remote,
        &format!("cat {} || true", ssh::fifo_request_path(&session)),
    );

    let output = result.unwrap();
    assert!(!output.status.success());
    assert!(output.stderr.contains("sync failed"));
}

// ---------------------------------------------------------------------------
// 13h. Session lifecycle tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn setup_creates_dir_fifos_pushes_hooks() {
    let remote = test_remote().unwrap();
    let session = unique_session("lifecycle-setup");
    let (dir, config) = make_local_repo(&remote);
    let _cleanup = RemoteCleanup {
        remote: remote.clone(),
        session: session.clone(),
    };
    let runner = ProcessRunner;

    std::fs::write(dir.path().join("data.txt"), "hello").unwrap();

    start::setup(&runner, &config, &session, dir.path(), false).unwrap();

    // Remote dir exists
    assert!(remote_file_exists(
        &remote,
        &format!("{}/data.txt", remote_dir(&session))
    ));

    // FIFOs exist
    assert!(remote_file_exists(
        &remote,
        &ssh::fifo_request_path(&session)
    ));

    // Hooks installed
    let settings = read_remote_file(
        &remote,
        &format!("{}/.claude/settings.json", remote_dir(&session)),
    );
    assert!(settings.is_some());
    assert!(settings.unwrap().contains("relocal-hook.sh"));
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn destroy_removes_dir_and_fifos() {
    let remote = test_remote().unwrap();
    let session = unique_session("lifecycle-destroy");
    let (dir, config) = make_local_repo(&remote);
    // No RemoteCleanup needed — destroy does the cleanup
    let runner = ProcessRunner;

    // Setup first
    start::setup(&runner, &config, &session, dir.path(), false).unwrap();
    start::cleanup(&runner, &config, &session).unwrap();

    // Now push some data so we have a working dir
    sync::sync_push(&runner, &config, &session, dir.path(), false).unwrap();

    // Destroy (no confirm in test)
    destroy::run(&runner, &config, &session, false).unwrap();

    assert!(!remote_file_exists(&remote, &remote_dir(&session)));
    assert!(!remote_file_exists(
        &remote,
        &ssh::fifo_request_path(&session)
    ));
}

// ---------------------------------------------------------------------------
// 13i. Remote install tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn install_creates_hook_script_and_fifos_dir() {
    let remote = test_remote().unwrap();
    let runner = ProcessRunner;

    // Run install (only hook script + fifos dir steps)
    // We test the hook script and fifos dir steps specifically
    runner.run_ssh(&remote, &ssh::mkdir_bin_dir()).unwrap();
    runner.run_ssh(&remote, &ssh::mkdir_fifos_dir()).unwrap();

    let script = hooks::hook_script_content();
    let write_cmd = format!(
        "cat > {} << 'RELOCAL_HOOK_EOF'\n{}\nRELOCAL_HOOK_EOF\nchmod +x {}",
        ssh::hook_script_path(),
        script,
        ssh::hook_script_path()
    );
    runner.run_ssh(&remote, &write_cmd).unwrap();

    assert!(remote_file_exists(&remote, &ssh::hook_script_path()));
    assert!(remote_file_exists(&remote, "~/relocal/.fifos"));

    // Idempotent: run again
    runner.run_ssh(&remote, &write_cmd).unwrap();
    assert!(remote_file_exists(&remote, &ssh::hook_script_path()));
}

// ---------------------------------------------------------------------------
// 13j. List / status / nuke tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn list_shows_sessions_and_excludes_dot_dirs() {
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
    let runner = ProcessRunner;

    // Create sessions
    runner
        .run_ssh(&remote, &ssh::mkdir_work_dir(&session1))
        .unwrap();
    runner
        .run_ssh(&remote, &ssh::mkdir_work_dir(&session2))
        .unwrap();
    // Ensure .bin and .fifos exist
    runner.run_ssh(&remote, &ssh::mkdir_bin_dir()).unwrap();
    runner.run_ssh(&remote, &ssh::mkdir_fifos_dir()).unwrap();

    // List sessions via SSH — output format is "name\tsize" per line
    let output = runner.run_ssh(&remote, &ssh::list_sessions()).unwrap();
    let session_names: Vec<&str> = output
        .stdout
        .lines()
        .filter_map(|line| line.split('\t').next())
        .collect();

    assert!(session_names.contains(&session1.as_str()));
    assert!(session_names.contains(&session2.as_str()));
    assert!(!session_names.contains(&".bin"));
    assert!(!session_names.contains(&".fifos"));
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
    let runner = ProcessRunner;

    // Before setup: dir should not exist
    let check = runner
        .run_ssh(&remote, &ssh::check_work_dir_exists(&session))
        .unwrap();
    assert!(!check.status.success());

    // After setup: dir and FIFOs should exist
    start::setup(&runner, &config, &session, dir.path(), false).unwrap();

    let check = runner
        .run_ssh(&remote, &ssh::check_work_dir_exists(&session))
        .unwrap();
    assert!(check.status.success());

    let check = runner
        .run_ssh(&remote, &ssh::check_fifos_exist(&session))
        .unwrap();
    assert!(check.status.success());
}

#[test]
#[ignore = "requires RELOCAL_TEST_REMOTE"]
fn nuke_removes_everything() {
    let remote = test_remote().unwrap();
    let session = unique_session("nuke-test");
    let config = Config::parse(&format!("remote = \"{remote}\"")).unwrap();
    let runner = ProcessRunner;

    // Create some state
    runner
        .run_ssh(&remote, &ssh::mkdir_work_dir(&session))
        .unwrap();
    runner.run_ssh(&remote, &ssh::mkdir_bin_dir()).unwrap();
    runner.run_ssh(&remote, &ssh::mkdir_fifos_dir()).unwrap();

    // Nuke (no confirm)
    nuke::run(&runner, &config, false).unwrap();

    assert!(!remote_file_exists(&remote, "~/relocal"));

    // Re-create relocal dir for other tests that may be running
    // (nuke is destructive to all sessions)
}

// ---------------------------------------------------------------------------
// 13k. Localhost-as-remote test
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
    let runner = ProcessRunner;

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
