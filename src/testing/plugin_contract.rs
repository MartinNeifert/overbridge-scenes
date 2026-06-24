//! Overbridge / VST3 contract tests on FakePlugin (no .vst3 bundle required).
//!
//! These assert host-facing behavior the real Analog Rytm VST3 must satisfy:
//! parameter table, preset state, fingerprinting, and audio bus layout.

use std::sync::Arc;

use crossbeam_channel::unbounded;
use parking_lot::Mutex;
use truce_rack_vst3::{set_editor_open_notifier, set_param_change_notifier, set_param_refresh_notifier};

use crate::host::fake_plugin::FakePlugin;
use crate::host::param_sync::plugin_state_fingerprint;
use crate::host::plugin_backend::{PluginInstance, SharedPlugin};
use crate::host::test_params;
use crate::host::PluginHost;

fn start_test_host() -> PluginHost {
    let (editor_open_tx, editor_open_rx) = unbounded();
    let (param_change_tx, param_change_rx) = unbounded();
    let (param_refresh_tx, param_refresh_rx) = unbounded();
    set_editor_open_notifier(editor_open_tx);
    set_param_change_notifier(param_change_tx);
    set_param_refresh_notifier(param_refresh_tx);
    PluginHost::start_fake(editor_open_rx, param_change_rx, param_refresh_rx)
        .expect("start fake host")
}

#[test]
fn fake_plugin_exposes_stable_parameter_table() {
    let table = test_params::parameter_table();
    assert_eq!(table.len(), 9);
    assert_eq!(table[0].id, test_params::PARAM_FILTER_CUTOFF);
    assert_eq!(table[0].name, "Filter Cutoff");
    assert_eq!(table[0].default, 0.5);
    assert_eq!(table[8].id, test_params::PARAM_LOAD_PRESET);
    assert_eq!(table[8].name, "Load Preset");

    let plugin = FakePlugin::new();
    assert_eq!(plugin.parameter_count(), 9);
    let info = plugin.parameter_info(0).expect("param 0");
    assert_eq!(info.name, "Filter Cutoff");
}

#[test]
fn shared_plugin_vst3_mut_rejects_fake_backend() {
    let shared: SharedPlugin = Arc::new(Mutex::new(PluginInstance::Fake(FakePlugin::new())));
    let mut guard = shared.lock();
    assert!(guard.vst3_mut().is_err());
}

#[test]
fn push_component_state_returns_not_implemented_on_fake() {
    let mut plugin = FakePlugin::new();
    let bytes = plugin.save_state().expect("save state");
    let status = plugin.push_component_state_to_controller(&bytes);
    assert_eq!(status, 3, "kNotImplemented");
}

#[test]
fn host_parameter_cache_tracks_plugin_writes() {
    let host = start_test_host();
    host.set_parameter(0, 0.25).expect("set 0");
    host.set_parameter(1, 0.75).expect("set 1");
    assert!((host.get_parameter(0).unwrap().value - 0.25).abs() < 1e-6);
    assert!((host.get_parameter(1).unwrap().value - 0.75).abs() < 1e-6);
}

#[test]
fn preset_load_changes_plugin_state_fingerprint() {
    let host = start_test_host();
    let shared = host.shared_plugin();

    let before = {
        let guard = shared.lock();
        plugin_state_fingerprint(&guard).expect("fingerprint")
    };

    {
        let mut guard = shared.lock();
        guard.fake_mut().expect("fake").load_preset(1);
    }

    let after = {
        let guard = shared.lock();
        plugin_state_fingerprint(&guard).expect("fingerprint")
    };

    assert_ne!(before, after);
}

#[test]
fn audio_bus_description_matches_stereo_in_out() {
    let plugin = FakePlugin::new();
    let buses = plugin.describe_audio_buses();
    assert_eq!(buses.len(), 2);
    assert_eq!(buses[0], ("Input".to_string(), true, 2));
    assert_eq!(buses[1], ("Output".to_string(), false, 2));
}

#[test]
fn plugin_instance_dispatches_to_fake() {
    let mut instance = PluginInstance::Fake(FakePlugin::new());
    assert_eq!(instance.parameter_count(), 9);
    instance.set_parameter(3, 0.42).expect("set");
    assert!((instance.parameter_value(3).expect("get") - 0.42).abs() < 1e-6);
}
