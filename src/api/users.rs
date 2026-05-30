use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use super::auth::{self, AuthUser};
use super::{api_error, db_unavailable, AppState};
use crate::db;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    username: String,
    password: String,
    #[serde(default)]
    is_admin: bool,
}

pub async fn handle_login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> Response {
    let username = body.username.trim().to_string();
    if username.is_empty() || body.password.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_login",
            "username and password are required",
        );
    }

    let cfg = state.config.read().await.clone();
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let password = body.password;
    let result =
        tokio::task::spawn_blocking(move || -> anyhow::Result<Option<(db::UserRow, String)>> {
            let user_count = db::count_users(&conn)?;
            if user_count == 0 {
                if cfg.admin_user.is_empty()
                    || !constant_time_str_eq(&username, &cfg.admin_user)
                    || !constant_time_str_eq(&password, &cfg.admin_pass)
                {
                    return Ok(None);
                }
                let hash = db::hash_password(&password)?;
                let user = db::create_user(&conn, &username, &hash, true)?;
                let token = db::new_session_token();
                db::create_session(&conn, &user.user_id, &token)?;
                db::touch_user_seen(&conn, &user.user_id)?;
                return Ok(Some((user, token)));
            }

            let Some(user) = db::get_user_by_username(&conn, &username)? else {
                return Ok(None);
            };
            if !db::verify_password(&password, &user.password_hash)? {
                return Ok(None);
            }
            let token = db::new_session_token();
            db::create_session(&conn, &user.user_id, &token)?;
            db::touch_user_seen(&conn, &user.user_id)?;
            Ok(Some((user, token)))
        })
        .await;

    match result {
        Ok(Ok(Some((user, token)))) => (
            [(
                header::SET_COOKIE,
                format!("session={token}; Path=/; Max-Age=2592000; HttpOnly; SameSite=Strict"),
            )],
            Json(json!({
                "authenticated": true,
                "user": public_user_json(&user),
            })),
        )
            .into_response(),
        Ok(Ok(None)) => api_error(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "invalid username or password",
        ),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "login_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "login_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::session_cookie(&headers).map(ToOwned::to_owned) {
        if let Ok(conn) = state.db_conn().await {
            let _ = tokio::task::spawn_blocking(move || db::delete_session(&conn, &token)).await;
        }
    }
    (
        [(
            header::SET_COOKIE,
            "session=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict".to_string(),
        )],
        Json(json!({ "logged_out": true })),
    )
        .into_response()
}

pub async fn handle_me(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    match auth::current_user_from_headers(&state, &headers).await {
        Some(user) => Json(json!({
            "authenticated": true,
            "user": {
                "user_id": user.user_id,
                "username": user.username,
                "is_admin": user.is_admin,
            }
        }))
        .into_response(),
        None => Json(json!({ "authenticated": false, "user": Value::Null })).into_response(),
    }
}

pub async fn handle_list_users(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthUser>,
) -> Response {
    if !auth.is_admin {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin required");
    }
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let result = tokio::task::spawn_blocking(move || db::list_users(&conn)).await;
    match result {
        Ok(Ok(users)) => Json(json!({
            "users": users.iter().map(public_user_json).collect::<Vec<_>>(),
        }))
        .into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "user_list_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "user_list_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_create_user(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthUser>,
    Json(body): Json<CreateUserRequest>,
) -> Response {
    if !auth.is_admin {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin required");
    }
    let username = body.username.trim().to_string();
    if username.is_empty() || username.len() > 64 || body.password.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_user",
            "username must be 1-64 characters and password is required",
        );
    }
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let result = tokio::task::spawn_blocking(move || {
        let hash = db::hash_password(&body.password)?;
        db::create_user(&conn, &username, &hash, body.is_admin)
    })
    .await;
    match result {
        Ok(Ok(user)) => (StatusCode::CREATED, Json(public_user_json(&user))).into_response(),
        Ok(Err(e)) => api_error(StatusCode::BAD_REQUEST, "user_create_failed", e.to_string()),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "user_create_failed",
            e.to_string(),
        ),
    }
}

pub async fn handle_delete_user(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthUser>,
    Path(user_id): Path<String>,
) -> Response {
    if !auth.is_admin {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin required");
    }
    if user_id == auth.user_id {
        return api_error(
            StatusCode::BAD_REQUEST,
            "cannot_delete_self",
            "cannot delete the current user",
        );
    }
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let result = tokio::task::spawn_blocking(move || db::delete_user(&conn, &user_id)).await;
    match result {
        Ok(Ok(deleted)) => Json(json!({ "deleted": deleted })).into_response(),
        Ok(Err(e)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "user_delete_failed",
            e.to_string(),
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "user_delete_failed",
            e.to_string(),
        ),
    }
}

fn public_user_json(user: &db::UserRow) -> Value {
    json!({
        "user_id": user.user_id,
        "username": user.username,
        "is_admin": user.is_admin,
        "created_at": user.created_at,
        "last_seen_at": user.last_seen_at,
    })
}

fn constant_time_str_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let max_len = left.len().max(right.len());
    let mut diff = left.len() ^ right.len();
    for i in 0..max_len {
        let a = left.get(i).copied().unwrap_or(0);
        let b = right.get(i).copied().unwrap_or(0);
        diff |= (a ^ b) as usize;
    }
    diff == 0
}
