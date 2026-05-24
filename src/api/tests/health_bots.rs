use super::super::*;
use super::common::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

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
