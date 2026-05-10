use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::to_bytes;
use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::json;

use super::{api_error, AppState};
use crate::{config::Config, db, telegram};

#[derive(Debug, Deserialize)]
struct ExportRequest {
    upload_to_telegram: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ImportJsonRequest {
    file_id: String,
    bot_index: i64,
    #[serde(default)]
    bot_index_map: Option<HashMap<i64, i64>>,
}

pub(super) async fn handle_db_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let upload_to_telegram = if is_json(&headers) {
        match to_bytes(request.into_body(), usize::MAX).await {
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

    let export = {
        let conn = state.db.lock().await;
        match db::export_to_dict(&conn) {
            Ok(export) => export,
            Err(e) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "db_export_failed",
                    e.to_string(),
                )
            }
        }
    };
    let bytes = match serde_json::to_vec_pretty(&export) {
        Ok(bytes) => bytes,
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_export_failed",
                e.to_string(),
            )
        }
    };

    if !upload_to_telegram {
        let filename = format!("streamer-export-{}.json", unix_ts());
        let mut response = bytes.into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        response.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
                .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        );
        return response;
    }

    let cfg = state.config.read().await.clone();
    let Some(bot) = cfg.bots.first().cloned() else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "no_bot",
            "no Telegram bot configured",
        );
    };
    let path = std::env::temp_dir().join(format!("thls_export_{}.json", uuid::Uuid::new_v4()));
    if let Err(e) = tokio::fs::write(&path, &bytes).await {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_export_failed",
            e.to_string(),
        );
    }
    let uploaded = telegram::upload_document(
        &state.http,
        &state.telegram,
        &state.telegram_base_url,
        bot,
        0,
        &path,
        "db/export.json".into(),
        cfg.telegram_max_file_size,
    )
    .await;
    let _ = tokio::fs::remove_file(&path).await;
    match uploaded {
        Ok(uploaded) => Json(json!({
            "file_id": uploaded.file_id,
            "bot_index": uploaded.bot_index,
            "size": uploaded.file_size,
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
        let conn = state.db.lock().await;
        db::backup_database_file(&conn, &state.db_path)
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
    let (bytes, bot_index_map) = if is_multipart(&headers) {
        let (bytes, opt_map) = match import_from_multipart(state.clone(), request).await {
            Ok(v) => v,
            Err(response) => return response,
        };
        let map = auto_fill_or_keep(opt_map, &bytes, 0);
        (bytes, map)
    } else {
        let bytes = match to_bytes(request.into_body(), usize::MAX).await {
            Ok(bytes) => bytes,
            Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e.to_string()),
        };
        let req = match serde_json::from_slice::<ImportJsonRequest>(&bytes) {
            Ok(req) => req,
            Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_payload", e.to_string()),
        };
        let cfg = state.config.read().await.clone();
        let downloaded_bytes = match telegram::get_file_bytes(
            &state.http,
            &state.telegram,
            &state.telegram_base_url,
            &cfg.bots,
            &req.file_id,
            req.bot_index,
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
        let map = auto_fill_or_keep(req.bot_index_map, &downloaded_bytes, req.bot_index);
        (downloaded_bytes, map)
    };
    import_export_bytes(&state, &bytes, bot_index_map).await
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
            match field.bytes().await {
                Ok(bytes) => database = Some(bytes.to_vec()),
                Err(e) => {
                    return api_error(StatusCode::BAD_REQUEST, "invalid_multipart", e.to_string())
                }
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
    let source =
        std::env::temp_dir().join(format!("thls_database_load_{}.db", uuid::Uuid::new_v4()));
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

fn auto_bot_index_map(export: &db::DbExport, default_target: i64) -> HashMap<i64, i64> {
    export
        .segments
        .iter()
        .map(|s| s.bot_index)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|src| (src, default_target))
        .collect()
}

fn auto_fill_or_keep(
    map: Option<HashMap<i64, i64>>,
    export_bytes: &[u8],
    default_target: i64,
) -> HashMap<i64, i64> {
    match map {
        Some(ref m) if !m.is_empty() => m.clone(),
        _ => match serde_json::from_slice::<db::DbExport>(export_bytes) {
            Ok(export) => auto_bot_index_map(&export, default_target),
            Err(_) => HashMap::new(),
        },
    }
}

async fn import_from_multipart(
    state: Arc<AppState>,
    request: Request,
) -> Result<(Vec<u8>, Option<HashMap<i64, i64>>), Response> {
    let mut multipart = Multipart::from_request(request, &state)
        .await
        .map_err(|e| api_error(StatusCode::BAD_REQUEST, "invalid_multipart", e.to_string()))?;
    let mut file = None;
    let mut map = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("file") => {
                let bytes = field.bytes().await.map_err(|e| {
                    api_error(StatusCode::BAD_REQUEST, "invalid_multipart", e.to_string())
                })?;
                file = Some(bytes.to_vec());
            }
            Some("bot_index_map") => {
                let text = field.text().await.map_err(|e| {
                    api_error(StatusCode::BAD_REQUEST, "invalid_multipart", e.to_string())
                })?;
                map = Some(
                    serde_json::from_str::<HashMap<i64, i64>>(&text).map_err(|e| {
                        api_error(
                            StatusCode::BAD_REQUEST,
                            "invalid_bot_index_map",
                            e.to_string(),
                        )
                    })?,
                );
            }
            _ => {}
        }
    }
    let Some(file) = file else {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_payload",
            "file field is required",
        ));
    };
    Ok((file, map))
}

async fn import_export_bytes(
    state: &Arc<AppState>,
    bytes: &[u8],
    bot_index_map: HashMap<i64, i64>,
) -> Response {
    let export = match serde_json::from_slice::<db::DbExport>(bytes) {
        Ok(export) => export,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_export", e.to_string()),
    };
    if export.version != 1 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_export",
            "export version must be 1",
        );
    }
    let mut missing_bot_indices = Vec::new();
    for segment in &export.segments {
        if !bot_index_map.contains_key(&segment.bot_index)
            && !missing_bot_indices.contains(&segment.bot_index)
        {
            missing_bot_indices.push(segment.bot_index);
        }
    }
    if !missing_bot_indices.is_empty() {
        missing_bot_indices.sort_unstable();
        let missing = missing_bot_indices
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_bot_index_map",
            format!(
                "missing bot_index_map entries for [{missing}]. Telegram import bot_index is only used to download the export JSON; bot_index_map must include every source segment bot index from the export."
            ),
        );
    }
    let result = {
        let mut conn = state.db.lock().await;
        db::merge_from_export(&mut conn, &export, &bot_index_map)
    };
    match result {
        Ok(result) => Json(json!({
            "merged_jobs": result.merged_jobs,
            "merged_segments": result.merged_segments,
            "message": "import complete",
        }))
        .into_response(),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_import_failed",
            e.to_string(),
        ),
    }
}

async fn replace_live_database(
    state: &Arc<AppState>,
    source: PathBuf,
) -> anyhow::Result<db::ReplaceDatabaseResult> {
    {
        let conn = db::init_db(&source)?;
        conn.close().map_err(|(_, e)| anyhow::anyhow!(e))?;
    }
    let mut guard = state.db.lock().await;
    let old = std::mem::replace(&mut *guard, Connection::open_in_memory()?);
    let _ = old.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    old.close().map_err(|(_, e)| anyhow::anyhow!(e))?;
    let result = db::replace_database_file(&state.db_path, &source)?;
    let new_conn = db::init_db(&state.db_path)?;
    let cfg = Config::load(&new_conn)?;
    *guard = new_conn;
    drop(guard);
    *state.config.write().await = Arc::new(cfg);
    Ok(result)
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

fn unix_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
