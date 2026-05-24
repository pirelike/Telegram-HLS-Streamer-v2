# Jobs Module — `src/api/jobs/`

Full job lifecycle: enqueue → analyze → process → upload to Telegram → save to SQLite. Also owns the jobs REST API for listing, reading, patching, deleting, downloading, reprocessing, and cancelling jobs.

## Files

| File | Responsibility | ~Lines |
|---|---|---|
| `mod.rs` | Module root and re-exports used by `api/mod.rs`, uploads, tests. | 21 |
| `types.rs` | `JobMetadata`, `JobStatus`, `JobState`, `JobRequest`, `JobsQuery`. | 87 |
| `handlers.rs` | HTTP handlers (`handle_*`) and `queue_metrics`. | 539 |
| `json.rs` | JSON serialization helpers, queue position, field/category validators. | 161 |
| `download.rs` | Original-source download, DB-complete lookup, segment reconstruction, response streaming. | 540 |
| `processing.rs` | Enqueue, dispatcher, `process_job` orchestrator, and processing re-exports. | 554 |
| `processing_upload.rs` | Upload file collection/splitting, Telegram upload fanout, and DB row construction. | 446 |
| `processing_lifecycle.rs` | Cancellation, completion/error transitions, timeout cleanup, webhooks, and startup recovery. | 376 |
| `processing_markers.rs` | Intro/outro marker detection preparation, marker save, and metadata auto-fetch trigger. | 208 |
| `tests.rs` | Focused jobs tests for DB row building, upload-file collection, integrity handling. | 454 |

## Public API re-exported through `mod.rs`

- `enqueue_job(state, filename, path, metadata, delete) -> Result<String>` — add a job to the in-memory queue.
- `start_background_tasks(state, receiver)` — spawn dispatcher and job watcher tasks.
- `JobRequest`, `JobState`, `JobMetadata` — shared API/job types.
- Handler functions used by `api/mod.rs` route registration.

## Request and processing flow

1. Upload/watch/reprocess code calls `enqueue_job()`.
2. `enqueue_job()` inserts a queued `JobState` into `state.jobs` and sends a `JobRequest`.
3. `job_dispatcher` acquires the concurrency semaphore and calls `process_job()`.
4. `process_job()` runs `media::analyze_media()` → `media::process_media()` → `processing_upload::upload_outputs()` → `build_db_rows()` → `db::save_job()`.
5. Background watchers handle timeout, cancellation, webhook notification, terminal cleanup, and path cleanup.

## Dependency direction

```text
api/mod.rs ──► jobs handlers
uploads.rs/watch_folder.rs ──► jobs::enqueue_job
jobs/handlers.rs ──► {json, download, processing, db}
jobs/processing.rs ──► {db, media, telegram, config}
jobs/download.rs ──► {db, telegram}
```

`jobs/` may orchestrate DB/media/Telegram, but those lower-level modules should not depend on `jobs/`.

## What belongs here

- Job REST handlers and response shaping.
- In-memory job state, progress, cancellation, queue metrics, timeout cleanup.
- Processing orchestration and conversion from media outputs/uploads into DB rows.
- Original-source reconstruction from persisted segments.

## What does not belong here

- Reusable SQL/schema logic: add it to `src/db/`.
- FFmpeg command details: add them to `src/media/`.
- Telegram retry/client internals: add them to `src/telegram.rs`.
- Upload protocol chunk handling: keep it in `api/uploads.rs` until final enqueue.
- Playlist/segment-serving logic: use `api/playlists.rs` or `api/playback/`.

## Editing conventions

- Add new job statuses in `types.rs`, then audit `is_terminal`, queue metrics, JSON, cancellation, timeout, and webhook paths.
- Add new metadata fields in `JobMetadata`, request validation, DB models/queries, JSON responses, and tests together.
- Keep handlers mostly as extract/validate/respond code; move processing side effects to `processing.rs` or reconstruction to `download.rs`.
- Do not duplicate DB row construction logic outside `build_db_rows()`.
- Keep webhook sends on terminal transitions only.

## Common modifications

| I want to... | Go to |
|---|---|
| Change list/get/patch/delete/cancel API behavior | `handlers.rs` plus `json.rs` if response shape changes. |
| Change processing steps or progress text | `processing.rs`. |
| Change Telegram upload file collection/splitting | `processing_upload.rs`. |
| Change original download/reconstruction | `download.rs`. |
| Add job metadata | `types.rs`, `handlers.rs`, `json.rs`, `processing.rs`, and `src/db/`. |
