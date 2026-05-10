mod bots_settings;
mod db_transfer;
mod frontend;
mod jobs;
mod playback;
mod playlists;
mod uploads;
mod watch_folder;

pub(crate) use jobs::start_background_tasks;
pub(crate) use watch_folder::load_watch_settings;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::Connection;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex, RwLock};
use tower_http::services::ServeDir;

use bots_settings::{
    handle_add_bot, handle_bot_health, handle_delete_bot, handle_get_bots, handle_get_settings,
    handle_post_settings, handle_reset_settings, BotHealthResult,
};
use db_transfer::{handle_database_load, handle_db_backup, handle_db_export, handle_db_import};
use jobs::{
    handle_cancel_job, handle_delete_job, handle_download_original, handle_get_job,
    handle_job_status, handle_list_jobs, handle_patch_job, handle_reprocess_job, queue_metrics,
    JobRequest, JobState,
};
use uploads::{
    handle_upload_chunk, handle_upload_finalize, handle_upload_init, handle_upload_status,
    PendingUpload,
};
use watch_folder::{
    handle_get_watch_settings, handle_post_watch_settings, WatchFileState, WatchSettings,
};

use crate::config::Config;
use crate::db;
use crate::telegram::TelegramRuntime;

pub use playback::SegmentCache;

pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub db_path: PathBuf,
    pub config: RwLock<Arc<Config>>,
    pub started_at: Instant,
    pub bot_health: RwLock<Vec<BotHealthResult>>,
    pub http: reqwest::Client,
    pub telegram: TelegramRuntime,
    pub telegram_base_url: String,
    pub uploads_dir: PathBuf,
    pub processing_dir: PathBuf,
    pub watch_settings_path: PathBuf,
    pub watch_settings: RwLock<WatchSettings>,
    pub watch_seen: Mutex<HashMap<PathBuf, WatchFileState>>,
    pub pending_uploads: Mutex<HashMap<String, PendingUpload>>,
    pub upload_rate_limits: Mutex<HashMap<std::net::IpAddr, VecDeque<Instant>>>,
    pub jobs: Mutex<HashMap<String, JobState>>,
    pub job_queue: mpsc::Sender<JobRequest>,
    pub cache: Arc<SegmentCache>,
}

pub(super) fn valid_job_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(handle_health))
        .route("/", get(frontend::handle_home))
        .route("/films", get(frontend::handle_films))
        .route("/series", get(frontend::handle_series_root))
        .route("/series/*path", get(frontend::handle_series_path))
        .route("/anime-films", get(frontend::handle_anime_films))
        .route("/anime-tv", get(frontend::handle_anime_tv_root))
        .route("/anime-tv/*path", get(frontend::handle_anime_tv_path))
        .route("/upload", get(frontend::handle_upload_page))
        .route("/settings", get(frontend::handle_settings_page))
        .route("/watch/:job_id", get(frontend::handle_watch_page))
        .nest_service("/static", ServeDir::new("static"))
        .route("/api/jobs", get(handle_list_jobs))
        .route(
            "/api/jobs/:job_id",
            get(handle_get_job)
                .patch(handle_patch_job)
                .delete(handle_delete_job),
        )
        .route(
            "/api/jobs/:job_id/download-original",
            get(handle_download_original),
        )
        .route("/api/jobs/:job_id/reprocess", post(handle_reprocess_job))
        .route("/api/upload/init", post(handle_upload_init))
        .route("/api/upload/chunk", post(handle_upload_chunk))
        .route("/api/upload/finalize", post(handle_upload_finalize))
        .route("/api/upload/status/:upload_id", get(handle_upload_status))
        .route("/api/status/:job_id", get(handle_job_status))
        .route("/api/cancel/:job_id", post(handle_cancel_job))
        .route(
            "/api/settings",
            get(handle_get_settings).post(handle_post_settings),
        )
        .route("/api/settings/reset", post(handle_reset_settings))
        .route("/api/bots", get(handle_get_bots))
        .route("/api/bots/health", post(handle_bot_health))
        .route("/api/bots/add", post(handle_add_bot))
        .route("/api/bots/:bot_id", delete(handle_delete_bot))
        .route(
            "/api/watch-settings",
            get(handle_get_watch_settings).post(handle_post_watch_settings),
        )
        .route("/api/db/export", post(handle_db_export))
        .route("/api/db/backup", post(handle_db_backup))
        .route("/api/db/import", post(handle_db_import))
        .route("/api/database/load", post(handle_database_load))
        .route("/api/metrics", get(handle_metrics))
        .route("/hls/:job_id/master.m3u8", get(playlists::handle_master))
        .route(
            "/hls/:job_id/video.m3u8",
            get(playlists::handle_legacy_video),
        )
        .route(
            "/hls/:job_id/:playlist",
            get(playlists::handle_media_playlist),
        )
        .route("/segment/:job_id/*key", get(playback::handle_segment))
        .route("/thumbnail/:job_id", get(playlists::handle_thumbnail))
        .layer(DefaultBodyLimit::disable())
        .with_state(state)
}

async fn handle_health(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.config.read().await.clone();
    let health = state.bot_health.read().await.clone();
    let (schema_revision, db_ok) = {
        let conn = state.db.lock().await;
        match db::current_schema_revision(&conn) {
            Ok(rev) => (rev, true),
            Err(e) => {
                tracing::warn!(error = %e, "health db revision check failed");
                (0, false)
            }
        }
    };
    let bots_configured = cfg.bots.len();
    let healthy = health.iter().filter(|r| r.ok).count();
    let cloudflared_running = false;
    let queue = queue_metrics(&state).await;
    let degraded = !db_ok
        || bots_configured == 0
        || (!health.is_empty() && healthy == 0)
        || (cfg.cloudflared_enabled && !cloudflared_running);

    Json(json!({
        "status": if degraded { "degraded" } else { "ok" },
        "uptime_seconds": state.started_at.elapsed().as_secs(),
        "db": {
            "schema_revision": schema_revision,
            "open_connections": 1,
            "ok": db_ok,
        },
        "bots": {
            "configured": bots_configured,
            "healthy": healthy,
            "last_probe": health,
        },
        "queue": queue,
        "cache": cache_metrics(&state.cache, &cfg),
        "cloudflared": {
            "enabled": cfg.cloudflared_enabled,
            "running": cloudflared_running,
            "url": Value::Null,
        },
        "metrics": metrics_json(&state, &cfg, queue).await,
    }))
}

async fn handle_metrics(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.config.read().await.clone();
    let queue = queue_metrics(&state).await;
    Json(metrics_json(&state, &cfg, queue).await)
}

fn cache_metrics(cache: &SegmentCache, cfg: &Config) -> Value {
    let snap = cache.snapshot();
    json!({
        "size_bytes": snap.size_bytes,
        "size_mb": snap.size_bytes as f64 / (1024.0 * 1024.0),
        "max_mb": cfg.segment_cache_size_mb,
        "entries": snap.entries,
        "hits": snap.hits,
        "misses": snap.misses,
        "evictions": snap.evictions,
    })
}

async fn metrics_json(state: &AppState, cfg: &Config, queue: Value) -> Value {
    let telegram = state.telegram.metrics_snapshot().await;
    json!({
        "cache": cache_metrics(&state.cache, cfg),
        "telegram": telegram,
        "queue": queue,
    })
}

fn api_error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(json!({
            "error": code,
            "message": message.into(),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests;
