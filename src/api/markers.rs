use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::{api_error, db_unavailable, valid_job_id, AppState};
use crate::db;

pub async fn handle_get_markers(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }

    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let jid = job_id.clone();
    let result =
        tokio::task::spawn_blocking(move || db::get_media_markers(&conn, &jid, true)).await;
    match result {
        Ok(Ok(markers)) => Json(json!({ "markers": markers, "job_id": job_id })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "markers_fetch_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "markers_fetch_failed",
            e.to_string(),
        ),
    }
}
