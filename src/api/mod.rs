mod bots_settings;
mod db_transfer;
mod frontend;
mod ingest;
pub(crate) mod jobs;
mod playback;
mod playlists;
mod uploads;
mod watch_folder;

pub(crate) use jobs::start_background_tasks;
pub(crate) use watch_folder::load_watch_settings;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex, RwLock};
use tower_http::services::ServeDir;

use bots_settings::{
    handle_add_bot, handle_bot_health, handle_delete_bot, handle_get_bots, handle_get_settings,
    handle_post_settings, handle_reset_settings, BotHealthResult,
};
use db_transfer::{handle_database_load, handle_db_backup, handle_db_export, handle_db_import};
use ingest::handle_url_ingest;
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

use crate::cloudflared::SharedCloudflaredStatus;
use crate::config::Config;
use crate::db;
use crate::media;
use crate::telegram::TelegramRuntime;

pub use playback::SegmentCache;

pub struct AppState {
    pub db: RwLock<db::DbPool>,
    pub db_path: PathBuf,
    pub env_path: PathBuf,
    pub config: RwLock<Arc<Config>>,
    pub started_at: Instant,
    pub bot_health: RwLock<Vec<BotHealthResult>>,
    pub cloudflared: SharedCloudflaredStatus,
    pub http: reqwest::Client,
    pub telegram: TelegramRuntime,
    pub telegram_base_url: String,
    pub uploads_dir: PathBuf,
    pub processing_dir: PathBuf,
    pub watch_settings: RwLock<WatchSettings>,
    pub watch_seen: Mutex<HashMap<PathBuf, WatchFileState>>,
    pub pending_uploads: Mutex<HashMap<String, PendingUpload>>,
    pub upload_rate_limits: Mutex<HashMap<std::net::IpAddr, VecDeque<Instant>>>,
    pub jobs: Mutex<HashMap<String, JobState>>,
    pub played_segments: Mutex<HashMap<String, (HashSet<String>, Instant)>>,
    pub job_queue: mpsc::Sender<JobRequest>,
    pub cache: Arc<SegmentCache>,
    pub ffmpeg_available: bool,
    pub ffprobe_available: bool,
    pub selected_encoder: RwLock<media::SelectedEncoder>,
    pub last_bot_index: std::sync::atomic::AtomicI64,
}

impl AppState {
    pub(crate) async fn db_pool(&self) -> db::DbPool {
        self.db.read().await.clone()
    }

    pub(crate) async fn db_conn(&self) -> anyhow::Result<db::DbConn> {
        self.db_pool()
            .await
            .get()
            .context("getting sqlite connection from pool")
    }
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
        .route("/api/ingest/url", post(handle_url_ingest))
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
    let encoder = state.selected_encoder.read().await.clone();
    let health = state.bot_health.read().await.clone();
    let pool = state.db_pool().await;
    let pool_state = pool.state();
    let (schema_revision, db_ok) = {
        match pool
            .get()
            .context("getting sqlite connection from pool")
            .and_then(|conn| db::current_schema_revision(&conn))
        {
            Ok(rev) => (rev, true),
            Err(e) => {
                tracing::warn!(error = %e, "health db revision check failed");
                (0, false)
            }
        }
    };
    let bots_configured = cfg.bots.len();
    let healthy = health.iter().filter(|r| r.ok).count();
    let cloudflared = state.cloudflared.read().await.clone();
    let queue = queue_metrics(&state).await;
    let disk_free_uploads = uploads::free_space_bytes(&state.uploads_dir).ok();
    let disk_free_processing = uploads::free_space_bytes(&state.processing_dir).ok();
    let degraded = !db_ok
        || bots_configured == 0
        || (!health.is_empty() && healthy == 0)
        || !state.ffmpeg_available
        || !state.ffprobe_available;

    Json(json!({
        "status": if degraded { "degraded" } else { "ok" },
        "uptime_seconds": state.started_at.elapsed().as_secs(),
        "db": {
            "schema_revision": schema_revision,
            "open_connections": pool_state.connections,
            "idle_connections": pool_state.idle_connections,
            "max_connections": pool.max_size(),
            "ok": db_ok,
        },
        "disk": {
            "uploads_free_bytes": disk_free_uploads,
            "processing_free_bytes": disk_free_processing,
        },
        "ffmpeg": {
            "ffmpeg_available": state.ffmpeg_available,
            "ffprobe_available": state.ffprobe_available,
            "encoder": encoder.name,
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
            "running": cloudflared.running,
            "uptime_seconds": cloudflared.uptime_seconds(),
            "url": cloudflared.url,
            "last_error": cloudflared.last_error,
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
        "virtual_abr": virtual_abr_metrics(cfg),
    })
}

fn virtual_abr_metrics(cfg: &Config) -> Value {
    let configured_encoder = if !cfg.enable_hw_accel || cfg.preferred_encoder == "cpu" {
        "libx264"
    } else {
        match cfg.preferred_encoder.as_str() {
            "vaapi" => "h264_vaapi",
            "nvenc" => "h264_nvenc",
            "qsv" => "h264_qsv",
            _ => "libx264",
        }
    };
    json!({
        "enabled": cfg.virtual_abr_tiers && !cfg.abr_enabled,
        "configured_encoder": configured_encoder,
        "preferred_encoder": cfg.preferred_encoder.as_str(),
        "hardware_accel_enabled": cfg.enable_hw_accel,
        "vaapi_device": if cfg.vaapi_device.is_empty() { Value::Null } else { json!(cfg.vaapi_device.as_str()) },
        "cpu_fallback_encoder": media::cpu_encoder().name,
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

fn db_unavailable(error: impl std::fmt::Display) -> Response {
    tracing::warn!(error = %error, "database connection unavailable");
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "db_unavailable",
        error.to_string(),
    )
}

#[cfg(test)]
mod tests;
