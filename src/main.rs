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

use clap::Parser;
use cli::{Cli, Command, RemoteCommand, SyncCommand};
use tracing_subscriber::FmtSubscriber;

fn main() {
    let cli = Cli::parse();

    FmtSubscriber::builder()
        .with_max_level(cli.log_level())
        .init();

    match cli.command {
        Command::Init => {
            if let Err(e) = commands::init::run(&std::env::current_dir().unwrap()) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Command::Remote { command } => match command {
            RemoteCommand::Install => eprintln!("not yet implemented: remote install"),
            RemoteCommand::Nuke => eprintln!("not yet implemented: remote nuke"),
        },
        Command::Start { .. } => eprintln!("not yet implemented: start"),
        Command::Sync { command } => match command {
            SyncCommand::Push { .. } => eprintln!("not yet implemented: sync push"),
            SyncCommand::Pull { .. } => eprintln!("not yet implemented: sync pull"),
        },
        Command::Status { .. } => eprintln!("not yet implemented: status"),
        Command::List => eprintln!("not yet implemented: list"),
        Command::Destroy { .. } => eprintln!("not yet implemented: destroy"),
    }
}
