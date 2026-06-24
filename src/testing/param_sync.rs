//! Parameter sync / fingerprint tests.

use crossbeam_channel::unbounded;
use truce_rack_vst3::{set_editor_open_notifier, set_param_change_notifier, set_param_refresh_notifier};

use crate::host::param_sync::plugin_state_fingerprint;
use crate::host::PluginHost;

#[test]
fn fingerprint_changes_on_preset_load() {
    let (editor_open_tx, editor_open_rx) = unbounded();
    let (param_change_tx, param_change_rx) = unbounded();
    let (param_refresh_tx, param_refresh_rx) = unbounded();
    set_editor_open_notifier(editor_open_tx);
    set_param_change_notifier(param_change_tx);
    set_param_refresh_notifier(param_refresh_tx);

    let host = PluginHost::start_fake(editor_open_rx, param_change_rx, param_refresh_rx)
        .expect("start fake host");

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
