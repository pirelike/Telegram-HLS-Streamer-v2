use super::super::*;
use super::common::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

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
    assert!(state.config.read().await.disk_cache_enabled);
    let file_settings = body["categories"]["file_handling"]["settings"]
        .as_array()
        .unwrap();
    assert!(file_settings
        .iter()
        .any(|s| s["key"] == "DISK_CACHE_ENABLED" && s["value"] == true));
    assert!(file_settings
        .iter()
        .any(|s| s["key"] == "AUDIO_SEGMENT_DURATION"));

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
async fn settings_save_treats_masked_tmdb_api_key_as_unchanged() {
    let state = app_state();
    {
        let conn = state.db_conn().await.unwrap();
        db::set_setting(&conn, "TMDB_API_KEY", "abcdefghijklmnopqrstuvwxyz").unwrap();
        *state.config.write().await = Arc::new(Config::load(&conn).unwrap());
    }

    assert_eq!(state.config.read().await.masked_tmdb_api_key(), "ab...yz");
    let response = router(state.clone())
        .oneshot(
            Request::post("/api/settings")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"TMDB_API_KEY":"ab...yz","MAX_CONCURRENT_JOBS":4}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cfg = state.config.read().await;
    assert_eq!(cfg.tmdb_api_key, "abcdefghijklmnopqrstuvwxyz");
    assert_eq!(cfg.max_concurrent_jobs, 4);
    drop(cfg);

    let conn = state.db_conn().await.unwrap();
    assert_eq!(
        db::get_all_settings(&conn)
            .unwrap()
            .get("TMDB_API_KEY")
            .unwrap(),
        "abcdefghijklmnopqrstuvwxyz"
    );
    let env = std::fs::read_to_string(&state.env_path).unwrap();
    assert!(env.contains("MAX_CONCURRENT_JOBS=4"));
    assert!(!env.contains("TMDB_API_KEY=ab...yz"));
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
    // Settings update reloads from DB — picks up any bot changes since last load
    assert_eq!(cfg.bots.len(), env_bot_count + 1);
}

#[tokio::test]
async fn settings_reset_reloads_bot_pool() {
    let state = app_state();
    let env_bot_count;
    {
        let conn = state.db_conn().await.unwrap();
        db::add_bot(
            &conn,
            "32345678:abcdefghijklmnopqrstuvwxyzabcdefghi",
            -100,
            "first",
        )
        .unwrap();
        *state.config.write().await = Arc::new(Config::load(&conn).unwrap());
        env_bot_count = state.config.read().await.bots.len();
        db::add_bot(
            &conn,
            "42345678:abcdefghijklmnopqrstuvwxyzabcdefghi",
            -101,
            "second",
        )
        .unwrap();
    }

    let response = router(state.clone())
        .oneshot(
            Request::post("/api/settings/reset")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"keys":["MAX_CONCURRENT_JOBS"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.config.read().await.bots.len(), env_bot_count + 1);
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
