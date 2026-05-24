use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;

use super::super::AppState;
use super::types::*;
use crate::config::{BotConfig, Config};
use crate::{db, media, telegram};

pub(crate) async fn enqueue_job(
    state: &Arc<AppState>,
    filename: String,
    source_path: PathBuf,
    metadata: JobMetadata,
    delete_source_on_finish: bool,
    original_source_path: Option<String>,
) -> Result<String> {
    let job_id = uuid::Uuid::new_v4().simple().to_string();
    enqueue_existing_job(
        state,
        job_id.clone(),
        filename,
        source_path,
        metadata,
        delete_source_on_finish,
        original_source_path,
        true,
    )
    .await?;
    Ok(job_id)
}

pub(crate) async fn enqueue_existing_job(
    state: &Arc<AppState>,
    job_id: String,
    filename: String,
    source_path: PathBuf,
    metadata: JobMetadata,
    delete_source_on_finish: bool,
    original_source_path: Option<String>,
    insert_state: bool,
) -> Result<()> {
    let original_source_path = sanitize_original_source_path(original_source_path)?;
    let processing_path = state.processing_dir.join(&job_id);
    {
        let mut jobs = state.jobs.lock().await;
        if insert_state {
            let job = JobState {
                job_id: job_id.clone(),
                filename: filename.clone(),
                source_path: source_path.clone(),
                processing_path,
                status: JobStatus::Queued,
                progress: 0.0,
                step: 0,
                total_steps: 5,
                description: "queued".into(),
                queued_at: Instant::now(),
                started_at: None,
                finished_at: None,
                cancel_requested: false,
                cancel_flag: Arc::new(AtomicBool::new(false)),
                error: None,
                metadata: metadata.clone(),
                analysis: None,
                delete_source_on_finish,
                original_source_path: original_source_path.clone(),
            };
            jobs.insert(job_id.clone(), job);
        } else if let Some(job) = jobs.get_mut(&job_id) {
            if job.cancel_requested || job.status == JobStatus::Cancelled {
                bail!("cancelled");
            }
            job.filename = filename.clone();
            job.source_path = source_path.clone();
            job.processing_path = processing_path;
            job.status = JobStatus::Queued;
            job.progress = 0.0;
            job.description = "queued".into();
            job.queued_at = Instant::now();
            job.started_at = None;
            job.metadata = metadata.clone();
            job.delete_source_on_finish = delete_source_on_finish;
            job.original_source_path = original_source_path.clone();
        } else {
            bail!("job state is unavailable");
        }
    }

    let request = JobRequest {
        job_id: job_id.clone(),
        filename,
        source_path,
        metadata,
        delete_source_on_finish,
        original_source_path,
    };
    tracing::info!(
        job_id = %job_id,
        filename = %request.filename,
        source_path = %request.source_path.display(),
        delete_source_on_finish,
        "job enqueued"
    );
    if let Ok(conn) = state.db_conn().await {
        let job_id_for_db = job_id.clone();
        let filename_for_db = request.filename.clone();
        let result = tokio::task::spawn_blocking(move || {
            db::insert_job_marker(&conn, &job_id_for_db, &filename_for_db, "queued")
        })
        .await;
        if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
            tracing::warn!(job_id = %job_id, error = %e, "failed to write queued DB marker");
        }
    }
    if state.job_queue.send(request).await.is_err() {
        state.jobs.lock().await.remove(&job_id);
        bail!("job queue is unavailable");
    }
    Ok(())
}

pub(super) fn sanitize_original_source_path(value: Option<String>) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if FsPath::new(value).is_absolute()
        || value.contains("..")
        || value.contains('/')
        || value.contains('\\')
        || value.contains('\0')
    {
        bail!("original_source_path must be a filename label");
    }
    Ok(Some(value.to_string()))
}

fn spawn_supervised<F, Fut>(name: &'static str, mut make_future: F)
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(8);
        loop {
            let handle = tokio::spawn(make_future());
            let result = handle.await;
            match result {
                Ok(()) => {
                    tracing::info!(worker = name, "worker exited normally, respawning");
                }
                Err(e) if e.is_panic() => {
                    tracing::error!(
                        worker = name,
                        "worker panicked, respawning in {:?}",
                        backoff
                    );
                }
                Err(e) => {
                    tracing::error!(worker = name, error = %e, "worker join error, respawning in {:?}", backoff);
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(max_backoff);
        }
    });
}

pub(crate) fn start_background_tasks(state: Arc<AppState>, receiver: mpsc::Receiver<JobRequest>) {
    let dispatcher_state = state.clone();
    tokio::spawn(async move {
        let handle = tokio::spawn(job_dispatcher(dispatcher_state, receiver));
        match handle.await {
            Ok(()) => tracing::info!(worker = "job_dispatcher", "exited"),
            Err(e) => tracing::error!(worker = "job_dispatcher", "panicked: {e}"),
        }
    });
    {
        let state = state.clone();
        spawn_supervised("upload_sweeper", move || {
            let state = state.clone();
            async move { super::super::uploads::upload_sweeper(state).await }
        });
    }
    {
        let state = state.clone();
        spawn_supervised("watch_folder_poller", move || {
            let state = state.clone();
            async move { super::super::watch_folder::watch_folder_poller(state).await }
        });
    }
    {
        let state = state.clone();
        spawn_supervised("job_timeout_watcher", move || {
            let state = state.clone();
            async move { job_timeout_watcher(state).await }
        });
    }
}

async fn job_dispatcher(state: Arc<AppState>, mut receiver: mpsc::Receiver<JobRequest>) {
    let max = state.config.read().await.max_concurrent_jobs.max(1) as usize;
    let semaphore = Arc::new(Semaphore::new(max));
    loop {
        tokio::select! {
            job = receiver.recv() => {
                let Some(job) = job else { break };
                let Ok(permit) = semaphore.clone().acquire_owned().await else {
                    break;
                };
                let state = state.clone();
                tokio::spawn(async move {
                    process_job(state, job).await;
                    drop(permit);
                });
            }
            _ = state.shutdown_token.cancelled() => {
                tracing::info!("job dispatcher shutting down");
                break;
            }
        }
    }
}

async fn process_job(state: Arc<AppState>, request: JobRequest) {
    let processing_path = state.processing_dir.join(&request.job_id);
    let job_started = Instant::now();
    tracing::info!(
        job_id = %request.job_id,
        filename = %request.filename,
        source_path = %request.source_path.display(),
        "job cycle started"
    );
    {
        let mut jobs = state.jobs.lock().await;
        let Some(job) = jobs.get_mut(&request.job_id) else {
            return;
        };
        if job.cancel_requested || job.status == JobStatus::Cancelled {
            return;
        }
        job.status = JobStatus::Analyzing;
        job.progress = 10.0;
        job.step = 1;
        job.description = "analyzing".into();
        job.started_at = Some(Instant::now());
    }
    tracing::info!(job_id = %request.job_id, "job analyzing started");

    {
        match state.db_conn().await {
            Ok(conn) => {
                let job_id_for_db = request.job_id.clone();
                let filename_for_db = request.filename.clone();
                let result = tokio::task::spawn_blocking(move || {
                    db::insert_processing_marker(&conn, &job_id_for_db, &filename_for_db)
                })
                .await;
                if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
                    tracing::warn!(job_id = %request.job_id, error = %e, "failed to write processing DB marker");
                }
            }
            Err(e) => {
                tracing::warn!(job_id = %request.job_id, error = %e, "failed to acquire DB connection for processing marker");
            }
        }
    }

    if let Err(e) = tokio::fs::create_dir_all(&processing_path).await {
        finish_job_error(
            &state,
            &request.job_id,
            format!("processing_dir_failed: {e}"),
        )
        .await;
        cleanup_request_paths(&request, &processing_path).await;
        return;
    }

    let analysis = match media::analyze_media(&request.source_path).await {
        Ok(analysis) => analysis,
        Err(e) => {
            finish_job_error(&state, &request.job_id, format!("analysis_failed: {e}")).await;
            cleanup_request_paths(&request, &processing_path).await;
            return;
        }
    };
    tracing::info!(
        job_id = %request.job_id,
        duration = analysis.duration,
        file_size = analysis.file_size,
        video_streams = analysis.video_streams.len(),
        audio_streams = analysis.audio_streams.len(),
        subtitle_streams = analysis.subtitle_streams.len(),
        "job analysis complete"
    );
    {
        let mut jobs = state.jobs.lock().await;
        let Some(job) = jobs.get_mut(&request.job_id) else {
            cleanup_request_paths(&request, &processing_path).await;
            return;
        };
        if job.cancel_requested || job.status == JobStatus::Cancelled {
            cleanup_request_paths(&request, &processing_path).await;
            return;
        }
        job.analysis = Some(analysis.clone());
        job.status = JobStatus::Processing;
        job.progress = 30.0;
        job.step = 2;
        job.description = "processing video, audio, subtitles, and thumbnail".into();
    }
    tracing::info!(job_id = %request.job_id, "job media processing started");

    let (cfg, cancel_flag) = {
        let jobs = state.jobs.lock().await;
        let flag = jobs
            .get(&request.job_id)
            .map(|j| j.cancel_flag.clone())
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        (state.config.read().await.clone(), flag)
    };
    let abr_override = request.metadata.abr_tiers_override.clone();

    let estimated_output_bytes = analysis.file_size
        * if !cfg.abr_tiers.is_empty() && cfg.abr_enabled {
            3
        } else {
            1
        }
        + 256 * 1024 * 1024;
    match super::super::uploads::free_space_bytes(&state.processing_dir) {
        Ok(free) if free < estimated_output_bytes => {
            finish_job_error(
                &state,
                &request.job_id,
                format!(
                    "insufficient_disk_space: need ~{} MB, have {} MB free",
                    estimated_output_bytes / (1024 * 1024),
                    free / (1024 * 1024)
                ),
            )
            .await;
            cleanup_request_paths(&request, &processing_path).await;
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, "disk space check failed, continuing");
        }
        _ => {}
    }

    let result = match media::process_media(
        &analysis,
        &request.job_id,
        &processing_path,
        &cfg,
        &cancel_flag,
        abr_override.as_deref(),
    )
    .await
    {
        Ok(result) => result,
        Err(e) => {
            let msg = e.to_string();
            if msg == "cancelled" || cancel_flag.load(Ordering::Relaxed) {
                cleanup_request_paths(&request, &processing_path).await;
            } else {
                finish_job_error(&state, &request.job_id, format!("processing_failed: {e}")).await;
                cleanup_request_paths(&request, &processing_path).await;
            }
            return;
        }
    };
    tracing::info!(
        job_id = %result.job_id,
        output_dir = %result.output_dir.display(),
        video_playlists = result.video_playlists.len(),
        audio_playlists = result.audio_playlists.len(),
        subtitle_files = result.subtitle_files.len(),
        segment_durations = result.segment_durations.len(),
        thumbnail = result.thumbnail_path.is_some(),
        oversized_repaired = result.oversized_segments_repaired,
        "media processing complete"
    );

    if result.oversized_segments_repaired > 0 {
        let mut jobs = state.jobs.lock().await;
        if let Some(job) = jobs.get_mut(&request.job_id) {
            job.description = format!(
                "repaired {} oversized segment(s)",
                result.oversized_segments_repaired
            );
        }
    }

    {
        let mut jobs = state.jobs.lock().await;
        let Some(job) = jobs.get_mut(&request.job_id) else {
            cleanup_request_paths(&request, &processing_path).await;
            return;
        };
        if job.cancel_requested || job.status == JobStatus::Cancelled {
            cleanup_request_paths(&request, &processing_path).await;
            return;
        }
        job.status = JobStatus::Uploading;
        job.progress = 80.0;
        job.step = 4;
        job.description = "uploading to Telegram".into();
    }
    tracing::info!(job_id = %request.job_id, "job telegram upload started");

    let (uploads, last_upload_bot_index) =
        match upload_outputs(state.clone(), &cfg, &result, &cancel_flag).await {
            Ok(uploads) => uploads,
            Err(e) => {
                finish_job_error(&state, &request.job_id, format!("upload_failed: {e}")).await;
                cleanup_request_paths(&request, &processing_path).await;
                return;
            }
        };
    if let Some(last_bot_index) = last_upload_bot_index {
        let conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => {
                finish_job_error(
                    &state,
                    &request.job_id,
                    format!("bot_index_persist_failed: {e}"),
                )
                .await;
                cleanup_request_paths(&request, &processing_path).await;
                return;
            }
        };
        let result =
            tokio::task::spawn_blocking(move || db::set_last_bot_index(&conn, last_bot_index))
                .await;
        if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
            finish_job_error(
                &state,
                &request.job_id,
                format!("bot_index_persist_failed: {e}"),
            )
            .await;
            cleanup_request_paths(&request, &processing_path).await;
            return;
        }
        state
            .last_bot_index
            .store(last_bot_index, std::sync::atomic::Ordering::Relaxed);
    }
    if job_cancelled(&state, &request.job_id).await {
        cleanup_request_paths(&request, &processing_path).await;
        return;
    }

    let (job, tracks, segments, segment_parts) =
        build_db_rows(&request, &analysis, &result, uploads);
    {
        let mut conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => {
                finish_job_error(&state, &request.job_id, format!("db_conn_failed: {e}")).await;
                cleanup_request_paths(&request, &processing_path).await;
                return;
            }
        };
        tracing::info!(
            job_id = %request.job_id,
            tracks = tracks.len(),
            segments = segments.len(),
            segment_parts = segment_parts.len(),
            "job db save started"
        );
        let result = tokio::task::spawn_blocking(move || {
            db::save_job(&mut conn, &job, &tracks, &segments, &segment_parts)
        })
        .await;
        if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
            finish_job_error(&state, &request.job_id, format!("db_save_failed: {e}")).await;
            cleanup_request_paths(&request, &processing_path).await;
            return;
        }
    }
    tracing::info!(
        job_id = %request.job_id,
        elapsed_ms = job_started.elapsed().as_millis(),
        "job cycle complete"
    );
    finish_job_complete(&state, &request.job_id).await;

    detect_and_save_markers(&state, &request, &analysis).await;
    auto_fetch_metadata_if_enabled(&state, &request).await;

    super::super::playback::spawn_cache_warmup(state.clone(), request.job_id.clone());
    super::super::db_transfer::trigger_automatic_db_sync(
        state.clone(),
        format!("job-{}", request.job_id),
    );
    cleanup_request_paths(&request, &processing_path).await;
}

async fn upload_outputs(
    state: Arc<AppState>,
    cfg: &Config,
    result: &media::ProcessingResult,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<(Vec<telegram::UploadedFile>, Option<i64>)> {
    // cfg.telegram_max_file_size is user-configurable; raise if Telegram increases Bot API limits.
    let files = prepare_upload_files(&result.output_dir, cfg.telegram_max_file_size).await?;
    if files.is_empty() {
        bail!("no uploadable HLS output files found");
    }
    let total = files.len();
    tracing::info!(
        job_id = %result.job_id,
        files = total,
        upload_parallelism = cfg.upload_parallelism,
        "telegram output upload batch prepared"
    );
    let assignments = assign_upload_bots(&state, files.len()).await?;
    let last_upload_bot_index = assignments.last().map(|(index, _)| *index);
    let parallelism = cfg
        .upload_parallelism
        .max(1)
        .min(assignments.len() as u32)
        .min(files.len() as u32) as usize;
    let semaphore = Arc::new(Semaphore::new(parallelism));
    let mut tasks = JoinSet::new();

    for ((segment_key, path), (bot_index, bot)) in files.into_iter().zip(assignments) {
        if cancel_flag.load(Ordering::Relaxed) {
            tasks.abort_all();
            bail!("cancelled");
        }
        let permit = semaphore.clone().acquire_owned().await?;
        let state = state.clone();
        let base_url = state.telegram_base_url.clone();
        let client = state.http.clone();
        let max_file_size = cfg.telegram_max_file_size;
        let cancel_flag_clone = cancel_flag.clone();
        tasks.spawn(async move {
            let _permit = permit;
            if cancel_flag_clone.load(Ordering::Relaxed) {
                return Err(anyhow::anyhow!("cancelled"));
            }
            telegram::upload_document(
                &client,
                &state.telegram,
                &base_url,
                bot,
                bot_index,
                &path,
                segment_key,
                max_file_size,
            )
            .await
        });
    }

    let mut uploaded = Vec::new();
    while let Some(task_result) = tasks.join_next().await {
        if cancel_flag.load(Ordering::Relaxed) {
            tasks.abort_all();
            bail!("cancelled");
        }
        uploaded.push(task_result.context("upload task panicked")??);
        if let Some(last) = uploaded.last() {
            tracing::info!(
                job_id = %result.job_id,
                segment_key = %last.segment_key,
                file_size = last.file_size,
                bot_index = last.bot_index,
                uploaded = uploaded.len(),
                total,
                "telegram output file uploaded"
            );
        }
        update_upload_progress(&state, &result.job_id, uploaded.len(), total).await;
    }
    uploaded.sort_by(|a, b| a.segment_key.cmp(&b.segment_key));
    Ok((uploaded, last_upload_bot_index))
}

async fn update_upload_progress(state: &AppState, job_id: &str, current: usize, total: usize) {
    let mut jobs = state.jobs.lock().await;
    let Some(job) = jobs.get_mut(job_id) else {
        return;
    };
    if job.status != JobStatus::Uploading || total == 0 {
        return;
    }
    let pct = current as f64 / total as f64;
    job.progress = 80.0 + pct * 18.0;
    job.description = format!("uploading to Telegram ({current}/{total})");
}

async fn split_file_for_upload(
    path: &PathBuf,
    segment_key: &str,
    max_size: u64,
    temp_dir: &FsPath,
) -> Result<Vec<(String, PathBuf)>> {
    let file_size = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("stat {}", path.display()))?
        .len();

    if file_size <= max_size {
        return Ok(vec![(segment_key.to_string(), path.clone())]);
    }

    let chunk_size = (max_size.saturating_mul(95) / 100).max(1);
    let total_parts = ((file_size - 1) / chunk_size + 1) as i64;
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    let mut parts = Vec::new();

    tracing::info!(
        segment_key,
        file_size,
        chunk_size,
        total_parts,
        "splitting oversized segment"
    );

    for part_index in 0..total_parts {
        let part_key = format!("{}/part_{}", segment_key, part_index);
        let part_path = temp_dir.join(part_index.to_string());
        let offset = part_index as u64 * chunk_size;
        let mut remaining = (file_size - offset).min(chunk_size);
        let mut part_file = tokio::fs::File::create(&part_path)
            .await
            .with_context(|| format!("create part {}", part_path.display()))?;
        let mut buf = vec![0_u8; 256 * 1024];
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = file
                .read(&mut buf[..want])
                .await
                .with_context(|| format!("read part {} from {}", part_index, path.display()))?;
            if n == 0 {
                bail!("unexpected EOF splitting {}", path.display());
            }
            part_file
                .write_all(&buf[..n])
                .await
                .with_context(|| format!("write part {}", part_path.display()))?;
            remaining -= n as u64;
        }
        part_file
            .flush()
            .await
            .with_context(|| format!("flush part {}", part_path.display()))?;
        parts.push((part_key, part_path));
    }

    Ok(parts)
}

pub(super) async fn collect_upload_files(output_dir: &FsPath) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut dirs = tokio::fs::read_dir(output_dir).await?;
    while let Some(dir) = dirs.next_entry().await? {
        if !dir.file_type().await?.is_dir() {
            continue;
        }
        let prefix = dir.file_name().to_string_lossy().to_string();
        let mut files = tokio::fs::read_dir(dir.path()).await?;
        while let Some(file) = files.next_entry().await? {
            if !file.file_type().await?.is_file() {
                continue;
            }
            let name = file.file_name().to_string_lossy().to_string();
            let path = file.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if name != "init.mp4" && !matches!(ext, "m4s" | "ts" | "vtt" | "jpg") {
                continue;
            }
            out.push((format!("{prefix}/{name}"), path));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

async fn prepare_upload_files(
    output_dir: &FsPath,
    // User-configurable; matches cfg.telegram_max_file_size. Raise if Telegram changes limits.
    max_file_size: u64,
) -> Result<Vec<(String, PathBuf)>> {
    let files = collect_upload_files(output_dir).await?;
    if files.is_empty() {
        return Ok(files);
    }

    let temp_dir = output_dir.join("temp_parts");
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .with_context(|| format!("create temp dir {}", temp_dir.display()))?;

    let mut result = Vec::new();
    for (segment_key, path) in files {
        let file_size = tokio::fs::metadata(&path)
            .await
            .with_context(|| format!("stat {}", path.display()))?
            .len();

        if file_size <= max_file_size {
            result.push((segment_key, path));
        } else {
            let parts_dir = temp_dir.join(segment_key.replace('/', "_"));
            tokio::fs::create_dir_all(&parts_dir)
                .await
                .with_context(|| format!("create parts dir {}", parts_dir.display()))?;
            let parts =
                split_file_for_upload(&path, &segment_key, max_file_size, &parts_dir).await?;
            result.extend(parts);
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

async fn assign_upload_bots(state: &AppState, file_count: usize) -> Result<Vec<(i64, BotConfig)>> {
    let cfg = state.config.read().await.clone();
    if cfg.bots.is_empty() {
        bail!("no Telegram bots configured");
    }
    let last = state
        .last_bot_index
        .load(std::sync::atomic::Ordering::Relaxed);
    let mut assignments = Vec::with_capacity(file_count);
    let bot_count = cfg.bots.len() as i64;
    for offset in 0..file_count {
        let index = (last + 1 + offset as i64).rem_euclid(bot_count);
        assignments.push((index, cfg.bots[index as usize].clone()));
    }
    Ok(assignments)
}

pub(super) fn build_db_rows(
    request: &JobRequest,
    analysis: &media::MediaAnalysis,
    result: &media::ProcessingResult,
    uploads: Vec<telegram::UploadedFile>,
) -> (
    db::NewJob,
    Vec<db::NewTrack>,
    Vec<db::NewSegment>,
    Vec<db::NewSegmentPart>,
) {
    let video = analysis.video_streams.first();
    let metadata = &request.metadata;
    let is_series = metadata.is_series.unwrap_or(false);
    let media_type = metadata
        .media_type
        .clone()
        .unwrap_or_else(|| if is_series { "Series" } else { "Film" }.into());
    let mut job = db::NewJob {
        job_id: request.job_id.clone(),
        filename: metadata
            .title
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| request.filename.clone()),
        duration: analysis.duration,
        file_size: analysis.file_size as i64,
        video_codec: video
            .map(|v| v.codec_name.clone())
            .unwrap_or_else(|| "unknown".into()),
        video_width: video.map(|v| v.width).unwrap_or(0),
        video_height: video.map(|v| v.height).unwrap_or(0),
        status: "complete".into(),
        media_type,
        series_name: metadata.series_name.clone().unwrap_or_default(),
        has_thumbnail: result.thumbnail_path.is_some(),
        is_series,
        season_number: metadata.season_number.map(i64::from),
        episode_number: metadata.episode_number.map(i64::from),
        part_number: metadata.part_number.map(i64::from),
        source_path: request.original_source_path.clone(),
    };
    if !job.is_series {
        job.series_name.clear();
    }

    let mut tracks = Vec::new();
    let source_video_index = video.map(|v| v.index).unwrap_or(-1);
    for (idx, playlist) in result.video_playlists.iter().enumerate() {
        tracks.push(db::NewTrack {
            track_type: "video".into(),
            track_index: idx as i64,
            codec: if playlist.bitrate == "copy" {
                job.video_codec.clone()
            } else {
                "h264".into()
            },
            language: "und".into(),
            title: String::new(),
            channels: 0,
            width: playlist.width,
            height: playlist.height,
            bitrate: playlist.bitrate.clone(),
            original_stream_index: source_video_index,
        });
    }
    for (idx, playlist) in result.audio_playlists.iter().enumerate() {
        let source = analysis.audio_streams.get(idx);
        tracks.push(db::NewTrack {
            track_type: "audio".into(),
            track_index: idx as i64,
            codec: "aac".into(),
            language: playlist.language.clone(),
            title: playlist.title.clone(),
            channels: playlist.channels,
            width: 0,
            height: 0,
            bitrate: String::new(),
            original_stream_index: source.map(|a| a.index).unwrap_or(-1),
        });
    }
    for sub in &result.subtitle_files {
        tracks.push(db::NewTrack {
            track_type: "subtitle".into(),
            track_index: sub.enum_idx as i64,
            codec: "webvtt".into(),
            language: sub.language.clone(),
            title: sub.title.clone(),
            channels: 0,
            width: 0,
            height: 0,
            bitrate: String::new(),
            original_stream_index: sub.original_stream_idx,
        });
    }

    let mut segments = Vec::new();
    let mut segment_parts = Vec::new();
    let mut split_segments: HashMap<String, (i64, i64)> = HashMap::new();

    for uploaded in uploads {
        if uploaded.segment_key.contains("/part_") {
            let parts: Vec<&str> = uploaded.segment_key.split("/part_").collect();
            if parts.len() == 2 {
                let logical_key = parts[0].to_string();
                let part_index: i64 = parts[1].parse().unwrap_or(0);
                let entry = split_segments
                    .entry(logical_key.clone())
                    .or_insert((0, uploaded.bot_index));
                entry.0 += uploaded.file_size as i64;
                segment_parts.push(db::NewSegmentPart {
                    job_id: job.job_id.clone(),
                    segment_key: logical_key,
                    part_index,
                    file_id: uploaded.file_id,
                    bot_index: uploaded.bot_index,
                    file_size: uploaded.file_size as i64,
                });
            }
        } else {
            let duration = if uploaded.segment_key.ends_with("/init.mp4") {
                None
            } else {
                result.segment_durations.get(&uploaded.segment_key).copied()
            };
            segments.push(db::NewSegment {
                segment_key: uploaded.segment_key,
                file_id: uploaded.file_id,
                bot_index: uploaded.bot_index,
                file_size: uploaded.file_size as i64,
                duration,
                is_split: false,
            });
        }
    }

    for (segment_key, (file_size, bot_index)) in split_segments {
        segments.push(db::NewSegment {
            duration: result.segment_durations.get(&segment_key).copied(),
            segment_key,
            file_id: String::new(),
            bot_index,
            file_size,
            is_split: true,
        });
    }

    (job, tracks, segments, segment_parts)
}

async fn job_cancelled(state: &AppState, job_id: &str) -> bool {
    let jobs = state.jobs.lock().await;
    jobs.get(job_id)
        .map(|j| j.cancel_requested || j.status == JobStatus::Cancelled)
        .unwrap_or(true)
}

pub(super) async fn finish_job_complete(state: &AppState, job_id: &str) {
    let should_send = {
        let mut jobs = state.jobs.lock().await;
        let Some(job) = jobs.get_mut(job_id) else {
            return;
        };
        if job.status == JobStatus::Cancelled {
            return;
        }
        tracing::info!(job_id = %job_id, "job marked complete");
        job.status = JobStatus::Complete;
        job.progress = 100.0;
        job.step = 5;
        job.description = "complete".into();
        job.error = None;
        job.finished_at = Some(Instant::now());
        true
    };
    if should_send {
        send_job_webhook(state, job_id, JobStatus::Complete, None).await;
    }
}

pub(super) async fn finish_job_error(state: &AppState, job_id: &str, error: String) {
    let should_send = {
        let mut jobs = state.jobs.lock().await;
        let Some(job) = jobs.get_mut(job_id) else {
            return;
        };
        if job.status == JobStatus::Cancelled {
            return;
        }
        tracing::error!(job_id = %job_id, error = %error, "job failed");
        job.status = JobStatus::Error;
        job.progress = 100.0;
        job.description = error.clone();
        job.error = Some(error.clone());
        job.finished_at = Some(Instant::now());
        true
    };
    if should_send {
        if let Ok(conn) = state.db_conn().await {
            let job_id_for_db = job_id.to_string();
            let error_for_db = error.clone();
            let result = tokio::task::spawn_blocking(move || {
                db::mark_job_as_failed(&conn, &job_id_for_db, &error_for_db)
            })
            .await;
            if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
                tracing::warn!(job_id = %job_id, error = %e, "failed to mark job as error in DB");
            }
        }
        send_job_webhook(state, job_id, JobStatus::Error, Some(error)).await;
    }
}

async fn job_timeout_watcher(state: Arc<AppState>) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        cleanup_old_terminal_jobs(&state).await;
        let (job_timeout, queue_timeout) = {
            let cfg = state.config.read().await;
            (
                Duration::from_secs(cfg.job_timeout_seconds as u64),
                Duration::from_secs(cfg.queue_timeout_seconds as u64),
            )
        };
        let cleanup = {
            let mut jobs = state.jobs.lock().await;
            let now = Instant::now();
            jobs.values_mut()
                .filter(|job| {
                    !job.status.is_terminal()
                        && job
                            .started_at
                            .map(|started| now.duration_since(started) > job_timeout)
                            .unwrap_or_else(|| now.duration_since(job.queued_at) > queue_timeout)
                })
                .map(|job| {
                    let job_id = job.job_id.clone();
                    job.cancel_flag.store(true, Ordering::Relaxed);
                    job.status = JobStatus::Error;
                    job.progress = 100.0;
                    job.description = "timed_out".into();
                    job.error = Some("timed_out".into());
                    job.finished_at = Some(now);
                    (
                        job_id,
                        job.source_path.clone(),
                        job.processing_path.clone(),
                        job.delete_source_on_finish,
                    )
                })
                .collect::<Vec<_>>()
        };
        for (job_id, source_path, processing_path, delete_source) in cleanup {
            if let Ok(conn) = state.db_conn().await {
                let job_id_for_db = job_id.clone();
                let result = tokio::task::spawn_blocking(move || {
                    db::mark_job_as_failed(&conn, &job_id_for_db, "timed_out")
                })
                .await;
                if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
                    tracing::warn!(job_id = %job_id, error = %e, "failed to persist timed-out job");
                }
            }
            cleanup_job_paths(&source_path, &processing_path, delete_source).await;
            send_job_webhook(&state, &job_id, JobStatus::Error, Some("timed_out".into())).await;
        }
        let retention_days = {
            let cfg = state.config.read().await;
            cfg.job_retention_days
        };
        if retention_days > 0 {
            match state.db_conn().await {
                Ok(conn) => {
                    let result = tokio::task::spawn_blocking(move || {
                        db::delete_old_jobs(&conn, retention_days as i64)
                    })
                    .await;
                    if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
                        tracing::warn!(error = %e, "failed to delete old jobs");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to acquire DB connection for retention cleanup")
                }
            }
        }
    }
}

async fn cleanup_old_terminal_jobs(state: &AppState) {
    let mut jobs = state.jobs.lock().await;
    let now = Instant::now();
    jobs.retain(|_, job| {
        !job.status.is_terminal()
            || job
                .finished_at
                .map(|finished| now.duration_since(finished) < Duration::from_secs(300))
                .unwrap_or(true)
    });
}

async fn cleanup_request_paths(request: &JobRequest, processing_path: &FsPath) {
    cleanup_job_paths(
        &request.source_path,
        processing_path,
        request.delete_source_on_finish,
    )
    .await;
}

pub(super) async fn cleanup_job_paths(
    source_path: &FsPath,
    processing_path: &FsPath,
    delete_source: bool,
) {
    if delete_source {
        defer_source_delete(source_path).await;
    }
    let _ = tokio::fs::remove_dir_all(processing_path).await;
}

async fn defer_source_delete(source_path: &FsPath) {
    if source_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".pending_delete"))
    {
        return;
    }
    let Some(file_name) = source_path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let pending_path = source_path.with_file_name(format!("{file_name}.pending_delete"));
    match tokio::fs::rename(source_path, &pending_path).await {
        Ok(()) => {
            tracing::info!(
                source_path = %source_path.display(),
                pending_path = %pending_path.display(),
                "source deletion deferred"
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(
                source_path = %source_path.display(),
                pending_path = %pending_path.display(),
                error = %e,
                "failed to defer source deletion"
            );
        }
    }
}

pub(super) async fn send_job_webhook(
    state: &AppState,
    job_id: &str,
    status: JobStatus,
    error: Option<String>,
) {
    let url = {
        let cfg = state.config.read().await;
        cfg.webhook_url.trim().to_string()
    };
    if url.is_empty() {
        return;
    }

    let error_text = error.as_deref();
    let payload = {
        let jobs = state.jobs.lock().await;
        if let Some(job) = jobs.get(job_id) {
            json!({
                "event": "job_terminal",
                "job_id": job_id,
                "status": status.as_str(),
                "filename": job.filename,
                "media_type": job.metadata.media_type,
                "series_name": job.metadata.series_name,
                "error": error_text,
            })
        } else {
            drop(jobs);
            match state.db_conn().await {
                Ok(conn) => match db::get_job(&conn, job_id) {
                    Ok(Some(job)) => json!({
                        "event": "job_terminal",
                        "job_id": job_id,
                        "status": status.as_str(),
                        "filename": job.filename,
                        "media_type": job.media_type,
                        "series_name": job.series_name,
                        "error": error_text,
                    }),
                    _ => json!({
                        "event": "job_terminal",
                        "job_id": job_id,
                        "status": status.as_str(),
                        "error": error_text,
                    }),
                },
                Err(e) => {
                    tracing::warn!(job_id = %job_id, error = %e, "failed to acquire DB connection for job webhook payload");
                    json!({
                        "event": "job_terminal",
                        "job_id": job_id,
                        "status": status.as_str(),
                        "error": error_text,
                    })
                }
            }
        }
    };

    match state.http.post(&url).json(&payload).send().await {
        Ok(resp) if !resp.status().is_success() => {
            tracing::warn!(job_id = %job_id, status = %resp.status(), "job webhook failed");
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(job_id = %job_id, error = %e, "job webhook failed");
        }
    }
}

pub async fn recover_stuck_processing_jobs(state: &Arc<AppState>) {
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => {
            tracing::error!(error = %e, "failed to acquire DB connection for stuck job recovery");
            return;
        }
    };
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<String>> {
        let stuck = db::get_stuck_processing_jobs(&conn)?;
        for job_id in &stuck {
            db::mark_job_as_failed(&conn, job_id, "Server restart - job interrupted")?;
        }
        Ok(stuck)
    })
    .await;
    let stuck = match result {
        Ok(Ok(jobs)) => jobs,
        Ok(Err(e)) => {
            tracing::error!(error = %e, "failed to query stuck jobs");
            return;
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to query stuck jobs");
            return;
        }
    };
    if stuck.is_empty() {
        return;
    }
    tracing::warn!(
        count = stuck.len(),
        "found stuck non-terminal jobs (queued/downloading/analyzing/processing/uploading), marking as failed"
    );
}

pub async fn clean_orphaned_processing_dirs(state: &Arc<AppState>) {
    let Ok(mut entries) = tokio::fs::read_dir(&state.processing_dir).await else {
        return;
    };
    // Single batch query: collect all non-terminal job_ids to know what's still active.
    let active_ids: HashSet<String> = match state.db_conn().await {
        Ok(conn) => {
            match tokio::task::spawn_blocking(move || {
                let mut stmt = conn.prepare(
                    "SELECT job_id FROM jobs WHERE status IN ('queued','downloading','analyzing','processing','uploading')",
                )?;
                let ids: Result<HashSet<String>, rusqlite::Error> = stmt
                    .query_map([], |r| r.get(0))?
                    .collect();
                ids
            })
            .await
            {
                Ok(Ok(ids)) => ids,
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "processing directory cleanup batch query failed");
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "processing directory cleanup batch task failed");
                    return;
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to acquire DB connection for processing directory cleanup");
            return;
        }
    };
    let mut cleaned = 0u32;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !active_ids.contains(dir_name) {
            let _ = tokio::fs::remove_dir_all(&path).await;
            cleaned += 1;
            tracing::info!(dir = %dir_name, "cleaned orphaned processing directory");
        }
    }
    if cleaned > 0 {
        tracing::info!(cleaned, "processing directory cleanup complete");
    }
}

async fn detect_and_save_markers(
    state: &Arc<AppState>,
    request: &JobRequest,
    analysis: &media::MediaAnalysis,
) {
    let cfg = state.config.read().await.clone();
    if !cfg.intro_detection_enabled {
        return;
    }
    let metadata = &request.metadata;
    let media_type = metadata.media_type.as_deref().unwrap_or("Film");
    let series_name = metadata.series_name.as_deref().unwrap_or("");
    let season_number = metadata.season_number.map(i64::from);

    let fingerprints = if series_name.is_empty() {
        Vec::new()
    } else {
        match state.db_conn().await {
            Ok(conn) => {
                let mt = media_type.to_string();
                let sn = series_name.to_string();
                let sn_copy = season_number;
                match tokio::task::spawn_blocking(move || {
                    let mut rows = db::get_media_fingerprints_for_series_window(
                        &conn, &mt, &sn, sn_copy, "intro",
                    )?;
                    rows.extend(db::get_media_fingerprints_for_series_window(
                        &conn, &mt, &sn, sn_copy, "outro",
                    )?);
                    Ok::<_, anyhow::Error>(rows)
                })
                .await
                {
                    Ok(Ok(fps)) => fps,
                    _ => Vec::new(),
                }
            }
            Err(e) => {
                tracing::warn!(job_id = %request.job_id, error = %e, "marker detection: db unavailable");
                return;
            }
        }
    };

    let cancel = Arc::new(AtomicBool::new(false));

    let new_fingerprints = if cfg.intro_chromaprint_enabled
        && !series_name.is_empty()
        && media::chromaprint_available()
    {
        match media::generate_fingerprints(&request.source_path, analysis.duration, &cancel).await {
            Ok(fp) => fp,
            Err(e) => {
                tracing::debug!(job_id = %request.job_id, error = %e, "chromaprint fingerprint generation skipped");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    match media::detect_markers(
        analysis,
        &fingerprints,
        &new_fingerprints,
        Some(&request.source_path),
        &cancel,
    )
    .await
    {
        Ok(result) => {
            let jid = request.job_id.clone();
            if !result.markers.is_empty() {
                let markers_copy = result.markers.clone();
                if let Ok(conn) = state.db_conn().await {
                    let _ = tokio::task::spawn_blocking(move || {
                        db::replace_auto_media_markers(&conn, &jid, &markers_copy)
                    })
                    .await;
                }
            }
            if !new_fingerprints.is_empty() {
                if let Ok(conn) = state.db_conn().await {
                    let fingerprint_entries: Vec<_> = new_fingerprints
                        .iter()
                        .map(|fp| db::NewMediaFingerprint {
                            job_id: request.job_id.clone(),
                            media_type: media_type.to_string(),
                            series_name: series_name.to_string(),
                            season_number,
                            window_type: fp.window_type.clone(),
                            window_start_seconds: fp.window_start_seconds,
                            window_duration_seconds: fp.window_duration_seconds,
                            duration_seconds: analysis.duration,
                            fingerprint: fp.fingerprint.clone(),
                            fingerprint_source: "chromaprint".to_string(),
                        })
                        .collect();
                    let _ = tokio::task::spawn_blocking(move || {
                        for entry in &fingerprint_entries {
                            db::save_media_fingerprint(&conn, entry)?;
                        }
                        Ok::<_, anyhow::Error>(())
                    })
                    .await;
                }
            }
        }
        Err(e) => {
            tracing::warn!(job_id = %request.job_id, error = %e, "marker detection failed (non-fatal)");
        }
    }
}

async fn auto_fetch_metadata_if_enabled(state: &Arc<AppState>, request: &JobRequest) {
    let cfg = state.config.read().await.clone();
    if !cfg.metadata_auto_fetch_enabled {
        return;
    }

    // Skip if already linked.
    let conn = match state.db_conn().await {
        Ok(c) => c,
        Err(_) => return,
    };
    let jid = request.job_id.clone();
    let already_linked = tokio::task::spawn_blocking(move || {
        db::get_job_metadata_links(&conn, &jid).map(|l| !l.is_empty())
    })
    .await
    .ok()
    .and_then(|r| r.ok())
    .unwrap_or(false);
    if already_linked {
        return;
    }

    let media_type = request.metadata.media_type.as_deref().unwrap_or("Film");
    let search_term = request
        .metadata
        .series_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| request.metadata.title.as_deref().filter(|s| !s.is_empty()))
        .unwrap_or("");
    if search_term.is_empty() {
        return;
    }

    super::super::metadata::auto_fetch_and_link(state, &request.job_id, search_term, media_type)
        .await;
}
