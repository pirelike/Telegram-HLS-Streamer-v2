use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{api_error, db_unavailable, valid_job_id, AppState};
use crate::db;

#[derive(Debug, Deserialize)]
pub struct ProgressQuery {
    client_id: Option<String>,
}

pub async fn handle_list_progress(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProgressQuery>,
) -> Response {
    let client_id = match validate_client_id_param(query.client_id) {
        Ok(id) => id,
        Err(response) => return response,
    };

    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let cid = client_id.clone();
    let result = tokio::task::spawn_blocking(move || db::list_playback_progress(&conn, &cid)).await;
    match result {
        Ok(Ok(progress)) => {
            Json(json!({ "progress": progress, "client_id": client_id })).into_response()
        }
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "progress_list_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "progress_list_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_get_progress(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Query(query): Query<ProgressQuery>,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let client_id = match validate_client_id_param(query.client_id) {
        Ok(id) => id,
        Err(response) => return response,
    };

    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let cid = client_id.clone();
    let jid = job_id.clone();
    let result =
        tokio::task::spawn_blocking(move || db::get_playback_progress(&conn, &cid, &jid)).await;
    match result {
        Ok(Ok(Some(progress))) => {
            Json(json!({ "progress": progress, "client_id": client_id })).into_response()
        }
        Ok(Ok(None)) => Json(json!({ "progress": null, "client_id": client_id })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "progress_get_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "progress_get_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_save_progress(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Json(body): Json<Map<String, Value>>,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }

    let client_id = match extract_string(&body, "client_id") {
        Some(id) if !id.is_empty() => id,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "missing_client_id",
                "client_id is required",
            )
        }
    };
    if !valid_client_id(&client_id) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_id",
            "client_id must be 1-128 alphanumeric, hyphens, or underscores",
        );
    }

    let position_seconds = match extract_f64(&body, "position_seconds") {
        Some(v) if v >= 0.0 => v,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_position",
                "position_seconds must be a non-negative number",
            )
        }
    };

    let duration_seconds = match extract_f64(&body, "duration_seconds") {
        Some(v) if v > 0.0 => v,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_duration",
                "duration_seconds must be a positive number",
            )
        }
    };

    let progress = db::NewPlaybackProgress {
        client_id,
        job_id: job_id.clone(),
        position_seconds,
        duration_seconds,
    };

    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let result =
        tokio::task::spawn_blocking(move || db::save_playback_progress(&conn, &progress)).await;
    match result {
        Ok(Ok(())) => Json(json!({ "saved": true })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "progress_save_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "progress_save_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_delete_progress(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Query(query): Query<ProgressQuery>,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let client_id = match validate_client_id_param(query.client_id) {
        Ok(id) => id,
        Err(response) => return response,
    };

    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let cid = client_id.clone();
    let jid = job_id.clone();
    let result =
        tokio::task::spawn_blocking(move || db::delete_playback_progress(&conn, &cid, &jid)).await;
    match result {
        Ok(Ok(deleted)) => Json(json!({ "deleted": deleted })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "progress_delete_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "progress_delete_failed",
            e.to_string(),
        ),
    }
}

fn validate_client_id_param(value: Option<String>) -> Result<String, Response> {
    let id = match value.filter(|s| !s.is_empty()) {
        Some(id) => id,
        None => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "missing_client_id",
                "client_id query parameter is required",
            ))
        }
    };
    if !valid_client_id(&id) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_id",
            "client_id must be 1-128 alphanumeric, hyphens, or underscores",
        ));
    }
    Ok(id)
}

fn valid_client_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn extract_string(map: &Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Value::String(s)) => Some(s.trim().to_string()),
        _ => None,
    }
}

fn extract_f64(map: &Map<String, Value>, key: &str) -> Option<f64> {
    match map.get(key) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}
