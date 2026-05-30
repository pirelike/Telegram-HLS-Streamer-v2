use std::collections::BTreeMap;
use std::net::IpAddr;

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingType {
    Int,
    Bool,
    Str,
    List,
    Tiers,
}

#[derive(Debug, Clone, Copy)]
pub struct SettingSpec {
    pub key: &'static str,
    pub env: &'static str,
    pub category: &'static str,
    pub setting_type: SettingType,
    pub default: &'static str,
    pub description: &'static str,
    pub min: Option<i64>,
    pub max: Option<i64>,
}

macro_rules! setting {
    ($key:expr, $env:expr, $category:expr, $kind:expr, $default:expr, $description:expr, $min:expr, $max:expr) => {
        SettingSpec {
            key: $key,
            env: $env,
            category: $category,
            setting_type: $kind,
            default: $default,
            description: $description,
            min: $min,
            max: $max,
        }
    };
}

#[rustfmt::skip]
pub const SETTINGS: &[SettingSpec] = &[
    setting!("ADMIN_USER", "ADMIN_USER", "server", SettingType::Str, "", "Admin username for HTTP Basic Auth. Leave empty to disable.", None, None),
    setting!("ADMIN_PASS", "ADMIN_PASS", "server", SettingType::Str, "", "Admin password for HTTP Basic Auth.", None, None),
    setting!("HOST", "LOCAL_HOST", "server", SettingType::Str, "0.0.0.0", "Bind address.", None, None),
    setting!("PORT", "LOCAL_PORT", "server", SettingType::Int, "5050", "Bind port.", Some(1), Some(65535)),
    setting!("FORCE_HTTPS", "FORCE_HTTPS", "server", SettingType::Bool, "false", "Redirect HTTP to HTTPS.", None, None),
    setting!("BEHIND_PROXY", "BEHIND_PROXY", "server", SettingType::Bool, "false", "Trust forwarded headers from configured proxies.", None, None),
    setting!("TRUSTED_PROXY_CIDRS", "TRUSTED_PROXY_CIDRS", "server", SettingType::List, "127.0.0.1/32,::1/128", "Trusted proxy CIDR ranges.", None, None),
    setting!("CORS_ALLOWED_ORIGINS", "CORS_ALLOWED_ORIGINS", "server", SettingType::List, "", "Allowed CORS origins.", None, None),
    setting!("CLOUDFLARED_ENABLED", "CLOUDFLARED_ENABLED", "cloudflared", SettingType::Bool, "false", "Enable Cloudflared tunnel management.", None, None),
    setting!("CLOUDFLARED_CONFIG", "CLOUDFLARED_CONFIG", "cloudflared", SettingType::Str, "", "Path to the Cloudflared tunnel config file.", None, None),
    setting!("TELEGRAM_MAX_FILE_SIZE", "TELEGRAM_MAX_FILE_SIZE", "file_handling", SettingType::Int, "20971520", "Telegram per-file upload ceiling. Raise if Telegram increases Bot API limits.", Some(1), None),
    setting!("MAX_UPLOAD_SIZE", "MAX_UPLOAD_SIZE", "file_handling", SettingType::Int, "107374182400", "Maximum accepted client upload size.", Some(1), None),
    setting!("UPLOAD_CHUNK_SIZE", "UPLOAD_CHUNK_SIZE", "file_handling", SettingType::Int, "10485760", "Server-advertised upload chunk size.", Some(1), None),
    setting!("SEGMENT_TARGET_SIZE", "SEGMENT_TARGET_SIZE", "file_handling", SettingType::Int, "15728640", "Preferred HLS segment size. User-configurable; adjust if upload ceiling changes.", Some(1), None),
    setting!("CACHE_DIR", "CACHE_DIR", "file_handling", SettingType::Str, "./cache/", "Ephemeral cache directory wiped on startup.", None, None),
    setting!("DISK_CACHE_ENABLED", "DISK_CACHE_ENABLED", "file_handling", SettingType::Bool, "false", "Store cached segment payloads on disk instead of memory-only cache.", None, None),
    setting!("CACHE_WARMUP_ENABLED", "CACHE_WARMUP_ENABLED", "file_handling", SettingType::Bool, "false", "Enable cache warm-up behavior.", None, None),
    setting!("SEGMENT_CACHE_SIZE_MB", "SEGMENT_CACHE_SIZE_MB", "file_handling", SettingType::Int, "200", "Segment cache budget in MB.", Some(0), None),
    setting!("SEGMENT_PREFETCH_COUNT", "SEGMENT_PREFETCH_COUNT", "file_handling", SettingType::Int, "3", "Segments to prefetch ahead.", Some(0), None),
    setting!("SEGMENT_PREFETCH_MIN_FREE_BYTES", "SEGMENT_PREFETCH_MIN_FREE_BYTES", "file_handling", SettingType::Int, "0", "Minimum free cache bytes before prefetch.", Some(0), None),
    setting!("ENABLE_HW_ACCEL", "ENABLE_HARDWARE_ACCELERATION", "hardware", SettingType::Bool, "true", "Enable hardware encoders.", None, None),
    setting!("PREFERRED_ENCODER", "PREFERRED_ENCODER", "hardware", SettingType::Str, "vaapi", "Preferred hardware encoder.", None, None),
    setting!("VAAPI_DEVICE", "VAAPI_DEVICE", "hardware", SettingType::Str, "", "VAAPI render device path.", None, None),
    setting!("MAX_PARALLEL_ENCODES", "MAX_PARALLEL_ENCODES", "hardware", SettingType::Int, "2", "Max parallel video encodes per job.", Some(1), None),
    setting!("VIDEO_BITRATE", "VIDEO_BITRATE", "hardware", SettingType::Str, "4M", "Default video bitrate.", None, None),
    setting!("AUDIO_BITRATE", "AUDIO_BITRATE", "hardware", SettingType::Str, "128k", "Default audio bitrate.", None, None),
    setting!("HLS_SEGMENT_DURATION", "HLS_SEGMENT_DURATION", "file_handling", SettingType::Int, "4", "Fallback HLS video segment duration in seconds for playlist rendering (not used during encoding).", Some(2), Some(10)),
    setting!("AUDIO_SEGMENT_DURATION", "AUDIO_SEGMENT_DURATION", "file_handling", SettingType::Int, "30", "Audio segment duration in seconds.", Some(1), None),
    setting!("ABR_ENABLED", "ABR_ENABLED", "adaptive_bitrate", SettingType::Bool, "true", "Produce eager ABR tiers.", None, None),
    setting!("ENABLE_COPY_MODE", "ENABLE_COPY_MODE", "adaptive_bitrate", SettingType::Bool, "true", "Enable tier-0 passthrough.", None, None),
    setting!("VIRTUAL_ABR_TIERS", "VIRTUAL_ABR_TIERS", "adaptive_bitrate", SettingType::Bool, "false", "Transcode lower tiers on demand.", None, None),
    setting!("ABR_TIERS", "ABR_TIERS", "adaptive_bitrate", SettingType::Tiers, "1080:10M,720:5M,480:2M,360:1200k", "Configured ABR tiers.", None, None),
    setting!("TIER0_BITRATES", "TIER0_BITRATES", "adaptive_bitrate", SettingType::Tiers, "2160:60M,1080:30M,720:15M,480:5M", "Source-height tier-0 bitrates.", None, None),
    setting!("TIER0_BITRATE_DEFAULT", "TIER0_BITRATE_DEFAULT", "adaptive_bitrate", SettingType::Str, "15M", "Fallback tier-0 bitrate.", None, None),
    setting!("JOB_TIMEOUT_SECONDS", "JOB_TIMEOUT_SECONDS", "reliability", SettingType::Int, "7200", "Per-job runtime cap.", Some(1), None),
    setting!("QUEUE_TIMEOUT_SECONDS", "QUEUE_TIMEOUT_SECONDS", "reliability", SettingType::Int, "7200", "Maximum time a job can wait in the queue before timing out.", Some(1), None),
    setting!("PENDING_UPLOAD_TTL_SECONDS", "PENDING_UPLOAD_TTL_SECONDS", "reliability", SettingType::Int, "86400", "Idle pending upload expiry.", Some(1), None),
    setting!("PENDING_UPLOAD_CLEANUP_INTERVAL_SECONDS", "PENDING_UPLOAD_CLEANUP_INTERVAL_SECONDS", "reliability", SettingType::Int, "300", "Pending upload sweeper interval.", Some(1), None),
    setting!("JOB_RETENTION_DAYS", "JOB_RETENTION_DAYS", "reliability", SettingType::Int, "0", "Completed-job retention period.", Some(0), None),
    setting!("MAX_CONCURRENT_JOBS", "MAX_CONCURRENT_JOBS", "reliability", SettingType::Int, "1", "Queue worker count.", Some(1), None),
    setting!("UPLOAD_RATE_LIMIT_WINDOW", "UPLOAD_RATE_LIMIT_WINDOW", "rate_limiting", SettingType::Int, "60", "Upload rate-limit window.", Some(1), None),
    setting!("UPLOAD_RATE_LIMIT_MAX_REQUESTS", "UPLOAD_RATE_LIMIT_MAX_REQUESTS", "rate_limiting", SettingType::Int, "100", "Upload requests per window.", Some(1), None),
    setting!("MAX_PENDING_UPLOADS_PER_IP", "MAX_PENDING_UPLOADS_PER_IP", "rate_limiting", SettingType::Int, "5", "Pending uploads per IP.", Some(0), None),
    setting!("WATCH_POLL_SECONDS", "WATCH_POLL_SECONDS", "watch_folder", SettingType::Int, "5", "Watch-folder scan interval.", Some(1), None),
    setting!("WATCH_STABLE_SECONDS", "WATCH_STABLE_SECONDS", "watch_folder", SettingType::Int, "30", "File stability threshold.", Some(1), None),
    setting!("WATCH_VIDEO_EXTENSIONS", "WATCH_VIDEO_EXTENSIONS", "watch_folder", SettingType::List, "mp4,mkv,avi,mov,webm,ts,m4v,flv", "Allowed watch-folder video extensions.", None, None),
    setting!("WATCH_IGNORE_SUFFIXES", "WATCH_IGNORE_SUFFIXES", "watch_folder", SettingType::List, ".part,.crdownload,.tmp,.partial", "Ignored partial-download suffixes.", None, None),
    setting!("UPLOAD_PARALLELISM", "UPLOAD_PARALLELISM", "telegram", SettingType::Int, "12", "Cross-bot upload semaphore size.", Some(1), None),
    setting!("DB_SYNC_ENABLED", "DB_SYNC_ENABLED", "telegram", SettingType::Bool, "true", "Automatically snapshot and upload streamer.db after completed jobs.", None, None),
    setting!("DB_SYNC_BOOTSTRAP", "DB_SYNC_BOOTSTRAP", "telegram", SettingType::Str, "", "Latest DB sync bootstrap descriptor.", None, None),
    setting!("DB_AUTO_MERGE_INTERVAL_MINUTES", "DB_AUTO_MERGE_INTERVAL_MINUTES", "telegram", SettingType::Int, "0", "DB auto-merge cadence.", Some(0), None),
    setting!("DB_AUTO_MERGE_FILE_ID", "DB_AUTO_MERGE_FILE_ID", "telegram", SettingType::Str, "", "Telegram file_id for DB auto-merge.", None, None),
    setting!("DB_AUTO_MERGE_BOT_INDEX", "DB_AUTO_MERGE_BOT_INDEX", "telegram", SettingType::Int, "0", "Bot index used for DB auto-merge.", Some(0), None),
    setting!("WEBHOOK_URL", "WEBHOOK_URL", "telegram", SettingType::Str, "", "POST terminal job events to this URL.", None, None),
    setting!("TMDB_API_KEY", "TMDB_API_KEY", "metadata", SettingType::Str, "", "TMDB API key for movie/TV metadata fetching.", None, None),
    setting!("METADATA_AUTO_FETCH_ENABLED", "METADATA_AUTO_FETCH_ENABLED", "metadata", SettingType::Bool, "false", "Automatically fetch metadata from providers after upload.", None, None),
    setting!("METADATA_REFRESH_DAYS", "METADATA_REFRESH_DAYS", "metadata", SettingType::Int, "30", "Days before cached metadata is eligible for refresh.", Some(1), None),
    setting!("INTRO_DETECTION_ENABLED", "INTRO_DETECTION_ENABLED", "metadata", SettingType::Bool, "true", "Enable automatic intro/outro marker detection.", None, None),
    setting!("INTRO_CHROMAPRINT_ENABLED", "INTRO_CHROMAPRINT_ENABLED", "metadata", SettingType::Bool, "true", "Enable Chromaprint-based audio fingerprint detection for markers.", None, None),
    setting!("TAC_COMMENTS_ENABLED", "TAC_COMMENTS_ENABLED", "metadata", SettingType::Bool, "true", "Enable Anime Community comment sections on watch pages.", None, None),
];

pub fn is_public_setting_key(key: &str) -> bool {
    setting_spec(key).is_some()
}

pub fn setting_spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|s| s.key == key)
}

pub fn default_settings() -> BTreeMap<&'static str, String> {
    SETTINGS
        .iter()
        .map(|s| {
            (
                s.key,
                normalize_str(s, s.default).expect("valid default setting"),
            )
        })
        .collect()
}

pub fn normalize_json(key: &str, value: &Value) -> Result<String, String> {
    let spec = setting_spec(key).ok_or_else(|| format!("unknown setting key: {key}"))?;
    let raw = match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(items) => {
            let mut vals = Vec::new();
            for item in items {
                match item {
                    Value::String(s) => vals.push(s.clone()),
                    _ => return Err(format!("{key} list values must be strings")),
                }
            }
            vals.join(",")
        }
        _ => return Err(format!("{key} has unsupported JSON value")),
    };
    normalize_str_inner(spec, &raw, false)
}

pub fn normalize_str_for_key(key: &str, value: &str) -> Result<String, String> {
    let spec = setting_spec(key).ok_or_else(|| format!("unknown setting key: {key}"))?;
    normalize_str(spec, value)
}

pub fn categories_for_values(values: &BTreeMap<&'static str, String>) -> Value {
    let mut out = BTreeMap::new();
    for spec in SETTINGS {
        let entry = out
            .entry(spec.category)
            .or_insert_with(|| json!({ "label": category_label(spec.category), "settings": [] }));
        let current = values
            .get(spec.key)
            .cloned()
            .unwrap_or_else(|| spec.default.to_string());
        entry["settings"]
            .as_array_mut()
            .expect("settings array")
            .push(json!({
                "key": spec.key,
                "env": spec.env,
                "type": setting_type_name(spec.setting_type),
                "value": value_to_json(spec, &current),
                "default": value_to_json(spec, spec.default),
                "description": spec.description,
            }));
    }
    json!(out)
}

pub fn parse_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn normalize_str_inner(
    spec: &SettingSpec,
    value: &str,
    strip_comments: bool,
) -> Result<String, String> {
    let value = if strip_comments {
        strip_inline_comment(value)
    } else {
        value.trim().to_string()
    };
    let value = value.as_str();
    match spec.setting_type {
        SettingType::Int => normalize_int(spec, value),
        SettingType::Bool => normalize_bool(value),
        SettingType::Str => normalize_string(spec.key, value),
        SettingType::List => normalize_list(spec.key, value),
        SettingType::Tiers => normalize_tiers(value),
    }
}

fn normalize_str(spec: &SettingSpec, value: &str) -> Result<String, String> {
    normalize_str_inner(spec, value, true)
}

fn strip_inline_comment(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(unquoted) = unquote_env_value(trimmed) {
        return unquoted;
    }
    trimmed
        .split_once('#')
        .map(|(head, _)| head)
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

fn unquote_env_value(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn normalize_int(spec: &SettingSpec, value: &str) -> Result<String, String> {
    let parsed = value
        .trim()
        .parse::<i64>()
        .map_err(|e| format!("not an integer: {e}"))?;
    if let Some(min) = spec.min {
        if parsed < min {
            return Err(format!("must be >= {min}"));
        }
    }
    if let Some(max) = spec.max {
        if parsed > max {
            return Err(format!("must be <= {max}"));
        }
    }
    Ok(parsed.to_string())
}

fn normalize_bool(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "true" | "1" => Ok("true".to_string()),
        "false" | "0" => Ok("false".to_string()),
        _ => Err(format!(
            "not a boolean (expected true/false/1/0): {trimmed}"
        )),
    }
}

fn normalize_string(key: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    match key {
        "HOST" => {
            value
                .parse::<IpAddr>()
                .map_err(|e| format!("invalid bind host: {e}"))?;
        }
        "PREFERRED_ENCODER" if !matches!(value, "vaapi" | "nvenc" | "qsv" | "cpu") => {
            return Err("must be one of vaapi, nvenc, qsv, cpu".to_string());
        }
        "VAAPI_DEVICE" if !value.is_empty() && !is_vaapi_device(value) => {
            return Err("must be empty or /dev/dri/renderD<N>".to_string());
        }
        "VIDEO_BITRATE" | "AUDIO_BITRATE" | "TIER0_BITRATE_DEFAULT" if !is_bitrate(value) => {
            return Err("invalid bitrate".to_string());
        }
        "DB_AUTO_MERGE_FILE_ID" if !value.is_empty() && !is_telegram_file_id(value) => {
            return Err("invalid Telegram file_id".to_string());
        }
        _ => {}
    }
    Ok(value.to_string())
}

fn normalize_list(key: &str, value: &str) -> Result<String, String> {
    let mut values = parse_list(value);
    match key {
        "TRUSTED_PROXY_CIDRS" => {
            for cidr in &values {
                validate_cidr(cidr)?;
            }
        }
        "CORS_ALLOWED_ORIGINS" => {
            for origin in &values {
                validate_origin(origin)?;
            }
        }
        "WATCH_VIDEO_EXTENSIONS" => {
            for ext in &mut values {
                if !ext.starts_with('.') {
                    *ext = format!(".{ext}");
                }
                validate_simple_list_item(ext)?;
            }
        }
        "WATCH_IGNORE_SUFFIXES" => {
            for suffix in &values {
                validate_simple_list_item(suffix)?;
            }
        }
        _ => {}
    }
    Ok(values.join(","))
}

fn normalize_tiers(value: &str) -> Result<String, String> {
    let mut pairs = Vec::new();
    for pair in parse_list(value) {
        let (height, bitrate) = pair
            .split_once(':')
            .ok_or_else(|| format!("invalid tier pair: {pair}"))?;
        let height = height
            .trim()
            .parse::<u32>()
            .map_err(|e| format!("invalid tier height: {e}"))?;
        if height == 0 {
            return Err("tier height must be positive".to_string());
        }
        let bitrate = bitrate.trim();
        if !is_bitrate(bitrate) {
            return Err(format!("invalid tier bitrate: {bitrate}"));
        }
        pairs.push(format!("{height}:{bitrate}"));
    }
    if pairs.is_empty() {
        return Err("at least one tier is required".to_string());
    }
    Ok(pairs.join(","))
}

fn value_to_json(spec: &SettingSpec, value: &str) -> Value {
    match spec.setting_type {
        SettingType::Int => value
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| json!(value)),
        SettingType::Bool => json!(value.eq_ignore_ascii_case("true")),
        SettingType::List => json!(parse_list(value)),
        SettingType::Str | SettingType::Tiers => json!(value),
    }
}

pub fn setting_type_name(setting_type: SettingType) -> &'static str {
    match setting_type {
        SettingType::Int => "int",
        SettingType::Bool => "bool",
        SettingType::Str => "str",
        SettingType::List => "list",
        SettingType::Tiers => "tiers",
    }
}

fn category_label(category: &str) -> &'static str {
    match category {
        "server" => "Server",
        "file_handling" => "File Handling",
        "hardware" => "Hardware Acceleration",
        "hls" => "HLS",
        "adaptive_bitrate" => "Adaptive Bitrate",
        "reliability" => "Reliability",
        "rate_limiting" => "Rate Limiting",
        "watch_folder" => "Watch Folder",
        "telegram" => "Telegram",
        "cloudflared" => "Cloudflared",
        _ => "Settings",
    }
}

fn validate_cidr(value: &str) -> Result<(), String> {
    let (ip, prefix) = value
        .split_once('/')
        .ok_or_else(|| format!("invalid CIDR: {value}"))?;
    let ip: IpAddr = ip.parse().map_err(|e| format!("invalid CIDR IP: {e}"))?;
    let prefix = prefix
        .parse::<u8>()
        .map_err(|e| format!("invalid CIDR prefix: {e}"))?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if prefix > max {
        return Err(format!("CIDR prefix must be <= {max}"));
    }
    Ok(())
}

fn validate_origin(value: &str) -> Result<(), String> {
    if value == "*" {
        return Ok(());
    }
    let rest = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))
        .ok_or_else(|| "origin must start with http:// or https://".to_string())?;
    if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return Err("origin must not include path, query, or fragment".to_string());
    }
    Ok(())
}

fn validate_simple_list_item(value: &str) -> Result<(), String> {
    if value.is_empty() || value.contains('/') || value.contains('\\') || value.contains('\n') {
        return Err("invalid list item".to_string());
    }
    Ok(())
}

fn is_vaapi_device(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("/dev/dri/renderD") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

fn is_bitrate(value: &str) -> bool {
    let Some(unit) = value.bytes().last() else {
        return false;
    };
    if !matches!(unit, b'k' | b'K' | b'm' | b'M' | b'g' | b'G') {
        return false;
    }
    let number = &value[..value.len() - 1];
    if number.is_empty() {
        return false;
    }
    let mut dot_count = 0;
    let mut seen_dot = false;
    let mut digits_before_dot = 0;
    let mut digits_after_dot = 0;
    for b in number.bytes() {
        if b == b'.' {
            dot_count += 1;
            seen_dot = true;
            if dot_count > 1 {
                return false;
            }
        } else if !b.is_ascii_digit() {
            return false;
        } else if seen_dot {
            digits_after_dot += 1;
        } else {
            digits_before_dot += 1;
        }
    }
    digits_before_dot > 0 && (!seen_dot || digits_after_dot > 0)
}

fn is_telegram_file_id(value: &str) -> bool {
    (50..=255).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_matches_public_key_list() {
        for spec in SETTINGS {
            let key = spec.key;
            assert!(setting_spec(key).is_some(), "{key}");
        }
    }

    #[test]
    fn validation_covers_special_setting_types() {
        assert_eq!(
            normalize_str_for_key("ABR_ENABLED", "TRUE").unwrap(),
            "true"
        );
        assert_eq!(
            normalize_str_for_key("DISK_CACHE_ENABLED", "TRUE").unwrap(),
            "true"
        );
        assert_eq!(normalize_str_for_key("ABR_ENABLED", "1").unwrap(), "true");
        assert_eq!(normalize_str_for_key("ABR_ENABLED", "0").unwrap(), "false");
        assert!(normalize_str_for_key("PORT", "70000").is_err());
        assert_eq!(
            normalize_str_for_key("WATCH_VIDEO_EXTENSIONS", "mp4,.mkv").unwrap(),
            ".mp4,.mkv"
        );
        assert!(normalize_str_for_key("PREFERRED_ENCODER", "x264").is_err());
        assert!(normalize_str_for_key("VAAPI_DEVICE", "/dev/dri/card0").is_err());
        assert!(normalize_str_for_key("VIDEO_BITRATE", "4000").is_err());
        assert!(normalize_str_for_key("VIDEO_BITRATE", ".5M").is_err());
        assert!(normalize_str_for_key("VIDEO_BITRATE", "5.M").is_err());
        assert!(normalize_str_for_key("ABR_TIERS", "720:5M,480:1200k").is_ok());
        assert!(normalize_str_for_key("TRUSTED_PROXY_CIDRS", "127.0.0.1/32").is_ok());
        assert!(normalize_str_for_key("CORS_ALLOWED_ORIGINS", "http://localhost:5050").is_ok());
        assert!(normalize_str_for_key("CORS_ALLOWED_ORIGINS", "http://localhost/x").is_err());
    }

    #[test]
    fn inline_env_comments_are_ignored_for_public_settings() {
        assert_eq!(
            normalize_str_for_key("TELEGRAM_MAX_FILE_SIZE", "20971520 # 20MB").unwrap(),
            "20971520"
        );
        assert_eq!(
            normalize_str_for_key("VAAPI_DEVICE", "/dev/dri/renderD129 # AMD").unwrap(),
            "/dev/dri/renderD129"
        );
    }

    #[test]
    fn json_api_preserves_hash_in_values() {
        // JSON API path must not strip # from URL fragments
        let url = "https://x.com/hook#frag";
        assert_eq!(
            normalize_json("WEBHOOK_URL", &Value::String(url.to_string())).unwrap(),
            url
        );
    }

    #[test]
    fn env_path_strips_hash_comment() {
        // env/DB path must still strip # comments
        assert_eq!(
            normalize_str_for_key("WEBHOOK_URL", "https://x.com/hook#frag").unwrap(),
            "https://x.com/hook"
        );
        assert_eq!(
            normalize_str_for_key("WEBHOOK_URL", "\"https://x.com/hook#frag\"").unwrap(),
            "https://x.com/hook#frag"
        );
    }
}
