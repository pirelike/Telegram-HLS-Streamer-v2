# Database Module — `src/db/`

SQLite persistence layer. All reusable DB schema, query, migration, import/export, and backup logic belongs here.

## Files

| File | Responsibility | ~Lines |
|---|---|---|
| `mod.rs` | Module root, re-exports, `init_db()`, schema revision check, DB tests. | 447 |
| `models.rs` | Public row/input/result structs, row mappers, SQL select constants, metadata normalization helpers. | 367 |
| `migrations.rs` | Schema migrations, bootstrap from legacy DBs, table/column/index helpers. | 616 |
| `queries.rs` | Public query functions for jobs, tracks, segments, settings, bots, library listings. | 655 |
| `transfer.rs` | Export/import/merge plus live DB replacement and backup helpers. | 176 |

## Public API surface

- `init_db(path) -> Result<Connection>` — create/open/migrate a database.
- `current_schema_revision(conn) -> Result<i64>` — inspect applied migration revision.
- `LATEST_SCHEMA_REVISION` — newest supported schema revision.
- `NewJob`/`NewTrack`/`NewSegment`/`NewSegmentPart` — insert/update inputs.
- `JobRow`/`TrackRow`/`SegmentRow`/group rows — read-model outputs.
- Query and transfer functions are re-exported through `crate::db::*`.
- `kv_internal` stores private runtime keys such as `_last_bot_index` and versioned watch settings; public runtime settings stay in `settings` with `value_type`.

## Dependency direction

```text
api/* ──► db::*
config.rs ──► db settings queries
db/ ──► {rusqlite, settings_registry}
```

`db/` must not depend on `api`, `media`, or `telegram`.

## What belongs here

- SQL schema migrations and migration detection/repair helpers.
- Reusable DB query functions used by API/config code.
- DB row/input/result structs that represent persisted data.
- Import/export/backup/replacement logic for SQLite files.

## What does not belong here

- HTTP status codes, Axum extractors, JSON response shaping, or route validation.
- Job-processing decisions such as progress updates or Telegram bot assignment.
- FFmpeg, filesystem media processing, or Telegram API calls.
- One-off SQL embedded in API handlers when a reusable query function is appropriate.

## Adding or changing schema

1. Add a new migration function in `migrations.rs` and append it to `MIGRATIONS`.
2. Increment `LATEST_SCHEMA_REVISION` in `mod.rs`.
3. Keep migrations idempotent where possible (`IF NOT EXISTS`, helper checks).
4. Update models/query functions for new columns.
5. Add/update DB tests in `mod.rs` tests.
6. Update this guide if responsibilities or file ownership change.

## Query conventions

- Functions take `&Connection` or `&mut Connection` as the first parameter; callers own locking.
- Keep transactions local to operations that must be atomic.
- Use typed input/output structs rather than passing loosely shaped maps.
- New `New*` structs are insert/update inputs; `*Row` structs are read outputs.
- Prefer adding a small query helper here over duplicating SQL in API code.
