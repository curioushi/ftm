use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Response types (mirrors server types for deserialization)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct MessageResponse {
    message: String,
}

#[derive(Deserialize)]
pub struct HealthInfo {
    #[allow(dead_code)]
    pub status: String,
    #[allow(dead_code)]
    pub pid: Option<u32>,
    pub watch_dir: Option<String>,
}

#[derive(Deserialize)]
pub struct FileTreeNode {
    pub name: String,
    pub count: Option<usize>,
    pub children: Option<Vec<FileTreeNode>>,
}

#[derive(Deserialize)]
pub struct HistoryEntry {
    pub timestamp: String,
    pub op: String,
    #[allow(dead_code)]
    pub file: String,
    pub checksum: Option<String>,
    pub size: Option<u64>,
}

#[derive(Deserialize)]
pub struct ScanResult {
    pub created: usize,
    pub modified: usize,
    pub deleted: usize,
    pub unchanged: usize,
}

#[derive(Serialize)]
struct CheckoutRequest {
    directory: String,
}

#[derive(Serialize)]
struct RestoreRequest {
    file: String,
    checksum: String,
}

#[derive(Deserialize)]
struct VersionInfo {
    version: String,
}

#[derive(Deserialize)]
struct ConfigResponse {
    data: String,
}

#[derive(Serialize)]
struct ConfigSetRequest {
    key: String,
    value: String,
}

#[derive(Deserialize)]
struct LogsInfo {
    log_dir: String,
    files: Vec<String>,
}

// ---------------------------------------------------------------------------
// Client helpers
// ---------------------------------------------------------------------------

fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{}", port)
}

fn make_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .expect("failed to build HTTP client")
}

/// Send a request and handle connection errors with a friendly message.
fn handle_connection_error(err: reqwest::Error) -> anyhow::Error {
    if err.is_connect() {
        anyhow::anyhow!("Server not running. Use 'ftm checkout <dir>' to start.")
    } else {
        err.into()
    }
}

/// Extract error message from a non-success HTTP response.
fn check_response(resp: reqwest::blocking::Response) -> Result<reqwest::blocking::Response> {
    if resp.status().is_success() {
        Ok(resp)
    } else {
        let status = resp.status();
        let body: MessageResponse = resp.json().unwrap_or(MessageResponse {
            message: format!("Server returned {}", status),
        });
        anyhow::bail!("{}", body.message)
    }
}

// ---------------------------------------------------------------------------
// Public client functions
// ---------------------------------------------------------------------------

/// Check whether the server is reachable on the given port.
pub fn is_server_running(port: u16) -> bool {
    make_client()
        .get(format!("{}/api/health", base_url(port)))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Fetch health info from the server (including current watch dir).
pub fn client_health(port: u16) -> Result<HealthInfo> {
    let resp = make_client()
        .get(format!("{}/api/health", base_url(port)))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .map_err(handle_connection_error)?;
    let resp = check_response(resp)?;
    resp.json().context("Failed to parse health response")
}

/// Request the server to shut down gracefully.
pub fn client_shutdown(port: u16) -> Result<()> {
    let resp = make_client()
        .post(format!("{}/api/shutdown", base_url(port)))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .map_err(handle_connection_error)?;
    let _ = check_response(resp)?;
    Ok(())
}

/// Poll the health endpoint until the server stops responding, or timeout.
/// Returns `true` if the server stopped, `false` on timeout.
pub fn wait_for_server_shutdown(port: u16, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if !is_server_running(port) {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

pub fn client_checkout(port: u16, directory: &str) -> Result<()> {
    let resp = make_client()
        .post(format!("{}/api/checkout", base_url(port)))
        .json(&CheckoutRequest {
            directory: directory.to_string(),
        })
        .send()
        .map_err(handle_connection_error)?;
    let resp = check_response(resp)?;
    let msg: MessageResponse = resp.json().context("Failed to parse response")?;
    println!("{}", msg.message);
    Ok(())
}

pub fn client_ls(port: u16, include_deleted: bool) -> Result<()> {
    // Best-effort: show current watch directory
    if let Ok(health) = client_health(port) {
        if let Some(dir) = &health.watch_dir {
            println!("Watch directory: {}", dir);
        }
    }

    let url = if include_deleted {
        format!("{}/api/files?include_deleted=true", base_url(port))
    } else {
        format!("{}/api/files", base_url(port))
    };
    let resp = make_client()
        .get(url)
        .send()
        .map_err(handle_connection_error)?;
    let resp = check_response(resp)?;
    let tree: Vec<FileTreeNode> = resp.json().context("Failed to parse response")?;

    if tree.is_empty() {
        println!("No files tracked yet.");
    } else {
        println!("Tracked files:");
        print_file_tree(&tree, "");
    }
    Ok(())
}

fn print_file_tree(nodes: &[FileTreeNode], prefix: &str) {
    let n = nodes.len();
    for (i, node) in nodes.iter().enumerate() {
        let is_last = i == n - 1;
        let (branch, next_prefix) = if is_last {
            ("└── ", "    ")
        } else {
            ("├── ", "│   ")
        };
        let line_prefix = if prefix.is_empty() {
            branch.to_string()
        } else {
            format!("{}{}", prefix, branch)
        };
        match &node.children {
            None => {
                let count = node.count.unwrap_or(0);
                println!("{}{} ({} entries)", line_prefix, node.name, count);
            }
            Some(children) => {
                println!("{}{}/", line_prefix, node.name);
                let new_prefix = format!("{}{}", prefix, next_prefix);
                print_file_tree(children, &new_prefix);
            }
        }
    }
}

pub fn client_history(port: u16, file: &str) -> Result<()> {
    let resp = make_client()
        .get(format!("{}/api/history", base_url(port)))
        .query(&[("file", file)])
        .send()
        .map_err(handle_connection_error)?;
    let resp = check_response(resp)?;
    let entries: Vec<HistoryEntry> = resp.json().context("Failed to parse response")?;

    if entries.is_empty() {
        println!("No history for '{}'", file);
    } else {
        println!("History for '{}':", file);
        for entry in entries.iter().rev() {
            let checksum_short = entry.checksum.as_ref().map(|c| &c[..8]).unwrap_or("-");
            let size_str = entry
                .size
                .map(|s| format!("{} bytes", s))
                .unwrap_or_else(|| "-".to_string());
            // Parse and reformat timestamp to local time
            let display_time = match chrono::DateTime::parse_from_rfc3339(&entry.timestamp) {
                Ok(dt) => {
                    let local = dt.with_timezone(&chrono::Local);
                    local.format("%Y-%m-%d %H:%M:%S").to_string()
                }
                Err(_) => entry.timestamp.clone(),
            };
            println!(
                "  {} | {} | {} | {}",
                display_time, entry.op, checksum_short, size_str
            );
        }
    }
    Ok(())
}

pub fn client_restore(port: u16, file: &str, checksum: &str) -> Result<()> {
    let resp = make_client()
        .post(format!("{}/api/restore", base_url(port)))
        .json(&RestoreRequest {
            file: file.to_string(),
            checksum: checksum.to_string(),
        })
        .send()
        .map_err(handle_connection_error)?;
    let resp = check_response(resp)?;
    let msg: MessageResponse = resp.json().context("Failed to parse response")?;
    println!("{}", msg.message);
    Ok(())
}

pub fn client_scan(port: u16) -> Result<()> {
    let resp = make_client()
        .post(format!("{}/api/scan", base_url(port)))
        .send()
        .map_err(handle_connection_error)?;
    let resp = check_response(resp)?;
    let result: ScanResult = resp.json().context("Failed to parse response")?;
    println!(
        "Scan complete: {} created, {} modified, {} deleted, {} unchanged",
        result.created, result.modified, result.deleted, result.unchanged
    );
    Ok(())
}

pub fn client_version(port: u16) -> Result<()> {
    println!("Client version: {}", env!("CARGO_PKG_VERSION"));

    match make_client()
        .get(format!("{}/api/version", base_url(port)))
        .timeout(std::time::Duration::from_secs(2))
        .send()
    {
        Ok(resp) => {
            let resp = check_response(resp)?;
            let info: VersionInfo = resp.json().context("Failed to parse version response")?;
            println!("Server version: {}", info.version);
        }
        Err(_) => {
            println!("Server: not running");
        }
    }
    Ok(())
}

pub fn client_config_get(port: u16, key: Option<&str>) -> Result<()> {
    let mut req = make_client().get(format!("{}/api/config", base_url(port)));
    if let Some(k) = key {
        req = req.query(&[("key", k)]);
    }
    let resp = req.send().map_err(handle_connection_error)?;
    let resp = check_response(resp)?;
    let config: ConfigResponse = resp.json().context("Failed to parse config response")?;
    println!("{}", config.data);
    Ok(())
}

pub fn client_config_set(port: u16, key: &str, value: &str) -> Result<()> {
    let resp = make_client()
        .post(format!("{}/api/config", base_url(port)))
        .json(&ConfigSetRequest {
            key: key.to_string(),
            value: value.to_string(),
        })
        .send()
        .map_err(handle_connection_error)?;
    let resp = check_response(resp)?;
    let msg: MessageResponse = resp.json().context("Failed to parse response")?;
    println!("{}", msg.message);
    Ok(())
}

pub fn client_logs(port: u16) -> Result<()> {
    let resp = make_client()
        .get(format!("{}/api/logs", base_url(port)))
        .send()
        .map_err(handle_connection_error)?;
    let resp = check_response(resp)?;
    let info: LogsInfo = resp.json().context("Failed to parse logs response")?;

    if info.files.is_empty() {
        println!("No log files found in {}", info.log_dir);
        return Ok(());
    }

    // Pick the latest log file (list is sorted newest-first by server)
    let latest = &info.files[0];
    let log_path = std::path::PathBuf::from(&info.log_dir).join(latest);
    let log_path_str = log_path.to_string_lossy().to_string();

    println!("Opening: {}", log_path_str);

    // Try to open with `less`
    let status = std::process::Command::new("less")
        .arg("+G") // start at end of file
        .arg(&log_path_str)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        _ => {
            // Fallback: read and print the file directly
            eprintln!("'less' not available, printing file content:");
            let content = std::fs::read_to_string(&log_path)
                .with_context(|| format!("Failed to read log file: {}", log_path_str))?;
            print!("{}", content);
            Ok(())
        }
    }
}
