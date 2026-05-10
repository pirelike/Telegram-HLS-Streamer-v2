use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use super::{api_error, valid_job_id, AppState};
use crate::config::Config;
use crate::db::{self, JobRow, SegmentRow, TrackRow};
use crate::media;

const M3U8_CONTENT_TYPE: &str = "application/vnd.apple.mpegurl";

pub(super) async fn handle_master(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let cfg = state.config.read().await.clone();
    let conn = state.db.lock().await;
    let job = match db::get_job(&conn, &job_id) {
        Ok(Some(job)) => job,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "job not found"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };
    let tracks = match db::get_job_tracks(&conn, &job_id, None) {
        Ok(t) => t,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };
    drop(conn);

    let body = build_master_playlist(&cfg, &job, &tracks);
    playlist_response(body)
}

pub(super) async fn handle_legacy_video(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let cfg = state.config.read().await.clone();
    let conn = state.db.lock().await;
    let job = match db::get_job(&conn, &job_id) {
        Ok(Some(j)) => j,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "job not found"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };
    let segs = match db::get_segments_for_prefix(&conn, &job_id, "video_0") {
        Ok(s) => s,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };
    drop(conn);
    if segs.is_empty() {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no video segments");
    }
    playlist_response(emit_media_playlist(&cfg, &job, &segs, false))
}

pub(super) async fn handle_media_playlist(
    State(state): State<Arc<AppState>>,
    Path((job_id, playlist)): Path<(String, String)>,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    let stem = match playlist.strip_suffix(".m3u8") {
        Some(s) => s,
        None => return api_error(StatusCode::NOT_FOUND, "not_found", "not a playlist"),
    };
    let cfg = state.config.read().await.clone();

    let (prefix, kind) = if let Some(h) = stem.strip_prefix("video_virtual_") {
        let height: i64 = match h.parse() {
            Ok(v) => v,
            Err(_) => return api_error(StatusCode::BAD_REQUEST, "bad_height", "bad height"),
        };
        return handle_virtual_playlist(state, cfg, &job_id, height).await;
    } else if let Some(idx) = stem.strip_prefix("video_") {
        if idx.parse::<u32>().is_err() {
            return api_error(StatusCode::NOT_FOUND, "not_found", "bad video index");
        }
        (format!("video_{idx}"), PlaylistKind::Video)
    } else if let Some(idx) = stem.strip_prefix("audio_") {
        if idx.parse::<u32>().is_err() {
            return api_error(StatusCode::NOT_FOUND, "not_found", "bad audio index");
        }
        (format!("audio_{idx}"), PlaylistKind::Audio)
    } else if let Some(idx) = stem.strip_prefix("sub_") {
        if idx.parse::<u32>().is_err() {
            return api_error(StatusCode::NOT_FOUND, "not_found", "bad sub index");
        }
        (format!("sub_{idx}"), PlaylistKind::Subtitle)
    } else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "unknown playlist");
    };

    let conn = state.db.lock().await;
    let job = match db::get_job(&conn, &job_id) {
        Ok(Some(j)) => j,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "job not found"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };
    let segs = match db::get_segments_for_prefix(&conn, &job_id, &prefix) {
        Ok(s) => s,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };
    drop(conn);
    if segs.is_empty() {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no segments");
    }
    let body = match kind {
        PlaylistKind::Subtitle => emit_subtitle_playlist(&cfg, &job, &segs),
        _ => emit_media_playlist(&cfg, &job, &segs, false),
    };
    playlist_response(body)
}

async fn handle_virtual_playlist(
    state: Arc<AppState>,
    cfg: Arc<Config>,
    job_id: &str,
    target_height: i64,
) -> Response {
    if !cfg.virtual_abr_tiers || cfg.abr_enabled {
        return api_error(
            StatusCode::NOT_FOUND,
            "virtual_disabled",
            "virtual ABR not enabled",
        );
    }
    let tiers = media::parse_tiers(&cfg.abr_tiers);
    if !tiers.iter().any(|(h, _)| *h == target_height) {
        return api_error(
            StatusCode::NOT_FOUND,
            "unknown_tier",
            "virtual tier not configured",
        );
    }
    let conn = state.db.lock().await;
    let job = match db::get_job(&conn, job_id) {
        Ok(Some(j)) => j,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "job not found"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };
    let segs = match db::get_segments_for_prefix(&conn, job_id, "video_0") {
        Ok(s) => s,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };
    drop(conn);
    if segs.is_empty() {
        return api_error(StatusCode::NOT_FOUND, "not_found", "no source segments");
    }
    playlist_response(emit_virtual_playlist(&cfg, &job, target_height, &segs))
}

pub(super) async fn handle_thumbnail(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Response {
    if !valid_job_id(&job_id) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_job_id", "invalid job id");
    }
    super::playback::serve_real_segment(state, job_id, "thumbnail/thumbnail.jpg".to_string()).await
}

enum PlaylistKind {
    Video,
    Audio,
    Subtitle,
}

fn playlist_response(body: String) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, M3U8_CONTENT_TYPE),
            (header::CACHE_CONTROL, "no-store"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        body,
    )
        .into_response()
}

// --- playlist generation -----------------------------------------------------

fn build_master_playlist(cfg: &Config, job: &JobRow, tracks: &[TrackRow]) -> String {
    let mut audio: Vec<&TrackRow> = tracks.iter().filter(|t| t.track_type == "audio").collect();
    let mut subs: Vec<&TrackRow> = tracks
        .iter()
        .filter(|t| t.track_type == "subtitle")
        .collect();
    let mut videos: Vec<&TrackRow> = tracks.iter().filter(|t| t.track_type == "video").collect();
    audio.sort_by_key(|t| t.track_index);
    subs.sort_by_key(|t| t.track_index);
    videos.sort_by_key(|t| t.track_index);

    let mut out = String::new();
    out.push_str("#EXTM3U\n#EXT-X-VERSION:4\n");

    for (i, track) in audio.iter().enumerate() {
        let default = if i == 0 { "YES" } else { "NO" };
        let lang = q(track.language.as_str());
        let name = q(if track.title.is_empty() {
            track.language.as_str()
        } else {
            track.title.as_str()
        });
        let uri = q(&format!("audio_{}.m3u8", track.track_index));
        out.push_str(&format!(
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"{name}\",LANGUAGE=\"{lang}\",DEFAULT={default},AUTOSELECT={default},CHANNELS=\"{ch}\",URI=\"{uri}\"\n",
            ch = track.channels
        ));
    }
    for (i, track) in subs.iter().enumerate() {
        let default = if i == 0 { "YES" } else { "NO" };
        let lang = q(track.language.as_str());
        let name = q(if track.title.is_empty() {
            track.language.as_str()
        } else {
            track.title.as_str()
        });
        let uri = q(&format!("sub_{}.m3u8", track.track_index));
        out.push_str(&format!(
            "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"{name}\",LANGUAGE=\"{lang}\",DEFAULT={default},AUTOSELECT={default},URI=\"{uri}\"\n",
        ));
    }

    let audio_attr = if audio.is_empty() {
        String::new()
    } else {
        ",AUDIO=\"audio\"".into()
    };
    let subs_attr = if subs.is_empty() {
        String::new()
    } else {
        ",SUBTITLES=\"subs\"".into()
    };

    for track in &videos {
        let bw = bandwidth_for(track, job, cfg);
        let codecs = format!(
            "{},{}",
            video_codec_string(&track.codec, track.height),
            audio_codec_string()
        );
        let res = format!("{}x{}", track.width.max(0), track.height.max(0));
        out.push_str(&format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={bw},RESOLUTION={res},CODECS=\"{codecs}\"{audio_attr}{subs_attr}\n",
        ));
        let uri = match sanitize_segment_uri(&format!("video_{}.m3u8", track.track_index)) {
            Some(u) => u,
            None => continue,
        };
        out.push_str(&uri);
        out.push('\n');
    }

    if cfg.virtual_abr_tiers && !cfg.abr_enabled && !videos.is_empty() {
        let source = videos.iter().max_by_key(|t| t.height).unwrap();
        let mut seen_heights = std::collections::HashSet::new();
        let mut tiers = media::parse_tiers_in_order(&cfg.abr_tiers)
            .into_iter()
            .filter(|(h, _)| *h < source.height)
            .filter(|(h, _)| seen_heights.insert(*h))
            .collect::<Vec<_>>();
        tiers.sort_by(|a, b| b.0.cmp(&a.0));
        for (height, bitrate) in tiers {
            let bw = parse_bitrate_str(&bitrate)
                .unwrap_or(2_000_000)
                .clamp(32_000, 50_000_000);
            let width = (height as f64 * 16.0 / 9.0 / 2.0).floor() as i64 * 2;
            let codecs = format!(
                "{},{}",
                video_codec_string("h264", height),
                audio_codec_string()
            );
            out.push_str(&format!(
                "#EXT-X-STREAM-INF:BANDWIDTH={bw},RESOLUTION={width}x{height},CODECS=\"{codecs}\"{audio_attr}{subs_attr}\n",
            ));
            // Relative URI — HLS players resolve it against the master playlist URL.
            // Avoids parsing X-Forwarded headers / TRUSTED_PROXY_CIDRS this phase.
            let uri = format!("video_virtual_{height}.m3u8");
            if let Some(u) = sanitize_segment_uri(&uri) {
                out.push_str(&u);
                out.push('\n');
            }
        }
    }

    out
}

fn emit_media_playlist(
    cfg: &Config,
    _job: &JobRow,
    segments: &[SegmentRow],
    _is_subtitle: bool,
) -> String {
    let init = segments
        .iter()
        .find(|s| s.segment_key.ends_with("/init.mp4"));
    let media_segs: Vec<&SegmentRow> = segments
        .iter()
        .filter(|s| !s.segment_key.ends_with("/init.mp4"))
        .collect();
    let fallback_dur = (cfg.hls_segment_duration as f64).max(0.001);
    let target = media_segs
        .iter()
        .filter_map(|s| s.duration)
        .filter(|d| *d > 0.0)
        .fold(0.0_f64, f64::max);
    let target_int = if target > 0.0 {
        target.ceil() as u64
    } else {
        cfg.hls_segment_duration as u64
    };

    let mut out = String::new();
    out.push_str(&format!(
        "#EXTM3U\n#EXT-X-VERSION:{ver}\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:{target_int}\n#EXT-X-MEDIA-SEQUENCE:0\n",
        ver = if init.is_some() { 7 } else { 4 },
    ));
    if let Some(init) = init {
        if let Some(uri) = sanitize_segment_uri(&init.segment_key) {
            out.push_str(&format!(
                "#EXT-X-MAP:URI=\"/segment/{}/{}\"\n",
                _job.job_id, uri
            ));
        } else {
            tracing::warn!(segment = %init.segment_key, "init segment key rejected by sanitiser");
        }
    }
    for seg in media_segs {
        let dur = seg.duration.filter(|d| *d > 0.0).unwrap_or(fallback_dur);
        let uri = match sanitize_segment_uri(&seg.segment_key) {
            Some(u) => u,
            None => {
                tracing::warn!(segment = %seg.segment_key, "segment key rejected by sanitiser");
                continue;
            }
        };
        out.push_str(&format!(
            "#EXTINF:{dur:.6},\n/segment/{}/{}\n",
            _job.job_id, uri
        ));
    }
    out.push_str("#EXT-X-ENDLIST\n");
    out
}

fn emit_subtitle_playlist(cfg: &Config, job: &JobRow, segments: &[SegmentRow]) -> String {
    let dur = job.duration.max(cfg.hls_segment_duration as f64).max(0.001);
    let target_int = dur.ceil() as u64;
    let mut out = String::new();
    out.push_str(&format!(
        "#EXTM3U\n#EXT-X-VERSION:4\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:{target_int}\n#EXT-X-MEDIA-SEQUENCE:0\n",
    ));
    for seg in segments {
        let uri = match sanitize_segment_uri(&seg.segment_key) {
            Some(u) => u,
            None => continue,
        };
        out.push_str(&format!(
            "#EXTINF:{dur:.6},\n/segment/{}/{}\n",
            job.job_id, uri
        ));
    }
    out.push_str("#EXT-X-ENDLIST\n");
    out
}

fn emit_virtual_playlist(
    cfg: &Config,
    job: &JobRow,
    target_height: i64,
    source_segs: &[SegmentRow],
) -> String {
    let init_present = source_segs
        .iter()
        .any(|s| s.segment_key.ends_with("/init.mp4"));
    let media_segs: Vec<&SegmentRow> = source_segs
        .iter()
        .filter(|s| !s.segment_key.ends_with("/init.mp4"))
        .collect();
    let fallback_dur = (cfg.hls_segment_duration as f64).max(0.001);
    let target = media_segs
        .iter()
        .filter_map(|s| s.duration)
        .filter(|d| *d > 0.0)
        .fold(0.0_f64, f64::max);
    let target_int = if target > 0.0 {
        target.ceil() as u64
    } else {
        cfg.hls_segment_duration as u64
    };
    let mut out = String::new();
    out.push_str(&format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:{target_int}\n#EXT-X-MEDIA-SEQUENCE:0\n",
    ));
    if init_present {
        out.push_str(&format!(
            "#EXT-X-MAP:URI=\"/segment/{}/virtual_{target_height}p/init.mp4\"\n",
            job.job_id
        ));
    }
    for seg in media_segs {
        let filename = match seg.segment_key.split_once('/') {
            Some((_, f)) => f,
            None => continue,
        };
        let dur = seg.duration.filter(|d| *d > 0.0).unwrap_or(fallback_dur);
        let virt_key = format!("virtual_{target_height}p/{filename}");
        let uri = match sanitize_segment_uri(&virt_key) {
            Some(u) => u,
            None => continue,
        };
        out.push_str(&format!(
            "#EXTINF:{dur:.6},\n/segment/{}/{}\n",
            job.job_id, uri
        ));
    }
    out.push_str("#EXT-X-ENDLIST\n");
    out
}

// --- helpers -----------------------------------------------------------------

pub(super) fn sanitize_segment_uri(key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    if key.contains('\r') || key.contains('\n') {
        return None;
    }
    for component in key.split('/') {
        if component.is_empty() || component.starts_with('#') {
            return None;
        }
        if component.contains(' ') {
            return None;
        }
    }
    let mut out = String::with_capacity(key.len());
    for &b in key.as_bytes() {
        let safe = b.is_ascii_alphanumeric() || matches!(b, b'_' | b'/' | b'-' | b'.' | b'~');
        if safe {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    Some(out)
}

fn video_codec_string(codec: &str, height: i64) -> &'static str {
    let c = codec.to_ascii_lowercase();
    if c == "hevc" || c == "h265" || c == "x265" {
        if height >= 2160 {
            "hvc1.1.6.L153.B0"
        } else if height >= 1080 {
            "hvc1.1.6.L120.B0"
        } else if height >= 720 {
            "hvc1.1.6.L93.B0"
        } else {
            "hvc1.1.6.L90.B0"
        }
    } else {
        // h264 / unknown → safe avc1 default
        if height >= 2160 {
            "avc1.640033"
        } else if height >= 1080 {
            "avc1.640028"
        } else if height >= 720 {
            "avc1.64001f"
        } else {
            "avc1.64001e"
        }
    }
}

fn audio_codec_string() -> &'static str {
    "mp4a.40.2"
}

fn bandwidth_for(track: &TrackRow, job: &JobRow, cfg: &Config) -> u64 {
    if let Some(bps) = parse_bitrate_str(&track.bitrate) {
        return bps.clamp(32_000, 50_000_000);
    }
    let dur = job.duration.max(cfg.hls_segment_duration as f64).max(1.0);
    let bps = ((job.file_size.max(0) as f64) * 8.0 / dur) as u64;
    bps.clamp(32_000, 50_000_000)
}

fn parse_bitrate_str(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("copy") {
        return None;
    }
    let (num_part, mult) = match s.as_bytes().last().copied() {
        Some(b'k') | Some(b'K') => (&s[..s.len() - 1], 1_000_u64),
        Some(b'm') | Some(b'M') => (&s[..s.len() - 1], 1_000_000_u64),
        Some(b'g') | Some(b'G') => (&s[..s.len() - 1], 1_000_000_000_u64),
        _ => (s, 1_u64),
    };
    let n: f64 = num_part.parse().ok()?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    Some((n * mult as f64) as u64)
}

fn q(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\r' | '\n' | ',' => out.push(' '),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
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
}
