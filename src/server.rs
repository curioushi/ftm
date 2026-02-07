use crate::config::Config;
use crate::scanner::Scanner;
use crate::storage::Storage;
use crate::types::HistoryEntry;
use crate::watcher::FileWatcher;
use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};
use tracing::info;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct WatchContext {
    watch_dir: PathBuf,
    config: Config,
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
            let storage = Storage::new(ftm_dir, c.config.settings.max_history);
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

#[derive(Serialize)]
struct FileEntry {
    path: String,
    count: usize,
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
    // created .ftm/log/ before checkout runs.
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

    // Start watcher in background thread
    let watch_dir = directory.clone();
    let watcher_config = config.clone();
    let watcher_storage = Storage::new(ftm_dir, config.settings.max_history);
    let watcher = FileWatcher::new(watch_dir.clone(), watcher_config, watcher_storage);
    watcher.watch_background();

    info!("Watching directory: {}", watch_dir.display());

    // Store context
    {
        let mut guard = state.ctx.write().await;
        *guard = Some(WatchContext {
            watch_dir: directory.clone(),
            config,
        });
    }

    Ok(Json(MessageResponse {
        message: format!("Checked out and watching: {}", directory.display()),
    }))
}

async fn files(State(state): State<SharedState>) -> Result<Json<Vec<FileEntry>>, ApiError> {
    let (storage, _) = state.storage().await.ok_or_else(not_checked_out)?;
    let file_list = storage
        .list_files()
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let entries: Vec<FileEntry> = file_list
        .into_iter()
        .map(|(path, count)| FileEntry { path, count })
        .collect();
    Ok(Json(entries))
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
        guard.as_ref().unwrap().config.clone()
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

    let data = if let Some(key) = q.key {
        ctx.config
            .get_value(&key)
            .map_err(|e| api_err(StatusCode::BAD_REQUEST, e.to_string()))?
    } else {
        serde_yaml::to_string(&ctx.config)
            .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    Ok(Json(ConfigResponse { data }))
}

async fn config_set(
    State(state): State<SharedState>,
    Json(req): Json<ConfigSetRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let mut guard = state.ctx.write().await;
    let ctx = guard.as_mut().ok_or_else(not_checked_out)?;

    ctx.config
        .set_value(&req.key, &req.value)
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e.to_string()))?;

    // Persist to config.yaml
    let config_path = ctx.watch_dir.join(".ftm").join("config.yaml");
    ctx.config
        .save(&config_path)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(MessageResponse {
        message: format!("Set {} = {}", req.key, req.value),
    }))
}

async fn logs_handler(State(state): State<SharedState>) -> Result<Json<LogsResponse>, ApiError> {
    let guard = state.ctx.read().await;
    let ctx = guard.as_ref().ok_or_else(not_checked_out)?;

    let log_dir = ctx.watch_dir.join(".ftm").join("log");
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
        .route("/api/shutdown", post(shutdown_handler))
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
