use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::json;
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
) -> Result<String> {
    let job_id = uuid::Uuid::new_v4().simple().to_string();
    let processing_path = state.processing_dir.join(&job_id);
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
    };
    state.jobs.lock().await.insert(job_id.clone(), job);

    let request = JobRequest {
        job_id: job_id.clone(),
        filename,
        source_path,
        metadata,
        delete_source_on_finish,
    };
    tracing::info!(
        job_id = %job_id,
        filename = %request.filename,
        source_path = %request.source_path.display(),
        delete_source_on_finish,
        "job enqueued"
    );
    if state.job_queue.send(request).await.is_err() {
        state.jobs.lock().await.remove(&job_id);
        bail!("job queue is unavailable");
    }
    Ok(job_id)
}

pub(crate) fn start_background_tasks(state: Arc<AppState>, receiver: mpsc::Receiver<JobRequest>) {
    tokio::spawn(job_dispatcher(state.clone(), receiver));
    tokio::spawn(super::super::uploads::upload_sweeper(state.clone()));
    tokio::spawn(super::super::watch_folder::watch_folder_poller(
        state.clone(),
    ));
    tokio::spawn(job_timeout_watcher(state));
}

async fn job_dispatcher(state: Arc<AppState>, mut receiver: mpsc::Receiver<JobRequest>) {
    let max = state.config.read().await.max_concurrent_jobs.max(1) as usize;
    let semaphore = Arc::new(Semaphore::new(max));
    while let Some(job) = receiver.recv().await {
        let Ok(permit) = semaphore.clone().acquire_owned().await else {
            break;
        };
        let state = state.clone();
        tokio::spawn(async move {
            process_job(state, job).await;
            drop(permit);
        });
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
        "media processing complete"
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
        job.status = JobStatus::Uploading;
        job.progress = 80.0;
        job.step = 4;
        job.description = "uploading to Telegram".into();
    }
    tracing::info!(job_id = %request.job_id, "job telegram upload started");

    let uploads = match upload_outputs(state.clone(), &cfg, &result, &cancel_flag).await {
        Ok(uploads) => uploads,
        Err(e) => {
            finish_job_error(&state, &request.job_id, format!("upload_failed: {e}")).await;
            cleanup_request_paths(&request, &processing_path).await;
            return;
        }
    };
    if job_cancelled(&state, &request.job_id).await {
        cleanup_request_paths(&request, &processing_path).await;
        return;
    }

    let (job, tracks, segments, segment_parts) =
        build_db_rows(&request, &analysis, &result, uploads);
    {
        let mut conn = state.db.lock().await;
        tracing::info!(
            job_id = %request.job_id,
            tracks = tracks.len(),
            segments = segments.len(),
            segment_parts = segment_parts.len(),
            "job db save started"
        );
        if let Err(e) = db::save_job(&mut conn, &job, &tracks, &segments, &segment_parts) {
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
    cleanup_request_paths(&request, &processing_path).await;
}

async fn upload_outputs(
    state: Arc<AppState>,
    cfg: &Config,
    result: &media::ProcessingResult,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<Vec<telegram::UploadedFile>> {
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
    let parallelism = cfg
        .upload_parallelism
        .max(1)
        .min(assignments.len() as u32)
        .min(files.len() as u32) as usize;
    let semaphore = Arc::new(Semaphore::new(parallelism));
    let mut tasks = JoinSet::new();

    for ((segment_key, path), (bot_index, bot)) in files.into_iter().zip(assignments) {
        let permit = semaphore.clone().acquire_owned().await?;
        let state = state.clone();
        let base_url = state.telegram_base_url.clone();
        let client = state.http.clone();
        let max_file_size = cfg.telegram_max_file_size;
        tasks.spawn(async move {
            let _permit = permit;
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
    Ok(uploaded)
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

    let chunk_size = (max_size as f64 * 0.95) as usize;
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;

    let total_parts = (file_size as f64 / chunk_size as f64).ceil() as i64;
    let mut parts = Vec::new();

    tracing::info!(
        segment_key,
        file_size,
        chunk_size,
        total_parts,
        "splitting oversized segment"
    );

    for (i, chunk) in bytes.chunks(chunk_size).enumerate() {
        let part_index = i as i64;
        let part_key = format!("{}/part_{}", segment_key, part_index);
        let part_path = temp_dir.join(part_index.to_string());
        tokio::fs::write(&part_path, chunk)
            .await
            .with_context(|| format!("write part {}", part_path.display()))?;
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
    let _guard = state.telegram.round_robin.lock().await;
    let conn = state.db.lock().await;
    let last = db::get_last_bot_index(&conn)?;
    let mut assignments = Vec::with_capacity(file_count);
    let bot_count = cfg.bots.len() as i64;
    for offset in 0..file_count {
        let index = (last + 1 + offset as i64).rem_euclid(bot_count);
        assignments.push((index, cfg.bots[index as usize].clone()));
    }
    if let Some((last_index, _)) = assignments.last() {
        db::set_last_bot_index(&conn, *last_index)?;
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
            });
        }
    }

    for (segment_key, (file_size, bot_index)) in split_segments {
        segments.push(db::NewSegment {
            duration: result.segment_durations.get(&segment_key).copied(),
            segment_key,
            file_id: "split".into(),
            bot_index,
            file_size,
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
        send_job_webhook(state, job_id, JobStatus::Error, Some(error)).await;
    }
}

async fn job_timeout_watcher(state: Arc<AppState>) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        cleanup_old_terminal_jobs(&state).await;
        let timeout = {
            let cfg = state.config.read().await;
            Duration::from_secs(cfg.job_timeout_seconds as u64)
        };
        let cleanup = {
            let mut jobs = state.jobs.lock().await;
            let now = Instant::now();
            jobs.values_mut()
                .filter(|job| {
                    !job.status.is_terminal()
                        && job
                            .started_at
                            .map(|started| now.duration_since(started) > timeout)
                            .unwrap_or(false)
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
            cleanup_job_paths(&source_path, &processing_path, delete_source).await;
            send_job_webhook(&state, &job_id, JobStatus::Error, Some("timed_out".into())).await;
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
        let _ = tokio::fs::remove_file(source_path).await;
    }
    let _ = tokio::fs::remove_dir_all(processing_path).await;
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
            let conn = state.db.lock().await;
            match db::get_job(&conn, job_id) {
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
