use crate::config::Config;
use crate::scanner::Scanner;
use crate::storage::Storage;
use crate::types::{CleanResult, FileTreeNode, HistoryEntry};
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
use tokio::sync::{Notify, RwLock, Semaphore};
use tokio::time::timeout;
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
    /// Only one diff computation at a time. Permit is held inside spawn_blocking
    /// so that on timeout the abandoned task keeps the permit until it finishes.
    diff_semaphore: Arc<Semaphore>,
}

impl AppState {
    fn new() -> Self {
        Self {
            ctx: RwLock::new(None),
            shutdown: Notify::new(),
            diff_semaphore: Arc::new(Semaphore::new(1)),
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
struct FilesQuery {
    /// When false or absent, files whose last history entry is Delete are excluded.
    include_deleted: Option<bool>,
}

#[derive(Deserialize)]
struct HistoryQuery {
    file: String,
}

#[derive(Deserialize)]
struct ActivityQuery {
    /// ISO 8601 timestamp for the start of the time range (inclusive).
    since: String,
    /// ISO 8601 timestamp for the end of the time range (inclusive). Defaults to now.
    until: Option<String>,
    /// When false or absent, entries for files whose last history entry is Delete are excluded.
    include_deleted: Option<bool>,
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

/// CPU-heavy diff computation. Returns hunks only; old_total/new_total are
/// computed by the caller from line counts. Uses imara-diff (Histogram) for
/// speed and stability.
fn compute_diff_hunks(old_text: String, new_text: String) -> Vec<DiffHunk> {
    const CONTEXT_LINES: u32 = 3;
    use imara_diff::{Algorithm, Diff, InternedInput};

    let input = InternedInput::new(old_text.as_str(), new_text.as_str());
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);

    let line_content = |idx: u32, is_old: bool| -> String {
        let token = if is_old {
            input.before[idx as usize]
        } else {
            input.after[idx as usize]
        };
        let s = &input.interner[token];
        s.strip_suffix('\n').unwrap_or(s).to_string()
    };

    let mut hunks: Vec<DiffHunk> = Vec::new();
    for hunk in diff.hunks() {
        let before_start = hunk.before.start;
        let before_end = hunk.before.end;
        let after_start = hunk.after.start;
        let after_end = hunk.after.end;

        let ctx_old_start = before_start.saturating_sub(CONTEXT_LINES);
        let ctx_new_end = (after_end + CONTEXT_LINES).min(input.after.len() as u32);

        let mut lines: Vec<DiffLine> = Vec::new();

        for i in ctx_old_start..before_start {
            lines.push(DiffLine {
                tag: "equal",
                content: line_content(i, true),
            });
        }
        for i in before_start..before_end {
            lines.push(DiffLine {
                tag: "delete",
                content: line_content(i, true),
            });
        }
        for i in after_start..after_end {
            lines.push(DiffLine {
                tag: "insert",
                content: line_content(i, false),
            });
        }
        for i in after_end..ctx_new_end {
            lines.push(DiffLine {
                tag: "equal",
                content: line_content(i, false),
            });
        }

        let old_start_1based = (ctx_old_start + 1) as usize;
        let new_start_1based = (after_start.saturating_sub(CONTEXT_LINES) + 1) as usize;

        hunks.push(DiffHunk {
            old_start: old_start_1based,
            new_start: new_start_1based,
            lines,
        });
    }
    hunks
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
    let watcher = FileWatcher::new(watch_dir.clone(), shared_config.clone());
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

    // Spawn periodic scanner — always started; reads scan_interval every ~1s so
    // changes via `config set` take effect immediately (no wait for current sleep).
    {
        let scan_watch_dir = directory.clone();
        let scan_config = shared_config.clone();
        let scan_ftm_dir = ftm_dir.clone();
        tokio::spawn(async move {
            let mut last_scan = tokio::time::Instant::now();
            loop {
                let (scan_interval, cfg_snapshot, max_history) = {
                    let cfg = scan_config.read().unwrap();
                    (
                        cfg.settings.scan_interval,
                        cfg.clone(),
                        cfg.settings.max_history,
                    )
                };

                let elapsed = last_scan.elapsed().as_secs();
                if elapsed < scan_interval {
                    let remaining = scan_interval - elapsed;
                    let sleep_secs = std::cmp::min(1, remaining);
                    tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
                    continue;
                }

                if !scan_ftm_dir.exists() {
                    break;
                }

                last_scan = tokio::time::Instant::now();
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

    // Spawn periodic cleaner — runs clean_orphan_snapshots every clean_interval seconds.
    {
        let clean_ftm_dir = ftm_dir.clone();
        let clean_config = shared_config.clone();
        tokio::spawn(async move {
            let mut last_clean = tokio::time::Instant::now();
            loop {
                let (clean_interval, max_history) = {
                    let cfg = clean_config.read().unwrap();
                    (cfg.settings.clean_interval, cfg.settings.max_history)
                };

                let elapsed = last_clean.elapsed().as_secs();
                if elapsed < clean_interval {
                    let remaining = clean_interval - elapsed;
                    let sleep_secs = std::cmp::min(1, remaining);
                    tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
                    continue;
                }

                if !clean_ftm_dir.exists() {
                    break;
                }

                last_clean = tokio::time::Instant::now();
                let fd = clean_ftm_dir.clone();
                match tokio::task::spawn_blocking(move || {
                    let storage = Storage::new(fd, max_history);
                    storage.clean_orphan_snapshots()
                })
                .await
                {
                    Ok(Ok(r)) => {
                        if r.files_removed > 0 || r.bytes_removed > 0 {
                            info!(
                                "Periodic clean: {} files, {} bytes removed",
                                r.files_removed, r.bytes_removed
                            );
                        }
                    }
                    Ok(Err(e)) => {
                        warn!("Periodic clean error: {}", e);
                    }
                    Err(e) => {
                        warn!("Periodic clean task panic: {}", e);
                    }
                }
            }
        });
        info!("Periodic cleaner started");
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

async fn files(
    State(state): State<SharedState>,
    Query(q): Query<FilesQuery>,
) -> Result<Json<Vec<FileTreeNode>>, ApiError> {
    let include_deleted = q.include_deleted.unwrap_or(false);
    let (storage, _) = state.storage().await.ok_or_else(not_checked_out)?;
    let tree = storage
        .list_files_tree(include_deleted)
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

async fn activity(
    State(state): State<SharedState>,
    Query(q): Query<ActivityQuery>,
) -> Result<Json<Vec<HistoryEntry>>, ApiError> {
    let (storage, _) = state.storage().await.ok_or_else(not_checked_out)?;

    let since = chrono::DateTime::parse_from_rfc3339(&q.since)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, format!("Invalid 'since': {}", e)))?;

    let until = if let Some(ref u) = q.until {
        chrono::DateTime::parse_from_rfc3339(u)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .map_err(|e| api_err(StatusCode::BAD_REQUEST, format!("Invalid 'until': {}", e)))?
    } else {
        chrono::Utc::now()
    };

    let include_deleted = q.include_deleted.unwrap_or(false);
    let entries = storage
        .list_activity(since, until, include_deleted)
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

    let old_total = old_text.lines().count();
    let new_total = new_text.lines().count();

    // Serialize diff: only one at a time. Permit is held inside the blocking task
    // so that on timeout the abandoned task keeps it until done; no new diff
    // can start until that task finishes, preventing runaway CPU from many tasks.
    let permit = state
        .diff_semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            api_err(
                StatusCode::SERVICE_UNAVAILABLE,
                "Another diff is in progress. Try again in a moment.",
            )
        })?;

    let hunks = match timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || {
            let result = compute_diff_hunks(old_text, new_text);
            drop(permit);
            result
        }),
    )
    .await
    {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => return Err(api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
        Err(_) => {
            return Err(api_err(
                StatusCode::REQUEST_TIMEOUT,
                "Diff computation timed out (1s limit). File may be too large.",
            ))
        }
    };

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

async fn clean_handler(State(state): State<SharedState>) -> Result<Json<CleanResult>, ApiError> {
    let (storage, _) = state.storage().await.ok_or_else(not_checked_out)?;
    let result = tokio::task::spawn_blocking(move || storage.clean_orphan_snapshots())
        .await
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
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
        .route("/api/activity", get(activity))
        .route("/api/restore", post(restore))
        .route("/api/scan", post(scan))
        .route("/api/clean", post(clean_handler))
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
