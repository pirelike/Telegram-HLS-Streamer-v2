use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use tokio::sync::{mpsc, Semaphore};

use super::super::AppState;
pub(super) use super::processing_lifecycle::send_job_webhook;
pub use super::processing_lifecycle::{
    clean_orphaned_processing_dirs, recover_stuck_processing_jobs,
};
use super::processing_lifecycle::{
    cleanup_request_paths, finish_job_complete, finish_job_error, job_cancelled,
    job_timeout_watcher,
};
use super::processing_markers::{
    auto_fetch_metadata_if_enabled, prepare_marker_detection, save_prepared_markers,
};
pub(super) use super::processing_upload::build_db_rows;
use super::processing_upload::upload_outputs;
#[cfg(test)]
pub(super) use super::processing_upload::{collect_upload_files, prepare_upload_files};
use super::types::*;
use crate::{db, media};

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

#[allow(clippy::too_many_arguments)] // job re-queue requires all scheduling context at once
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
            if job.status.is_terminal() {
                bail!("job already terminal: {:?}", job.status);
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

fn spawn_supervised<F, Fut>(
    name: &'static str,
    shutdown_token: tokio_util::sync::CancellationToken,
    mut make_future: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(8);
        loop {
            if shutdown_token.is_cancelled() {
                break;
            }
            let inner = tokio::spawn(make_future());
            // Wait for the inner task or a shutdown signal
            let result = tokio::select! {
                r = inner => r,
                _ = shutdown_token.cancelled() => break,
            };
            if shutdown_token.is_cancelled() {
                break;
            }
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
                    tracing::error!(
                        worker = name,
                        error = %e,
                        "worker join error, respawning in {:?}",
                        backoff
                    );
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = shutdown_token.cancelled() => break,
            }
            backoff = (backoff * 2).min(max_backoff);
        }
        tracing::info!(worker = name, "worker supervisor exiting");
    })
}

pub(crate) fn start_background_tasks(
    state: Arc<AppState>,
    receiver: mpsc::Receiver<JobRequest>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let shutdown = state.shutdown_token.clone();
    let dispatcher_state = state.clone();
    let dispatcher_handle = tokio::spawn(async move {
        let handle = tokio::spawn(job_dispatcher(dispatcher_state, receiver));
        match handle.await {
            Ok(()) => tracing::info!(worker = "job_dispatcher", "exited"),
            Err(e) => tracing::error!(worker = "job_dispatcher", "panicked: {e}"),
        }
    });
    let sweeper_handle = {
        let state = state.clone();
        let token = shutdown.clone();
        spawn_supervised("upload_sweeper", token, move || {
            let state = state.clone();
            async move { super::super::uploads::upload_sweeper(state).await }
        })
    };
    let poller_handle = {
        let state = state.clone();
        let token = shutdown.clone();
        spawn_supervised("watch_folder_poller", token, move || {
            let state = state.clone();
            async move { super::super::watch_folder::watch_folder_poller(state).await }
        })
    };
    let watcher_handle = {
        let state = state.clone();
        let token = shutdown.clone();
        spawn_supervised("job_timeout_watcher", token, move || {
            let state = state.clone();
            async move { job_timeout_watcher(state).await }
        })
    };
    vec![
        dispatcher_handle,
        sweeper_handle,
        poller_handle,
        watcher_handle,
    ]
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

#[tracing::instrument(name = "job", skip_all, fields(job_id = %request.job_id))]
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

    let (cfg, cancel_flag) = {
        let jobs = state.jobs.lock().await;
        let flag = jobs
            .get(&request.job_id)
            .map(|j| j.cancel_flag.clone())
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        (state.config.read().await.clone(), flag)
    };

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
        job.progress = 25.0;
        job.step = 2;
        job.description = "detecting intro/outro markers".into();
    }

    let prepared_markers =
        prepare_marker_detection(&state, &request, &analysis, &cfg, &cancel_flag).await;
    if job_cancelled(&state, &request.job_id).await {
        cleanup_request_paths(&request, &processing_path).await;
        return;
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
        job.progress = 30.0;
        job.description = "processing video, audio, subtitles, and thumbnail".into();
    }
    tracing::info!(job_id = %request.job_id, "job media processing started");

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
        "job db save complete"
    );

    save_prepared_markers(&state, &request, &analysis, prepared_markers).await;

    tracing::info!(
        job_id = %request.job_id,
        elapsed_ms = job_started.elapsed().as_millis(),
        "job cycle complete"
    );
    finish_job_complete(&state, &request.job_id).await;

    auto_fetch_metadata_if_enabled(&state, &request).await;

    super::super::playback::spawn_cache_warmup(state.clone(), request.job_id.clone());
    super::super::db_transfer::trigger_automatic_db_sync(
        state.clone(),
        format!("job-{}", request.job_id),
    );
    cleanup_request_paths(&request, &processing_path).await;
}
