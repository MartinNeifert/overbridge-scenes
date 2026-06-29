//! HTTP API tests — the control surface scenes/crossfader depend on.

use std::sync::Arc;

use crate::test_support::{fake_app_state, get_json, post_json, pump_runloop, test_router};

#[tokio::test(flavor = "current_thread")]
async fn status_reports_fake_plugin() {
    let state = fake_app_state().expect("fake app state");
    let app = test_router(state);

    let (status, json) = get_json(&app, "/api/status").await;
    assert!(status.is_success());
    assert_eq!(json["plugin"], "OB Test Host");
    assert_eq!(json["parameter_count"], 9);
}

#[tokio::test(flavor = "current_thread")]
async fn list_parameters_includes_stable_ids() {
    let state = fake_app_state().expect("fake app state");
    let app = test_router(state);

    let (status, json) = get_json(&app, "/api/parameters").await;
    assert!(status.is_success());
    let params = json.as_array().expect("parameter array");
    assert_eq!(params.len(), 9);

    let cutoff = params
        .iter()
        .find(|p| p["name"] == "Filter Cutoff")
        .expect("Filter Cutoff");
    assert_eq!(cutoff["id"], 1);
    assert_eq!(cutoff["index"], 0);
}

#[tokio::test(flavor = "current_thread")]
async fn parameter_roundtrip_by_index() {
    let state = fake_app_state().expect("fake app state");
    let app = test_router(state);

    let post = post_json(&app, "/api/parameters/0", r#"{"value":0.75}"#).await;
    assert!(post.is_success());

    let (status, json) = get_json(&app, "/api/parameters/0").await;
    assert!(status.is_success());
    let value = json["value"].as_f64().unwrap();
    assert!((value - 0.75).abs() < 1e-6, "expected 0.75, got {value}");
}

#[tokio::test(flavor = "current_thread")]
async fn parameter_set_by_name() {
    let state = fake_app_state().expect("fake app state");
    let app = test_router(Arc::clone(&state));

    let post = post_json(
        &app,
        "/api/parameters/by-name",
        r#"{"name":"Drive","value":0.42}"#,
    )
    .await;
    assert!(post.is_success());

    let drive = state
        .host()
        .find_parameter_by_name("Drive")
        .expect("Drive param");
    assert!((drive.value - 0.42).abs() < 1e-6);
}

#[tokio::test(flavor = "current_thread")]
async fn batch_morph_applies_all_parameters() {
    let state = fake_app_state().expect("fake app state");
    let app = test_router(Arc::clone(&state));

    let post = post_json(
        &app,
        "/api/parameters/batch",
        r#"{"updates":[{"index":0,"value":0.1},{"index":1,"value":0.9},{"index":2,"value":0.55}]}"#,
    )
    .await;
    assert!(post.is_success());

    let cutoff = state
        .host()
        .get_parameter(0)
        .expect("cutoff index 0");
    let reso = state.host().get_parameter(1).expect("reso index 1");
    let drive = state.host().get_parameter(2).expect("drive index 2");

    assert!((cutoff.value - 0.1).abs() < 1e-6);
    assert!((reso.value - 0.9).abs() < 1e-6);
    assert!((drive.value - 0.55).abs() < 1e-6);
}

#[tokio::test(flavor = "current_thread")]
async fn hardware_edit_updates_cache_and_epoch() {
    let state = fake_app_state().expect("fake app state");
    let epoch_before = state.host().param_epoch();

    state
        .host()
        .inject_hardware_edit(crate::host::test_params::PARAM_SIM_KNOB, 0.33);

    let knob = state
        .host()
        .find_parameter_by_name("Sim Knob")
        .expect("sim knob");
    assert!((knob.value - 0.33).abs() < 1e-6);
    assert!(state.host().param_epoch() > epoch_before);

    let pending = state.host().take_pending_ws_updates();
    assert!(!pending.is_empty(), "expected WS delta for hardware edit");
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

#[tokio::test(flavor = "current_thread")]
async fn host_edit_not_treated_as_preset_load() {
    let state = fake_app_state().expect("fake app state");

    state
        .host()
        .set_parameter(0, 0.88)
        .expect("host set cutoff");

    pump_runloop(&state, 30);

    let cutoff = state
        .host()
        .find_parameter_by_name("Filter Cutoff")
        .expect("cutoff");
    assert!(
        (cutoff.value - 0.88).abs() < 1e-6,
        "host edit should not be overwritten by preset refresh, got {}",
        cutoff.value
    );
}

#[tokio::test(flavor = "current_thread")]
async fn crossfader_apply_morphs_via_http() {
    use std::sync::Arc;

    let dir = std::env::temp_dir().join(format!("ob-xf-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let state = crate::test_support::fake_app_state_with_scenes_dir(dir.clone())
        .expect("app state");
    let app = test_router(Arc::clone(&state));

    let payload = serde_json::json!({
        "scenes": [
            {"id":"1","name":"A","params":[{"index":0,"id":1,"name":"Filter Cutoff","value":0.0}]},
            {"id":"2","name":"B","params":[{"index":0,"id":1,"name":"Filter Cutoff","value":1.0}]}
        ],
        "crossfader": {"a":"1","b":"2","pos":0.0},
        "baseline": {"explicit":false,"values":[]}
    });
    state
        .scenes_store()
        .save("OB Test Host", "A01", &payload)
        .unwrap();

    let post = post_json(
        &app,
        "/api/crossfader/apply",
        r#"{"pattern":"A01","pos":0.6}"#,
    )
    .await;
    assert!(post.is_success());

    let cutoff = state.host().get_parameter(0).expect("cutoff");
    assert!((cutoff.value - 0.6).abs() < 1e-6);

    let _ = std::fs::remove_dir_all(&dir);
}
