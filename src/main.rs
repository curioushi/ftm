mod config;
mod storage;
mod types;
mod watcher;

use anyhow::{Context, Result};
use chrono::Local;
use clap::{Parser, Subcommand};
use config::Config;
use std::path::PathBuf;
use storage::Storage;
use tracing::info;
use watcher::FileWatcher;

const FTM_DIR: &str = ".ftm";

#[derive(Parser)]
#[command(name = "ftm", about = "File Time Machine - Text file version tracking")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize .ftm in current directory
    Init,
    /// Start watching for file changes
    Watch,
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
}

fn get_ftm_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(cwd.join(FTM_DIR))
}

fn ensure_initialized() -> Result<PathBuf> {
    let ftm_dir = get_ftm_dir()?;
    if !ftm_dir.exists() {
        anyhow::bail!("Not initialized. Run 'ftm init' first.");
    }
    Ok(ftm_dir)
}

fn cmd_init() -> Result<()> {
    let ftm_dir = get_ftm_dir()?;
    if ftm_dir.exists() {
        println!("Already initialized.");
        return Ok(());
    }

    std::fs::create_dir_all(&ftm_dir)?;
    let config = Config::default();
    config.save(&ftm_dir.join("config.yaml"))?;

    // Create empty index with history array
    let index = types::Index::default();
    let index_content = serde_json::to_string_pretty(&index)?;
    std::fs::write(ftm_dir.join("index.json"), index_content)?;

    println!("Initialized .ftm in current directory.");
    Ok(())
}

fn cmd_watch() -> Result<()> {
    let ftm_dir = ensure_initialized()?;
    let root_dir = std::env::current_dir()?;

    let config = Config::load(&ftm_dir.join("config.yaml")).context("Failed to load config")?;
    let storage = Storage::new(ftm_dir, config.settings.max_history);
    let watcher = FileWatcher::new(root_dir, config, storage);

    info!("Starting file watcher...");
    println!("Watching for changes. Press Ctrl+C to stop.");
    watcher.watch()
}

fn cmd_ls() -> Result<()> {
    let ftm_dir = ensure_initialized()?;
    let config = Config::load(&ftm_dir.join("config.yaml"))?;
    let storage = Storage::new(ftm_dir, config.settings.max_history);
    let files = storage.list_files()?;

    if files.is_empty() {
        println!("No files tracked yet.");
    } else {
        println!("Tracked files:");
        for (path, count) in &files {
            println!("  {} ({} entries)", path, count);
        }
    }
    Ok(())
}

fn cmd_history(file: &str) -> Result<()> {
    let ftm_dir = ensure_initialized()?;
    let config = Config::load(&ftm_dir.join("config.yaml"))?;
    let storage = Storage::new(ftm_dir, config.settings.max_history);
    let entries = storage.list_history(file)?;

    if entries.is_empty() {
        println!("No history for '{}'", file);
    } else {
        println!("History for '{}':", file);
        for entry in entries.iter().rev() {
            let local_time = entry.timestamp.with_timezone(&Local);
            let checksum_short = entry.checksum.as_ref().map(|c| &c[..8]).unwrap_or("-");
            let size_str = entry
                .size
                .map(|s| format!("{} bytes", s))
                .unwrap_or_else(|| "-".to_string());
            println!(
                "  {} | {} | {} | {}",
                local_time.format("%Y-%m-%d %H:%M:%S"),
                entry.op,
                checksum_short,
                size_str
            );
        }
    }
    Ok(())
}

fn cmd_restore(file: &str, checksum: &str) -> Result<()> {
    let ftm_dir = ensure_initialized()?;
    let root_dir = std::env::current_dir()?;
    let config = Config::load(&ftm_dir.join("config.yaml"))?;
    let storage = Storage::new(ftm_dir, config.settings.max_history);

    storage.restore(file, checksum, &root_dir)?;
    println!(
        "Restored '{}' to checksum '{}'",
        file,
        &checksum[..8.min(checksum.len())]
    );
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => cmd_init(),
        Commands::Watch => cmd_watch(),
        Commands::Ls => cmd_ls(),
        Commands::History { file } => cmd_history(&file),
        Commands::Restore { file, checksum } => cmd_restore(&file, &checksum),
    }
}
