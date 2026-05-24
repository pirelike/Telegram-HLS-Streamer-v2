use super::super::*;
use super::common::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::time::Duration;
use tower::ServiceExt;

use super::super::uploads::cleanup_expired_uploads;

#[tokio::test]
async fn upload_init_rejects_oversized_file() {
    let state = app_state();
    {
        let mut cfg = state.config.read().await.as_ref().clone();
        cfg.max_upload_size = 10;
        *state.config.write().await = Arc::new(cfg);
    }
    let response = router(state)
        .oneshot(
            Request::post("/api/upload/init")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"filename":"movie.mp4","total_size":11,"total_chunks":1}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn upload_init_computes_chunk_count() {
    let state = app_state();
    {
        let mut cfg = state.config.read().await.as_ref().clone();
        cfg.upload_chunk_size = 4;
        *state.config.write().await = Arc::new(cfg);
    }
    let response = router(state)
        .oneshot(
            Request::post("/api/upload/init")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"filename":"movie.mp4","total_size":10}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert_eq!(body["chunk_size"], 4);
    assert_eq!(body["total_chunks"], 3);
}

#[tokio::test]
async fn url_ingest_rejects_non_http_and_local_urls() {
    let state = app_state();
    let response = router(state.clone())
        .oneshot(
            Request::post("/api/ingest/url")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"file:///tmp/movie.mp4"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = router(state)
        .oneshot(
            Request::post("/api/ingest/url")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"http://127.0.0.1/movie.mp4"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn url_ingest_busy_does_not_create_job_marker() {
    let state = app_state();
    let mut permits = Vec::new();
    for _ in 0..5 {
        permits.push(
            state
                .ingest_download_semaphore
                .clone()
                .try_acquire_owned()
                .unwrap(),
        );
    }

    let response = router(state.clone())
        .oneshot(
            Request::post("/api/ingest/url")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"url":"http://93.184.216.34/movie.mp4"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(state.jobs.lock().await.is_empty());

    let conn = state.db_conn().await.unwrap();
    let jobs = crate::db::list_jobs(
        &conn,
        &crate::db::JobListFilter {
            limit: 10,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(jobs.is_empty());

    drop(permits);
}

#[tokio::test]
async fn upload_rate_limit_counts_first_request() {
    let state = app_state();
    {
        let mut cfg = state.config.read().await.as_ref().clone();
        cfg.upload_rate_limit_max_requests = 2;
        cfg.max_pending_uploads_per_ip = 10;
        *state.config.write().await = Arc::new(cfg);
    }

    for i in 0..2 {
        let body = format!(r#"{{"filename":"limited{i}.mp4","total_size":4}}"#);
        let response = router(state.clone())
            .oneshot(
                Request::post("/api/upload/init")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let response = router(state)
        .oneshot(
            Request::post("/api/upload/init")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"filename":"limited2.mp4","total_size":4}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn sixth_pending_upload_from_same_ip_is_rejected() {
    let state = app_state();
    for i in 0..5 {
        let body = format!(r#"{{"filename":"movie{i}.mp4","total_size":4,"total_chunks":1}}"#);
        let response = router(state.clone())
            .oneshot(
                Request::post("/api/upload/init")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let response = router(state)
        .oneshot(
            Request::post("/api/upload/init")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"filename":"movie5.mp4","total_size":4,"total_chunks":1}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn duplicate_chunk_is_retry_and_does_not_rewrite() {
    let state = app_state();
    let init = init_upload(state.clone(), "movie.mp4", 6).await;
    let upload_id = init["upload_id"].as_str().unwrap();

    let response = router(state.clone())
        .oneshot(
            Request::post("/api/upload/chunk")
                .header("x-upload-id", upload_id)
                .header("x-chunk-index", "0")
                .body(Body::from("abcdef"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!json_response(response).await["is_retry"].as_bool().unwrap());

    let response = router(state.clone())
        .oneshot(
            Request::post("/api/upload/chunk")
                .header("x-upload-id", upload_id)
                .header("x-chunk-index", "0")
                .body(Body::from("ghijkl"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(json_response(response).await["is_retry"].as_bool().unwrap());

    let path = {
        let pending = state.pending_uploads.lock().await;
        pending.get(upload_id).unwrap().path.clone()
    };
    assert_eq!(tokio::fs::read(path).await.unwrap(), b"abcdef");
}

#[tokio::test]
async fn out_of_order_chunks_resume_and_finalize() {
    let state = app_state();
    {
        let mut cfg = state.config.read().await.as_ref().clone();
        cfg.upload_chunk_size = 4;
        *state.config.write().await = Arc::new(cfg);
    }
    let response = router(state.clone())
        .oneshot(
            Request::post("/api/upload/init")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"filename":"movie.mp4","total_size":10,"total_chunks":3}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let init = json_response(response).await;
    let upload_id = init["upload_id"].as_str().unwrap();

    for (index, bytes) in [("2", "ij"), ("0", "abcd"), ("1", "efgh")] {
        let response = router(state.clone())
            .oneshot(
                Request::post("/api/upload/chunk")
                    .header("x-upload-id", upload_id)
                    .header("x-chunk-index", index)
                    .body(Body::from(bytes))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let response = router(state.clone())
        .oneshot(
            Request::get(format!("/api/upload/status/{upload_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = json_response(response).await;
    assert_eq!(status["received_indices"], json!([0, 1, 2]));

    let path = {
        let pending = state.pending_uploads.lock().await;
        pending.get(upload_id).unwrap().path.clone()
    };
    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"abcdefghij");

    let response = router(state.clone())
        .oneshot(
            Request::post("/api/upload/finalize")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"upload_id":"{upload_id}","metadata":{{"media_type":"Film"}}}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert!(body["job_id"].as_str().unwrap().len() <= 64);
    assert!(state.pending_uploads.lock().await.get(upload_id).is_none());
}

#[tokio::test]
async fn expired_pending_uploads_are_swept_from_disk() {
    let state = app_state();
    let init = init_upload(state.clone(), "movie.mp4", 4).await;
    let upload_id = init["upload_id"].as_str().unwrap();
    let path = {
        let mut pending = state.pending_uploads.lock().await;
        let upload = pending.get_mut(upload_id).unwrap();
        upload.last_activity = Instant::now() - Duration::from_secs(90_000);
        upload.path.clone()
    };
    assert!(path.exists());

    cleanup_expired_uploads(&state).await;
    assert!(!path.exists());
    assert!(state.pending_uploads.lock().await.get(upload_id).is_none());
}
