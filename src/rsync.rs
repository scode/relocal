//! rsync argument construction for push and pull syncs.
//!
//! This module builds the full argument list for rsync, including the complex
//! `.claude/` directory filtering. The functions are pure (no I/O) so they can
//! be thoroughly unit-tested. The caller passes the resulting `Vec<String>` to
//! [`CommandRunner::run_rsync`].

use std::path::Path;

use crate::config::Config;
use crate::ssh::remote_work_dir;

/// Sync direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Push,
    Pull,
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
) -> Vec<String> {
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

    args
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
        let args = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), false);
        assert!(args.contains(&"-az".to_string()));
        assert!(args.contains(&"--delete".to_string()));
    }

    #[test]
    fn gitignore_filter_included() {
        let args = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), false);
        assert!(args.contains(&"--filter=:- .gitignore".to_string()));
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
        let args = build_rsync_args(&config, Direction::Push, "s1", &root(), false);
        assert!(args.contains(&"--exclude=.env".to_string()));
        assert!(args.contains(&"--exclude=secrets/".to_string()));
    }

    #[test]
    fn claude_dir_excluded() {
        let args = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), false);
        assert!(args.contains(&"--exclude=.claude/".to_string()));
    }

    #[test]
    fn claude_dir_excluded_on_pull() {
        let args = build_rsync_args(&minimal_config(), Direction::Pull, "s1", &root(), false);
        assert!(args.contains(&"--exclude=.claude/".to_string()));
    }

    #[test]
    fn push_source_dest_paths() {
        let args = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), false);
        let last_two: Vec<&String> = args.iter().rev().take(2).collect();
        // dest is last, source is second-to-last
        assert_eq!(last_two[1], "/home/user/my-project/");
        assert_eq!(last_two[0], "user@host:~/relocal/s1/");
    }

    #[test]
    fn pull_source_dest_paths() {
        let args = build_rsync_args(&minimal_config(), Direction::Pull, "s1", &root(), false);
        let last_two: Vec<&String> = args.iter().rev().take(2).collect();
        assert_eq!(last_two[1], "user@host:~/relocal/s1/");
        assert_eq!(last_two[0], "/home/user/my-project/");
    }

    #[test]
    fn verbose_adds_progress() {
        let args = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), true);
        assert!(args.contains(&"--progress".to_string()));
    }

    #[test]
    fn non_verbose_no_progress() {
        let args = build_rsync_args(&minimal_config(), Direction::Push, "s1", &root(), false);
        assert!(!args.contains(&"--progress".to_string()));
    }
}
