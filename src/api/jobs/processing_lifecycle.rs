use std::collections::HashSet;
use std::path::Path as FsPath;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::json;

use super::super::AppState;
use super::types::*;
use crate::db;

pub(super) async fn job_cancelled(state: &AppState, job_id: &str) -> bool {
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

pub(super) async fn job_timeout_watcher(state: Arc<AppState>) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = state.shutdown_token.cancelled() => break,
        }
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

pub(super) async fn cleanup_old_terminal_jobs(state: &AppState) {
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

pub(super) async fn cleanup_request_paths(request: &JobRequest, processing_path: &FsPath) {
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

pub(super) async fn defer_source_delete(source_path: &FsPath) {
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
