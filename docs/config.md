# `config.rs` — Developer Reference

The central configuration module. Defines the `Config` struct (every runtime knob), the `BotConfig`/`BotSource` types for the Telegram bot pool, and the multi-layered loading pipeline: hardcoded defaults → environment variable overrides → database overrides → invariant enforcement. Also handles runtime hot-reload via `apply_normalized_settings()` and provides helper parsers and token validation used during bootstrap.

---

## Types

### `BotSource`

**Signature:** `pub enum BotSource { Env, Db }`

**Purpose:** Tag each `BotConfig` with its origin — environment variable or database row. Used by the Telegram uploader to decide how to reference the bot (by env name or by DB id) and for display purposes.

**Variants:**
- `Env` — sourced from `TELEGRAM_BOT_TOKEN_N` / `TELEGRAM_CHANNEL_ID_N` env vars
- `Db` — sourced from the `bots` database table

Serialized as lowercase via `#[serde(rename_all = "lowercase")]`.

---

### `BotConfig`

**Signature:** `pub struct BotConfig { pub token: String, pub channel_id: i64, pub source: BotSource, pub db_id: Option<i64>, pub label: String }`

**Purpose:** A configured Telegram bot that the streamer uses to upload finished HLS segments. One `BotConfig` = one bot identity + one target channel.

| Field | Type | Purpose |
|---|---|---|
| `token` | `String` | Telegram Bot API token (`<id>:<secret>`) |
| `channel_id` | `i64` | Target chat/channel ID (always negative, e.g. `-1001234567890`) |
| `source` | `BotSource` | Whether this bot came from env vars or the DB |
| `db_id` | `Option<i64>` | If sourced from DB, the row primary key (used for updates/deletes) |
| `label` | `String` | Human-readable label from the DB (empty for env-sourced bots) |

---

### `Config`

**Signature:** `pub struct Config { ... }`

**Purpose:** The single source of truth for all runtime configuration. Every setting that can be tuned by the operator — network, encoding, upload, job lifecycle, ABR — lives here. Constructed by `Config::load()` and consumed read-only by all subsystems.

#### Server / network

| Field | Type | Default | Purpose |
|---|---|---|---|
| `host` | `IpAddr` | `0.0.0.0` | Bind address |
| `port` | `u16` | `5050` | Bind port |
| `admin_user` | `String` | `""` | HTTP Basic Auth username (empty = disabled) |
| `admin_pass` | `String` | `""` | HTTP Basic Auth password |
| `force_https` | `bool` | `false` | Redirect HTTP → HTTPS |
| `behind_proxy` | `bool` | `false` | Trust `X-Forwarded-*` headers |
| `trusted_proxy_cidrs` | `Vec<String>` | `["127.0.0.1/32", "::1/128"]` | CIDRs whose forwarded headers are trusted |
| `cors_allowed_origins` | `Vec<String>` | `[]` | Allowed CORS origins (empty = all) |
| `cloudflared_enabled` | `bool` | `false` | Auto-manage a Cloudflared tunnel |
| `cloudflared_config` | `String` | `""` | Path to the Cloudflared tunnel config file |

#### File handling / upload

| Field | Type | Default | Purpose |
|---|---|---|---|
| `telegram_max_file_size` | `u64` | `20 MiB` | Telegram's per-file ceiling (must not exceed 20 MB) |
| `max_upload_size` | `u64` | `100 GiB` | Maximum accepted client upload body |
| `upload_chunk_size` | `u64` | `10 MiB` | Chunk size advertised to upload clients |
| `segment_target_size` | `u64` | `15 MiB` | Preferred target size for HLS segments |
| `cache_dir` | `String` | `"./cache/"` | Ephemeral cache directory (wiped on startup) |
| `disk_cache_enabled` | `bool` | `false` | Store cached segment payloads on disk instead of memory-only |
| `cache_warmup_enabled` | `bool` | `false` | Enable cache warm-up behavior |
| `segment_cache_size_mb` | `u32` | `200` | In-memory segment cache budget (MB) |
| `segment_prefetch_count` | `u32` | `3` | Number of segments to prefetch ahead of playback |
| `segment_prefetch_min_free_bytes` | `u64` | `0` | Free cache bytes threshold before prefetch stops |
| `audio_segment_duration` | `u32` | `30` | Audio segment duration in seconds |

#### Hardware / encoding

| Field | Type | Default | Purpose |
|---|---|---|---|
| `enable_hw_accel` | `bool` | `true` | Enable hardware-accelerated encoding |
| `preferred_encoder` | `String` | `"vaapi"` | Encoder name to prefer |
| `vaapi_device` | `String` | `""` | VAAPI render device path (e.g. `/dev/dri/renderD128`) |
| `max_parallel_encodes` | `u32` | `2` | Concurrent encode process limit |
| `video_bitrate` | `String` | `"4M"` | Output video bitrate |
| `audio_bitrate` | `String` | `"128k"` | Output audio bitrate |

#### HLS

| Field | Type | Default | Purpose |
|---|---|---|---|
| `hls_segment_duration` | `u32` | `4` | Target segment duration in seconds |

#### Rate limiting

| Field | Type | Default | Purpose |
|---|---|---|---|
| `upload_rate_limit_window` | `u32` | `60` | Rate limit window in seconds |
| `upload_rate_limit_max_requests` | `u32` | `100` | Max requests per window |
| `max_pending_uploads_per_ip` | `u32` | `5` | Max concurrent pending uploads per client IP |

#### Folder watcher

| Field | Type | Default | Purpose |
|---|---|---|---|
| `watch_poll_seconds` | `u32` | `5` | How often to scan watched directories |
| `watch_stable_seconds` | `u32` | `30` | How long a file must be untouched before it is considered "ready" |
| `watch_video_extensions` | `Vec<String>` | `[".mp4", ".mkv", ...]` | Extensions that trigger a job |
| `watch_ignore_suffixes` | `Vec<String>` | `[".part", ".crdownload", ...]` | Suffixes to skip during watch |

#### Bot pool

| Field | Type | Default | Purpose |
|---|---|---|---|
| `bots` | `Vec<BotConfig>` | `[]` | Active Telegram bots |
| `upload_parallelism` | `u32` | `12` | Concurrent upload tasks per job |
| `db_sync_enabled` | `bool` | `true` | Auto-snapshot and upload streamer.db after completed jobs |
| `db_sync_bootstrap` | `String` | `""` | Latest auto-written DB sync descriptor for fresh-server restore |
| `db_auto_merge_interval_minutes` | `u32` | `0` | DB auto-merge interval (0 = disabled) |
| `db_auto_merge_file_id` | `String` | `""` | File ID for auto-merge targeting |
| `db_auto_merge_bot_index` | `u32` | `0` | Bot index for auto-merge |
| `webhook_url` | `String` | `""` | Outgoing webhook URL |

#### ABR

| Field | Type | Default | Purpose |
|---|---|---|---|
| `abr_enabled` | `bool` | `true` | Enable adaptive bitrate ladder |
| `enable_copy_mode` | `bool` | `true` | Enable stream-copy mode for compatible sources |
| `virtual_abr_tiers` | `bool` | `false` | Virtual ABR tiers (experimental, mutually exclusive with `abr_enabled`) |
| `abr_tiers` | `String` | `"1080:10M,..."` | ABR ladder bitrates by resolution |
| `tier0_bitrates` | `String` | `"2160:60M,..."` | Tier-0 (source) bitrates by resolution |
| `tier0_bitrate_default` | `String` | `"15M"` | Fallback bitrate when resolution not in tier0 map |

#### Metadata

| Field | Type | Default | Purpose |
|---|---|---|---|
| `tmdb_api_key` | `String` | `""` | TMDB API key for movie/TV metadata |
| `metadata_auto_fetch_enabled` | `bool` | `false` | Auto-fetch metadata after upload |
| `metadata_refresh_days` | `u32` | `30` | Days before cached metadata is eligible for refresh |
| `intro_detection_enabled` | `bool` | `true` | Auto-detect intro/outro markers from chapters |
| `intro_chromaprint_enabled` | `bool` | `true` | Use Chromaprint audio fingerprints for intro detection |
| `tac_comments_enabled` | `bool` | `true` | Show Anime Community comments on anime watch pages |

#### Job lifecycle

| Field | Type | Default | Purpose |
|---|---|---|---|
| `job_timeout_seconds` | `u32` | `7200` | Max time a job may run before force-cancel |
| `queue_timeout_seconds` | `u32` | `7200` | Max time a job may sit queued before discard |
| `pending_upload_ttl_seconds` | `u32` | `86400` | TTL for incomplete upload sessions |
| `pending_upload_cleanup_interval_seconds` | `u32` | `300` | Stale upload sweep interval |
| `job_retention_days` | `u32` | `0` | Days to keep completed job records (0 = forever) |
| `max_concurrent_jobs` | `u32` | `1` | Simultaneous job limit |

---

## `Config::default()`

**Signature:** `impl Default for Config { fn default() -> Self }`

**Purpose:** Returns a `Config` with every field set to its hardcoded factory default. These values match the defaults declared in `settings_registry::SETTINGS` (they must be kept in sync manually).

**Reasoning:** This is the innermost layer of the onion-skin config model. The defaults exist so that every field always has a valid value — the system can start with zero DB rows and zero env vars and still be functional. Each default is chosen for a production-local deployment (e.g. binding `0.0.0.0:5050`, 4-second HLS segments, VAAPI encoding enabled).

**Returns:** `Config` — all fields populated, `bots` empty.

**Side effects:** None.

**Called by:** `Config::from_values()` — the defaults are the base layer that environment and DB overrides then patch.

---

## `Config::load()`

**Signature:** `pub fn load(conn: &Connection) -> Result<Self>`

**Purpose:** The primary entry point for building the runtime configuration. Shortcut for: compute effective values, then build.

**Reasoning:** Most callers just need "give me the config". This function hides the two-step (values → build) pipeline.

**Parameters:**
- `conn` — read-only SQLite connection for DB-backed settings and bot pool

**Returns:** `Ok(Config)` on success, or `Err` if DB queries fail.

**Side effects:** Reads the entire `settings` and `bots` DB tables, reads all matching env vars.

**Called by:** Application startup (`main` or `setup`).

---

## `Config::from_values()`

**Signature:** `pub fn from_values(conn: &Connection, values: &BTreeMap<&'static str, String>) -> Result<Self>`

**Purpose:** Build a `Config` from a pre-resolved key-value map. Starts from `Default`, then applies every entry in `values`, then assembles the bot pool, then enforces invariants.

**Reasoning:** Separating "resolve the values" from "build the struct" allows the hot-reload API path to reuse the same builder with a partial map — it calls `apply_normalized_settings` directly instead. Taking a `BTreeMap` guarantees deterministic iteration order, which matters for logging reproducibility.

**Parameters:**
- `conn` — for `build_bot_pool` (reads the `bots` DB table)
- `values` — a fully-resolved, normalized, precedence-merged map of settings (the output of `effective_setting_values`)

**Returns:** `Ok(Config)` with all fields populated.

**Side effects:** Reads the `bots` DB table.

**Called by:** `Config::load()`.

---

## `Config::setting_values()`

**Signature:** `pub fn setting_values(&self) -> BTreeMap<&'static str, String>`

**Purpose:** Serialize the entire `Config` back into a key-value map keyed by the same setting keys used in the registry.

**Reasoning:** This is the inverse of loading — it's used by the API's settings endpoint to return the current effective config. Iterating `SETTINGS` guarantees every registered key appears (even if empty), which gives a stable schema to API consumers.

**Returns:** `BTreeMap<&'static str, String>` — one entry per registered setting, sorted by key.

**Called by:** API handler that exposes current configuration to the dashboard.

---

## `Config::setting_value()`

**Signature:** `fn setting_value(&self, key: &str) -> String`

**Purpose:** Map a single setting key to its current value as a serialized string.

**Reasoning:** A large match arm that mirrors `apply_setting` in reverse. Private because it is only called internally by `setting_values()`. Each case simply formats the corresponding field; the wildcard `_ => String::new()` is a safety net for keys added to the registry but not yet wired — they silently return empty rather than panicking.

**Parameters:**
- `key` — the setting key (e.g. `"HOST"`, `"PORT"`, `"ABR_TIERS"`)

**Returns:** The field's current value as a `String` (or `String::new()` if the key is not mapped).

**Side effects:** None.

---

## `apply_normalized_settings()`

**Signature:** `pub fn apply_normalized_settings(cfg: &mut Config, settings: &HashMap<String, String>)`

**Purpose:** Apply a partial set of normalized key-value pairs to an existing `Config`, then re-enforce invariants. The runtime hot-reload entry point.

**Reasoning:** Unlike the load path (which resets to defaults first), this function patches in-place. It receives settings that have already been normalized by the caller (the API handler), so it skips normalization. The `HashMap` (non-deterministic order) is intentional — this is the hot path and order doesn't matter for a set of discrete assignments.

**Parameters:**
- `cfg` — mutable reference to the active config
- `settings` — already-normalized key-value pairs from the API payload

**Returns:** Nothing.

**Side effects:** Mutates `cfg` in place. Logs warnings for invalid values via `apply_setting`.

**Called by:** The settings update API endpoint.

---

## `env_or_default_value()`

**Signature:** `pub fn env_or_default_value(key: &str) -> Option<String>`

**Purpose:** Look up a single setting: start with the default, try the env var, fall back to the default if the env var is unset or invalid.

**Reasoning:** This is a convenience for one-off env reads outside the full config pipeline. Used by `main.rs` to pull `HOST` and `PORT` before the config system is fully loaded (e.g. to bind the health-check listener early). The function always returns `Some` — it never fails, only warns and falls back.

**Parameters:**
- `key` — a setting key from the registry (e.g. `"HOST"`)

**Returns:** `Some(String)` with the resolved value. Never returns `None` in practice (only `None` if the key is not in the registry, which is a caller bug).

**Side effects:** Reads one env var. May emit a `tracing::warn!` on parse failure.

---

## `effective_setting_values()`

**Signature:** `pub fn effective_setting_values(conn: &Connection) -> Result<BTreeMap<&'static str, String>>`

**Purpose:** Resolve the full effective settings map by merging three layers in ascending precedence order:

1. **Registry defaults** — the base map from `settings_registry::default_settings()`
2. **Environment variables** — for each registered spec, if the env var is set and valid, it overrides the default
3. **Database** — for each row in the `settings` table, if the key exists in the registry and is valid, it overrides both default and env

**Reasoning:** This is the core of the config layering model. The precedence is deliberately: DB > env > default, because DB values are set through the API at runtime and must take priority over env vars that were set at process start. Invalid DB entries are logged and skipped (keeping the lower-precedence value).

**Parameters:**
- `conn` — SQLite connection to read the `settings` table

**Returns:** `Ok(BTreeMap<&'static str, String>)` — one entry per registered key. Never returns `Err` in practice (invalid values are silently dropped with a warning).

**Side effects:** Reads all env vars (filtered by registered keys). Reads the full `settings` DB table.

**Called by:** `Config::load()`.

---

## `apply_setting()`

**Signature:** `fn apply_setting(cfg: &mut Config, key: &str, value: &str, source: &str)`

**Purpose:** Map a single normalized string key+value to the corresponding `Config` field. This is the dispatch point where all setting keys are defined.

**Reasoning:** A large match arm that handles ~40 keys. Each arm parses the value according to the expected type (via `parse_int`, `parse_bool`, or direct string assignment) and sets the field. Errors are caught per-arm and logged as warnings — the system degrades gracefully by keeping the previous value. The `source` parameter (`"effective"`, `"runtime"`, etc.) is used only in the success log line.

**Parameters:**
- `cfg` — mutable config to update
- `key` — the setting key (e.g. `"HOST"`)
- `value` — the normalized string value
- `source` — human-readable origin tag for logging

**Returns:** Nothing.

**Side effects:** Mutates `cfg.xyz` on match. Logs `tracing::debug!` on success, `tracing::warn!` on parse failure.

**Called by:** `Config::from_values()`, `apply_normalized_settings()`.

---

## `enforce_invariants()`

**Signature:** `fn enforce_invariants(cfg: &mut Config)`

**Purpose:** Post-load validation that checks mutually exclusive combinations and corrects them.

**Reasoning:** Currently enforces a single invariant: `ABR_ENABLED` and `VIRTUAL_ABR_TIERS` cannot both be true (they use conflicting segmenting strategies). If both are set, `virtual_abr_tiers` is silently disabled with a warning. This function is called at the end of every config-building path (initial load and hot-reload).

**Parameters:**
- `cfg` — mutable config, potentially with contradictory settings

**Returns:** Nothing.

**Side effects:** Mutates `cfg.virtual_abr_tiers` if the invariant is violated. Logs a warning.

**Called by:** `Config::from_values()`, `apply_normalized_settings()`.

---

## `parse_int()`

**Signature:** `fn parse_int<T>(s: &str) -> Result<T, String> where T: std::str::FromStr, T::Err: std::fmt::Display`

**Purpose:** Trim and parse a string as any integer type (`u16`, `u32`, `u64`, etc.).

**Reasoning:** Generic over `FromStr` so the same helper works for every numeric config field. Trims whitespace because users copy-paste env values with trailing spaces. The `Err: Display` bound lets us convert any parse error into a `String`.

**Parameters:**
- `s` — raw input string

**Returns:** `Ok(T)` on success, `Err(String)` with the parse error message.

---

## `parse_bool()`

**Signature:** `fn parse_bool(s: &str) -> Result<bool, String>`

**Purpose:** Parse a string as a boolean, case-insensitive for `"true"` / `"false"` and also accepts `"1"` / `"0"`.

**Reasoning:** Accepts both word-form and numeric boolean representations commonly used in env files and API payloads. The registry normalizes bools to `"true"` / `"false"` before they reach this function in most paths, but the numeric forms are accepted for direct env-var parsing flexibility.

**Parameters:**
- `s` — raw input string

**Returns:** `Ok(true)` or `Ok(false)`, or `Err(format!("not a boolean (expected true/false/1/0): {s}"))`.

---

## `is_valid_bot_token()`

**Signature:** `pub fn is_valid_bot_token(s: &str) -> bool`

**Purpose:** Validate a Telegram Bot API token format: `<numeric_id>:<alphanumeric_secret>`.

**Reasoning:** Telegram tokens follow a strict pattern: an 8-12 digit bot ID, a colon, and a 35-45 character secret composed of alphanumerics, underscores, and hyphens. This validation catches typo'd env values early rather than letting the Telegram API return 401 at upload time. It also rejects `"your_bot_token_here"` placeholder values that users forget to replace.

**Parameters:**
- `s` — the raw token string

**Returns:** `true` if the token matches the Telegram Bot API format, `false` otherwise.

**Edge cases:**
- Returns `false` for strings without a colon
- Returns `false` if the numeric ID is too short (<8) or too long (>12) or contains non-digits
- Returns `false` if the secret is too short (<35) or too long (>45) or contains invalid characters
- Returns `false` for anything with more than one colon
- Returns `false` for placeholder values (checked at call site, not here)

**Called by:** `build_bot_pool()`.

---

## `build_bot_pool()`

**Signature:** `fn build_bot_pool(conn: &Connection) -> Result<Vec<BotConfig>>`

**Purpose:** Assemble the Telegram bot pool from two sources: environment variables and the `bots` DB table. Environment bots come first (sorted by suffix number), then DB bots. Duplicate tokens (across both sources) are silently deduplicated — first occurrence wins.

**Reasoning:** The discovery order matters: env bots are scanned by enumerating `TELEGRAM_BOT_TOKEN_1`, `TELEGRAM_BOT_TOKEN_2`, etc., sorted numerically. This gives a predictable order that operators can rely on (bot index in the config matches the env suffix). DB bots are appended after env bots so env bots occupy indices 0..N and DB bots start at N.

Both sources are validated: tokens must pass `is_valid_bot_token` and channel IDs must be negative (Telegram channel/supergroup IDs are negative integers). Rows with `your_` prefix tokens are skipped (placeholder detection). Duplicate tokens (across env+DB or within a single source) are dropped with a warning.

**Parameters:**
- `conn` — SQLite connection to read the `bots` table

**Returns:** `Ok(Vec<BotConfig>)` — possibly empty. Never returns `Err` from validation (invalid entries are silently skipped with warnings; only DB query failures propagate as `Err`).

**Side effects:** Reads all `TELEGRAM_BOT_TOKEN_N` + `TELEGRAM_CHANNEL_ID_N` env vars. Reads all rows from the `bots` DB table. Emits `tracing::warn!` for every skipped entry.

**Called by:** `Config::from_values()`.
