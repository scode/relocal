//! rsync argument construction for push and pull syncs.
//!
//! This module builds the full argument list for rsync, including the complex
//! `.claude/` directory filtering. The functions are pure (no I/O) so they can
//! be thoroughly unit-tested. The caller passes the resulting [`RsyncParams`] to
//! [`CommandRunner::run_rsync`].

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::ssh::remote_work_dir;

/// Sync direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Push,
    Pull,
}

/// Structured rsync invocation carrying both the argument list and metadata
/// needed for safety validation before execution.
///
/// Fields are private to enforce that `args`, `direction`, and `local_path` are
/// always consistent — they must all originate from the same
/// [`build_rsync_args`] call. [`ProcessRunner::run_rsync`](crate::runner::ProcessRunner)
/// uses `direction` and `local_path` to validate the local destination on pull,
/// refusing to run `rsync --delete` against a directory that doesn't contain
/// `relocal.toml`.
#[derive(Debug)]
pub struct RsyncParams {
    args: Vec<String>,
    direction: Direction,
    local_path: PathBuf,
}

impl RsyncParams {
    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn direction(&self) -> Direction {
        self.direction
    }

    pub fn local_path(&self) -> &Path {
        &self.local_path
    }

    /// Test-only constructor for unit tests that need to exercise
    /// [`CommandRunner::run_rsync`](crate::runner::CommandRunner) directly.
    #[cfg(test)]
    pub fn for_test(args: Vec<String>, direction: Direction, local_path: PathBuf) -> Self {
        Self {
            args,
            direction,
            local_path,
        }
    }
}

/// Builds the complete rsync argument list for a sync operation.
///
/// The `.claude/` directory is excluded entirely — hook configuration is managed
/// separately via SSH (see [`reinject_hooks`](crate::commands::sync::reinject_hooks)).
pub fn build_rsync_args(
    config: &Config,
    direction: Direction,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
) -> RsyncParams {
    let mut args = Vec::new();

    // Base flags
    args.push("-az".to_string());
    args.push("--delete".to_string());

    // Respect .gitignore at every directory level
    args.push("--filter=:- .gitignore".to_string());

    // User-configured exclusions
    for pattern in &config.exclude {
        args.push(format!("--exclude={pattern}"));
    }

    // Exclude .claude/ entirely — hooks are managed via SSH, not rsync.
    args.push("--exclude=.claude/".to_string());

    // Verbose mode adds progress
    if verbose {
        args.push("--progress".to_string());
    }

    // Source and destination (trailing slash ensures contents are synced)
    let local_path = format!("{}/", repo_root.display());
    let remote_path = format!("{}:{}/", config.remote, remote_work_dir(session_name));

    match direction {
        Direction::Push => {
            args.push(local_path);
            args.push(remote_path);
        }
        Direction::Pull => {
            args.push(remote_path);
            args.push(local_path);
        }
    }

    RsyncParams {
        args,
        direction,
        local_path: repo_root.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn minimal_config() -> Config {
        Config::parse("remote = \"user@host\"").unwrap()
    }

    fn root() -> PathBuf {
        PathBuf::from("/home/user/my-project")
    }

    #[test]
    fn base_flags_present() {
        let params = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), false);
        assert!(params.args().contains(&"-az".to_string()));
        assert!(params.args().contains(&"--delete".to_string()));
    }

    #[test]
    fn gitignore_filter_included() {
        let params = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), false);
        assert!(params
            .args()
            .contains(&"--filter=:- .gitignore".to_string()));
    }

    #[test]
    fn custom_excludes() {
        let config = Config::parse(
            r#"
remote = "user@host"
exclude = [".env", "secrets/"]
"#,
        )
        .unwrap();
        let params = build_rsync_args(&config, Direction::Push, "s1", &root(), false);
        assert!(params.args().contains(&"--exclude=.env".to_string()));
        assert!(params.args().contains(&"--exclude=secrets/".to_string()));
    }

    #[test]
    fn claude_dir_excluded() {
        let params = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), false);
        assert!(params.args().contains(&"--exclude=.claude/".to_string()));
    }

    #[test]
    fn claude_dir_excluded_on_pull() {
        let params = build_rsync_args(&minimal_config(), Direction::Pull, "s1", &root(), false);
        assert!(params.args().contains(&"--exclude=.claude/".to_string()));
    }

    #[test]
    fn push_source_dest_paths() {
        let params = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), false);
        let last_two: Vec<&String> = params.args().iter().rev().take(2).collect();
        assert_eq!(last_two[1], "/home/user/my-project/");
        assert_eq!(last_two[0], "user@host:~/relocal/s1/");
    }

    #[test]
    fn pull_source_dest_paths() {
        let params = build_rsync_args(&minimal_config(), Direction::Pull, "s1", &root(), false);
        let last_two: Vec<&String> = params.args().iter().rev().take(2).collect();
        assert_eq!(last_two[1], "user@host:~/relocal/s1/");
        assert_eq!(last_two[0], "/home/user/my-project/");
    }

    #[test]
    fn verbose_adds_progress() {
        let params = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), true);
        assert!(params.args().contains(&"--progress".to_string()));
    }

    #[test]
    fn non_verbose_no_progress() {
        let params = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), false);
        assert!(!params.args().contains(&"--progress".to_string()));
    }

    #[test]
    fn params_carry_direction_and_local_path() {
        let push = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), false);
        assert_eq!(push.direction(), Direction::Push);
        assert_eq!(push.local_path(), root());

        let pull = build_rsync_args(&minimal_config(), Direction::Pull, "s1", &root(), false);
        assert_eq!(pull.direction(), Direction::Pull);
        assert_eq!(pull.local_path(), root());
    }
}
