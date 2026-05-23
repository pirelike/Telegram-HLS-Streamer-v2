use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::IpAddr;

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use crate::db;
use crate::settings_registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BotSource {
    Env,
    Db,
}

#[derive(Debug, Clone, Serialize)]
pub struct BotConfig {
    pub token: String,
    pub channel_id: i64,
    pub source: BotSource,
    pub db_id: Option<i64>,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host: IpAddr,
    pub port: u16,
    pub force_https: bool,
    pub behind_proxy: bool,
    pub trusted_proxy_cidrs: Vec<String>,
    pub cors_allowed_origins: Vec<String>,
    pub cloudflared_enabled: bool,
    pub cloudflared_config: String,

    // User-configurable; raise if Telegram increases Bot API file size limits.
    pub telegram_max_file_size: u64,
    pub max_upload_size: u64,
    pub upload_chunk_size: u64,
    // User-configurable HLS segment target; adjust if upload ceiling changes.
    pub segment_target_size: u64,
    pub cache_dir: String,
    pub disk_cache_enabled: bool,
    pub cache_warmup_enabled: bool,
    pub segment_cache_size_mb: u32,
    pub segment_prefetch_count: u32,
    pub segment_prefetch_min_free_bytes: u64,

    pub enable_hw_accel: bool,
    pub preferred_encoder: String,
    pub vaapi_device: String,
    pub max_parallel_encodes: u32,
    pub video_bitrate: String,
    pub audio_bitrate: String,

    // INTERNAL FALLBACK ONLY — NOT loaded from settings (no registry entry).
    // Used by playlist-rendering paths (api/playlists.rs, api/playback/virtual_.rs)
    // when there is no job context to compute a real value from. Encode-time
    // call sites derive a per-job value via
    // media::process::target_segment_seconds_for_tier. Do not plumb this field
    // into the encode pipeline. See the comment block on that helper for full
    // background.
    pub hls_segment_duration: u32,
    pub audio_segment_duration: u32,

    pub job_timeout_seconds: u32,
    pub queue_timeout_seconds: u32,
    pub pending_upload_ttl_seconds: u32,
    pub pending_upload_cleanup_interval_seconds: u32,
    pub job_retention_days: u32,
    pub max_concurrent_jobs: u32,
    pub upload_rate_limit_window: u32,
    pub upload_rate_limit_max_requests: u32,
    pub max_pending_uploads_per_ip: u32,

    pub watch_poll_seconds: u32,
    pub watch_stable_seconds: u32,
    pub watch_video_extensions: Vec<String>,
    pub watch_ignore_suffixes: Vec<String>,

    pub bots: Vec<BotConfig>,
    pub upload_parallelism: u32,
    pub db_sync_enabled: bool,
    pub db_sync_bootstrap: String,
    pub db_auto_merge_interval_minutes: u32,
    pub db_auto_merge_file_id: String,
    pub db_auto_merge_bot_index: u32,
    pub webhook_url: String,

    pub tmdb_api_key: String,
    pub metadata_auto_fetch_enabled: bool,
    pub metadata_refresh_days: u32,
    pub intro_detection_enabled: bool,
    pub intro_chromaprint_enabled: bool,
    pub tac_comments_enabled: bool,

    pub abr_enabled: bool,
    pub enable_copy_mode: bool,
    pub virtual_abr_tiers: bool,
    pub abr_tiers: String,
    pub tier0_bitrates: String,
    pub tier0_bitrate_default: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".parse().expect("default host"),
            port: 5050,
            force_https: false,
            behind_proxy: false,
            trusted_proxy_cidrs: vec!["127.0.0.1/32".into(), "::1/128".into()],
            cors_allowed_origins: Vec::new(),
            cloudflared_enabled: false,
            cloudflared_config: String::new(),
            telegram_max_file_size: 20 * 1024 * 1024, // default matches current Bot API limit; raise if Telegram changes it
            max_upload_size: 100 * 1024 * 1024 * 1024,
            upload_chunk_size: 10 * 1024 * 1024,
            segment_target_size: 15 * 1024 * 1024, // user-configurable; adjust if upload ceiling changes
            cache_dir: "./cache/".into(),
            disk_cache_enabled: false,
            cache_warmup_enabled: false,
            segment_cache_size_mb: 200,
            segment_prefetch_count: 3,
            segment_prefetch_min_free_bytes: 0,
            enable_hw_accel: true,
            preferred_encoder: "vaapi".into(),
            vaapi_device: String::new(),
            max_parallel_encodes: 2,
            video_bitrate: "4M".into(),
            audio_bitrate: "128k".into(),
            hls_segment_duration: 4,
            audio_segment_duration: 30,
            job_timeout_seconds: 7200,
            queue_timeout_seconds: 7200,
            pending_upload_ttl_seconds: 86_400,
            pending_upload_cleanup_interval_seconds: 300,
            job_retention_days: 0,
            max_concurrent_jobs: 1,
            upload_rate_limit_window: 60,
            upload_rate_limit_max_requests: 500,
            max_pending_uploads_per_ip: 5,
            watch_poll_seconds: 5,
            watch_stable_seconds: 30,
            watch_video_extensions: vec![
                ".mp4".into(),
                ".mkv".into(),
                ".avi".into(),
                ".mov".into(),
                ".webm".into(),
                ".ts".into(),
                ".m4v".into(),
                ".flv".into(),
            ],
            watch_ignore_suffixes: vec![
                ".part".into(),
                ".crdownload".into(),
                ".tmp".into(),
                ".partial".into(),
            ],
            bots: Vec::new(),
            upload_parallelism: 12,
            db_sync_enabled: true,
            db_sync_bootstrap: String::new(),
            db_auto_merge_interval_minutes: 0,
            db_auto_merge_file_id: String::new(),
            db_auto_merge_bot_index: 0,
            webhook_url: String::new(),
            tmdb_api_key: String::new(),
            metadata_auto_fetch_enabled: false,
            metadata_refresh_days: 30,
            intro_detection_enabled: true,
            intro_chromaprint_enabled: true,
            tac_comments_enabled: true,
            abr_enabled: true,
            enable_copy_mode: true,
            virtual_abr_tiers: false,
            abr_tiers: "1080:10M,720:5M,480:2M,360:1200k".into(),
            tier0_bitrates: "2160:60M,1080:30M,720:15M,480:5M".into(),
            tier0_bitrate_default: "15M".into(),
        }
    }
}

impl Config {
    pub fn load(conn: &Connection) -> Result<Self> {
        let values = effective_setting_values(conn)?;
        Self::from_values(conn, &values)
    }

    pub fn from_values(conn: &Connection, values: &BTreeMap<&'static str, String>) -> Result<Self> {
        let mut cfg = Self::default();

        for (key, value) in values {
            apply_setting(&mut cfg, key, value, "effective");
        }

        cfg.bots = build_bot_pool(conn)?;

        enforce_invariants(&mut cfg);

        Ok(cfg)
    }

    pub fn setting_values(&self) -> BTreeMap<&'static str, String> {
        settings_registry::SETTINGS
            .iter()
            .map(|s| (s.key, self.setting_value(s.key)))
            .collect()
    }

    fn setting_value(&self, key: &str) -> String {
        match key {
            "HOST" => self.host.to_string(),
            "PORT" => self.port.to_string(),
            "FORCE_HTTPS" => self.force_https.to_string(),
            "BEHIND_PROXY" => self.behind_proxy.to_string(),
            "TRUSTED_PROXY_CIDRS" => self.trusted_proxy_cidrs.join(","),
            "CORS_ALLOWED_ORIGINS" => self.cors_allowed_origins.join(","),
            "CLOUDFLARED_ENABLED" => self.cloudflared_enabled.to_string(),
            "CLOUDFLARED_CONFIG" => self.cloudflared_config.clone(),
            "TELEGRAM_MAX_FILE_SIZE" => self.telegram_max_file_size.to_string(),
            "MAX_UPLOAD_SIZE" => self.max_upload_size.to_string(),
            "UPLOAD_CHUNK_SIZE" => self.upload_chunk_size.to_string(),
            "SEGMENT_TARGET_SIZE" => self.segment_target_size.to_string(),
            "CACHE_DIR" => self.cache_dir.clone(),
            "DISK_CACHE_ENABLED" => self.disk_cache_enabled.to_string(),
            "CACHE_WARMUP_ENABLED" => self.cache_warmup_enabled.to_string(),
            "SEGMENT_CACHE_SIZE_MB" => self.segment_cache_size_mb.to_string(),
            "SEGMENT_PREFETCH_COUNT" => self.segment_prefetch_count.to_string(),
            "SEGMENT_PREFETCH_MIN_FREE_BYTES" => self.segment_prefetch_min_free_bytes.to_string(),
            "ENABLE_HW_ACCEL" => self.enable_hw_accel.to_string(),
            "PREFERRED_ENCODER" => self.preferred_encoder.clone(),
            "VAAPI_DEVICE" => self.vaapi_device.clone(),
            "MAX_PARALLEL_ENCODES" => self.max_parallel_encodes.to_string(),
            "VIDEO_BITRATE" => self.video_bitrate.clone(),
            "AUDIO_BITRATE" => self.audio_bitrate.clone(),
            "AUDIO_SEGMENT_DURATION" => self.audio_segment_duration.to_string(),
            "JOB_TIMEOUT_SECONDS" => self.job_timeout_seconds.to_string(),
            "QUEUE_TIMEOUT_SECONDS" => self.queue_timeout_seconds.to_string(),
            "PENDING_UPLOAD_TTL_SECONDS" => self.pending_upload_ttl_seconds.to_string(),
            "PENDING_UPLOAD_CLEANUP_INTERVAL_SECONDS" => {
                self.pending_upload_cleanup_interval_seconds.to_string()
            }
            "JOB_RETENTION_DAYS" => self.job_retention_days.to_string(),
            "MAX_CONCURRENT_JOBS" => self.max_concurrent_jobs.to_string(),
            "UPLOAD_RATE_LIMIT_WINDOW" => self.upload_rate_limit_window.to_string(),
            "UPLOAD_RATE_LIMIT_MAX_REQUESTS" => self.upload_rate_limit_max_requests.to_string(),
            "MAX_PENDING_UPLOADS_PER_IP" => self.max_pending_uploads_per_ip.to_string(),
            "WATCH_POLL_SECONDS" => self.watch_poll_seconds.to_string(),
            "WATCH_STABLE_SECONDS" => self.watch_stable_seconds.to_string(),
            "WATCH_VIDEO_EXTENSIONS" => self.watch_video_extensions.join(","),
            "WATCH_IGNORE_SUFFIXES" => self.watch_ignore_suffixes.join(","),
            "UPLOAD_PARALLELISM" => self.upload_parallelism.to_string(),
            "DB_SYNC_ENABLED" => self.db_sync_enabled.to_string(),
            "DB_SYNC_BOOTSTRAP" => self.db_sync_bootstrap.clone(),
            "DB_AUTO_MERGE_INTERVAL_MINUTES" => self.db_auto_merge_interval_minutes.to_string(),
            "DB_AUTO_MERGE_FILE_ID" => self.db_auto_merge_file_id.clone(),
            "DB_AUTO_MERGE_BOT_INDEX" => self.db_auto_merge_bot_index.to_string(),
            "WEBHOOK_URL" => self.webhook_url.clone(),
            "TMDB_API_KEY" => {
                let masked = if self.tmdb_api_key.len() > 4 {
                    format!(
                        "{}...{}",
                        &self.tmdb_api_key[..2],
                        &self.tmdb_api_key[self.tmdb_api_key.len() - 2..]
                    )
                } else if self.tmdb_api_key.is_empty() {
                    String::new()
                } else {
                    "***".to_string()
                };
                masked
            }
            "METADATA_AUTO_FETCH_ENABLED" => self.metadata_auto_fetch_enabled.to_string(),
            "METADATA_REFRESH_DAYS" => self.metadata_refresh_days.to_string(),
            "INTRO_DETECTION_ENABLED" => self.intro_detection_enabled.to_string(),
            "INTRO_CHROMAPRINT_ENABLED" => self.intro_chromaprint_enabled.to_string(),
            "TAC_COMMENTS_ENABLED" => self.tac_comments_enabled.to_string(),
            "ABR_ENABLED" => self.abr_enabled.to_string(),
            "ENABLE_COPY_MODE" => self.enable_copy_mode.to_string(),
            "VIRTUAL_ABR_TIERS" => self.virtual_abr_tiers.to_string(),
            "ABR_TIERS" => self.abr_tiers.clone(),
            "TIER0_BITRATES" => self.tier0_bitrates.clone(),
            "TIER0_BITRATE_DEFAULT" => self.tier0_bitrate_default.clone(),
            _ => String::new(),
        }
    }
}

pub fn apply_normalized_settings(cfg: &mut Config, settings: &HashMap<String, String>) {
    for (key, value) in settings {
        apply_setting(cfg, key, value, "runtime");
    }
    enforce_invariants(cfg);
}

pub fn env_or_default_value(key: &str) -> Option<String> {
    let spec = settings_registry::setting_spec(key)?;
    let default = settings_registry::default_settings()
        .get(spec.key)
        .cloned()
        .unwrap_or_else(|| spec.default.to_string());
    let Ok(raw) = std::env::var(spec.env) else {
        return Some(default);
    };
    match settings_registry::normalize_str_for_key(spec.key, &raw) {
        Ok(value) => Some(value),
        Err(e) => {
            tracing::warn!(
                key = spec.key,
                env = spec.env,
                error = %e,
                "invalid environment config value; default kept"
            );
            Some(default)
        }
    }
}

pub fn effective_setting_values(conn: &Connection) -> Result<BTreeMap<&'static str, String>> {
    let mut values = settings_registry::default_settings();
    for spec in settings_registry::SETTINGS {
        if let Ok(raw) = std::env::var(spec.env) {
            match settings_registry::normalize_str_for_key(spec.key, &raw) {
                Ok(value) => {
                    values.insert(spec.key, value);
                }
                Err(e) => tracing::warn!(
                    key = spec.key,
                    env = spec.env,
                    error = %e,
                    "invalid environment config value; default kept"
                ),
            }
        }
    }
    for (key, raw) in db::get_all_settings(conn)? {
        match settings_registry::setting_spec(&key) {
            Some(spec) => match settings_registry::normalize_str_for_key(spec.key, &raw) {
                Ok(value) => {
                    values.insert(spec.key, value);
                }
                Err(e) => tracing::warn!(
                    key = %key,
                    error = %e,
                    "invalid DB config value; lower-precedence value kept"
                ),
            },
            None => tracing::warn!(key = %key, "settings table holds unknown key; ignored"),
        }
    }
    Ok(values)
}

fn apply_setting(cfg: &mut Config, key: &str, value: &str, source: &str) {
    let result: Result<(), String> = (|| -> Result<(), String> {
        match key {
            "HOST" => {
                cfg.host = value
                    .parse()
                    .map_err(|e: std::net::AddrParseError| e.to_string())?;
            }
            "PORT" => cfg.port = parse_int(value)?,
            "FORCE_HTTPS" => cfg.force_https = parse_bool(value)?,
            "BEHIND_PROXY" => cfg.behind_proxy = parse_bool(value)?,
            "TRUSTED_PROXY_CIDRS" => cfg.trusted_proxy_cidrs = settings_registry::parse_list(value),
            "CORS_ALLOWED_ORIGINS" => {
                cfg.cors_allowed_origins = settings_registry::parse_list(value)
            }
            "CLOUDFLARED_ENABLED" => cfg.cloudflared_enabled = parse_bool(value)?,
            "CLOUDFLARED_CONFIG" => cfg.cloudflared_config = value.to_string(),
            "TELEGRAM_MAX_FILE_SIZE" => cfg.telegram_max_file_size = parse_int(value)?,
            "MAX_UPLOAD_SIZE" => cfg.max_upload_size = parse_int(value)?,
            "UPLOAD_CHUNK_SIZE" => cfg.upload_chunk_size = parse_int(value)?,
            "SEGMENT_TARGET_SIZE" => cfg.segment_target_size = parse_int(value)?,
            "CACHE_DIR" => cfg.cache_dir = value.to_string(),
            "DISK_CACHE_ENABLED" => cfg.disk_cache_enabled = parse_bool(value)?,
            "CACHE_WARMUP_ENABLED" => cfg.cache_warmup_enabled = parse_bool(value)?,
            "SEGMENT_CACHE_SIZE_MB" => cfg.segment_cache_size_mb = parse_int(value)?,
            "SEGMENT_PREFETCH_COUNT" => cfg.segment_prefetch_count = parse_int(value)?,
            "SEGMENT_PREFETCH_MIN_FREE_BYTES" => {
                cfg.segment_prefetch_min_free_bytes = parse_int(value)?
            }
            "ENABLE_HW_ACCEL" => cfg.enable_hw_accel = parse_bool(value)?,
            "PREFERRED_ENCODER" => cfg.preferred_encoder = value.to_string(),
            "VAAPI_DEVICE" => cfg.vaapi_device = value.to_string(),
            "MAX_PARALLEL_ENCODES" => cfg.max_parallel_encodes = parse_int(value)?,
            "VIDEO_BITRATE" => cfg.video_bitrate = value.to_string(),
            "AUDIO_BITRATE" => cfg.audio_bitrate = value.to_string(),
            "AUDIO_SEGMENT_DURATION" => cfg.audio_segment_duration = parse_int(value)?,
            "JOB_TIMEOUT_SECONDS" => cfg.job_timeout_seconds = parse_int(value)?,
            "QUEUE_TIMEOUT_SECONDS" => cfg.queue_timeout_seconds = parse_int(value)?,
            "PENDING_UPLOAD_TTL_SECONDS" => cfg.pending_upload_ttl_seconds = parse_int(value)?,
            "PENDING_UPLOAD_CLEANUP_INTERVAL_SECONDS" => {
                cfg.pending_upload_cleanup_interval_seconds = parse_int(value)?
            }
            "JOB_RETENTION_DAYS" => cfg.job_retention_days = parse_int(value)?,
            "MAX_CONCURRENT_JOBS" => cfg.max_concurrent_jobs = parse_int(value)?,
            "UPLOAD_RATE_LIMIT_WINDOW" => cfg.upload_rate_limit_window = parse_int(value)?,
            "UPLOAD_RATE_LIMIT_MAX_REQUESTS" => {
                cfg.upload_rate_limit_max_requests = parse_int(value)?
            }
            "MAX_PENDING_UPLOADS_PER_IP" => cfg.max_pending_uploads_per_ip = parse_int(value)?,
            "WATCH_POLL_SECONDS" => cfg.watch_poll_seconds = parse_int(value)?,
            "WATCH_STABLE_SECONDS" => cfg.watch_stable_seconds = parse_int(value)?,
            "WATCH_VIDEO_EXTENSIONS" => {
                cfg.watch_video_extensions = settings_registry::parse_list(value)
            }
            "WATCH_IGNORE_SUFFIXES" => {
                cfg.watch_ignore_suffixes = settings_registry::parse_list(value)
            }
            "UPLOAD_PARALLELISM" => cfg.upload_parallelism = parse_int(value)?,
            "DB_SYNC_ENABLED" => cfg.db_sync_enabled = parse_bool(value)?,
            "DB_SYNC_BOOTSTRAP" => cfg.db_sync_bootstrap = value.to_string(),
            "DB_AUTO_MERGE_INTERVAL_MINUTES" => {
                cfg.db_auto_merge_interval_minutes = parse_int(value)?
            }
            "DB_AUTO_MERGE_FILE_ID" => cfg.db_auto_merge_file_id = value.to_string(),
            "DB_AUTO_MERGE_BOT_INDEX" => cfg.db_auto_merge_bot_index = parse_int(value)?,
            "WEBHOOK_URL" => cfg.webhook_url = value.to_string(),
            "TMDB_API_KEY" => cfg.tmdb_api_key = value.to_string(),
            "METADATA_AUTO_FETCH_ENABLED" => cfg.metadata_auto_fetch_enabled = parse_bool(value)?,
            "METADATA_REFRESH_DAYS" => cfg.metadata_refresh_days = parse_int(value)?,
            "INTRO_DETECTION_ENABLED" => cfg.intro_detection_enabled = parse_bool(value)?,
            "INTRO_CHROMAPRINT_ENABLED" => cfg.intro_chromaprint_enabled = parse_bool(value)?,
            "TAC_COMMENTS_ENABLED" => cfg.tac_comments_enabled = parse_bool(value)?,
            "ABR_ENABLED" => cfg.abr_enabled = parse_bool(value)?,
            "ENABLE_COPY_MODE" => cfg.enable_copy_mode = parse_bool(value)?,
            "VIRTUAL_ABR_TIERS" => cfg.virtual_abr_tiers = parse_bool(value)?,
            "ABR_TIERS" => cfg.abr_tiers = value.to_string(),
            "TIER0_BITRATES" => cfg.tier0_bitrates = value.to_string(),
            "TIER0_BITRATE_DEFAULT" => cfg.tier0_bitrate_default = value.to_string(),
            _ => return Err(format!("unmapped key {key}")),
        }
        Ok(())
    })();
    match result {
        Ok(()) => tracing::debug!(key, value, source, "config override applied"),
        Err(e) => {
            tracing::warn!(key, value, source, error = %e, "invalid config value; default kept")
        }
    }
}

fn enforce_invariants(cfg: &mut Config) {
    if cfg.abr_enabled && cfg.virtual_abr_tiers {
        tracing::warn!(
            "ABR_ENABLED and VIRTUAL_ABR_TIERS are mutually exclusive; \
             disabling virtual ABR (spec invariant 1.5.10)"
        );
        cfg.virtual_abr_tiers = false;
    }
}

fn parse_int<T>(s: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    s.trim().parse::<T>().map_err(|e| e.to_string())
}

fn parse_bool(s: &str) -> Result<bool, String> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("true") {
        Ok(true)
    } else if t.eq_ignore_ascii_case("false") {
        Ok(false)
    } else {
        Err(format!("not a boolean: {s}"))
    }
}

pub fn is_valid_bot_token(s: &str) -> bool {
    let mut parts = s.splitn(2, ':');
    let id = parts.next().unwrap_or("");
    let secret = parts.next().unwrap_or("");
    if id.is_empty() || secret.is_empty() {
        return false;
    }
    if id.len() < 8 || id.len() > 12 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if secret.len() < 35 || secret.len() > 45 {
        return false;
    }
    secret
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn build_bot_pool(conn: &Connection) -> Result<Vec<BotConfig>> {
    let mut out: Vec<BotConfig> = Vec::new();
    let mut seen_tokens: HashSet<String> = HashSet::new();

    let mut env_pairs: Vec<(u32, String, String)> = Vec::new();
    for (k, v) in std::env::vars() {
        let Some(suffix) = k.strip_prefix("TELEGRAM_BOT_TOKEN_") else {
            continue;
        };
        let Ok(n) = suffix.parse::<u32>() else {
            continue;
        };
        if n == 0 {
            continue;
        }
        let Ok(channel_str) = std::env::var(format!("TELEGRAM_CHANNEL_ID_{n}")) else {
            tracing::warn!(
                suffix = n,
                "TELEGRAM_BOT_TOKEN_N set without matching CHANNEL_ID_N; skipping"
            );
            continue;
        };
        env_pairs.push((n, v, channel_str));
    }
    env_pairs.sort_by_key(|t| t.0);

    for (n, token, channel_str) in env_pairs {
        let token_t = token.trim().to_string();
        if token_t.starts_with("your_") {
            continue;
        }
        if !is_valid_bot_token(&token_t) {
            tracing::warn!(
                suffix = n,
                "TELEGRAM_BOT_TOKEN_N fails token format; skipping"
            );
            continue;
        }
        let channel_id: i64 = match channel_str.trim().parse() {
            Ok(v) if v < 0 => v,
            Ok(_) => {
                tracing::warn!(
                    suffix = n,
                    "TELEGRAM_CHANNEL_ID_N must be a negative integer; skipping"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(suffix = n, error = %e, "TELEGRAM_CHANNEL_ID_N not parseable; skipping");
                continue;
            }
        };
        if !seen_tokens.insert(token_t.clone()) {
            tracing::warn!(suffix = n, "duplicate bot token in env; skipping");
            continue;
        }
        out.push(BotConfig {
            token: token_t,
            channel_id,
            source: BotSource::Env,
            db_id: None,
            label: String::new(),
        });
    }

    for row in db::get_all_bots(conn)? {
        if !row.enabled {
            continue;
        }
        let token_t = row.token.trim().to_string();
        if token_t.starts_with("your_") {
            continue;
        }
        if !is_valid_bot_token(&token_t) {
            tracing::warn!(db_id = row.id, "bots row token fails format; skipping");
            continue;
        }
        if row.channel_id >= 0 {
            tracing::warn!(
                db_id = row.id,
                "bots row channel_id must be negative; skipping"
            );
            continue;
        }
        if !seen_tokens.insert(token_t.clone()) {
            tracing::warn!(db_id = row.id, "duplicate bot token in db; skipping");
            continue;
        }
        out.push(BotConfig {
            token: token_t,
            channel_id: row.channel_id,
            source: BotSource::Db,
            db_id: Some(row.id),
            label: row.label,
        });
    }

    Ok(out)
}
