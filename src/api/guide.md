# API Module Guide — `src/api/`

HTTP layer for THLS. This folder owns Axum routes, request/response shaping, browser pages, upload/watch flows, job orchestration, playlists, and segment playback.

## Module map

| Module | Type | Responsibility | ~Lines |
|---|---|---|---|
| `mod.rs` | file | Router definition, `AppState`, health/metrics endpoints, shared API helpers. | 496 |
| `auth.rs` | file | Session-cookie/Basic-auth middleware, current-user extraction, and credential helpers. | 175 |
| `bots_settings.rs` | file | Settings CRUD and bot-management endpoints. | 508 |
| `db_transfer.rs` | file | DB export/import/backup/load HTTP handlers and request helpers. | 414 |
| `db_transfer_replace.rs` | file | Import staging, merge, live replacement, and pool drain helpers. | 142 |
| `db_transfer_sync.rs` | file | Automatic DB sync/bootstrap snapshot orchestration and Telegram upload. | 395 |
| `discovery.rs` | file | User-aware Next Up plus recently-added feed endpoints. | 100 |
| `favorites.rs` | file | Per-user favorites toggle/list endpoints. | 76 |
| `frontend.rs` | file | Server-rendered page handlers, routing/resolution helpers, shell/chrome, slug helpers. | 385 |
| `frontend_bodies.rs` | file | Per-page HTML body builders used by `frontend.rs`. | 483 |
| `ingest.rs` | file | URL ingest validation/download and enqueue flow. | 592 |
| `jobs/` | dir | Job lifecycle and jobs REST API. Read `jobs/guide.md`. | ~2838 |
| `markers.rs` | file | Media marker endpoint: intro/outro for Shaka skip UI. | 40 |
| `metadata.rs` | file | External metadata cache/search/link API (TMDB, AniList). | 748 |
| `playback/` | dir | Segment serving, cache, real/virtual segment fetches. Read `playback/guide.md`. | ~2731 |
| `playlists.rs` | file | HLS master/media/subtitle/virtual playlist generation and thumbnail route. | 588 |
| `playlists/` | dir | Playlist unit tests. | ~226 |
| `preferences.rs` | file | Per-user playback preference get/patch endpoints. | 105 |
| `progress.rs` | file | Browser/user-scoped playback progress persistence API. | 267 |
| `ratings.rs` | file | Per-user thumbs up/down rating endpoints. | 95 |
| `tests.rs` | file | Cross-API integration test module root. | 7 |
| `tests/` | dir | Split integration test harness and feature clusters. | ~1759 |
| `uploads.rs` | file | Chunked resumable upload protocol and pending-upload cleanup. | 835 |
| `users.rs` | file | Login/logout/me plus admin user CRUD endpoints. | 261 |
| `watchlist.rs` | file | Per-user watchlist toggle/list endpoints. | 76 |
| `watch_folder.rs` | file | Watch-folder settings, scanning, and auto-enqueue flow. | 680 |

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
| Change login/session/user account APIs | `auth.rs`, `users.rs`, and `src/db/queries_users.rs`. |
| Add/change per-user library actions | `favorites.rs`, `watchlist.rs`, `ratings.rs`, `preferences.rs`, and the matching DB query modules. |
| Add external metadata provider | `metadata.rs` and `src/db/`. |
| Change playback progress persistence | `progress.rs` and `src/db/`. |
| Add media markers (intro/outro) | `markers.rs`, `src/media/markers.rs`, `src/db/`. |

## Conventions

- Keep `mod.rs` focused on state, router wiring, health, and metrics; do not add feature logic there.
- Use `api_error()` for JSON API errors where practical.
- Validate path IDs before DB lookup (`valid_job_id`).
- Keep route handlers thin when logic naturally belongs in `jobs/`, `playback/`, `db/`, or `media/`.
- Update this guide and a child guide when moving responsibility between API modules.
