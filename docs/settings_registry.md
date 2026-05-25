# Settings Registry — `settings_registry.rs`

## Module Overview

This module is the **single source of truth** for every configurable setting in the application. It defines:

- What settings exist
- Their data types (`int`, `bool`, `str`, `list`, `tiers`)
- Their environment variable names
- Their category groupings (for UI presentation)
- Their default values
- Human-readable descriptions
- Validation rules for each setting

The central data structure is the compile-time `SETTINGS` array — a `&[SettingSpec]` slice that the rest of the system references. This design keeps all setting definitions in one place instead of scattering key names, env variables, and validation logic across the codebase.

**Why compile-time constants instead of a config file?** Because settings are developer-defined, not user-extensible. A recompile is required to add/remove settings, which is the correct trade-off: the code knows what it needs to configure.

**Why string-based internal storage?** All values are stored, parsed, and returned as strings. This makes serialization trivially simple — env vars, JSON, `.env` files, and query parameters all speak strings. Typed conversion happens at the boundary (in `normalize_*` functions), not in storage.

---

## Types

### `SettingType` enum

```rust
pub enum SettingType {
    Int,
    Bool,
    Str,
    List,
    Tiers,
}
```

| Variant | Meaning | Example |
|---------|---------|---------|
| `Int` | Integer number | `PORT = 5050` |
| `Bool` | Boolean flag | `ENABLE_HW_ACCEL = true` |
| `Str` | Free-form string | `PREFERRED_ENCODER = "vaapi"` |
| `List` | Comma-separated list | `TRUSTED_PROXY_CIDRS = "127.0.0.1/32,::1/128"` |
| `Tiers` | ABR tier list (`height:bitrate` pairs) | `ABR_TIERS = "1080:10M,720:5M"` |

**Why not just use JSON Schema / typed config?** Because the settings surface is small (~59 items) and simple. A full schema system would be disproportionate complexity. The `SettingType` enum is enough to route normalization and JSON conversion correctly.

---

### `SettingSpec` struct

```rust
pub struct SettingSpec {
    pub key: &'static str,       // Programmatic identifier (e.g. "PORT")
    pub env: &'static str,       // Environment variable name (e.g. "LOCAL_PORT")
    pub category: &'static str,  // Grouping key for UI (e.g. "server")
    pub setting_type: SettingType,
    pub default: &'static str,   // Default value as string
    pub description: &'static str,
    pub min: Option<i64>,        // Optional minimum (Int-only)
    pub max: Option<i64>,        // Optional maximum (Int-only)
}
```

Each setting is one row of metadata. The `key` is the internal identifier, `env` is the external (env/file) name — they differ because environment variable naming conventions (uppercase, underscored) don't always match internal naming preferences.

---

## `setting!` Macro

```rust
macro_rules! setting {
    ($key, $env, $category, $kind, $default, $description, $min, $max) => { ... }
}
```

A pure boilerplate reducer. Without it, each of the 59 entries in `SETTINGS` would need `SettingSpec { key: ..., env: ..., ... }`. With it, each entry is a single-line call.

---

## SETTINGS Reference Table

Below are all 59 registered settings, their env var names, types, defaults, and descriptions.

### Server

| Key | Env Var | Type | Default | Description |
|-----|---------|------|---------|-------------|
| `ADMIN_USER` | `ADMIN_USER` | str | `` | Admin username for HTTP Basic Auth. Leave empty to disable. |
| `ADMIN_PASS` | `ADMIN_PASS` | str | `` | Admin password for HTTP Basic Auth. |
| `HOST` | `LOCAL_HOST` | str | `0.0.0.0` | Bind address. |
| `PORT` | `LOCAL_PORT` | int | `5050` | Bind port. |
| `FORCE_HTTPS` | `FORCE_HTTPS` | bool | `false` | Redirect HTTP to HTTPS. |
| `BEHIND_PROXY` | `BEHIND_PROXY` | bool | `false` | Trust forwarded headers from configured proxies. |
| `TRUSTED_PROXY_CIDRS` | `TRUSTED_PROXY_CIDRS` | list | `127.0.0.1/32,::1/128` | Trusted proxy CIDR ranges. |
| `CORS_ALLOWED_ORIGINS` | `CORS_ALLOWED_ORIGINS` | list | `` | Allowed CORS origins. |

### Cloudflared

| Key | Env Var | Type | Default | Description |
|-----|---------|------|---------|-------------|
| `CLOUDFLARED_ENABLED` | `CLOUDFLARED_ENABLED` | bool | `false` | Enable Cloudflared tunnel management. |
| `CLOUDFLARED_CONFIG` | `CLOUDFLARED_CONFIG` | str | `` | Path to the Cloudflared tunnel config file. |

### File Handling

| Key | Env Var | Type | Default | Description |
|-----|---------|------|---------|-------------|
| `TELEGRAM_MAX_FILE_SIZE` | `TELEGRAM_MAX_FILE_SIZE` | int | `20971520` | Telegram per-file upload ceiling. Raise if Telegram increases Bot API limits. |
| `MAX_UPLOAD_SIZE` | `MAX_UPLOAD_SIZE` | int | `107374182400` | Maximum accepted client upload size. |
| `UPLOAD_CHUNK_SIZE` | `UPLOAD_CHUNK_SIZE` | int | `10485760` | Server-advertised upload chunk size. |
| `SEGMENT_TARGET_SIZE` | `SEGMENT_TARGET_SIZE` | int | `15728640` | Preferred HLS segment size. User-configurable; adjust if upload ceiling changes. |
| `CACHE_DIR` | `CACHE_DIR` | str | `./cache/` | Ephemeral cache directory wiped on startup. |
| `DISK_CACHE_ENABLED` | `DISK_CACHE_ENABLED` | bool | `false` | Store cached segment payloads on disk instead of memory-only cache. |
| `CACHE_WARMUP_ENABLED` | `CACHE_WARMUP_ENABLED` | bool | `false` | Enable cache warm-up behavior. |
| `SEGMENT_CACHE_SIZE_MB` | `SEGMENT_CACHE_SIZE_MB` | int | `200` | Segment cache budget in MB. |
| `SEGMENT_PREFETCH_COUNT` | `SEGMENT_PREFETCH_COUNT` | int | `3` | Segments to prefetch ahead. |
| `SEGMENT_PREFETCH_MIN_FREE_BYTES` | `SEGMENT_PREFETCH_MIN_FREE_BYTES` | int | `0` | Minimum free cache bytes before prefetch. |
| `AUDIO_SEGMENT_DURATION` | `AUDIO_SEGMENT_DURATION` | int | `30` | Audio segment duration in seconds. |

### Hardware Acceleration

| Key | Env Var | Type | Default | Description |
|-----|---------|------|---------|-------------|
| `ENABLE_HW_ACCEL` | `ENABLE_HARDWARE_ACCELERATION` | bool | `true` | Enable hardware encoders. |
| `PREFERRED_ENCODER` | `PREFERRED_ENCODER` | str | `vaapi` | Preferred hardware encoder. |
| `VAAPI_DEVICE` | `VAAPI_DEVICE` | str | `` | VAAPI render device path. |
| `MAX_PARALLEL_ENCODES` | `MAX_PARALLEL_ENCODES` | int | `2` | Max parallel video encodes per job. |
| `VIDEO_BITRATE` | `VIDEO_BITRATE` | str | `4M` | Default video bitrate. |
| `AUDIO_BITRATE` | `AUDIO_BITRATE` | str | `128k` | Default audio bitrate. |

### HLS

| Key | Env Var | Type | Default | Description |
|-----|---------|------|---------|-------------|
| `HLS_SEGMENT_DURATION` | `HLS_SEGMENT_DURATION` | int | `4` | Target HLS segment duration. |

### Adaptive Bitrate

| Key | Env Var | Type | Default | Description |
|-----|---------|------|---------|-------------|
| `ABR_ENABLED` | `ABR_ENABLED` | bool | `true` | Produce eager ABR tiers. |
| `ENABLE_COPY_MODE` | `ENABLE_COPY_MODE` | bool | `true` | Enable tier-0 passthrough. |
| `VIRTUAL_ABR_TIERS` | `VIRTUAL_ABR_TIERS` | bool | `false` | Transcode lower tiers on demand. |
| `ABR_TIERS` | `ABR_TIERS` | tiers | `1080:10M,720:5M,480:2M,360:1200k` | Configured ABR tiers. |
| `TIER0_BITRATES` | `TIER0_BITRATES` | tiers | `2160:60M,1080:30M,720:15M,480:5M` | Source-height tier-0 bitrates. |
| `TIER0_BITRATE_DEFAULT` | `TIER0_BITRATE_DEFAULT` | str | `15M` | Fallback tier-0 bitrate. |

### Metadata

| Key | Env Var | Type | Default | Description |
|-----|---------|------|---------|-------------|
| `TMDB_API_KEY` | `TMDB_API_KEY` | str | `` | TMDB API key for movie/TV metadata fetching. |
| `METADATA_AUTO_FETCH_ENABLED` | `METADATA_AUTO_FETCH_ENABLED` | bool | `false` | Automatically fetch metadata from providers after upload. |
| `METADATA_REFRESH_DAYS` | `METADATA_REFRESH_DAYS` | int | `30` | Days before cached metadata is eligible for refresh. |
| `INTRO_DETECTION_ENABLED` | `INTRO_DETECTION_ENABLED` | bool | `true` | Enable automatic intro/outro marker detection. |
| `INTRO_CHROMAPRINT_ENABLED` | `INTRO_CHROMAPRINT_ENABLED` | bool | `true` | Enable Chromaprint-based audio fingerprint detection for markers. |
| `TAC_COMMENTS_ENABLED` | `TAC_COMMENTS_ENABLED` | bool | `true` | Enable Anime Community comment sections on watch pages. |

### Reliability

| Key | Env Var | Type | Default | Description |
|-----|---------|------|---------|-------------|
| `JOB_TIMEOUT_SECONDS` | `JOB_TIMEOUT_SECONDS` | int | `7200` | Per-job runtime cap. |
| `QUEUE_TIMEOUT_SECONDS` | `QUEUE_TIMEOUT_SECONDS` | int | `7200` | Maximum time a job can wait in the queue before timing out. |
| `PENDING_UPLOAD_TTL_SECONDS` | `PENDING_UPLOAD_TTL_SECONDS` | int | `86400` | Idle pending upload expiry. |
| `PENDING_UPLOAD_CLEANUP_INTERVAL_SECONDS` | `PENDING_UPLOAD_CLEANUP_INTERVAL_SECONDS` | int | `300` | Pending upload sweeper interval. |
| `JOB_RETENTION_DAYS` | `JOB_RETENTION_DAYS` | int | `0` | Completed-job retention period. |
| `MAX_CONCURRENT_JOBS` | `MAX_CONCURRENT_JOBS` | int | `1` | Queue worker count. |

### Rate Limiting

| Key | Env Var | Type | Default | Description |
|-----|---------|------|---------|-------------|
| `UPLOAD_RATE_LIMIT_WINDOW` | `UPLOAD_RATE_LIMIT_WINDOW` | int | `60` | Upload rate-limit window. |
| `UPLOAD_RATE_LIMIT_MAX_REQUESTS` | `UPLOAD_RATE_LIMIT_MAX_REQUESTS` | int | `100` | Upload requests per window. |
| `MAX_PENDING_UPLOADS_PER_IP` | `MAX_PENDING_UPLOADS_PER_IP` | int | `5` | Pending uploads per IP. |

### Watch Folder

| Key | Env Var | Type | Default | Description |
|-----|---------|------|---------|-------------|
| `WATCH_POLL_SECONDS` | `WATCH_POLL_SECONDS` | int | `5` | Watch-folder scan interval. |
| `WATCH_STABLE_SECONDS` | `WATCH_STABLE_SECONDS` | int | `30` | File stability threshold. |
| `WATCH_VIDEO_EXTENSIONS` | `WATCH_VIDEO_EXTENSIONS` | list | `mp4,mkv,avi,mov,webm,ts,m4v,flv` | Allowed watch-folder video extensions. |
| `WATCH_IGNORE_SUFFIXES` | `WATCH_IGNORE_SUFFIXES` | list | `.part,.crdownload,.tmp,.partial` | Ignored partial-download suffixes. |

### Telegram

| Key | Env Var | Type | Default | Description |
|-----|---------|------|---------|-------------|
| `UPLOAD_PARALLELISM` | `UPLOAD_PARALLELISM` | int | `12` | Cross-bot upload semaphore size. |
| `DB_SYNC_ENABLED` | `DB_SYNC_ENABLED` | bool | `true` | Automatically snapshot and upload streamer.db after completed jobs. |
| `DB_SYNC_BOOTSTRAP` | `DB_SYNC_BOOTSTRAP` | str | `` | Latest DB sync bootstrap descriptor. |
| `DB_AUTO_MERGE_INTERVAL_MINUTES` | `DB_AUTO_MERGE_INTERVAL_MINUTES` | int | `0` | DB auto-merge cadence. |
| `DB_AUTO_MERGE_FILE_ID` | `DB_AUTO_MERGE_FILE_ID` | str | `` | Telegram file_id for DB auto-merge. |
| `DB_AUTO_MERGE_BOT_INDEX` | `DB_AUTO_MERGE_BOT_INDEX` | int | `0` | Bot index used for DB auto-merge. |
| `WEBHOOK_URL` | `WEBHOOK_URL` | str | `` | POST terminal job events to this URL. |

---

## Public Functions

---

### `is_public_setting_key(key: &str) -> bool`

**Purpose:** Answers "is this string a valid setting key?" — the public gate for key existence checks.

**Intent:** Provides a boolean API for code that only needs a yes/no answer (e.g., query parameter filtering, API input validation). The alternative is `setting_spec(key).is_some()`, but a dedicated function makes the intent explicit.

**How it works:** Delegates to `setting_spec(key)` and returns `true` if a spec was found.

**Params:**
- `key` — the setting key string

**Returns:** `true` if the key exists in SETTINGS, `false` otherwise.

---

### `setting_spec(key: &str) -> Option<&'static SettingSpec>`

**Purpose:** Looks up a `SettingSpec` by its key.

**Intent:** This is the core lookup primitive. Linear search over a 59-element array is intentional — it's simple, the array is small enough that binary search buys nothing, and it keeps the code trivial to verify.

**How it works:** Calls `SETTINGS.iter().find(|s| s.key == key)` — walks the slice front-to-back until a key matches, returns `None` if no match.

**Params:**
- `key` — the setting key string

**Returns:** `Some(&SettingSpec)` if found, `None` otherwise.

---

### `default_settings() -> BTreeMap<&'static str, String>`

**Purpose:** Returns a map of every setting key to its normalized default value.

**Intent:** Provides a baseline configuration that callers (typically the startup config loader) can then override with env vars, file values, etc. The values go through `normalize_str` because a "raw" default string might not be in its canonical form.

**How it works:**
1. Iterates all `SETTINGS`
2. For each spec, calls `normalize_str(spec, spec.default)`
3. Collects into a `BTreeMap<&str, String>`

Normalization can theoretically fail on a default value (e.g., an invalid bitrate in the source), which is why `.expect("valid default setting")` panics — a bad default is a programming error, not a runtime condition.

**Returns:** `BTreeMap` with keys like `"PORT"` → `"5050"`, `"ABR_ENABLED"` → `"true"`, etc.

---

### `normalize_json(key: &str, value: &Value) -> Result<String, String>`

**Purpose:** Accepts a JSON `Value` from an API request and normalizes it into the internal string representation.

**Intent:** The settings API accepts JSON, but the internal storage is string-based. This function bridges the two worlds — it converts JSON types to their canonical string form and then validates/normalizes through `normalize_str`.

**How it works:**
1. Looks up the setting spec by key (fails with "unknown setting key" if not found)
2. Converts the JSON value to a raw string:
   - `Value::String` → the string as-is
   - `Value::Bool` → `"true"` or `"false"`
   - `Value::Number` → its decimal string representation
   - `Value::Array` → each element must be a string (rejects non-string arrays with "list values must be strings"), then joined by `,`
   - Any other JSON type → error "unsupported JSON value"
3. Passes the raw string to `normalize_str` for type-specific validation

**Errors:**
- Unknown setting key
- Array elements that aren't strings
- Unsupported JSON types
- Any validation failure from `normalize_str`

---

### `normalize_str_for_key(key: &str, value: &str) -> Result<String, String>`

**Purpose:** Takes a raw string value and a key, validates + normalizes the value according to the setting's type rules.

**Intent:** This is the main public entry point for string normalization. Used by env var parsing, `.env` file loading, and any code path that already has a string value.

**How it works:**
1. Looks up the spec by key
2. Delegates to `normalize_str(spec, value)`

**Errors:**
- Unknown setting key
- Type-specific validation failures

---

### `categories_for_values(values: &BTreeMap<&'static str, String>) -> Value`

**Purpose:** Produces the complete settings UI payload as a JSON `Value`.

**Intent:** This function exists for the settings management API endpoint. It groups all settings by their category, includes their current values (from the passed map), and formats everything as structured JSON. The UI renders this directly.

**How it works:**
1. Iterates `SETTINGS`
2. Groups settings into their categories, creating entries like:
   ```json
   { "server": { "label": "Server", "settings": [...] } }
   ```
3. For each setting, reads the current value from the provided `values` map (falls back to `spec.default` if missing)
4. Serializes each value through `value_to_json` to produce typed JSON (booleans stay booleans, ints stay numbers)
5. Attaches `key`, `env`, `type` (from `setting_type_name`), `value`, `default`, and `description` to each entry

**Params:**
- `values` — a map of current setting values (key → string)

**Returns:** A JSON object keyed by category, each containing `label` and `settings` array.

---

### `parse_list(value: &str) -> Vec<String>`

**Purpose:** Splits a comma-separated string into a vector of trimmed, non-empty strings.

**Intent:** A simple utility extracted so it can be reused by `normalize_list` and external callers. The comma-separated format is used for `SettingType::List` settings; splitting logic needs to be consistent everywhere.

**How it works:**
1. `value.split(',')` — splits on every comma
2. `.map(str::trim)` — strips whitespace from each item
3. `.filter(|s| !s.is_empty())` — removes empty items (consecutive commas, leading/trailing commas)
4. `.map(ToOwned::to_owned)` — converts `&str` to `String`

**Returns:** `Vec<String>` of list items (may be empty if input is empty or all-commas).

---

## Internal Functions (Private)

---

### `normalize_str(spec: &SettingSpec, value: &str) -> Result<String, String>`

**Purpose:** The central normalization dispatch — runs every setting value through type-specific validation.

**Intent:** This is the heart of the validation pipeline. Every path that feeds a setting value into the system eventually calls this function. It first strips inline comments (for `.env` file friendliness), then dispatches to the type-specific normalizer.

**How it works:**
1. `strip_inline_comment(value)` — removes everything after `#`
2. Matches `spec.setting_type`:
   - `Int` → `normalize_int(spec, value)`
   - `Bool` → `normalize_bool(value)`
   - `Str` → `normalize_string(spec.key, value)`
   - `List` → `normalize_list(spec.key, value)`
   - `Tiers` → `normalize_tiers(value)`

**Errors:** Depends on the type-specific normalizer — invalid int, bad bool, etc.

---

### `strip_inline_comment(value: &str) -> String`

**Purpose:** Removes everything after the first `#` character and trims whitespace.

**Intent:** `.env` files and environment variable overrides often contain inline comments like `TELEGRAM_MAX_FILE_SIZE=20971520 # 20MB`. This function strips the comment part so that only the actual value reaches the normalizer.

**Design choice:** Only one `#` is special — the first one. This means values cannot contain `#`. That's fine because no valid setting value uses `#`.

**How it works:**
1. `split_once('#')` — splits on first `#`
2. Takes the head (before `#`), trims it
3. If no `#` exists, uses the entire value, trimmed

---

### `normalize_int(spec: &SettingSpec, value: &str) -> Result<String, String>`

**Purpose:** Parses an integer value and validates min/max bounds.

**Intent:** Numeric validation happens here — the spec's `min` and `max` fields are only meaningful for `Int` settings. Parsing as `i64` covers the full range of possible values (port numbers, byte sizes, timeouts).

**How it works:**
1. Trims input, parses as `i64`
2. If `spec.min` exists and parsed < min → error "must be >= {min}"
3. If `spec.max` exists and parsed > max → error "must be <= {max}"
4. Returns the parsed integer as a string (`parsed.to_string()`, which is the canonical form — no leading zeros, no `+` sign)

**Errors:**
- Not a valid integer
- Below minimum
- Above maximum

---

### `normalize_bool(value: &str) -> Result<String, String>`

**Purpose:** Case-insensitive boolean parsing. Accepts `true`/`false` and `1`/`0`.

**Intent:** Bool values must be explicit — "true", "false", "1", or "0" (any casing for the words). This strictness prevents typos like "ture" or "flse" from silently defaulting while accepting common boolean representations used in env files and API payloads.

**Design choice:** Case-insensitive matching (`eq_ignore_ascii_case`) means `True`, `TRUE`, `true` all work. Numeric `1`/`0` are also accepted as common boolean shorthand.

**How it works:**
1. Trims input, lowercases
2. If matches `"true"` or `"1"` → `Ok("true")`
3. If matches `"false"` or `"0"` → `Ok("false")`
4. Otherwise → error "not a boolean (expected true/false/1/0): {value}"

**Note:** Returns the canonical lower-case form `"true"`/`"false"`, never `"1"`/`"0"`.

---

### `normalize_string(key: &str, value: &str) -> Result<String, String>`

**Purpose:** Per-key string validation — each string-typed setting that needs specific validation gets its own branch.

**Intent:** General `Str` settings (like `WEBHOOK_URL`) just pass through with trimming. But some settings need format validation before they're accepted. Rather than a generic regex-based approach, each key gets explicit validation.

**How it works:**
1. Trims the input
2. Per-key match:
   - **`HOST`**: Parses as `IpAddr` — ensures the bind address is a valid IP (not a hostname, not garbage)
   - **`PREFERRED_ENCODER`**: Must be one of `vaapi`, `nvenc`, `qsv`, `cpu` — only these four encoders are supported
   - **`VAAPI_DEVICE`**: If non-empty, must match `/dev/dri/renderD<N>` regex — prevents pointing FFmpeg at a non-existent device
   - **`VIDEO_BITRATE`**, **`AUDIO_BITRATE`**, **`TIER0_BITRATE_DEFAULT`**: Must be a valid bitrate format (number + unit suffix)
   - **`DB_AUTO_MERGE_FILE_ID`**: If non-empty, must be a valid Telegram file_id (alphanumeric, 50-255 chars) — prevents storing invalid IDs from misconfiguration

**Why not validate all Str settings?** Many string settings (like `WEBHOOK_URL`) don't need format validation at this level, or their consumers validate them. The design is: validate at the boundary only what's cheap and catches real misconfigurations.

---

### `normalize_list(key: &str, value: &str) -> Result<String, String>`

**Purpose:** Validates a comma-separated list setting, applying per-key format rules.

**Intent:** Lists are comma-separated strings, but different list settings have different format requirements: CIDRs need network validation, origins need URL validation, file extensions need dot-prefix normalization.

**How it works:**
1. Calls `parse_list(value)` to get individual items
2. Per-key validation:
   - **`TRUSTED_PROXY_CIDRS`**: Each item must be a valid CIDR notation (validates via `validate_cidr`)
   - **`CORS_ALLOWED_ORIGINS`**: Each item must be a valid origin (validates via `validate_origin`)
   - **`WATCH_VIDEO_EXTENSIONS`**: Auto-prepends `.` if missing (user writes "mp4", it becomes ".mp4"), then validates via `validate_simple_list_item`
   - **`WATCH_IGNORE_SUFFIXES`**: Each suffix validated via `validate_simple_list_item`
3. Joins items back with `,` and returns

**Errors:** Per-item validation failures, propagated from the validators.

---

### `normalize_tiers(value: &str) -> Result<String, String>`

**Purpose:** Validates ABR tier definitions in `height:bitrate,height:bitrate` format.

**Intent:** ABR tiers (like `1080:10M,720:5M`) have a specific format that's rich enough to warrant its own normalizer. Each pair must have a positive height and a valid bitrate string.

**How it works:**
1. Calls `parse_list(value)` to split into comma-separated pairs
2. For each pair:
   - Splits on `:` → must have exactly two parts
   - Left part: parses as `u32`, must be > 0 (height 0 makes no sense)
   - Right part: validates as bitrate via `is_bitrate`
3. Re-joins as `height:bitrate,height:bitrate` (canonical form)
4. Requires at least one tier — an empty tier list is a configuration error

**Errors:**
- Missing `:` separator in a pair
- Height is 0 or not a valid integer
- Bitrate is invalid
- Empty tier list

---

### `value_to_json(spec: &SettingSpec, value: &str) -> Value`

**Purpose:** Converts a string-encoded setting value back to its typed JSON representation.

**Intent:** The settings UI expects typed JSON — booleans should be `true`/`false`, ints should be numbers, lists should be arrays. This function reverses the string-based storage back to JSON types for API responses.

**How it works:**
1. Matches `spec.setting_type`:
   - `Int` → parses as `i64` → `Value::Number` (falls back to `Value::String` if parse fails — but parse shouldn't fail if the value was normalized)
   - `Bool` → `json!(value.eq_ignore_ascii_case("true"))`
   - `List` → calls `parse_list(value)` → `Value::Array` of strings
   - `Str` / `Tiers` → `json!(value)` — passed as plain string

---

### `setting_type_name(setting_type: SettingType) -> &'static str`

**Purpose:** Maps `SettingType` enum to lowercase string labels for API responses.

**How it works:** Simple match:
- `Int` → `"int"`
- `Bool` → `"bool"`
- `Str` → `"str"`
- `List` → `"list"`
- `Tiers` → `"tiers"`

---

### `category_label(category: &str) -> &'static str`

**Purpose:** Converts internal category keys (like `"rate_limiting"`, `"watch_folder"`) into human-readable labels (like `"Rate Limiting"`, `"Watch Folder"`).

**Intent:** The UI needs display-friendly category names, not internal snake_case keys. This mapping is explicit rather than algorithmic (e.g., auto-capitalizing + replacing underscores) because some category names might need special handling.

**How it works:** Static match:
| Key | Label |
|-----|-------|
| `server` | Server |
| `cloudflared` | Cloudflared |
| `file_handling` | File Handling |
| `hardware` | Hardware Acceleration |
| `hls` | HLS |
| `metadata` | Metadata |
| `adaptive_bitrate` | Adaptive Bitrate |
| `reliability` | Reliability |
| `rate_limiting` | Rate Limiting |
| `watch_folder` | Watch Folder |
| `telegram` | Telegram |
| `_` (fallback) | Settings |

---

### `validate_cidr(value: &str) -> Result<(), String>`

**Purpose:** Validates a CIDR notation string (e.g., `"192.168.1.0/24"` or `"::1/128"`).

**Intent:** Trusted proxy CIDRs are a security boundary — a misconfigured CIDR could either block legitimate proxies or allow spoofed headers from untrusted IPs. This validator ensures the format is syntactically correct before the CIDR enters the ACL system.

**How it works:**
1. Splits on `/` — must have exactly two parts
2. Left part: parses as `IpAddr` (supports both IPv4 and IPv6)
3. Right part: parses as `u8` prefix length
4. Validates prefix ≤ 32 (IPv4) or ≤ 128 (IPv6)

**Errors:**
- Missing `/`
- IP parsing failure
- Prefix parsing failure
- Prefix too large for the address family

---

### `validate_origin(value: &str) -> Result<(), String>`

**Purpose:** Validates a CORS origin string.

**Intent:** CORS misconfiguration is a common source of hard-to-debug browser issues. This validator catches common mistakes early:
- `*` is allowed as a wildcard
- Otherwise must start with `http://` or `https://`
- Must not include path, query string, or fragment

**Why reject paths?** Because browsers send origins without paths. Including a path in an origin setting means the CORS check will never match, and the developer gets a confusing CORS error in the console. Rejecting at config time is better.

**How it works:**
1. If `"*"` → OK (wildcard)
2. Strip `http://` or `https://` prefix — fail if neither present
3. Remaining string must not be empty, and must not contain `/`, `?`, or `#`

**Errors:**
- Missing `http://` or `https://` prefix
- Empty host after stripping prefix
- Contains path (`/`), query (`?`), or fragment (`#`)

---

### `validate_simple_list_item(value: &str) -> Result<(), String>`

**Purpose:** Validates that a list item is a "simple" string — no path separators, no newlines, not empty.

**Intent:** Used for watch folder extensions and suffixes, which must be simple identifiers. This prevents injection-like patterns (no path traversals, no multiline entries).

**How it works:**
1. If empty → error
2. If contains `/`, `\`, or newline → error
3. Otherwise → OK

---

### `is_vaapi_device(value: &str) -> bool`

**Purpose:** Checks if a string matches the VAAPI render device path pattern `/dev/dri/renderD<N>`.

**Intent:** On Linux, VAAPI hardware encoding uses DRI render nodes. This function validates that the path looks correct before FFmpeg tries to use it.

**How it works:**
1. Tries to strip prefix `/dev/dri/renderD`
2. If prefix matches, checks that the remainder is non-empty and all ASCII digits
3. Returns `true` only if both conditions pass

Examples: `/dev/dri/renderD129` → true, `/dev/dri/card0` → false, `/dev/dri/renderD` → false (no digits), empty string → false.

---

### `is_bitrate(value: &str) -> bool`

**Purpose:** Validates a bitrate string like `"128k"`, `"4M"`, `"1.5G"`, `"1200k"`.

**Intent:** Bitrate strings are used throughout the application (video bitrate, audio bitrate, ABR tiers). They follow FFmpeg's convention: a numeric value suffixed with a unit. This validator ensures bitrates are syntactically valid before FFmpeg processing.

**How it works:**
1. Checks the last byte is one of `k`, `K`, `m`, `M`, `g`, `G` (the unit suffix)
2. Checks the prefix (everything before the last byte):
   - Must not be empty
   - May contain digits and at most one `.` (decimal point)
   - Must contain at least one digit (so `"."` or `"M"` alone doesn't pass)

Examples: `128k` → true, `4M` → true, `1.5G` → true, `4000` → false (no suffix), `4.0.0M` → false (two dots), `M` → false (no digits).

---

### `is_telegram_file_id(value: &str) -> bool`

**Purpose:** Validates a Telegram file_id string.

**Intent:** Telegram file_ids have a known format: they're alphanumeric strings (with underscores and hyphens) of length 50-255 characters. This validator catches accidentally copied wrong values early.

**How it works:**
1. Length must be between 50 and 255 (inclusive)
2. Every character must be ASCII alphanumeric, underscore, or hyphen

---

## Validation Rules Quick Reference

This table summarizes all per-key validation rules.

| Key | Type | Validation |
|-----|------|------------|
| `ADMIN_USER` | str | No validation (pass-through) |
| `ADMIN_PASS` | str | No validation (pass-through) |
| `HOST` | str | Must parse as `std::net::IpAddr` |
| `PORT` | int | Min 1, Max 65535 |
| `FORCE_HTTPS` | bool | Must be `true`/`false`/`1`/`0` (case-insensitive) |
| `BEHIND_PROXY` | bool | Must be `true`/`false`/`1`/`0` |
| `TRUSTED_PROXY_CIDRS` | list | Each item must be valid CIDR (`ip/prefix`) |
| `CORS_ALLOWED_ORIGINS` | list | Each item must be `*` or `http(s)://host` without path |
| `CLOUDFLARED_ENABLED` | bool | Must be `true`/`false`/`1`/`0` |
| `CLOUDFLARED_CONFIG` | str | No validation (pass-through) |
| `TELEGRAM_MAX_FILE_SIZE` | int | Min 1 |
| `MAX_UPLOAD_SIZE` | int | Min 1 |
| `UPLOAD_CHUNK_SIZE` | int | Min 1 |
| `SEGMENT_TARGET_SIZE` | int | Min 1 |
| `CACHE_DIR` | str | No validation (pass-through) |
| `DISK_CACHE_ENABLED` | bool | Must be `true`/`false`/`1`/`0` |
| `CACHE_WARMUP_ENABLED` | bool | Must be `true`/`false`/`1`/`0` |
| `SEGMENT_CACHE_SIZE_MB` | int | Min 0 |
| `SEGMENT_PREFETCH_COUNT` | int | Min 0 |
| `SEGMENT_PREFETCH_MIN_FREE_BYTES` | int | Min 0 |
| `AUDIO_SEGMENT_DURATION` | int | Min 1 |
| `ENABLE_HW_ACCEL` | bool | Must be `true`/`false`/`1`/`0` |
| `PREFERRED_ENCODER` | str | Must be one of: `vaapi`, `nvenc`, `qsv`, `cpu` |
| `VAAPI_DEVICE` | str | Must be empty or match `/dev/dri/renderD\d+` |
| `MAX_PARALLEL_ENCODES` | int | Min 1 |
| `VIDEO_BITRATE` | str | Must be valid bitrate (number + `k`/`M`/`G` suffix) |
| `AUDIO_BITRATE` | str | Must be valid bitrate |
| `HLS_SEGMENT_DURATION` | int | Min 1 |
| `ABR_ENABLED` | bool | Must be `true`/`false`/`1`/`0` |
| `ENABLE_COPY_MODE` | bool | Must be `true`/`false`/`1`/`0` |
| `VIRTUAL_ABR_TIERS` | bool | Must be `true`/`false`/`1`/`0` |
| `ABR_TIERS` | tiers | `height:bitrate` pairs, height > 0, valid bitrate, at least 1 tier |
| `TIER0_BITRATES` | tiers | Same format as ABR_TIERS |
| `TIER0_BITRATE_DEFAULT` | str | Must be valid bitrate |
| `TMDB_API_KEY` | str | No validation (pass-through) |
| `METADATA_AUTO_FETCH_ENABLED` | bool | Must be `true`/`false`/`1`/`0` |
| `METADATA_REFRESH_DAYS` | int | Min 1 |
| `INTRO_DETECTION_ENABLED` | bool | Must be `true`/`false`/`1`/`0` |
| `INTRO_CHROMAPRINT_ENABLED` | bool | Must be `true`/`false`/`1`/`0` |
| `TAC_COMMENTS_ENABLED` | bool | Must be `true`/`false`/`1`/`0` |
| `JOB_TIMEOUT_SECONDS` | int | Min 1 |
| `QUEUE_TIMEOUT_SECONDS` | int | Min 1 |
| `PENDING_UPLOAD_TTL_SECONDS` | int | Min 1 |
| `PENDING_UPLOAD_CLEANUP_INTERVAL_SECONDS` | int | Min 1 |
| `JOB_RETENTION_DAYS` | int | Min 0 |
| `MAX_CONCURRENT_JOBS` | int | Min 1 |
| `UPLOAD_RATE_LIMIT_WINDOW` | int | Min 1 |
| `UPLOAD_RATE_LIMIT_MAX_REQUESTS` | int | Min 1 |
| `MAX_PENDING_UPLOADS_PER_IP` | int | Min 0 |
| `WATCH_POLL_SECONDS` | int | Min 1 |
| `WATCH_STABLE_SECONDS` | int | Min 1 |
| `WATCH_VIDEO_EXTENSIONS` | list | Auto-prepends `.`, validates no path separators |
| `WATCH_IGNORE_SUFFIXES` | list | Validates no path separators |
| `UPLOAD_PARALLELISM` | int | Min 1 |
| `DB_SYNC_ENABLED` | bool | Must be `true`/`false`/`1`/`0` |
| `DB_SYNC_BOOTSTRAP` | str | No validation (pass-through) |
| `DB_AUTO_MERGE_INTERVAL_MINUTES` | int | Min 0 |
| `DB_AUTO_MERGE_FILE_ID` | str | If non-empty, must be valid Telegram file_id (50-255 alphanumeric chars + `_` `-`) |
| `DB_AUTO_MERGE_BOT_INDEX` | int | Min 0 |
| `WEBHOOK_URL` | str | No validation (pass-through) |

## Design Principles

1. **Fail at startup, not at runtime.** Validation happens when settings are loaded and normalized, not when they're first used. A bad `PORT` value is caught when the process starts, not when someone tries to connect.

2. **Explicit over implicit.** Every setting has a defined type, default, and description. No magic defaults, no hidden behaviors.

3. **One canonical form.** After normalization, a setting value has exactly one valid string representation. `"true"` is always lower-case, integers never have leading zeros, bitrates always use the original unit suffix.

4. **Inline comments for env files.** The `strip_inline_comment` function acknowledges that `.env` files benefit from documentation next to the value — and ensures the comment never leaks into the parsed value.

5. **Validate at key boundaries.** Per-key validation in `normalize_string` and `normalize_list` handles the reality that different settings of the same type need different rules. A generic validation approach wouldn't catch the difference between a `HOST` (must be IP) and a `PREFERRED_ENCODER` (must be one of 4 values).
