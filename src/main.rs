mod client;
mod config;
mod path_util;
mod scanner;
mod server;
mod storage;
mod types;
mod watcher;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ftm", about = "File Time Machine - Text file version tracking")]
struct Cli {
    /// Server port (used by serve and all client commands)
    #[arg(long, default_value_t = 13580, global = true)]
    port: u16,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print client and server version
    Version,
    /// Initialize .ftm in a directory and start watching
    Checkout {
        /// Directory to watch (absolute or relative path)
        directory: PathBuf,
    },
    /// List tracked files (excludes deleted by default; use --include-deleted to show all)
    Ls {
        /// Include files whose last history entry is Delete
        #[arg(long, action = clap::ArgAction::SetTrue)]
        include_deleted: bool,
    },
    /// Scan directory for changes (detect creates, modifies, deletes)
    Scan,
    /// Remove snapshot files not referenced by any history entry
    Clean,
    /// Show version history for a file
    History { file: String },
    /// Restore a file to a specific version
    Restore {
        file: String,
        /// Checksum of the version to restore (at least first 8 chars)
        checksum: String,
    },
    /// Get or set configuration values
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Show history and quota usage (current / max)
    Stats,
    /// Start the FTM server (daemon mode, internal use only)
    #[command(hide = true)]
    Serve {
        /// Custom log directory (default: .ftm/logs/)
        #[arg(long)]
        log_dir: Option<PathBuf>,
    },
    /// Show logs (opens latest log file with less)
    Logs,
    /// Stop the running FTM server gracefully
    Stop,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Get config value (all if no key specified)
    Get {
        /// Config key (e.g. settings.max_history, watch.patterns)
        key: Option<String>,
    },
    /// Set a config value
    Set {
        /// Config key (e.g. settings.max_history, watch.patterns)
        key: String,
        /// New value (use comma-separated for list keys)
        value: String,
    },
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

            // Start async server (Web UI always enabled)
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

            // If a server is already watching the exact same directory, keep it
            // but still kill every other ftm process to guarantee a single server.
            if client::is_server_running(cli.port) {
                if let Ok(health) = client::client_health(cli.port) {
                    if let Some(ref watch_dir) = health.watch_dir {
                        if PathBuf::from(watch_dir) == abs_dir {
                            kill_all_servers(health.pid);
                            println!("Already watching: {}", abs_dir.display());
                            println!("Web UI: http://127.0.0.1:{}", cli.port);
                            return Ok(());
                        }
                    }
                }
            }

            // Kill all ftm server processes, then start a fresh one.
            kill_all_servers(None);
            wait_for_port_free(cli.port);
            auto_start_server(cli.port, &abs_dir)?;

            client::client_checkout(cli.port, &abs_dir.to_string_lossy())?;
            println!("Web UI: http://127.0.0.1:{}", cli.port);
            Ok(())
        }
        Commands::Version => client::client_version(cli.port),
        Commands::Ls { include_deleted } => client::client_ls(cli.port, include_deleted),
        Commands::History { file } => client::client_history(cli.port, &file),
        Commands::Restore { file, checksum } => client::client_restore(cli.port, &file, &checksum),
        Commands::Scan => client::client_scan(cli.port),
        Commands::Clean => client::client_clean(cli.port),
        Commands::Config { action } => match action {
            ConfigAction::Get { key } => client::client_config_get(cli.port, key.as_deref()),
            ConfigAction::Set { key, value } => client::client_config_set(cli.port, &key, &value),
        },
        Commands::Stats => client::client_stats(cli.port),
        Commands::Logs => client::client_logs(cli.port),
        Commands::Stop => {
            if !client::is_server_running(cli.port) {
                println!("Server is not running on port {}.", cli.port);
                return Ok(());
            }
            client::client_shutdown(cli.port)?;
            if client::wait_for_server_shutdown(cli.port, std::time::Duration::from_secs(5)) {
                println!("Server stopped.");
            } else {
                anyhow::bail!("Server did not stop within 5 seconds");
            }
            Ok(())
        }
    }
}

/// Kill every ftm process except ourselves and an optional `keep_pid`.
fn kill_all_servers(keep_pid: Option<u32>) {
    use sysinfo::System;

    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let my_pid = std::process::id();

    for (pid, process) in sys.processes() {
        let p = pid.as_u32();
        if p == my_pid || Some(p) == keep_pid {
            continue;
        }

        if !process
            .name()
            .to_str()
            .is_some_and(|n| n.starts_with("ftm"))
        {
            continue;
        }

        eprintln!("Killing ftm process (pid: {})", p);
        process.kill();
    }
}

/// Wait until the given port is free (nothing listening). On Windows, the OS
/// may not release the port immediately after the process exits; this avoids
/// "address already in use" when starting a new server on the same port.
fn wait_for_port_free(port: u16) {
    use std::io::ErrorKind;
    use std::net::TcpListener;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(2);
    while start.elapsed() < timeout {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(_listener) => return,
            Err(e) if e.kind() == ErrorKind::AddrInUse => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => return,
        }
    }
}

/// Start a detached FTM server process in the background and wait for it to
/// become healthy before returning.
///
/// The server is started with `--log-dir {watch_dir}/.ftm/logs/` so that
/// tracing output is persisted to disk and accessible via `ftm logs`.
fn auto_start_server(port: u16, watch_dir: &std::path::Path) -> Result<()> {
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe().context("Failed to determine current executable path")?;

    let log_dir = watch_dir.join(".ftm").join("logs");
    let mut cmd = Command::new(&exe);
    cmd.arg("--port")
        .arg(port.to_string())
        .arg("serve")
        .arg("--log-dir")
        .arg(&log_dir);

    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // On Unix, put the child in its own process group so it won't receive
    // signals (e.g. Ctrl-C) sent to the parent's group.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd.spawn().context("Failed to start FTM server")?;
    let pid = child.id();

    eprintln!("Starting FTM server on port {} (pid: {})...", port, pid);

    // Poll until the server is healthy or timeout.
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);

    loop {
        if client::is_server_running(port) {
            eprintln!("Server is ready.");
            return Ok(());
        }
        if start.elapsed() > timeout {
            anyhow::bail!("Timed out waiting for FTM server to start on port {}", port);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Remove old log files in `log_dir`, keeping only the most recent `keep` files.
/// Log filenames are YYYYMMDD-HHMMSS.mmm.log, so sorting by name descending gives newest first.
fn prune_old_logs(log_dir: &std::path::Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return;
    };
    let mut names: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "log"))
        .collect();
    if names.len() <= keep {
        return;
    }
    names.sort_unstable_by(|a, b| b.cmp(a));
    for path in names.into_iter().skip(keep) {
        let _ = std::fs::remove_file(&path);
    }
}

/// Initialize file-based logging to a directory.
fn init_file_logging(log_dir: &std::path::Path) -> Result<()> {
    use chrono::Local;
    use std::sync::Mutex;

    const KEEP_LOGS: usize = 100;

    std::fs::create_dir_all(log_dir)?;
    prune_old_logs(log_dir, KEEP_LOGS);
    let now = Local::now();
    let log_filename = format!(
        "{}.{:03}.log",
        now.format("%Y%m%d-%H%M%S"),
        now.timestamp_subsec_millis()
    );
    let log_file_path = log_dir.join(&log_filename);
    let log_file = std::fs::File::create(&log_file_path)?;

    tracing_subscriber::fmt()
        .with_writer(Mutex::new(log_file))
        .with_ansi(false)
        .init();

    eprintln!("Log file: {}", log_file_path.display());
    Ok(())
}
