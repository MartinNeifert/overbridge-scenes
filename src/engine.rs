use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Ensure Overbridge Engine is running.
/// Engine process to bridge USB communication to the hardware.
pub fn ensure_overbridge_engine(engine_path: &str) -> Result<()> {
    if is_engine_running() {
        tracing::info!("Overbridge Engine already running");
        return Ok(());
    }

    let path = Path::new(engine_path);
    if !path.exists() {
        tracing::warn!(
            "Overbridge Engine not found at {} — copy from /Applications/Elektron/ or install Overbridge",
            engine_path
        );
        return Ok(());
    }

    tracing::info!("Starting Overbridge Engine...");
    Command::new("open")
        .arg("-a")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("launch Overbridge Engine")?;

    for attempt in 1..=20 {
        std::thread::sleep(Duration::from_millis(500));
        if is_engine_running() {
            tracing::info!("Overbridge Engine started (attempt {attempt})");
            return Ok(());
        }
    }

    tracing::warn!("Overbridge Engine may not have started — plugin may fail to connect to device");
    Ok(())
}

fn is_engine_running() -> bool {
    Command::new("pgrep")
        .arg("-f")
        .arg("Overbridge Engine")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[allow(dead_code)]
pub fn engine_status() -> EngineStatus {
    EngineStatus {
        running: is_engine_running(),
    }
}

#[derive(Debug, serde::Serialize)]
pub struct EngineStatus {
    pub running: bool,
}
