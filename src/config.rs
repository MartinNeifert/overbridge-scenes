use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub api_port: u16,
    pub plugin_dir: String,
    pub default_plugin: Option<String>,
    pub sample_rate: u32,
    pub block_size: usize,
    pub overbridge_engine: String,
    /// Optional cpal output device name substring. When omitted, the host
    /// auto-selects a CoreAudio device matching the loaded plugin /
    /// connected hardware.
    #[serde(default)]
    pub audio_device: Option<String>,
    /// Control-only: drive the plugin's `process()` loop (so parameter / MIDI
    /// changes still reach the device) without opening the Overbridge audio
    /// device. Streaming to that device interrupts the hardware's own audio
    /// output, so this is the right default for a control surface.
    #[serde(default)]
    pub control_only: bool,
    /// Native single-AUHAL duplex audio path on the Elektron device. This is the
    /// DAW-equivalent path (one device, one clock) the Overbridge Engine can
    /// measure without faulting, and it monitors the device's own audio back to
    /// its output so the analog Main Out stays audible while the host is
    /// connected.
    #[serde(default)]
    pub duplex: DuplexConfig,
    pub midi: MidiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DuplexConfig {
    /// Enable the native duplex audio path.
    pub enabled: bool,
    /// Device name substring to host as the duplex AUHAL (e.g. "Digitakt").
    /// When empty, the loaded plugin's name is used as the hint.
    pub device: String,
    /// Monitor the device's own audio back to its output. While an Overbridge
    /// host is connected the device's analog Main Out plays the USB return, so
    /// this is what keeps it audible (a DAW does the same by monitoring tracks).
    pub monitor: bool,
    /// First device INPUT channel used as the monitor source (left); the next
    /// channel is the right. 0 = Main L/R.
    pub monitor_source: usize,
    /// Linear gain applied to the monitored signal.
    pub monitor_gain: f32,
}

impl Default for DuplexConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            device: String::new(),
            monitor: true,
            monitor_source: 0,
            monitor_gain: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiConfig {
    pub enabled: bool,
    pub virtual_port_name: String,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        serde_json::from_str(&data).context("parse config JSON")
    }
}
