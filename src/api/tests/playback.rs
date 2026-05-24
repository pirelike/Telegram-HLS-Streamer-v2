use super::super::*;
use super::common::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::atomic::Ordering;
use tower::ServiceExt;

#[tokio::test]
async fn cold_segment_waiter_succeeds_when_leader_disconnects() {
    let fake_telegram = fake_telegram_server().await;
    let state = app_state_with_telegram_base(fake_telegram);
    let cache_dir = state
        .db_path
        .parent()
        .unwrap()
        .join("cache")
        .to_string_lossy()
        .to_string();
    {
        let mut cfg = state.config.read().await.as_ref().clone();
        cfg.cache_dir = cache_dir;
        cfg.bots = vec![crate::config::BotConfig {
            token: "12345678:abcdefghijklmnopqrstuvwxyzABCDEFGHI".into(),
            channel_id: -100,
            source: crate::config::BotSource::Env,
            db_id: None,
            label: "test".into(),
        }];
        *state.config.write().await = Arc::new(cfg);
    }
    let file_id = "A".repeat(50);
    {
        let mut conn = state.db_conn().await.unwrap();
        let job = db::NewJob::complete("coldwait", "Cold Wait");
        let segment = db::NewSegment {
            segment_key: "video_0/video_0001.m4s".into(),
            file_id,
            bot_index: 0,
            file_size: 18,
            duration: Some(4.0),
            is_split: false,
            encryption_nonce: None,
        };
        db::save_job(&mut conn, &job, &[], &[segment], &[]).unwrap();
    }

    let leader_response = router(state.clone())
        .oneshot(
            Request::get("/segment/coldwait/video_0/video_0001.m4s")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(leader_response.status(), StatusCode::OK);
    drop(leader_response);

    let follower_response = router(state.clone())
        .oneshot(
            Request::get("/segment/coldwait/video_0/video_0001.m4s")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(follower_response.status(), StatusCode::OK);
    let bytes = response_bytes(follower_response).await;
    assert_eq!(&bytes, b"cold segment bytes");

    assert!(state.cache.snapshot().entries >= 1);
}

#[tokio::test]
async fn timed_out_segment_fetch_clears_inflight_and_allows_retry() {
    let (fake_telegram, fake) = fake_telegram_hang_once_server().await;
    let state = app_state_with_telegram_base(fake_telegram);
    {
        let mut cfg = state.config.read().await.as_ref().clone();
        cfg.bots = vec![crate::config::BotConfig {
            token: "12345678:abcdefghijklmnopqrstuvwxyzABCDEFGHI".into(),
            channel_id: -100,
            source: crate::config::BotSource::Env,
            db_id: None,
            label: "test".into(),
        }];
        *state.config.write().await = Arc::new(cfg);
    }
    {
        let mut conn = state.db_conn().await.unwrap();
        let job = db::NewJob::complete("retrytimeout", "Retry Timeout");
        let segment = db::NewSegment {
            segment_key: "video_0/video_0001.m4s".into(),
            file_id: "A".repeat(50),
            bot_index: 0,
            file_size: 19,
            duration: Some(4.0),
            is_split: false,
            encryption_nonce: None,
        };
        db::save_job(&mut conn, &job, &[], &[segment], &[]).unwrap();
    }

    let first = router(state.clone())
        .oneshot(
            Request::get("/segment/retrytimeout/video_0/video_0001.m4s")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_response(first).await["error"], "fetch_failed");

    let second = router(state.clone())
        .oneshot(
            Request::get("/segment/retrytimeout/video_0/video_0001.m4s")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(second.headers().get("content-type").unwrap(), "video/mp4");
    assert_eq!(
        second.headers().get("cache-control").unwrap(),
        "public, max-age=3600"
    );
    assert_eq!(response_bytes(second).await, b"retry segment bytes");
    assert_eq!(fake.file_gets.load(Ordering::SeqCst), 2);
    assert_eq!(state.cache.snapshot().entries, 1);
}

#[tokio::test]
async fn encrypted_segment_is_decrypted_before_response() {
    let key = crate::crypto::EncryptionKey::from_hex(&"44".repeat(crate::crypto::KEY_LEN)).unwrap();
    let encrypted = key
        .encrypt(b"plain segment bytes", "video_0/video_0001.m4s")
        .unwrap();
    let fake_telegram = fake_telegram_bytes_server(encrypted.ciphertext).await;
    let state = app_state_with_telegram_base(fake_telegram);
    {
        let mut cfg = state.config.read().await.as_ref().clone();
        cfg.telegram_encryption_key = Some(key);
        cfg.bots = vec![crate::config::BotConfig {
            token: "12345678:abcdefghijklmnopqrstuvwxyzABCDEFGHI".into(),
            channel_id: -100,
            source: crate::config::BotSource::Env,
            db_id: None,
            label: "test".into(),
        }];
        *state.config.write().await = Arc::new(cfg);
    }
    {
        let mut conn = state.db_conn().await.unwrap();
        let job = db::NewJob::complete("encrypted", "Encrypted");
        let segment = db::NewSegment {
            segment_key: "video_0/video_0001.m4s".into(),
            file_id: "A".repeat(50),
            bot_index: 0,
            file_size: 19,
            duration: Some(4.0),
            is_split: false,
            encryption_nonce: Some(encrypted.nonce_hex),
        };
        db::save_job(&mut conn, &job, &[], &[segment], &[]).unwrap();
    }

    let response = router(state)
        .oneshot(
            Request::get("/segment/encrypted/video_0/video_0001.m4s")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_bytes(response).await, b"plain segment bytes");
}
