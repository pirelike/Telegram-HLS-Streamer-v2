use super::*;
use crate::db::{JobRow, SegmentRow, TrackRow};

fn job(duration: f64, file_size: i64, height: i64) -> JobRow {
    JobRow {
        job_id: "abc".into(),
        filename: "movie.mkv".into(),
        duration,
        file_size,
        video_codec: "h264".into(),
        video_width: 1920,
        video_height: height,
        status: "complete".into(),
        created_at: "2026-01-01 00:00:00".into(),
        media_type: "Film".into(),
        series_name: String::new(),
        has_thumbnail: false,
        is_series: false,
        season_number: None,
        episode_number: None,
        part_number: None,
        error: None,
        source_path: None,
        episode_title: None,
        source_bitrate: 0,
    }
}

fn vtrack(idx: i64, h: i64, bitrate: &str) -> TrackRow {
    TrackRow {
        id: idx,
        job_id: "abc".into(),
        track_type: "video".into(),
        track_index: idx,
        codec: "h264".into(),
        language: "und".into(),
        title: String::new(),
        channels: 0,
        width: (h * 16 / 9 / 2) * 2,
        height: h,
        bitrate: bitrate.into(),
        original_stream_index: 0,
    }
}

fn seg(key: &str, duration: Option<f64>) -> SegmentRow {
    SegmentRow {
        id: 0,
        job_id: "abc".into(),
        segment_key: key.into(),
        file_id: "fid".into(),
        bot_index: 0,
        file_size: 1000,
        duration,
        is_split: false,
        encryption_nonce: None,
    }
}

#[test]
fn sanitiser_rejects_unsafe_keys() {
    assert!(sanitize_segment_uri("video_0/init.mp4").is_some());
    assert!(sanitize_segment_uri("a\nb").is_none());
    assert!(sanitize_segment_uri("a\rb").is_none());
    assert!(sanitize_segment_uri("video_0/#EXT-X-EVIL").is_none());
    assert!(sanitize_segment_uri("video_0/has space.m4s").is_none());
    assert!(sanitize_segment_uri("").is_none());
    let out = sanitize_segment_uri("foo/é.m4s").unwrap();
    assert!(out.contains('%'));
    // Path traversal with .. and . components
    assert!(
        sanitize_segment_uri("../etc/passwd").is_none(),
        "parent traversal"
    );
    assert!(sanitize_segment_uri("video_0/..").is_none(), "trailing ..");
    assert!(
        sanitize_segment_uri("video_0/./init.mp4").is_none(),
        "dot component"
    );
    assert!(
        sanitize_segment_uri("../../etc/shadow").is_none(),
        "deep traversal"
    );
}

#[test]
fn bandwidth_zero_duration_does_not_divide_by_zero() {
    let cfg = Config::default();
    let mut t = vtrack(0, 1080, "copy");
    t.bitrate.clear();
    let j = job(0.0, 100_000_000, 1080);
    let bw = bandwidth_for(&t, &j, &cfg);
    assert!(bw >= 32_000 && bw <= 50_000_000);
}

#[test]
fn explicit_track_bitrate_is_used() {
    let cfg = Config::default();
    let t = vtrack(0, 1080, "5M");
    let j = job(120.0, 100_000_000, 1080);
    assert_eq!(bandwidth_for(&t, &j, &cfg), 5_000_000);
}

#[test]
fn media_playlist_with_init_uses_version_7() {
    let cfg = Config::default();
    let j = job(20.0, 1_000_000, 1080);
    let segs = vec![
        seg("video_0/init.mp4", None),
        seg("video_0/video_0001.m4s", Some(4.0)),
        seg("video_0/video_0002.m4s", Some(4.0)),
    ];
    let body = emit_media_playlist(&cfg, &j, &segs, false);
    assert!(body.contains("#EXT-X-VERSION:7"));
    assert!(body.contains("#EXT-X-MAP:URI=\"/segment/abc/video_0/init.mp4\""));
    assert!(body.contains("/segment/abc/video_0/video_0001.m4s"));
    assert!(!body.contains("EXTINF:0,"));
}

#[test]
fn legacy_ts_playlist_uses_version_4() {
    let cfg = Config::default();
    let j = job(20.0, 1_000_000, 1080);
    let segs = vec![
        seg("video_0/video_0001.ts", Some(4.0)),
        seg("video_0/video_0002.ts", Some(4.0)),
    ];
    let body = emit_media_playlist(&cfg, &j, &segs, false);
    assert!(body.contains("#EXT-X-VERSION:4"));
    assert!(!body.contains("#EXT-X-MAP"));
}

#[test]
fn subtitle_playlist_never_emits_extinf_zero() {
    let cfg = Config::default();
    let j = job(0.0, 100, 1080);
    let segs = vec![seg("sub_0/subtitles.vtt", None)];
    let body = emit_subtitle_playlist(&cfg, &j, &segs);
    assert!(!body.contains("EXTINF:0"));
    assert!(body.contains("EXTINF:"));
}

#[test]
fn master_playlist_lists_video_audio_subs() {
    let cfg = Config::default();
    let j = job(120.0, 1_000_000, 1080);
    let tracks = vec![
        vtrack(0, 1080, "10M"),
        TrackRow {
            id: 1,
            job_id: "abc".into(),
            track_type: "audio".into(),
            track_index: 0,
            codec: "aac".into(),
            language: "eng".into(),
            title: "English".into(),
            channels: 2,
            width: 0,
            height: 0,
            bitrate: String::new(),
            original_stream_index: 1,
        },
        TrackRow {
            id: 2,
            job_id: "abc".into(),
            track_type: "subtitle".into(),
            track_index: 0,
            codec: "webvtt".into(),
            language: "eng".into(),
            title: String::new(),
            channels: 0,
            width: 0,
            height: 0,
            bitrate: String::new(),
            original_stream_index: 2,
        },
    ];
    let body = build_master_playlist(&cfg, &j, &tracks);
    assert!(body.contains("#EXT-X-MEDIA:TYPE=AUDIO"));
    assert!(body.contains("#EXT-X-MEDIA:TYPE=SUBTITLES"));
    assert!(body.contains("#EXT-X-STREAM-INF:BANDWIDTH=10000000"));
    assert!(body.contains("AUDIO=\"audio\""));
    assert!(body.contains("SUBTITLES=\"subs\""));
    assert!(body.contains("video_0.m3u8"));
}

#[test]
fn master_includes_virtual_streams_when_enabled() {
    let mut cfg = Config::default();
    cfg.abr_enabled = false;
    cfg.virtual_abr_tiers = true;
    cfg.abr_tiers = "720:5M,480:2M".into();
    let j = job(120.0, 1_000_000, 1080);
    let tracks = vec![vtrack(0, 1080, "10M")];
    let body = build_master_playlist(&cfg, &j, &tracks);
    assert!(body.contains("video_virtual_720.m3u8"));
    assert!(body.contains("video_virtual_480.m3u8"));
}

#[test]
fn virtual_master_deduplicates_repeated_heights() {
    let mut cfg = Config::default();
    cfg.abr_enabled = false;
    cfg.virtual_abr_tiers = true;
    cfg.abr_tiers = "720:6M,720:3M,480:2M,480:800k".into();
    let j = job(120.0, 1_000_000, 1080);
    let tracks = vec![vtrack(0, 1080, "copy")];
    let body = build_master_playlist(&cfg, &j, &tracks);
    assert_eq!(body.matches("video_virtual_720.m3u8").count(), 1);
    assert_eq!(body.matches("video_virtual_480.m3u8").count(), 1);
    assert!(body.contains("BANDWIDTH=6000000"));
    assert!(body.contains("BANDWIDTH=2000000"));
}

#[test]
fn parse_bitrate_handles_all_suffixes() {
    assert_eq!(parse_bitrate_str("5M"), Some(5_000_000));
    assert_eq!(parse_bitrate_str("1200k"), Some(1_200_000));
    assert_eq!(parse_bitrate_str("100"), Some(100));
    assert_eq!(parse_bitrate_str("copy"), None);
    assert_eq!(parse_bitrate_str(""), None);
}
