use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::host::{HostCommand, ParameterIndex};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MapperConfig {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub mappings: Vec<MidiMapping>,
    #[serde(default)]
    pub macros: Vec<MacroDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiMapping {
    pub source: MidiSource,
    pub target: MappingTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MidiSource {
    Cc { channel: u8, controller: u8 },
    Note { channel: u8, note: u8 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingTarget {
    pub parameter: String,
    #[serde(default = "default_curve")]
    pub curve: String,
}

fn default_curve() -> String {
    "linear".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub parameters: Vec<MacroParam>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroParam {
    pub name: String,
    #[serde(default = "default_weight")]
    pub weight: f64,
}

fn default_weight() -> f64 {
    1.0
}

impl MapperConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("read mappings {}", path.display()))?;
        serde_json::from_str(&data).context("parse mappings JSON")
    }
}

pub struct MidiMapper {
    cc_map: HashMap<(u8, u8), String>,
    param_ranges: HashMap<String, (f64, f64)>,
    param_index: ParameterIndex,
}

impl MidiMapper {
    pub fn new(config: MapperConfig, param_index: ParameterIndex) -> Self {
        let mut cc_map = HashMap::new();
        for m in config.mappings {
            if let MidiSource::Cc { channel, controller } = m.source {
                cc_map.insert((channel, controller), m.target.parameter);
            }
        }
        Self {
            cc_map,
            param_ranges: HashMap::new(),
            param_index,
        }
    }

    pub fn translate(&self, message: &[u8]) -> Option<HostCommand> {
        if message.len() < 3 {
            return None;
        }
        let status = message[0];
        let channel = status & 0x0F;
        let msg_type = status & 0xF0;

        if msg_type == 0xB0 {
            let controller = message[1];
            let value = message[2];
            let param_name = self.cc_map.get(&(channel, controller))?;
            let normalized = f64::from(value) / 127.0;
            let scaled = self.scale_parameter(param_name, normalized);
            return Some(HostCommand::SetParameterByName {
                name: param_name.clone(),
                value: scaled,
            });
        }

        None
    }

    pub fn forward_raw(&self, message: &[u8]) -> Option<HostCommand> {
        if message.is_empty() {
            return None;
        }
        Some(HostCommand::SendRawMidi {
            data: message.to_vec(),
        })
    }

    fn scale_parameter(&self, name: &str, normalized: f64) -> f64 {
        let idx = *self
            .param_index
            .read()
            .get(&name.to_ascii_lowercase())
            .unwrap_or(&0);
        let _ = idx;
        normalized
    }
}

#[allow(dead_code)]
pub fn send_cc(cmd_tx: &Sender<HostCommand>, channel: u8, controller: u8, value: u8) {
    let _ = cmd_tx.send(HostCommand::SendMidiCc {
        channel,
        controller,
        value,
    });
}
