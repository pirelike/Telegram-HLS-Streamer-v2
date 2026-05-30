use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::json;

use super::auth::AuthUser;
use super::jobs::json::job_json;
use super::{api_error, db_unavailable, valid_job_id, AppState};
use crate::db;

pub async fn handle_toggle_watchlist(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthUser>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let user_id = auth.user_id;
    let result =
        tokio::task::spawn_blocking(move || db::toggle_watchlist(&conn, &user_id, &job_id)).await;
    match result {
        Ok(Ok(active)) => Json(json!({ "watchlisted": active })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "watchlist_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "watchlist_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_list_watchlist(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthUser>,
) -> Response {
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let user_id = auth.user_id;
    let result = tokio::task::spawn_blocking(move || db::list_watchlist(&conn, &user_id)).await;
    match result {
        Ok(Ok(rows)) => Json(json!({
            "jobs": rows.into_iter().map(|row| {
                let mut value = job_json(row.job, None);
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("watchlisted_at".to_string(), json!(row.marked_at));
                }
                value
            }).collect::<Vec<_>>(),
        }))
        .into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "watchlist_list_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "watchlist_list_failed",
            e.to_string(),
        ),
    }
}
