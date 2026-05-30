use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{json, Map, Value};

use super::auth::AuthUser;
use super::{api_error, db_unavailable, AppState};
use crate::db;

const ALLOWED_KEYS: &[&str] = &[
    "audio_language",
    "subtitle_language",
    "autoplay_next",
    "default_quality",
    "skip_intro",
];

pub async fn handle_get_preferences(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthUser>,
) -> Response {
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let user_id = auth.user_id;
    let result =
        tokio::task::spawn_blocking(move || db::list_user_preferences(&conn, &user_id)).await;
    match result {
        Ok(Ok(rows)) => {
            let mut prefs = Map::new();
            for row in rows {
                prefs.insert(row.key, Value::String(row.value));
            }
            Json(json!({ "preferences": prefs })).into_response()
        }
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "preferences_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "preferences_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_patch_preferences(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthUser>,
    Json(body): Json<Map<String, Value>>,
) -> Response {
    let mut updates = Vec::new();
    for (key, value) in body {
        if !ALLOWED_KEYS.contains(&key.as_str()) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_preference",
                format!("unknown preference key: {key}"),
            );
        }
        updates.push((key, preference_value(value)));
    }
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let user_id = auth.user_id;
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        for (key, value) in updates {
            db::set_user_preference(&conn, &user_id, &key, &value)?;
        }
        Ok(())
    })
    .await;
    match result {
        Ok(Ok(())) => Json(json!({ "saved": true })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "preferences_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "preferences_failed",
            e.to_string(),
        ),
    }
}

fn preference_value(value: Value) -> String {
    match value {
        Value::Bool(true) => "1".to_string(),
        Value::Bool(false) => "0".to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
