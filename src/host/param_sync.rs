use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use parking_lot::RwLock;
use truce_rack_vst3::{hardware_edit_active, recent_hardware_values};

use crate::host::plugin_backend::PluginInstance;
use crate::host::plugin_host::ParameterSnapshot;

const VALUE_EPSILON: f64 = 1e-5;

pub type ParamWsUpdate = (usize, f64, String);

/// Refresh one entry in the host parameter cache from the plugin.
pub fn update_param_snapshot(
    plugin: &mut PluginInstance,
    parameters: &Arc<RwLock<Vec<ParameterSnapshot>>>,
    index: usize,
) {
    let mut params = parameters.write();
    if let Some(snap) = params.get_mut(index) {
        if let Ok(value) = plugin.parameter_value(index) {
            snap.value = value;
            snap.display = plugin
                .parameter_value_string(index, value)
                .unwrap_or_else(|_| format!("{value:.4}"));
        }
    }
}

/// Hash of component/plugin state — preset/settings loads change this blob.
pub fn plugin_state_fingerprint(plugin: &PluginInstance) -> Option<u64> {
    let component = plugin.save_state().ok()?;
    let mut hasher = DefaultHasher::new();
    component.hash(&mut hasher);
    Some(hasher.finish())
}

/// Read parameter values into the host cache. During hardware knob moves,
/// `performEdit` values take precedence over stale controller reads.
pub fn sync_params_from_plugin(
    plugin: &mut PluginInstance,
    parameters: &Arc<RwLock<Vec<ParameterSnapshot>>>,
    force: bool,
    pending_ws: Option<&Mutex<Vec<ParamWsUpdate>>>,
) -> usize {
    let hw = recent_hardware_values();
    let hw_active = hardware_edit_active(Duration::from_millis(800));

    let mut params = parameters.write();
    let mut changed = 0usize;
    for (i, snap) in params.iter_mut().enumerate() {
        let (value, from_hw) = if hw_active {
            if let Some(&hw_val) = hw.get(&snap.id) {
                (hw_val, true)
            } else if let Ok(v) = plugin.parameter_value(i) {
                (v, false)
            } else {
                continue;
            }
        } else if let Ok(v) = plugin.parameter_value(i) {
            (v, false)
        } else {
            continue;
        };

        let diff = (value - snap.value).abs() > VALUE_EPSILON;
        if force || diff {
            if diff {
                changed += 1;
            }
            snap.value = value;
            snap.display = if from_hw {
                format!("{value:.4}")
            } else {
                plugin
                    .parameter_value_string(i, value)
                    .unwrap_or_else(|_| format!("{value:.4}"))
            };
            if diff {
                if let Some(pending) = pending_ws {
                    pending.lock().push((i, snap.value, snap.display.clone()));
                }
            }
        }
    }
    changed
}

pub fn log_refresh_outcome(changed: usize, final_attempt: bool) {
    if !final_attempt {
        return;
    }
    if changed > 0 {
        tracing::info!("Plugin parameters refreshed ({changed} changed)");
    } else if !hardware_edit_active(Duration::from_millis(800)) {
        tracing::debug!(
            "Component state changed but no parameter values diverged \
             (non-parameter state, or already in sync)"
        );
    }
}
