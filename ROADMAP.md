# THLS Next Roadmap

This file tracks the next iteration — bug fixes, guards, performance, and
feature work discovered during codebase review.

Priorities: bug > guard > perf > feature.

---

## Active Work

### P2 — Reliability

- [ ] **Out-of-order browser upload can mark a chunk received without writing its bytes**
  Current `cargo test` fails in `api::tests::upload::out_of_order_chunks_resume_and_finalize`: after uploading chunks in order `2, 0, 1`, `/api/upload/status/:id` reports `received_indices = [0, 1, 2]`, but reading the staged upload file returns `abcd\0\0\0\0ij` instead of `abcdefghij`. The affected path is `src/api/uploads.rs:270-340`, where the handler records the chunk as received after the write path; audit the current chunk body/write/offset flow before trusting resumable uploads with sparse or out-of-order chunks.

### P6 — New Features

- [ ] **Remote URL ingest UI**
  `POST /api/ingest/url` exists, but `/upload` has no form for it. Add a small URL input on the upload page that submits a remote URL, shows the existing `downloading` job progress through `/api/status/:job_id`, and then reuses the normal processing/result UI.

- [ ] **Operations panel on settings**
  Add a compact read-only panel on `/settings` that fetches `/health` and `/api/metrics`: DB health, disk free, queue depth, cache usage/hit rate, Telegram bot session stats, FFmpeg encoder, and cloudflared status. Keep it simple and refresh manually or on a slow interval.

- [ ] **Auto-play next episode**
  `static/watch.js` already fetches sibling episodes and renders previous/next navigation. When a series episode ends, optionally navigate to the next episode if one exists, with a small cancelable countdown.

- [ ] **Library cleanup tools**
  Add a settings page section for failed/cancelled jobs, old backups, orphaned upload files, and orphaned processing/cache entries. Default to dry-run counts and require an explicit button per cleanup action.

### P7 — Code Quality

### Current Verification Baseline
- `cargo test` passes: 176 passed, 1 ignored.
- `cargo fmt --check` passes (clean).
- `cargo clippy --all-targets --all-features` passes with 0 warnings.

---

## Completed Archive

### P0 — Critical Bugs
- [x] `playback_progress` import overwrites newer local watch positions — newer-wins UPSERT (`WHERE excluded.updated_at > playback_progress.updated_at`)
- [x] `rename_series` corrupts fingerprints across media types + is non-transactional — added `AND media_type = ?3` scope and wrapped all 3 stmts in a transaction
- [x] `finish_inflight` removes key from inflight map before `notify_waiters` — reordered: set outcome → notify → remove, eliminating spurious-leader window
- [x] `single_flight` inflight entry permanently unresolvable if leader panics — RAII `InflightGuard` calls `finish_inflight` with an error on Drop, resolving all waiters immediately
- [x] Chunk uploads can buffer unbounded request bodies
- [x] Concurrent duplicate chunks can corrupt uploaded files
- [x] Concurrent finalize can enqueue duplicate jobs for one upload
- [x] Database load can run while active jobs are still writing
- [x] Live DB replacement can leave no active database after cross-device rename failure
- [x] DB backups can miss committed WAL data
- [x] Virtual ABR distorts non-16:9 sources
- [x] Oversized `.m4s` segments detected but never repaired; repair count incorrectly reports success
- [x] `replace_live_database` installs an empty pool before the file rename completes
- [x] Stored XSS via `</script>` in series name embedded in `<script>` tag — post-process serde_json output to escape forward slashes as `\/` before embedding
- [x] `save_job` never populates `created_at_unix` — added `created_at_unix` with `strftime('%s','now')` to INSERT
- [x] `merge_from_export` loses `episode_title` for new jobs — added `episode_title` to INSERT column list and bound exported value
- [x] `merge_from_export` does not populate `created_at_unix` — added `created_at_unix` with `strftime('%s','now')` to INSERT
- [x] `validate_sqlite_header` reads entire DB into memory — replaced with `File::open` + `read_exact` on 16-byte buffer
- [x] `replace_live_database` race condition — held jobs lock across entire swap to prevent concurrent enqueue orphan
- [x] Virtual ABR FFmpeg transcoding has no timeout/cancellation — wrapped in `run_ffmpeg_cancellable` with job timeout and cancellation token
- [x] Remux to fMP4 uses global `hls_segment_duration` — passed per-job `target_secs` to remux `-hls_time`
- [x] `analysis_from_ffprobe` does not filter attached-pic/album-art streams — skip streams with `attached_pic == 1` or `mjpeg`/`png` codec when real video exists
- [x] `RetryAfter(0s)` zero-delay retry loop — clamped `retry_after` to `max(1, value)`
- [x] Overly broad stale-file-id detection — match specific patterns (`file_id_invalid`, `wrong file_id`, `invalid file id`) only
- [x] `store_config` writes `selected_encoder` before `config` — reordered to write config first, then encoder
- [x] `handle_reset_settings` does not reload config from DB — added `Config::load` call after restoring defaults
- [x] `enqueue_existing_job` overwrites terminal job state — added `is_terminal()` guard before overwriting state back to `Queued`
- [x] **Intro/outro fingerprint hardcodes `0:a:0`, silently fails on video-only sources** — added `has_audio` check in `processing_markers.rs`; chromaprint pass skipped when analysis has no audio channels; falls back to chapter/black/silence detection only
- [x] **`replace_database_file` can leave no active DB after a failed install** — wrapped post-backup install steps in a closure; on any error, `fs::rename(backup → active)` is attempted before returning the original error

### P1 — Performance (High Impact)
- [x] Browser upload is strictly serial even though chunk retries are resumable
- [x] Upload resume assumes contiguous chunks
- [x] No lightweight playback/cache benchmark exists
- [x] Cache warm-up is too coarse for home-server bandwidth
- [x] **`ffprobe`/`ffmpeg` in marker and probe paths run without timeout or cancellation** — `analyze_media` wrapped with 60s `tokio::time::timeout`; `probe_duration` with 30s timeout; `generate_fingerprint_window`, `detect_silence_points`, `detect_black_points` each wrapped with appropriate wall-clock timeouts

### P2 — Reliability
- [x] **`encode_video_tier` always reports zero oversized-segment repairs** — `repair_oversized_video_segments` now returns `Result<usize>` with actual repair count; hardcoded `m4s_repair_count = 0` eliminated
- [x] **`mark_segment_played_and_cleanup` runs full DB query on every segment access** — `segment_meta_cache` with 1-hour TTL on `AppState`; DB queried only on cache miss/expiry
- [x] **`played_segments` HashMap grows per job until source cleanup triggers** — 2-hour TTL pruning on every access; entries removed on source cleanup
- [x] **`client_ip` returns `127.0.0.1` when `behind_proxy = false`, defeating per-IP rate limiting** — `ConnectInfo<SocketAddr>` extracts real peer address; wired via `into_make_service_with_connect_info`
- [x] **`backfill_tmdb_episode_titles` makes N+1 API calls with no rate-limit backoff** — 429 detection with `Retry-After` header + single retry; 250ms inter-request throttle
- [x] **`handle_post_watch_settings` calls `seen.clear()` on every settings save** — conditional clear only when `watch_enabled`, `watch_root`, or `watch_done_dir` actually change
- [x] No graceful shutdown for in-flight jobs and uploads
- [x] Background workers die silently with no supervisor
- [x] Crash recovery only catches `processing` state, not `queued`/`uploading`/`analyzing`
- [x] Telegram retries have no jitter, no max-sleep cap, and no per-bot circuit breaker
- [x] FFmpeg has no per-process timeout and SIGKILL is sent with no SIGTERM grace
- [x] `save_job` races with concurrent reads because SQLite pragmas are missing
- [x] Watch-folder re-enqueues files after restart because dedup is in-memory only
- [x] Orphaned pending-upload files survive restarts and accumulate
- [x] Round-robin upload-bot counter is unfair under partial failure
- [x] Terminal jobs evicted from memory after 5 min, losing status responses and log correlation
- [x] URL ingest can spawn unbounded background downloads
- [x] Remote download has no explicit wall-clock or idle timeout
- [x] Ingest download task ignores `job_timeout_watcher` cancellation
- [x] `handle_cancel_job` deletes processing directory while FFmpeg may still be writing to it
- [x] Missing `spawn_blocking` for blocking rusqlite calls in DB transfer and frontend handlers
- [x] `selected_encoder` not refreshed when encoder-related settings change
- [x] Stale Telegram `file_id` recovery triggers full source re-encode for a single segment
- [x] `transcode_segment` leaks FFmpeg output file when `tokio::fs::read` fails after encode
- [x] `env_writer` `Mutex` not poison-safe; subsequent writes panic after any rewrite failure
- [x] `env_writer` missing `fsync` before rename; power loss can corrupt `.env`
- [x] `handle_post_settings` has a TOCTOU race under concurrent modification
- [x] Ingest disk-space pre-check is silently skipped when remote server omits `Content-Length`
- [x] `analysis_from_ffprobe` silently accepts zero-duration media, causing downstream FFmpeg failures
- [x] Settings persistence returns success when `.env` write fails, causing config to diverge on restart
- [x] `collect_segment_durations` swallows probe failures — propagated error via `.with_context()?` instead of `unwrap_or(0.0)`
- [x] `repair_needs_split` floor is off-by-one — changed `<=` to `<` for 32 kbps threshold
- [x] `save_media_markers` performs N inserts with no transaction — wrapped in `unchecked_transaction()`
- [x] db-sync partial per-bot upload produces inconsistent bootstrap descriptor — only include bot's uploads in descriptor if all parts succeeded
- [x] `probe_duration` uses FFprobe with `concat:` URL — probe `.m4s` segments directly without `concat:` wrapper
- [x] `count_series_groups`/`count_season_groups` WHERE contradiction — folded condition into CTE to avoid cross-clause conflicts
- [x] `upload_rate_limits` HashMap grows unboundedly — prune empty deque entries from map
- [x] Watch-folder dedup in-memory only / `seen.clear()` on every settings save — `seen.clear()` now only fires when watch-relevant fields change
- [x] `reconstruct_job_source` has no timeout or cancellation — wrapped in `run_ffmpeg_cancellable`
- [x] Reprocess leaks reconstructed temp file on enqueue failure — cleanup temp file in error arm when `delete_source` is true
- [x] Graceful shutdown does not wait for background workers — wired `shutdown_token.cancelled()` into supervised workers, collect and await join handles
- [x] Search `LIKE` does not escape `%` and `_` wildcards — added `escape_like()` with `ESCAPE '\'`
- [x] `get_season_episode_job_ids` missing `media_type` filter — added `AND media_type = ?2` to WHERE clause
- [x] `merge_from_export` media_markers dedup uses exact float equality — range comparison with `ABS() < 0.01`
- [x] `position_seconds`/`duration_seconds` accept `Infinity` — added `is_finite()` guard
- [x] `stream_to_file` acquires `jobs` Mutex on every download chunk — batched progress updates
- [x] Local `double_bitrate` in virtual ABR mishandles fractional bitrates — parse full numeric portion including decimal
- [x] `handle_db_export` loads entire DB snapshot into memory — stream via `ReaderStream`/`Body::from_stream`
- [x] `parse_bitrate_bps` rejects multi-character suffixes — ported multi-char suffix parsing from `bitrate_bits`
- [x] Virtual ABR 16:9 fallback for zero source dimensions — fall back to copy without scaling
- [x] `insert_job_marker` / `insert_processing_marker` don't populate `created_at_unix` — added `strftime('%s','now')`
- [x] `save_playback_progress` backward-seek blocked — removed `WHERE position_seconds >` guard; in-session writes always overwrite, allowing backward seek and replay
- [x] `save_playback_progress` and `merge_from_export` recency semantics differ — `merge_from_export` already uses `updated_at`; `save_playback_progress` now always overwrites (in-session), keeping them aligned
- [x] `copyPlayerUrl` crashes — `nextElementSibling` returns null — select button by ID instead of sibling DOM order
- [x] Browse page search non-functional — references hidden input — dynamically create visible `#pageSearchInput` in filter bar
- [x] Extra `</div>` in video card HTML breaks layout — removed stray closing div
- [x] Event listeners accumulate on player re-initialization — AbortController pattern for cleanup
- [x] `readEntries` called only once — loop until `readEntries` returns empty array
- [x] Double HTML escaping in `metaParts` — escape exactly once at render time
- [x] `loadMoreJobs` increments page counter before fetch — increment only after successful fetch
- [x] `browse-home.js` globally overrides utility functions — wrapped in IIFE, helpers module-scoped
- [x] `_seriesDetailSelected`/`_seriesDetailEpisodes` never reset — reset on series detail key change
- [x] Event listeners accumulate on `metadataTableWrap` — add delegated listener once via dataset flag
- [x] Resume position applied twice causing visible jump — single seek via `player.load()` start time, removed redundant second seek
- [x] **Watch-folder overwrite path loses the pre-existing done file when enqueue fails** — renamed existing done file to `.done.bak` sidecar before overwrite; restored on enqueue failure; deleted on success
- [x] **Watch-folder duplicate check ran after rename, silently losing the source file** — moved `job_exists_active_by_filename` check before `fs::rename`; both queries now scoped with `media_type = "Film"`
- [x] **`handle_cancel_job` skipped processing-dir and source-file cleanup** — extracts `source_path`, `processing_path`, `delete_source_on_finish` while holding jobs lock; calls `cleanup_job_paths` after DB write
- [x] **Per-IP upload rate limiter recorded denied request before rejecting** — length check moved before `push_back`; 429 path no longer records a timestamp slot
- [x] **`ts_dir` leaked on every error path in `encode_video_tier`** — wrapped entire fallible body in an immediately-awaited `async { }` block; `remove_dir_all(&ts_dir)` called unconditionally after the block exits
- [x] **`upload_locks` HashMap grew unboundedly across bot-token rotations** — added `prune_upload_locks(active_tokens)` to `TelegramRuntime`; called from bot-add and bot-delete handlers after config write
- [x] **Telegram 401/404/410 misclassified as `Retryable`** — extended `Permanent` branch in `classify_api_body` to `matches!(code, 400 | 401 | 403 | 404 | 410)`
- [x] **`get_file_bytes_attempt` passed empty JSON to classifier on non-2xx file GET** — synthetic body `{"error_code": status, "description": "file GET HTTP {status}"}` enables 404→Permanent path and stale-file_id text matcher

### P3 — Data Model
- [x] DB export/import loses split segment parts
- [x] Segment prefix lookup uses unescaped SQL `LIKE`
- [x] Legacy rev-18 detection can miss `segment_parts` columns
- [x] `output_audio_channels` exact-string match misses `"5.1(side)"` surround layout
- [x] `select_video_tiers_with` ignores `cfg.abr_enabled`
- [x] `X-TIMESTAMP-MAP` injection silently skipped for WebVTT files with non-standard header
- [x] `double_bitrate` silently misparses multi-character unit suffixes
- [x] Rate-limit default disagrees between `Config::default()` and the settings registry
- [x] No config-load invariant tying `segment_target_size` to `telegram_max_file_size`
- [x] `enforce_invariants` is incomplete — missing several cross-field constraints
- [x] DB silently overrides env vars with no warning — manual `.env` edits ignored
- [x] Values containing `#` written unquoted to `.env` — fragment silently stripped on reload
- [x] Misleading error message in `handle_post_settings` — claims changes will be lost when they won't
- [x] `db_auto_merge_bot_index` used as vector index without bounds check
- [x] `validate_table_name` allowlist missing 7 tables
- [x] `validate_column_name` allowlist missing many columns from newer tables
- [x] **`get_job_source_bitrate_by_filename` missing `media_type` scope** — added `media_type: &str` parameter and `AND media_type = ?2` to WHERE clause; watch-folder caller passes `"Film"`
- [x] **`job_exists_active_by_filename` missing `media_type` filter** — same shape as above; watch-folder dedup now correctly scoped
- [x] **`delete_job` not transactional across cascade** — wrapped DELETE in `unchecked_transaction()` matching surrounding style

### P4 — Security Hardening
- [x] Settings-to-`.env` write path allows newline injection
- [x] JSON/API settings silently strip `#` fragments
- [x] Segment URI sanitizer allows `.` and `..` components
- [x] Upload status endpoint does not validate ownership
- [x] Telegram errors may leak bot tokens
- [x] Public proxy settings are not enforced consistently
- [x] **Basic-auth credential comparison is not constant-time** — replaced `user == ... && pass == ...` with custom `constant_time_eq` that XOR-accumulates byte differences across the entire length
- [x] **TMDB API key exposed in request URLs (logged by reqwest/proxies)** — `reqwest` built with `default-features = false` (no `log` feature); URL logging disabled; documented in code comment
- [x] **`upload_document` records upload-error metric for pre-check size failures** — removed `record_upload_error` call from size pre-check path; metrics only recorded for actual Telegram API failures
- [x] **`redact_bot_token` aborted on short false-match, leaving later real tokens unredacted** — replaced `break` with `pos = end; continue;` so scanning continues past the false match
- [x] **Proxy client IP extraction trusted wrong XFF hop** — rewrote `client_ip` to check peer against `trusted_proxy_cidrs` first; returns leftmost non-trusted XFF entry when peer is a trusted proxy; hand-rolled IPv4/IPv6 CIDR membership, no new dependency

### P5 — Operational
- [x] **Marker `max_offset` capped at `a_len/4` silently misses real episode drift** — removed `.min(a_len / 4)` cap; `max_offset` is now `MAX_OFFSET_POINTS` alone
- [x] **`max_plaintext_size` parameter name is misleading** — renamed parameter from `max_upload_size` to `telegram_max_file_size`
- [x] Watch-folder claim can overwrite existing done files
- [x] Startup probes for `ffmpeg`/`ffprobe` can hang indefinitely
- [x] Cloudflared child can survive THLS shutdown
- [x] Runtime cache directory is not ignored or documented as runtime data
- [x] `jittered_backoff` jitter is always 0 for every real retry — replaced integer-truncating formula with `random() % base_ms.max(1)` additive jitter
- [x] `delete_old_jobs` silently no-ops on negative input — added `bail!` guard for `older_than_days < 0`
- [x] `env_writer` glues stale inline comments onto rewritten values — removed `find_inline_comment`; full line replacement; `#` values properly quoted
- [x] `parse_bool` rejects `0`/`1` — both `parse_bool` and `normalize_bool` now accept `0`/`1`
- [x] `setting_value` silently returns empty string for unmapped keys — added `tracing::warn!` in wildcard arm
- [x] `validate_file_id` may reject valid Telegram file_ids — changed to sanity-check approach (reject only empty/short/long/control chars)
- [x] `repair_oversized_segment_max_bitrate` leaks `.tmp` file on rename failure — added `remove_file` cleanup in error arm
- [x] `is_bitrate` validation accepts `.5M` and `5.M` — now requires digits before and after optional dot
- [x] `browse-home.js` overrides `escapeHtml` globally — wrapped in IIFE, renamed to `esc`
- [x] Global polling interval in `shared.js` never cleared — cleared on `pagehide`/`beforeunload`
- [x] Hero rotation `setInterval` in `browse-home.js` never cleared — `stopTimer()` on `pagehide`/`beforeunload`; `startTimer()` clears before creating
- [x] `pollStatus` interval in `upload.js` leaks on navigation — `activeStatusPolls` Set + cleanup on `pagehide`/`beforeunload`
- [x] **`HLS_SEGMENT_DURATION` not runtime-settable** — added registry entry (Int, min 2, max 10, default 4); added `setting_value` and `apply_setting` arms; updated field comment
- [x] **`set_setting`/`set_settings` accepted arbitrary values without validation** — added `normalize_str_for_key(key, value)` call before INSERT; write fails with typed error if validation fails

### P7 — Code Quality (Completed)
- [x] Audit and remove fragile `.unwrap()` / `.expect()` calls in runtime + worker paths — propagated via `?` in handlers; `.expect("reason")` for provably-infallible sites; startup/migration unwraps left loud-at-boot by design
- [x] `cargo fmt --check` formatting drift — ran `cargo fmt`; all files now pass `--check`
- [x] `cargo clippy --all-targets --all-features` emitting warnings — auto-fixed mechanical lints; `#[allow(clippy::too_many_arguments)]` where justified; boxed large `Err` variant; `sort_by_key` replacements; 0 warnings

### P6 — New Features (Completed)
- [x] Continue watching (server-side)
- [x] External metadata cache (TMDB, AniList)
- [x] Intro/outro marker detection
- [x] Anime Community comment embed
- [x] Skip intro/outro button in Shaka player
- [x] Metadata search/link UI on upload/edit
- [x] Continue watching (browser localStorage)

---

## Non-Goals (Preserved)

- Do not add application-level authentication.
- Do not introduce multi-tenant isolation.
- Do not require distributed workers or horizontal scaling.
- Do not store source videos permanently on local disk.
- Do not expose Telegram `file_id`, bot tokens, or channel IDs to clients.
- Do not bypass the local DB for segment lookup.
- Do not add new crate dependencies without documented justification.
