use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::download;
use super::json::{self, job_json, season_group_json, series_group_json};
use super::processing::{enqueue_job, send_job_webhook};
use super::types::*;

use super::super::{api_error, db_unavailable, valid_job_id as valid_slug_id, AppState};
use crate::db;

pub async fn handle_job_status(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_slug_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let jobs = state.jobs.lock().await;
    if let Some(job) = jobs.get(&job_id) {
        return Json(json::job_status_json(
            job,
            json::queue_position(&jobs, &job_id),
            json::queue_depth(&jobs, &job_id),
        ))
        .into_response();
    }
    drop(jobs);

    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let job_id_for_db = job_id.clone();
    let lookup = tokio::task::spawn_blocking(move || db::get_job(&conn, &job_id_for_db)).await;
    match lookup {
        Ok(Ok(Some(job))) => {
            let (progress, step, total_steps, description) = match job.status.as_str() {
                "complete" => (100.0, 5, 5, "complete".to_string()),
                "error" => (100.0, 5, 5, "error".to_string()),
                "cancelled" => (100.0, 5, 5, "cancelled".to_string()),
                _ => (0.0, 0, 5, job.status.clone()),
            };
            let error_value = match &job.error {
                Some(e) => Value::String(e.clone()),
                None => Value::Null,
            };
            Json(json!({
                "job_id": job.job_id,
                "status": job.status,
                "progress": progress,
                "step": step,
                "total_steps": total_steps,
                "description": description,
                "queue_position": Value::Null,
                "queue_depth": Value::Null,
                "analysis": Value::Null,
                "error": error_value,
                "filename": job.filename,
                "duration": job.duration,
                "file_size": job.file_size,
                "created_at": job.created_at,
            }))
            .into_response()
        }
        Ok(Ok(None)) => api_error(StatusCode::NOT_FOUND, "not_found", "job not found"),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "job_lookup_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "job_lookup_failed",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct JobsQuery {
    page: Option<i64>,
    limit: Option<i64>,
    search: Option<String>,
    category: Option<String>,
    group_by: Option<String>,
    series_name: Option<String>,
    season_number: Option<String>,
}

pub async fn handle_list_jobs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<JobsQuery>,
) -> Response {
    let page = query.page.unwrap_or(1).max(1);
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let category = match json::normalize_category(query.category.as_deref()) {
        Ok(category) => category,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_category", e),
    };
    let group_by = match query.group_by.as_deref().filter(|s| !s.is_empty()) {
        Some("series") => Some("series"),
        Some("season") => Some("season"),
        Some(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_group_by",
                "group_by must be series or season",
            )
        }
        None => None,
    };
    let (season_number, season_number_is_null) = match query.season_number.as_deref() {
        Some("null") | Some("specials") | Some("") => (None, true),
        Some(raw) => match raw.parse::<i64>() {
            Ok(value) => (Some(value), false),
            Err(_) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_season_number",
                    "season_number must be an integer or null",
                )
            }
        },
        None => (None, false),
    };
    let filter = db::JobListFilter {
        limit,
        offset: (page - 1) * limit,
        search: query.search.filter(|s| !s.trim().is_empty()),
        category,
        series_name: query.series_name.filter(|s| !s.trim().is_empty()),
        season_number,
        season_number_is_null,
    };

    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let group_by = group_by.map(str::to_string);
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(Vec<Value>, i64)> {
        match group_by.as_deref() {
            Some("series") => {
                let rows = db::list_series_groups(&conn, &filter)?;
                let total = db::count_series_groups(&conn, &filter)?;
                let names: Vec<String> = rows.iter().map(|r| r.series_name.clone()).collect();
                let posters = db::get_series_poster_urls(&conn, &names)?;
                Ok((
                    rows.into_iter()
                        .map(|r| {
                            let p = posters.get(&r.series_name).map(String::as_str);
                            series_group_json(r, p)
                        })
                        .collect(),
                    total,
                ))
            }
            Some("season") => {
                let rows = db::list_season_groups(&conn, &filter)?;
                let total = db::count_season_groups(&conn, &filter)?;
                let names: Vec<String> = rows.iter().map(|r| r.series_name.clone()).collect();
                let posters = db::get_series_poster_urls(&conn, &names)?;
                Ok((
                    rows.into_iter()
                        .map(|r| {
                            let p = posters.get(&r.series_name).map(String::as_str);
                            season_group_json(r, p)
                        })
                        .collect(),
                    total,
                ))
            }
            _ => {
                let rows = db::list_jobs(&conn, &filter)?;
                let total = db::count_jobs(&conn, &filter)?;
                let ids: Vec<String> = rows.iter().map(|r| r.job_id.clone()).collect();
                let posters = db::get_job_poster_urls(&conn, &ids)?;
                Ok((
                    rows.into_iter()
                        .map(|r| {
                            let p = posters.get(&r.job_id).map(String::as_str);
                            job_json(r, p)
                        })
                        .collect(),
                    total,
                ))
            }
        }
    })
    .await;
    match result {
        Ok(Ok((jobs, total))) => Json(json!({
            "jobs": jobs,
            "total": total,
            "page": page,
            "limit": limit,
            "has_more": page * limit < total,
        }))
        .into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "job_list_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "job_list_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_get_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_slug_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    download::full_job_response(&state, &job_id).await
}

pub async fn handle_patch_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Json(body): Json<Map<String, Value>>,
) -> Response {
    if !valid_slug_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }

    let mut filename = None;
    let mut media_type = None;
    let mut series_name = None;
    let mut is_series = None;
    let mut season_number = None;
    let mut episode_number = None;
    let mut part_number = None;

    for (key, value) in body {
        match key.as_str() {
            "title" => match json::non_empty_string(&key, value) {
                Ok(v) => filename = Some(v),
                Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e),
            },
            "media_type" => {
                let category = match json::non_empty_string(&key, value) {
                    Ok(v) => v,
                    Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e),
                };
                media_type = match json::normalize_category(Some(&category)) {
                    Ok(Some(category)) => Some(category),
                    _ => {
                        return api_error(
                            StatusCode::BAD_REQUEST,
                            "invalid_category",
                            "media_type must be Film, Series, Anime Film, or Anime TV",
                        )
                    }
                };
            }
            "series_name" => match json::nullable_string(&key, value) {
                Ok(v) => series_name = Some(v),
                Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e),
            },
            "is_series" => match json::bool_value(&key, value) {
                Ok(v) => is_series = Some(v),
                Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e),
            },
            "season_number" => match json::nullable_i64(&key, value) {
                Ok(v) => season_number = Some(v),
                Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e),
            },
            "episode_number" => match json::nullable_i64(&key, value) {
                Ok(v) => episode_number = Some(v),
                Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e),
            },
            "part_number" => match json::nullable_i64(&key, value) {
                Ok(v) => part_number = Some(v),
                Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e),
            },
            _ => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_payload",
                    format!("unknown metadata field: {key}"),
                )
            }
        }
    }

    let mut conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let updated = match tokio::task::spawn_blocking(move || {
        db::update_job_metadata_fields(
            &mut conn,
            &job_id,
            filename,
            media_type,
            series_name,
            is_series,
            season_number,
            episode_number,
            part_number,
        )
    })
    .await
    {
        Ok(Ok(Some(job))) => job,
        Ok(Ok(None)) => return api_error(StatusCode::NOT_FOUND, "not_found", "job not found"),
        Ok(Err(e)) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "job_update_failed",
                e.to_string(),
            )
        }
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "job_update_failed",
                e.to_string(),
            )
        }
    };
    Json(job_json(updated, None)).into_response()
}

pub async fn handle_delete_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_slug_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    match tokio::task::spawn_blocking(move || db::delete_job(&conn, &job_id)).await {
        Ok(Ok(true)) => Json(json!({ "message": "deleted" })).into_response(),
        Ok(Ok(false)) => api_error(StatusCode::NOT_FOUND, "not_found", "job not found"),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "job_delete_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "job_delete_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_download_original(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_slug_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let job = match download::complete_job(&state, &job_id).await {
        Ok(job) => job,
        Err(response) => return response,
    };
    let path = match download::reconstruct_job_source(&state, &job).await {
        Ok(path) => path,
        Err(e) => return api_error(StatusCode::BAD_GATEWAY, "reconstruct_failed", e.to_string()),
    };
    download::stream_temp_file(
        path,
        &format!("{}.mp4", download::safe_download_name(&job.filename)),
    )
    .await
}

pub async fn handle_reprocess_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_slug_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let job = match download::complete_job(&state, &job_id).await {
        Ok(job) => job,
        Err(response) => return response,
    };
    let (source, delete_source) = match source_from_uploads(&state, &job).await {
        Some(path) => (path, false),
        None => match download::reconstruct_job_source(&state, &job).await {
            Ok(path) => (path, true),
            Err(e) => {
                return api_error(StatusCode::BAD_GATEWAY, "reconstruct_failed", e.to_string())
            }
        },
    };
    let metadata = download::metadata_from_job(&job);
    match enqueue_job(
        &state,
        job.filename.clone(),
        source,
        metadata,
        delete_source,
        job.source_path.clone(),
    )
    .await
    {
        Ok(new_job_id) => Json(json!({
            "job_id": new_job_id,
            "source_job_id": job.job_id,
            "message": "queued",
        }))
        .into_response(),
        Err(e) => api_error(StatusCode::SERVICE_UNAVAILABLE, "queue_full", e.to_string()),
    }
}

async fn source_from_uploads(state: &AppState, job: &db::JobRow) -> Option<std::path::PathBuf> {
    let rel = job.source_path.as_deref()?;
    if rel.contains("..") || rel.contains('/') || rel.contains('\\') || rel.contains('\0') {
        tracing::warn!(job_id = %job.job_id, source_path = %rel, "ignoring invalid stored source path");
        return None;
    }
    let path = state.uploads_dir.join(rel);
    match tokio::fs::metadata(&path).await {
        Ok(meta) if meta.is_file() => Some(path),
        _ => {
            let pending_path = state.uploads_dir.join(format!("{rel}.pending_delete"));
            match tokio::fs::metadata(&pending_path).await {
                Ok(meta) if meta.is_file() => Some(pending_path),
                _ => None,
            }
        }
    }
}

pub async fn handle_cancel_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_slug_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let cancelled = {
        let mut jobs = state.jobs.lock().await;
        let Some(job) = jobs.get_mut(&job_id) else {
            return api_error(StatusCode::NOT_FOUND, "not_found", "job not found");
        };
        if job.status.is_terminal() {
            return Json(json!({
                "job_id": job_id,
                "status": job.status.as_str(),
                "message": format!("already {}", job.status.as_str()),
            }))
            .into_response();
        }
        job.cancel_requested = true;
        job.cancel_flag.store(true, Ordering::Relaxed);
        job.status = JobStatus::Cancelled;
        job.progress = 100.0;
        job.description = "cancelled".into();
        job.finished_at = Some(Instant::now());
        true
    };
    if cancelled {
        if let Ok(conn) = state.db_conn().await {
            let job_id_clone = job_id.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Err(e) = db::mark_job_as_cancelled(&conn, &job_id_clone) {
                    tracing::warn!(job_id = %job_id_clone, error = %e, "failed to persist cancelled job");
                }
            })
            .await;
        }
    }
    send_job_webhook(&state, &job_id, JobStatus::Cancelled, None).await;
    Json(json!({ "job_id": job_id, "status": "cancelled", "message": "cancelled" })).into_response()
}

pub async fn handle_active_jobs(State(state): State<Arc<AppState>>) -> Response {
    let jobs = state.jobs.lock().await;
    let active: Vec<Value> = jobs
        .values()
        .filter(|j| !j.status.is_terminal())
        .map(|j| {
            json!({
                "job_id": j.job_id,
                "status": j.status.as_str(),
                "progress": j.progress,
                "description": j.description,
                "filename": j.filename,
            })
        })
        .collect();
    Json(json!({ "jobs": active })).into_response()
}

pub async fn queue_metrics(state: &AppState) -> Value {
    let cfg = state.config.read().await;
    let jobs = state.jobs.lock().await;
    let running = jobs
        .values()
        .filter(|j| {
            matches!(
                j.status,
                JobStatus::Downloading
                    | JobStatus::Analyzing
                    | JobStatus::Processing
                    | JobStatus::Uploading
            )
        })
        .count();
    let queued = jobs
        .values()
        .filter(|j| j.status == JobStatus::Queued)
        .count();
    json!({
        "running": running,
        "queued": queued,
        "max_concurrent": cfg.max_concurrent_jobs,
    })
}
