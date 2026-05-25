use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use super::AppState;
use crate::{config::Config, db, env_writer, telegram};

use super::db_transfer::{DbSnapshot, SnapshotUploadResult};

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
    let mut by_bot: BTreeMap<i64, Vec<BootstrapPart>> = BTreeMap::new();
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
        let encryption_nonce = upload
            .get("encryption_nonce")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        by_bot.entry(bot_index).or_default().push(BootstrapPart {
            part_index,
            file_id: file_id.to_string(),
            encryption_nonce,
        });
    }
    for (bot_index, mut parts) in by_bot {
        parts.sort_by_key(|part| part.part_index);
        match download_bootstrap_parts(&state, &cfg, snapshot_id, bot_index, &parts).await {
            Ok(bytes) => match super::db_transfer_replace::stage_import_database(&state, bytes)
                .await
            {
                Ok(path) => {
                    let response =
                        super::db_transfer_replace::merge_database_path(&state, path.clone()).await;
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

#[derive(Debug, Clone)]
struct BootstrapPart {
    part_index: i64,
    file_id: String,
    encryption_nonce: Option<String>,
}

async fn download_bootstrap_parts(
    state: &AppState,
    cfg: &Config,
    snapshot_id: &str,
    bot_index: i64,
    parts: &[BootstrapPart],
) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    for part in parts {
        let bytes = telegram::get_file_bytes(
            &state.http,
            &state.telegram,
            &state.telegram_base_url,
            &cfg.bots,
            &part.file_id,
            bot_index,
        )
        .await?;
        let aad = crate::crypto::db_sync_aad(snapshot_id, part.part_index);
        let bytes = crate::crypto::decrypt_optional(
            cfg.telegram_encryption_key.as_ref(),
            part.encryption_nonce.as_deref(),
            &aad,
            bytes,
        )?;
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

pub(super) async fn create_db_snapshot(
    state: Arc<AppState>,
    reason: &str,
) -> anyhow::Result<DbSnapshot> {
    let _sync_guard = state.db_sync_lock.lock().await;
    let stamp = super::db_transfer::unix_ts();
    let id = format!("{stamp}-{}", uuid::Uuid::new_v4());
    let filename = format!(
        "streamer-{}-{stamp}.db",
        super::db_transfer_replace::sanitize_reason(reason)
    );
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

pub(super) async fn upload_snapshot_to_all_bots(
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
    let upload_paths = snapshot_upload_paths(
        &snapshot,
        cfg.telegram_max_file_size,
        cfg.telegram_encryption_key.is_some(),
    )
    .await?;

    for (bot_index, bot) in cfg.bots.iter().cloned().enumerate() {
        // Buffer this bot's uploads; only commit to the descriptor after ALL parts succeed
        struct BotUploadRecord {
            part_index: usize,
            file_id: String,
            file_size: u64,
            encryption_nonce: Option<String>,
        }
        let mut bot_buffer: Vec<BotUploadRecord> = Vec::new();
        let mut bot_failed = false;

        for (part_index, path) in upload_paths.iter().enumerate() {
            let logical_key = crate::crypto::db_sync_aad(&snapshot.id, part_index as i64);
            let uploaded = telegram::upload_document(
                &state.http,
                &state.telegram,
                &state.telegram_base_url,
                bot.clone(),
                bot_index as i64,
                path,
                logical_key,
                cfg.telegram_encryption_key.as_ref(),
                cfg.telegram_max_file_size,
            )
            .await;
            match uploaded {
                Ok(uploaded) => {
                    bot_buffer.push(BotUploadRecord {
                        part_index,
                        file_id: uploaded.file_id,
                        file_size: uploaded.file_size,
                        encryption_nonce: uploaded.encryption_nonce,
                    });
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
                    bot_failed = true;
                    break;
                }
            }
        }

        // Only include this bot in the descriptor if every part succeeded
        if !bot_failed {
            let conn = state.db_conn().await?;
            for rec in bot_buffer {
                db::record_db_sync_upload(
                    &conn,
                    &snapshot.id,
                    bot_index as i64,
                    rec.part_index as i64,
                    &rec.file_id,
                    rec.file_size,
                    rec.encryption_nonce.as_deref(),
                )?;
                uploads.push(json!({
                    "bot_index": bot_index,
                    "part_index": rec.part_index,
                    "file_id": rec.file_id,
                    "size": rec.file_size,
                    "encryption_nonce": rec.encryption_nonce,
                }));
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

pub(super) async fn snapshot_upload_paths(
    snapshot: &DbSnapshot,
    max_size: u64,
    encrypted: bool,
) -> anyhow::Result<Vec<PathBuf>> {
    if max_size == 0 {
        anyhow::bail!("telegram_max_file_size must be greater than zero");
    }
    let max_plaintext_size = crate::crypto::max_plaintext_size(max_size, encrypted)?;
    if snapshot.size_bytes <= max_plaintext_size {
        return Ok(vec![snapshot.path.clone()]);
    }
    let bytes = tokio::fs::read(&snapshot.path).await?;
    let chunk_size = max_plaintext_size as usize;
    let mut paths = Vec::new();
    for (i, chunk) in bytes.chunks(chunk_size).enumerate() {
        let path = snapshot.path.with_extension(format!("db.part{i:03}"));
        tokio::fs::write(&path, chunk).await?;
        paths.push(path);
    }
    Ok(paths)
}

pub(super) async fn persist_bootstrap_descriptor(
    state: &AppState,
    snapshot: &DbSnapshot,
    uploads: &[serde_json::Value],
) -> anyhow::Result<()> {
    let descriptor = json!({
        "version": 1,
        "snapshot_id": snapshot.id,
        "filename": snapshot.filename,
        "created_at": super::db_transfer::unix_ts(),
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
