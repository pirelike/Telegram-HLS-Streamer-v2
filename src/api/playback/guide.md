# Playback Module — `src/api/playback/`

Serves HLS segment-like assets on demand with an in-memory LRU cache, single-flight deduplication, prefetching, Telegram fetches, multipart reconstruction, and virtual ABR transcoding.

## Files

| File | Responsibility | ~Lines |
|---|---|---|
| `mod.rs` | Module root, `handle_segment`, shared response/content helpers, `SegmentLookup` helper impl. | 220 |
| `cache.rs` | `SegmentCache`, LRU bookkeeping, cache snapshots, in-flight single-flight coordination. | 220 |
| `real.rs` | Persisted segment serving entry points, DB lookup, played-segment cleanup, and source/cache recovery helpers. | 389 |
| `real_fetch.rs` | Real segment single-flight fetch core, multipart reconstruction, Telegram fetch timeout handling. | 241 |
| `real_recovery.rs` | Stale Telegram file-id recovery and re-upload decision helpers. | 112 |
| `real_prefetch.rs` | Real segment prefetch and cache warm-up selection/execution. | 218 |
| `virtual_.rs` | Virtual ABR: source fetch, init generation, FFmpeg transcode, fMP4 box parsing, virtual prefetch. | 637 |
| `tests.rs` | Cache, content-type, virtual-key, WebVTT, and fMP4 parsing tests. | 480 |

## Public API

- `SegmentCache` — re-exported from `cache.rs`, used by `AppState` and `main.rs`.
- `handle_segment` — `/segment/:job_id/*key` route handler.
- `serve_real_segment` — used by `playlists::handle_thumbnail` for thumbnail serving.

## Serving flow

1. `handle_segment` validates `job_id` and segment key using `playlists::sanitize_segment_uri()`.
2. Virtual keys (`virtual/...`) dispatch to `virtual_.rs`; all other keys dispatch to `real.rs`.
3. Real path: check cache → check DB for multipart rows → reconstruct or single-flight fetch from Telegram via `real_fetch.rs` → cache/respond → prefetch next real segments via `real_prefetch.rs`.
4. Virtual path: check cache → single-flight → fetch tier-0 source/init → transcode via FFmpeg when needed → cache/respond → prefetch next virtual segments.

## Dependency direction

```text
api/mod.rs ──► playback::handle_segment
playlists.rs ──► playback::serve_real_segment (thumbnail)
playback/real*.rs ──► {db, telegram}
playback/virtual_.rs ──► {db, telegram, media helpers, ffmpeg}
playback/cache.rs ──► std/tokio sync only
```

`playback/` should not depend on `jobs/` or upload internals.

## What belongs here

- Segment/cache response behavior for `/segment/...` and thumbnail-backed segment serving.
- Cache metrics/snapshots and single-flight fetch coordination.
- Telegram-backed persisted segment fetching and multipart reconstruction.
- On-demand virtual ABR segment generation and fMP4 parsing helpers.

## What does not belong here

- Playlist text generation: keep it in `api/playlists.rs`.
- Job processing/upload decisions: keep them in `api/jobs/`.
- General FFmpeg processing pipeline: keep eager processing in `src/media/`; only on-demand virtual segment transcode lives here.
- DB schema/query expansion: add reusable queries to `src/db/`.

## Editing conventions

- Keep cache internals in `cache.rs`; do not spread LRU bookkeeping across serving modules.
- Keep real persisted segment logic in `real.rs`; keep virtual/transcode logic in `virtual_.rs`.
- Maintain single-flight behavior when adding new fetch paths so concurrent identical requests do not duplicate Telegram/FFmpeg work.
- Preserve content-type and `Cache-Control` behavior when changing responses.
- Update tests for key sanitization, content type, cache behavior, and fMP4/WebVTT helpers when touching those areas.

## Common modifications

| I want to... | Go to |
|---|---|
| Change cache budget/eviction/snapshots | `cache.rs`. |
| Change Telegram segment fetch or multipart reconstruction | `real_fetch.rs`. |
| Change virtual ABR transcode or fMP4 parsing | `virtual_.rs`. |
| Add a served extension/content type | `mod.rs` content-type helper and tests. |
| Change playlist URIs for segments | `api/playlists.rs`, not this module. |
