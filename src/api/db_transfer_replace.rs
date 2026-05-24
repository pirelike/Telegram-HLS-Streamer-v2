use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::{api_error, db_unavailable, AppState};
use crate::{config::Config, db, media};

pub(super) async fn merge_database_path(state: &Arc<AppState>, source: PathBuf) -> Response {
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

pub(super) async fn reload_runtime_config(state: &AppState) -> anyhow::Result<()> {
    let conn = state.db_conn().await?;
    let cfg = tokio::task::spawn_blocking(move || Config::load(&conn)).await??;
    let encoder = media::select_encoder(&cfg).await;
    *state.selected_encoder.write().await = encoder;
    *state.config.write().await = Arc::new(cfg);
    Ok(())
}

pub(super) async fn stage_import_database(
    state: &AppState,
    bytes: Vec<u8>,
) -> anyhow::Result<PathBuf> {
    let source = state
        .db_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(format!(".thls_db_import_{}.db", uuid::Uuid::new_v4()));
    tokio::fs::write(&source, bytes).await?;
    Ok(source)
}

pub(super) fn sanitize_reason(reason: &str) -> String {
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

pub(super) async fn replace_live_database(
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
    let encoder = media::select_encoder(&cfg).await;
    *state.selected_encoder.write().await = encoder;
    *state.config.write().await = Arc::new(cfg);
    Ok(result)
}

pub(super) async fn wait_for_pool_drain(pool: &db::DbPool) -> anyhow::Result<()> {
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
