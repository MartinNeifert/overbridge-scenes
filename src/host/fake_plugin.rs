//! In-process test plugin for headless parameter e2e on Linux and in `cargo test`.

use anyhow::{Context, Result};
use truce_rack_core::info::{PluginCategory, PluginInfo, ParameterInfo};
use truce_rack_vst3::{note_host_edit, simulate_perform_edit};

use crate::host::test_params::{
    self, PARAM_FILTER_CUTOFF, PARAM_FIRE_PERFORM_EDIT, PARAM_LOAD_PRESET, PARAM_SIM_KNOB,
};

const STATE_MAGIC: &[u8] = b"OBTEST";

#[derive(Debug)]
pub struct FakePlugin {
    info: PluginInfo,
    params: Vec<f64>,
    preset: u8,
    last_fire_perform_edit: f64,
    pending_process_changes: usize,
}

impl FakePlugin {
    pub fn new() -> Self {
        let table = test_params::parameter_table();
        let params: Vec<f64> = table.iter().map(|p| p.default).collect();
        Self {
            info: PluginInfo {
                name: "OB Test Host".to_string(),
                vendor: "Overbridge Scenes".to_string(),
                version: 1,
                category: PluginCategory::Effect,
                path: std::path::PathBuf::from("fake://ob-test-host"),
                unique_id: "FAKEOBTEST".to_string(),
                format: "fake",
                has_editor: false,
                accepts_midi: true,
            },
            params,
            preset: 0,
            last_fire_perform_edit: 0.0,
            pending_process_changes: 0,
        }
    }

    pub fn fire_perform_edit(&mut self, id: u32, value: f64) {
        simulate_perform_edit(id, value.clamp(0.0, 1.0));
        if let Some(idx) = test_params::index_of(id) {
            self.params[idx] = value.clamp(0.0, 1.0);
        }
    }

    pub fn load_preset(&mut self, preset: u8) {
        self.preset = preset % 4;
        if let Some(idx) = test_params::index_of(PARAM_FILTER_CUTOFF) {
            self.params[idx] = test_params::preset_filter_cutoff(self.preset);
        }
        if let Some(idx) = test_params::index_of(PARAM_LOAD_PRESET) {
            self.params[idx] = f64::from(self.preset) / 3.0;
        }
    }

    pub fn pending_process_delivery_count(&self) -> usize {
        self.pending_process_changes
    }
}

impl FakePlugin {
    pub fn info(&self) -> &PluginInfo {
        &self.info
    }

    pub fn parameter_count(&self) -> usize {
        test_params::PARAM_COUNT
    }

    pub fn parameter_info(&self, index: usize) -> Result<ParameterInfo> {
        test_params::parameter_table()
            .get(index)
            .cloned()
            .context("parameter index out of range")
    }

    pub fn parameter_value(&self, index: usize) -> Result<f64> {
        self.params
            .get(index)
            .copied()
            .context("parameter index out of range")
    }

    pub fn parameter_value_string(&self, index: usize, value: f64) -> Result<String> {
        let info = self.parameter_info(index)?;
        Ok(format!("{} {:.4}", info.short_name, value))
    }

    pub fn set_parameter(&mut self, index: usize, value: f64) -> Result<()> {
        let info = self.parameter_info(index)?;
        let clamped = value.clamp(info.min, info.max);
        note_host_edit();
        self.params[index] = clamped;
        self.pending_process_changes += 1;

        if info.id == PARAM_FIRE_PERFORM_EDIT {
            if clamped > 0.5 && self.last_fire_perform_edit <= 0.5 {
                let knob = self
                    .params
                    .get(test_params::index_of(PARAM_SIM_KNOB).unwrap_or(0))
                    .copied()
                    .unwrap_or(0.0);
                simulate_perform_edit(PARAM_SIM_KNOB, knob);
            }
            self.last_fire_perform_edit = clamped;
        } else if info.id == PARAM_LOAD_PRESET {
            let preset = (clamped * 3.999).round() as u8;
            self.load_preset(preset);
        }

        Ok(())
    }

    pub fn clear_pending_param_changes(&mut self) {
        self.pending_process_changes = 0;
    }

    pub fn deliver_pending_via_process(&mut self) -> Result<()> {
        self.pending_process_changes = 0;
        Ok(())
    }

    pub fn save_state(&self) -> Result<Vec<u8>> {
        let mut blob = Vec::with_capacity(16);
        blob.extend_from_slice(STATE_MAGIC);
        blob.push(1); // version
        blob.push(self.preset);
        blob.extend_from_slice(&(self.params[0].to_bits()).to_le_bytes());
        Ok(blob)
    }

    pub fn push_component_state_to_controller(&mut self, bytes: &[u8]) -> i32 {
        if bytes.len() < 4 || &bytes[..STATE_MAGIC.len()] != STATE_MAGIC {
            return 3; // kNotImplemented
        }
        if bytes.len() >= 7 {
            let preset = bytes[6];
            self.load_preset(preset);
        }
        3
    }

    pub fn describe_audio_buses(&self) -> Vec<(String, bool, i32)> {
        vec![
            ("Input".to_string(), true, 2),
            ("Output".to_string(), false, 2),
        ]
    }
}
