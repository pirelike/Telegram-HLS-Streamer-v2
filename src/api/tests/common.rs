use super::super::*;
use axum::body::{to_bytes, Body};
use axum::extract::Path as AxumPath;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response as AxumResponse};
use axum::routing::any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_stream::wrappers::ReceiverStream;
use tower::ServiceExt;

pub(super) fn app_state() -> Arc<AppState> {
    app_state_with_telegram_base(crate::telegram::DEFAULT_API_BASE.to_string())
}

pub(super) fn app_state_with_telegram_base(telegram_base_url: String) -> Arc<AppState> {
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
        db_sync_lock: Mutex::new(()),
        jobs: Mutex::new(HashMap::new()),
        played_segments: Mutex::new(HashMap::new()),
        job_queue,
        cache: Arc::new(SegmentCache::new(64 * 1024 * 1024)),
        ffmpeg_available: true,
        ffprobe_available: true,
        selected_encoder: RwLock::new(crate::media::cpu_encoder()),
        last_bot_index: std::sync::atomic::AtomicI64::new(0),
        shutdown_token: tokio_util::sync::CancellationToken::new(),
        ingest_download_semaphore: Arc::new(tokio::sync::Semaphore::new(5)),
    });
    start_background_tasks(state.clone(), job_receiver);
    state
}

pub(super) async fn json_response(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

pub(super) async fn exhaust_db_pool(state: &Arc<AppState>) -> crate::db::DbConn {
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

pub(super) async fn response_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

pub(super) fn multipart_body(fields: &[(&str, Option<&str>, Vec<u8>)]) -> (String, Vec<u8>) {
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

pub(super) struct FakeWebhook {
    pub(super) hits: AtomicUsize,
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

pub(super) async fn fake_webhook_server() -> (String, Arc<FakeWebhook>) {
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

pub(super) async fn fake_telegram_server() -> String {
    let app = axum::Router::new().route("/*path", any(fake_telegram_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

pub(super) struct FakeTelegramBytes {
    pub(super) bytes: Vec<u8>,
}

async fn fake_telegram_bytes_handler(
    State(fake): State<Arc<FakeTelegramBytes>>,
    AxumPath(path): AxumPath<String>,
) -> AxumResponse {
    if path.ends_with("/getFile") {
        return Json(json!({
            "ok": true,
            "result": { "file_path": "payload.bin" }
        }))
        .into_response();
    }
    if path.contains("file/bot") {
        return fake.bytes.clone().into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

pub(super) async fn fake_telegram_bytes_server(bytes: Vec<u8>) -> String {
    let fake = Arc::new(FakeTelegramBytes { bytes });
    let app = axum::Router::new()
        .route("/*path", any(fake_telegram_bytes_handler))
        .with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

pub(super) struct FakeTelegramHangOnce {
    pub(super) file_gets: AtomicUsize,
    pub(super) held_senders:
        Mutex<Vec<tokio::sync::mpsc::Sender<Result<axum::body::Bytes, std::io::Error>>>>,
}

async fn fake_telegram_hang_once_handler(
    State(fake): State<Arc<FakeTelegramHangOnce>>,
    AxumPath(path): AxumPath<String>,
) -> AxumResponse {
    if path.ends_with("/getFile") {
        return Json(json!({
            "ok": true,
            "result": { "file_path": "payload.bin" }
        }))
        .into_response();
    }
    if path.contains("file/bot") {
        let n = fake.file_gets.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            let (tx, rx) =
                tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(1);
            fake.held_senders.lock().await.push(tx);
            return (StatusCode::OK, Body::from_stream(ReceiverStream::new(rx))).into_response();
        }
        return b"retry segment bytes".as_slice().into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

pub(super) async fn fake_telegram_hang_once_server() -> (String, Arc<FakeTelegramHangOnce>) {
    let fake = Arc::new(FakeTelegramHangOnce {
        file_gets: AtomicUsize::new(0),
        held_senders: Mutex::new(Vec::new()),
    });
    let app = axum::Router::new()
        .route("/*path", any(fake_telegram_hang_once_handler))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), fake)
}

pub(super) async fn init_upload(state: Arc<AppState>, filename: &str, total_size: u64) -> Value {
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

pub(super) async fn save_complete_job(
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
        encryption_nonce: None,
    }];
    db::save_job(&mut conn, &job, &tracks, &segments, &[]).unwrap();
}
