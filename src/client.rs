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
    pub watch_dir: Option<String>,
}

#[derive(Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub count: usize,
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

// ---------------------------------------------------------------------------
// Client helpers
// ---------------------------------------------------------------------------

fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{}", port)
}

fn make_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::new()
}

/// Send a request and handle connection errors with a friendly message.
fn handle_connection_error(err: reqwest::Error) -> anyhow::Error {
    if err.is_connect() {
        anyhow::anyhow!("Server not running. Start with 'ftm serve'")
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

pub fn client_ls(port: u16) -> Result<()> {
    // Best-effort: show current watch directory
    if let Ok(health) = client_health(port) {
        if let Some(dir) = &health.watch_dir {
            println!("Watch directory: {}", dir);
        }
    }

    let resp = make_client()
        .get(format!("{}/api/files", base_url(port)))
        .send()
        .map_err(handle_connection_error)?;
    let resp = check_response(resp)?;
    let files: Vec<FileEntry> = resp.json().context("Failed to parse response")?;

    if files.is_empty() {
        println!("No files tracked yet.");
    } else {
        println!("Tracked files:");
        for f in &files {
            println!("  {} ({} entries)", f.path, f.count);
        }
    }
    Ok(())
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
