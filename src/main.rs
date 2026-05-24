mod api;
mod cloudflared;
mod config;
mod db;
mod env_writer;
mod media;
mod settings_registry;
mod telegram;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<()> {
    let env_path = dotenvy::dotenv().unwrap_or_else(|_| Path::new(".env").to_path_buf());
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let db_path = Path::new("streamer.db").to_path_buf();
    let db_pool = db::init_db_pool(&db_path).context("initialising database")?;
    let db_conn = db_pool
        .get()
        .context("getting initial database connection")?;
    let cfg = config::Config::load(&db_conn).context("loading config")?;
    let watch_settings_path = Path::new("watch_settings.json").to_path_buf();
    let watch_settings = api::load_watch_settings(&db_conn, &watch_settings_path)
        .context("loading watch settings")?;
    drop(db_conn);
    let uploads_dir = Path::new("uploads").to_path_buf();
    let processing_dir = Path::new("processing").to_path_buf();
    let cache_dir = Path::new(&cfg.cache_dir).to_path_buf();
    tokio::fs::create_dir_all(&uploads_dir)
        .await
        .context("creating uploads directory")?;
    api::cleanup_orphaned_uploads(&uploads_dir).await;
    tokio::fs::create_dir_all(&processing_dir)
        .await
        .context("creating processing directory")?;
    if cfg.disk_cache_enabled {
        prepare_cache_dir(&cache_dir, &uploads_dir, &processing_dir)
            .await
            .context("preparing cache directory")?;
    }

    tracing::info!(
        bots = cfg.bots.len(),
        max_concurrent_jobs = cfg.max_concurrent_jobs,
        upload_chunk_size = cfg.upload_chunk_size,
        "config loaded"
    );
    log_startup_warnings(&cfg);

    let (ffmpeg_available, ffprobe_available, selected_encoder) = probe_ffmpeg_tools(&cfg).await;

    let last_bot_index = db_pool
        .get()
        .ok()
        .and_then(|c| db::get_last_bot_index(&c).ok())
        .unwrap_or(0);

    let addr = SocketAddr::new(cfg.host, cfg.port);
    let cache_budget = (cfg.segment_cache_size_mb as u64) * 1024 * 1024;
    let (job_queue, job_receiver) = mpsc::channel(100);
    let shutdown_token = CancellationToken::new();
    let state = Arc::new(api::AppState {
        db: RwLock::new(db_pool),
        db_path,
        env_path,
        config: RwLock::new(Arc::new(cfg)),
        started_at: Instant::now(),
        bot_health: RwLock::new(Vec::new()),
        cloudflared: cloudflared::SharedCloudflaredStatus::default(),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .context("creating HTTP client")?,
        telegram: telegram::TelegramRuntime::new(),
        telegram_base_url: telegram::DEFAULT_API_BASE.to_string(),
        uploads_dir,
        processing_dir,
        watch_settings: RwLock::new(watch_settings),
        watch_seen: Mutex::new(std::collections::HashMap::new()),
        pending_uploads: Mutex::new(std::collections::HashMap::new()),
        upload_rate_limits: Mutex::new(std::collections::HashMap::new()),
        db_sync_lock: Mutex::new(()),
        jobs: Mutex::new(std::collections::HashMap::new()),
        played_segments: Mutex::new(std::collections::HashMap::new()),
        job_queue,
        cache: Arc::new(api::SegmentCache::new(cache_budget)),
        ffmpeg_available,
        ffprobe_available,
        selected_encoder: RwLock::new(selected_encoder),
        last_bot_index: std::sync::atomic::AtomicI64::new(last_bot_index),
        shutdown_token: shutdown_token.clone(),
        ingest_download_semaphore: Arc::new(tokio::sync::Semaphore::new(5)),
    });
    api::db_transfer::bootstrap_db_sync_if_configured(state.clone()).await;
    api::start_background_tasks(state.clone(), job_receiver);
    cloudflared::start_manager(state.clone());

    crate::api::jobs::processing::recover_stuck_processing_jobs(&state).await;
    crate::api::jobs::processing::clean_orphaned_processing_dirs(&state).await;

    let app = api::router(state.clone());

    tracing::info!(%addr, "thls listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let server_shutdown = shutdown_token.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutdown signal received");
        server_shutdown.cancel();
    });
    let graceful = shutdown_token.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { graceful.cancelled().await })
        .await?;

    tracing::info!("http server stopped, draining in-flight jobs");
    let drain_deadline = Duration::from_secs(30);
    let drain_start = Instant::now();
    loop {
        let active = {
            let jobs = state.jobs.lock().await;
            jobs.values().filter(|j| !j.status.is_terminal()).count()
        };
        if active == 0 {
            tracing::info!("all jobs drained");
            break;
        }
        if drain_start.elapsed() > drain_deadline {
            tracing::warn!(
                active,
                "drain deadline exceeded, marking remaining jobs as error"
            );
            let mut jobs = state.jobs.lock().await;
            for (_, job) in jobs.iter_mut() {
                if !job.status.is_terminal() {
                    job.cancel_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    job.status = api::jobs::JobStatus::Error;
                    job.error = Some("shutdown_timeout".into());
                    job.finished_at = Some(Instant::now());
                }
            }
            if let Ok(conn) = state.db_conn().await {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = db::mark_non_terminal_jobs_failed(&conn, "shutdown_timeout");
                })
                .await;
            }
            break;
        }
        tracing::info!(
            active,
            elapsed_secs = drain_start.elapsed().as_secs(),
            "waiting for in-flight jobs"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Ok(())
}

async fn prepare_cache_dir(
    cache_dir: &Path,
    uploads_dir: &Path,
    processing_dir: &Path,
) -> Result<()> {
    let existed = cache_dir.exists();
    tokio::fs::create_dir_all(cache_dir)
        .await
        .context("creating cache directory")?;
    if !existed {
        return Ok(());
    }
    if !cache_cleanup_allowed(cache_dir, uploads_dir, processing_dir) {
        tracing::warn!(
            cache_dir = %cache_dir.display(),
            "skipping startup cache cleanup for non-dedicated or unsafe cache path"
        );
        return Ok(());
    }

    let mut entries = tokio::fs::read_dir(cache_dir)
        .await
        .context("reading cache directory")?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .context("reading cache directory entry")?
    {
        let path = entry.path();
        let meta = tokio::fs::symlink_metadata(&path)
            .await
            .with_context(|| format!("reading cache entry metadata for {}", path.display()))?;
        let file_type = meta.file_type();
        if file_type.is_dir() && !file_type.is_symlink() {
            tokio::fs::remove_dir_all(&path)
                .await
                .with_context(|| format!("removing cache subdirectory {}", path.display()))?;
        } else {
            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("removing cache file {}", path.display()))?;
        }
    }
    Ok(())
}

fn cache_cleanup_allowed(cache_dir: &Path, uploads_dir: &Path, processing_dir: &Path) -> bool {
    let Ok(cache_abs) = cache_dir.canonicalize() else {
        return false;
    };
    if cache_abs.parent().is_none() {
        return false;
    }
    let Ok(project_root) = std::env::current_dir().and_then(|p| p.canonicalize()) else {
        return false;
    };
    if cache_abs == project_root {
        return false;
    }
    for reserved in [uploads_dir, processing_dir, Path::new("/tmp")] {
        if let Ok(reserved_abs) = reserved.canonicalize() {
            if cache_abs == reserved_abs {
                return false;
            }
        }
    }
    cache_abs
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase().contains("cache"))
        .unwrap_or(false)
}

async fn probe_ffmpeg_tools(cfg: &config::Config) -> (bool, bool, media::SelectedEncoder) {
    let ffmpeg_ok = tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    let ffprobe_ok = tokio::process::Command::new("ffprobe")
        .arg("-version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    let encoder = media::select_encoder(cfg).await;
    let encoder_info = encoder.name.clone();

    if !ffmpeg_ok {
        tracing::error!("ffmpeg binary not found or not working; uploads will be rejected");
    }
    if !ffprobe_ok {
        tracing::error!("ffprobe binary not found or not working; uploads will be rejected");
    }
    tracing::info!(
        ffmpeg_available = ffmpeg_ok,
        ffprobe_available = ffprobe_ok,
        encoder = %encoder_info,
        "FFmpeg tool probe complete"
    );

    (ffmpeg_ok, ffprobe_ok, encoder)
}

fn log_startup_warnings(cfg: &config::Config) {
    if cfg.job_retention_days == 0 {
        tracing::warn!("JOB_RETENTION_DAYS=0; completed jobs will grow until manually removed");
    }
    if cfg.segment_cache_size_mb == 0 {
        tracing::warn!("SEGMENT_CACHE_SIZE_MB=0; segment cache is effectively unbounded/disabled");
    }
    if !cfg.disk_cache_enabled {
        tracing::info!("DISK_CACHE_ENABLED=false; segment cache is memory-only");
    }
    if cfg.db_auto_merge_interval_minutes == 0 {
        tracing::warn!("DB_AUTO_MERGE_INTERVAL_MINUTES=0; DB auto-merge is disabled");
    }
    if cfg.cloudflared_enabled && cfg.cloudflared_config.trim().is_empty() {
        tracing::warn!("CLOUDFLARED_ENABLED=true but CLOUDFLARED_CONFIG is empty");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("thls-{name}-{unique}"))
    }

    #[tokio::test]
    async fn prepare_cache_dir_removes_only_children_for_dedicated_cache_path() {
        let root = temp_root("cache-cleanup");
        let cache = root.join("cache");
        let uploads = root.join("uploads");
        let processing = root.join("processing");
        tokio::fs::create_dir_all(cache.join("nested"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(&uploads).await.unwrap();
        tokio::fs::create_dir_all(&processing).await.unwrap();
        tokio::fs::write(cache.join("nested/file.bin"), b"cached")
            .await
            .unwrap();
        tokio::fs::write(cache.join("entry.bin"), b"cached")
            .await
            .unwrap();

        prepare_cache_dir(&cache, &uploads, &processing)
            .await
            .unwrap();

        assert!(cache.exists());
        assert!(!cache.join("nested").exists());
        assert!(!cache.join("entry.bin").exists());
        assert!(uploads.exists());
        assert!(processing.exists());

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn prepare_cache_dir_skips_reserved_paths() {
        let root = temp_root("cache-reserved");
        let uploads = root.join("uploads");
        let processing = root.join("processing");
        tokio::fs::create_dir_all(&uploads).await.unwrap();
        tokio::fs::create_dir_all(&processing).await.unwrap();
        tokio::fs::write(uploads.join("keep.bin"), b"upload")
            .await
            .unwrap();

        prepare_cache_dir(&uploads, &uploads, &processing)
            .await
            .unwrap();

        assert!(uploads.join("keep.bin").exists());

        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
