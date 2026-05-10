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
use super::processing::{cleanup_job_paths, enqueue_job, send_job_webhook};
use super::types::*;

use super::super::{api_error, valid_job_id as valid_slug_id, AppState};
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
        ))
        .into_response();
    }
    drop(jobs);

    let conn = state.db.lock().await;
    match db::get_job(&conn, &job_id) {
        Ok(Some(job)) => Json(json!({
            "job_id": job.job_id,
            "status": "complete",
            "progress": 100.0,
            "step": 5,
            "total_steps": 5,
            "description": "complete",
            "queue_position": Value::Null,
            "analysis": Value::Null,
            "error": Value::Null,
            "filename": job.filename,
            "duration": job.duration,
            "file_size": job.file_size,
            "created_at": job.created_at,
        }))
        .into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "not_found", "job not found"),
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

    let conn = state.db.lock().await;
    let result: anyhow::Result<(Vec<Value>, i64)> = (|| match group_by {
        Some("series") => {
            let rows = db::list_series_groups(&conn, &filter)?;
            let total = db::count_series_groups(&conn, &filter)?;
            Ok((rows.into_iter().map(series_group_json).collect(), total))
        }
        Some("season") => {
            let rows = db::list_season_groups(&conn, &filter)?;
            let total = db::count_season_groups(&conn, &filter)?;
            Ok((rows.into_iter().map(season_group_json).collect(), total))
        }
        _ => {
            let rows = db::list_jobs(&conn, &filter)?;
            let total = db::count_jobs(&conn, &filter)?;
            Ok((rows.into_iter().map(job_json).collect(), total))
        }
    })();
    match result {
        Ok((jobs, total)) => Json(json!({
            "jobs": jobs,
            "total": total,
            "page": page,
            "limit": limit,
            "has_more": page * limit < total,
        }))
        .into_response(),
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

    let conn = state.db.lock().await;
    let updated = match db::update_job_metadata_fields(
        &conn,
        &job_id,
        filename,
        media_type,
        series_name,
        is_series,
        season_number,
        episode_number,
        part_number,
    ) {
        Ok(Some(job)) => job,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "job not found"),
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "job_update_failed",
                e.to_string(),
            )
        }
    };
    Json(job_json(updated)).into_response()
}

pub async fn handle_delete_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_slug_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let conn = state.db.lock().await;
    match db::delete_job(&conn, &job_id) {
        Ok(true) => Json(json!({ "message": "deleted" })).into_response(),
        Ok(false) => api_error(StatusCode::NOT_FOUND, "not_found", "job not found"),
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
    let source = match download::reconstruct_job_source(&state, &job).await {
        Ok(path) => path,
        Err(e) => return api_error(StatusCode::BAD_GATEWAY, "reconstruct_failed", e.to_string()),
    };
    let metadata = download::metadata_from_job(&job);
    match enqueue_job(&state, job.filename.clone(), source, metadata, true).await {
        Ok(new_job_id) => Json(json!({
            "job_id": new_job_id,
            "source_job_id": job.job_id,
            "message": "queued",
        }))
        .into_response(),
        Err(e) => api_error(StatusCode::SERVICE_UNAVAILABLE, "queue_full", e.to_string()),
    }
}

pub async fn handle_cancel_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_slug_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let cleanup = {
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
        Some((
            job.source_path.clone(),
            job.processing_path.clone(),
            job.delete_source_on_finish,
        ))
    };
    if let Some((source_path, processing_path, delete_source)) = cleanup {
        cleanup_job_paths(&source_path, &processing_path, delete_source).await;
    }
    send_job_webhook(&state, &job_id, JobStatus::Cancelled, None).await;
    Json(json!({ "job_id": job_id, "status": "cancelled", "message": "cancelled" })).into_response()
}

pub async fn queue_metrics(state: &AppState) -> Value {
    let cfg = state.config.read().await;
    let jobs = state.jobs.lock().await;
    let running = jobs
        .values()
        .filter(|j| {
            matches!(
                j.status,
                JobStatus::Analyzing | JobStatus::Processing | JobStatus::Uploading
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
