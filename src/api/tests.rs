use super::*;
use axum::body::{to_bytes, Body};
use axum::extract::Path as AxumPath;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response as AxumResponse};
use axum::routing::any;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_stream::wrappers::ReceiverStream;
use tower::ServiceExt;

use super::uploads::cleanup_expired_uploads;

fn app_state() -> Arc<AppState> {
    app_state_with_telegram_base(crate::telegram::DEFAULT_API_BASE.to_string())
}

fn app_state_with_telegram_base(telegram_base_url: String) -> Arc<AppState> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("thls_api_tests_{unique}"));
    std::fs::create_dir_all(&dir).unwrap();
    let uploads_dir = dir.join("uploads");
    let processing_dir = dir.join("processing");
    let db_path = dir.join("streamer.db");
    std::fs::create_dir_all(&uploads_dir).unwrap();
    std::fs::create_dir_all(&processing_dir).unwrap();
    let pool = crate::db::init_db_pool(&db_path).unwrap();
    let conn = pool.get().unwrap();
    let cfg = Config::load(&conn).unwrap();
    drop(conn);
    let watch_settings = watch_folder::WatchSettings {
        watch_enabled: false,
        watch_root: String::new(),
        watch_done_dir: String::new(),
    };
    let (job_queue, job_receiver) = mpsc::channel(100);
    let state = Arc::new(AppState {
        db: RwLock::new(pool),
        db_path: db_path.clone(),
        env_path: db_path.parent().unwrap().join(".env"),
        config: RwLock::new(Arc::new(cfg)),
        started_at: Instant::now(),
        bot_health: RwLock::new(Vec::new()),
        cloudflared: crate::cloudflared::SharedCloudflaredStatus::default(),
        http: reqwest::Client::new(),
        telegram: TelegramRuntime::new(),
        telegram_base_url,
        uploads_dir,
        processing_dir,
        watch_settings: RwLock::new(watch_settings),
        watch_seen: Mutex::new(HashMap::new()),
        pending_uploads: Mutex::new(HashMap::new()),
        upload_rate_limits: Mutex::new(HashMap::new()),
        jobs: Mutex::new(HashMap::new()),
        played_segments: Mutex::new(HashMap::new()),
        job_queue,
        cache: Arc::new(SegmentCache::new(64 * 1024 * 1024)),
        ffmpeg_available: true,
        ffprobe_available: true,
        selected_encoder: RwLock::new(crate::media::cpu_encoder()),
        last_bot_index: std::sync::atomic::AtomicI64::new(0),
    });
    start_background_tasks(state.clone(), job_receiver);
    state
}

async fn json_response(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn exhaust_db_pool(state: &Arc<AppState>) -> crate::db::DbConn {
    let manager = r2d2_sqlite::SqliteConnectionManager::file(&state.db_path);
    let pool = r2d2::Pool::builder()
        .max_size(1)
        .connection_timeout(Duration::from_millis(10))
        .build(manager)
        .unwrap();
    let held_conn = pool.get().unwrap();
    *state.db.write().await = pool;
    held_conn
}

async fn response_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn orphaned_processing_cleanup_removes_orphans_and_keeps_active_jobs() {
    let state = app_state();
    let orphan = state.processing_dir.join("orphan-job");
    let active = state.processing_dir.join("active-job");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::create_dir_all(&active).unwrap();
    {
        let conn = state.db_conn().await.unwrap();
        crate::db::insert_processing_marker(&conn, "active-job", "active.mkv").unwrap();
    }

    jobs::processing::clean_orphaned_processing_dirs(&state).await;

    assert!(!orphan.exists());
    assert!(active.exists());
}

#[tokio::test]
async fn request_handlers_return_503_when_db_pool_is_exhausted() {
    let state = app_state();
    let _held_conn = exhaust_db_pool(&state).await;

    for request in [
        Request::post("/api/db/backup").body(Body::empty()).unwrap(),
        Request::get("/api/bots").body(Body::empty()).unwrap(),
        Request::get("/hls/missing/master.m3u8")
            .body(Body::empty())
            .unwrap(),
        Request::get("/series/missing").body(Body::empty()).unwrap(),
    ] {
        let response = router(state.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = json_response(response).await;
        assert_eq!(body["error"], "db_unavailable");
    }
}

fn multipart_body(fields: &[(&str, Option<&str>, Vec<u8>)]) -> (String, Vec<u8>) {
    let boundary = format!(
        "thls-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mut body = Vec::new();
    for (name, filename, bytes) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match filename {
            Some(filename) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\r\n"
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            ),
        }
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

struct FakeWebhook {
    hits: AtomicUsize,
}

async fn webhook_handler(
    State(fake): State<Arc<FakeWebhook>>,
    body: axum::body::Bytes,
) -> StatusCode {
    fake.hits.fetch_add(1, Ordering::SeqCst);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["event"], "job_terminal");
    StatusCode::OK
}

async fn fake_webhook_server() -> (String, Arc<FakeWebhook>) {
    let fake = Arc::new(FakeWebhook {
        hits: AtomicUsize::new(0),
    });
    let app = axum::Router::new()
        .route("/*path", any(webhook_handler))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/hook"), fake)
}

async fn fake_telegram_handler(AxumPath(path): AxumPath<String>) -> AxumResponse {
    if path.ends_with("/getFile") {
        return Json(json!({
            "ok": true,
            "result": { "file_path": "segment.bin" }
        }))
        .into_response();
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(4);
    tokio::spawn(async move {
        for chunk in [
            axum::body::Bytes::from_static(b"cold "),
            axum::body::Bytes::from_static(b"segment "),
            axum::body::Bytes::from_static(b"bytes"),
        ] {
            let _ = tx.send(Ok(chunk)).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    (StatusCode::OK, Body::from_stream(ReceiverStream::new(rx))).into_response()
}

async fn fake_telegram_server() -> String {
    let app = axum::Router::new().route("/*path", any(fake_telegram_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn init_upload(state: Arc<AppState>, filename: &str, total_size: u64) -> Value {
    let response = router(state)
        .oneshot(
            Request::post("/api/upload/init")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"filename":"{filename}","total_size":{total_size},"total_chunks":1}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json_response(response).await
}

async fn save_complete_job(
    state: &Arc<AppState>,
    job_id: &str,
    filename: &str,
    media_type: &str,
    series_name: &str,
    season_number: Option<i64>,
    episode_number: Option<i64>,
) {
    let mut conn = state.db_conn().await.unwrap();
    let mut job = db::NewJob::complete(job_id, filename);
    job.media_type = media_type.into();
    job.series_name = series_name.into();
    job.is_series = !series_name.is_empty();
    job.season_number = season_number;
    job.episode_number = episode_number;
    let tracks = vec![
        db::NewTrack {
            track_type: "video".into(),
            track_index: 0,
            codec: "h264".into(),
            language: "und".into(),
            title: String::new(),
            channels: 0,
            width: 1920,
            height: 1080,
            bitrate: "5M".into(),
            original_stream_index: 0,
        },
        db::NewTrack {
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
    ];
    let segments = vec![db::NewSegment {
        segment_key: "video_0/video_0001.m4s".into(),
        file_id: format!("file-{job_id}"),
        bot_index: 0,
        file_size: 123,
        duration: Some(4.0),
        is_split: false,
    }];
    db::save_job(&mut conn, &job, &tracks, &segments, &[]).unwrap();
}

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

#[tokio::test]
async fn settings_get_post_and_reset_update_runtime_config() {
    let state = app_state();
    let app = router(state.clone());
    let response = router(state.clone())
        .oneshot(Request::get("/api/settings").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert!(body["categories"]["reliability"]["settings"].is_array());

    let response = app
        .oneshot(
            Request::post("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"MAX_CONCURRENT_JOBS":2,"ABR_ENABLED":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.config.read().await.max_concurrent_jobs, 2);

    let response = router(state.clone())
        .oneshot(
            Request::post("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"DISK_CACHE_ENABLED":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert_eq!(state.config.read().await.disk_cache_enabled, true);
    let file_settings = body["categories"]["file_handling"]["settings"]
        .as_array()
        .unwrap();
    assert!(file_settings
        .iter()
        .any(|s| s["key"] == "DISK_CACHE_ENABLED" && s["value"] == true));

    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::post("/api/settings/reset")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"keys":["MAX_CONCURRENT_JOBS"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.config.read().await.max_concurrent_jobs, 1);
}

#[tokio::test]
async fn settings_reject_invalid_values() {
    let state = app_state();
    let response = router(state)
        .oneshot(
            Request::post("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"PREFERRED_ENCODER":"x264"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let state = app_state();
    let response = router(state)
        .oneshot(
            Request::post("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"NOT_A_SETTING":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn settings_update_does_not_reload_bot_pool() {
    let state = app_state();
    let env_bot_count;
    {
        let conn = state.db_conn().await.unwrap();
        db::add_bot(
            &conn,
            "12345678:abcdefghijklmnopqrstuvwxyzabcdefghi",
            -100,
            "first",
        )
        .unwrap();
        *state.config.write().await = Arc::new(Config::load(&conn).unwrap());
        env_bot_count = state.config.read().await.bots.len();
        // env bots + the one we just added
        assert!(env_bot_count >= 1);
        db::add_bot(
            &conn,
            "22345678:abcdefghijklmnopqrstuvwxyzabcdefghi",
            -101,
            "second",
        )
        .unwrap();
    }
    // Config still has env_bot_count bots (second add not loaded yet)
    assert_eq!(state.config.read().await.bots.len(), env_bot_count);

    let response = router(state.clone())
        .oneshot(
            Request::post("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"MAX_CONCURRENT_JOBS":3}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cfg = state.config.read().await;
    assert_eq!(cfg.max_concurrent_jobs, 3);
    // Settings update does not reload bot pool — count stays the same
    assert_eq!(cfg.bots.len(), env_bot_count);
}

#[tokio::test]
async fn watch_settings_validate_paths_and_persist() {
    let state = app_state();
    let base = state.db_path.parent().unwrap().to_path_buf();
    let root = base.join("watch");
    let done = root.join("done");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();

    let response = router(state.clone())
        .oneshot(
            Request::post("/api/watch-settings")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"watch_enabled":true,"watch_root":"{}","watch_done_dir":"{}"}}"#,
                    root.display(),
                    done.display()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let conn = state.db_conn().await.unwrap();
    assert!(crate::db::get_internal_value(&conn, "watch_settings")
        .unwrap()
        .is_some());
    drop(conn);
    assert!(done.exists());
    let body = json_response(response).await;
    assert_eq!(body["watch_running"], true);

    let response = router(state)
        .oneshot(
            Request::post("/api/watch-settings")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"watch_enabled":true,"watch_root":"{}","watch_done_dir":"{}"}}"#,
                    root.display(),
                    outside.display()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn db_export_import_reports_missing_source_bot_map_entries_and_allows_explicit_remap() {
    let source = app_state();
    save_complete_job(&source, "job1", "Movie", "Film", "", None, None).await;
    let response = router(source)
        .oneshot(
            Request::post("/api/db/export")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"upload_to_telegram":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let export_bytes = response_bytes(response).await;
    let mut export: db::DbExport = serde_json::from_slice(&export_bytes).unwrap();
    assert_eq!(export.version, 1);
    assert_eq!(export.jobs.len(), 1);
    export.segments[0].bot_index = 1;
    let export_bytes = serde_json::to_vec(&export).unwrap();

    let target = app_state();
    // Without bot_index_map — auto-fills to 0
    let (content_type, body) =
        multipart_body(&[("file", Some("export.json"), export_bytes.clone())]);
    let response = router(target.clone())
        .oneshot(
            Request::post("/api/db/import")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert_eq!(body["merged_jobs"], 1);
    assert_eq!(body["merged_segments"], 1);
    let conn = target.db_conn().await.unwrap();
    // Segment was auto-mapped from bot_index=1 to 0
    assert_eq!(
        db::get_segment(&conn, "job1", "video_0/video_0001.m4s")
            .unwrap()
            .unwrap()
            .bot_index,
        0
    );
    drop(conn);

    let (content_type, body) = multipart_body(&[
        ("file", Some("export.json"), export_bytes),
        ("bot_index_map", None, br#"{"0":0}"#.to_vec()),
    ]);
    let response = router(target.clone())
        .oneshot(
            Request::post("/api/db/import")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_response(response).await;
    assert_eq!(body["error"], "invalid_bot_index_map");
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("missing bot_index_map entries for [1]"));

    let (content_type, body) = multipart_body(&[
        (
            "file",
            Some("export.json"),
            serde_json::to_vec(&export).unwrap(),
        ),
        ("bot_index_map", None, br#"{"1":0}"#.to_vec()),
    ]);
    let response = router(target.clone())
        .oneshot(
            Request::post("/api/db/import")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    // Data was already merged by auto-fill above (INSERT OR IGNORE)
    assert_eq!(body["merged_jobs"], 0);
    assert_eq!(body["merged_segments"], 0);
    let conn = target.db_conn().await.unwrap();
    assert!(db::get_job(&conn, "job1").unwrap().is_some());
    assert_eq!(
        db::get_segment(&conn, "job1", "video_0/video_0001.m4s")
            .unwrap()
            .unwrap()
            .bot_index,
        0
    );
}

#[tokio::test]
async fn database_load_replaces_live_db_and_reports_backup() {
    let state = app_state();
    let source_path = state.db_path.parent().unwrap().join("replacement.db");
    {
        let mut conn = db::init_db(&source_path).unwrap();
        let job = db::NewJob::complete("loaded", "Loaded Movie");
        db::save_job(&mut conn, &job, &[], &[], &[]).unwrap();
    }
    let bytes = std::fs::read(&source_path).unwrap();
    let (content_type, body) = multipart_body(&[("database", Some("replacement.db"), bytes)]);
    let response = router(state.clone())
        .oneshot(
            Request::post("/api/database/load")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert_eq!(body["schema_revision"], db::LATEST_SCHEMA_REVISION);
    assert!(std::path::Path::new(body["backup_path"].as_str().unwrap()).exists());
    let conn = state.db_conn().await.unwrap();
    assert!(db::get_job(&conn, "loaded").unwrap().is_some());
}

#[tokio::test]
async fn database_load_waits_for_checked_out_pool_connections() {
    let state = app_state();
    let held_conn = state.db_conn().await.unwrap();
    let source_path = state.db_path.parent().unwrap().join("replacement_wait.db");
    {
        let mut conn = db::init_db(&source_path).unwrap();
        let job = db::NewJob::complete("loaded-after-drain", "Loaded After Drain");
        db::save_job(&mut conn, &job, &[], &[], &[]).unwrap();
    }
    let bytes = std::fs::read(&source_path).unwrap();
    let (content_type, body) = multipart_body(&[("database", Some("replacement.db"), bytes)]);
    let request_state = state.clone();
    let handle = tokio::spawn(async move {
        router(request_state)
            .oneshot(
                Request::post("/api/database/load")
                    .header("content-type", content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !handle.is_finished(),
        "database load must wait for checked-out old pool connections"
    );
    drop(held_conn);

    let response = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let conn = state.db_conn().await.unwrap();
    assert!(db::get_job(&conn, "loaded-after-drain").unwrap().is_some());
}

#[tokio::test]
async fn database_backup_endpoint_creates_server_file() {
    let state = app_state();
    let response = router(state)
        .oneshot(Request::post("/api/db/backup").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert_eq!(body["schema_revision"], db::LATEST_SCHEMA_REVISION);
    assert!(std::path::Path::new(body["backup_path"].as_str().unwrap()).exists());
    assert!(body["size_bytes"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn reconstruct_endpoints_reject_missing_jobs() {
    let state = app_state();
    let response = router(state.clone())
        .oneshot(
            Request::get("/api/jobs/missing/download-original")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = router(state)
        .oneshot(
            Request::post("/api/jobs/missing/reprocess")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bots_are_masked_and_metrics_shape_exists() {
    let state = app_state();
    let token = "12345678:abcdefghijklmnopqrstuvwxyzabcdefghi";
    {
        let conn = state.db_conn().await.unwrap();
        db::add_bot(&conn, token, -100, "main").unwrap();
        *state.config.write().await = Arc::new(Config::load(&conn).unwrap());
    }
    let response = router(state.clone())
        .oneshot(Request::get("/api/bots").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    let bots = body["bots"].as_array().expect("bots array");
    // Find the DB-added bot (last in the list since env bots come first)
    let db_bot = bots
        .iter()
        .find(|b| b["token_masked"].as_str().unwrap().starts_with("12345678:"))
        .expect("db bot");
    let masked = db_bot["token_masked"].as_str().unwrap();
    assert!(masked.contains("***"), "token must be masked: {masked}");
    assert_eq!(masked, "12345678:abc***ghi");
    assert!(db_bot["token"].is_null());

    let response = router(state)
        .oneshot(Request::get("/api/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert!(body.get("cache").is_some());
    assert!(body.get("telegram").is_some());
}

#[tokio::test]
async fn health_expands_operational_fields() {
    let state = app_state();
    let response = router(state)
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert!(body.get("cloudflared").is_some());
    assert!(body.get("cache").is_some());
    assert_eq!(body["status"], "degraded");
}

#[tokio::test]
async fn cancelling_job_sends_terminal_webhook() {
    let state = app_state();
    let (url, fake) = fake_webhook_server().await;
    {
        let mut cfg = state.config.read().await.as_ref().clone();
        cfg.webhook_url = url;
        *state.config.write().await = Arc::new(cfg);
    }
    let base = state.db_path.parent().unwrap();
    let source_path = base.join("queued.mp4");
    std::fs::write(&source_path, b"video").unwrap();
    let processing_path = base.join("processing-job");
    state.jobs.lock().await.insert(
        "queued-job".into(),
        JobState {
            job_id: "queued-job".into(),
            filename: "Queued".into(),
            source_path,
            processing_path,
            status: jobs::JobStatus::Queued,
            progress: 0.0,
            step: 0,
            total_steps: 5,
            description: "queued".into(),
            queued_at: Instant::now(),
            started_at: None,
            finished_at: None,
            cancel_requested: false,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            error: None,
            metadata: jobs::JobMetadata {
                media_type: Some("Film".into()),
                ..jobs::JobMetadata::default()
            },
            analysis: None,
            delete_source_on_finish: true,
            original_source_path: None,
        },
    );

    let response = router(state)
        .oneshot(
            Request::post("/api/cancel/queued-job")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(fake.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn jobs_api_lists_groups_updates_and_deletes() {
    let state = app_state();
    save_complete_job(&state, "film1", "Film One", "Film", "", None, None).await;
    save_complete_job(
        &state,
        "show1",
        "Episode 1",
        "Series",
        "My Show",
        Some(1),
        Some(1),
    )
    .await;
    save_complete_job(
        &state,
        "show2",
        "Episode 2",
        "Series",
        "My Show",
        Some(1),
        Some(2),
    )
    .await;

    let response = router(state.clone())
        .oneshot(
            Request::get("/api/jobs?category=Series&group_by=series")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["jobs"][0]["series_name"], "My Show");
    assert_eq!(body["jobs"][0]["episode_count"], 2);

    let response = router(state.clone())
        .oneshot(Request::get("/api/jobs/show1").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert_eq!(body["audio_count"], 1);
    assert_eq!(body["segment_count"], 1);

    let response = router(state.clone())
        .oneshot(
            Request::patch("/api/jobs/show1")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"title":"Renamed","is_series":false,"season_number":null}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    assert_eq!(body["filename"], "Renamed");
    assert_eq!(body["is_series"], false);
    assert!(body["season_number"].is_null());

    let response = router(state.clone())
        .oneshot(
            Request::delete("/api/jobs/show2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = router(state)
        .oneshot(
            Request::get("/hls/show2/master.m3u8")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn phase_7_pages_return_html() {
    let state = app_state();
    save_complete_job(
        &state,
        "show1",
        "Episode 1",
        "Series",
        "My Show",
        Some(1),
        Some(1),
    )
    .await;
    save_complete_job(
        &state,
        "anime1",
        "Episode 1",
        "Anime TV",
        "Ani Show",
        Some(1),
        Some(1),
    )
    .await;
    for path in [
        "/",
        "/films",
        "/series",
        "/series/my-show",
        "/series/my-show/s1",
        "/series/my-show/specials",
        "/anime-films",
        "/anime-tv",
        "/anime-tv/ani-show",
        "/anime-tv/ani-show/s1",
        "/anime-tv/ani-show/specials",
        "/upload",
        "/settings",
        "/watch/show1",
    ] {
        let response = router(state.clone())
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn finalize_order_enqueues_before_removing_pending() {
    let state = app_state();
    let chunk_size = 10u64;
    let total_size = chunk_size;
    let init = init_upload(state.clone(), "order.mkv", total_size).await;
    let upload_id = init["upload_id"].as_str().unwrap();

    // Write the single chunk
    let response = router(state.clone())
        .oneshot(
            Request::post("/api/upload/chunk")
                .header("x-upload-id", upload_id)
                .header("x-chunk-index", "0")
                .header("content-type", "application/octet-stream")
                .body(Body::from(vec![0u8; chunk_size as usize]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Finalize
    let response = router(state.clone())
        .oneshot(
            Request::post("/api/upload/finalize")
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"upload_id":"{upload_id}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_response(response).await;
    let job_id = body["job_id"].as_str().unwrap();

    // Invariant: job was enqueued (present in state.jobs)
    assert!(state.jobs.lock().await.contains_key(job_id));
    // Invariant: pending bookkeeping was removed after enqueue
    assert!(state.pending_uploads.lock().await.get(upload_id).is_none());
}

#[tokio::test]
async fn cancel_during_processing_cleans_up_and_leaves_no_db_row() {
    let state = app_state();
    let base = state.db_path.parent().unwrap();
    let source_path = base.join("source-cancel.mp4");
    std::fs::write(&source_path, b"video").unwrap();
    let processing_path = base.join("processing-cancel");
    std::fs::create_dir_all(&processing_path).unwrap();
    std::fs::write(processing_path.join("partial.m4s"), b"partial").unwrap();

    state.jobs.lock().await.insert(
        "cancel-proc".into(),
        jobs::JobState {
            job_id: "cancel-proc".into(),
            filename: "cancel.mp4".into(),
            source_path: source_path.clone(),
            processing_path: processing_path.clone(),
            status: jobs::JobStatus::Processing,
            progress: 50.0,
            step: 2,
            total_steps: 5,
            description: "processing".into(),
            queued_at: Instant::now(),
            started_at: Some(Instant::now()),
            finished_at: None,
            cancel_requested: false,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            error: None,
            metadata: jobs::JobMetadata::default(),
            analysis: None,
            delete_source_on_finish: true,
            original_source_path: None,
        },
    );

    let response = router(state.clone())
        .oneshot(
            Request::post("/api/cancel/cancel-proc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Processing directory removed
    assert!(!processing_path.exists());
    // Source file retained for deferred deletion (delete_source_on_finish = true)
    assert!(!source_path.exists());
    assert!(source_path
        .with_file_name("source-cancel.mp4.pending_delete")
        .exists());
    // Job status is cancelled
    let jobs = state.jobs.lock().await;
    let job = jobs.get("cancel-proc").unwrap();
    assert_eq!(job.status.as_str(), "cancelled");
    drop(jobs);
    // No DB row for this job
    let conn = state.db_conn().await.unwrap();
    assert!(db::get_job(&conn, "cancel-proc").unwrap().is_none());
}

#[tokio::test]
async fn malformed_job_id_returns_400_before_db_lookup() {
    let state = app_state();

    for bad_id in ["", "has spaces", "a/b", "..", "id with spaces"] {
        assert!(!valid_job_id(bad_id), "should reject: {bad_id}");
    }

    // Test with a newline-containing ID
    assert!(!valid_job_id("job\n123"));

    // Overlength ID
    assert!(!valid_job_id(&"a".repeat(65)));

    // Positive test: valid IDs are accepted
    assert!(valid_job_id("abc123"));
    assert!(valid_job_id("a1b2c3d4e5f6"));
    assert!(valid_job_id("job_ID-123"));

    // HTTP-level test: cancel a non-existent but valid-format ID returns 404 (not 400)
    // This proves the valid_job_id check passes and the handler proceeds to DB lookup
    let response = router(state.clone())
        .oneshot(
            Request::post("/api/cancel/validbutmissing123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
