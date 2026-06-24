//! HTTP scenes persistence API tests.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::test_support::{fake_app_state_with_scenes_dir, test_router};

fn temp_scenes_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "ob-scenes-api-test-{}",
        std::process::id()
    ))
}

async fn put_body(app: &axum::Router, uri: &str, body: &str) -> StatusCode {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    response.status()
}

async fn get_status_body(app: &axum::Router, uri: &str) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

#[tokio::test(flavor = "current_thread")]
async fn scenes_roundtrip_via_http() {
    let dir = temp_scenes_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let state = fake_app_state_with_scenes_dir(dir.clone()).expect("app state");
    let app = test_router(Arc::clone(&state));

    let payload = r#"{
        "scenes": [
            {"id":"1","name":"Scene 1","params":[{"index":0,"id":1,"name":"Filter Cutoff","value":0.25}]}
        ],
        "crossfader": {"a":"1","b":null},
        "baseline": {"explicit":true,"values":[{"index":0,"id":1,"value":0.5}]}
    }"#;

    let put = put_body(&app, "/api/scenes/OB%20Test%20Host/A01", payload).await;
    assert_eq!(put, StatusCode::NO_CONTENT);

    let (status, body) = get_status_body(&app, "/api/scenes/OB%20Test%20Host/A01").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["scenes"][0]["params"][0]["value"], 0.25);
    assert_eq!(json["crossfader"]["a"], "1");
    assert_eq!(json["baseline"]["explicit"], true);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "current_thread")]
async fn scenes_missing_returns_404() {
    let dir = temp_scenes_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let state = fake_app_state_with_scenes_dir(dir.clone()).expect("app state");
    let app = test_router(state);

    let (status, _) = get_status_body(&app, "/api/scenes/OB%20Test%20Host/Z99").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "current_thread")]
async fn active_pattern_roundtrip_via_http() {
    let dir = temp_scenes_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let state = fake_app_state_with_scenes_dir(dir.clone()).expect("app state");
    let app = test_router(Arc::clone(&state));

    let put = put_body(
        &app,
        "/api/scenes/OB%20Test%20Host/active",
        r#"{"pattern":"B05"}"#,
    )
    .await;
    assert_eq!(put, StatusCode::NO_CONTENT);

    let (status, body) = get_status_body(&app, "/api/scenes/OB%20Test%20Host/active").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["pattern"], "B05");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "current_thread")]
async fn active_pattern_rejects_empty() {
    let dir = temp_scenes_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let state = fake_app_state_with_scenes_dir(dir.clone()).expect("app state");
    let app = test_router(state);

    let put = put_body(
        &app,
        "/api/scenes/OB%20Test%20Host/active",
        r#"{"pattern":"  "}"#,
    )
    .await;
    assert_eq!(put, StatusCode::BAD_REQUEST);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "current_thread")]
async fn scenes_store_survives_host_restart() {
    let dir = temp_scenes_dir();
    let _ = std::fs::remove_dir_all(&dir);

    {
        let state = fake_app_state_with_scenes_dir(dir.clone()).expect("app state");
        let app = test_router(state);
        let put = put_body(
            &app,
            "/api/scenes/OB%20Test%20Host/C03",
            r#"{"scenes":[],"crossfader":{"a":null,"b":null},"baseline":{"explicit":false,"values":[]}}"#,
        )
        .await;
        assert_eq!(put, StatusCode::NO_CONTENT);
    }

    {
        let state = fake_app_state_with_scenes_dir(dir.clone()).expect("app state");
        let app = test_router(state);
        let (status, body) = get_status_body(&app, "/api/scenes/OB%20Test%20Host/C03").await;
        assert_eq!(status, StatusCode::OK);
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["crossfader"].is_object());
    }

    let _ = std::fs::remove_dir_all(&dir);
}
