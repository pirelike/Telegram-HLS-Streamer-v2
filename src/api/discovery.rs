use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::json;

use super::auth::AuthUser;
use super::jobs::json::{job_json, normalize_category};
use super::{api_error, db_unavailable, AppState};
use crate::db;

#[derive(Debug, Deserialize)]
pub struct NextUpQuery {
    series_name: String,
    media_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RecentlyAddedQuery {
    limit: Option<i64>,
}

pub async fn handle_next_up(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthUser>,
    Query(query): Query<NextUpQuery>,
) -> Response {
    let series_name = query.series_name.trim().to_string();
    if series_name.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_series",
            "series_name is required",
        );
    }
    let media_type = match normalize_category(query.media_type.as_deref()) {
        Ok(Some(value)) => value,
        Ok(None) => "Series".to_string(),
        Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_media_type", e),
    };
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let user_id = auth.user_id;
    let result = tokio::task::spawn_blocking(move || {
        db::next_unwatched_episode(&conn, &user_id, &media_type, &series_name)
    })
    .await;
    match result {
        Ok(Ok(Some(job))) => Json(json!({ "job": job_json(job, None) })).into_response(),
        Ok(Ok(None)) => Json(json!({ "job": null })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "next_up_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "next_up_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_recently_added(
    State(state): State<Arc<AppState>>,
    Query(query): Query<RecentlyAddedQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let filter = db::JobListFilter {
        limit,
        offset: 0,
        ..Default::default()
    };
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let result = tokio::task::spawn_blocking(move || db::list_jobs(&conn, &filter)).await;
    match result {
        Ok(Ok(jobs)) => Json(json!({
            "jobs": jobs.into_iter().map(|job| job_json(job, None)).collect::<Vec<_>>(),
        }))
        .into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "recently_added_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "recently_added_failed",
            e.to_string(),
        ),
    }
}
