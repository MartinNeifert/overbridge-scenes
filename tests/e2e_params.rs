//! In-process HTTP e2e tests against the fake plugin backend.

use std::path::PathBuf;

use axum::body::Body;
use http_body_util::BodyExt;
use overbridge_host::api;
use overbridge_host::test_support::{fake_app_state, pump_runloop};
use tower::ServiceExt;

#[tokio::test(flavor = "current_thread")]
async fn status_reports_fake_plugin() {
    let state = fake_app_state().expect("fake app state");
    let app = api::router(state, PathBuf::from("web"));

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_success());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["plugin"], "OB Test Host");
    assert_eq!(json["parameter_count"], 9);
}

#[tokio::test(flavor = "current_thread")]
async fn parameter_roundtrip_by_index() {
    let state = fake_app_state().expect("fake app state");
    let app = api::router(state.clone(), PathBuf::from("web"));

    let post = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/parameters/0")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"value":0.75}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(post.status().is_success());

    let get = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/parameters/0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(get.status().is_success());
    let body = get.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let value = json["value"].as_f64().unwrap();
    assert!((value - 0.75).abs() < 1e-6, "expected 0.75, got {value}");
}

#[tokio::test(flavor = "current_thread")]
async fn parameter_by_name_and_batch() {
    let state = fake_app_state().expect("fake app state");
    let app = api::router(state, PathBuf::from("web"));

    let by_name = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/parameters/by-name")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Drive","value":0.42}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(by_name.status().is_success());

    let batch = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/parameters/batch")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"updates":[{"index":0,"value":0.1},{"index":1,"value":0.9}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(batch.status().is_success());
}

#[tokio::test(flavor = "current_thread")]
async fn perform_edit_reaches_cache() {
    let state = fake_app_state().expect("fake app state");
    let epoch_before = state.host().param_epoch();

    {
        let host = state.host();
        let shared = host.shared_plugin();
        let mut guard = shared.lock();
        guard
            .fake_mut()
            .expect("fake")
            .fire_perform_edit(overbridge_host::host::test_params::PARAM_SIM_KNOB, 0.33);
    }

    pump_runloop(&state, 4);

    let knob = state
        .host()
        .find_parameter_by_name("Sim Knob")
        .expect("sim knob");
    assert!((knob.value - 0.33).abs() < 1e-6);
    assert!(state.host().param_epoch() > epoch_before);
}

#[tokio::test(flavor = "current_thread")]
async fn preset_load_updates_filter_cutoff() {
    let state = fake_app_state().expect("fake app state");

    {
        let host = state.host();
        let shared = host.shared_plugin();
        let mut guard = shared.lock();
        guard.fake_mut().expect("fake").load_preset(2);
    }

    pump_runloop(&state, 80);

    let cutoff = state
        .host()
        .find_parameter_by_name("Filter Cutoff")
        .expect("cutoff");
    assert!(
        (cutoff.value - 0.65).abs() < 1e-6,
        "expected preset 2 cutoff 0.65, got {}",
        cutoff.value
    );
}
