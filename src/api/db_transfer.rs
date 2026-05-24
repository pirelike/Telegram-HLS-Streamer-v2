use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::to_bytes;
use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::db_transfer_replace::{
    merge_database_path, replace_live_database, stage_import_database,
};
pub(crate) use super::db_transfer_sync::{
    bootstrap_db_sync_if_configured, trigger_automatic_db_sync,
};
use super::db_transfer_sync::{create_db_snapshot, upload_snapshot_to_all_bots};
use super::{api_error, db_unavailable, AppState};
use crate::{db, telegram};

#[derive(Debug, Deserialize)]
struct ExportRequest {
    upload_to_telegram: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ImportJsonRequest {
    file_id: Option<String>,
    bot_index: Option<i64>,
    encryption_nonce: Option<String>,
    pub(super) snapshot_id: Option<String>,
    part_index: Option<i64>,
}

pub(super) struct DbSnapshot {
    pub(super) id: String,
    pub(super) filename: String,
    pub(super) path: PathBuf,
    pub(super) size_bytes: u64,
    pub(super) schema_revision: i64,
}

pub(super) struct SnapshotUploadResult {
    pub(super) snapshot_id: String,
    pub(super) filename: String,
    pub(super) size_bytes: u64,
    pub(super) uploads: Vec<serde_json::Value>,
    pub(super) failed_bots: Vec<serde_json::Value>,
}

pub(super) async fn handle_db_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let upload_to_telegram = if is_json(&headers) {
        // Enforce 1 MB limit on the export JSON request body
        match to_bytes(request.into_body(), 1024 * 1024).await {
            Ok(bytes) if bytes.is_empty() => true,
            Ok(bytes) => match serde_json::from_slice::<ExportRequest>(&bytes) {
                Ok(req) => req.upload_to_telegram.unwrap_or(true),
                Err(e) => {
                    return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e.to_string())
                }
            },
            Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e.to_string()),
        }
    } else {
        true
    };

    let snapshot = match create_db_snapshot(state.clone(), "manual").await {
        Ok(snapshot) => snapshot,
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_export_failed",
                e.to_string(),
            )
        }
    };

    if !upload_to_telegram {
        let bytes = match tokio::fs::read(&snapshot.path).await {
            Ok(bytes) => bytes,
            Err(e) => {
                let _ = tokio::fs::remove_file(&snapshot.path).await;
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "db_export_failed",
                    e.to_string(),
                );
            }
        };
        let filename = snapshot.filename.clone();
        let _ = tokio::fs::remove_file(&snapshot.path).await;
        let mut response = bytes.into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.sqlite3"),
        );
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
                .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        );
        return response;
    }

    match upload_snapshot_to_all_bots(state.clone(), snapshot).await {
        Ok(result) => Json(json!({
            "snapshot_id": result.snapshot_id,
            "filename": result.filename,
            "size": result.size_bytes,
            "uploads": result.uploads,
            "failed_bots": result.failed_bots,
        }))
        .into_response(),
        Err(e) => api_error(
            StatusCode::BAD_GATEWAY,
            "telegram_upload_failed",
            e.to_string(),
        ),
    }
}

pub(super) async fn handle_db_backup(State(state): State<Arc<AppState>>) -> Response {
    let result = {
        let conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => return db_unavailable(e),
        };
        let db_path = state.db_path.clone();
        tokio::task::spawn_blocking(move || db::backup_database_file(&conn, &db_path))
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!(e)))
    };
    match result {
        Ok(result) => Json(json!({
            "backup_path": result.backup_path.to_string_lossy(),
            "size_bytes": result.size_bytes,
            "schema_revision": result.schema_revision,
        }))
        .into_response(),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_backup_failed",
            e.to_string(),
        ),
    }
}

pub(super) async fn handle_db_import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    if is_multipart(&headers) {
        let source = match import_db_from_multipart(state.clone(), request).await {
            Ok(v) => v,
            Err(response) => return response,
        };
        let response = merge_database_path(&state, source.clone()).await;
        let _ = tokio::fs::remove_file(source).await;
        response
    } else {
        let bytes = match to_bytes(request.into_body(), 1024 * 1024).await {
            Ok(bytes) => bytes,
            Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e.to_string()),
        };
        let req = match serde_json::from_slice::<ImportJsonRequest>(&bytes) {
            Ok(req) => req,
            Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e.to_string()),
        };
        let Some(file_id) = req.file_id else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                "file_id is required for Telegram import",
            );
        };
        let bot_index = req.bot_index.unwrap_or(0);
        let cfg = state.config.read().await.clone();
        let downloaded_bytes = match telegram::get_file_bytes(
            &state.http,
            &state.telegram,
            &state.telegram_base_url,
            &cfg.bots,
            &file_id,
            bot_index,
        )
        .await
        {
            Ok(bytes) => bytes,
            Err(e) => {
                return api_error(
                    StatusCode::BAD_GATEWAY,
                    "telegram_download_failed",
                    e.to_string(),
                )
            }
        };
        let downloaded_bytes = match req.encryption_nonce.as_deref() {
            Some(nonce) => {
                let snapshot_id = req.snapshot_id.as_deref().unwrap_or("manual");
                let aad = crate::crypto::db_sync_aad(snapshot_id, req.part_index.unwrap_or(0));
                match crate::crypto::decrypt_optional(
                    cfg.telegram_encryption_key.as_ref(),
                    Some(nonce),
                    &aad,
                    downloaded_bytes,
                ) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        return api_error(StatusCode::BAD_REQUEST, "decrypt_failed", e.to_string())
                    }
                }
            }
            None => downloaded_bytes,
        };
        let source = stage_import_database(&state, downloaded_bytes).await;
        match source {
            Ok(source) => {
                let response = merge_database_path(&state, source.clone()).await;
                let _ = tokio::fs::remove_file(source).await;
                response
            }
            Err(e) => api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_import_failed",
                e.to_string(),
            ),
        }
    }
}

pub(super) async fn handle_database_load(
    State(state): State<Arc<AppState>>,
    request: Request,
) -> Response {
    let mut multipart = match Multipart::from_request(request, &state).await {
        Ok(multipart) => multipart,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_multipart", e.to_string()),
    };
    let mut database = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("database") {
            // Limit database file upload to 100 MB
            match read_field_bytes_limit(field, 100 * 1024 * 1024).await {
                Ok(bytes) => database = Some(bytes),
                Err(resp) => return resp,
            }
        }
    }
    let Some(bytes) = database else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "database field is required",
        );
    };
    // Stage the replacement file next to the active DB so the final rename is
    // intra-filesystem and atomic; staging under temp_dir() can be on a different
    // mount and cause cross-device rename failure after the active DB is moved aside.
    let source = state
        .db_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(format!(".thls_db_load_{}.tmp", uuid::Uuid::new_v4()));
    if let Err(e) = tokio::fs::write(&source, bytes).await {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_load_failed",
            e.to_string(),
        );
    }

    let result = replace_live_database(&state, source.clone()).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(source).await;
    }
    match result {
        Ok(result) => Json(json!({
            "backup_path": result.backup_path.to_string_lossy(),
            "schema_revision": result.schema_revision,
        }))
        .into_response(),
        Err(e) => api_error(
            StatusCode::BAD_REQUEST,
            "database_load_failed",
            e.to_string(),
        ),
    }
}

async fn import_db_from_multipart(
    state: Arc<AppState>,
    request: Request,
) -> Result<PathBuf, Response> {
    let mut multipart = Multipart::from_request(request, &state)
        .await
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, "invalid_multipart", e.to_string()))?;
    let mut file = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if matches!(field.name(), Some("database") | Some("file")) {
            let bytes = read_field_bytes_limit(field, 100 * 1024 * 1024).await?;
            file = Some(bytes);
        }
    }
    let Some(file) = file else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "database field is required",
        ));
    };
    stage_import_database(&state, file).await.map_err(|e| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_import_failed",
            e.to_string(),
        )
    })
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("application/json"))
        .unwrap_or(false)
}

fn is_multipart(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("multipart/form-data"))
        .unwrap_or(false)
}

pub(super) fn unix_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn read_field_bytes_limit(
    mut field: axum::extract::multipart::Field<'_>,
    limit: usize,
) -> Result<Vec<u8>, Response> {
    let mut buf = Vec::new();
    while match field.chunk().await {
        Ok(Some(chunk)) => {
            if buf.len() + chunk.len() > limit {
                return Err(api_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "payload_too_large",
                    format!("field exceeds limit of {limit} bytes"),
                ));
            }
            buf.extend_from_slice(&chunk);
            true
        }
        Ok(None) => false,
        Err(e) => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "invalid_multipart",
                e.to_string(),
            ))
        }
    } {}
    Ok(buf)
}
