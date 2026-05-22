# THLS

THLS is a single-process Telegram-backed HLS media streamer written in Rust. It accepts video uploads, processes them with FFmpeg, stores HLS outputs in Telegram, keeps metadata in SQLite, and serves a small web UI plus HTTP APIs for browsing, playback, settings, bot management, watch-folder ingest, and database transfer.

The project is intentionally simple operationally: one Rust server process, one SQLite database, local temporary working directories, and Telegram as durable segment storage.

## Requirements

- Rust stable toolchain. The repo pins `stable` with `rustfmt` and `clippy` in `rust-toolchain.toml`.
- FFmpeg and ffprobe on `PATH`.
- A Telegram bot token and channel ID for durable media storage.
- Linux or another environment where the Rust toolchain, FFmpeg, and network access to Telegram are available.

SQLite is provided through bundled `rusqlite`; no separate SQLite server is required.

## Quick Start

```sh
cp .env.example .env
```

Edit `.env` and set at least:

```sh
TELEGRAM_BOT_TOKEN_1=your_bot_token_here
TELEGRAM_CHANNEL_ID_1=-1001234567890
```

Run the server:

```sh
cargo run
```

Open:

```text
http://localhost:5050/
```

By default the server binds to `LOCAL_HOST=0.0.0.0` and `LOCAL_PORT=5050`.

## Configuration

Configuration is loaded from defaults, environment variables, and persisted runtime settings in `streamer.db`. The UI and `/api/settings` can update public settings at runtime, but bind host/port still only matter at process startup.

Common `.env` values:

| Env var | Default | Purpose |
| --- | --- | --- |
| `LOCAL_HOST` | `0.0.0.0` | Server bind address. |
| `LOCAL_PORT` | `5050` | Server bind port. |
| `TELEGRAM_BOT_TOKEN_1` | unset | First Telegram bot token. |
| `TELEGRAM_CHANNEL_ID_1` | unset | Channel used by the first bot. |
| `TELEGRAM_MAX_FILE_SIZE` | `20971520` | Per-file Telegram upload ceiling. Raise if Telegram increases Bot API limits. |
| `SEGMENT_TARGET_SIZE` | `15728640` | Preferred HLS segment target. User-configurable; adjust if upload ceiling changes. |
| `MAX_UPLOAD_SIZE` | `107374182400` | Max accepted client upload size. |
| `UPLOAD_CHUNK_SIZE` | `10485760` | Browser/client upload chunk size. |
| `MAX_CONCURRENT_JOBS` | `1` | Number of queue workers. |
| `ABR_ENABLED` | `true` | Produce eager ABR tiers. |
| `ENABLE_COPY_MODE` | `true` | Use source passthrough tier when possible. |
| `VIRTUAL_ABR_TIERS` | `false` | Transcode lower tiers on demand. |
| `RUST_LOG` | `info` | Logging filter. |

For the full live setting registry, see `src/settings_registry.rs`. For the full intended contract, see `REBUILD.md`.

## Runtime Data

The server creates and uses these local paths:

| Path | Purpose |
| --- | --- |
| `streamer.db` | SQLite system of record for jobs, tracks, segments, settings, bots, and schema migrations. |
| `streamer.db-shm`, `streamer.db-wal` | SQLite WAL sidecar files. |
| `uploads/` | Temporary chunked upload storage. |
| `processing/` | Temporary FFmpeg output and reconstruction workspace. |
| `watch_settings.json` | Watch-folder settings, created when watch settings are saved. |
| `streamer.db.backup_*` | Backups created by database load/replace flows. |

Do not treat `uploads/`, `processing/`, `target/`, `streamer.db*`, or `TEST_FILE.mkv` as source files. They are local runtime or test artifacts.

The database is critical. Telegram stores the media bytes, but `streamer.db` stores the mapping between jobs, segment keys, and Telegram `file_id`s. Losing the database can make uploaded media unreachable even if Telegram still has the files.

## Main Workflows

### Browser Upload

Use `/upload` in the web UI. The client initializes an upload, sends chunks, finalizes metadata, and polls job status until processing/upload completes.

### Watch Folder

Use `/settings` or `/api/watch-settings` to configure watch-folder ingest. The server polls for stable files with allowed video extensions and queues them using the same processing pipeline as browser uploads.

### Playback

Browse pages list jobs through `/api/jobs`. The watch page at `/watch/<job_id>` loads metadata and plays HLS using the generated master playlist at `/hls/<job_id>/master.m3u8`.

Segments are served through `/segment/<job_id>/<segment_key>`. The server retrieves them from Telegram, caches them in-process, and can prefetch nearby segments.

### Database Transfer

The API supports JSON export/import plus direct SQLite database load. Use these flows for backup, restore, and moving a library between instances.

### Bot Management

Environment bots are loaded from numbered pairs:

```text
TELEGRAM_BOT_TOKEN_1 + TELEGRAM_CHANNEL_ID_1
TELEGRAM_BOT_TOKEN_2 + TELEGRAM_CHANNEL_ID_2
...
```

DB-backed bots can also be added and removed through `/settings` or the bot APIs.

## HTTP Surface

Page routes:

| Route | Purpose |
| --- | --- |
| `/` | Browse home. |
| `/films` | Film browse view. |
| `/series` and `/series/*path` | Series browse views. |
| `/anime-films` | Anime film browse view. |
| `/anime-tv` and `/anime-tv/*path` | Anime TV browse views. |
| `/upload` | Upload page. |
| `/settings` | Settings, bots, watch-folder, and DB tools. |
| `/watch/:job_id` | Player page. |
| `/health` | Health JSON. |

Core API routes:

| Route | Purpose |
| --- | --- |
| `GET /api/jobs` | List and filter jobs. |
| `GET/PATCH/DELETE /api/jobs/:job_id` | Read, edit, or delete a job. |
| `GET /api/jobs/:job_id/download-original` | Reconstruct and download original media when available. |
| `POST /api/jobs/:job_id/reprocess` | Queue a reprocess job. |
| `POST /api/upload/init` | Create a chunked upload. |
| `POST /api/upload/chunk` | Upload one chunk. |
| `POST /api/upload/finalize` | Finalize upload and queue processing. |
| `GET /api/upload/status/:upload_id` | Inspect chunked upload status. |
| `GET /api/status/:job_id` | Poll processing status. |
| `POST /api/cancel/:job_id` | Cancel a queued or running job. |
| `GET/POST /api/settings` | Read or update settings. |
| `POST /api/settings/reset` | Reset settings. |
| `GET /api/bots` | List configured bots. |
| `POST /api/bots/health` | Probe bot health. |
| `POST /api/bots/add` | Add a DB-backed bot. |
| `DELETE /api/bots/:bot_id` | Delete a DB-backed bot. |
| `GET/POST /api/watch-settings` | Read or update watch-folder settings. |
| `POST /api/db/export` | Export DB content as portable JSON. |
| `POST /api/db/backup` | Create a DB backup. |
| `POST /api/db/import` | Import portable DB JSON. |
| `POST /api/database/load` | Replace the live SQLite DB from an uploaded DB file. |
| `GET /api/metrics` | Queue, cache, and Telegram metrics. |

HLS and media routes:

| Route | Purpose |
| --- | --- |
| `GET /hls/:job_id/master.m3u8` | Master playlist. |
| `GET /hls/:job_id/video.m3u8` | Legacy video playlist. |
| `GET /hls/:job_id/:playlist` | Media playlist. |
| `GET /segment/:job_id/*key` | Segment proxy/cache. |
| `GET /thumbnail/:job_id` | Thumbnail image. |

## Development

Source layout:

| Path | Purpose |
| --- | --- |
| `src/main.rs` | Process startup, config load, SQLite init, shared state, server bind. |
| `src/config.rs` | Effective runtime config and bot pool loading. |
| `src/settings_registry.rs` | Public settings metadata, defaults, and validation. |
| `src/db/` | SQLite schema, migrations, queries, and DB transfer helpers. |
| `src/media/` | ffprobe analysis, ABR tier selection, encoder probing, and FFmpeg processing. |
| `src/telegram.rs` | Telegram upload/download runtime. |
| `src/api/` | Axum router, page handlers, APIs, playback, playlists, uploads, jobs, watch-folder, DB transfer. See nested `guide.md` files for split modules. |
| `static/` | Browser UI CSS and JavaScript. |
| `scripts/upload_and_wait.py` | Manual upload/process/playback smoke test helper. |

Useful commands:

```sh
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features
```

Manual end-to-end check with Telegram credentials configured:

```sh
python3 scripts/upload_and_wait.py TEST_FILE.mkv --timeout 7200 --start-timeout 180 --request-timeout 180
```

The helper starts `cargo run` if no server is already reachable at `http://127.0.0.1:5050`, uploads the file, waits for terminal job status, and verifies the master playlist.

## Troubleshooting

- Missing `ffmpeg` or `ffprobe`: install FFmpeg and make sure both binaries are on `PATH`.
- `/health` reports degraded with `bots.configured = 0`: set `TELEGRAM_BOT_TOKEN_1` and `TELEGRAM_CHANNEL_ID_1`, or add a bot through the settings UI.
- Port already in use: change `LOCAL_PORT` in `.env` or stop the process using the port.
- Upload rejected early: check `MAX_UPLOAD_SIZE`, `UPLOAD_CHUNK_SIZE`, disk space, and upload rate-limit settings.
- Processing fails: inspect server logs with `RUST_LOG=debug`, confirm FFmpeg can read the input file, and check hardware acceleration settings.
- Playback returns missing segments: confirm the job completed, the DB has not been replaced with stale data, and the configured Telegram bots can download stored files.
- Database load/import problems: keep the generated `streamer.db.backup_*` file until playback and `/health` are verified.

## Related Docs

- `REBUILD.md`: detailed behavior and API contract.
- `ROADMAP.md`: implementation status and acceptance checklist.
- `src/api/guide.md`: concise guide to the API modules.
- `plans/`: historical implementation plans.
- `AGENTS.md`, `CODEX.md`, `CLAUDE.md`: agent instructions for working in this repo.
