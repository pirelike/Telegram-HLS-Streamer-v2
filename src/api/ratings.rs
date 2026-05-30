use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::json;

use super::auth::AuthUser;
use super::{api_error, db_unavailable, valid_job_id, AppState};
use crate::db;

#[derive(Debug, Deserialize)]
struct RatingRequest {
    liked: Option<bool>,
}

pub async fn handle_set_rating(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthUser>,
    Path(job_id): Path<String>,
    body: Bytes,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let liked = if body.is_empty() {
        None
    } else {
        match serde_json::from_slice::<RatingRequest>(&body) {
            Ok(payload) => payload.liked,
            Err(e) => {
                return api_error(StatusCode::BAD_REQUEST, "invalid_rating", e.to_string());
            }
        }
    };
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let user_id = auth.user_id;
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<bool>> {
        match liked {
            Some(value) => {
                db::set_rating(&conn, &user_id, &job_id, value)?;
                Ok(Some(value))
            }
            None => {
                db::delete_rating(&conn, &user_id, &job_id)?;
                Ok(None)
            }
        }
    })
    .await;
    match result {
        Ok(Ok(value)) => Json(json!({ "liked": value })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "rating_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "rating_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_list_ratings(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthUser>,
) -> Response {
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let user_id = auth.user_id;
    let result = tokio::task::spawn_blocking(move || db::list_ratings(&conn, &user_id)).await;
    match result {
        Ok(Ok(ratings)) => Json(json!({ "ratings": ratings })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "ratings_list_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "ratings_list_failed",
            e.to_string(),
        ),
    }
}
