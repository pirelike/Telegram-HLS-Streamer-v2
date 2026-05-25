use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use reqwest::redirect::Policy;
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::jobs::{enqueue_existing_job, JobMetadata, JobState, JobStatus};
use super::{api_error, uploads, AppState};

const DOWNLOAD_TIMEOUT_SECS: u64 = 3600;
const DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 30;
const DOWNLOAD_READ_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Deserialize)]
pub(super) struct UrlIngestRequest {
    url: String,
}

pub(super) async fn handle_url_ingest(
    State(state): State<Arc<AppState>>,
    Json(body): Json<UrlIngestRequest>,
) -> Response {
    if !state.ffmpeg_available || !state.ffprobe_available {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "tools_unavailable",
            "ffmpeg and ffprobe are required",
        );
    }
    let url = match Url::parse(body.url.trim()) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => url,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_url",
                "url must be http or https",
            )
        }
    };
    if let Err(e) = validate_public_url(&url).await {
        return api_error(StatusCode::BAD_REQUEST, "blocked_url", e);
    }
    let Ok(permit) = state.ingest_download_semaphore.clone().try_acquire_owned() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "ingest_busy",
            "too many concurrent URL downloads, try again later",
        );
    };

    let cfg = state.config.read().await.clone();
    let job_id = uuid::Uuid::new_v4().simple().to_string();
    let filename = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| uploads::sanitize_filename(s).ok())
        .unwrap_or_else(|| "remote-video.bin".into());
    let stored_name = format!("{job_id}_{filename}");
    let path = state.uploads_dir.join(&stored_name);
    insert_download_state(&state, &job_id, &filename, &path).await;

    let job_id_response = job_id.clone();
    tokio::spawn(async move {
        let _permit = permit;
        download_and_enqueue(
            state,
            url,
            job_id,
            filename,
            stored_name,
            path,
            cfg.max_upload_size,
        )
        .await;
    });
    Json(json!({ "job_id": job_id_response, "message": "downloading" })).into_response()
}

async fn download_and_enqueue(
    state: Arc<AppState>,
    mut url: Url,
    job_id: String,
    filename: String,
    stored_name: String,
    path: std::path::PathBuf,
    max_upload_size: u64,
) {
    let download_result = tokio::time::timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS), async {
        for redirects in 0..=5 {
            if redirects == 5 {
                let _ = download_failed(
                    &state,
                    &job_id,
                    &path,
                    StatusCode::BAD_REQUEST,
                    "too_many_redirects",
                    "too many redirects",
                )
                .await;
                break;
            }
            let addrs = match checked_addrs(&url).await {
                Ok(addrs) => addrs,
                Err(e) => {
                    let _ = download_failed(
                        &state,
                        &job_id,
                        &path,
                        StatusCode::BAD_REQUEST,
                        "blocked_url",
                        e,
                    )
                    .await;
                    break;
                }
            };
            let client = match client_for_url(&url, &addrs) {
                Ok(client) => client,
                Err(e) => {
                    let _ = download_failed(
                        &state,
                        &job_id,
                        &path,
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "http_client_failed",
                        e.to_string(),
                    )
                    .await;
                    break;
                }
            };
            let resp = match client.get(url.clone()).send().await {
                Ok(resp) => resp,
                Err(e) => {
                    let _ = download_failed(
                        &state,
                        &job_id,
                        &path,
                        StatusCode::BAD_GATEWAY,
                        "download_failed",
                        e.to_string(),
                    )
                    .await;
                    break;
                }
            };
            if resp.status().is_redirection() {
                let Some(location) = resp
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                else {
                    let _ = download_failed(
                        &state,
                        &job_id,
                        &path,
                        StatusCode::BAD_GATEWAY,
                        "invalid_redirect",
                        "redirect missing location",
                    )
                    .await;
                    break;
                };
                url = match url.join(location) {
                    Ok(next) => next,
                    Err(e) => {
                        let _ = download_failed(
                            &state,
                            &job_id,
                            &path,
                            StatusCode::BAD_GATEWAY,
                            "invalid_redirect",
                            e.to_string(),
                        )
                        .await;
                        break;
                    }
                };
                if let Err(e) = validate_public_url(&url).await {
                    let _ = download_failed(
                        &state,
                        &job_id,
                        &path,
                        StatusCode::BAD_REQUEST,
                        "blocked_url",
                        e,
                    )
                    .await;
                    break;
                }
                continue;
            }
            if !resp.status().is_success() {
                let _ = download_failed(
                    &state,
                    &job_id,
                    &path,
                    StatusCode::BAD_GATEWAY,
                    "download_failed",
                    format!("remote returned {}", resp.status()),
                )
                .await;
                break;
            }
            if let Some(len) = resp.content_length() {
                if len > max_upload_size {
                    let _ = download_failed(
                        &state,
                        &job_id,
                        &path,
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "too_large",
                        "remote file is too large",
                    )
                    .await;
                    break;
                }
                if let Err(e) = check_disk_space(&state, len) {
                    let _ = download_failed(&state, &job_id, &path, e.0, e.1, e.2).await;
                    break;
                }
            } else if let Err(e) = check_disk_space(&state, max_upload_size) {
                // No Content-Length header; check that at least max_upload_size bytes of free space exist
                // before starting the download.
                let _ = download_failed(&state, &job_id, &path, e.0, e.1, e.2).await;
                break;
            }
            if let Some(content_type) = resp
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
            {
                if clearly_non_media(content_type) {
                    let _ = download_failed(
                        &state,
                        &job_id,
                        &path,
                        StatusCode::BAD_REQUEST,
                        "invalid_content_type",
                        "remote content type is not media",
                    )
                    .await;
                    break;
                }
            }
            match stream_to_file(&state, &job_id, &path, resp, max_upload_size).await {
                Ok(()) => {
                    if job_cancelled(&state, &job_id).await {
                        break;
                    }
                    {
                        let jobs = state.jobs.lock().await;
                        if let Some(job) = jobs.get(&job_id) {
                            if job.status == JobStatus::Error {
                                break;
                            }
                        }
                    }
                    let metadata = JobMetadata {
                        title: Some(filename.clone()),
                        ..Default::default()
                    };
                    if let Err(e) = enqueue_existing_job(
                        &state,
                        job_id.clone(),
                        filename,
                        path.clone(),
                        metadata,
                        true,
                        Some(stored_name),
                        false,
                    )
                    .await
                    {
                        let _ = download_failed(
                            &state,
                            &job_id,
                            &path,
                            StatusCode::SERVICE_UNAVAILABLE,
                            "queue_full",
                            e.to_string(),
                        )
                        .await;
                    }
                }
                Err(e) => {
                    if e == "cancelled" {
                        break;
                    }
                    let _ = download_failed(
                        &state,
                        &job_id,
                        &path,
                        StatusCode::BAD_GATEWAY,
                        "download_failed",
                        e,
                    )
                    .await;
                }
            }
            break;
        }
    })
    .await;

    if download_result.is_err() {
        let _ = download_failed(
            &state,
            &job_id,
            &path,
            StatusCode::GATEWAY_TIMEOUT,
            "download_timed_out",
            "download timed out",
        )
        .await;
    }
}

async fn insert_download_state(
    state: &Arc<AppState>,
    job_id: &str,
    filename: &str,
    path: &std::path::Path,
) {
    let job = JobState {
        job_id: job_id.into(),
        filename: filename.into(),
        source_path: path.to_path_buf(),
        processing_path: state.processing_dir.join(job_id),
        status: JobStatus::Downloading,
        progress: 0.0,
        step: 0,
        total_steps: 5,
        description: "downloading remote file".into(),
        queued_at: Instant::now(),
        started_at: Some(Instant::now()),
        finished_at: None,
        cancel_requested: false,
        cancel_flag: Arc::new(AtomicBool::new(false)),
        error: None,
        metadata: JobMetadata::default(),
        analysis: None,
        delete_source_on_finish: true,
        original_source_path: None,
    };
    state.jobs.lock().await.insert(job_id.into(), job);
    if let Ok(conn) = state.db_conn().await {
        if let Err(e) = crate::db::insert_job_marker(&conn, job_id, filename, "downloading") {
            tracing::warn!(job_id, error = %e, "failed to persist URL ingest marker");
        }
    }
}

async fn job_cancelled(state: &AppState, job_id: &str) -> bool {
    state
        .jobs
        .lock()
        .await
        .get(job_id)
        .map(|job| job.cancel_requested || job.status == JobStatus::Cancelled)
        .unwrap_or(true)
}

async fn stream_to_file(
    state: &Arc<AppState>,
    job_id: &str,
    path: &std::path::Path,
    mut resp: reqwest::Response,
    max_size: u64,
) -> Result<(), String> {
    let total = resp.content_length();
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .map_err(|e| e.to_string())?;
    let mut downloaded = 0u64;
    let mut last_progress_update = std::time::Instant::now();
    loop {
        let chunk = resp.chunk().await.map_err(|e| e.to_string())?;
        let Some(chunk) = chunk else { break };
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > max_size {
            return Err("remote file is too large".into());
        }
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        // Batch progress updates to avoid locking the jobs map on every chunk
        if last_progress_update.elapsed() >= std::time::Duration::from_millis(500) {
            last_progress_update = std::time::Instant::now();
            let mut jobs = state.jobs.lock().await;
            if let Some(job) = jobs.get_mut(job_id) {
                if job.cancel_requested
                    || job.status == JobStatus::Cancelled
                    || job.status == JobStatus::Error
                {
                    return Err("cancelled".into());
                }
                job.progress = total
                    .map(|t| (downloaded as f64 / t as f64) * 100.0)
                    .unwrap_or(0.0)
                    .min(99.0);
                job.description = match total {
                    Some(t) => format!("downloading remote file ({downloaded}/{t} bytes)"),
                    None => format!("downloading remote file ({downloaded} bytes)"),
                };
            }
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn download_failed(
    state: &Arc<AppState>,
    job_id: &str,
    path: &std::path::Path,
    status: StatusCode,
    code: &str,
    message: impl Into<String>,
) -> Response {
    let message = message.into();
    let _ = tokio::fs::remove_file(path).await;
    if let Some(job) = state.jobs.lock().await.get_mut(job_id) {
        job.status = JobStatus::Error;
        job.progress = 100.0;
        job.description = "download failed".into();
        job.error = Some(message.clone());
        job.finished_at = Some(Instant::now());
    }
    if let Ok(conn) = state.db_conn().await {
        if let Err(e) = crate::db::mark_job_as_failed(&conn, job_id, &message) {
            tracing::warn!(job_id, error = %e, "failed to persist URL ingest failure");
        }
    }
    api_error(status, code, message)
}

fn check_disk_space(
    state: &AppState,
    total_size: u64,
) -> Result<(), (StatusCode, &'static str, String)> {
    let free = uploads::free_space_bytes(&state.uploads_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "disk_check_failed",
            e.to_string(),
        )
    })?;
    let required = total_size.saturating_add(64 * 1024 * 1024);
    if free < required {
        return Err((
            StatusCode::INSUFFICIENT_STORAGE,
            "insufficient_storage",
            "not enough free disk space".into(),
        ));
    }
    Ok(())
}

async fn validate_public_url(url: &Url) -> Result<(), String> {
    checked_addrs(url).await.map(|_| ())
}

fn client_for_url(url: &Url, addrs: &[SocketAddr]) -> Result<Client, reqwest::Error> {
    let host = url.host_str().unwrap_or_default();
    Client::builder()
        .connect_timeout(Duration::from_secs(DOWNLOAD_CONNECT_TIMEOUT_SECS))
        .read_timeout(Duration::from_secs(DOWNLOAD_READ_TIMEOUT_SECS))
        .redirect(Policy::none())
        .resolve_to_addrs(host, addrs)
        .build()
}

async fn checked_addrs(url: &Url) -> Result<Vec<SocketAddr>, String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("url must be http or https".into());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "url host is required".to_string())?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err("localhost URLs are not allowed".into());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return Err("private or local IPs are not allowed".into());
        }
        return Ok(vec![SocketAddr::new(
            ip,
            url.port_or_known_default().unwrap_or(80),
        )]);
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("failed to resolve host: {e}"))?
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        return Err("host resolved to no addresses".into());
    }
    for addr in &addrs {
        if is_blocked_socket(*addr) {
            return Err("private or local IPs are not allowed".into());
        }
    }
    Ok(addrs)
}

fn is_blocked_socket(addr: SocketAddr) -> bool {
    is_blocked_ip(addr.ip())
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip == Ipv4Addr::new(255, 255, 255, 255)
                || ip.octets()[0] == 0
                || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
                || ip == Ipv4Addr::new(169, 254, 169, 254)
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_blocked_ip(IpAddr::V4(mapped));
            }
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

fn clearly_non_media(content_type: &str) -> bool {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    ct.starts_with("text/")
        || matches!(
            ct.as_str(),
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "image/png"
                | "image/jpeg"
                | "image/gif"
                | "image/webp"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_ips() {
        assert!(is_blocked_ip("127.0.0.1".parse().unwrap()));
        assert!(is_blocked_ip("::ffff:127.0.0.1".parse().unwrap()));
        assert!(is_blocked_ip("10.0.0.1".parse().unwrap()));
        assert!(is_blocked_ip("169.254.169.254".parse().unwrap()));
        assert!(is_blocked_ip("100.64.0.1".parse().unwrap()));
        assert!(!is_blocked_ip("93.184.216.34".parse().unwrap()));
    }

    #[test]
    fn rejects_clear_non_media_types() {
        assert!(clearly_non_media("text/html; charset=utf-8"));
        assert!(clearly_non_media("application/json"));
        assert!(!clearly_non_media("video/mp4"));
        assert!(!clearly_non_media("application/octet-stream"));
    }
}
