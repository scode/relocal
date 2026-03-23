mod cli;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use clap::Parser;
use cli::{Cli, Command, RemoteCommand, SyncCommand};
use relocal::{commands, config, daemon, discovery, runner, session, ssh};
use tracing::error;
use tracing_subscriber::FmtSubscriber;

/// Returns the user's home directory, or exits with an error.
fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| {
        error!("could not determine home directory");
        std::process::exit(1);
    })
}

/// Finds the repo root and loads merged config (user + project). Exits on failure.
fn load_config() -> (PathBuf, config::Config) {
    let root = discovery::find_repo_root(&std::env::current_dir().unwrap()).unwrap_or_else(|e| {
        error!("{e}");
        std::process::exit(1);
    });
    let cfg = config::load_merged_config(&home_dir(), &root).unwrap_or_else(|e| {
        error!("{e}");
        std::process::exit(1);
    });
    (root, cfg)
}

/// Resolves the session name: explicit name if given, otherwise hashed from
/// the repo root path and git origin.
fn resolve_session(name: Option<String>, repo_root: &Path) -> String {
    match name {
        Some(n) => {
            session::validate_session_name(&n).unwrap_or_else(|e| {
                error!("{e}");
                std::process::exit(1);
            });
            n
        }
        None => session::hashed_session_name(repo_root).unwrap_or_else(|e| {
            error!("{e}");
            std::process::exit(1);
        }),
    }
}

/// Initializes the tracing subscriber for the daemon, writing to a log file.
///
/// Returns the parsed config so `run_daemon` can reuse it without re-reading
/// `relocal.toml`.
///
/// The daemon's tracing output goes to a file rather than stderr because the
/// daemon process outlives the spawning client. Background sync noise on the
/// client's terminal would be distracting during interactive sessions.
///
/// Errors here use eprintln! because tracing is not yet initialized.
fn init_daemon_tracing(
    level: tracing::Level,
    session_name: &str,
    repo_root: &str,
) -> config::Config {
    let cfg = config::load_merged_config(&home_dir(), Path::new(repo_root)).unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    });

    let log_path = ssh::daemon_log_path(session_name, &cfg.remote);
    let log_file = std::fs::File::create(&log_path).unwrap_or_else(|e| {
        eprintln!("Error creating daemon log file: {e}");
        std::process::exit(1);
    });
    // Restrict to owner-only, consistent with the daemon socket and flock.
    let _ = std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o600));

    FmtSubscriber::builder()
        .with_max_level(level)
        .with_writer(std::sync::Mutex::new(log_file))
        .with_ansi(false)
        .init();

    cfg
}

fn main() {
    let cli = Cli::parse();
    let verbose = cli.verbose > 0;

    let daemon_config = if let Command::Daemon {
        ref session_name,
        ref repo_root,
    } = cli.command
    {
        Some(init_daemon_tracing(
            cli.log_level(),
            session_name,
            repo_root,
        ))
    } else {
        FmtSubscriber::builder()
            .with_max_level(cli.log_level())
            .with_writer(std::io::stderr)
            .init();
        None
    };

    match cli.command {
        Command::Init => {
            if let Err(e) = commands::init::run(&std::env::current_dir().unwrap()) {
                error!("{e}");
                std::process::exit(1);
            }
        }
        Command::Remote { command } => match command {
            RemoteCommand::Install => {
                let (_root, cfg) = load_config();
                let runner = runner::ProcessRunner::default();
                if let Err(e) = commands::install::run(&runner, &cfg) {
                    error!("{e}");
                    std::process::exit(1);
                }
            }
            RemoteCommand::Nuke => {
                let (_root, cfg) = load_config();
                let runner = runner::ProcessRunner::default();
                if let Err(e) = commands::nuke::run(&runner, &cfg, true) {
                    error!("{e}");
                    std::process::exit(1);
                }
            }
        },
        Command::Claude {
            session_name,
            claude_args,
        } => {
            let (root, cfg) = load_config();
            let session = resolve_session(session_name, &root);
            if let Err(e) = commands::claude::run(&cfg, &session, &root, cli.verbose, &claude_args)
            {
                error!("{e}");
                std::process::exit(1);
            }
        }
        Command::Codex {
            session_name,
            codex_args,
        } => {
            let (root, cfg) = load_config();
            let session = resolve_session(session_name, &root);
            if let Err(e) = commands::codex::run(&cfg, &session, &root, cli.verbose, &codex_args) {
                error!("{e}");
                std::process::exit(1);
            }
        }
        Command::Ssh { session_name } => {
            let (root, cfg) = load_config();
            let runner = runner::ProcessRunner::default();
            let session = resolve_session(session_name, &root);
            if let Err(e) = commands::ssh::run(&runner, &cfg, &session) {
                error!("{e}");
                std::process::exit(1);
            }
        }
        Command::Sync { command } => {
            let (root, cfg) = load_config();
            let runner = runner::ProcessRunner::default();
            match command {
                SyncCommand::Push { session_name } => {
                    let session = resolve_session(session_name, &root);
                    if let Err(e) =
                        commands::sync::sync_push(&runner, &cfg, &session, &root, verbose)
                    {
                        error!("{e}");
                        std::process::exit(1);
                    }
                }
                SyncCommand::Pull { session_name } => {
                    let session = resolve_session(session_name, &root);
                    if let Err(e) =
                        commands::sync::sync_pull(&runner, &cfg, &session, &root, verbose)
                    {
                        error!("{e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        Command::Status { session_name } => {
            let (root, cfg) = load_config();
            let runner = runner::ProcessRunner::default();
            let session = resolve_session(session_name, &root);
            if let Err(e) = commands::status::run(&runner, &cfg, &session) {
                error!("{e}");
                std::process::exit(1);
            }
        }
        Command::Log { session_name } => {
            let (root, cfg) = load_config();
            let session = resolve_session(session_name, &root);
            if let Err(e) = commands::log::run(&cfg, &session) {
                error!("{e}");
                std::process::exit(1);
            }
        }
        Command::List => {
            let (_root, cfg) = load_config();
            let runner = runner::ProcessRunner::default();
            if let Err(e) = commands::list::run(&runner, &cfg) {
                error!("{e}");
                std::process::exit(1);
            }
        }
        Command::Destroy { session_name } => {
            let (root, cfg) = load_config();
            let runner = runner::ProcessRunner::default();
            let session = resolve_session(session_name, &root);
            if let Err(e) = commands::destroy::run(&runner, &cfg, &session, true, true) {
                error!("{e}");
                std::process::exit(1);
            }
        }
        Command::Daemon {
            session_name,
            repo_root,
        } => {
            let cfg = daemon_config.expect("daemon config set above");
            if let Err(e) = daemon::run_daemon(&cfg, &session_name, Path::new(&repo_root), verbose)
            {
                error!("{e}");
                std::process::exit(1);
            }
        }
    }
}
