use std::sync::Arc;

use super::cache::CacheEntry;
use super::virtual_::*;
use super::*;

#[tokio::test]
async fn cache_hits_misses_evictions() {
    let cache = SegmentCache::new(10);
    let entry = |b: &[u8]| CacheEntry {
        bytes: Arc::new(b.to_vec()),
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
