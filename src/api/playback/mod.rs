mod cache;
mod real;
#[cfg(test)]
mod tests;
mod virtual_;

pub use cache::SegmentCache;

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::io::AsyncReadExt;
use tokio_stream::wrappers::ReceiverStream;

use super::{api_error, db_unavailable, valid_job_id, AppState};

use super::playlists::sanitize_segment_uri;
use cache::CacheEntry;

async fn cache_entry_for_bytes(
    cfg: &crate::config::Config,
    cache_key: &str,
    response_key: &str,
    bytes: Vec<u8>,
) -> anyhow::Result<CacheEntry> {
    let bytes = bytes_for_key(response_key, bytes);
    let path = if cfg.disk_cache_enabled {
        Some(write_cache_file(&cfg.cache_dir, cache_key, &bytes).await?)
    } else {
        None
    };
    Ok(CacheEntry {
        bytes: Arc::new(bytes),
        file_path: path,
        content_type: content_type_for(response_key),
    })
}

async fn write_cache_file(
    cache_dir: &str,
    cache_key: &str,
    bytes: &[u8],
) -> anyhow::Result<PathBuf> {
    let path = cache_file_path(FsPath::new(cache_dir), cache_key);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("cache"),
        uuid::Uuid::new_v4().simple()
    ));
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, &path).await?;
    Ok(path)
}

fn cache_file_path(cache_dir: &FsPath, cache_key: &str) -> PathBuf {
    let mut path = cache_dir.to_path_buf();
    for part in cache_key.split('/') {
        let safe: String = part
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        path.push(if safe.is_empty() || safe == "." || safe == ".." {
            "_"
        } else {
            &safe
        });
    }
    path
}

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
pub(super) use real::spawn_cache_warmup;

fn cache_response(entry: CacheEntry) -> Response {
    if let Some(path) = entry.file_path.clone() {
        return response_with_headers(
            stream_file_or_bytes(path, entry.bytes.clone()),
            entry.content_type,
        );
    }
    let body = Body::from((*entry.bytes).clone());
    response_with_headers(body, entry.content_type)
}

fn stream_file_or_bytes(path: std::path::PathBuf, fallback: Arc<Vec<u8>>) -> Body {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "cache file open failed; falling back to buffered bytes");
                let _ = tx.send(Ok(Bytes::from((*fallback).clone()))).await;
                return;
            }
        };
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match file.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tx
                        .send(Ok(Bytes::copy_from_slice(&buf[..n])))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    break;
                }
            }
        }
    });
    Body::from_stream(ReceiverStream::new(rx))
}

fn response_with_headers(body: Body, content_type: &'static str) -> Response {
    let mut resp = (StatusCode::OK, body).into_response();
    let headers = resp.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
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
        file_path: None,
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
    if bytes.starts_with(b"WEBVTT") {
        if let Some(pos) = bytes.iter().position(|&b| b == b'\n') {
            let nl = pos + 1;
            let mut out = Vec::with_capacity(bytes.len() + 64);
            out.extend_from_slice(&bytes[..nl]);
            out.extend_from_slice(b"X-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:0\n");
            out.extend_from_slice(&bytes[nl..]);
            return out;
        }
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
