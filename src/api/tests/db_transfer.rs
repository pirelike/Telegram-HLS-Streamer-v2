use super::super::*;
use super::common::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::time::Duration;
use tower::ServiceExt;

#[tokio::test]
async fn db_export_import_downloads_sqlite_and_merges_file() {
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
    assert_eq!(&export_bytes[..16], b"SQLite format 3\0");

    let target = app_state();
    let (content_type, body) =
        multipart_body(&[("database", Some("export.db"), export_bytes.clone())]);
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
    assert_eq!(body["merged_segment_parts"], 0);
    let conn = target.db_conn().await.unwrap();
    assert_eq!(
        db::get_segment(&conn, "job1", "video_0/video_0001.m4s")
            .unwrap()
            .unwrap()
            .bot_index,
        0
    );
    drop(conn);

    let (content_type, body) = multipart_body(&[("database", Some("export.db"), export_bytes)]);
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
    assert_eq!(body["merged_jobs"], 0);
    assert_eq!(body["merged_segments"], 0);
    assert_eq!(body["merged_segment_parts"], 0);
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
