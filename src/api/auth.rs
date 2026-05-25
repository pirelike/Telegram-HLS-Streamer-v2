use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

use crate::api::AppState;

pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let cfg = state.config.read().await;
    if cfg.admin_user.is_empty() {
        drop(cfg);
        return next.run(request).await;
    }

    let valid = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|encoded| {
            let decoded = base64_decode(encoded)?;
            let creds = std::str::from_utf8(&decoded).ok()?;
            let (user, pass) = creds.split_once(':')?;
            Some(
                constant_time_eq(user.as_bytes(), cfg.admin_user.as_bytes())
                    & constant_time_eq(pass.as_bytes(), cfg.admin_pass.as_bytes()),
            )
        })
        .unwrap_or(false);

    drop(cfg);

    if valid {
        next.run(request).await
    } else {
        Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(
                header::WWW_AUTHENTICATE,
                r#"Basic realm="THLS", charset="UTF-8""#,
            )
            .body(Body::from("Unauthorized"))
            .expect("static 401 response with valid header values")
    }
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
