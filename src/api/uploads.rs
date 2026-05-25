use std::collections::HashSet;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::connect_info::ConnectInfo;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::jobs::{enqueue_job, JobMetadata};
use super::{api_error, AppState};

#[derive(Debug)]
pub(crate) struct PendingUpload {
    pub(super) upload_id: String,
    pub(super) filename: String,
    pub(super) total_size: u64,
    pub(super) total_chunks: u32,
    pub(super) chunk_size: u64,
    pub(super) path: PathBuf,
    pub(super) received_chunks: HashSet<u32>,
    /// Chunks currently being written; prevents duplicate concurrent writes.
    pub(super) in_flight: HashSet<u32>,
    pub(super) received_bytes: u64,
    pub(super) ip: std::net::IpAddr,
    #[allow(dead_code)]
    pub(super) created_at: Instant,
    pub(super) last_activity: Instant,
    /// Set to true when finalize is in progress; prevents duplicate finalize enqueues.
    pub(super) finalizing: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct UploadInitRequest {
    filename: String,
    total_size: u64,
    total_chunks: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UploadFinalizeRequest {
    upload_id: String,
    metadata: Option<JobMetadata>,
    media_type: Option<String>,
    is_series: Option<bool>,
    series_name: Option<String>,
    season_number: Option<i32>,
    episode_number: Option<i32>,
    part_number: Option<i32>,
    title: Option<String>,
}

pub(super) async fn handle_upload_init(
    State(state): State<Arc<AppState>>,
    maybe_addr: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<UploadInitRequest>,
) -> Response {
    let cfg = state.config.read().await.clone();
    let peer = maybe_addr
        .map(|ConnectInfo(a)| a.ip())
        .unwrap_or_else(|| "127.0.0.1".parse().expect("loopback"));
    let ip = client_ip(&headers, cfg.behind_proxy, peer);
    if let Some(response) = check_upload_rate_limit(&state, ip).await {
        return response;
    }
    if !state.ffmpeg_available || !state.ffprobe_available {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "tools_unavailable",
            if !state.ffmpeg_available && !state.ffprobe_available {
                "ffmpeg and ffprobe are not available"
            } else if !state.ffmpeg_available {
                "ffmpeg is not available"
            } else {
                "ffprobe is not available"
            },
        );
    }
    if body.total_size == 0 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "total_size must be positive",
        );
    }
    if body.total_chunks == Some(0) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "total_chunks must be positive when provided",
        );
    }
    if body.total_size > cfg.max_upload_size {
        return api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "upload is too large",
        );
    }
    let total_chunks = body.total_size.div_ceil(cfg.upload_chunk_size);
    if total_chunks > u32::MAX as u64 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "upload has too many chunks",
        );
    }
    let total_chunks = total_chunks as u32;
    let filename = match sanitize_filename(&body.filename) {
        Ok(filename) => filename,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_filename", e),
    };
    if cfg.max_pending_uploads_per_ip > 0 {
        let pending = state.pending_uploads.lock().await;
        let count = pending.values().filter(|u| u.ip == ip).count() as u32;
        if count >= cfg.max_pending_uploads_per_ip {
            return api_error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "too many pending uploads from this IP",
            );
        }
    }

    let free = match free_space_bytes(&state.uploads_dir) {
        Ok(free) => free,
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "disk_check_failed",
                e.to_string(),
            )
        }
    };
    let required = body.total_size.saturating_add(64 * 1024 * 1024);
    if free < required {
        return api_error(
            StatusCode::INSUFFICIENT_STORAGE,
            "insufficient_storage",
            "not enough free disk space",
        );
    }

    let upload_id = uuid::Uuid::new_v4().simple().to_string();
    let path = state.uploads_dir.join(format!("{upload_id}_{filename}"));
    let file = match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return api_error(
                StatusCode::CONFLICT,
                "upload_id_exists",
                "upload id collision",
            )
        }
        Err(e) => {
            return upload_io_error("upload_allocate_failed", e);
        }
    };
    if let Err(e) = file.set_len(body.total_size).await {
        let _ = tokio::fs::remove_file(&path).await;
        return upload_io_error("upload_allocate_failed", e);
    }

    let pending = PendingUpload {
        upload_id: upload_id.clone(),
        filename,
        total_size: body.total_size,
        total_chunks,
        chunk_size: cfg.upload_chunk_size,
        path,
        received_chunks: HashSet::new(),
        in_flight: HashSet::new(),
        received_bytes: 0,
        ip,
        created_at: Instant::now(),
        last_activity: Instant::now(),
        finalizing: false,
    };
    state
        .pending_uploads
        .lock()
        .await
        .insert(upload_id.clone(), pending);
    tracing::info!(
        upload_id = %upload_id,
        filename = %body.filename,
        total_size = body.total_size,
        chunk_size = cfg.upload_chunk_size,
        total_chunks,
        client_ip = %ip,
        "upload init accepted"
    );
    Json(json!({
        "upload_id": upload_id,
        "chunk_size": cfg.upload_chunk_size,
        "total_chunks": total_chunks,
    }))
    .into_response()
}

pub(super) async fn handle_upload_chunk(
    State(state): State<Arc<AppState>>,
    maybe_addr: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let cfg = state.config.read().await.clone();
    let peer = maybe_addr
        .map(|ConnectInfo(a)| a.ip())
        .unwrap_or_else(|| "127.0.0.1".parse().expect("loopback"));
    let ip = client_ip(&headers, cfg.behind_proxy, peer);
    if let Some(response) = check_upload_rate_limit(&state, ip).await {
        return response;
    }

    let upload_id = match required_header(&headers, "x-upload-id") {
        Ok(value) => value,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e),
    };
    if uuid::Uuid::parse_str(&upload_id).is_err() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "invalid upload id format",
        );
    }
    let chunk_index: u32 = match required_header(&headers, "x-chunk-index").and_then(|v| {
        v.parse::<u32>()
            .map_err(|_| "invalid chunk index".to_string())
    }) {
        Ok(value) => value,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e),
    };
    let (path, _chunk_size, total_chunks, total_size, start_offset) = {
        let mut pending = state.pending_uploads.lock().await;
        let Some(upload) = pending.get_mut(&upload_id) else {
            return api_error(StatusCode::NOT_FOUND, "not_found", "upload not found");
        };
        if upload.ip != ip {
            return api_error(StatusCode::NOT_FOUND, "not_found", "upload not found");
        }
        if upload.last_activity.elapsed()
            > Duration::from_secs(cfg.pending_upload_ttl_seconds as u64)
        {
            let path = upload.path.clone();
            pending.remove(&upload_id);
            drop(pending);
            let _ = tokio::fs::remove_file(path).await;
            return api_error(StatusCode::NOT_FOUND, "not_found", "upload expired");
        }
        if chunk_index >= upload.total_chunks {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                "chunk index is out of range",
            );
        }
        let expected = expected_chunk_size(upload, chunk_index);
        if body.len() as u64 != expected {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                format!("chunk size must be {expected} bytes"),
            );
        }
        upload.last_activity = Instant::now();
        // Return early if already received or currently being written by a concurrent request.
        // Both checks are under the same mutex, so only one writer can proceed per chunk.
        if upload.received_chunks.contains(&chunk_index) || upload.in_flight.contains(&chunk_index)
        {
            return Json(json!({
                "chunk_index": chunk_index,
                "received_bytes": upload.received_bytes,
                "received_chunks": upload.received_chunks.len(),
                "is_retry": true,
            }))
            .into_response();
        }
        upload.in_flight.insert(chunk_index);
        (
            upload.path.clone(),
            upload.chunk_size,
            upload.total_chunks,
            upload.total_size,
            chunk_index as u64 * upload.chunk_size,
        )
    };

    let mut file = match tokio::fs::OpenOptions::new().write(true).open(&path).await {
        Ok(file) => file,
        Err(e) => {
            // Clear in_flight so the client can retry this chunk.
            if let Some(upload) = state.pending_uploads.lock().await.get_mut(&upload_id) {
                upload.in_flight.remove(&chunk_index);
            }
            return upload_io_error("chunk_write_failed", e);
        }
    };
    if let Err(e) = file.seek(std::io::SeekFrom::Start(start_offset)).await {
        if let Some(upload) = state.pending_uploads.lock().await.get_mut(&upload_id) {
            upload.in_flight.remove(&chunk_index);
        }
        return upload_io_error("chunk_write_failed", e);
    }
    if let Err(e) = file.write_all(&body).await {
        if let Some(upload) = state.pending_uploads.lock().await.get_mut(&upload_id) {
            upload.in_flight.remove(&chunk_index);
        }
        return upload_io_error("chunk_write_failed", e);
    }

    let mut pending = state.pending_uploads.lock().await;
    let Some(upload) = pending.get_mut(&upload_id) else {
        return api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "upload expired or completed during write",
        );
    };
    if upload.ip != ip {
        return api_error(StatusCode::NOT_FOUND, "not_found", "upload not found");
    }

    upload.in_flight.remove(&chunk_index);
    let is_retry = upload.received_chunks.contains(&chunk_index);
    if !is_retry {
        upload.received_chunks.insert(chunk_index);
        upload.received_bytes += body.len() as u64;
    }
    upload.last_activity = Instant::now();

    let received_bytes = upload.received_bytes;
    let received_chunks_len = upload.received_chunks.len();

    tracing::info!(
        upload_id = %upload_id,
        chunk_index,
        total_chunks = total_chunks,
        received_chunks = received_chunks_len,
        received_bytes = received_bytes,
        total_size = total_size,
        "upload chunk stored"
    );

    Json(json!({
        "chunk_index": chunk_index,
        "received_bytes": received_bytes,
        "received_chunks": received_chunks_len,
        "is_retry": is_retry,
    }))
    .into_response()
}

pub(super) async fn handle_upload_finalize(
    State(state): State<Arc<AppState>>,
    maybe_addr: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<UploadFinalizeRequest>,
) -> Response {
    let cfg = state.config.read().await.clone();
    let peer = maybe_addr
        .map(|ConnectInfo(a)| a.ip())
        .unwrap_or_else(|| "127.0.0.1".parse().expect("loopback"));
    let ip = client_ip(&headers, cfg.behind_proxy, peer);
    if let Some(response) = check_upload_rate_limit(&state, ip).await {
        return response;
    }

    if uuid::Uuid::parse_str(&body.upload_id).is_err() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "invalid upload id format",
        );
    }

    let upload = {
        let mut pending = state.pending_uploads.lock().await;
        let Some(upload) = pending.get_mut(&body.upload_id) else {
            return api_error(StatusCode::NOT_FOUND, "not_found", "upload not found");
        };
        if upload.ip != ip {
            return api_error(StatusCode::NOT_FOUND, "not_found", "upload not found");
        }
        if upload.finalizing {
            return api_error(
                StatusCode::CONFLICT,
                "already_finalizing",
                "finalize already in progress for this upload",
            );
        }
        if upload.received_chunks.len() != upload.total_chunks as usize
            || upload.received_bytes != upload.total_size
        {
            return api_error(
                StatusCode::BAD_REQUEST,
                "incomplete",
                "upload is incomplete",
            );
        }
        // Mark as finalizing before releasing the lock so a concurrent finalize request
        // sees it and returns 409 instead of enqueuing a duplicate job.
        upload.finalizing = true;
        (
            upload.upload_id.clone(),
            upload.filename.clone(),
            upload.path.clone(),
        )
    };

    let metadata = body.metadata.unwrap_or(JobMetadata {
        media_type: body.media_type,
        is_series: body.is_series,
        series_name: body.series_name,
        season_number: body.season_number,
        episode_number: body.episode_number,
        part_number: body.part_number,
        title: body.title,
        abr_tiers_override: None,
    });
    let original_source_path = upload
        .2
        .file_name()
        .and_then(|s| s.to_str())
        .map(ToOwned::to_owned);
    let job_id = match enqueue_job(
        &state,
        upload.1,
        upload.2,
        metadata,
        true,
        original_source_path,
    )
    .await
    {
        Ok(job_id) => job_id,
        Err(_) => {
            // Reset finalizing so the client can retry.
            if let Some(u) = state.pending_uploads.lock().await.get_mut(&upload.0) {
                u.finalizing = false;
            }
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "queue_full",
                "job queue is unavailable",
            );
        }
    };

    state.pending_uploads.lock().await.remove(&upload.0);
    tracing::info!(
        upload_id = %upload.0,
        job_id = %job_id,
        "upload finalized and job queued"
    );
    Json(json!({ "job_id": job_id, "message": "queued" })).into_response()
}

pub(super) async fn handle_upload_status(
    State(state): State<Arc<AppState>>,
    Path(upload_id): Path<String>,
    maybe_addr: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    if uuid::Uuid::parse_str(&upload_id).is_err() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "invalid upload id format",
        );
    }

    let cfg = state.config.read().await.clone();
    let peer = maybe_addr
        .map(|ConnectInfo(a)| a.ip())
        .unwrap_or_else(|| "127.0.0.1".parse().expect("loopback"));
    let ip = client_ip(&headers, cfg.behind_proxy, peer);

    let pending = state.pending_uploads.lock().await;
    let Some(upload) = pending.get(&upload_id) else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "upload not found");
    };
    if upload.ip != ip {
        return api_error(StatusCode::NOT_FOUND, "not_found", "upload not found");
    }
    Json(upload_status_json(upload, "pending")).into_response()
}

pub(super) async fn upload_sweeper(state: Arc<AppState>) {
    loop {
        let interval = {
            let cfg = state.config.read().await;
            cfg.pending_upload_cleanup_interval_seconds.max(1)
        };
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval as u64)) => {
                cleanup_expired_uploads(&state).await;
            }
            _ = state.shutdown_token.cancelled() => break,
        }
    }
}

pub(super) async fn cleanup_expired_uploads(state: &AppState) {
    let ttl = {
        let cfg = state.config.read().await;
        Duration::from_secs(cfg.pending_upload_ttl_seconds as u64)
    };
    let expired = {
        let mut pending = state.pending_uploads.lock().await;
        let now = Instant::now();
        let expired: Vec<String> = pending
            .iter()
            .filter(|(_, upload)| now.duration_since(upload.last_activity) > ttl)
            .map(|(id, _)| id.clone())
            .collect();
        expired
            .iter()
            .filter_map(|id| pending.remove(id).map(|upload| upload.path))
            .collect::<Vec<_>>()
    };
    for path in expired {
        let _ = tokio::fs::remove_file(path).await;
    }
}

fn upload_status_json(upload: &PendingUpload, status: &str) -> Value {
    let mut indices: Vec<u32> = upload.received_chunks.iter().copied().collect();
    indices.sort_unstable();
    json!({
        "upload_id": upload.upload_id,
        "filename": upload.filename,
        "total_size": upload.total_size,
        "chunk_size": upload.chunk_size,
        "total_chunks": upload.total_chunks,
        "received_chunks": upload.received_chunks.len(),
        "received_bytes": upload.received_bytes,
        "received_indices": indices,
        "status": status,
    })
}

fn expected_chunk_size(upload: &PendingUpload, chunk_index: u32) -> u64 {
    let offset = chunk_index as u64 * upload.chunk_size;
    upload
        .total_size
        .saturating_sub(offset)
        .min(upload.chunk_size)
}

pub(crate) fn sanitize_filename(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("filename is empty".into());
    }
    if trimmed.len() > 255 {
        return Err("filename is too long".into());
    }
    if trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('\0')
    {
        return Err("filename must not contain path components".into());
    }
    let Some(name) = FsPath::new(trimmed).file_name().and_then(|s| s.to_str()) else {
        return Err("invalid filename".into());
    };
    if name != trimmed {
        return Err("filename must not contain path components".into());
    }
    Ok(name.to_string())
}

fn required_header(headers: &HeaderMap, name: &str) -> Result<String, String> {
    headers
        .get(name)
        .ok_or_else(|| format!("missing {name} header"))?
        .to_str()
        .map(|s| s.to_string())
        .map_err(|_| format!("invalid {name} header"))
}

fn client_ip(
    headers: &HeaderMap,
    behind_proxy: bool,
    peer_addr: std::net::IpAddr,
) -> std::net::IpAddr {
    if behind_proxy {
        headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                let entries: Vec<&str> = v
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                entries.last().and_then(|s| s.parse().ok())
            })
            .unwrap_or_else(|| "127.0.0.1".parse().expect("loopback ip"))
    } else {
        peer_addr
    }
}

async fn check_upload_rate_limit(state: &AppState, ip: std::net::IpAddr) -> Option<Response> {
    let cfg = state.config.read().await;
    let window = Duration::from_secs(cfg.upload_rate_limit_window as u64);
    let max = cfg.upload_rate_limit_max_requests as usize;
    drop(cfg);

    let mut limits = state.upload_rate_limits.lock().await;
    let now = Instant::now();
    // Prune all stale per-IP deques and remove empty entries to prevent HashMap growth
    limits.retain(|_, dq| {
        while dq
            .front()
            .map(|t| now.duration_since(*t) > window)
            .unwrap_or(false)
        {
            dq.pop_front();
        }
        !dq.is_empty()
    });
    let requests = limits.entry(ip).or_default();
    requests.push_back(now);
    if requests.len() > max {
        return Some(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "upload rate limit exceeded",
        ));
    }
    None
}

fn upload_io_error(code: &str, e: std::io::Error) -> Response {
    if is_disk_full(&e) {
        api_error(
            StatusCode::INSUFFICIENT_STORAGE,
            "insufficient_storage",
            "not enough free disk space",
        )
    } else {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, code, e.to_string())
    }
}

pub(crate) fn is_disk_full(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(28)
}

#[cfg(unix)]
pub(crate) fn free_space_bytes(path: &FsPath) -> std::io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stats.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stats = unsafe { stats.assume_init() };
    Ok(stats.f_bavail * stats.f_frsize)
}

pub(crate) async fn cleanup_orphaned_uploads(uploads_dir: &std::path::Path) {
    let mut dir = match tokio::fs::read_dir(uploads_dir).await {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!(dir = %uploads_dir.display(), error = %e, "cannot read uploads directory for orphan cleanup");
            return;
        }
    };
    let mut cleaned = 0u64;
    while let Ok(Some(entry)) = dir.next_entry().await {
        let path = entry.path();
        let Ok(ft) = entry.file_type().await else {
            continue;
        };
        if ft.is_dir() {
            continue;
        }
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                tracing::info!(path = %path.display(), "cleaned orphaned upload file");
                cleaned += 1;
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to clean orphaned upload file");
            }
        }
    }
    if cleaned > 0 {
        tracing::info!(count = cleaned, dir = %uploads_dir.display(), "cleaned orphaned upload files");
    }
}

#[cfg(not(unix))]
pub(crate) fn free_space_bytes(_path: &FsPath) -> std::io::Result<u64> {
    Ok(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn make_headers(forwarded: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", forwarded.parse().unwrap());
        h
    }

    fn loopback() -> IpAddr {
        "127.0.0.1".parse().unwrap()
    }

    fn peer(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn client_ip_takes_rightmost_entry() {
        let headers = make_headers("1.2.3.4, 10.0.0.1");
        let ip = client_ip(&headers, true, loopback());
        assert_eq!(ip, peer("10.0.0.1"));
    }

    #[test]
    fn client_ip_single_entry() {
        let headers = make_headers("10.0.0.1");
        let ip = client_ip(&headers, true, loopback());
        assert_eq!(ip, peer("10.0.0.1"));
    }

    #[test]
    fn client_ip_no_proxy_returns_peer_addr() {
        let headers = HeaderMap::new();
        let ip = client_ip(&headers, false, peer("192.168.1.5"));
        assert_eq!(ip, peer("192.168.1.5"));
    }

    #[test]
    fn client_ip_missing_header_returns_loopback() {
        let headers = HeaderMap::new();
        let ip = client_ip(&headers, true, loopback());
        assert_eq!(ip, loopback());
    }
}
