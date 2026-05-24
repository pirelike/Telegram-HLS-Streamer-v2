# API Module Guide — `src/api/`

HTTP layer for THLS. This folder owns Axum routes, request/response shaping, browser pages, upload/watch flows, job orchestration, playlists, and segment playback.

## Module map

| Module | Type | Responsibility | ~Lines |
|---|---|---|---|
| `mod.rs` | file | Router definition, `AppState`, health/metrics endpoints, shared API helpers. | 460 |
| `auth.rs` | file | Lightweight auth/session helpers. | 75 |
| `bots_settings.rs` | file | Settings CRUD and bot-management endpoints. | 483 |
| `db_transfer.rs` | file | DB export/import/backup/load HTTP handlers and request helpers. | 378 |
| `db_transfer_replace.rs` | file | Import staging, merge, live replacement, and pool drain helpers. | 146 |
| `db_transfer_sync.rs` | file | Automatic DB sync/bootstrap snapshot orchestration and Telegram upload. | 372 |
| `frontend.rs` | file | Server-rendered page handlers, routing/resolution helpers, shell/chrome, slug helpers. | 372 |
| `frontend_bodies.rs` | file | Per-page HTML body builders used by `frontend.rs`. | 421 |
| `ingest.rs` | file | URL ingest validation/download and enqueue flow. | 587 |
| `jobs/` | dir | Job lifecycle and jobs REST API. Read `jobs/guide.md`. | ~3386 |
| `markers.rs` | file | Media marker endpoint: intro/outro for Shaka skip UI. | 40 |
| `metadata.rs` | file | External metadata cache/search/link API (TMDB, AniList). | 717 |
| `playback/` | dir | Segment serving, cache, real/virtual segment fetches. Read `playback/guide.md`. | ~2517 |
| `playlists.rs` | file | HLS master/media/subtitle/virtual playlist generation and thumbnail route. | 585 |
| `playlists/` | dir | Playlist unit tests. | ~222 |
| `progress.rs` | file | Browser-client playback progress persistence API. | 244 |
| `tests.rs` | file | Cross-API integration test module root. | 7 |
| `tests/` | dir | Split integration test harness and feature clusters. | ~1667 |
| `uploads.rs` | file | Chunked resumable upload protocol and pending-upload cleanup. | 727 |
| `watch_folder.rs` | file | Watch-folder settings, scanning, and auto-enqueue flow. | 640 |

## Key public items from `mod.rs`

- `AppState` — shared application state struct.
- `router()` — builds the Axum router with all routes.
- `SegmentCache` — re-exported from `playback`, used by `main.rs` and `AppState`.
- `start_background_tasks()` — re-exported from `jobs`.
- `load_watch_settings(conn, legacy_path)` — re-exported from `watch_folder`; loads versioned DB-backed watch settings and migrates/tolerates legacy JSON.
- `api_error()` — JSON error-response helper for sibling modules.
- `valid_job_id()` — route parameter validator for job/segment paths.

## Dependency direction

```text
api/mod.rs ──► sibling handler modules
handlers ──► {db, media, telegram, config}
jobs/ ──► {db, media, telegram}
playback/ ──► {db, telegram, media helpers, playlists::sanitize_segment_uri}
playlists.rs ──► {db, media, playback for thumbnail serving}
```

The API layer may orchestrate other modules. Lower-level modules (`db`, `media`, `telegram`) should not call back into `api`.

## What belongs here

- Route registration, extractors, response JSON/HTML, status codes, and API validation.
- Application orchestration that combines DB, Telegram, media, config, and filesystem operations.
- API integration tests that exercise routes and shared `AppState` behavior.

## What does not belong here

- Raw schema migration or reusable SQL logic: put it in `src/db/`.
- FFmpeg/FFprobe command construction for processing: put it in `src/media/`.
- Telegram retry/upload/download internals: put them in `src/telegram.rs`.
- Large UI frameworks or template engines; existing frontend pages use simple server-rendered strings and static assets.

## Common modifications

| I want to... | Go to |
|---|---|
| Add a new API endpoint | Register the route in `mod.rs`, then put the handler in the smallest matching module. |
| Add a route for a new feature area | Prefer an existing file; create a new submodule only when one file would become a mixed-responsibility module. |
| Change upload validation/finalization | `uploads.rs`; job creation still goes through `jobs::enqueue_job()`. |
| Change segment caching | `playback/cache.rs`. |
| Modify real segment fetching | `playback/real.rs`. |
| Modify virtual ABR transcoding | `playback/virtual_.rs`. |
| Add/change HLS playlist output | `playlists.rs`, then route in `mod.rs` if a new endpoint is needed. |
| Change job metadata fields | `jobs/types.rs`, `jobs/handlers.rs`, and the DB model/query layer. |
| Change HTML layout or page markup | `frontend.rs` and static assets if needed. |
| Add external metadata provider | `metadata.rs` and `src/db/`. |
| Change playback progress persistence | `progress.rs` and `src/db/`. |
| Add media markers (intro/outro) | `markers.rs`, `src/media/markers.rs`, `src/db/`. |

## Conventions

- Keep `mod.rs` focused on state, router wiring, health, and metrics; do not add feature logic there.
- Use `api_error()` for JSON API errors where practical.
- Validate path IDs before DB lookup (`valid_job_id`).
- Keep route handlers thin when logic naturally belongs in `jobs/`, `playback/`, `db/`, or `media/`.
- Update this guide and a child guide when moving responsibility between API modules.
