mod cli;
mod config;
mod discovery;
mod error;
mod session;

use clap::Parser;
use cli::{Cli, Command, RemoteCommand, SyncCommand};
use tracing_subscriber::FmtSubscriber;

fn main() {
    let cli = Cli::parse();

    FmtSubscriber::builder()
        .with_max_level(cli.log_level())
        .init();

    match cli.command {
        Command::Init => eprintln!("not yet implemented: init"),
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
