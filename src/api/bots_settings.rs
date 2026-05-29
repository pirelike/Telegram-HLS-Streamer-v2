use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{api_error, db_unavailable, AppState};
use crate::config::{self, BotConfig, Config};
use crate::db;
use crate::env_writer;
use crate::media;
use crate::settings_registry;
use crate::telegram;

pub type BotHealthResult = telegram::TelegramHealthResult;

#[derive(Debug, Deserialize)]
pub(super) struct ResetSettingsRequest {
    keys: Option<Vec<String>>,
    key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BotHealthRequest {
    index: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AddBotRequest {
    token: String,
    channel_id: i64,
    label: Option<String>,
}

pub(super) async fn handle_get_settings(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.config.read().await;
    Json(settings_response(&cfg))
}

pub(super) async fn handle_post_settings(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Map<String, Value>>,
) -> Response {
    let mut normalized = HashMap::new();
    let settings = match body.get("settings") {
        Some(Value::Object(settings)) => settings.clone(),
        _ => body,
    };
    for (key, value) in settings {
        match settings_registry::normalize_json(&key, &value) {
            Ok(value) => {
                normalized.insert(key, value);
            }
            Err(e) => return api_error(StatusCode::BAD_REQUEST, "invalid_setting", e),
        }
    }
    // Check masked TMDB key against current config (read-lock only)
    {
        let guard = state.config.read().await;
        if normalized
            .get("TMDB_API_KEY")
            .is_some_and(|value| guard.is_unchanged_masked_tmdb_api_key(value))
        {
            normalized.remove("TMDB_API_KEY");
        }
    }

    {
        let mut conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => return db_unavailable(e),
        };
        let settings_for_db = normalized.clone();
        if let Err(e) =
            tokio::task::spawn_blocking(move || db::set_settings(&mut conn, &settings_for_db))
                .await
                .unwrap_or_else(|e| Err(anyhow::anyhow!(e)))
        {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "settings_write_failed",
                e.to_string(),
            );
        }
    }
    if let Err(e) = write_settings_to_env(&state, &normalized).await {
        return api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "env_write_failed",
            format!("Settings saved to database but .env file update failed: {e}. Settings will still take effect on restart from the database."),
        );
    }
    // Reload config from DB (canonical source — eliminates TOCTOU)
    let new_cfg = {
        let conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => return db_unavailable(e),
        };
        match tokio::task::spawn_blocking(move || Config::load(&conn)).await {
            Ok(Ok(cfg)) => cfg,
            Ok(Err(e)) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "config_reload_failed",
                    e.to_string(),
                );
            }
            Err(e) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "config_reload_failed",
                    e.to_string(),
                );
            }
        }
    };

    let response = settings_response(&new_cfg);
    store_config(&state, new_cfg, updates_encoder_settings(&normalized)).await;
    if matches!(
        normalized.get("DISK_CACHE_ENABLED").map(String::as_str),
        Some("false")
    ) {
        state.cache.drop_disk_files().await;
    }
    Json(response).into_response()
}

pub(super) async fn handle_reset_settings(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ResetSettingsRequest>,
) -> Response {
    let keys = body
        .keys
        .or_else(|| body.key.map(|key| vec![key]))
        .unwrap_or_else(|| {
            settings_registry::SETTINGS
                .iter()
                .map(|s| s.key.to_string())
                .collect()
        });
    for key in &keys {
        if !settings_registry::is_public_setting_key(key) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_setting",
                format!("unknown setting key: {key}"),
            );
        }
    }

    {
        let conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => return db_unavailable(e),
        };
        let keys_for_db = keys.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || {
            for key in &keys_for_db {
                db::delete_setting(&conn, key)?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!(e)))
        {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "settings_reset_failed",
                e.to_string(),
            );
        }
    }
    let restored: HashMap<String, String> = keys
        .iter()
        .filter_map(|key| config::env_or_default_value(key).map(|value| (key.clone(), value)))
        .collect();
    if let Err(e) = write_settings_to_env(&state, &restored).await {
        tracing::warn!(error = %e, "failed to update .env file on reset");
    }
    let new_cfg = {
        let conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => return db_unavailable(e),
        };
        match tokio::task::spawn_blocking(move || Config::load(&conn)).await {
            Ok(Ok(cfg)) => cfg,
            Ok(Err(e)) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "config_reload_failed",
                    e.to_string(),
                );
            }
            Err(e) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "config_reload_failed",
                    e.to_string(),
                );
            }
        }
    };
    let response = settings_response(&new_cfg);
    store_config(&state, new_cfg, resets_encoder_settings(&keys)).await;
    if matches!(
        restored.get("DISK_CACHE_ENABLED").map(String::as_str),
        Some("false")
    ) {
        state.cache.drop_disk_files().await;
    }
    Json(response).into_response()
}

pub(super) async fn handle_get_bots(State(state): State<Arc<AppState>>) -> Response {
    let cfg = state.config.read().await.clone();
    let session_metrics = state.telegram.metrics_snapshot().await;
    let workloads = {
        let conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => return db_unavailable(e),
        };
        match tokio::task::spawn_blocking(move || db::get_bot_workload_stats(&conn)).await {
            Ok(Ok(stats)) => stats,
            Ok(Err(e)) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "bot_stats_failed",
                    e.to_string(),
                )
            }
            Err(e) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "bot_stats_failed",
                    e.to_string(),
                )
            }
        }
    };

    let bots: Vec<Value> = cfg
        .bots
        .iter()
        .enumerate()
        .map(|(index, bot)| {
            let workload = workloads.get(&(index as i64));
            let session = session_metrics.per_bot.get(&(index as i64));
            json!({
                "index": index,
                "channel_id": bot.channel_id,
                "token_masked": mask_bot_token(&bot.token),
                "source": bot.source,
                "db_id": bot.db_id,
                "label": bot.label,
                "stats": {
                    "segment_count": workload.map(|w| w.segment_count).unwrap_or(0),
                    "total_bytes": workload.map(|w| w.total_bytes).unwrap_or(0),
                    "session_uploads": session.map(|m| m.upload_count).unwrap_or(0),
                    "session_upload_bytes": session.map(|m| m.upload_bytes).unwrap_or(0),
                    "session_downloads": session.map(|m| m.download_count).unwrap_or(0),
                    "session_errors": session.map(|m| m.upload_errors + m.download_errors).unwrap_or(0),
                }
            })
        })
        .collect();
    Json(json!({ "bots": bots })).into_response()
}

pub(super) async fn handle_bot_health(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BotHealthRequest>,
) -> Response {
    let cfg = state.config.read().await.clone();
    let bots: Vec<(usize, BotConfig)> = match body.index {
        Some(index) => match cfg.bots.get(index) {
            Some(bot) => vec![(index, bot.clone())],
            None => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_bot_index",
                    format!("bot index {index} is not configured"),
                )
            }
        },
        None => cfg.bots.iter().cloned().enumerate().collect(),
    };

    let mut results = Vec::new();
    for (index, bot) in bots {
        results.push(telegram::probe_bot(&state.http, &state.telegram_base_url, index, &bot).await);
    }
    *state.bot_health.write().await = results.clone();
    Json(json!({ "results": results })).into_response()
}

pub(super) async fn handle_add_bot(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddBotRequest>,
) -> Response {
    let token = body.token.trim().to_string();
    if !config::is_valid_bot_token(&token) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_bot_token",
            "token does not match Telegram bot token format",
        );
    }
    if body.channel_id >= 0 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_channel_id",
            "channel_id must be a negative integer",
        );
    }
    {
        let cfg = state.config.read().await;
        if cfg.bots.iter().any(|b| b.token == token) {
            return api_error(
                StatusCode::CONFLICT,
                "duplicate_bot",
                "bot token already exists",
            );
        }
    }

    let probe = telegram::probe_bot(
        &state.http,
        &state.telegram_base_url,
        0,
        &BotConfig {
            token: token.clone(),
            channel_id: body.channel_id,
            source: crate::config::BotSource::Db,
            db_id: None,
            label: body.label.clone().unwrap_or_default(),
        },
    )
    .await;
    if !probe.ok {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bot_health_failed",
            probe.error.unwrap_or_else(|| "health probe failed".into()),
        );
    }

    let (id, new_cfg) = {
        let conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => return db_unavailable(e),
        };
        let token_for_db = token.clone();
        let label = body.label.unwrap_or_default();
        match tokio::task::spawn_blocking(move || -> anyhow::Result<(i64, Config)> {
            if db::bot_exists(&conn, &token_for_db)? {
                anyhow::bail!("duplicate_bot");
            }
            let id = db::add_bot(&conn, &token_for_db, body.channel_id, &label)?;
            let cfg = Config::load(&conn)?;
            Ok((id, cfg))
        })
        .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(e)) if e.to_string() == "duplicate_bot" => {
                return api_error(
                    StatusCode::CONFLICT,
                    "duplicate_bot",
                    "bot token already exists",
                )
            }
            Ok(Err(e)) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "bot_add_failed",
                    e.to_string(),
                )
            }
            Err(e) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "bot_add_failed",
                    e.to_string(),
                )
            }
        }
    };
    let active_tokens: Vec<String> = new_cfg.bots.iter().map(|b| b.token.clone()).collect();
    *state.config.write().await = Arc::new(new_cfg);
    state.telegram.prune_upload_locks(&active_tokens).await;
    Json(json!({ "id": id, "message": "bot added" })).into_response()
}

pub(super) async fn handle_delete_bot(
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<i64>,
) -> Response {
    if bot_id <= 0 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_bot_id",
            "bot_id must be positive",
        );
    }
    let new_cfg = {
        let conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => return db_unavailable(e),
        };
        match tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Config>> {
            if !db::delete_bot(&conn, bot_id)? {
                return Ok(None);
            }
            Ok(Some(Config::load(&conn)?))
        })
        .await
        {
            Ok(Ok(Some(cfg))) => cfg,
            Ok(Ok(None)) => {
                return api_error(StatusCode::NOT_FOUND, "bot_not_found", "bot not found")
            }
            Ok(Err(e)) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "bot_delete_failed",
                    e.to_string(),
                )
            }
            Err(e) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "bot_delete_failed",
                    e.to_string(),
                )
            }
        }
    };
    let active_tokens: Vec<String> = new_cfg.bots.iter().map(|b| b.token.clone()).collect();
    *state.config.write().await = Arc::new(new_cfg);
    state.telegram.prune_upload_locks(&active_tokens).await;
    Json(json!({ "message": "bot deleted" })).into_response()
}
fn settings_response(cfg: &Config) -> Value {
    let values = cfg.setting_values();
    json!({ "categories": settings_registry::categories_for_values(&values) })
}

async fn store_config(state: &AppState, cfg: Config, refresh_encoder: bool) {
    *state.config.write().await = Arc::new(cfg.clone());
    if refresh_encoder {
        let selected = media::select_encoder(&cfg).await;
        tracing::info!(
            encoder = %selected.name,
            vaapi_device = selected.vaapi_device.as_deref().unwrap_or(""),
            "encoder selection refreshed after settings update"
        );
        *state.selected_encoder.write().await = selected;
    }
}

fn updates_encoder_settings(settings: &HashMap<String, String>) -> bool {
    settings.keys().any(|key| is_encoder_setting(key))
}

fn resets_encoder_settings(keys: &[String]) -> bool {
    keys.iter().any(|key| is_encoder_setting(key))
}

fn is_encoder_setting(key: &str) -> bool {
    matches!(
        key,
        "ENABLE_HW_ACCEL" | "PREFERRED_ENCODER" | "VAAPI_DEVICE"
    )
}

fn mask_bot_token(token: &str) -> String {
    let Some((id, secret)) = token.split_once(':') else {
        return "***".into();
    };
    let head: String = secret.chars().take(3).collect();
    let tail_chars: Vec<char> = secret.chars().rev().take(3).collect();
    let tail: String = tail_chars.into_iter().rev().collect();
    format!("{id}:{head}***{tail}")
}

async fn write_settings_to_env(
    state: &AppState,
    settings: &HashMap<String, String>,
) -> anyhow::Result<()> {
    let mut env_map: HashMap<&str, String> = HashMap::new();
    for (key, value) in settings {
        if let Some(spec) = settings_registry::setting_spec(key) {
            env_map.insert(spec.env, value.clone());
        }
    }
    if env_map.is_empty() {
        return Ok(());
    }
    let env_path = state.env_path.clone();
    tokio::task::spawn_blocking(move || env_writer::write_env_values(&env_path, &env_map))
        .await??;
    Ok(())
}
