use axum::{
    Json, Router,
    extract::{Path, State, WebSocketUpgrade, ws::{Message, WebSocket}},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use axum::http::{header, HeaderValue};

use crate::crossfader::{self, CrossfaderApplyRequest, CrossfaderApplyResponse};
use crate::devices;
use crate::host::ParameterSnapshot;
use crate::match_devices;
use crate::net_util;
use crate::state::{
    AppState, MidiCcRequest, MidiNoteRequest, RawMidiRequest, SelectPluginRequest,
    SetParameterByNameRequest, SetParameterRequest, BatchSetParametersRequest, SharedState, StatusResponse,
};

pub fn router(state: SharedState, web_dir: PathBuf) -> Router {
    let api = Router::new()
        .route("/api/status", get(status))
        .route("/api/devices", get(devices))
        .route("/api/selector", get(selector))
        .route("/api/select-plugin", post(select_plugin))
        .route("/api/parameters", get(list_parameters))
        .route("/api/parameters/batch", post(set_parameters_batch))
        .route("/api/crossfader/apply", post(crossfader_apply))
        .route("/api/parameters/{index}", get(get_parameter))
        .route("/api/parameters/{index}", post(set_parameter))
        .route("/api/parameters/by-name", post(set_parameter_by_name))
        .route("/api/midi/note", post(midi_note))
        .route("/api/midi/cc", post(midi_cc))
        .route("/api/midi/raw", post(midi_raw))
        .route("/api/midi/inputs", get(list_midi_inputs))
        .route("/api/scenes/{plugin}/active", get(get_active_pattern).put(put_active_pattern))
        .route("/api/scenes/{plugin}/{pattern}", get(get_scenes))
        .route("/api/scenes/{plugin}/{pattern}", put(put_scenes))
        .route("/api/ws", get(ws_handler))
        .with_state(state.clone());

    Router::new()
        .merge(api)
        .fallback_service(ServeDir::new(web_dir))
        // Web assets change between runs; tell browsers to always revalidate so
        // a rebuilt scenes.js/html is never served from a stale disk cache.
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        ))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

async fn status(State(state): State<SharedState>) -> Json<StatusResponse> {
    // Device discovery shells out to external tools; keep it off the async
    // workers so concurrent polls + the WebSocket loop don't get starved.
    let snapshot = tokio::task::spawn_blocking(devices::discover)
        .await
        .unwrap_or_else(|_| devices::discover());

    let host = state.host();
    let plugin = state.plugin_info().name;
    let plugin_matches_device = snapshot.devices.iter().any(|d| {
        match_devices::plugin_name_matches_device(&plugin, &d.name)
    });

    Json(StatusResponse {
        plugin: plugin.clone(),
        vendor: host.plugin_info().vendor.clone(),
        parameter_count: host.parameters().len(),
        engine_running: snapshot.engine_running,
        api_version: "0.1.0",
        audio_device: plugin.clone(),
        audio_channels: 0,
        devices: snapshot.devices,
        plugin_matches_device,
        debug: state.debug(),
        api_port: state.config().api_port,
        lan_ip: net_util::local_lan_ip(),
        lan_hostname: net_util::local_hostname(),
    })
}

async fn selector(State(state): State<SharedState>) -> Json<crate::state::SelectorResponse> {
    let options = tokio::task::spawn_blocking(move || state.selector_options())
        .await
        .expect("selector_options panicked");
    Json(options)
}

async fn select_plugin(
    State(state): State<SharedState>,
    Json(body): Json<SelectPluginRequest>,
) -> Result<Json<crate::state::SelectorResponse>, StatusCode> {
    let state = state.clone();
    let plugin = body.plugin;
    let options = tokio::task::spawn_blocking(move || state.switch_plugin(&plugin))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            tracing::error!("select plugin failed: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(options))
}

async fn devices(State(_state): State<SharedState>) -> Json<devices::DevicesSnapshot> {
    let snapshot = tokio::task::spawn_blocking(devices::discover)
        .await
        .unwrap_or_else(|_| devices::discover());
    Json(snapshot)
}

async fn list_parameters(State(state): State<SharedState>) -> Json<Vec<ParameterSnapshot>> {
    Json(state.host().parameters())
}

async fn get_parameter(
    State(state): State<SharedState>,
    Path(index): Path<usize>,
) -> Result<Json<ParameterSnapshot>, StatusCode> {
    state
        .host()
        .get_parameter(index)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn set_parameter(
    State(state): State<SharedState>,
    Path(index): Path<usize>,
    Json(body): Json<SetParameterRequest>,
) -> Result<Json<ParameterSnapshot>, StatusCode> {
    state
        .host()
        .set_parameter(index, body.value)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    state
        .host()
        .get_parameter(index)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn set_parameter_by_name(
    State(state): State<SharedState>,
    Json(body): Json<SetParameterByNameRequest>,
) -> Result<Json<ParameterSnapshot>, StatusCode> {
    state
        .host()
        .set_parameter_by_name(&body.name, body.value)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    state
        .host()
        .find_parameter_by_name(&body.name)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn crossfader_apply(
    State(state): State<SharedState>,
    Json(body): Json<CrossfaderApplyRequest>,
) -> Result<Json<CrossfaderApplyResponse>, StatusCode> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || crossfader::apply_crossfader(&state, &body))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Json)
        .map_err(|e| {
            tracing::error!("crossfader apply failed: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn set_parameters_batch(
    State(state): State<SharedState>,
    Json(body): Json<BatchSetParametersRequest>,
) -> Result<StatusCode, StatusCode> {
    let updates: Vec<(usize, f64)> = body
        .updates
        .into_iter()
        .map(|u| (u.index, u.value))
        .collect();
    state
        .host()
        .set_parameters_batch(&updates)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

async fn midi_note(
    State(state): State<SharedState>,
    Json(body): Json<MidiNoteRequest>,
) -> Result<StatusCode, StatusCode> {
    state
        .host()
        .send_midi_note(body.channel, body.note, body.velocity, body.on)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

async fn midi_cc(
    State(state): State<SharedState>,
    Json(body): Json<MidiCcRequest>,
) -> Result<StatusCode, StatusCode> {
    state
        .host()
        .send_midi_cc(body.channel, body.controller, body.value)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

async fn midi_raw(
    State(state): State<SharedState>,
    Json(body): Json<RawMidiRequest>,
) -> Result<StatusCode, StatusCode> {
    state
        .host()
        .send_raw_midi(body.data)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
}

async fn list_midi_inputs(State(state): State<SharedState>) -> Json<Vec<crate::midi::MidiInputPort>> {
    Json(state.midi_input_ports())
}

#[derive(serde::Serialize)]
struct ActivePatternResponse {
    pattern: Option<String>,
}

#[derive(serde::Deserialize)]
struct ActivePatternRequest {
    pattern: String,
}

async fn get_active_pattern(
    State(state): State<SharedState>,
    Path(plugin): Path<String>,
) -> Result<Json<ActivePatternResponse>, StatusCode> {
    let pattern = tokio::task::spawn_blocking(move || {
        state.scenes_store().load_active_pattern(&plugin)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        tracing::error!("load active pattern failed: {e:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(ActivePatternResponse { pattern }))
}

async fn put_active_pattern(
    State(state): State<SharedState>,
    Path(plugin): Path<String>,
    Json(body): Json<ActivePatternRequest>,
) -> Result<StatusCode, StatusCode> {
    let pattern = body.pattern.trim().to_string();
    if pattern.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let state = state.clone();
    tokio::task::spawn_blocking(move || state.scenes_store().save_active_pattern(&plugin, &pattern))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            tracing::error!("save active pattern failed: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_scenes(
    State(state): State<SharedState>,
    Path((plugin, pattern)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let data = tokio::task::spawn_blocking(move || {
        state.scenes_store().load(&plugin, &pattern)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|e| {
        tracing::error!("load scenes failed: {e:#}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    match data {
        Some(v) => Ok(Json(v)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn put_scenes(
    State(state): State<SharedState>,
    Path((plugin, pattern)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, StatusCode> {
    if !body.is_object() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let state = state.clone();
    tokio::task::spawn_blocking(move || state.scenes_store().save(&plugin, &pattern, &body))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|e| {
            tracing::error!("save scenes failed: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<SharedState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(socket: WebSocket, state: SharedState) {
    let (mut sender, mut receiver) = socket.split();
    let mut midi_rx = state.midi_subscribe();

    let params = state.host().parameters();
    let init = serde_json::json!({ "type": "parameters", "data": params });
    if sender
        .send(Message::Text(init.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    let mut tick = tokio::time::interval(std::time::Duration::from_millis(50));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_epoch = 0u64;
    let mut full_sync_counter = 0u32;

    loop {
        tokio::select! {
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(req) = serde_json::from_str::<WsRequest>(&text) {
                            handle_ws_request(&state, &req);
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            midi = midi_rx.recv() => {
                match midi {
                    Ok(event) => {
                        let msg = serde_json::json!({
                            "type": "midi",
                            "port": event.port,
                            "data": event.data,
                        });
                        if sender.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = tick.tick() => {
                let msg = {
                    let host = state.host();
                    let epoch = host.param_epoch();
                    let updates = host.take_pending_ws_updates();
                    full_sync_counter = full_sync_counter.wrapping_add(1);

                    if !updates.is_empty() {
                        last_epoch = epoch;
                        let data: Vec<_> = updates
                            .into_iter()
                            .map(|(index, value, display)| {
                                serde_json::json!({ "index": index, "value": value, "display": display })
                            })
                            .collect();
                        Some(serde_json::json!({ "type": "param_updates", "data": data }))
                    } else if epoch != last_epoch || full_sync_counter % 40 == 0 {
                        last_epoch = epoch;
                        let params = host.parameters();
                        Some(serde_json::json!({ "type": "parameters", "data": params }))
                    } else {
                        None
                    }
                };

                let Some(msg) = msg else { continue };

                if sender.send(Message::Text(msg.to_string().into())).await.is_err() {
                    break;
                }
            }
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct WsRequest {
    action: String,
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<f64>,
}

fn handle_ws_request(state: &AppState, req: &WsRequest) {
    let host = state.host();
    match req.action.as_str() {
        "set_parameter" => {
            if let (Some(index), Some(value)) = (req.index, req.value) {
                let _ = host.set_parameter(index, value);
            }
        }
        "set_parameter_by_name" => {
            if let (Some(name), Some(value)) = (&req.name, req.value) {
                let _ = host.set_parameter_by_name(name, value);
            }
        }
        _ => {}
    }
}
