# THLS Next Roadmap

This file tracks the next iteration — bug fixes, guards, performance, and
feature work discovered during codebase review.

Priorities: bug > guard > perf > feature.

---

## P0 — Critical Bugs

- [x] **Chunk uploads can buffer unbounded request bodies**
  `src/api/mod.rs:166` disables Axum's default body limit globally while `src/api/uploads.rs:202-206` extracts the chunk as `Bytes` before handler validation. A malformed or oversized `POST /api/upload/chunk` can be fully buffered in memory before `UPLOAD_CHUNK_SIZE` checks run, risking process OOM. Add a route/body-size limit that rejects oversized chunks before buffering.

- [x] **Concurrent duplicate chunks can corrupt uploaded files**
  `src/api/uploads.rs:227-274` checks `received_chunks` under `pending_uploads`, then drops the lock before writing to the target offset. Two same-index chunk requests can both pass the check and race-write different bytes; finalization can still see a complete upload. Serialize writes per upload/chunk or reserve the chunk while holding state.

- [x] **Concurrent finalize can enqueue duplicate jobs for one upload**
  `src/api/uploads.rs:343-402` validates completion, releases the pending-upload lock, enqueues, then removes the upload. Two finalize requests can both enqueue jobs pointing at the same source file. Atomically remove/mark the pending upload as finalizing before enqueue, and make duplicate finalize return a clear 404/409.

- [x] **Database load can run while active jobs are still writing**
  `src/api/db_transfer.rs:396-418` waits for SQLite pool drain but does not reject queued/downloading/processing/uploading jobs in `state.jobs`. A job can finish after DB replacement and save old work into the newly loaded database. Block live DB replacement while non-terminal jobs exist.

- [x] **Live DB replacement can leave no active database after cross-device rename failure**
  `src/api/db_transfer.rs:232-234` writes uploaded replacement DBs under `std::env::temp_dir()`, while `src/db/transfer.rs:163-168` renames the active DB to backup before renaming the temp source into place. If the temp path and active DB are on different filesystems, `rename(source, active)` can fail after the active DB was moved aside. Copy or stage the replacement on the active DB filesystem before swapping, and roll back on failure.

- [x] **DB backups can miss committed WAL data**
  `src/db/transfer.rs:190-198` runs `PRAGMA wal_checkpoint(TRUNCATE)` best-effort, ignores checkpoint result rows/errors, then copies only the main `.db` file. If WAL frames remain busy, the backup can be stale or incomplete. Treat failed/busy checkpoint as backup failure or use SQLite's backup API.

- [x] **Virtual ABR distorts non-16:9 sources**
  `src/api/playback/virtual_.rs:346` hardcodes `scale='trunc({target_height}*16/9/2)*2':{target_height}` and the playlist advertises the same assumed ratio. 4:3, vertical, and ultrawide sources are transcoded and advertised incorrectly. Use the source track's actual aspect ratio for playlist resolution and FFmpeg scale.

- [x] **Oversized `.m4s` segments detected but never repaired; repair count incorrectly reports success**
  `src/media/process.rs:300-342` — After fMP4 remux, oversized `.m4s` segments are collected into `oversized_m4s`, but the repair loop (lines 324-342) only emits a warning. No re-encode or keyframe-split is performed. `m4s_repair_count` is then set to `oversized_m4s.len()` and returned as the "repaired" count, falsely reporting success. Jobs with large `.m4s` segments will fail at Telegram upload with no actionable error; callers have no signal that repair was skipped. The `.ts` repair path works correctly; the `.m4s` path is an unimplemented stub. Either implement re-encode to a lower bitrate (same resolution, per the Telegram invariant) or set `m4s_repair_count = 0` and document that the upload-time byte-splitting path handles these.

- [x] **`replace_live_database` installs an empty pool before the file rename completes**
  `src/api/db_transfer.rs:409-411` — `std::mem::replace` at line 409 swaps the live pool with a freshly-initialised lazy pool pointing at the original DB path. `replace_database_file` (line 411) then renames the original to a backup and moves the uploaded file into place. Any request that acquires a connection between line 409 and the completion of the rename can either connect to the already-moved backup file or receive a file-not-found error, both surfacing as unexpected 500s. Move the file rename entirely before installing the new pool, or hold the `state.db` write-lock across both operations.

---

## P1 — Performance (High Impact)

- [x] **Browser upload is strictly serial even though chunk retries are resumable**
  `static/upload.js:418-456` sends chunks one at a time. After the upload race fixes in P0, add small bounded parallelism (for example 2-4 chunks) so large local uploads use available bandwidth without overwhelming disk writes or request limits. Keep retries per chunk and stop all in-flight work on cancel.

- [x] **Upload resume assumes contiguous chunks**
  `static/upload.js:396-404` resumes from `received_chunks`, but `src/api/uploads.rs:456-468` can return explicit `received_indices`. If chunks are uploaded out of order or a future parallel upload leaves gaps, the browser can skip missing chunks. Resume from `received_indices` and send only missing indices.

- [x] **No lightweight playback/cache benchmark exists**
  The repo has `/api/metrics`, cache counters, Telegram metrics, and an ignored manual cache smoke test, but no repeatable command that reports first-segment latency, cache-hit latency, Telegram fetch latency, and virtual ABR transcode latency. Add a small script or ignored test that exercises one completed job and prints these timings.

- [x] **Cache warm-up is too coarse for home-server bandwidth**
  `CACHE_WARMUP_ENABLED` exists and `spawn_cache_warmup` is called after job completion, but warm-up should stay conservative: first playable video segment, first audio segment, and thumbnail only, gated by cache budget and `SEGMENT_PREFETCH_MIN_FREE_BYTES`. Avoid warming whole jobs or all tiers.

---

## P2 — Reliability

- [x] **No graceful shutdown for in-flight jobs and uploads**
  `src/main.rs:107-114` only awaits Ctrl-C; Axum's `with_graceful_shutdown` drains HTTP connections but the spawned `job_dispatcher`, `process_job` tasks, FFmpeg children, Telegram uploads, and chunked uploads are abruptly killed. Jobs left in `processing`/`uploading` accumulate as stuck rows in the DB. Add a shared `CancellationToken`/broadcast, signal all worker tasks, and wait for them (with a deadline) before the process exits.

- [x] **Background workers die silently with no supervisor**
  `src/api/jobs/processing.rs:132-145` breaks the dispatcher loop on `acquire_owned` error with no log or restart; `upload_sweeper`, `watch_folder_poller`, and `job_timeout_watcher` are all `tokio::spawn`ed and never monitored. A panic in `process_job` also loses one semaphore permit permanently. Wrap each worker in a supervisor that logs the exit cause, catches panics (`JoinHandle::is_panicked`), and respawns with backoff; track semaphore permits explicitly so they cannot leak.

- [x] **Crash recovery only catches `processing` state, not `queued`/`uploading`/`analyzing`**
  `recover_stuck_processing_jobs` at `src/api/jobs/processing.rs:935-956` and `src/db/queries.rs:686-697` queries `status='processing'`. Jobs that crashed before the processing marker was written (see `src/api/jobs/processing.rs:172-177`) stay as `queued`/`uploading`/`analyzing` in the DB with no in-memory state and no heartbeat. Add a `jobs.lease_expires_at` column, treat any non-terminal row whose lease lapsed as stuck, and either re-enqueue from `source_path` or mark as failed.

- [ ] **Telegram retries have no jitter, no max-sleep cap, and no per-bot circuit breaker**
  `src/telegram.rs:205-282,297-330` hardcodes `MAX_ATTEMPTS=3`, backs off as `2^attempt` seconds with no jitter (concurrent uploads thunder-herd), and honors `RetryAfter` but won't sleep past the attempt budget — a single 60s flood-wait permanently fails the job. No per-bot failure counter means `assign_upload_bots` keeps round-robining to banned or blocked bots. Parametrize retries from config, add full jitter, cap individual sleep duration, and track per-bot rolling error rates to skip unhealthy bots.

- [ ] **FFmpeg has no per-process timeout and SIGKILL is sent with no SIGTERM grace**
  `run_ffmpeg_cancellable` at `src/media/process.rs:794-858` reacts to `cancel_flag` but has no wall-clock timeout; a hung encoder only stops when the global `job_timeout_watcher` flips the flag (`src/api/jobs/processing.rs:783-840`). `child.kill()` sends SIGKILL directly, leaking hwaccel state. The stderr ring buffer keeps only the last 8 KB (`src/media/process.rs:810-813`), truncating real error lines on long encodes. Add a configurable per-tier timeout, SIGTERM-then-SIGKILL with a grace period, and keep head + tail of stderr (or stream lines through `tracing` on Debug).

- [ ] **`save_job` races with concurrent reads because SQLite pragmas are missing**
  `src/db/queries.rs:13-110` runs `DELETE` + bulk `INSERT` inside one transaction; without `busy_timeout` or `synchronous=NORMAL` (`src/db/mod.rs:28-30,75-78` only set WAL + foreign_keys), concurrent playback readers hit `SQLITE_BUSY` and surface as 500s. The `INSERT OR REPLACE` on the `jobs` row also fires `ON DELETE CASCADE`, which is exactly what the explicit deletes are working around — fragile. Set `PRAGMA busy_timeout=5000; PRAGMA synchronous=NORMAL` and replace `INSERT OR REPLACE` with `INSERT … ON CONFLICT … DO UPDATE`.

- [ ] **Watch-folder re-enqueues files after restart because dedup is in-memory only**
  `src/api/watch_folder.rs:127-177` keeps `watch_seen` in a runtime `HashMap`; on restart it's empty (`src/main.rs:86`), so any file still in `watch_root` (not yet moved to `done/`) is re-stat'd, marked stable, and re-enqueued — duplicating a job that may already be in the DB. `seen.clear()` also runs on every settings save (`src/api/watch_folder.rs:102`). Persist watch claims (e.g., a `watch_claims` table keyed by canonical inode/path + size + mtime) and consult it before enqueue.

- [ ] **Orphaned pending-upload files survive restarts and accumulate**
  `src/api/uploads.rs:233-239,394-426` relies on an in-memory `Mutex<HashMap>` for TTL tracking. On restart the map is empty, so partial files in `uploads/` preallocated to `total_size` via `set_len` (`src/api/uploads.rs:163-166`) are never reconciled; `uploads/` accumulates orphaned ~100 GB sparse files. At startup, scan `uploads/` and delete files whose `upload_id` has no matching pending state and no completed job; persist pending uploads to a `pending_uploads` table.

- [ ] **Round-robin upload-bot counter is unfair under partial failure**
  `src/api/jobs/processing.rs:556-574` calls `set_last_bot_index` before uploads run; if `upload_outputs` fails, the counter has already advanced. The `round_robin` mutex is also held only across `get_last_bot_index` + `set_last_bot_index`, so concurrent jobs interleave bot assignments after the lock drops, defeating fairness. Persist the new counter only after the last upload succeeds, or atomically reserve a range in the DB via `UPDATE … RETURNING`.

- [ ] **Terminal jobs evicted from memory after 5 min, losing status responses and log correlation**
  `cleanup_old_terminal_jobs` at `src/api/jobs/processing.rs:842-852` drops terminal jobs from `state.jobs` after 300s; subsequent status/poll requests cannot return the error message even though the DB has it. There is also no per-job `tracing` span, so concurrent jobs interleave logs unreadably. Wrap every per-job task in `tracing::info_span!("job", job_id)` and have the status handler fall back to a DB read for terminal jobs after in-memory eviction.

- [ ] **URL ingest can spawn unbounded background downloads**
  `src/api/ingest.rs:24-70` accepts each URL and immediately `tokio::spawn`s a downloader. A client can start many remote downloads that consume disk, outbound sockets, and queue slots before normal upload limits apply. Add a small in-memory semaphore for URL ingest, clear status/cancel behavior while waiting, and reuse the existing upload/job limits where practical.

- [ ] **Remote download has no explicit wall-clock or idle timeout**
  `src/api/ingest.rs:72-282` builds a fresh reqwest client and streams chunks until completion, but there is no per-download deadline or per-read idle timeout. A slow or stalled origin can keep a job in `downloading` indefinitely until broader job timeout logic notices. Add bounded timeout behavior and surface `download_timed_out` as a clear job error.

- [x] **Ingest download task ignores `job_timeout_watcher` cancellation**
  `src/api/ingest.rs:354-356` — `stream_to_file` exits early when `cancel_requested || status == Cancelled`. The `job_timeout_watcher` sets `job.status = Error` (not `Cancelled`), so the per-chunk cancel check never fires on timeout. The spawned download task runs to completion after the job has already been marked `error` in memory, then calls `enqueue_existing_job` on a job whose in-memory state is `Error`, which passes the `!cancel_requested && status != Cancelled` guard and re-queues a job that was intentionally timed out. Add `status == Error` (or check the `cancel_flag` atomic) to the `stream_to_file` early-exit condition.

- [x] **`single_flight` inflight entry permanently unresolvable if streaming leader panics**
  `src/api/playback/real.rs:190-249` — The `tokio::spawn`'d leader accumulates bytes and calls `finish_inflight` on success and known-error paths. If the task panics (e.g. OOM in `extend_from_slice`), `finish_inflight` is never called: `outcome` stays `None` and `notify_waiters()` is never called. All subsequent requests for the same `cache_key` call `wait_for_outcome`, loop forever on a notify that never fires, and are permanently hung. Add a `Drop`-guard or `catch_unwind` wrapper that calls `finish_inflight` with an error on any early exit.

- [x] **`finish_inflight` removes key from inflight map before calling `notify_waiters`**
  `src/api/playback/cache.rs:193-194` — The key is removed from `state.cache.inflight` before `inflight.notify.notify_waiters()` fires. A new request arriving in that window calls `claim_inflight`, finds no entry, becomes a spurious leader, and starts a duplicate Telegram fetch for a segment that is already in the cache or being written. Swap the order: notify while the entry still exists in the map, then remove.

- [x] **`handle_cancel_job` deletes processing directory while FFmpeg may still be writing to it**
  `src/api/jobs/handlers.rs:461` — The cancel handler sets `cancel_flag`, then immediately calls `cleanup_job_paths` (which deletes source + processing dir). `process_job` polls `cancel_flag` asynchronously at specific checkpoints; between flag set and FFmpeg noticing the cancellation, the processing directory is deleted from under the encoder. Intermediate files are corrupted and FFmpeg emits confusing errors. Restrict `cleanup_job_paths` to the `process_job` task; the cancel handler should only set the flag and update in-memory status.

- [ ] **Missing `spawn_blocking` for blocking rusqlite calls in DB transfer and frontend handlers**
  `src/api/db_transfer.rs:56,142,371`, `src/api/frontend.rs:192` — `db::export_to_dict`, `db::backup_database_file`, `db::merge_from_export`, and `db::distinct_series_names` are invoked directly on Tokio async task threads. A DB export or import on a large database blocks the runtime thread for seconds to minutes, starving other requests. Wrap each call in `tokio::task::spawn_blocking` as is done throughout the rest of the API layer.

- [ ] **`selected_encoder` not refreshed when encoder-related settings change**
  `src/api/playback/virtual_.rs:37` — `serve_virtual_segment` reads `state.selected_encoder.read().await.clone()` at request time. The encoder was cached correctly per P1, but cache invalidation on settings change may not be wired: if the user changes `preferred_encoder` or GPU path via the settings API, virtual ABR transcodes silently continue using the stale selection (e.g. old VAAPI device path, or CPU encoder after enabling NVENC). Verify that `media::encoder::select_encoder` is called and written to `state.selected_encoder` on any encoder-relevant settings save, and confirm it ends up in the same code path as the initial probe in `main.rs`.

- [ ] **Stale Telegram `file_id` recovery triggers full source re-encode for a single segment**
  `src/api/playback/real.rs:464-518` — `extract_recovery_segment_from_source` calls `media::process_media` on the full source file when recovering from a stale `file_id`. This produces all segments and tiers in `work_dir`; only the one needed segment is read back, and the rest are discarded with `remove_dir_all`. For a 2-hour film this triggers a multi-hour full re-encode consuming tens of GB of disk, just to recover one segment. Add a targeted extraction path (e.g., seek-based single-segment encode, or byte-range extraction from the original `.ts`/`.m4s`) or cap this recovery path to init segments only.

- [ ] **`transcode_segment` leaks FFmpeg output file when `tokio::fs::read` fails after encode**
  `src/api/playback/virtual_.rs:401` — The output file `out_path` is only removed via `let _ = remove_file(&out_path).await` on the success path. If `tokio::fs::read(&out_path).await?` returns an error (disk full, I/O error), the `?` propagates and the `out_path` removal is never reached. The encoded `.mp4` sits in `temp_dir()` permanently. Clean up `out_path` unconditionally (e.g. via defer pattern or an explicit `remove_file` in the error arm).

- [x] **`env_writer` `Mutex` not poison-safe; subsequent writes panic after any rewrite failure**
  `src/env_writer.rs:11` — `WRITE_MUTEX.lock().unwrap()` — if a thread panics while holding the mutex (e.g., from an `unwrap` inside `write_env_values`), the `Mutex` is poisoned. Every subsequent `.env` write panics at this `unwrap`, bubbling out of `spawn_blocking` as a `JoinError`. All future settings persisted to `.env` silently fail. Use `.unwrap_or_else(|e| e.into_inner())` to recover from poisoning or return a typed error.

- [x] **`env_writer` missing `fsync` before rename; power loss can corrupt `.env`**
  `src/env_writer.rs:58-62` — `std::fs::write(&tmp, &content)` followed by `std::fs::rename(&tmp, env_path)`. On Linux, `rename(2)` is atomic at the directory-entry level but does not flush the file's data to disk. A power loss after rename but before the OS flushes dirty pages produces a zero-byte or partial `.env`. Add `File::open`+ `write`+ `sync_all` + `rename` so the data is durable before the directory entry changes.

- [ ] **`handle_post_settings` has a TOCTOU race under concurrent modification**
  `src/api/bots_settings.rs:83-86` — The handler reads `state.config` under a read-lock, clones it, applies changes, drops the lock, does DB work, then re-acquires a write-lock to store the result. Two concurrent POST requests reading the same base snapshot will each overwrite the other's changes. The last writer wins, silently dropping the other request's settings. Read, apply, and store inside a single `write()` lock acquisition, or use a DB-level compare-and-swap keyed on a settings version counter.

- [ ] **`count_series_groups` and `count_season_groups` produce wrong counts when `series_name IS NULL` filter is active**
  `src/db/queries.rs:452,522` — Both functions append `AND series_name != ''` to the caller-supplied `where_sql`. When `where_sql` already contains `WHERE season_number IS NULL`, the compound clause is `WHERE season_number IS NULL AND series_name != ''`, which is contradictory (IS NULL rows cannot satisfy `!= ''`). The query returns 0 instead of the real group count, silently breaking series-grouped pagination. Fold the `series_name != ''` condition into the filter-building logic so it is compatible with other WHERE clauses.

- [ ] **`upload_rate_limits` `HashMap` grows unboundedly with rotating source IPs**
  `src/api/uploads.rs:532-550` — Each unique client IP creates an entry in `state.upload_rate_limits`. Per-IP deque timestamps are pruned on access, but the `HashMap` entry itself is never removed when its deque empties. With `behind_proxy = true` and large NAT pools or CDN forwarding IPs, this is a slow permanent memory leak. After pruning a deque to empty, call `limits.remove(&ip)`, or run a periodic `limits.retain(|_, q| !q.is_empty())` sweep.

- [ ] **Ingest disk-space pre-check is silently skipped when remote server omits `Content-Length`**
  `src/api/ingest.rs:197-213` — The `check_disk_space` call is inside `if let Some(len) = resp.content_length()`. When the origin omits the header, no space check occurs; `stream_to_file` streams up to `max_upload_size` (default 100 GB) before the per-chunk byte counter catches it. Multiple concurrent header-less downloads can exhaust disk. Move the space check to before the response body read using available free space vs. `max_upload_size` as a conservative bound, regardless of whether `Content-Length` is present.

- [ ] **`analysis_from_ffprobe` silently accepts zero-duration media, causing downstream FFmpeg failures**
  `src/media/analysis.rs:41-44` — `duration` defaults to `0.0` when the JSON field is absent or unparseable. Zero-duration analysis is accepted without error. `max_bitrate_for_segment` clamps to a 0.1 s minimum to avoid division by zero, but `encode_video_tier_ts` produces no segments for a zero-duration source, causing `remux_video_ts_to_fmp4` to fail with "no video TS segments produced" — a confusing error that hides the root cause. Add `if duration <= 0.0 { bail!("file reports zero or unknown duration") }` in `analysis_from_ffprobe` to surface the problem immediately.

- [ ] **`probe_duration` uses FFprobe with `concat:` URL which FFprobe does not support as a bare filename**
  `src/media/process.rs:989` — `fmp4_input_arg(path)` formats `concat:/path/init.mp4|/path/video_N.m4s` and passes it as a filename argument to FFprobe. FFmpeg's `concat:` demuxer works via a bare filename; FFprobe does not honour it the same way and will fail to probe `.m4s` segments, silently falling back to `cfg.hls_segment_duration as f64`. This produces inaccurate per-segment duration data used for Telegram split-size calculations. Probe `.m4s` segments directly without the `concat:` wrapper, or derive durations from the HLS playlist instead.

- [x] **Settings persistence returns success when `.env` write fails, causing config to diverge on restart**
  `src/api/bots_settings.rs:68-86` — `write_settings_to_env` failure is logged as a warning but the handler continues to update `state.config` in memory and returns HTTP 200. On the next restart, the `.env` file dominates (per loading order), so the in-memory update is silently reverted. The user receives no indication that their settings will not survive a restart. Return an HTTP error (or at minimum a partial-persistence response) when the `.env` write fails, so callers know the update is non-durable.

---

## P3 — Data Model

- [ ] **DB export/import loses split segment parts**
  `src/db/models.rs:247-253` defines `DbExport` with only jobs/tracks/segments; `src/db/transfer.rs:16-42` never exports `segment_parts`, and `src/db/transfer.rs:125-135` never imports them. Imported split segments keep `is_split=true` but lose all part `file_id`s, breaking playback/download. Include `segment_parts` in export/import and add a round-trip test.

- [ ] **Segment prefix lookup uses unescaped SQL `LIKE`**
  `src/db/queries.rs:337-348` builds `LIKE "{prefix}/%"`, so `_` and `%` in prefixes match unrelated rows. Migration 18 added exact `prefix` columns; use `WHERE prefix = ?` or escape LIKE wildcards. Add a test with `video_0` and `videoA0`.

- [ ] **Legacy rev-18 detection can miss `segment_parts` columns**
  `src/db/migrations.rs:471-478` marks a legacy DB as revision 18 when `tracks`, `jobs`, and `segments` rev-18 columns exist, but does not require `segment_parts.prefix/name`. Bootstrapping can stamp a partially migrated DB as current, then later `save_job()` fails on split-part inserts. Require all rev-18 columns in detection.

- [ ] **`count_series_groups`/`count_season_groups` WHERE contradiction produces wrong pagination counts**
  `src/db/queries.rs:452,522` — Tracked above under P2 (reliability impact); also a data-model invariant violation since the filter logic is part of the query contract.

- [ ] **`output_audio_channels` exact-string match misses `"5.1(side)"` surround layout**
  `src/media/process.rs:1001-1010` — FFprobe often reports 5.1 surround as `"5.1(side)"`. The guard `layout == "5.1"` is an exact match; `"5.1(side)"` falls through to the 2-channel stereo downmix, silently discarding surround audio. Match on `layout.starts_with("5.1")` and `layout.starts_with("3.1")`, or check `audio.channels == 6` / `== 4` directly.

- [ ] **`select_video_tiers_with` ignores `cfg.abr_enabled`**
  `src/media/tiers.rs:60` — `select_video_tiers` gates multi-tier ABR on `cfg.abr_enabled && !cfg.virtual_abr_tiers`. `select_video_tiers_with` (the per-job override variant) checks only `!cfg.virtual_abr_tiers`, ignoring `abr_enabled`. A job submitted with an explicit `abr_tiers_override` on a system with `abr_enabled = false` will silently encode multiple tiers against the operator's intent. Gate on `cfg.abr_enabled` in `select_video_tiers_with` as well, or document the override as intentionally bypassing the global toggle.

- [ ] **`X-TIMESTAMP-MAP` injection silently skipped for WebVTT files with non-standard header**
  `src/api/playback/mod.rs:187-197` — Injection only triggers on bytes starting exactly with `b"WEBVTT\r\n"` or `b"WEBVTT\n"`. A WebVTT file with a BOM, or a header like `WEBVTT NOTE something\n` (valid per spec), silently skips injection. HLS players receive no `X-TIMESTAMP-MAP` and subtitle sync drifts. Match on `starts_with(b"WEBVTT")` and find the first newline rather than requiring an exact header terminator.

- [ ] **`double_bitrate` silently misparses multi-character unit suffixes**
  `src/media/process.rs:1048-1079` — The function checks only `last_char.is_ascii_alphabetic()` and takes one character as the unit suffix. Input like `"128kbps"` has last char `'s'`; `"128kbp"` fails to parse as `f64`, so the function returns the original string unchanged. The returned value is passed to FFmpeg which rejects it. In the current call-graph `double_bitrate` receives only `"Nk"` and `"Nk"` format strings, so this is not triggered today, but the function is incorrect and silently wrong for any multi-char suffix. Apply the same suffix-stripping logic used in `bitrate_bits`.

---

## P4 — Security Hardening

- [ ] **Settings-to-`.env` write path allows newline injection**
  `src/env_writer.rs:30-36` writes normalized setting values directly as `KEY=value`, and generic string settings are not rejected for `\n`/`\r`. A settings update can inject additional `.env` keys. Reject control characters or write properly quoted dotenv values.

- [ ] **JSON/API settings silently strip `#` fragments**
  `src/settings_registry.rs:175-194` strips text after the first `#` for every source, including JSON settings updates. Valid values such as webhook URLs with fragments or paths containing `#` are silently truncated. Restrict inline-comment stripping to env/default parsing or make it whitespace-comment aware.

- [ ] **Segment URI sanitizer allows `.` and `..` components**
  `src/api/playlists.rs:487-497` rejects empty, spaces, `#`, and CR/LF, but still treats `.` as a safe byte and does not reject `.`/`..` path components. Imported DB keys can produce playlist URIs that clients normalize before requesting. Reject dot path components.

- [ ] **Upload status endpoint does not validate ownership**
  `src/api/uploads.rs:411-419` returns pending upload metadata by ID without UUID validation or the IP binding used by chunk/finalize handlers. Anyone with a pending upload ID can read filename, size, and progress. Validate ID format and enforce the same client ownership check.

- [ ] **Telegram errors may leak bot tokens**
  Telegram request URLs include `/bot<TOKEN>/`, and request/decode errors are surfaced through normalized error strings in paths such as `src/telegram.rs:348`. Ensure all Telegram errors and logs redact bot tokens before returning or logging.

- [ ] **Public proxy settings are not enforced consistently**
  `FORCE_HTTPS`, `CORS_ALLOWED_ORIGINS`, and `TRUSTED_PROXY_CIDRS` are exposed in `src/settings_registry.rs:46-49`, but router middleware does not enforce HTTPS redirects or CORS, and `src/api/uploads.rs:513-524` trusts `X-Forwarded-For` whenever `BEHIND_PROXY=true` without checking trusted proxy ranges. Either implement the settings end-to-end or remove/hide them from public settings until they work.

---

## P5 — Operational

- [ ] **Watch-folder claim can overwrite existing done files**
  `src/api/watch_folder.rs:343-358` maps a source path to `done.join(rel)` and uses `std::fs::rename`; on Unix, this can replace an existing file at the done target. A later file with the same relative path can overwrite bytes still being processed. Fail or choose a unique target when the done path already exists.

- [ ] **Startup probes for `ffmpeg`/`ffprobe` can hang indefinitely**
  `src/main.rs:192-204` awaits `ffmpeg -version` and `ffprobe -version` without a timeout, and `src/media/encoder.rs:65-87` does the same for encoder probes. A hung binary or blocked lookup can stall startup. Wrap probes in bounded timeouts.

- [ ] **Cloudflared child can survive THLS shutdown**
  `src/cloudflared.rs:68` spawns the tunnel child without `kill_on_drop`, and the manager has no shutdown path tied to Axum shutdown. A managed tunnel can remain orphaned after THLS exits. Add shutdown signaling and child cleanup.

- [ ] **Runtime cache directory is not ignored or documented as runtime data**
  `src/config.rs:103` defaults `cache_dir` to `./cache/`, and the current worktree has an untracked `cache/` with HLS artifacts. `.gitignore` ignores `/uploads` and `/processing` but not `/cache`, and `AGENTS.md` omits `cache/` from runtime-data warnings. Ignore and document `cache/` as runtime data.

- [ ] **DB auto-merge settings are exposed but no worker is wired**
  `DB_AUTO_MERGE_INTERVAL_MINUTES`, `DB_AUTO_MERGE_FILE_ID`, and `DB_AUTO_MERGE_BOT_INDEX` are loaded in `src/config.rs` and shown in settings, but only `log_startup_warnings` mentions auto-merge being disabled. Implement a simple scheduled import/export flow or remove these settings from the public registry until there is real behavior.

- [ ] **Global body-limit disabling makes every route responsible for its own cap**
  `src/api/mod.rs:166` disables Axum's default body limit for the whole router. Some DB transfer handlers use manual limits, but JSON, multipart, URL ingest, and upload endpoints now depend on each handler remembering to cap body size. Replace the global disable with route-specific limits, or document and test a per-route manual limit for every mutating endpoint.

---

## P6 — New Features

- [x] **Continue watching (server-side)**
  Playback progress is saved to the database per browser client via `/api/playback/progress`. The home page fetches in-progress items from the server and shows a "Continue Watching" section. Server progress takes precedence over localStorage fallback.

- [x] **External metadata cache (TMDB, AniList)**
  `/api/metadata/search` supports TMDB and AniList. Metadata can be linked to individual jobs or series. Poster images, backdrops, overviews, and external IDs are cached in `external_metadata`. Requires `TMDB_API_KEY` for TMDB; AniList is keyless. See `src/api/metadata.rs`.

- [x] **Intro/outro marker detection**
  Chapter-based detection parses FFprobe chapters for intro/opening/ending/credits markers. Chromaprint-based fingerprint comparison now stores separate intro/outro window fingerprints and compares recurring audio across episodes in the same series/season. FFmpeg silence and black-frame scans are used as best-effort boundary hints. Markers stored in `media_markers`, fingerprints in `media_fingerprints`. Detection runs after job completion, non-fatal on failure. API at `/api/jobs/:job_id/markers`.

- [x] **Anime Community comment embed**
  Watch pages for `Anime TV` and `Anime Film` embed the official `https://theanimecommunity.com/embed.js` widget when `TAC_COMMENTS_ENABLED=true` and an AniList or MAL ID is linked. Timestamp clicks seek the Shaka player. Film episodes use `episodeChapterNumber=0` for overview comments.

- [x] **Skip intro/outro button in Shaka player**
  `static/watch.js` loads enabled markers from `/api/jobs/:job_id/markers`, shows a skip overlay while playback is inside an intro/outro marker, and seeks to `marker.end_seconds` on click.

- [x] **Metadata search/link UI on upload/edit**
  The upload metadata table and watch-page edit modal both expose TMDB/AniList search. Upload selections are linked after job creation; edit-modal selections link directly to the existing job.

- [ ] **Remote URL ingest UI**
  `POST /api/ingest/url` exists, but `/upload` has no form for it. Add a small URL input on the upload page that submits a remote URL, shows the existing `downloading` job progress through `/api/status/:job_id`, and then reuses the normal processing/result UI.

- [ ] **Operations panel on settings**
  Add a compact read-only panel on `/settings` that fetches `/health` and `/api/metrics`: DB health, disk free, queue depth, cache usage/hit rate, Telegram bot session stats, FFmpeg encoder, and cloudflared status. Keep it simple and refresh manually or on a slow interval.

- [x] **Continue watching**
  Store last playback position in browser localStorage keyed by job id from `static/watch.js`. Resume near the saved time on next open, ignore positions near the end, and show recent in-progress jobs on the home page.

- [ ] **Auto-play next episode**
  `static/watch.js` already fetches sibling episodes and renders previous/next navigation. When a series episode ends, optionally navigate to the next episode if one exists, with a small cancelable countdown.

- [ ] **Library cleanup tools**
  Add a settings page section for failed/cancelled jobs, old backups, orphaned upload files, and orphaned processing/cache entries. Default to dry-run counts and require an explicit button per cleanup action.

---

## P7 — Code Quality

### Code Quality and Error Handling
- [ ] Audit and remove fragile `.unwrap()` and `.expect()` calls in non-test paths. Handle errors gracefully.
- [ ] **`cargo fmt --check` currently fails**
  Formatting drift is present in `src/api/jobs/download.rs`, `src/api/uploads.rs`, `src/api/watch_folder.rs`, and `src/env_writer.rs`. Run `cargo fmt` after preserving unrelated user changes.

- [ ] **`cargo clippy --all-targets --all-features` emits warnings**
  Clippy exits successfully, but reports warnings for needless conversions/borrows, simple style issues, and a few too-many-arguments functions. Fix the mechanical warnings first; only refactor argument-heavy functions when it clearly reduces current complexity.

- [x] **Agent docs still contain placeholders and are out of sync**
  `AGENTS.md` now has canonical commands and project overview. New Telegram limit / FFmpeg quality / oversized segment rules exist in `AGENTS.md`. Agent docs are synced.

### Current Verification Baseline
- `cargo test` passes: 129 passed, 1 ignored.
- `cargo fmt --check` passes.
- `cargo clippy --all-targets --all-features` completes with warnings (no errors).

---

## Non-Goals (Preserved)

- Do not add application-level authentication.
- Do not introduce multi-tenant isolation.
- Do not require distributed workers or horizontal scaling.
- Do not store source videos permanently on local disk.
- Do not expose Telegram `file_id`, bot tokens, or channel IDs to clients.
- Do not bypass the local DB for segment lookup.
- Do not add new crate dependencies without documented justification.
