mod cache;
mod real;
#[cfg(test)]
mod tests;
mod virtual_;

pub use cache::SegmentCache;

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use super::{api_error, valid_job_id, AppState};
use crate::db;

use super::playlists::sanitize_segment_uri;
use cache::CacheEntry;

pub(super) async fn handle_segment(
    State(state): State<Arc<AppState>>,
    Path((job_id, key)): Path<(String, String)>,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    if sanitize_segment_uri(&key).is_none() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_key",
            "invalid segment key",
        );
    }
    if virtual_::is_virtual_key(&key) {
        return virtual_::serve_virtual_segment(state, job_id, key).await;
    }
    real::serve_real_segment(state, job_id, key).await
}

// Exposed for playlists::handle_thumbnail
pub(super) use real::serve_real_segment;

fn cache_response(entry: CacheEntry) -> Response {
    let body = Body::from((*entry.bytes).clone());
    let mut resp = (StatusCode::OK, body).into_response();
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(entry.content_type),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    resp
}

fn entry_for_key(key: &str, entry: CacheEntry) -> CacheEntry {
    if !key.ends_with(".vtt") {
        return entry;
    }
    let bytes = bytes_for_key(key, (*entry.bytes).clone());
    if bytes.as_slice() == entry.bytes.as_slice() {
        return entry;
    }
    CacheEntry {
        bytes: Arc::new(bytes),
        content_type: entry.content_type,
    }
}

fn bytes_for_key(key: &str, bytes: Vec<u8>) -> Vec<u8> {
    if !key.ends_with(".vtt")
        || bytes
            .windows(b"X-TIMESTAMP-MAP".len())
            .any(|w| w == b"X-TIMESTAMP-MAP")
    {
        return bytes;
    }
    if bytes.starts_with(b"WEBVTT\r\n") {
        let mut out = b"WEBVTT\r\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:0\r\n".to_vec();
        out.extend_from_slice(&bytes[b"WEBVTT\r\n".len()..]);
        return out;
    }
    if bytes.starts_with(b"WEBVTT\n") {
        let mut out = b"WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:0\n".to_vec();
        out.extend_from_slice(&bytes[b"WEBVTT\n".len()..]);
        return out;
    }
    bytes
}

fn content_type_for(key_or_filename: &str) -> &'static str {
    let lower = key_or_filename.to_ascii_lowercase();
    if lower.ends_with(".ts") {
        "video/mp2t"
    } else if lower.ends_with(".m4s") || lower.ends_with(".mp4") {
        "video/mp4"
    } else if lower.ends_with(".vtt") {
        "text/vtt"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "application/octet-stream"
    }
}

// SegmentLookup helper
impl db::SegmentLookup {
    pub(super) fn into_tuple(self) -> (String, i64) {
        (self.file_id, self.bot_index)
    }
}
