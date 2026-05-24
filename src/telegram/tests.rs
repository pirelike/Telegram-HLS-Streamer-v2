use super::errors::{redact_bot_token, validate_file_id};
use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use serde_json::json;
use tokio::sync::Mutex;

use crate::config::{BotConfig, BotSource};

#[derive(Debug)]
struct FakeTelegram {
    send_attempts: AtomicUsize,
    upload_size: u64,
    mismatch: bool,
    rate_limit_first: bool,
    bad_request: bool,
    forbidden: bool,
    download_bytes: Vec<u8>,
    upload_bodies: Mutex<Vec<Vec<u8>>>,
}

async fn fake_handler(
    State(fake): State<Arc<FakeTelegram>>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    let path = uri.path();
    if method == Method::POST && path.ends_with("/sendDocument") {
        fake.upload_bodies.lock().await.push(body.to_vec());
        let attempt = fake.send_attempts.fetch_add(1, Ordering::SeqCst);
        if fake.rate_limit_first && attempt == 0 {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                axum::Json(json!({
                    "ok": false,
                    "description": "Too Many Requests",
                    "parameters": { "retry_after": 0 }
                })),
            )
                .into_response();
        }
        if fake.bad_request {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({
                    "ok": false,
                    "description": "Bad Request: rejected"
                })),
            )
                .into_response();
        }
        if fake.forbidden {
            return (
                StatusCode::FORBIDDEN,
                axum::Json(json!({
                    "ok": false,
                    "description": "Forbidden: bot was blocked"
                })),
            )
                .into_response();
        }
        let remote_size = if fake.mismatch {
            fake.upload_size + 1
        } else {
            fake.upload_size
        };
        return axum::Json(json!({
            "ok": true,
            "result": {
                "document": {
                    "file_id": long_file_id(),
                    "file_size": remote_size
                }
            }
        }))
        .into_response();
    }
    if method == Method::POST && path.ends_with("/getFile") {
        return axum::Json(json!({
            "ok": true,
            "result": { "file_path": "payload.bin" }
        }))
        .into_response();
    }
    if method == Method::GET && path.contains("/file/bot") {
        return fake.download_bytes.clone().into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

async fn fake_server(fake: FakeTelegram) -> (String, Arc<FakeTelegram>) {
    let fake = Arc::new(fake);
    let app = Router::new()
        .route("/*path", any(fake_handler))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), fake)
}

fn bot() -> BotConfig {
    BotConfig {
        token: "12345678:abcdefghijklmnopqrstuvwxyzABCDEFGHI".into(),
        channel_id: -100,
        source: BotSource::Db,
        db_id: None,
        label: String::new(),
    }
}

fn long_file_id() -> String {
    "A".repeat(50)
}

async fn temp_file(bytes: &[u8]) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "thls_telegram_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    tokio::fs::write(&path, bytes).await.unwrap();
    path
}

#[tokio::test]
async fn upload_success_records_metrics() {
    let path = temp_file(b"abcdef").await;
    let (base_url, fake) = fake_server(FakeTelegram {
        send_attempts: AtomicUsize::new(0),
        upload_size: 6,
        mismatch: false,
        rate_limit_first: false,
        bad_request: false,
        forbidden: false,
        download_bytes: Vec::new(),
        upload_bodies: Mutex::new(Vec::new()),
    })
    .await;
    let runtime = TelegramRuntime::new();
    let uploaded = upload_document(
        &reqwest::Client::new(),
        &runtime,
        &base_url,
        bot(),
        2,
        &path,
        "video_0/video_0001.m4s".into(),
        None,
        20,
    )
    .await
    .unwrap();

    assert_eq!(uploaded.file_size, 6);
    assert_eq!(uploaded.bot_index, 2);
    assert_eq!(fake.send_attempts.load(Ordering::SeqCst), 1);
    let metrics = runtime.metrics_snapshot().await;
    assert_eq!(metrics.upload_count, 1);
    assert_eq!(metrics.per_bot.get(&2).unwrap().upload_bytes, 6);
    let _ = tokio::fs::remove_file(path).await;
}

#[tokio::test]
async fn encrypted_upload_uses_random_dat_filename_and_records_plain_size() {
    let path = temp_file(b"abcdef").await;
    let (base_url, fake) = fake_server(FakeTelegram {
        send_attempts: AtomicUsize::new(0),
        upload_size: 22,
        mismatch: false,
        rate_limit_first: false,
        bad_request: false,
        forbidden: false,
        download_bytes: Vec::new(),
        upload_bodies: Mutex::new(Vec::new()),
    })
    .await;
    let runtime = TelegramRuntime::new();
    let key = crate::crypto::EncryptionKey::from_hex(&"33".repeat(crate::crypto::KEY_LEN)).unwrap();
    let uploaded = upload_document(
        &reqwest::Client::new(),
        &runtime,
        &base_url,
        bot(),
        0,
        &path,
        "video_0/video_0001.m4s".into(),
        Some(&key),
        22,
    )
    .await
    .unwrap();

    assert_eq!(uploaded.file_size, 6);
    assert!(uploaded.encryption_nonce.is_some());
    let metrics = runtime.metrics_snapshot().await;
    assert_eq!(metrics.per_bot.get(&0).unwrap().upload_bytes, 22);
    let bodies = fake.upload_bodies.lock().await;
    let body = String::from_utf8_lossy(&bodies[0]);
    assert!(body.contains(".dat\""));
    assert!(!body.contains("video_0001.m4s"));
    assert!(!body.as_bytes().windows(6).any(|w| w == b"abcdef"));
    let _ = tokio::fs::remove_file(path).await;
}

#[tokio::test]
async fn oversized_upload_fails_before_contacting_telegram() {
    let path = temp_file(b"abcdef").await;
    let (base_url, fake) = fake_server(FakeTelegram {
        send_attempts: AtomicUsize::new(0),
        upload_size: 6,
        mismatch: false,
        rate_limit_first: false,
        bad_request: false,
        forbidden: false,
        download_bytes: Vec::new(),
        upload_bodies: Mutex::new(Vec::new()),
    })
    .await;
    let runtime = TelegramRuntime::new();
    let err = upload_document(
        &reqwest::Client::new(),
        &runtime,
        &base_url,
        bot(),
        0,
        &path,
        "video_0/video_0001.m4s".into(),
        None,
        5,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("telegram_file_too_large"));
    assert_eq!(fake.send_attempts.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.metrics_snapshot().await.upload_errors, 1);
    let _ = tokio::fs::remove_file(path).await;
}

#[tokio::test]
async fn integrity_mismatch_fails_after_upload_response() {
    let path = temp_file(b"abcdef").await;
    let (base_url, _fake) = fake_server(FakeTelegram {
        send_attempts: AtomicUsize::new(0),
        upload_size: 6,
        mismatch: true,
        rate_limit_first: false,
        bad_request: false,
        forbidden: false,
        download_bytes: Vec::new(),
        upload_bodies: Mutex::new(Vec::new()),
    })
    .await;
    let runtime = TelegramRuntime::new();
    let err = upload_document(
        &reqwest::Client::new(),
        &runtime,
        &base_url,
        bot(),
        0,
        &path,
        "video_0/video_0001.m4s".into(),
        None,
        20,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("upload_integrity_mismatch"));
    assert_eq!(runtime.metrics_snapshot().await.upload_errors, 1);
    let _ = tokio::fs::remove_file(path).await;
}

#[tokio::test]
async fn retry_after_retries_but_bad_request_does_not() {
    let path = temp_file(b"abcdef").await;
    let (base_url, fake) = fake_server(FakeTelegram {
        send_attempts: AtomicUsize::new(0),
        upload_size: 6,
        mismatch: false,
        rate_limit_first: true,
        bad_request: false,
        forbidden: false,
        download_bytes: Vec::new(),
        upload_bodies: Mutex::new(Vec::new()),
    })
    .await;
    let runtime = TelegramRuntime::new();
    upload_document(
        &reqwest::Client::new(),
        &runtime,
        &base_url,
        bot(),
        0,
        &path,
        "video_0/video_0001.m4s".into(),
        None,
        20,
    )
    .await
    .unwrap();
    assert_eq!(fake.send_attempts.load(Ordering::SeqCst), 2);

    let (base_url, fake) = fake_server(FakeTelegram {
        send_attempts: AtomicUsize::new(0),
        upload_size: 6,
        mismatch: false,
        rate_limit_first: false,
        bad_request: true,
        forbidden: false,
        download_bytes: Vec::new(),
        upload_bodies: Mutex::new(Vec::new()),
    })
    .await;
    let runtime = TelegramRuntime::new();
    let err = upload_document(
        &reqwest::Client::new(),
        &runtime,
        &base_url,
        bot(),
        0,
        &path,
        "video_0/video_0001.m4s".into(),
        None,
        20,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Bad Request"));
    assert_eq!(fake.send_attempts.load(Ordering::SeqCst), 1);
    let _ = tokio::fs::remove_file(path).await;
}

#[tokio::test]
async fn get_file_bytes_uses_configured_bot_index() {
    let (base_url, _fake) = fake_server(FakeTelegram {
        send_attempts: AtomicUsize::new(0),
        upload_size: 0,
        mismatch: false,
        rate_limit_first: false,
        bad_request: false,
        forbidden: false,
        download_bytes: b"payload".to_vec(),
        upload_bodies: Mutex::new(Vec::new()),
    })
    .await;
    let runtime = TelegramRuntime::new();
    let bytes = get_file_bytes(
        &reqwest::Client::new(),
        &runtime,
        &base_url,
        &[bot()],
        &long_file_id(),
        0,
    )
    .await
    .unwrap();

    assert_eq!(bytes, b"payload");
    let metrics = runtime.metrics_snapshot().await;
    assert_eq!(metrics.download_count, 1);
    assert_eq!(metrics.per_bot.get(&0).unwrap().download_bytes, 7);
}

#[tokio::test]
async fn retry_matrix_forbidden_is_not_retried() {
    let path = temp_file(b"abcdef").await;
    let (base_url, fake) = fake_server(FakeTelegram {
        send_attempts: AtomicUsize::new(0),
        upload_size: 6,
        mismatch: false,
        rate_limit_first: false,
        bad_request: false,
        forbidden: true,
        download_bytes: Vec::new(),
        upload_bodies: Mutex::new(Vec::new()),
    })
    .await;
    let runtime = TelegramRuntime::new();
    let err = upload_document(
        &reqwest::Client::new(),
        &runtime,
        &base_url,
        bot(),
        0,
        &path,
        "video_0/video_0001.m4s".into(),
        None,
        20,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("forbidden"));
    // Forbidden is permanent — only 1 attempt
    assert_eq!(fake.send_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.metrics_snapshot().await.upload_errors, 1);
    let _ = tokio::fs::remove_file(path).await;
}

#[tokio::test]
async fn bot_reload_preserves_in_flight_uploads() {
    // The TelegramRuntime's per-bot locks are keyed by token string.
    // When a bot is "reloaded" (new config), old locks persist for in-flight tasks.
    // This test verifies that upload_lock returns the same Arc for the same token
    // even when called from concurrent tasks.
    let runtime = TelegramRuntime::new();

    // Simulate two concurrent tasks acquiring the lock for the same bot token
    let lock1 = runtime.upload_lock("bot_token_A").await;
    let lock2 = runtime.upload_lock("bot_token_A").await;

    // Both should be the same Arc (pointer-equal)
    assert!(
        Arc::ptr_eq(&lock1, &lock2),
        "same token must yield the same lock Arc"
    );

    // Different token gets a different lock
    let lock3 = runtime.upload_lock("bot_token_B").await;
    assert!(
        !Arc::ptr_eq(&lock1, &lock3),
        "different tokens must yield different lock Arcs"
    );
}

#[tokio::test]
async fn concurrent_upload_lock_requests_get_same_lock() {
    let runtime = Arc::new(TelegramRuntime::new());
    let mut handles = Vec::new();

    for _ in 0..10 {
        let rt = runtime.clone();
        handles.push(tokio::spawn(
            async move { rt.upload_lock("shared_token").await },
        ));
    }

    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.unwrap());
    }
    let locks: Vec<Arc<Mutex<()>>> = results;

    // All 10 concurrent requests must return pointer-identical Arcs
    let first = &locks[0];
    for lock in &locks[1..] {
        assert!(
            Arc::ptr_eq(first, lock),
            "concurrent upload_lock calls must return the same Arc"
        );
    }
}

#[test]
fn validate_file_id_rejects_short_input() {
    let err = validate_file_id("").unwrap_err();
    assert!(
        err.to_string().contains("invalid Telegram file_id"),
        "empty string should be rejected: {err}"
    );

    let err = validate_file_id("abc").unwrap_err();
    assert!(
        err.to_string().contains("invalid Telegram file_id"),
        "short string (3 chars) should be rejected: {err}"
    );

    let err = validate_file_id(&"a".repeat(49)).unwrap_err();
    assert!(
        err.to_string().contains("invalid Telegram file_id"),
        "49-char string should be rejected: {err}"
    );
}

#[test]
fn validate_file_id_accepts_valid_ids() {
    assert!(validate_file_id(&"A".repeat(50)).is_ok());
    assert!(validate_file_id(&"aB3_z-".repeat(10)).is_ok());
    assert!(validate_file_id(&"A".repeat(255)).is_ok());
}

#[test]
fn redact_bot_token_redacts_api_url() {
    let msg = "error sending request for url (https://api.telegram.org/bot123456789:abcdefGHIJKLmnopQRStuvwxYZ/sendDocument)";
    let redacted = redact_bot_token(msg);
    assert!(
        !redacted.contains("123456789:abcdefGHIJKLmnopQRStuvwxYZ"),
        "token should be redacted: {redacted}"
    );
    assert!(
        redacted.contains("***REDACTED***"),
        "redaction marker missing: {redacted}"
    );
    assert!(
        redacted.contains("/sendDocument"),
        "method name should be preserved: {redacted}"
    );
}

#[test]
fn redact_bot_token_redacts_file_url() {
    let msg = "error for https://api.telegram.org/file/bot987654321:ZYXwvuTSRqpONMLK/getFile";
    let redacted = redact_bot_token(msg);
    assert!(
        !redacted.contains("987654321:ZYXwvuTSRqpONMLK"),
        "token should be redacted: {redacted}"
    );
    assert!(redacted.contains("***REDACTED***"));
    assert!(redacted.contains("/file/bot"));
}

#[test]
fn redact_bot_token_preserves_non_token_text() {
    let msg = "no bot token here";
    assert_eq!(redact_bot_token(msg), msg);
}

#[test]
fn redact_bot_token_ignores_short_patterns() {
    // "/bot" followed by less than 10 chars before "/" — not a real token
    let msg = "path /bot/abc/method";
    assert_eq!(redact_bot_token(msg), msg);
}
