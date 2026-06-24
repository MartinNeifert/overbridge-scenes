//! Direct [`PluginHost`] tests — batch coalescing and plugin backend wiring.

use crossbeam_channel::unbounded;
use truce_rack_vst3::{set_editor_open_notifier, set_param_change_notifier, set_param_refresh_notifier};

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
fn batch_write_updates_all_params_atomically() {
    let host = start_test_host();
    host.set_parameters_batch(&[(0, 0.2), (1, 0.8), (3, 0.5)])
        .expect("batch set");

    assert!((host.get_parameter(0).unwrap().value - 0.2).abs() < 1e-6);
    assert!((host.get_parameter(1).unwrap().value - 0.8).abs() < 1e-6);
    assert!((host.get_parameter(3).unwrap().value - 0.5).abs() < 1e-6);
}

#[test]
fn set_parameter_by_name_resolves_case_insensitive() {
    let host = start_test_host();
    host.set_parameter_by_name("drive", 0.77)
        .expect("set by name");

    let drive = host.find_parameter_by_name("Drive").expect("Drive");
    assert!((drive.value - 0.77).abs() < 1e-6);
}

#[test]
fn control_only_clears_pending_process_delivery() {
    let host = start_test_host();
    host.set_parameter(0, 0.5).expect("set");

    let shared = host.shared_plugin();
    let guard = shared.lock();
    let fake = guard.fake().expect("fake");
    assert_eq!(
        fake.pending_process_delivery_count(),
        0,
        "control-only should clear pending IParameterChanges"
    );
}
