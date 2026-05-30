use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, Request, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

use crate::api::AppState;
use crate::db;

#[derive(Debug, Clone)]
pub(crate) struct AuthUser {
    pub(crate) user_id: String,
    pub(crate) username: String,
    pub(crate) is_admin: bool,
}

pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    if public_path(request.uri().path()) {
        return next.run(request).await;
    }

    if let Some(user) = current_user_from_headers(&state, request.headers()).await {
        request.extensions_mut().insert(user);
        return next.run(request).await;
    }

    let cfg = state.config.read().await.clone();
    if cfg.admin_user.is_empty()
        && users_empty(&state).await.unwrap_or(true)
        && !requires_user_path(request.uri().path())
    {
        return next.run(request).await;
    }

    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(
            header::WWW_AUTHENTICATE,
            r#"Basic realm="THLS", charset="UTF-8""#,
        )
        .body(Body::from("Unauthorized"))
        .expect("static 401 response with valid header values")
}

pub(crate) async fn current_user_from_headers(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<AuthUser> {
    if let Some(token) = session_cookie(headers) {
        let conn = state.db_conn().await.ok()?;
        let token = token.to_string();
        if let Ok(Some(user)) =
            tokio::task::spawn_blocking(move || db::get_session_user(&conn, &token))
                .await
                .ok()?
        {
            return Some(AuthUser {
                user_id: user.user_id,
                username: user.username,
                is_admin: user.is_admin,
            });
        }
    }

    let cfg = state.config.read().await.clone();
    if cfg.admin_user.is_empty() {
        return None;
    }
    if basic_credentials_valid(headers, &cfg.admin_user, &cfg.admin_pass) {
        return Some(AuthUser {
            user_id: "basic-admin".to_string(),
            username: cfg.admin_user.clone(),
            is_admin: true,
        });
    }
    None
}

pub(crate) fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == "session" && valid_session_token(value)).then_some(value)
    })
}

fn public_path(path: &str) -> bool {
    path == "/login"
        || path == "/api/auth/login"
        || path == "/api/auth/logout"
        || path == "/api/auth/me"
        || path == "/health"
        || path.starts_with("/static/")
}

fn requires_user_path(path: &str) -> bool {
    path.starts_with("/api/users")
        || path.starts_with("/api/favorites")
        || path.starts_with("/api/watchlist")
        || path.starts_with("/api/ratings")
        || path.starts_with("/api/preferences")
        || path.starts_with("/api/next-up")
}

async fn users_empty(state: &AppState) -> Option<bool> {
    let conn = state.db_conn().await.ok()?;
    tokio::task::spawn_blocking(move || db::count_users(&conn).map(|count| count == 0))
        .await
        .ok()?
        .ok()
}

fn basic_credentials_valid(headers: &HeaderMap, admin_user: &str, admin_pass: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|encoded| {
            let decoded = base64_decode(encoded)?;
            let creds = std::str::from_utf8(&decoded).ok()?;
            let (user, pass) = creds.split_once(':')?;
            Some(
                constant_time_eq(user.as_bytes(), admin_user.as_bytes())
                    & constant_time_eq(pass.as_bytes(), admin_pass.as_bytes()),
            )
        })
        .unwrap_or(false)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let max_len = left.len().max(right.len());
    let mut diff = left.len() ^ right.len();
    for i in 0..max_len {
        let a = left.get(i).copied().unwrap_or(0);
        let b = right.get(i).copied().unwrap_or(0);
        diff |= (a ^ b) as usize;
    }
    diff == 0
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;

    for &b in input.as_bytes() {
        if b == b'=' {
            break;
        }
        let val = CHARS.iter().position(|&c| c == b)? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }

    if out.is_empty() && !input.is_empty() {
        return None;
    }
    Some(out)
}

fn valid_session_token(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
