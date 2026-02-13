//! Helper functions that construct remote shell command strings.
//!
//! These are pure string-building functions — they don't execute anything.
//! Orchestration code passes the returned strings to [`CommandRunner::run_ssh`]
//! or [`CommandRunner::run_ssh_interactive`].

use shell_quote::{Bash, QuoteRefExt};

/// Remote base directory for all relocal state.
const RELOCAL_DIR: &str = "~/relocal";

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

/// Returns the path to a session's request FIFO.
pub fn fifo_request_path(session: &str) -> String {
    format!("{RELOCAL_DIR}/.fifos/{session}-request")
}

/// Returns the path to a session's ack FIFO.
pub fn fifo_ack_path(session: &str) -> String {
    format!("{RELOCAL_DIR}/.fifos/{session}-ack")
}

/// Command to create both FIFOs for a session.
pub fn create_fifos(session: &str) -> String {
    format!(
        "mkfifo {} {}",
        fifo_request_path(session),
        fifo_ack_path(session)
    )
}

/// Command to check whether either FIFO exists (exit 0 = exists).
pub fn check_fifos_exist(session: &str) -> String {
    format!(
        "test -e {} -o -e {}",
        fifo_request_path(session),
        fifo_ack_path(session)
    )
}

/// Command to remove both FIFOs for a session.
pub fn remove_fifos(session: &str) -> String {
    format!(
        "rm -f {} {}",
        fifo_request_path(session),
        fifo_ack_path(session)
    )
}

/// Command to read from the request FIFO (blocks until a writer sends data).
/// Wrapped in a loop because each `cat` exits after one write/close cycle.
pub fn read_request_fifo(session: &str) -> String {
    format!("while true; do cat {}; done", fifo_request_path(session))
}

/// Command to write an ack message to the ack FIFO.
///
/// The message is shell-quoted to prevent injection via single quotes
/// or other metacharacters in error messages.
pub fn write_ack(session: &str, message: &str) -> String {
    let quoted: String = message.quoted(Bash);
    format!("echo {} > {}", quoted, fifo_ack_path(session))
}

/// Command to read the remote `.claude/settings.json` for a session.
pub fn read_settings_json(session: &str) -> String {
    format!("cat {}/.claude/settings.json", remote_work_dir(session))
}

/// Command to write content to the remote `.claude/settings.json`.
/// Uses a heredoc to handle arbitrary JSON content safely.
pub fn write_settings_json(session: &str, content: &str) -> String {
    format!(
        "mkdir -p {}/.claude && cat > {}/.claude/settings.json << 'RELOCAL_EOF'\n{}\nRELOCAL_EOF",
        remote_work_dir(session),
        remote_work_dir(session),
        content
    )
}

/// Command to create the FIFO directory.
pub fn mkdir_fifos_dir() -> String {
    format!("mkdir -p {RELOCAL_DIR}/.fifos")
}

/// Command to create the bin directory.
pub fn mkdir_bin_dir() -> String {
    format!("mkdir -p {RELOCAL_DIR}/.bin")
}

/// Path to the hook helper script on the remote.
pub fn hook_script_path() -> String {
    format!("{RELOCAL_DIR}/.bin/relocal-hook.sh")
}

/// Command to remove the entire relocal directory (nuke).
pub fn rm_relocal_dir() -> String {
    format!("rm -rf {RELOCAL_DIR}")
}

/// Command to list session directories with sizes (excludes `.bin/` and `.fifos/`).
///
/// Output format: `<name>\t<size>` per line, e.g. `my-session\t4.0K`.
pub fn list_sessions() -> String {
    format!(
        "cd {RELOCAL_DIR} 2>/dev/null && for d in $(ls -1 | grep -v '^\\.bin$' | grep -v '^\\.fifos$'); do size=$(du -sh \"$d\" 2>/dev/null | cut -f1); printf '%s\\t%s\\n' \"$d\" \"$size\"; done"
    )
}

/// Command to check whether the remote working directory exists.
pub fn check_work_dir_exists(session: &str) -> String {
    format!("test -d {}", remote_work_dir(session))
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

/// Command to launch an interactive Claude session in the working directory.
pub fn start_claude_session(session: &str) -> String {
    format!(
        "cd {} && claude --dangerously-skip-permissions",
        remote_work_dir(session)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn fifo_paths() {
        assert_eq!(fifo_request_path("s1"), "~/relocal/.fifos/s1-request");
        assert_eq!(fifo_ack_path("s1"), "~/relocal/.fifos/s1-ack");
    }

    #[test]
    fn create_fifos_format() {
        let cmd = create_fifos("s1");
        assert!(cmd.contains("mkfifo"));
        assert!(cmd.contains("s1-request"));
        assert!(cmd.contains("s1-ack"));
    }

    #[test]
    fn check_fifos_exist_format() {
        let cmd = check_fifos_exist("s1");
        assert!(cmd.contains("test -e"));
        assert!(cmd.contains("s1-request"));
        assert!(cmd.contains("s1-ack"));
    }

    #[test]
    fn remove_fifos_format() {
        let cmd = remove_fifos("s1");
        assert!(cmd.contains("rm -f"));
        assert!(cmd.contains("s1-request"));
        assert!(cmd.contains("s1-ack"));
    }

    #[test]
    fn read_request_fifo_loops() {
        let cmd = read_request_fifo("s1");
        assert!(cmd.contains("while true"));
        assert!(cmd.contains("cat"));
        assert!(cmd.contains("s1-request"));
    }

    #[test]
    fn write_ack_format() {
        let ack = write_ack("s1", "ok");
        assert!(ack.contains("ok"));
        assert!(ack.ends_with("~/relocal/.fifos/s1-ack"));

        let err_ack = write_ack("s1", "error:rsync failed");
        assert!(err_ack.contains("error:rsync failed"));
        assert!(err_ack.ends_with("~/relocal/.fifos/s1-ack"));
    }

    #[test]
    fn write_ack_escapes_single_quotes() {
        let ack = write_ack("s1", "error:it's broken");
        // Must not produce unbalanced quotes — shell_quote handles this
        assert!(ack.contains("it"));
        assert!(ack.contains("broken"));
        // The raw string "error:it's broken" with unescaped quote must NOT appear
        assert!(!ack.contains("echo 'error:it's broken'"));
    }

    #[test]
    fn read_settings_json_format() {
        let cmd = read_settings_json("s1");
        assert_eq!(cmd, "cat ~/relocal/s1/.claude/settings.json");
    }

    #[test]
    fn write_settings_json_creates_dir() {
        let cmd = write_settings_json("s1", "{\"hooks\":{}}");
        assert!(cmd.contains("mkdir -p ~/relocal/s1/.claude"));
        assert!(cmd.contains("{\"hooks\":{}}"));
        assert!(cmd.contains("RELOCAL_EOF"));
    }

    #[test]
    fn hook_script_path_format() {
        assert_eq!(hook_script_path(), "~/relocal/.bin/relocal-hook.sh");
    }

    #[test]
    fn list_sessions_excludes_dot_dirs() {
        let cmd = list_sessions();
        assert!(cmd.contains("grep -v"));
        assert!(cmd.contains(".bin"));
        assert!(cmd.contains(".fifos"));
        assert!(cmd.contains("du -sh"));
    }

    #[test]
    fn start_claude_session_format() {
        let cmd = start_claude_session("s1");
        assert!(cmd.contains("cd ~/relocal/s1"));
        assert!(cmd.contains("claude --dangerously-skip-permissions"));
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
}
