mod api;
mod config;
mod db;
mod media;
mod settings_registry;
mod telegram;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::{mpsc, Mutex, RwLock};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let db_path = Path::new("streamer.db").to_path_buf();
    let db_conn = db::init_db(&db_path).context("initialising database")?;
    let cfg = config::Config::load(&db_conn).context("loading config")?;
    let watch_settings_path = Path::new("watch_settings.json").to_path_buf();
    let watch_settings =
        api::load_watch_settings(&watch_settings_path).context("loading watch settings")?;
    let uploads_dir = Path::new("uploads").to_path_buf();
    let processing_dir = Path::new("processing").to_path_buf();
    tokio::fs::create_dir_all(&uploads_dir)
        .await
        .context("creating uploads directory")?;
    tokio::fs::create_dir_all(&processing_dir)
        .await
        .context("creating processing directory")?;

    tracing::info!(
        bots = cfg.bots.len(),
        max_concurrent_jobs = cfg.max_concurrent_jobs,
        upload_chunk_size = cfg.upload_chunk_size,
        "config loaded"
    );
    log_startup_warnings(&cfg);

    let addr = SocketAddr::new(cfg.host, cfg.port);
    let cache_budget = (cfg.segment_cache_size_mb as u64) * 1024 * 1024;
    let (job_queue, job_receiver) = mpsc::channel(100);
    let state = Arc::new(api::AppState {
        db: Arc::new(Mutex::new(db_conn)),
        db_path,
        config: RwLock::new(Arc::new(cfg)),
        started_at: Instant::now(),
        bot_health: RwLock::new(Vec::new()),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .context("creating HTTP client")?,
        telegram: telegram::TelegramRuntime::new(),
        telegram_base_url: telegram::DEFAULT_API_BASE.to_string(),
        uploads_dir,
        processing_dir,
        watch_settings_path,
        watch_settings: RwLock::new(watch_settings),
        watch_seen: Mutex::new(std::collections::HashMap::new()),
        pending_uploads: Mutex::new(std::collections::HashMap::new()),
        upload_rate_limits: Mutex::new(std::collections::HashMap::new()),
        jobs: Mutex::new(std::collections::HashMap::new()),
        job_queue,
        cache: Arc::new(api::SegmentCache::new(cache_budget)),
    });
    api::start_background_tasks(state.clone(), job_receiver);

    let app = api::router(state);

    tracing::info!(%addr, "thls listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}

fn log_startup_warnings(cfg: &config::Config) {
    if cfg.job_retention_days == 0 {
        tracing::warn!("JOB_RETENTION_DAYS=0; completed jobs will grow until manually removed");
    }
    if cfg.segment_cache_size_mb == 0 {
        tracing::warn!("SEGMENT_CACHE_SIZE_MB=0; segment cache is effectively unbounded/disabled");
    }
    if cfg.db_auto_merge_interval_minutes == 0 {
        tracing::warn!("DB_AUTO_MERGE_INTERVAL_MINUTES=0; DB auto-merge is disabled");
    }
    if cfg.cloudflared_enabled {
        tracing::warn!(
            "CLOUDFLARED_ENABLED=true but Cloudflared manager is not implemented in this phase"
        );
    }
}
