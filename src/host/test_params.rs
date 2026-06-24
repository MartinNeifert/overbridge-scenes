//! Stable parameter table shared by the in-process fake plugin and future
//! `OB-Test-Host.vst3` integration tests.

use truce_rack_core::info::{ParameterFlags, ParameterInfo};

pub const PARAM_FILTER_CUTOFF: u32 = 0x0001;
pub const PARAM_FILTER_RESO: u32 = 0x0002;
pub const PARAM_DRIVE: u32 = 0x0003;
pub const PARAM_MORPH_A: u32 = 0x0004;
pub const PARAM_MORPH_B: u32 = 0x0005;
pub const PARAM_MORPH_C: u32 = 0x0006;
pub const PARAM_SIM_KNOB: u32 = 0x00F0;
pub const PARAM_FIRE_PERFORM_EDIT: u32 = 0x00F1;
pub const PARAM_LOAD_PRESET: u32 = 0x00F2;

pub const PARAM_COUNT: usize = 9;

pub fn parameter_table() -> [ParameterInfo; PARAM_COUNT] {
    [
        float_param(PARAM_FILTER_CUTOFF, "Filter Cutoff", "Cutoff", 0.5),
        float_param(PARAM_FILTER_RESO, "Filter Reso", "Reso", 0.25),
        float_param(PARAM_DRIVE, "Drive", "Drive", 0.0),
        float_param(PARAM_MORPH_A, "Morph A", "MorphA", 0.0),
        float_param(PARAM_MORPH_B, "Morph B", "MorphB", 0.0),
        float_param(PARAM_MORPH_C, "Morph C", "MorphC", 0.0),
        float_param(PARAM_SIM_KNOB, "Sim Knob", "SimKnob", 0.0),
        float_param(PARAM_FIRE_PERFORM_EDIT, "Fire PerformEdit", "FirePE", 0.0),
        float_param(PARAM_LOAD_PRESET, "Load Preset", "Preset", 0.0),
    ]
}

fn float_param(id: u32, name: &str, short: &str, default: f64) -> ParameterInfo {
    ParameterInfo {
        id,
        name: name.to_string(),
        short_name: short.to_string(),
        unit: String::new(),
        min: 0.0,
        max: 1.0,
        default,
        step_count: 0,
        flags: ParameterFlags::AUTOMATABLE,
    }
}

pub fn index_of(id: u32) -> Option<usize> {
    parameter_table().iter().position(|p| p.id == id)
}

pub fn preset_filter_cutoff(preset: u8) -> f64 {
    match preset % 4 {
        0 => 0.1,
        1 => 0.35,
        2 => 0.65,
        3 => 0.9,
        _ => unreachable!(),
    }
}
