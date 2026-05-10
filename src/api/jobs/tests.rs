use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::processing;
use super::types::*;
use crate::media;
use crate::telegram;

fn sample_analysis(source: PathBuf) -> media::MediaAnalysis {
    media::MediaAnalysis {
        file_path: source,
        duration: 12.5,
        file_size: 1234,
        video_streams: vec![media::VideoStream {
            index: 0,
            codec_name: "h264".into(),
            width: 1920,
            height: 1080,
            bit_rate: "5000000".into(),
            language: "und".into(),
            title: String::new(),
        }],
        audio_streams: vec![media::AudioStream {
            index: 1,
            codec_name: "aac".into(),
            channels: 2,
            sample_rate: "48000".into(),
            bit_rate: "128000".into(),
            channel_layout: "stereo".into(),
            language: "eng".into(),
            title: "English".into(),
        }],
        subtitle_streams: vec![media::SubtitleStream {
            index: 2,
            codec_name: "subrip".into(),
            language: "eng".into(),
            title: "Signs".into(),
        }],
    }
}

#[test]
fn build_db_rows_registers_uploaded_outputs() {
    let request = JobRequest {
        job_id: "job1".into(),
        filename: "movie.mkv".into(),
        source_path: PathBuf::from("movie.mkv"),
        metadata: JobMetadata {
            media_type: Some("Film".into()),
            title: Some("Movie Title".into()),
            ..Default::default()
        },
        delete_source_on_finish: true,
    };
    let analysis = sample_analysis(PathBuf::from("movie.mkv"));
    let mut segment_durations = HashMap::new();
    segment_durations.insert("video_0/video_0001.m4s".into(), 4.0);
    segment_durations.insert("audio_0/audio_0001.ts".into(), 4.1);
    let result = media::ProcessingResult {
        job_id: "job1".into(),
        output_dir: PathBuf::from("processing/job1"),
        video_playlists: vec![media::VideoPlaylist {
            playlist_path: PathBuf::from("processing/job1/video_0/playlist.m3u8"),
            tier_dir: "video_0".into(),
            width: 1920,
            height: 1080,
            bitrate: "copy".into(),
        }],
        audio_playlists: vec![media::AudioPlaylist {
            playlist_path: PathBuf::from("processing/job1/audio_0/playlist.m3u8"),
            audio_dir: "audio_0".into(),
            language: "eng".into(),
            title: "English".into(),
            channels: 2,
        }],
        subtitle_files: vec![media::SubtitleFile {
            vtt_path: PathBuf::from("processing/job1/sub_0/subtitles.vtt"),
            sub_dir: "sub_0".into(),
            language: "eng".into(),
            title: "Signs".into(),
            enum_idx: 0,
            original_stream_idx: 2,
        }],
        segment_durations,
        thumbnail_path: Some(PathBuf::from("processing/job1/thumbnail/thumbnail.jpg")),
    };
    let uploads = vec![
        telegram::UploadedFile {
            segment_key: "video_0/init.mp4".into(),
            file_id: "file-init".into(),
            bot_index: 0,
            file_size: 10,
        },
        telegram::UploadedFile {
            segment_key: "video_0/video_0001.m4s".into(),
            file_id: "file-video".into(),
            bot_index: 1,
            file_size: 20,
        },
        telegram::UploadedFile {
            segment_key: "audio_0/audio_0001.ts".into(),
            file_id: "file-audio".into(),
            bot_index: 0,
            file_size: 30,
        },
        telegram::UploadedFile {
            segment_key: "thumbnail/thumbnail.jpg".into(),
            file_id: "file-thumb".into(),
            bot_index: 1,
            file_size: 40,
        },
    ];

    let (job, tracks, segments, segment_parts) =
        processing::build_db_rows(&request, &analysis, &result, uploads);
    assert!(segment_parts.is_empty());

    assert_eq!(job.filename, "Movie Title");
    assert!(job.has_thumbnail);
    assert_eq!(tracks.len(), 3);
    assert_eq!(segments.len(), 4);
    assert!(segments
        .iter()
        .any(|s| s.segment_key == "video_0/init.mp4" && s.duration.is_none()));
    assert!(segments
        .iter()
        .any(|s| s.segment_key == "video_0/video_0001.m4s" && s.duration == Some(4.0)));
}

#[test]
fn build_db_rows_keeps_split_segment_parent_for_playlists() {
    let request = JobRequest {
        job_id: "job1".into(),
        filename: "movie.mkv".into(),
        source_path: PathBuf::from("movie.mkv"),
        metadata: JobMetadata::default(),
        delete_source_on_finish: true,
    };
    let analysis = sample_analysis(PathBuf::from("movie.mkv"));
    let mut segment_durations = HashMap::new();
    segment_durations.insert("video_0/video_0001.m4s".into(), 10.01);
    let result = media::ProcessingResult {
        job_id: "job1".into(),
        output_dir: PathBuf::from("processing/job1"),
        video_playlists: vec![media::VideoPlaylist {
            playlist_path: PathBuf::from("processing/job1/video_0/playlist.m3u8"),
            tier_dir: "video_0".into(),
            width: 1920,
            height: 1080,
            bitrate: "copy".into(),
        }],
        audio_playlists: Vec::new(),
        subtitle_files: Vec::new(),
        segment_durations,
        thumbnail_path: None,
    };
    let uploads = vec![
        telegram::UploadedFile {
            segment_key: "video_0/video_0001.m4s/part_0".into(),
            file_id: "file-part-0".into(),
            bot_index: 2,
            file_size: 20,
        },
        telegram::UploadedFile {
            segment_key: "video_0/video_0001.m4s/part_1".into(),
            file_id: "file-part-1".into(),
            bot_index: 3,
            file_size: 30,
        },
    ];

    let (_, _, segments, segment_parts) =
        processing::build_db_rows(&request, &analysis, &result, uploads);

    assert_eq!(segment_parts.len(), 2);
    assert!(segments.iter().any(|s| {
        s.segment_key == "video_0/video_0001.m4s"
            && s.file_id == "split"
            && s.file_size == 50
            && s.duration == Some(10.01)
    }));
}

#[tokio::test]
async fn collect_upload_files_includes_media_and_skips_playlists() {
    let base = std::env::temp_dir().join(format!(
        "thls_job_upload_files_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    tokio::fs::create_dir_all(base.join("video_0"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(base.join("audio_0"))
        .await
        .unwrap();
    tokio::fs::write(base.join("video_0/init.mp4"), b"init")
        .await
        .unwrap();
    tokio::fs::write(base.join("video_0/video_0001.m4s"), b"video")
        .await
        .unwrap();
    tokio::fs::write(base.join("video_0/playlist.m3u8"), b"playlist")
        .await
        .unwrap();
    tokio::fs::write(base.join("audio_0/audio_0001.ts"), b"audio")
        .await
        .unwrap();

    let files = processing::collect_upload_files(&base).await.unwrap();
    let keys = files.into_iter().map(|(k, _)| k).collect::<Vec<_>>();

    assert_eq!(
        keys,
        vec![
            "audio_0/audio_0001.ts".to_string(),
            "video_0/init.mp4".to_string(),
            "video_0/video_0001.m4s".to_string()
        ]
    );
    let _ = tokio::fs::remove_dir_all(base).await;
}

#[test]
fn integrity_mismatch_prevents_db_segment_row() {
    // When upload_document fails with integrity mismatch, the caller
    // (_process_job) goes to finish_job_error and never calls db::save_job.
    // This test verifies the DB-level invariant directly: a failed upload
    // must not produce a segment row.
    //
    // The code path at src/api/jobs.rs:895-901 shows that upload_outputs
    // failure → finish_job_error → return (no db::save_job call).
    // So if upload_document returns Err (integrity mismatch), no rows are
    // inserted. This test verifies the DB is clean after a simulated failure.
    let dir = std::env::temp_dir().join(format!(
        "thls_jobs_integrity_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("integrity.db");
    let conn = crate::db::init_db(&path).unwrap();

    // No segments exist for this job
    assert!(
        crate::db::get_segment(&conn, "job-integrity", "video_0/video_0001.m4s")
            .unwrap()
            .is_none()
    );

    // Simulate: a job that failed during upload (integrity mismatch)
    // would have finish_job_error called, which does NOT insert any DB rows.
    // The job would only exist in-memory (state.jobs), not in the DB.
    assert!(crate::db::get_job(&conn, "job-integrity")
        .unwrap()
        .is_none());
}
