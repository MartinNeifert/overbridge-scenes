use anyhow::{bail, Context, Result};
use truce_rack::vst3::Vst3Plugin;
use truce_rack_core::buffer::AudioBuffer;
use truce_rack_core::bus::BusLayout;
use truce_rack_core::events::EventList;
use truce_rack_core::info::{ParameterInfo, PluginInfo};
use truce_rack_core::plugin::{Plugin, PluginCore, ProcessContext, ProcessStatus};

use crate::host::fake_plugin::FakePlugin;

/// Loaded plugin backend — real VST3 or the in-process fake for tests.
pub enum PluginInstance {
    Vst3(Vst3Plugin),
    Fake(FakePlugin),
}

impl PluginInstance {
    pub fn info(&self) -> &PluginInfo {
        match self {
            Self::Vst3(p) => p.info(),
            Self::Fake(p) => p.info(),
        }
    }

    pub fn is_fake(&self) -> bool {
        matches!(self, Self::Fake(_))
    }

    pub fn parameter_count(&self) -> usize {
        match self {
            Self::Vst3(p) => p.parameter_count(),
            Self::Fake(p) => p.parameter_count(),
        }
    }

    pub fn parameter_info(&self, index: usize) -> Result<ParameterInfo> {
        match self {
            Self::Vst3(p) => p.parameter_info(index).context("parameter_info"),
            Self::Fake(p) => p.parameter_info(index),
        }
    }

    pub fn parameter_value(&self, index: usize) -> Result<f64> {
        match self {
            Self::Vst3(p) => p.parameter_value(index).context("parameter_value"),
            Self::Fake(p) => p.parameter_value(index),
        }
    }

    pub fn parameter_value_string(&self, index: usize, value: f64) -> Result<String> {
        match self {
            Self::Vst3(p) => p
                .parameter_value_string(index, value)
                .context("parameter_value_string"),
            Self::Fake(p) => p.parameter_value_string(index, value),
        }
    }

    pub fn set_parameter(&mut self, index: usize, value: f64) -> Result<()> {
        match self {
            Self::Vst3(p) => p.set_parameter(index, value).context("set_parameter"),
            Self::Fake(p) => p.set_parameter(index, value),
        }
    }

    pub fn clear_pending_param_changes(&mut self) {
        match self {
            Self::Vst3(p) => p.clear_pending_param_changes(),
            Self::Fake(p) => p.clear_pending_param_changes(),
        }
    }

    pub fn deliver_pending_via_process(&mut self) -> Result<()> {
        match self {
            Self::Vst3(p) => p.deliver_pending_via_process().context("deliver_pending"),
            Self::Fake(p) => p.deliver_pending_via_process(),
        }
    }

    pub fn save_state(&self) -> Result<Vec<u8>> {
        match self {
            Self::Vst3(p) => p.save_state().context("save_state"),
            Self::Fake(p) => p.save_state(),
        }
    }

    pub fn push_component_state_to_controller(&mut self, bytes: &[u8]) -> i32 {
        match self {
            Self::Vst3(p) => p.push_component_state_to_controller(bytes),
            Self::Fake(p) => p.push_component_state_to_controller(bytes),
        }
    }

    pub fn describe_audio_buses(&self) -> Vec<(String, bool, i32)> {
        match self {
            Self::Vst3(p) => p.describe_audio_buses(),
            Self::Fake(p) => p.describe_audio_buses(),
        }
    }

    pub fn activate(&mut self, layout: BusLayout, sample_rate: f64, max_block: usize) -> Result<()> {
        self.vst3_mut()?
            .activate(layout, sample_rate, max_block)
            .context("activate")
    }

    pub fn process(
        &mut self,
        buffer: &mut AudioBuffer<f32>,
        events: &EventList,
        ctx: &mut ProcessContext,
    ) -> Result<ProcessStatus> {
        match self {
            Self::Vst3(p) => p
                .process(buffer, events, ctx)
                .map_err(|e| anyhow::anyhow!("{e}")),
            Self::Fake(_) => Ok(ProcessStatus::Continue),
        }
    }

    pub fn vst3_mut(&mut self) -> Result<&mut Vst3Plugin> {
        match self {
            Self::Vst3(p) => Ok(p),
            Self::Fake(_) => bail!("operation requires a VST3 plugin"),
        }
    }

    pub fn fake_mut(&mut self) -> Result<&mut FakePlugin> {
        match self {
            Self::Fake(p) => Ok(p),
            Self::Vst3(_) => bail!("operation requires the fake plugin"),
        }
    }
}

pub type SharedPlugin = std::sync::Arc<parking_lot::Mutex<PluginInstance>>;
