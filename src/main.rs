mod cli;
mod commands;
mod config;
mod discovery;
mod error;
mod hooks;
mod rsync;
mod runner;
mod session;
mod ssh;
#[cfg(test)]
mod test_support;

use std::path::{Path, PathBuf};

use clap::Parser;
use cli::{Cli, Command, RemoteCommand, SyncCommand};
use tracing_subscriber::FmtSubscriber;

/// Finds the repo root and loads `relocal.toml`. Exits on failure.
fn load_config() -> (PathBuf, config::Config) {
    let root = discovery::find_repo_root(&std::env::current_dir().unwrap()).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    });
    let toml_str = std::fs::read_to_string(root.join("relocal.toml")).unwrap_or_else(|e| {
        eprintln!("Error reading relocal.toml: {e}");
        std::process::exit(1);
    });
    let cfg = config::Config::parse(&toml_str).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    });
    (root, cfg)
}

/// Resolves the session name: uses the explicit name if given, otherwise
/// derives it from the repo root directory name.
fn resolve_session(name: Option<String>, repo_root: &Path) -> String {
    match name {
        Some(n) => {
            session::validate_session_name(&n).unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                std::process::exit(1);
            });
            n
        }
        None => session::default_session_name(repo_root).unwrap_or_else(|e| {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }),
    }
}

fn main() {
    let cli = Cli::parse();

    FmtSubscriber::builder()
        .with_max_level(cli.log_level())
        .init();

    let verbose = cli.verbose > 0;

    match cli.command {
        Command::Init => {
            if let Err(e) = commands::init::run(&std::env::current_dir().unwrap()) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Command::Remote { command } => match command {
            RemoteCommand::Install => {
                let (_root, cfg) = load_config();
                let runner = runner::ProcessRunner;
                if let Err(e) = commands::install::run(&runner, &cfg) {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
            RemoteCommand::Nuke => eprintln!("not yet implemented: remote nuke"),
        },
        Command::Start { .. } => eprintln!("not yet implemented: start"),
        Command::Sync { command } => {
            let (root, cfg) = load_config();
            let runner = runner::ProcessRunner;
            match command {
                SyncCommand::Push { session_name } => {
                    let session = resolve_session(session_name, &root);
                    if let Err(e) =
                        commands::sync::sync_push(&runner, &cfg, &session, &root, verbose)
                    {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
                SyncCommand::Pull { session_name } => {
                    let session = resolve_session(session_name, &root);
                    if let Err(e) =
                        commands::sync::sync_pull(&runner, &cfg, &session, &root, verbose)
                    {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        Command::Status { .. } => eprintln!("not yet implemented: status"),
        Command::List => eprintln!("not yet implemented: list"),
        Command::Destroy { .. } => eprintln!("not yet implemented: destroy"),
    }
}
