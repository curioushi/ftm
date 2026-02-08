use crate::config::Config;
use crate::scanner::Scanner;
use crate::storage::Storage;
use crate::types::{FileTreeNode, HistoryEntry};
use crate::watcher::FileWatcher;
use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;
use tokio::sync::{Notify, RwLock};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Shared config wrapped in std RwLock so both async handlers and blocking
/// threads (FileWatcher) can read/write without requiring a tokio runtime.
type SharedConfig = Arc<StdRwLock<Config>>;

struct WatchContext {
    watch_dir: PathBuf,
    config: SharedConfig,
}

pub struct AppState {
    ctx: RwLock<Option<WatchContext>>,
    shutdown: Notify,
}

impl AppState {
    fn new() -> Self {
        Self {
            ctx: RwLock::new(None),
            shutdown: Notify::new(),
        }
    }

    /// Create a Storage instance for the current watch context.
    async fn storage(&self) -> Option<(Storage, PathBuf)> {
        let guard = self.ctx.read().await;
        guard.as_ref().map(|c| {
            let ftm_dir = c.watch_dir.join(".ftm");
            let max_history = c.config.read().unwrap().settings.max_history;
            let storage = Storage::new(ftm_dir, max_history);
            (storage, c.watch_dir.clone())
        })
    }
}

type SharedState = Arc<AppState>;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CheckoutRequest {
    directory: String,
}

#[derive(Serialize)]
struct MessageResponse {
    message: String,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    pid: u32,
    watch_dir: Option<String>,
}

#[derive(Deserialize)]
struct HistoryQuery {
    file: String,
}

#[derive(Deserialize)]
struct RestoreRequest {
    file: String,
    checksum: String,
}

#[derive(Serialize)]
struct VersionResponse {
    version: String,
}

#[derive(Deserialize)]
struct ConfigQuery {
    key: Option<String>,
}

#[derive(Deserialize)]
struct ConfigSetRequest {
    key: String,
    value: String,
}

#[derive(Serialize)]
struct ConfigResponse {
    /// Full YAML dump when no key is specified, or the single value.
    data: String,
}

#[derive(Serialize)]
struct LogsResponse {
    log_dir: String,
    files: Vec<String>,
}

#[derive(Deserialize)]
struct SnapshotQuery {
    checksum: String,
}

#[derive(Deserialize)]
struct DiffQuery {
    /// Checksum of the "old" version. Empty or absent means diff against empty.
    from: Option<String>,
    /// Checksum of the "new" version.
    to: String,
}

#[derive(Serialize)]
struct DiffResponse {
    hunks: Vec<DiffHunk>,
    old_total: usize,
    new_total: usize,
}

#[derive(Serialize)]
struct DiffHunk {
    old_start: usize,
    new_start: usize,
    lines: Vec<DiffLine>,
}

#[derive(Serialize)]
struct DiffLine {
    /// "equal", "insert", or "delete"
    tag: &'static str,
    content: String,
}

#[derive(Embed)]
#[folder = "frontend/"]
struct FrontendAssets;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

type ApiError = (StatusCode, Json<MessageResponse>);

fn api_err(status: StatusCode, msg: impl Into<String>) -> ApiError {
    (
        status,
        Json(MessageResponse {
            message: msg.into(),
        }),
    )
}

fn not_checked_out() -> ApiError {
    api_err(
        StatusCode::BAD_REQUEST,
        "No directory checked out. Use 'ftm checkout <dir>' first.",
    )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health(State(state): State<SharedState>) -> impl IntoResponse {
    let guard = state.ctx.read().await;
    let watch_dir = guard
        .as_ref()
        .map(|c| c.watch_dir.to_string_lossy().to_string());
    Json(HealthResponse {
        status: "ok".into(),
        pid: std::process::id(),
        watch_dir,
    })
}

async fn checkout(
    State(state): State<SharedState>,
    Json(req): Json<CheckoutRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let directory = PathBuf::from(&req.directory);
    if !directory.is_absolute() {
        return Err(api_err(
            StatusCode::BAD_REQUEST,
            "Directory must be an absolute path",
        ));
    }
    if !directory.exists() {
        return Err(api_err(StatusCode::BAD_REQUEST, "Directory does not exist"));
    }

    // Check if already checked out
    {
        let guard = state.ctx.read().await;
        if guard.is_some() {
            return Err(api_err(
                StatusCode::CONFLICT,
                "Already watching a directory. Restart server to switch.",
            ));
        }
    }

    // Initialize .ftm if needed.
    // Check config.yaml (not .ftm/ dir) because --log-dir may have already
    // created .ftm/logs/ before checkout runs.
    let ftm_dir = directory.join(".ftm");
    let config_path = ftm_dir.join("config.yaml");
    if !config_path.exists() {
        std::fs::create_dir_all(&ftm_dir)
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let config = Config::default();
        config
            .save(&config_path)
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let index = crate::types::Index::default();
        let index_content = serde_json::to_string_pretty(&index)
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        std::fs::write(ftm_dir.join("index.json"), index_content)
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        info!("Initialized .ftm in {}", directory.display());
    }

    let config = Config::load(&ftm_dir.join("config.yaml"))
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Wrap config in Arc<StdRwLock> so all components share the same instance.
    let shared_config: SharedConfig = Arc::new(StdRwLock::new(config));

    // Start watcher in background thread
    let watch_dir = directory.clone();
    let max_history = shared_config.read().unwrap().settings.max_history;
    let watcher_storage = Storage::new(ftm_dir.clone(), max_history);
    let watcher = FileWatcher::new(watch_dir.clone(), shared_config.clone(), watcher_storage);
    watcher.watch_background();

    info!("Watching directory: {}", watch_dir.display());

    // Spawn .ftm directory watchdog — auto-shutdown when .ftm is deleted
    {
        let ftm_dir = ftm_dir.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            interval.tick().await; // skip immediate first tick
            loop {
                interval.tick().await;
                if !ftm_dir.exists() {
                    warn!(
                        ".ftm directory deleted ({}), shutting down server",
                        ftm_dir.display()
                    );
                    state.shutdown.notify_one();
                    break;
                }
            }
        });
    }

    // Spawn periodic scanner — always started; dynamically reads scan_interval
    // from shared config so changes via `config set` take effect immediately.
    {
        let scan_watch_dir = directory.clone();
        let scan_config = shared_config.clone();
        let scan_ftm_dir = ftm_dir.clone();
        tokio::spawn(async move {
            loop {
                let (scan_interval, cfg_snapshot, max_history) = {
                    let cfg = scan_config.read().unwrap();
                    (
                        cfg.settings.scan_interval,
                        cfg.clone(),
                        cfg.settings.max_history,
                    )
                };

                if scan_interval == 0 {
                    // Scanning disabled; re-check config periodically
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    continue;
                }

                tokio::time::sleep(Duration::from_secs(scan_interval)).await;

                if !scan_ftm_dir.exists() {
                    break;
                }

                let wd = scan_watch_dir.clone();
                let cfg = cfg_snapshot;
                let fd = scan_ftm_dir.clone();
                match tokio::task::spawn_blocking(move || {
                    let storage = Storage::new(fd, max_history);
                    Scanner::new(wd, cfg, storage).scan()
                })
                .await
                {
                    Ok(Ok(r)) => {
                        info!(
                            "Periodic scan: {} created, {} modified, {} deleted, {} unchanged",
                            r.created, r.modified, r.deleted, r.unchanged
                        );
                    }
                    Ok(Err(e)) => {
                        warn!("Periodic scan error: {}", e);
                    }
                    Err(e) => {
                        warn!("Periodic scan task panic: {}", e);
                    }
                }
            }
        });
        info!("Periodic scanner started");
    }

    // Store context
    {
        let mut guard = state.ctx.write().await;
        *guard = Some(WatchContext {
            watch_dir: directory.clone(),
            config: shared_config,
        });
    }

    Ok(Json(MessageResponse {
        message: format!("Checked out and watching: {}", directory.display()),
    }))
}

async fn files(State(state): State<SharedState>) -> Result<Json<Vec<FileTreeNode>>, ApiError> {
    let (storage, _) = state.storage().await.ok_or_else(not_checked_out)?;
    let tree = storage
        .list_files_tree()
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(tree))
}

async fn history(
    State(state): State<SharedState>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<Vec<HistoryEntry>>, ApiError> {
    let (storage, _) = state.storage().await.ok_or_else(not_checked_out)?;
    let entries = storage
        .list_history(&q.file)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(entries))
}

async fn restore(
    State(state): State<SharedState>,
    Json(req): Json<RestoreRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let (storage, watch_dir) = state.storage().await.ok_or_else(not_checked_out)?;
    storage
        .restore(&req.file, &req.checksum, &watch_dir)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(MessageResponse {
        message: format!(
            "Restored '{}' to checksum '{}'",
            req.file,
            &req.checksum[..8.min(req.checksum.len())]
        ),
    }))
}

async fn snapshot_handler(
    State(state): State<SharedState>,
    Query(q): Query<SnapshotQuery>,
) -> Result<Response, ApiError> {
    let (storage, _) = state.storage().await.ok_or_else(not_checked_out)?;
    let content = storage
        .read_snapshot(&q.checksum)
        .map_err(|e| api_err(StatusCode::NOT_FOUND, e.to_string()))?;
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(content))
        .unwrap())
}

async fn diff_handler(
    State(state): State<SharedState>,
    Query(q): Query<DiffQuery>,
) -> Result<Json<DiffResponse>, ApiError> {
    let (storage, _) = state.storage().await.ok_or_else(not_checked_out)?;

    let old_text = if let Some(ref from) = q.from {
        if from.is_empty() {
            String::new()
        } else {
            let bytes = storage
                .read_snapshot(from)
                .map_err(|e| api_err(StatusCode::NOT_FOUND, e.to_string()))?;
            String::from_utf8_lossy(&bytes).into_owned()
        }
    } else {
        String::new()
    };

    let new_bytes = storage
        .read_snapshot(&q.to)
        .map_err(|e| api_err(StatusCode::NOT_FOUND, e.to_string()))?;
    let new_text = String::from_utf8_lossy(&new_bytes).into_owned();

    let diff = similar::TextDiff::from_lines(&old_text, &new_text);

    let old_total = old_text.lines().count();
    let new_total = new_text.lines().count();

    // Build hunks with context collapsing: show up to CONTEXT_LINES around
    // each change and collapse longer equal runs.
    const CONTEXT_LINES: usize = 3;

    let all_changes: Vec<_> = diff.iter_all_changes().collect();

    // Assign line numbers to every change entry.
    struct Numbered {
        tag: similar::ChangeTag,
        value: String,
        old_line: usize,
        new_line: usize,
    }

    let mut numbered: Vec<Numbered> = Vec::with_capacity(all_changes.len());
    let mut old_line: usize = 1;
    let mut new_line: usize = 1;
    for c in &all_changes {
        let ol = old_line;
        let nl = new_line;
        match c.tag() {
            similar::ChangeTag::Equal => {
                old_line += 1;
                new_line += 1;
            }
            similar::ChangeTag::Delete => {
                old_line += 1;
            }
            similar::ChangeTag::Insert => {
                new_line += 1;
            }
        }
        numbered.push(Numbered {
            tag: c.tag(),
            value: c.value().to_string(),
            old_line: ol,
            new_line: nl,
        });
    }

    // Mark which indices are "interesting" (within CONTEXT_LINES of a change).
    let len = numbered.len();
    let mut visible = vec![false; len];
    for (i, n) in numbered.iter().enumerate() {
        if n.tag != similar::ChangeTag::Equal {
            let lo = i.saturating_sub(CONTEXT_LINES);
            let hi = (i + CONTEXT_LINES + 1).min(len);
            for v in &mut visible[lo..hi] {
                *v = true;
            }
        }
    }

    // Build hunks by grouping consecutive visible runs.
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut i = 0;
    while i < len {
        if !visible[i] {
            i += 1;
            continue;
        }
        // Start a new hunk
        let mut lines: Vec<DiffLine> = Vec::new();
        let hunk_old_start = numbered[i].old_line;
        let hunk_new_start = numbered[i].new_line;
        while i < len && visible[i] {
            let n = &numbered[i];
            let tag = match n.tag {
                similar::ChangeTag::Equal => "equal",
                similar::ChangeTag::Delete => "delete",
                similar::ChangeTag::Insert => "insert",
            };
            let content = n.value.strip_suffix('\n').unwrap_or(&n.value);
            lines.push(DiffLine {
                tag,
                content: content.to_string(),
            });
            i += 1;
        }
        hunks.push(DiffHunk {
            old_start: hunk_old_start,
            new_start: hunk_new_start,
            lines,
        });
    }

    Ok(Json(DiffResponse {
        hunks,
        old_total,
        new_total,
    }))
}

async fn shutdown_handler(State(state): State<SharedState>) -> Json<MessageResponse> {
    info!("Shutdown requested via API");
    state.shutdown.notify_one();
    Json(MessageResponse {
        message: "Shutting down".into(),
    })
}

async fn scan(State(state): State<SharedState>) -> Result<impl IntoResponse, ApiError> {
    let (storage, watch_dir) = state.storage().await.ok_or_else(not_checked_out)?;
    let config = {
        let guard = state.ctx.read().await;
        let ctx = guard.as_ref().unwrap();
        let cfg = ctx.config.read().unwrap();
        cfg.clone()
    };
    let scanner = Scanner::new(watch_dir, config, storage);
    let result = scanner
        .scan()
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(result))
}

async fn version_handler() -> impl IntoResponse {
    Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn config_get(
    State(state): State<SharedState>,
    Query(q): Query<ConfigQuery>,
) -> Result<Json<ConfigResponse>, ApiError> {
    let guard = state.ctx.read().await;
    let ctx = guard.as_ref().ok_or_else(not_checked_out)?;
    let cfg = ctx.config.read().unwrap();

    let data = if let Some(key) = q.key {
        cfg.get_value(&key)
            .map_err(|e| api_err(StatusCode::BAD_REQUEST, e.to_string()))?
    } else {
        serde_yaml::to_string(&*cfg)
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    Ok(Json(ConfigResponse { data }))
}

async fn config_set(
    State(state): State<SharedState>,
    Json(req): Json<ConfigSetRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let guard = state.ctx.read().await;
    let ctx = guard.as_ref().ok_or_else(not_checked_out)?;

    let mut cfg = ctx.config.write().unwrap();
    cfg.set_value(&req.key, &req.value)
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e.to_string()))?;

    // Persist to config.yaml
    let config_path = ctx.watch_dir.join(".ftm").join("config.yaml");
    cfg.save(&config_path)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let hint = if req.key == "settings.web_port" {
        " (web_port change requires server restart to take effect)"
    } else {
        ""
    };

    Ok(Json(MessageResponse {
        message: format!("Set {} = {}{}", req.key, req.value, hint),
    }))
}

async fn logs_handler(State(state): State<SharedState>) -> Result<Json<LogsResponse>, ApiError> {
    let guard = state.ctx.read().await;
    let ctx = guard.as_ref().ok_or_else(not_checked_out)?;

    let log_dir = ctx.watch_dir.join(".ftm").join("logs");
    let log_dir_str = log_dir.to_string_lossy().to_string();

    if !log_dir.exists() {
        return Ok(Json(LogsResponse {
            log_dir: log_dir_str,
            files: vec![],
        }));
    }

    let mut files: Vec<String> = std::fs::read_dir(&log_dir)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".log") {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    // Sort descending (newest first) — filenames are YYYYMMDD-HHMMSS.log
    files.sort();
    files.reverse();

    Ok(Json(LogsResponse {
        log_dir: log_dir_str,
        files,
    }))
}

// ---------------------------------------------------------------------------
// Server startup
// ---------------------------------------------------------------------------

/// Serve an embedded frontend asset or fall back to index.html.
async fn static_handler(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    // Try exact file first, then fall back to index.html
    let path = if path.is_empty() { "index.html" } else { path };

    match FrontendAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string();
            Response::builder()
                .header(header::CONTENT_TYPE, mime)
                .body(Body::from(file.data.to_vec()))
                .unwrap()
        }
        None => {
            // SPA fallback
            match FrontendAssets::get("index.html") {
                Some(file) => Response::builder()
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .body(Body::from(file.data.to_vec()))
                    .unwrap(),
                None => Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::from("Not Found"))
                    .unwrap(),
            }
        }
    }
}

pub async fn serve(port: u16) -> Result<()> {
    let state = Arc::new(AppState::new());
    let shutdown_state = state.clone();

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/version", get(version_handler))
        .route("/api/checkout", post(checkout))
        .route("/api/files", get(files))
        .route("/api/history", get(history))
        .route("/api/restore", post(restore))
        .route("/api/scan", post(scan))
        .route("/api/config", get(config_get).post(config_set))
        .route("/api/logs", get(logs_handler))
        .route("/api/snapshot", get(snapshot_handler))
        .route("/api/diff", get(diff_handler))
        .route("/api/shutdown", post(shutdown_handler))
        .fallback(static_handler)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .context("Failed to bind server port")?;

    let local_addr = listener.local_addr()?;
    // Print the actual address so tests can parse it when using port 0
    println!("Listening on {}", local_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_state))
        .await?;

    info!("Server stopped");
    Ok(())
}

/// Wait for either an API shutdown request or an OS termination signal.
async fn shutdown_signal(state: SharedState) {
    let api = state.shutdown.notified();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = api => info!("Graceful shutdown triggered via API"),
            _ = sigterm.recv() => info!("Received SIGTERM, shutting down"),
        }
    }

    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = api => info!("Graceful shutdown triggered via API"),
            _ = tokio::signal::ctrl_c() => info!("Received Ctrl-C, shutting down"),
        }
    }
}
