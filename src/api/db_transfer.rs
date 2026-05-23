use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::to_bytes;
use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::{api_error, db_unavailable, AppState};
use crate::{config::Config, db, env_writer, telegram};

#[derive(Debug, Deserialize)]
struct ExportRequest {
    upload_to_telegram: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ImportJsonRequest {
    file_id: Option<String>,
    bot_index: Option<i64>,
}

struct DbSnapshot {
    id: String,
    filename: String,
    path: PathBuf,
    size_bytes: u64,
    schema_revision: i64,
}

struct SnapshotUploadResult {
    snapshot_id: String,
    filename: String,
    size_bytes: u64,
    uploads: Vec<serde_json::Value>,
    failed_bots: Vec<serde_json::Value>,
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

async fn merge_database_path(state: &Arc<AppState>, source: PathBuf) -> Response {
    let _sync_guard = state.db_sync_lock.lock().await;
    let result = {
        let mut conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => return db_unavailable(e),
        };
        tokio::task::spawn_blocking(move || db::merge_from_database_file(&mut conn, &source)).await
    };
    match result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
        Ok(result) => {
            if let Err(e) = reload_runtime_config(state).await {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "db_import_failed",
                    e.to_string(),
                );
            }
            Json(json!({
                "merged_jobs": result.merged_jobs,
                "merged_segments": result.merged_segments,
                "merged_segment_parts": result.merged_segment_parts,
                "message": "import complete",
            }))
            .into_response()
        }
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_import_failed",
            e.to_string(),
        ),
    }
}

async fn reload_runtime_config(state: &AppState) -> anyhow::Result<()> {
    let conn = state.db_conn().await?;
    let cfg = tokio::task::spawn_blocking(move || Config::load(&conn)).await??;
    *state.config.write().await = Arc::new(cfg);
    Ok(())
}

async fn stage_import_database(state: &AppState, bytes: Vec<u8>) -> anyhow::Result<PathBuf> {
    let source = state
        .db_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(format!(".thls_db_import_{}.db", uuid::Uuid::new_v4()));
    tokio::fs::write(&source, bytes).await?;
    Ok(source)
}

pub(crate) fn trigger_automatic_db_sync(state: Arc<AppState>, reason: String) {
    tokio::spawn(async move {
        let cfg = state.config.read().await.clone();
        if !cfg.db_sync_enabled {
            return;
        }
        drop(cfg);
        match create_db_snapshot(state.clone(), &reason).await {
            Ok(snapshot) => {
                if let Err(e) = upload_snapshot_to_all_bots(state, snapshot).await {
                    tracing::warn!(error = %e, "automatic db sync upload failed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "automatic db sync snapshot failed"),
        }
    });
}

pub(crate) async fn bootstrap_db_sync_if_configured(state: Arc<AppState>) {
    let cfg = state.config.read().await.clone();
    if !cfg.db_sync_enabled || cfg.db_sync_bootstrap.trim().is_empty() {
        return;
    }
    let descriptor: serde_json::Value = match serde_json::from_str(&cfg.db_sync_bootstrap) {
        Ok(value) => value,
        Err(e) => {
            tracing::warn!(error = %e, "DB_SYNC_BOOTSTRAP is not valid JSON");
            return;
        }
    };
    let snapshot_id = descriptor
        .get("snapshot_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if snapshot_id.is_empty() {
        tracing::warn!("DB_SYNC_BOOTSTRAP has no snapshot_id");
        return;
    }
    if let Ok(conn) = state.db_conn().await {
        if db::get_internal_value(&conn, "db_sync_bootstrap_merged")
            .ok()
            .flatten()
            .as_deref()
            == Some(snapshot_id)
        {
            return;
        }
    }

    let Some(upload_values) = descriptor
        .get("uploads")
        .and_then(serde_json::Value::as_array)
    else {
        tracing::warn!("DB_SYNC_BOOTSTRAP has no uploads array");
        return;
    };
    let mut by_bot: BTreeMap<i64, Vec<(i64, String)>> = BTreeMap::new();
    for upload in upload_values {
        let Some(bot_index) = upload.get("bot_index").and_then(serde_json::Value::as_i64) else {
            continue;
        };
        let part_index = upload
            .get("part_index")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let Some(file_id) = upload
            .get("file_id")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        by_bot
            .entry(bot_index)
            .or_default()
            .push((part_index, file_id.to_string()));
    }
    for (bot_index, mut parts) in by_bot {
        parts.sort_by_key(|(part_index, _)| *part_index);
        match download_bootstrap_parts(&state, &cfg, bot_index, &parts).await {
            Ok(bytes) => match stage_import_database(&state, bytes).await {
                Ok(path) => {
                    let response = merge_database_path(&state, path.clone()).await;
                    let _ = tokio::fs::remove_file(path).await;
                    if response.status().is_success() {
                        if let Ok(conn) = state.db_conn().await {
                            let _ = db::set_internal_value(
                                &conn,
                                "db_sync_bootstrap_merged",
                                snapshot_id,
                            );
                        }
                        tracing::info!(snapshot_id, bot_index, "DB bootstrap merge complete");
                        return;
                    }
                }
                Err(e) => tracing::warn!(error = %e, "failed to stage DB bootstrap snapshot"),
            },
            Err(e) => tracing::warn!(
                snapshot_id,
                bot_index,
                error = %e,
                "DB bootstrap download failed for bot"
            ),
        }
    }
}

async fn download_bootstrap_parts(
    state: &AppState,
    cfg: &Config,
    bot_index: i64,
    parts: &[(i64, String)],
) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    for (_, file_id) in parts {
        let bytes = telegram::get_file_bytes(
            &state.http,
            &state.telegram,
            &state.telegram_base_url,
            &cfg.bots,
            file_id,
            bot_index,
        )
        .await?;
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

async fn create_db_snapshot(state: Arc<AppState>, reason: &str) -> anyhow::Result<DbSnapshot> {
    let _sync_guard = state.db_sync_lock.lock().await;
    let stamp = unix_ts();
    let id = format!("{stamp}-{}", uuid::Uuid::new_v4());
    let filename = format!("streamer-{}-{stamp}.db", sanitize_reason(reason));
    let path = state
        .db_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(format!(".thls_{id}_{filename}"));
    let conn = state.db_conn().await?;
    let path_for_export = path.clone();
    let result =
        tokio::task::spawn_blocking(move || db::export_database_file(&conn, &path_for_export))
            .await??;
    {
        let conn = state.db_conn().await?;
        db::record_db_sync_snapshot(
            &conn,
            &id,
            result.schema_revision,
            result.size_bytes,
            "pending",
            None,
        )?;
    }
    Ok(DbSnapshot {
        id,
        filename,
        path,
        size_bytes: result.size_bytes,
        schema_revision: result.schema_revision,
    })
}

async fn upload_snapshot_to_all_bots(
    state: Arc<AppState>,
    snapshot: DbSnapshot,
) -> anyhow::Result<SnapshotUploadResult> {
    let cfg = state.config.read().await.clone();
    if cfg.bots.is_empty() {
        let _ = tokio::fs::remove_file(&snapshot.path).await;
        anyhow::bail!("no Telegram bot configured");
    }

    let mut uploads = Vec::new();
    let mut failed_bots = Vec::new();
    let upload_paths = snapshot_upload_paths(&snapshot, cfg.telegram_max_file_size).await?;

    for (bot_index, bot) in cfg.bots.iter().cloned().enumerate() {
        for (part_index, path) in upload_paths.iter().enumerate() {
            let name = if upload_paths.len() == 1 {
                snapshot.filename.clone()
            } else {
                format!("{}.part{:03}", snapshot.filename, part_index)
            };
            let uploaded = telegram::upload_document(
                &state.http,
                &state.telegram,
                &state.telegram_base_url,
                bot.clone(),
                bot_index as i64,
                path,
                format!("db-sync/{name}"),
                cfg.telegram_max_file_size,
            )
            .await;
            match uploaded {
                Ok(uploaded) => {
                    let conn = state.db_conn().await?;
                    db::record_db_sync_upload(
                        &conn,
                        &snapshot.id,
                        bot_index as i64,
                        part_index as i64,
                        &uploaded.file_id,
                        uploaded.file_size,
                    )?;
                    uploads.push(json!({
                        "bot_index": bot_index,
                        "part_index": part_index,
                        "file_id": uploaded.file_id,
                        "size": uploaded.file_size,
                    }));
                }
                Err(e) => {
                    failed_bots.push(json!({
                        "bot_index": bot_index,
                        "part_index": part_index,
                        "error": e.to_string(),
                    }));
                    tracing::warn!(
                        snapshot_id = %snapshot.id,
                        bot_index,
                        part_index,
                        error = %e,
                        "db sync upload failed for bot"
                    );
                    break;
                }
            }
        }
    }

    for path in &upload_paths {
        if path != &snapshot.path {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
    let _ = tokio::fs::remove_file(&snapshot.path).await;

    let status = if uploads.is_empty() {
        "failed"
    } else if failed_bots.is_empty() {
        "complete"
    } else {
        "partial"
    };
    let error_text = if failed_bots.is_empty() {
        None
    } else {
        Some(format!("{} bot upload(s) failed", failed_bots.len()))
    };
    {
        let conn = state.db_conn().await?;
        db::record_db_sync_snapshot(
            &conn,
            &snapshot.id,
            snapshot.schema_revision,
            snapshot.size_bytes,
            status,
            error_text.as_deref(),
        )?;
    }
    if !uploads.is_empty() {
        persist_bootstrap_descriptor(&state, &snapshot, &uploads).await?;
    }

    Ok(SnapshotUploadResult {
        snapshot_id: snapshot.id,
        filename: snapshot.filename,
        size_bytes: snapshot.size_bytes,
        uploads,
        failed_bots,
    })
}

async fn snapshot_upload_paths(
    snapshot: &DbSnapshot,
    max_size: u64,
) -> anyhow::Result<Vec<PathBuf>> {
    if max_size == 0 {
        anyhow::bail!("telegram_max_file_size must be greater than zero");
    }
    if snapshot.size_bytes <= max_size {
        return Ok(vec![snapshot.path.clone()]);
    }
    let bytes = tokio::fs::read(&snapshot.path).await?;
    let chunk_size = max_size as usize;
    let mut paths = Vec::new();
    for (i, chunk) in bytes.chunks(chunk_size).enumerate() {
        let path = snapshot.path.with_extension(format!("db.part{i:03}"));
        tokio::fs::write(&path, chunk).await?;
        paths.push(path);
    }
    Ok(paths)
}

async fn persist_bootstrap_descriptor(
    state: &AppState,
    snapshot: &DbSnapshot,
    uploads: &[serde_json::Value],
) -> anyhow::Result<()> {
    let descriptor = json!({
        "version": 1,
        "snapshot_id": snapshot.id,
        "filename": snapshot.filename,
        "created_at": unix_ts(),
        "schema_revision": snapshot.schema_revision,
        "size_bytes": snapshot.size_bytes,
        "uploads": uploads,
    })
    .to_string();

    {
        let conn = state.db_conn().await?;
        db::set_setting(&conn, "DB_SYNC_BOOTSTRAP", &descriptor)?;
    }
    let mut env_map = HashMap::new();
    env_map.insert("DB_SYNC_BOOTSTRAP", descriptor);
    let env_path = state.env_path.clone();
    tokio::task::spawn_blocking(move || env_writer::write_env_values(&env_path, &env_map))
        .await??;
    Ok(())
}

fn sanitize_reason(reason: &str) -> String {
    let cleaned: String = reason
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_matches('-').chars().take(48).collect()
}

async fn replace_live_database(
    state: &Arc<AppState>,
    source: PathBuf,
) -> anyhow::Result<db::ReplaceDatabaseResult> {
    {
        let conn = db::init_db(&source)?;
        conn.close().map_err(|(_, e)| anyhow::anyhow!(e))?;
    }
    // Block live DB replacement while any non-terminal job exists. A job that finishes
    // after the swap would write stale state into the newly loaded database.
    {
        let jobs = state.jobs.lock().await;
        if jobs.values().any(|j| !j.status.is_terminal()) {
            anyhow::bail!(
                "cannot replace database while jobs are active; wait for all jobs to finish"
            );
        }
    }
    let old_pool = {
        let guard = state.db.read().await;
        guard.clone()
    };
    wait_for_pool_drain(&old_pool).await?;
    let mut guard = state.db.write().await;
    wait_for_pool_drain(&guard).await?;
    if let Ok(conn) = guard.get() {
        if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
            tracing::warn!(error = %e, "WAL checkpoint failed during live database replacement");
        }
    }
    wait_for_pool_drain(&guard).await?;
    // Rename the file first, then install the new pool. This closes the gap where the pool
    // could be pointing at a path that no longer exists (causing 500s on any DB request).
    let result = db::replace_database_file(&state.db_path, &source)?;
    let new_pool = db::init_db_pool(&state.db_path)?;
    let new_conn = new_pool.get()?;
    let cfg = Config::load(&new_conn)?;
    drop(new_conn);
    let old_pool = std::mem::replace(&mut *guard, new_pool);
    drop(guard);
    drop(old_pool);
    *state.config.write().await = Arc::new(cfg);
    Ok(result)
}

async fn wait_for_pool_drain(pool: &db::DbPool) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let state = pool.state();
        if state.connections == state.idle_connections {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for sqlite pool to drain ({} total, {} idle)",
                state.connections,
                state.idle_connections
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
