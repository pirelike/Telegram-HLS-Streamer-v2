# Media Module — `src/media/`

FFprobe/FFmpeg-based media analysis and HLS processing. This module turns an input file into local HLS outputs; it does not upload, persist DB rows, or serve HTTP.

## Files

| File | Responsibility | ~Lines |
|---|---|---|
| `mod.rs` | Module root, public re-exports, media tests. | 314 |
| `models.rs` | `MediaAnalysis`, stream structs, `ProcessingResult`, playlist descriptors, tier/encoder structs. | 98 |
| `analysis.rs` | FFprobe invocation and JSON output parsing. | 108 |
| `tiers.rs` | ABR tier parsing and source-aware tier selection. | 113 |
| `encoder.rs` | Encoder selection/probing, hardware-device args, video filter construction. | 113 |
| `process.rs` | Core FFmpeg pipeline for video/audio/subtitle/thumbnail outputs and segment-duration collection. | 769 |

## Public API surface

- `analyze_media(path) -> Result<MediaAnalysis>` — inspect an input file with FFprobe.
- `process_media(analysis, job_id, output_dir, config, cancel_flag, abr_override) -> Result<ProcessingResult>` — generate local HLS outputs.
- `select_video_tiers(...)` / `select_video_tiers_with(...)` — choose eager ABR tiers.
- `select_encoder(config) -> SelectedEncoder` — detect best available encoder.
- `video_filter(encoder, scale) -> Option<String>` — build an FFmpeg `-vf` string.
- `tier_bitrate(tiers_config, height) -> Option<String>` — look up bitrate for a virtual tier.
- `output_audio_channels(audio) -> i64` — preserve/downmix channel counts for output.

## Dependency direction

```text
api/jobs/processing.rs ──► media::{analyze_media, process_media}
api/playback/virtual_.rs ──► media::{tier_bitrate, video_filter, SelectedEncoder}
media/ ──► config
```

`media/` must not depend on `api`, `db`, or `telegram`.

## What belongs here

- FFprobe parsing and media stream metadata structs.
- FFmpeg command construction for local HLS output generation.
- ABR tier parsing/selection and encoder probe/filter helpers.
- Media-output tests that create or inspect local files.

## What does not belong here

- Uploading generated files to Telegram.
- Saving DB rows or deciding persisted segment keys.
- HTTP responses, Axum handlers, or route validation.
- Job queue/progress state transitions beyond respecting the cancellation flag.

## Editing conventions

- Keep command-building helpers close to the FFmpeg path they support.
- Keep cancellation-aware processing using `run_ffmpeg_cancellable`.
- Let errors be loud except for intentionally best-effort outputs such as thumbnails.
- Avoid adding new dependencies; prefer FFmpeg/FFprobe plus existing stdlib/tokio utilities.
- If a config string parser changes, update the small parser/unit tests in `mod.rs`.

## Common modifications

| I want to... | Go to |
|---|---|
| Parse new FFprobe fields | `analysis.rs` and `models.rs`. |
| Change ABR tier selection | `tiers.rs`. |
| Add/change encoder probing or filter args | `encoder.rs`. |
| Change video/audio/subtitle output commands | `process.rs`. |
| Change thumbnail behavior | `process.rs`. |
