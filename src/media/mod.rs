mod analysis;
mod encoder;
mod markers;
mod models;
mod process;
mod tiers;

pub use analysis::*;
pub use encoder::*;
pub use markers::*;
pub use models::*;
pub use process::*;
pub use tiers::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn parses_ffprobe_streams() {
        let value = serde_json::json!({
            "format": { "duration": "12.5", "size": "1000" },
            "streams": [
                { "index": 0, "codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080, "bit_rate": "5000000", "tags": { "language": "eng", "title": "Main" } },
                { "index": 1, "codec_type": "audio", "codec_name": "aac", "channels": 6, "sample_rate": "48000", "channel_layout": "5.1" },
                { "index": 2, "codec_type": "subtitle", "codec_name": "subrip", "tags": { "language": "hun" } }
            ]
        });
        let analysis = analysis::analysis_from_ffprobe(std::path::Path::new("/missing"), &value)
            .await
            .unwrap();
        assert_eq!(analysis.duration, 12.5);
        assert_eq!(analysis.video_streams[0].height, 1080);
        assert_eq!(analysis.audio_streams[0].channels, 6);
        assert_eq!(analysis.subtitle_streams[0].language, "hun");
    }

    #[test]
    fn copy_mode_abr_matrix_selects_expected_tiers() {
        let mut cfg = crate::config::Config::default();
        cfg.abr_tiers = "1080:10M,720:5M,480:2M".into();
        cfg.enable_copy_mode = true;
        cfg.abr_enabled = true;
        let tiers = select_video_tiers(&cfg, "h264", 1080);
        assert_eq!(
            tiers.iter().map(|t| t.height).collect::<Vec<_>>(),
            [1080, 720, 480]
        );
        assert!(tiers[0].copy);

        let tiers = select_video_tiers(&cfg, "vp9", 1080);
        assert_eq!(
            tiers.iter().map(|t| t.height).collect::<Vec<_>>(),
            [1080, 1080, 720, 480]
        );
        assert!(!tiers[0].copy);

        cfg.abr_enabled = false;
        let tiers = select_video_tiers(&cfg, "h264", 1080);
        assert_eq!(tiers.len(), 1);
        assert!(tiers[0].copy);

        cfg.enable_copy_mode = false;
        let tiers = select_video_tiers(&cfg, "h264", 1080);
        assert_eq!(tiers.len(), 1);
        assert!(!tiers[0].copy);
    }

    #[test]
    fn virtual_abr_generates_only_tier_zero() {
        let mut cfg = crate::config::Config::default();
        cfg.abr_enabled = true;
        cfg.virtual_abr_tiers = true;
        let tiers = select_video_tiers(&cfg, "h264", 1080);
        assert_eq!(tiers.len(), 1);
    }

    #[test]
    fn subtitle_codec_filter_skips_bitmap() {
        assert!(process::is_text_subtitle("subrip"));
        assert!(process::is_text_subtitle("mov_text"));
        assert!(!process::is_text_subtitle("hdmv_pgs_subtitle"));
        assert!(!process::is_text_subtitle("dvd_subtitle"));
    }

    #[test]
    fn hls_playlist_parser_maps_extinf_to_segment_keys() {
        let playlist = r#"#EXTM3U
#EXT-X-VERSION:7
#EXT-X-MAP:URI="init.mp4"
#EXTINF:4.006000,
video_0000.m4s
#EXTINF:3.500000,
nested/video_0001.m4s?token=ignored
#EXT-X-ENDLIST
"#;
        let durations = process::parse_hls_segment_durations("video_0", playlist);
        assert_eq!(durations.get("video_0/video_0000.m4s"), Some(&4.006));
        assert_eq!(durations.get("video_0/video_0001.m4s"), Some(&3.5));
        assert!(!durations.contains_key("video_0/init.mp4"));
    }

    #[tokio::test]
    async fn generated_media_processes_to_fmp4_video_and_audio() {
        let base = std::env::temp_dir().join(format!(
            "thls_media_test_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::create_dir_all(&base).await.unwrap();
        let source = base.join("sample.mp4");
        let status = tokio::process::Command::new("ffmpeg")
            .arg("-y")
            .arg("-v")
            .arg("error")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("testsrc2=size=128x72:rate=10:duration=3")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("sine=frequency=1000:duration=3")
            .arg("-c:v")
            .arg("libx264")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-c:a")
            .arg("aac")
            .arg(&source)
            .status()
            .await
            .unwrap();
        assert!(status.success());

        let analysis = analyze_media(&source).await.unwrap();
        assert_eq!(analysis.video_streams[0].codec_name, "h264");
        assert_eq!(analysis.audio_streams.len(), 1);

        let mut cfg = crate::config::Config::default();
        cfg.enable_hw_accel = false;
        cfg.abr_enabled = false;
        // Drive the per-job formula toward ~1 s segments by making the byte ceiling tiny.
        cfg.segment_target_size = 256 * 1024;
        let output = base.join("out");
        let cancel = Arc::new(AtomicBool::new(false));
        let result = process_media(&analysis, "job1", &output, &cfg, &cancel, None)
            .await
            .unwrap();

        assert_eq!(result.video_playlists.len(), 1);
        assert!(output.join("video_0/init.mp4").exists());
        assert!(output.join("video_0/playlist.m3u8").exists());
        assert!(has_extension(&output.join("video_0"), "m4s").await);
        assert!(output.join("audio_0/init.mp4").exists());
        assert!(output.join("audio_0/playlist.m3u8").exists());
        assert!(has_extension(&output.join("audio_0"), "m4s").await);
        assert!(!has_extension(&output.join("audio_0"), "ts").await);
        assert!(result
            .segment_durations
            .keys()
            .any(|k| k.starts_with("video_0/")));
        let video_duration = duration_sum(&result.segment_durations, "video_0/");
        let audio_duration = duration_sum(&result.segment_durations, "audio_0/");
        assert!(video_duration > 0.0, "video duration sum was zero");
        assert!(
            (video_duration - analysis.duration).abs() < 0.5,
            "video duration sum {video_duration} differed from source {}",
            analysis.duration
        );
        assert!(
            (audio_duration - analysis.duration).abs() < 0.5,
            "audio duration sum {audio_duration} differed from source {}",
            analysis.duration
        );
        let _ = tokio::fs::remove_dir_all(base).await;
    }

    fn duration_sum(durations: &std::collections::HashMap<String, f64>, prefix: &str) -> f64 {
        durations
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .map(|(_, duration)| *duration)
            .sum()
    }

    async fn has_extension(dir: &std::path::Path, ext: &str) -> bool {
        let mut entries = tokio::fs::read_dir(dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            if entry.path().extension().and_then(|e| e.to_str()) == Some(ext) {
                return true;
            }
        }
        false
    }

    #[tokio::test]
    async fn copy_alignment_check_flags_empty_or_long_segments() {
        let base = std::env::temp_dir().join(format!(
            "thls_copy_alignment_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::create_dir_all(&base).await.unwrap();
        // 4 s target — alignment check tolerates up to 1.75× = 7 s.
        let target_secs: u32 = 4;

        tokio::fs::write(
            base.join("playlist.m3u8"),
            "#EXTM3U\n#EXTINF:4.0,\nvideo_0000.ts\n#EXT-X-ENDLIST\n",
        )
        .await
        .unwrap();
        assert!(!process::copied_segments_need_reencode(&base, target_secs)
            .await
            .unwrap());

        tokio::fs::write(
            base.join("playlist.m3u8"),
            "#EXTM3U\n#EXTINF:9.0,\nvideo_0000.ts\n#EXT-X-ENDLIST\n",
        )
        .await
        .unwrap();
        assert!(process::copied_segments_need_reencode(&base, target_secs)
            .await
            .unwrap());

        tokio::fs::write(base.join("playlist.m3u8"), "#EXTM3U\n")
            .await
            .unwrap();
        assert!(process::copied_segments_need_reencode(&base, target_secs)
            .await
            .unwrap());
        let _ = tokio::fs::remove_dir_all(base).await;
    }

    #[test]
    fn audio_output_channels_preserves_51_downmixes_71() {
        // 5.1 source → preserved as 5.1 (6 channels)
        let audio_51 = AudioStream {
            index: 0,
            codec_name: "aac".into(),
            channels: 6,
            sample_rate: "48000".into(),
            bit_rate: "384k".into(),
            channel_layout: "5.1".into(),
            language: "eng".into(),
            title: String::new(),
        };
        assert_eq!(output_audio_channels(&audio_51), 6);

        // 7.1 source → downmixed to stereo (2 channels)
        let audio_71 = AudioStream {
            index: 1,
            codec_name: "aac".into(),
            channels: 8,
            sample_rate: "48000".into(),
            bit_rate: "512k".into(),
            channel_layout: "7.1".into(),
            language: "eng".into(),
            title: String::new(),
        };
        assert_eq!(output_audio_channels(&audio_71), 2);

        // Mono → preserved as mono
        let audio_mono = AudioStream {
            index: 2,
            codec_name: "aac".into(),
            channels: 1,
            sample_rate: "48000".into(),
            bit_rate: "64k".into(),
            channel_layout: "mono".into(),
            language: "eng".into(),
            title: String::new(),
        };
        assert_eq!(output_audio_channels(&audio_mono), 1);

        // Stereo → preserved as stereo
        let audio_stereo = AudioStream {
            index: 3,
            codec_name: "aac".into(),
            channels: 2,
            sample_rate: "48000".into(),
            bit_rate: "128k".into(),
            channel_layout: "stereo".into(),
            language: "eng".into(),
            title: String::new(),
        };
        assert_eq!(output_audio_channels(&audio_stereo), 2);

        // 3.1 → preserved
        let audio_31 = AudioStream {
            index: 4,
            codec_name: "aac".into(),
            channels: 4,
            sample_rate: "48000".into(),
            bit_rate: "256k".into(),
            channel_layout: "3.1".into(),
            language: "eng".into(),
            title: String::new(),
        };
        assert_eq!(output_audio_channels(&audio_31), 4);
    }

    #[test]
    fn max_bitrate_for_segment_targets_95_percent_of_limit() {
        // 20 MB default matches cfg.telegram_max_file_size; raise if Telegram changes limits.
        let max_file_size = 20 * 1024 * 1024u64;
        let duration = 10.0f64;
        let bps = process::max_bitrate_for_segment(max_file_size, duration);
        let max_bytes_for_bitrate = (bps as f64 / 8.0) * duration;
        assert!(
            max_bytes_for_bitrate <= (max_file_size as f64 * 0.95),
            "max_bytes {} should be within 95% of limit {}",
            max_bytes_for_bitrate,
            max_file_size as f64 * 0.95
        );
    }

    #[test]
    fn oversized_segment_long_duration_triggers_split() {
        // 20 MB default matches cfg.telegram_max_file_size; raise if Telegram changes limits.
        let max_file_size = 20 * 1024 * 1024u64;
        let bps = process::max_bitrate_for_segment(max_file_size, 5000.0);
        assert!(
            process::repair_needs_split(bps),
            "very long segment at 20MB limit should need split, got {} bps",
            bps
        );
    }

    #[test]
    fn repair_needs_split_at_32kbps_floor() {
        assert!(process::repair_needs_split(31999));
        assert!(process::repair_needs_split(32000));
        assert!(process::repair_needs_split(32001));
        assert!(!process::repair_needs_split(33000));
        assert!(!process::repair_needs_split(1_000_000));
    }

    #[test]
    fn fmp4_input_arg_uses_concat_for_m4s() {
        use std::path::PathBuf;
        let m4s = PathBuf::from("/tmp/video_0/video_0001.m4s");
        let arg = process::fmp4_input_arg(&m4s);
        assert!(
            arg.starts_with("concat:"),
            "expected concat format for m4s, got: {arg}"
        );
        assert!(
            arg.contains("init.mp4"),
            "should include init.mp4, got: {arg}"
        );
        assert!(
            arg.contains("video_0001.m4s"),
            "should include the segment, got: {arg}"
        );

        let ts = PathBuf::from("/tmp/video_0/video_0001.ts");
        let arg = process::fmp4_input_arg(&ts);
        assert_eq!(arg, "/tmp/video_0/video_0001.ts", "ts should be plain path");
    }

    #[test]
    fn double_bitrate_doubles_preserving_unit() {
        assert_eq!(process::double_bitrate("5000k"), "10000k");
        assert_eq!(process::double_bitrate("2M"), "4M");
        assert_eq!(process::double_bitrate("1500K"), "3000K");
    }

    #[test]
    fn bitrate_bits_parses_units() {
        assert_eq!(process::bitrate_bits("500k"), 500_000);
        assert_eq!(process::bitrate_bits("1M"), 1_000_000);
        assert_eq!(process::bitrate_bits("1G"), 1_000_000_000);
        assert_eq!(process::bitrate_bits("100k"), 100_000);
        assert_eq!(process::bitrate_bits("128000"), 128_000);
        assert_eq!(process::bitrate_bits("128kbps"), 128_000);
        assert_eq!(process::bitrate_bits("2Mbps"), 2_000_000);
    }

    #[test]
    fn scaled_width_computes_even_width() {
        assert_eq!(process::scaled_width(1920, 1080, 720), 1280);
        assert_eq!(process::scaled_width(1920, 1080, 480), 852);
        assert_eq!(process::scaled_width(1920, 1080, 360), 640);
        assert_eq!(process::scaled_width(0, 0, 720), 0);
        assert_eq!(process::scaled_width(1920, 1080, 1080), 1920);
    }

    #[tokio::test]
    async fn oversized_m4s_detection_identifies_files_over_limit() {
        let base = std::env::temp_dir().join(format!(
            "thls_oversized_detect_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::create_dir_all(&base).await.unwrap();

        let ok_size = 1024u64;
        let max_size = 2048u64;
        tokio::fs::write(base.join("video_0000.m4s"), vec![0u8; ok_size as usize])
            .await
            .unwrap();
        tokio::fs::write(
            base.join("video_0001.m4s"),
            vec![0u8; (max_size + 1) as usize],
        )
        .await
        .unwrap();
        tokio::fs::write(
            base.join("video_0002.ts"),
            vec![0u8; (max_size + 100) as usize],
        )
        .await
        .unwrap();

        let mut oversized = Vec::new();
        let mut entries = tokio::fs::read_dir(&base).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("m4s") {
                continue;
            }
            let size = entry.metadata().await.unwrap().len();
            if size > max_size {
                oversized.push(path);
            }
        }
        assert_eq!(oversized.len(), 1);
        assert!(oversized[0].ends_with("video_0001.m4s"));

        let _ = tokio::fs::remove_dir_all(base).await;
    }
}
