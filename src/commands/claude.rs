//! `relocal claude [session-name]` — main orchestration command.
//!
//! Syncs the repo to the remote, launches an interactive Claude session with
//! continuous background synchronization, and performs a final pull on clean exit.

use std::path::Path;

use crate::commands::session::ToolConfig;
use crate::config::Config;
use crate::error::Result;
use crate::ssh;

const TOOL: ToolConfig = ToolConfig {
    display_name: "Claude Code",
    check_installed: ssh::check_claude_installed,
    start_session: ssh::start_claude_session,
};

pub fn run(
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbose: bool,
    claude_args: &[String],
) -> Result<()> {
    super::session::run(&TOOL, config, session_name, repo_root, verbose, claude_args)
}
