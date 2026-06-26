//! HTTP client for the ob-host control API.

use serde::Deserialize;
use std::time::Duration;

const DEFAULT_HOST: &str = "127.0.0.1";

#[derive(Debug, Clone)]
pub struct HostClient {
    host: String,
    port: u16,
}

#[derive(Debug, Deserialize)]
struct StatusResponse {
    plugin: String,
}

#[derive(Debug, Deserialize)]
pub struct ApplyResponse {
    pub pattern: String,
    pub applied: usize,
}

impl HostClient {
    pub fn new(port: u16) -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port,
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    pub fn ping(&self) -> Result<String, String> {
        let url = format!("{}/api/status", self.base_url());
        let response = ureq::get(&url)
            .timeout(Duration::from_millis(800))
            .call()
            .map_err(|e| format!("{e}"))?;
        if response.status() / 100 != 2 {
            return Err(format!("HTTP {}", response.status()));
        }
        let status: StatusResponse = response.into_json().map_err(|e| format!("{e}"))?;
        Ok(status.plugin)
    }

    pub fn apply_crossfader(&self, pos: f64) -> Result<ApplyResponse, String> {
        let url = format!("{}/api/crossfader/apply", self.base_url());
        let body = serde_json::json!({ "pos": pos });
        let response = ureq::post(&url)
            .set("Content-Type", "application/json")
            .timeout(Duration::from_millis(1200))
            .send_json(body)
            .map_err(|e| format!("{e}"))?;
        if response.status() / 100 != 2 {
            return Err(format!("HTTP {}", response.status()));
        }
        response.into_json().map_err(|e| format!("{e}"))
    }
}
