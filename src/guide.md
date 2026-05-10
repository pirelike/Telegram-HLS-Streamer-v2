# Source Layout — `src/`

Start here before editing code under `src/`. This file is the top-level map; then read the nearest module guide such as `src/api/guide.md`, `src/db/guide.md`, or `src/api/jobs/guide.md`.

## Top-level modules

| Module | Type | Responsibility |
|---|---|---|
| `main.rs` | file | Entry point: loads config, initializes SQLite, builds `AppState`, starts the Axum server. |
| `config.rs` | file | Effective runtime config loaded from DB settings plus env-var overrides. |
| `settings_registry.rs` | file | Setting key definitions, defaults, validation rules, and public-key allowlist. |
| `telegram.rs` | file | Telegram Bot API client/runtime: upload, download, bot pool management, metrics, retry behavior. |
| `api/` | dir | HTTP layer: router, handlers, upload flow, job processing, segment serving, pages. See `src/api/guide.md`. |
| `db/` | dir | SQLite persistence: schema migrations, query functions, export/import/backup. See `src/db/guide.md`. |
| `media/` | dir | FFprobe/FFmpeg pipeline: analysis, ABR tier selection, encoder probe, HLS processing. See `src/media/guide.md`. |

## Dependency direction

```text
main.rs
  └─ api/ ──► {config, db, media, telegram}
       ├─ jobs/ ──► {db, media, telegram}
       └─ playback/ ──► {db, telegram, media helpers}

db/ ──► {rusqlite, settings_registry}
media/ ──► config
telegram.rs ──► reqwest
```

Keep this direction acyclic. `api` orchestrates application flows; `db`, `media`, and `telegram` stay focused support modules.

## Where new code belongs

| Change | Put it here |
|---|---|
| New route or HTTP handler | `api/mod.rs` for route registration, handler in the smallest matching API module. |
| Job queue/state/processing behavior | `api/jobs/` — read `src/api/jobs/guide.md` first. |
| HLS segment serving/cache/virtual ABR | `api/playback/` — read `src/api/playback/guide.md` first. |
| Playlist text generation | `api/playlists.rs`. |
| Upload protocol changes | `api/uploads.rs`. |
| SQLite schema/query/export behavior | `db/`. Do not write SQL in unrelated modules unless it is a test setup. |
| FFprobe/FFmpeg media processing | `media/`. Do not put HTTP, DB, or Telegram logic here. |
| Telegram Bot API behavior | `telegram.rs`. |
| Runtime setting key/default/validation | `settings_registry.rs` and, if needed, `config.rs`. |

## What does not belong here

- Do not add runtime/generated data under `src/`.
- Do not create a new top-level module unless an existing module clearly cannot own the responsibility.
- Do not introduce framework-style layers (`service`, `repository`, `factory`) for one implementation.
- Do not bypass module ownership: DB code belongs in `db/`, media processing in `media/`, HTTP concerns in `api/`.

## Editing conventions

- Prefer moving or editing the smallest module that owns the behavior.
- Module directories use `mod.rs` as the root that re-exports the public surface.
- Use `pub(crate)` only for items used across top-level modules; use `pub(super)` for sibling submodules.
- Keep tests close to the module they cover; large cross-API integration tests live in `api/tests.rs`.
- If you move files, split modules, or change responsibility boundaries, update the affected `guide.md` in the same change.
