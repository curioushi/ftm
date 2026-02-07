mod client;
mod config;
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
    #[arg(long, default_value_t = 8765, global = true)]
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
        /// Directory to watch (must be absolute path)
        directory: PathBuf,
    },
    /// List tracked files
    Ls,
    /// Scan directory for changes (detect creates, modifies, deletes)
    Scan,
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
    /// Start the FTM server (daemon mode)
    Serve {
        /// Custom log directory (default: .ftm/log/)
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

            // Kill any stale (unreachable) ftm server processes so that at
            // most one healthy server remains.
            kill_stale_servers(cli.port);

            if client::is_server_running(cli.port) {
                // Server is alive — check what it is watching
                if let Ok(health) = client::client_health(cli.port) {
                    if let Some(ref old_dir) = health.watch_dir {
                        let old_path = PathBuf::from(old_dir);
                        if old_path == abs_dir {
                            // Same directory — nothing to do
                            println!("Already watching: {}", abs_dir.display());
                            return Ok(());
                        }
                        // Different directory — switch
                        eprintln!(
                            "Switching watch directory: {} -> {}",
                            old_dir,
                            abs_dir.display()
                        );
                        let _ = client::client_shutdown(cli.port);
                        if !client::wait_for_server_shutdown(
                            cli.port,
                            std::time::Duration::from_secs(2),
                        ) {
                            anyhow::bail!("Server did not stop within 2 seconds");
                        }
                        // Start a fresh server for the new directory
                        auto_start_server(cli.port, &abs_dir)?;
                    }
                    // else: server running, not watching anything — just checkout below
                }
                // else: health call failed — try checkout anyway
            } else {
                // Server not running — auto-start
                auto_start_server(cli.port, &abs_dir)?;
            }

            client::client_checkout(cli.port, &abs_dir.to_string_lossy())
        }
        Commands::Version => client::client_version(cli.port),
        Commands::Ls => client::client_ls(cli.port),
        Commands::History { file } => client::client_history(cli.port, &file),
        Commands::Restore { file, checksum } => client::client_restore(cli.port, &file, &checksum),
        Commands::Scan => client::client_scan(cli.port),
        Commands::Config { action } => match action {
            ConfigAction::Get { key } => client::client_config_get(cli.port, key.as_deref()),
            ConfigAction::Set { key, value } => client::client_config_set(cli.port, &key, &value),
        },
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

/// Kill every ftm process except ourselves and the healthy server on `port`.
///
/// Strategy: ask the server on `port` for its PID via the health endpoint.
/// Then enumerate all system processes named "ftm" via `sysinfo` and kill
/// every one that is neither this CLI process nor the healthy server.
fn kill_stale_servers(port: u16) {
    use sysinfo::System;

    // If a server is reachable on our port, protect its PID.
    let healthy_pid: Option<u32> = client::client_health(port).ok().and_then(|h| h.pid);

    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let my_pid = std::process::id();

    for (pid, process) in sys.processes() {
        let p = pid.as_u32();
        if p == my_pid || Some(p) == healthy_pid {
            continue;
        }

        if !process
            .name()
            .to_str()
            .map_or(false, |n| n.starts_with("ftm"))
        {
            continue;
        }

        eprintln!("Killing stale ftm process (pid: {})", p);
        process.kill();
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

/// Initialize file-based logging to a directory.
fn init_file_logging(log_dir: &std::path::Path) -> Result<()> {
    use chrono::Local;
    use std::sync::Mutex;

    std::fs::create_dir_all(log_dir)?;
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
