use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Ensure Overbridge Engine is running.
/// Engine process to bridge USB communication to the hardware.
pub fn ensure_overbridge_engine(engine_path: &str) -> Result<()> {
    if is_engine_running() {
        tracing::info!("Overbridge Engine already running");
        return Ok(());
    }

    let Some(path) = resolve_engine_path(engine_path) else {
        tracing::warn!(
            "Overbridge Engine not found (configured: {engine_path}) — \
             macOS: install to /Applications/Elektron/; \
             Linux: mise run wine:extract-overbridge"
        );
        return Ok(());
    };

    tracing::info!("Starting Overbridge Engine from {}", path.display());
    launch_engine(&path).context("launch Overbridge Engine")?;

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

/// Resolve configured path, then platform fallbacks (`OB_OVERBRIDGE_ENGINE`, `WINEPREFIX`).
pub fn resolve_engine_path(configured: &str) -> Option<PathBuf> {
    let configured_path = Path::new(configured);
    if configured_path.exists() {
        return Some(configured_path.to_path_buf());
    }

    if let Ok(env) = std::env::var("OB_OVERBRIDGE_ENGINE") {
        let path = PathBuf::from(&env);
        if path.exists() {
            return Some(path);
        }
    }

    #[cfg(target_os = "linux")]
    {
        let prefix = std::env::var("WINEPREFIX")
            .ok()
            .filter(|p| !p.is_empty())
            .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.wine-overbridge")))
            .map(PathBuf::from)?;

        let wine_exe = prefix.join(
            "drive_c/Program Files/Elektron/Overbridge Engine/Overbridge Engine.exe",
        );
        if wine_exe.exists() {
            return Some(wine_exe);
        }
    }

    None
}

#[cfg(target_os = "macos")]
fn launch_engine(path: &Path) -> Result<()> {
    Command::new("open")
        .arg("-a")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(Into::into)
}

#[cfg(target_os = "linux")]
fn launch_engine(path: &Path) -> Result<()> {
    Command::new("wine")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(Into::into)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn launch_engine(_path: &Path) -> Result<()> {
    anyhow::bail!("Overbridge Engine auto-launch is not supported on this OS");
}

fn is_engine_running() -> bool {
    #[cfg(target_os = "linux")]
    {
        if wine_engine_pgrep() {
            return true;
        }
    }

    for pattern in engine_process_patterns() {
        if pgrep_matches(pattern) {
            return true;
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn wine_engine_pgrep() -> bool {
    let output = match Command::new("pgrep")
        .args(["-f", "Overbridge Engine.exe"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return false,
    };

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(pid) = line.trim().parse::<u32>() else {
            continue;
        };
        let cmdline_path = format!("/proc/{pid}/cmdline");
        let Ok(bytes) = std::fs::read(&cmdline_path) else {
            continue;
        };
        let cmdline = bytes
            .split(|b| *b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part))
            .collect::<Vec<_>>()
            .join(" ");
        if cmdline.contains("/Overbridge Engine/Overbridge Engine.exe") {
            return true;
        }
    }
    false
}

fn engine_process_patterns() -> &'static [&'static str] {
    &[
        "Overbridge Engine/Overbridge Engine.exe",
        "Elektron/Overbridge Engine/Overbridge Engine.exe",
        "Overbridge Engine.app/Contents/MacOS",
    ]
}

fn pgrep_matches(pattern: &str) -> bool {
    Command::new("pgrep")
        .arg("-f")
        .arg(pattern)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn engine_status() -> EngineStatus {
    EngineStatus {
        running: is_engine_running(),
    }
}

#[derive(Debug, serde::Serialize)]
pub struct EngineStatus {
    pub running: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_engine_path_falls_back_to_wineprefix_on_linux() {
        let prefix = std::env::var("WINEPREFIX").unwrap_or_else(|_| {
            format!(
                "{}/.wine-overbridge",
                std::env::var("HOME").expect("HOME")
            )
        });
        let expected = PathBuf::from(&prefix).join(
            "drive_c/Program Files/Elektron/Overbridge Engine/Overbridge Engine.exe",
        );
        if !expected.exists() {
            return;
        }
        let resolved = resolve_engine_path("/nonexistent/Overbridge Engine.app");
        assert_eq!(resolved, Some(expected));
    }
}
