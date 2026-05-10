# THLS Next Roadmap

Baseline: all original 9 phases complete (see `ROADMAP.md`).
This file tracks the next iteration — bug fixes, guards, performance, and
feature work discovered during codebase review.

Priorities: bug > guard > perf > feature.

---

## Phase 10: Reliability Fixes

### 10.1 Oversized segment adaptive re-encode
The single remaining open item from the original ROADMAP (Phase 4).
Segments exceeding `TELEGRAM_MAX_FILE_SIZE` (50 MB) are detected but cause
job failure — no adaptive re-encode happens.

- [ ] Detect oversized `.m4s` segments after FFmpeg processing.
- [ ] Re-encode the individual segment with a lower CRF or shorter GOP.
- [ ] Re-split if needed so the result stays under the limit.
- [ ] Retry upload of the re-encoded segment.
- [ ] Surface segment re-encode as a processing step in job progress.

### 10.2 Disk space pre-check
Processing starts without verifying free disk space. If the disk fills
mid-encode, the job fails with a cryptic FFmpeg error. Upload allocation
already handles this (returns 507); processing should too.

- [ ] Estimate required disk: source size × ABR multiplier + overhead.
- [ ] Check `statvfs` free space before starting processing.
- [ ] Reject the job with a clear error if estimated space > free space.
- [ ] Surface disk pressure in `/health`.

### 10.3 FFmpeg / ffprobe availability check
`/health` checks DB, bots, queue, and cloudflared — but not whether
`ffprobe` and `ffmpeg` are executable. A missing binary only surfaces
when the first job is enqueued.

- [ ] Run `ffprobe -version` and `ffmpeg -version` at startup.
- [ ] Expose `ffprobe_available` and `ffmpeg_available` in `/health`.
- [ ] Reject uploads with a clear error if either binary is missing.
- [ ] Include detected encoder capabilities (VAAPI/NVENC/QSV) in health output.

### 10.4 Better processing failure surfacing
When FFmpeg fails during processing, the error shown to users is often
an opaque "processing failed". The actual FFmpeg stderr is logged but
not propagated.

- [ ] Extract the last N lines of FFmpeg stderr on failure.
- [ ] Surface them in `GET /api/status/<job_id>` error detail.
- [ ] Surface them in the frontend job status display.

---

## Phase 11: Performance Optimizations

### 11.1 Streaming segment responses (not buffer-in-memory)
Segments are currently read fully into memory before the response body
starts. A 50 MB segment × N concurrent viewers = memory pressure.

- [ ] Stream `.m4s`, `.ts`, `.vtt`, `.jpg` responses from the cache file.
- [ ] Use `tokio::fs::File` + `Stream` body for cached segments.
- [ ] Keep the single-flight gate; only the winner opens the file.
- [ ] Fall back to buffered response for Telegram-fetched segments (no local file).

### 11.2 Hardware encoder for virtual ABR
Virtual ABR on-demand transcode hardcodes `libx264`. The encoder
selection from `media::select_encoder` (VAAPI/NVENC/QSV) exists but
is not wired into the virtual transcode path.

- [ ] Pass the selected hardware encoder through to the virtual ABR FFmpeg command.
- [ ] Validate that hardware encoder produces valid fMP4 output.
- [ ] Fall back to CPU if hardware encoder fails for a given resolution.
- [ ] Expose virtual-ABR encoder type in `/api/metrics`.

### 11.3 Segment cache warm-up after job completion
After a job completes, all segments exist in Telegram but not in the
local LRU cache. The first playback is cold (fetches from Telegram).

- [ ] Optionally prefetch the first N segments of each track into cache.
- [ ] Throttle prefetch to avoid saturating Telegram rate limits.
- [ ] Gate behind a config setting: `CACHE_WARMUP_ENABLED` (default off).

---

## Phase 12: Operational Hardening

### 12.1 Segment re-upload recovery
If a Telegram `file_id` becomes invalid (bot removed from channel,
message deleted, token revoked), segment playback returns an error.
No recovery path exists.

- [ ] Detect Telegram 400/403 errors during segment fetch.
- [ ] If the segment file is still in local cache, re-upload it to the same bot.
- [ ] Update the `file_id` in the DB on successful re-upload.
- [ ] If re-upload fails, try the next bot in the pool.
- [ ] Log a warning and return a temporary error to the client if all bots fail.

### 12.2 Prometheus / OpenMetrics endpoint
Only JSON metrics exist at `/api/metrics`. Standard monitoring stacks
expect Prometheus format at `/metrics`.

- [ ] Add `GET /metrics` endpoint returning Prometheus text format.
- [ ] Expose counters: uploads_total, jobs_total, segments_served, cache_hits, cache_misses.
- [ ] Expose gauges: cache_size_bytes, queue_depth, active_jobs, disk_free_bytes.
- [ ] Expose histograms: upload_duration_seconds, processing_duration_seconds.
- [ ] Keep the existing JSON endpoint. Add Prometheus as an additional format.

### 12.3 Orphaned processing directory cleanup
If the process crashes during a job, `processing/<job_id>/` directories
may be left behind. They are only cleaned on successful job completion
or explicit cancellation.

- [ ] On startup, scan `processing/` for orphaned directories.
- [ ] Remove any directory that has no corresponding active job in the DB.
- [ ] Log removed directories for audit.

### 12.4 Cloudflared tunnel process manager
`CLOUDFLARED_ENABLED` logs a warning that it's unimplemented. The tunnel
is referenced in `/health` but there is no process lifecycle.

- [ ] Spawn `cloudflared tunnel run` as a managed child process.
- [ ] Monitor process health and restart on unexpected exit.
- [ ] Expose tunnel status and uptime in `/health`.
- [ ] Wire `CLOUDFLARED_TUNNEL_TOKEN` and `CLOUDFLARED_CONFIG` from config.
- [ ] Log tunnel URL on successful connection.

---

## Phase 13: Feature Completeness

### 13.1 Per-job ABR tier overrides
All jobs get the same ABR ladder from global config. No way to vary
tiers per job (e.g., a 4K film gets 4 tiers, a 480p clip gets 1).

- [ ] Accept optional `tiers` field on `POST /api/upload/init` and watch-folder metadata.
- [ ] Store per-job tier configuration in the `jobs` table.
- [ ] Fall back to global ABR config when no per-job tiers are specified.
- [ ] Validate that specified tiers are within global bounds.

### 13.2 Multi-track audio and subtitle selection in watch UI
The watch player currently auto-selects the first audio and subtitle
track. Users cannot switch languages or commentary tracks.

- [ ] Populate Shaka Player track list from HLS master playlist.
- [ ] Add audio track selector to the watch UI.
- [ ] Add subtitle track selector (including "off" option).
- [ ] Persist user track preference in browser local storage.

### 13.3 URL / remote ingest
Currently the only way to get content into THLS is local file upload
(browser chunked upload or watch folder). No way to submit a URL.

- [ ] Add `POST /api/ingest/url` accepting a URL.
- [ ] Download the remote file to `uploads/` using `reqwest` streaming download.
- [ ] Report download progress via `GET /api/status/<job_id>`.
- [ ] Validate URL scheme (http/https only), size, and content type.
- [ ] Respect `MAX_UPLOAD_SIZE` and disk space checks.

### 13.4 Job reprocessing with original parameters
`POST /api/reprocess/<job_id>` exists but reconstructs from HLS segments
(lossy for re-encoded tiers). No option to re-process from original source.

- [ ] If the original source file is still in `uploads/`, use it directly.
- [ ] If the original is gone, fall back to HLS reconstruction (current behavior).
- [ ] Preserve the original job metadata (title, category, series) on reprocess.

---

## Phase 14: Quality of Life

### 14.1 Job queue visibility
Users uploading a file see no indication of queue position or estimated wait.

- [ ] Expose queue position and depth in `GET /api/status/<job_id>` while queued.
- [ ] Show "position N of M" in the upload complete / processing UI.

### 14.2 Dark mode for web UI
- [ ] Add `prefers-color-scheme` detection to `app.css`.
- [ ] Provide light and dark CSS variables.
- [ ] Add a manual toggle that overrides system preference.
- [ ] Persist preference in local storage.

### 14.3 Keyboard shortcuts for watch UI
- [ ] Space: play/pause.
- [ ] Left/Right arrow: seek ±10s.
- [ ] F: fullscreen.
- [ ] M: mute/unmute.

### 14.4 Upload drag-and-drop
The upload page currently requires clicking the file input. Drag-and-drop
is a standard expectation.

- [ ] Accept file drops on the upload page.
- [ ] Show a drop zone visual affordance.
- [ ] Reuse the existing chunked upload pipeline.

---

## Acceptance Test Checklist (for new phases)

### Phase 10
- [ ] Oversized segment is detected, re-encoded, and uploaded successfully.
- [ ] Re-encoded segment passes Telegram `file_size` integrity check.
- [ ] Disk space pre-check rejects job when free space < estimated need.
- [ ] Disk space pre-check passes job when free space > estimated need.
- [ ] Missing FFmpeg binary returns clear error on upload attempt.
- [ ] FFmpeg stderr is surfaced in job status on processing failure.

### Phase 11
- [ ] Cached segment response streams from file without buffering in memory.
- [ ] Virtual ABR uses hardware encoder when available.
- [ ] Virtual ABR falls back to CPU when hardware encoder fails.

### Phase 12
- [ ] Invalid Telegram file_id triggers re-upload from cache.
- [ ] Re-upload on the same bot succeeds and updates DB file_id.
- [ ] `/metrics` returns valid Prometheus format.
- [ ] Orphaned processing directories are cleaned on startup.
- [ ] Cloudflared tunnel starts, stays alive, and is reflected in `/health`.

### Phase 13
- [ ] Per-job ABR tiers override global config.
- [ ] Invalid per-job tiers return 400.
- [ ] Audio track selector appears in watch UI and switches tracks.
- [ ] Subtitle track selector appears in watch UI and switches tracks.
- [ ] URL ingest downloads file and enqueues for processing.
- [ ] URL ingest rejects non-http(s) URLs.

### Phase 14
- [ ] Queue position is visible during job wait.
- [ ] Dark mode toggle works and persists across page loads.
- [ ] Keyboard shortcuts function in watch UI.
- [ ] Drag-and-drop upload initiates chunked upload.

---

## Non-Goals (Preserved)

- Do not add application-level authentication.
- Do not introduce multi-tenant isolation.
- Do not require distributed workers or horizontal scaling.
- Do not store source videos permanently on local disk.
- Do not expose Telegram `file_id`, bot tokens, or channel IDs to clients.
- Do not bypass the local DB for segment lookup.
- Do not add new crate dependencies without documented justification.
