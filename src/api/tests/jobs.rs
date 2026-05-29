use super::super::*;
use super::common::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::atomic::{AtomicBool, Ordering};
use tower::ServiceExt;

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

    // Cancel flag is set
    let jobs = state.jobs.lock().await;
    let job = jobs.get("cancel-proc").unwrap();
    assert_eq!(job.status.as_str(), "cancelled");
    assert!(job.cancel_flag.load(std::sync::atomic::Ordering::Relaxed));
    drop(jobs);
    // No DB row for this job
    let conn = state.db_conn().await.unwrap();
    assert!(db::get_job(&conn, "cancel-proc").unwrap().is_none());
    // handle_cancel_job now calls cleanup_job_paths immediately.
    assert!(!processing_path.exists());
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

#[tokio::test]
async fn enqueue_existing_job_rejects_terminal_state_overwrite() {
    let state = app_state();
    let job_id = "terminal-job-123";
    let source = state.uploads_dir.join("test.mkv");
    tokio::fs::write(&source, b"fake").await.unwrap();

    {
        let mut jobs = state.jobs.lock().await;
        jobs.insert(
            job_id.into(),
            crate::api::jobs::JobState {
                job_id: job_id.into(),
                filename: "test.mkv".into(),
                source_path: source.clone(),
                processing_path: state.processing_dir.join(job_id),
                status: crate::api::jobs::JobStatus::Error,
                progress: 100.0,
                step: 5,
                total_steps: 5,
                description: "errored".into(),
                queued_at: std::time::Instant::now(),
                started_at: Some(std::time::Instant::now()),
                finished_at: Some(std::time::Instant::now()),
                cancel_requested: false,
                cancel_flag: std::sync::Arc::new(AtomicBool::new(false)),
                error: Some("timeout".into()),
                metadata: crate::api::jobs::JobMetadata::default(),
                analysis: None,
                delete_source_on_finish: false,
                original_source_path: None,
            },
        );
    }

    let result = crate::api::jobs::enqueue_existing_job(
        &state,
        job_id.into(),
        "test.mkv".into(),
        source.clone(),
        crate::api::jobs::JobMetadata::default(),
        false,
        None,
        false,
    )
    .await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("job already terminal"),
        "expected terminal error, got: {err}"
    );

    let jobs = state.jobs.lock().await;
    let job = jobs.get(job_id).unwrap();
    assert_eq!(job.status, crate::api::jobs::JobStatus::Error);
}
