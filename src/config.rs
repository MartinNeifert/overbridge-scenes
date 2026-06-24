use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub api_port: u16,
    pub plugin_dir: String,
    pub default_plugin: Option<String>,
    pub overbridge_engine: String,
    pub midi: MidiConfig,
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
