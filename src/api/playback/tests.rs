use std::sync::Arc;
use std::time::Instant;

use axum::body::to_bytes;

use super::cache::{CacheEntry, Inflight};
use super::virtual_::*;
use super::*;

#[tokio::test]
async fn cache_hits_misses_evictions() {
    let cache = SegmentCache::new(10);
    let entry = |b: &[u8]| CacheEntry {
        bytes: Arc::new(b.to_vec()),
        file_path: None,
        content_type: "video/mp4",
    };
    cache.insert("a".into(), entry(b"1234"), None).await;
    cache.insert("b".into(), entry(b"5678"), None).await;
    assert!(cache.get("a").await.is_some());
    assert_eq!(cache.snapshot().hits, 1);
    cache.insert("c".into(), entry(b"abcde"), None).await;
    // bytes total 4+4+5=13 > 10 → evict oldest by access (b is oldest now)
    assert!(cache.get("b").await.is_none());
    let snap = cache.snapshot();
    assert!(snap.evictions >= 1);
    assert!(snap.entries <= 2);
}

#[tokio::test]
async fn cache_get_bytes_returns_reupload_payload() {
    let cache = SegmentCache::new(100);
    cache
        .insert(
            "job1/video_0/video_0001.m4s".into(),
            CacheEntry {
                bytes: Arc::new(b"segment bytes".to_vec()),
                file_path: None,
                content_type: "video/mp4",
            },
            None,
        )
        .await;

    let bytes = cache
        .get_bytes("job1/video_0/video_0001.m4s")
        .await
        .unwrap();
    assert_eq!(bytes.as_slice(), b"segment bytes");
}

#[tokio::test]
async fn inflight_wait_for_outcome_handles_prior_notify() {
    let entry = CacheEntry {
        bytes: Arc::new(b"done".to_vec()),
        file_path: None,
        content_type: "video/mp4",
    };
    let inflight = Inflight {
        outcome: tokio::sync::Mutex::new(Some(Ok(Some(entry.clone())))),
        notify: tokio::sync::Notify::new(),
    };

    inflight.notify.notify_waiters();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        inflight.wait_for_outcome(),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();

    assert_eq!(outcome.bytes.as_slice(), entry.bytes.as_slice());
    assert_eq!(outcome.content_type, entry.content_type);
}

#[test]
fn stale_file_id_error_detection_covers_telegram_permanent_errors() {
    assert!(super::real::telegram_error_suggests_stale_file_id(
        &anyhow::anyhow!("telegram_api_error status=400 description=Bad Request")
    ));
    assert!(super::real::telegram_error_suggests_stale_file_id(
        &anyhow::anyhow!("telegram_api_error status=403 description=Forbidden")
    ));
    assert!(!super::real::telegram_error_suggests_stale_file_id(
        &anyhow::anyhow!("network timeout")
    ));
}

#[tokio::test]
async fn cache_snapshot_correctness() {
    let cache = SegmentCache::new(100);
    let entry = |b: &[u8]| CacheEntry {
        bytes: Arc::new(b.to_vec()),
        file_path: None,
        content_type: "video/mp4",
    };

    // Fresh cache: all counters zero, empty
    let snap = cache.snapshot();
    assert_eq!(snap.entries, 0);
    assert_eq!(snap.size_bytes, 0);
    assert_eq!(snap.hits, 0);
    assert_eq!(snap.misses, 0);
    assert_eq!(snap.evictions, 0);

    // Insert 2 entries totaling 8 bytes
    cache.insert("key1".into(), entry(b"abcd"), None).await;
    cache.insert("key2".into(), entry(b"efgh"), None).await;

    let snap = cache.snapshot();
    assert_eq!(snap.entries, 2);
    assert_eq!(snap.size_bytes, 8);
    assert_eq!(snap.hits, 0);
    assert_eq!(snap.misses, 0);
    assert_eq!(snap.evictions, 0);

    // Get existing key -> hit counted
    assert!(cache.get("key1").await.is_some());
    let snap = cache.snapshot();
    assert_eq!(snap.hits, 1);

    // Get missing key -> None, but get() does not increment misses
    assert!(cache.get("missing").await.is_none());
    let snap = cache.snapshot();
    assert_eq!(snap.misses, 0);
    assert_eq!(snap.entries, 2); // entries unchanged
}

#[tokio::test]
async fn cache_response_streams_file_when_available() {
    let path = std::env::temp_dir().join(format!(
        "thls-cache-response-{}.bin",
        uuid::Uuid::new_v4().simple()
    ));
    tokio::fs::write(&path, b"file bytes").await.unwrap();

    let entry = CacheEntry {
        bytes: Arc::new(b"buffer bytes".to_vec()),
        file_path: Some(path.clone()),
        content_type: "video/mp4",
    };
    let response = cache_response(entry);
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_TYPE],
        "video/mp4"
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"file bytes");

    let _ = tokio::fs::remove_file(path).await;
}

#[tokio::test]
async fn cache_response_falls_back_to_buffer_when_file_open_fails() {
    let entry = CacheEntry {
        bytes: Arc::new(b"buffer bytes".to_vec()),
        file_path: Some(std::env::temp_dir().join("thls-missing-cache-file.bin")),
        content_type: "video/mp4",
    };
    let response = cache_response(entry);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"buffer bytes");
}

#[tokio::test]
#[ignore = "manual P1 performance smoke; run with --ignored --nocapture"]
async fn p1_performance_file_backed_cache_response_smoke() {
    const BYTES_PER_RESPONSE: usize = 4 * 1024 * 1024;
    const ITERATIONS: usize = 8;

    let path = std::env::temp_dir().join(format!(
        "thls-p1-cache-response-{}.bin",
        uuid::Uuid::new_v4().simple()
    ));
    let payload = vec![0x5au8; BYTES_PER_RESPONSE];
    tokio::fs::write(&path, &payload).await.unwrap();

    let started = Instant::now();
    let mut served = 0usize;
    for _ in 0..ITERATIONS {
        let entry = CacheEntry {
            bytes: Arc::new(Vec::new()),
            file_path: Some(path.clone()),
            content_type: "video/mp4",
        };
        let response = cache_response(entry);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        served += bytes.len();
    }
    let elapsed = started.elapsed();

    println!(
        "p1_performance_file_backed_cache_response_smoke: iterations={ITERATIONS} bytes_per_response={BYTES_PER_RESPONSE} bytes_served={served} elapsed_ms={} throughput_mib_s={:.2}",
        elapsed.as_millis(),
        served as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64()
    );

    let _ = tokio::fs::remove_file(path).await;
}

#[tokio::test]
async fn cache_eviction_removes_file_best_effort() {
    let cache = SegmentCache::new(4);
    let path = std::env::temp_dir().join(format!(
        "thls-cache-evict-{}.bin",
        uuid::Uuid::new_v4().simple()
    ));
    tokio::fs::write(&path, b"cached").await.unwrap();

    cache
        .insert(
            "a".into(),
            CacheEntry {
                bytes: Arc::new(b"1234".to_vec()),
                file_path: Some(path.clone()),
                content_type: "video/mp4",
            },
            None,
        )
        .await;
    cache
        .insert(
            "b".into(),
            CacheEntry {
                bytes: Arc::new(b"5678".to_vec()),
                file_path: None,
                content_type: "video/mp4",
            },
            None,
        )
        .await;

    for _ in 0..20 {
        if tokio::fs::metadata(&path).await.is_err() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(tokio::fs::metadata(&path).await.is_err());
}

#[test]
fn content_type_mapping_covers_phase6_extensions() {
    assert_eq!(content_type_for("foo.ts"), "video/mp2t");
    assert_eq!(content_type_for("foo.m4s"), "video/mp4");
    assert_eq!(content_type_for("init.mp4"), "video/mp4");
    assert_eq!(content_type_for("subs.vtt"), "text/vtt");
    assert_eq!(content_type_for("thumb.jpg"), "image/jpeg");
    assert_eq!(content_type_for("x.bin"), "application/octet-stream");
}

#[test]
fn is_virtual_key_recognises_virtual_prefix() {
    assert!(is_virtual_key("virtual_720p/video_0001.m4s"));
    assert!(is_virtual_key("virtual_480p/init.mp4"));
    assert!(!is_virtual_key("video_0/video_0001.m4s"));
    assert!(!is_virtual_key("virtual_/foo.m4s"));
}

#[test]
fn webvtt_hls_timestamp_map_is_injected_once() {
    let input = b"WEBVTT\n\n00:01.000 --> 00:02.000\nHi\n".to_vec();
    let once = bytes_for_key("sub_0/subtitles.vtt", input);
    assert!(String::from_utf8_lossy(&once).contains("X-TIMESTAMP-MAP"));

    let twice = bytes_for_key("sub_0/subtitles.vtt", once.clone());
    assert_eq!(once, twice);
}

#[test]
fn scan_top_level_boxes_walks_concat() {
    // ftyp box (size=20) + moov box (size=12) + custom (size=10)
    let mut data = Vec::new();
    data.extend_from_slice(&20u32.to_be_bytes());
    data.extend_from_slice(b"ftyp");
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(&12u32.to_be_bytes());
    data.extend_from_slice(b"moov");
    data.extend_from_slice(&[0u8; 4]);
    data.extend_from_slice(&10u32.to_be_bytes());
    data.extend_from_slice(b"mdat");
    data.extend_from_slice(&[0u8; 2]);
    let boxes = scan_top_level_boxes(&data).unwrap();
    assert_eq!(boxes.len(), 3);
    assert_eq!(boxes[0].0, *b"ftyp");
    assert_eq!(boxes[1].0, *b"moov");
    assert_eq!(boxes[2].0, *b"mdat");

    let init = extract_init_from_fmp4(&data).unwrap();
    assert!(init.starts_with(&20u32.to_be_bytes()));
    let stripped = strip_init_from_fmp4(&data).unwrap();
    assert!(stripped.starts_with(&10u32.to_be_bytes()));
}

#[test]
fn scan_top_level_boxes_short_input() {
    // input shorter than 8-byte box header — loop never enters, empty result
    let result = scan_top_level_boxes(&[0x00, 0x00, 0x00]);
    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[test]
fn scan_top_level_boxes_truncated_box() {
    // box header declares size=9999 but only 8 bytes of data exist
    let mut data = Vec::new();
    data.extend_from_slice(&9999u32.to_be_bytes());
    data.extend_from_slice(b"ftyp");
    let result = scan_top_level_boxes(&data);
    assert!(result.is_err());
}

#[test]
fn scan_top_level_boxes_invalid_size() {
    // valid-length buffer but box size < 8
    let mut data = Vec::new();
    data.extend_from_slice(&4u32.to_be_bytes());
    data.extend_from_slice(b"ftyp");
    data.extend_from_slice(&[0u8; 8]);
    let result = scan_top_level_boxes(&data);
    assert!(result.is_err());
}

#[test]
fn scan_top_level_boxes_truncated_64bit_header() {
    // size=1 flag set (64-bit extended size) but buffer < 16 bytes
    let mut data = Vec::new();
    data.extend_from_slice(&1u32.to_be_bytes());
    data.extend_from_slice(b"ftyp");
    let result = scan_top_level_boxes(&data);
    assert!(result.is_err());
}

#[test]
fn scan_top_level_boxes_empty_input() {
    // empty slice — no boxes, no error
    let result = scan_top_level_boxes(&[]);
    assert_eq!(result.unwrap(), vec![]);
}

#[test]
fn scan_top_level_boxes_garbage_input() {
    // Arbitrary garbage bytes: loop should return an error, not panic.
    let garbage = b"\xff\xfe\xfd\xfc\xfb\xfa\xf9\xf8extra";
    let result = scan_top_level_boxes(garbage);
    assert!(result.is_err());
}

#[test]
fn extract_init_from_fmp4_garbage_input() {
    // Garbage bytes must propagate a parse error (not panic) and surface a useful message.
    let garbage = b"\x00\x00\xff\xfegarbagegarbagegarbage";
    let err = extract_init_from_fmp4(garbage).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("invalid box")
            || msg.contains("box extends")
            || msg.contains("ftyp")
            || msg.contains("moov")
            || msg.contains("no ftyp"),
        "unexpected error message: {msg}",
    );
}

#[test]
fn extract_init_from_fmp4_malformed_data_returns_err() {
    // 1. Empty input — scan_top_level_boxes returns Ok(vec![]),
    //    then extract_init_from_fmp4 returns Err("no ftyp/moov found")
    assert!(extract_init_from_fmp4(&[]).is_err());

    // 2. Data shorter than 8-byte box header — no boxes found, same empty-check error
    assert!(extract_init_from_fmp4(&[0x00, 0x00, 0x00]).is_err());

    // 3. Box header declares size 9999 but only 8 bytes exist — scan error propagates
    let mut data = Vec::new();
    data.extend_from_slice(&9999u32.to_be_bytes());
    data.extend_from_slice(b"ftyp");
    assert!(extract_init_from_fmp4(&data).is_err());

    // 4. Valid ftyp box followed by moov header declaring truncated body — scan error propagates
    let mut data = Vec::new();
    data.extend_from_slice(&20u32.to_be_bytes());
    data.extend_from_slice(b"ftyp");
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(&9999u32.to_be_bytes()); // moov claims 9999 bytes
    data.extend_from_slice(b"moov"); // but no body data follows
    assert!(extract_init_from_fmp4(&data).is_err());
}
