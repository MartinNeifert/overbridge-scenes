//! Helpers for in-process HTTP e2e tests with the fake plugin backend.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use crossbeam_channel::unbounded;
use truce_rack_core::info::PluginInfo;
use truce_rack_vst3::{set_editor_open_notifier, set_param_change_notifier, set_param_refresh_notifier};

use crate::config::{AppConfig, DuplexConfig, MidiConfig};
use crate::host::PluginHost;
use crate::midi::MapperConfig;
use crate::scenes_store::ScenesStore;
use crate::state::AppState;
use tokio::sync::broadcast;

/// Build an [`AppState`] backed by the in-process fake plugin (control-only).
pub fn fake_app_state() -> Result<Arc<AppState>> {
    let (editor_open_tx, editor_open_rx) = unbounded();
    let (param_change_tx, param_change_rx) = unbounded();
    let (param_refresh_tx, param_refresh_rx) = unbounded();
    set_editor_open_notifier(editor_open_tx);
    set_param_change_notifier(param_change_tx);
    set_param_refresh_notifier(param_refresh_tx);

    let host = PluginHost::start_fake(editor_open_rx, param_change_rx, param_refresh_rx)?;
    let plugin_info = host.plugin_info().clone();

    let config = AppConfig {
        api_port: 0,
        plugin_dir: "plugins".to_string(),
        default_plugin: Some(plugin_info.name.clone()),
        sample_rate: 48_000,
        block_size: 128,
        overbridge_engine: String::new(),
        audio_device: None,
        control_only: true,
        duplex: DuplexConfig::default(),
        midi: MidiConfig {
            enabled: false,
            virtual_port_name: "test".to_string(),
        },
    };

    let (midi_tx, _) = broadcast::channel(16);
    let scenes_store = ScenesStore::new(PathBuf::from("data/scenes"));

    Ok(Arc::new(AppState::new(
        host,
        plugin_info,
        config,
        PathBuf::from("plugins"),
        vec![fake_catalog_entry()],
        MapperConfig::default(),
        None,
        midi_tx,
        None,
        scenes_store,
        false,
    )))
}

fn fake_catalog_entry() -> PluginInfo {
    PluginInfo {
        name: "OB Test Host".to_string(),
        vendor: "Overbridge Scenes".to_string(),
        version: 1,
        category: truce_rack_core::info::PluginCategory::Effect,
        path: PathBuf::from("fake://ob-test-host"),
        unique_id: "FAKEOBTEST".to_string(),
        format: "fake",
        has_editor: false,
        accepts_midi: true,
    }
}

/// Pump the host run loop `ticks` times (4 ms cadence in production).
pub fn pump_runloop(state: &AppState, ticks: usize) {
    for _ in 0..ticks {
        state.host().runloop_tick();
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
}
