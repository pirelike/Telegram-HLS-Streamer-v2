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

pub async fn handle_toggle_favorite(
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
        tokio::task::spawn_blocking(move || db::toggle_favorite(&conn, &user_id, &job_id)).await;
    match result {
        Ok(Ok(active)) => Json(json!({ "favorite": active })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "favorite_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "favorite_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_list_favorites(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthUser>,
) -> Response {
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let user_id = auth.user_id;
    let result = tokio::task::spawn_blocking(move || db::list_favorites(&conn, &user_id)).await;
    match result {
        Ok(Ok(rows)) => Json(json!({
            "jobs": rows.into_iter().map(|row| {
                let mut value = job_json(row.job, None);
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("favorited_at".to_string(), json!(row.marked_at));
                }
                value
            }).collect::<Vec<_>>(),
        }))
        .into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "favorites_list_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "favorites_list_failed",
            e.to_string(),
        ),
    }
}
