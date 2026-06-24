//! Unit tests for the in-process fake plugin.

use crate::host::fake_plugin::FakePlugin;
use crate::host::test_params::{self, PARAM_FILTER_CUTOFF, PARAM_LOAD_PRESET};

#[test]
fn preset_changes_state_blob() {
    let mut fake = FakePlugin::new();
    let before = fake.save_state().expect("save state");
    fake.load_preset(3);
    let after = fake.save_state().expect("save state");
    assert_ne!(before, after);
    assert_eq!(after[7], 3, "preset byte in state blob");
}

#[test]
fn preset_updates_filter_cutoff_in_plugin() {
    let mut fake = FakePlugin::new();
    fake.load_preset(1);
    let idx = test_params::index_of(PARAM_FILTER_CUTOFF).unwrap();
    let value = fake.parameter_value(idx).unwrap();
    assert!((value - test_params::preset_filter_cutoff(1)).abs() < 1e-6);
}

#[test]
fn set_load_preset_param_applies_preset() {
    let mut fake = FakePlugin::new();
    let preset_idx = test_params::index_of(PARAM_LOAD_PRESET).unwrap();
    fake.set_parameter(preset_idx, 0.75)
        .expect("set preset param to preset 3");

    let cutoff_idx = test_params::index_of(PARAM_FILTER_CUTOFF).unwrap();
    let value = fake.parameter_value(cutoff_idx).unwrap();
    assert!((value - test_params::preset_filter_cutoff(3)).abs() < 1e-6);
}

#[test]
fn push_component_state_refreshes_from_blob() {
    let mut fake = FakePlugin::new();
    fake.load_preset(2);
    let bytes = fake.save_state().expect("save");

    let mut other = FakePlugin::new();
    let status = other.push_component_state_to_controller(&bytes);
    assert_eq!(status, 3, "mirrors Overbridge kNotImplemented return");

    let idx = test_params::index_of(PARAM_FILTER_CUTOFF).unwrap();
    let value = other.parameter_value(idx).unwrap();
    assert!((value - test_params::preset_filter_cutoff(2)).abs() < 1e-6);
}
