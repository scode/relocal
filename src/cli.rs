//! CLI argument definitions using clap's derive API.
//!
//! This module defines the full command tree. Every subcommand and flag is
//! declared here; the rest of the codebase receives already-parsed structs.

use clap::{Parser, Subcommand};

/// Run Claude Code remotely, work locally.
#[derive(Debug, Parser)]
#[command(name = "relocal", version)]
pub struct Cli {
    /// Increase log verbosity (-v = INFO, -vv = DEBUG, -vvv = TRACE).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Maps the verbosity count to a tracing filter level.
    pub fn log_level(&self) -> tracing::Level {
        match self.verbose {
            0 => tracing::Level::WARN,
            1 => tracing::Level::INFO,
            2 => tracing::Level::DEBUG,
            _ => tracing::Level::TRACE,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a relocal.toml in the current directory.
    Init,

    /// Manage the remote environment.
    Remote {
        #[command(subcommand)]
        command: RemoteCommand,
    },

    /// Sync and launch an interactive Claude session on the remote.
    Claude {
        /// Session name (defaults to directory name).
        session_name: Option<String>,

        /// Extra arguments passed through to `claude` (after `--`).
        #[arg(last = true)]
        claude_args: Vec<String>,
    },

    /// Manually sync files between local and remote.
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },

    /// Show session status.
    Status {
        /// Session name (defaults to directory name).
        session_name: Option<String>,
    },

    /// List all sessions on the remote.
    List,

    /// Remove a session's remote working copy and FIFOs.
    Destroy {
        /// Session name (defaults to directory name).
        session_name: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Install the full environment on the remote host.
    Install,
    /// Delete everything under ~/relocal/ on the remote.
    Nuke,
}

#[derive(Debug, Subcommand)]
pub enum SyncCommand {
    /// Push local files to the remote.
    Push {
        /// Session name (defaults to directory name).
        session_name: Option<String>,
    },
    /// Pull remote files to local.
    Pull {
        /// Session name (defaults to directory name).
        session_name: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::parse_from(args)
    }

    #[test]
    fn init() {
        let cli = parse(&["relocal", "init"]);
        assert!(matches!(cli.command, Command::Init));
    }

    #[test]
    fn remote_install() {
        let cli = parse(&["relocal", "remote", "install"]);
        assert!(matches!(
            cli.command,
            Command::Remote {
                command: RemoteCommand::Install
            }
        ));
    }

    #[test]
    fn remote_nuke() {
        let cli = parse(&["relocal", "remote", "nuke"]);
        assert!(matches!(
            cli.command,
            Command::Remote {
                command: RemoteCommand::Nuke
            }
        ));
    }

    #[test]
    fn claude_no_session() {
        let cli = parse(&["relocal", "claude"]);
        match &cli.command {
            Command::Claude {
                session_name,
                claude_args,
            } => {
                assert!(session_name.is_none());
                assert!(claude_args.is_empty());
            }
            _ => panic!("expected Claude"),
        }
    }

    #[test]
    fn claude_with_session() {
        let cli = parse(&["relocal", "claude", "my-session"]);
        match &cli.command {
            Command::Claude {
                session_name,
                claude_args,
            } => {
                assert_eq!(session_name.as_deref(), Some("my-session"));
                assert!(claude_args.is_empty());
            }
            _ => panic!("expected Claude"),
        }
    }

    #[test]
    fn claude_with_extra_args() {
        let cli = parse(&["relocal", "claude", "--", "--debug"]);
        match &cli.command {
            Command::Claude {
                session_name,
                claude_args,
            } => {
                assert!(session_name.is_none());
                assert_eq!(claude_args, &["--debug"]);
            }
            _ => panic!("expected Claude"),
        }
    }

    #[test]
    fn claude_with_session_and_extra_args() {
        let cli = parse(&[
            "relocal",
            "claude",
            "my-session",
            "--",
            "--debug",
            "--resume",
        ]);
        match &cli.command {
            Command::Claude {
                session_name,
                claude_args,
            } => {
                assert_eq!(session_name.as_deref(), Some("my-session"));
                assert_eq!(claude_args, &["--debug", "--resume"]);
            }
            _ => panic!("expected Claude"),
        }
    }

    #[test]
    fn sync_push_no_session() {
        let cli = parse(&["relocal", "sync", "push"]);
        assert!(matches!(
            cli.command,
            Command::Sync {
                command: SyncCommand::Push { session_name: None }
            }
        ));
    }

    #[test]
    fn sync_push_with_session() {
        let cli = parse(&["relocal", "sync", "push", "s1"]);
        match &cli.command {
            Command::Sync {
                command: SyncCommand::Push { session_name },
            } => assert_eq!(session_name.as_deref(), Some("s1")),
            _ => panic!("expected Sync Push"),
        }
    }

    #[test]
    fn sync_pull() {
        let cli = parse(&["relocal", "sync", "pull"]);
        assert!(matches!(
            cli.command,
            Command::Sync {
                command: SyncCommand::Pull { session_name: None }
            }
        ));
    }

    #[test]
    fn status_no_session() {
        let cli = parse(&["relocal", "status"]);
        assert!(matches!(
            cli.command,
            Command::Status { session_name: None }
        ));
    }

    #[test]
    fn status_with_session() {
        let cli = parse(&["relocal", "status", "s1"]);
        match &cli.command {
            Command::Status { session_name } => {
                assert_eq!(session_name.as_deref(), Some("s1"));
            }
            _ => panic!("expected Status"),
        }
    }

    #[test]
    fn list() {
        let cli = parse(&["relocal", "list"]);
        assert!(matches!(cli.command, Command::List));
    }

    #[test]
    fn destroy_no_session() {
        let cli = parse(&["relocal", "destroy"]);
        assert!(matches!(
            cli.command,
            Command::Destroy { session_name: None }
        ));
    }

    #[test]
    fn destroy_with_session() {
        let cli = parse(&["relocal", "destroy", "s1"]);
        match &cli.command {
            Command::Destroy { session_name } => {
                assert_eq!(session_name.as_deref(), Some("s1"));
            }
            _ => panic!("expected Destroy"),
        }
    }

    #[test]
    fn verbosity_default_warn() {
        let cli = parse(&["relocal", "init"]);
        assert_eq!(cli.verbose, 0);
        assert_eq!(cli.log_level(), tracing::Level::WARN);
    }

    #[test]
    fn verbosity_v_info() {
        let cli = parse(&["relocal", "-v", "init"]);
        assert_eq!(cli.verbose, 1);
        assert_eq!(cli.log_level(), tracing::Level::INFO);
    }

    #[test]
    fn verbosity_vv_debug() {
        let cli = parse(&["relocal", "-vv", "init"]);
        assert_eq!(cli.verbose, 2);
        assert_eq!(cli.log_level(), tracing::Level::DEBUG);
    }

    #[test]
    fn verbosity_vvv_trace() {
        let cli = parse(&["relocal", "-vvv", "init"]);
        assert_eq!(cli.verbose, 3);
        assert_eq!(cli.log_level(), tracing::Level::TRACE);
    }

    #[test]
    fn verbosity_after_subcommand() {
        let cli = parse(&["relocal", "claude", "-vv"]);
        assert_eq!(cli.verbose, 2);
    }
}
