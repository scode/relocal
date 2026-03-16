//! `relocal codex [session-name]` — runs Codex on the remote.
//!
//! Identical to `relocal claude` except it launches Codex instead of Claude Code.
//! Syncs the repo to the remote, launches an interactive Codex session with
//! continuous background synchronization, and performs a final pull on clean exit.

use std::path::Path;

use crate::commands::session::ToolConfig;
use crate::config::Config;
use crate::error::Result;
use crate::ssh;

const TOOL: ToolConfig = ToolConfig {
    display_name: "Codex",
    check_installed: ssh::check_codex_installed,
    start_session: ssh::start_codex_session,
};

pub fn run(
    config: &Config,
    session_name: &str,
    repo_root: &Path,
    verbosity: u8,
    codex_args: &[String],
) -> Result<()> {
    super::session::run(
        &TOOL,
        config,
        session_name,
        repo_root,
        verbosity,
        codex_args,
    )
}
