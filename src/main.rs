mod client;
mod config;
mod scanner;
mod server;
mod storage;
mod types;
mod watcher;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ftm", about = "File Time Machine - Text file version tracking")]
struct Cli {
    /// Server port (used by serve and all client commands)
    #[arg(long, default_value_t = 8765, global = true)]
    port: u16,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the FTM server (daemon mode)
    Serve {
        /// Custom log directory (default: .ftm/log/)
        #[arg(long)]
        log_dir: Option<PathBuf>,
    },
    /// Initialize .ftm in a directory and start watching
    Checkout {
        /// Directory to watch (must be absolute path)
        directory: PathBuf,
    },
    /// List tracked files
    Ls,
    /// Show version history for a file
    History { file: String },
    /// Restore a file to a specific version
    Restore {
        file: String,
        /// Checksum of the version to restore (at least first 8 chars)
        #[arg(short, long)]
        checksum: String,
    },
    /// Scan directory for changes (detect creates, modifies, deletes)
    Scan,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { log_dir } => {
            // Initialize logging
            if let Some(log_dir) = log_dir {
                init_file_logging(&log_dir)?;
            } else {
                tracing_subscriber::fmt::init();
            }

            // Start async server
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(server::serve(cli.port))
        }
        Commands::Checkout { directory } => {
            // Resolve to absolute path
            let abs_dir = if directory.is_absolute() {
                directory
            } else {
                std::env::current_dir()?.join(directory)
            };
            let abs_dir = abs_dir.canonicalize().unwrap_or_else(|_| abs_dir.clone());
            client::client_checkout(cli.port, &abs_dir.to_string_lossy())
        }
        Commands::Ls => client::client_ls(cli.port),
        Commands::History { file } => client::client_history(cli.port, &file),
        Commands::Restore { file, checksum } => client::client_restore(cli.port, &file, &checksum),
        Commands::Scan => client::client_scan(cli.port),
    }
}

/// Initialize file-based logging to a directory.
fn init_file_logging(log_dir: &std::path::Path) -> Result<()> {
    use chrono::Local;
    use std::sync::Mutex;

    std::fs::create_dir_all(log_dir)?;
    let now = Local::now();
    let log_filename = now.format("%Y%m%d-%H%M%S.log").to_string();
    let log_file_path = log_dir.join(&log_filename);
    let log_file = std::fs::File::create(&log_file_path)?;

    tracing_subscriber::fmt()
        .with_writer(Mutex::new(log_file))
        .with_ansi(false)
        .init();

    eprintln!("Log file: {}", log_file_path.display());
    Ok(())
}
