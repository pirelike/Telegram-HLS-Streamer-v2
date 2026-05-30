use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::jobs::{enqueue_job, JobMetadata};
use super::{api_error, AppState};
use crate::db;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WatchSettings {
    pub watch_enabled: bool,
    pub watch_root: String,
    pub watch_done_dir: String,
}

#[derive(Debug, Clone)]
pub(crate) struct WatchFileState {
    size: u64,
    modified: Option<SystemTime>,
    stable_since: Instant,
}

const WATCH_SETTINGS_KEY: &str = "watch_settings";
const WATCH_SETTINGS_VERSION: i64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredWatchSettings {
    version: i64,
    watch_enabled: bool,
    watch_root: String,
    watch_done_dir: String,
}

impl From<WatchSettings> for StoredWatchSettings {
    fn from(value: WatchSettings) -> Self {
        Self {
            version: WATCH_SETTINGS_VERSION,
            watch_enabled: value.watch_enabled,
            watch_root: value.watch_root,
            watch_done_dir: value.watch_done_dir,
        }
    }
}

impl From<StoredWatchSettings> for WatchSettings {
    fn from(value: StoredWatchSettings) -> Self {
        Self {
            watch_enabled: value.watch_enabled,
            watch_root: value.watch_root,
            watch_done_dir: value.watch_done_dir,
        }
    }
}

pub(crate) fn load_watch_settings(
    conn: &rusqlite::Connection,
    legacy_path: &Path,
) -> anyhow::Result<WatchSettings> {
    if let Some(raw) = db::get_internal_value(conn, WATCH_SETTINGS_KEY)? {
        match serde_json::from_str::<StoredWatchSettings>(&raw) {
            Ok(stored) if stored.version == WATCH_SETTINGS_VERSION => return Ok(stored.into()),
            Ok(stored) => tracing::warn!(
                version = stored.version,
                "unsupported watch settings version; using defaults"
            ),
            Err(e) => tracing::warn!(error = %e, "watch settings row is invalid; using defaults"),
        }
    }

    let settings = load_legacy_or_env_watch_settings(legacy_path);
    let stored = StoredWatchSettings::from(settings.clone());
    if let Ok(raw) = serde_json::to_string(&stored) {
        db::set_internal_value(conn, WATCH_SETTINGS_KEY, &raw)?;
    }
    Ok(settings)
}

fn load_legacy_or_env_watch_settings(path: &Path) -> WatchSettings {
    if path.exists() {
        match std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<WatchSettings>(&bytes).ok())
        {
            Some(settings) => return settings,
            None => tracing::warn!(
                path = %path.display(),
                "watch settings json is invalid; using env/default values"
            ),
        }
    }
    let watch_root = std::env::var("WATCH_ROOT").unwrap_or_default();
    let watch_done_dir = std::env::var("WATCH_DONE_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if watch_root.trim().is_empty() {
                String::new()
            } else {
                Path::new(&watch_root)
                    .join("done")
                    .to_string_lossy()
                    .to_string()
            }
        });
    let watch_enabled = std::env::var("WATCH_ENABLED")
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    WatchSettings {
        watch_enabled,
        watch_root,
        watch_done_dir,
    }
}

pub(super) async fn handle_get_watch_settings(State(state): State<Arc<AppState>>) -> Response {
    let settings = state.watch_settings.read().await.clone();
    Json(json!({
        "watch_enabled": settings.watch_enabled,
        "watch_root": settings.watch_root,
        "watch_done_dir": settings.watch_done_dir,
        "watch_running": settings.watch_enabled && validate_watch_paths(&settings).is_ok(),
    }))
    .into_response()
}

pub(super) async fn handle_post_watch_settings(
    State(state): State<Arc<AppState>>,
    Json(body): Json<WatchSettings>,
) -> Response {
    if let Err(e) = validate_watch_paths(&body) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_watch_settings", e);
    }
    if let Err(e) = tokio::fs::create_dir_all(&body.watch_done_dir).await {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "watch_settings_save_failed",
            e.to_string(),
        );
    }
    let bytes = match serde_json::to_string(&StoredWatchSettings::from(body.clone())) {
        Ok(bytes) => bytes,
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "watch_settings_save_failed",
                e.to_string(),
            )
        }
    };
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "watch_settings_save_failed",
                e.to_string(),
            )
        }
    };
    if let Err(e) = db::set_internal_value(&conn, WATCH_SETTINGS_KEY, &bytes) {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "watch_settings_save_failed",
            e.to_string(),
        );
    }
    let old = state.watch_settings.read().await.clone();
    *state.watch_settings.write().await = body.clone();
    // Only reset the in-memory dedup cache when watch-relevant settings change
    if old.watch_enabled != body.watch_enabled
        || old.watch_root != body.watch_root
        || old.watch_done_dir != body.watch_done_dir
    {
        state.watch_seen.lock().await.clear();
    }
    Json(json!({
        "watch_enabled": body.watch_enabled,
        "watch_root": body.watch_root,
        "watch_done_dir": body.watch_done_dir,
        "watch_running": body.watch_enabled,
    }))
    .into_response()
}

pub(super) async fn watch_folder_poller(state: Arc<AppState>) {
    loop {
        let settings = state.watch_settings.read().await.clone();
        let cfg = state.config.read().await.clone();
        if settings.watch_enabled {
            if let Err(e) = scan_watch_folder(&state, &settings, &cfg).await {
                tracing::warn!(error = %e, "watch folder scan failed");
            }
        }
        let sleep_secs = if settings.watch_enabled {
            cfg.watch_poll_seconds.max(1) as u64
        } else {
            1
        };
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(sleep_secs)) => {}
            _ = state.shutdown_token.cancelled() => break,
        }
    }
}

async fn scan_watch_folder(
    state: &Arc<AppState>,
    settings: &WatchSettings,
    cfg: &crate::config::Config,
) -> anyhow::Result<()> {
    let (root, done) = validate_watch_paths(settings).map_err(anyhow::Error::msg)?;
    let candidates = collect_candidate_metadata(root.clone(), done.clone(), cfg.clone()).await?;

    let now = Instant::now();
    let mut current = HashSet::new();
    let mut ready = Vec::new();
    {
        let mut seen = state.watch_seen.lock().await;
        for (path, size, modified) in candidates {
            current.insert(path.clone());
            match seen.get_mut(&path) {
                Some(prev) if prev.size == size && prev.modified == modified => {
                    if now.duration_since(prev.stable_since)
                        >= Duration::from_secs(cfg.watch_stable_seconds.max(1) as u64)
                    {
                        ready.push(path.clone());
                    }
                }
                Some(prev) => {
                    prev.size = size;
                    prev.modified = modified;
                    prev.stable_since = now;
                }
                None => {
                    seen.insert(
                        path,
                        WatchFileState {
                            size,
                            modified,
                            stable_since: now,
                        },
                    );
                }
            }
        }
        seen.retain(|path, _| current.contains(path));
    }

    for path in ready {
        if let Err(e) = claim_and_enqueue(state, &root, &done, &path).await {
            tracing::warn!(path = %path.display(), error = %e, "watch file claim failed");
        }
        state.watch_seen.lock().await.remove(&path);
    }
    Ok(())
}

async fn collect_candidate_metadata(
    root: PathBuf,
    done: PathBuf,
    cfg: crate::config::Config,
) -> anyhow::Result<Vec<(PathBuf, u64, Option<SystemTime>)>> {
    tokio::task::spawn_blocking(move || {
        let mut candidates = Vec::new();
        collect_candidates(&root, &done, &cfg, &mut candidates);

        let mut with_metadata = Vec::new();
        for path in candidates {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            with_metadata.push((path, meta.len(), meta.modified().ok()));
        }
        with_metadata
    })
    .await
    .map_err(anyhow::Error::from)
}

fn collect_candidates(
    dir: &Path,
    done: &Path,
    cfg: &crate::config::Config,
    out: &mut Vec<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(canon) = path.canonicalize() else {
            continue;
        };
        if canon == done || canon.starts_with(done) {
            continue;
        }
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            collect_candidates(&canon, done, cfg, out);
        } else if ft.is_file() && is_watch_video(&canon, cfg) {
            out.push(canon);
        }
    }
}

fn is_watch_video(path: &Path, cfg: &crate::config::Config) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if cfg
        .watch_ignore_suffixes
        .iter()
        .any(|suffix| name.ends_with(&suffix.to_ascii_lowercase()))
    {
        return false;
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| format!(".{}", s.to_ascii_lowercase()))
        .unwrap_or_default();
    cfg.watch_video_extensions
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&ext))
}

async fn claim_and_enqueue(
    state: &Arc<AppState>,
    root: &Path,
    done: &Path,
    source: &Path,
) -> anyhow::Result<()> {
    let root = root.to_path_buf();
    let done = done.to_path_buf();
    let source = source.to_path_buf();
    let (source, target) = tokio::task::spawn_blocking(move || {
        let source = source.canonicalize()?;
        if !source.starts_with(&root) || source.starts_with(&done) {
            anyhow::bail!("watch source escapes root or is inside done dir");
        }
        let rel = source.strip_prefix(&root)?;
        let target = done.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok::<(PathBuf, PathBuf), anyhow::Error>((source, target))
    })
    .await??;

    let filename = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("watch-file")
        .to_string();

    // Check for active job BEFORE rename so the source stays in the watch root on dedup
    {
        let conn = state.db_conn().await?;
        let filename_check = filename.clone();
        let exists = tokio::task::spawn_blocking(move || {
            crate::db::job_exists_active_by_filename(&conn, &filename_check, "Film")
        })
        .await??;
        if exists {
            tracing::info!(filename = %filename, "watch folder file already has an active job; skipping");
            return Ok(());
        }
    }

    // Check if target already exists — protect against overwriting higher-quality files
    let target_exists = tokio::task::spawn_blocking({
        let target = target.clone();
        move || target.exists()
    })
    .await?;

    // Stage the existing done file (rather than deleting it) so we can restore on enqueue failure
    let staged_backup: Option<PathBuf> = if target_exists {
        let source_path = source.clone();
        let new_analysis = crate::media::analyze_media(&source_path).await;
        let new_bitrate = match &new_analysis {
            Ok(a) => a
                .video_streams
                .first()
                .map(|v| v.bit_rate.trim().parse::<i64>().unwrap_or(0))
                .unwrap_or(0),
            Err(e) => {
                tracing::warn!(error = %e, "failed to probe watch file; refusing to overwrite existing done file");
                anyhow::bail!(
                    "watch file '{}' already exists and new file probe failed; refusing to overwrite",
                    target.display()
                );
            }
        };

        let conn = state.db_conn().await?;
        let filename_for_db = filename.clone();
        let existing_bitrate =
            crate::db::get_job_source_bitrate_by_filename(&conn, &filename_for_db, "Film")?;

        match existing_bitrate {
            None => {
                anyhow::bail!(
                    "watch file '{}' already exists and no previous job found in DB; refusing to overwrite",
                    target.display()
                );
            }
            Some(0) => {
                anyhow::bail!(
                    "watch file '{}' already exists and previous job has no bitrate data; refusing to overwrite",
                    target.display()
                );
            }
            Some(existing) if new_bitrate > existing => {
                tracing::info!(
                    filename = %filename,
                    new_bitrate,
                    existing_bitrate = existing,
                    "watch file: new source has higher bitrate; staging existing done file for replacement"
                );
                let backup = target.with_extension("done.bak");
                let target_clone = target.clone();
                let backup_clone = backup.clone();
                tokio::task::spawn_blocking(move || std::fs::rename(&target_clone, &backup_clone))
                    .await??;
                Some(backup)
            }
            Some(existing) => {
                anyhow::bail!(
                    "watch file '{}' already exists with equal or higher bitrate ({} >= {} bps); refusing to overwrite",
                    target.display(),
                    existing,
                    new_bitrate
                );
            }
        }
    } else {
        None
    };

    let rename_target = target.clone();
    let source_clone = source.clone();
    if let Err(e) =
        tokio::task::spawn_blocking(move || std::fs::rename(&source_clone, &rename_target)).await?
    {
        if let Some(ref backup) = staged_backup {
            let backup_clone = backup.clone();
            let target_clone = target.clone();
            let _ =
                tokio::task::spawn_blocking(move || std::fs::rename(&backup_clone, &target_clone))
                    .await;
        }
        return Err(e.into());
    }

    let enqueue_result = enqueue_job(
        state,
        filename.clone(),
        target.clone(),
        JobMetadata {
            media_type: Some("Film".into()),
            title: Some(filename),
            ..Default::default()
        },
        false,
        None,
    )
    .await;

    match enqueue_result {
        Ok(_) => {
            if let Some(backup) = staged_backup {
                let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(&backup)).await;
            }
            Ok(())
        }
        Err(e) => {
            let source_clone = source.clone();
            let target_clone = target.clone();
            let _ =
                tokio::task::spawn_blocking(move || std::fs::rename(&target_clone, &source_clone))
                    .await;
            if let Some(backup) = staged_backup {
                let target_clone = target.clone();
                let _ =
                    tokio::task::spawn_blocking(move || std::fs::rename(&backup, &target_clone))
                        .await;
            }
            Err(e)
        }
    }
}

fn validate_watch_paths(settings: &WatchSettings) -> Result<(PathBuf, PathBuf), String> {
    let root = PathBuf::from(settings.watch_root.trim());
    if root.as_os_str().is_empty() {
        return Err("watch_root is required".into());
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("watch_root is invalid: {e}"))?;
    if !root.is_dir() {
        return Err("watch_root must be a directory".into());
    }

    let done = PathBuf::from(settings.watch_done_dir.trim());
    if done.as_os_str().is_empty() {
        return Err("watch_done_dir is required".into());
    }
    let done = canonicalize_existing_or_parent(&done)?;
    if !done.starts_with(&root) || done == root {
        return Err("watch_done_dir must be inside watch_root".into());
    }
    Ok((root, done))
}

fn canonicalize_existing_or_parent(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return path
            .canonicalize()
            .map_err(|e| format!("watch_done_dir is invalid: {e}"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| "watch_done_dir must have a parent".to_string())?;
    let parent = parent
        .canonicalize()
        .map_err(|e| format!("watch_done_dir parent is invalid: {e}"))?;
    let name = path
        .file_name()
        .ok_or_else(|| "watch_done_dir must have a final path component".to_string())?;
    Ok(parent.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock};

    use crate::api::AppState;
    use crate::telegram::TelegramRuntime;
    use std::collections::HashMap;

    fn make_state(dir: &std::path::Path) -> Arc<AppState> {
        let db_path = dir.join("streamer.db");
        let uploads_dir = dir.join("uploads");
        let processing_dir = dir.join("processing");
        std::fs::create_dir_all(&uploads_dir).unwrap();
        std::fs::create_dir_all(&processing_dir).unwrap();
        let pool = crate::db::init_db_pool(&db_path).unwrap();
        let conn = pool.get().unwrap();
        let cfg = Config::load(&conn).unwrap();
        drop(conn);
        let (job_queue, _job_receiver) = tokio::sync::mpsc::channel(100);
        Arc::new(AppState {
            db: RwLock::new(pool),
            db_path: db_path.clone(),
            env_path: db_path.parent().unwrap().join(".env"),
            config: RwLock::new(Arc::new(cfg)),
            started_at: std::time::Instant::now(),
            bot_health: RwLock::new(Vec::new()),
            cloudflared: crate::cloudflared::SharedCloudflaredStatus::default(),
            http: reqwest::Client::new(),
            telegram: TelegramRuntime::new(),
            telegram_base_url: crate::telegram::DEFAULT_API_BASE.to_string(),
            uploads_dir,
            processing_dir,
            watch_settings: RwLock::new(WatchSettings {
                watch_enabled: true,
                watch_root: String::new(),
                watch_done_dir: String::new(),
            }),
            watch_seen: Mutex::new(HashMap::new()),
            pending_uploads: Mutex::new(HashMap::new()),
            upload_rate_limits: Mutex::new(HashMap::new()),
            db_sync_lock: Mutex::new(()),
            jobs: Mutex::new(HashMap::new()),
            played_segments: Mutex::new(HashMap::new()),
            segment_meta_cache: Mutex::new(HashMap::new()),
            job_queue,
            cache: Arc::new(crate::api::playback::SegmentCache::new(64 * 1024 * 1024)),
            ffmpeg_available: true,
            ffprobe_available: true,
            selected_encoder: RwLock::new(crate::media::cpu_encoder()),
            last_bot_index: std::sync::atomic::AtomicI64::new(0),
            shutdown_token: tokio_util::sync::CancellationToken::new(),
            ingest_download_semaphore: Arc::new(tokio::sync::Semaphore::new(5)),
        })
    }

    #[tokio::test]
    async fn scan_watch_folder_tolerates_disappearing_files() {
        let dir = std::env::temp_dir().join(format!(
            "thls_watch_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = dir.join("watch");
        let done = root.join("done");
        std::fs::create_dir_all(&done).unwrap();

        // Create a video file
        let video = root.join("test.mp4");
        std::fs::write(&video, b"fake video").unwrap();

        let state = make_state(&dir);
        let settings = WatchSettings {
            watch_enabled: true,
            watch_root: root.to_string_lossy().to_string(),
            watch_done_dir: done.to_string_lossy().to_string(),
        };
        let cfg = state.config.read().await.clone();

        // Delete the file between candidate collection and metadata stat
        // collect_candidates will find it (canonicalize succeeds), but
        // by the time scan_watch_folder stats it, it's gone.
        // We can't easily interleave, so instead: test that a scan on a
        // directory where files disappeared returns Ok.
        std::fs::remove_file(&video).unwrap();

        let result = scan_watch_folder(&state, &settings, &cfg).await;
        assert!(result.is_ok(), "scan should tolerate disappearing files");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn claim_keeps_file_in_place_when_enqueue_fails() {
        let dir = std::env::temp_dir().join(format!(
            "thls_watch_claim_fail_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = dir.join("watch");
        let done = root.join("done");
        std::fs::create_dir_all(&done).unwrap();

        let video = root.join("test.mp4");
        std::fs::write(&video, b"fake video").unwrap();

        let state = make_state(&dir);
        let result = claim_and_enqueue(
            &state,
            &root.canonicalize().unwrap(),
            &done.canonicalize().unwrap(),
            &video,
        )
        .await;

        assert!(result.is_err(), "enqueue should fail with closed receiver");
        assert!(video.exists(), "failed enqueue must leave source in place");
        assert!(
            !done.join("test.mp4").exists(),
            "failed enqueue must not claim the file"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
