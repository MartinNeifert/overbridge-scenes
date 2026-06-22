use serde::Serialize;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a device-discovery result is reused before re-scanning. Discovery
/// shells out to `ioreg` and the slow `system_profiler`, so without this every
/// `/api/selector` and `/api/status` poll (one per browser tab, every few
/// seconds) would spawn those processes and block the async runtime.
const DISCOVER_CACHE_TTL: Duration = Duration::from_millis(2000);

/// A hardware unit visible to macOS / Overbridge.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectedDevice {
    pub name: String,
    pub manufacturer: String,
    pub source: String,
    pub transport: Option<String>,
    pub input_channels: Option<u32>,
    pub output_channels: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub serial: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DevicesSnapshot {
    pub engine_running: bool,
    pub devices: Vec<ConnectedDevice>,
}

/// Discover Elektron hardware, reusing a recent result when available.
///
/// The actual scan ([`discover_uncached`]) is expensive (spawns external
/// processes), so results are cached for [`DISCOVER_CACHE_TTL`]. The cache lock
/// is never held across the scan, so concurrent callers don't serialize on it.
pub fn discover() -> DevicesSnapshot {
    static CACHE: OnceLock<Mutex<Option<(Instant, DevicesSnapshot)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));

    if let Ok(guard) = cache.lock() {
        if let Some((at, snap)) = guard.as_ref() {
            if at.elapsed() < DISCOVER_CACHE_TTL {
                return snap.clone();
            }
        }
    }

    let snapshot = discover_uncached();
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((Instant::now(), snapshot.clone()));
    }
    snapshot
}

/// Discover Elektron hardware via USB registry and CoreAudio (Overbridge HAL).
fn discover_uncached() -> DevicesSnapshot {
    let mut devices = discover_usb();
    for audio in discover_coreaudio() {
        if !devices.iter().any(|d| names_match(&d.name, &audio.name)) {
            devices.push(audio);
        } else if let Some(existing) = devices.iter_mut().find(|d| names_match(&d.name, &audio.name)) {
            existing.input_channels = existing.input_channels.or(audio.input_channels);
            existing.output_channels = existing.output_channels.or(audio.output_channels);
            existing.sample_rate_hz = existing.sample_rate_hz.or(audio.sample_rate_hz);
            existing.transport = existing.transport.take().or(audio.transport);
            if existing.source == "usb" {
                existing.source = "usb+coreaudio".into();
            }
        }
    }
    devices.sort_by(|a, b| a.name.cmp(&b.name));

    DevicesSnapshot {
        engine_running: crate::engine::engine_status().running,
        devices,
    }
}

fn discover_usb() -> Vec<ConnectedDevice> {
    let Ok(output) = Command::new("ioreg").args(["-p", "IOUSB", "-l"]).output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut devices = Vec::new();

    for block in text.split("\n+-o ") {
        if !block.to_ascii_lowercase().contains("elektron") {
            continue;
        }
        let product = extract_ioreg_string(block, "USB Product Name")
            .or_else(|| extract_ioreg_string(block, "kUSBProductString"));
        let vendor = extract_ioreg_string(block, "USB Vendor Name")
            .or_else(|| extract_ioreg_string(block, "kUSBVendorString"));
        let serial = extract_ioreg_string(block, "USB Serial Number")
            .or_else(|| extract_ioreg_string(block, "kUSBSerialNumberString"));

        let Some(name) = product else { continue };
        if !is_elektron(vendor.as_deref(), &name) {
            continue;
        }

        devices.push(ConnectedDevice {
            name,
            manufacturer: vendor.unwrap_or_else(|| "Elektron".into()),
            source: "usb".into(),
            transport: Some("usb".into()),
            input_channels: None,
            output_channels: None,
            sample_rate_hz: None,
            serial,
        });
    }

    devices
}

fn discover_coreaudio() -> Vec<ConnectedDevice> {
    let Ok(output) = Command::new("system_profiler")
        .args(["SPAudioDataType", "-json"])
        .output()
    else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return Vec::new();
    };

    let mut devices = Vec::new();
    let Some(items) = json.get("SPAudioDataType").and_then(|v| v.as_array()) else {
        return devices;
    };

    for block in items {
        let Some(entries) = block.get("_items").and_then(|v| v.as_array()) else {
            continue;
        };
        for entry in entries {
            let name = entry.get("_name").and_then(|v| v.as_str()).unwrap_or("");
            let manufacturer = entry
                .get("_manufacturer")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !is_elektron(Some(manufacturer), name) {
                continue;
            }
            devices.push(ConnectedDevice {
                name: name.to_string(),
                manufacturer: manufacturer.to_string(),
                source: "coreaudio".into(),
                transport: entry
                    .get("coreaudio_device_transport")
                    .and_then(|v| v.as_str())
                    .map(|s| s.replace("coreaudio_device_type_", "")),
                input_channels: entry
                    .get("coreaudio_device_input")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32),
                output_channels: entry
                    .get("coreaudio_device_output")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32),
                sample_rate_hz: entry
                    .get("coreaudio_device_srate")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32),
                serial: None,
            });
        }
    }

    devices
}

fn extract_ioreg_string(block: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\" = ");
    let line = block.lines().find(|l| l.contains(&needle))?;
    let value = line.split(&needle).nth(1)?.trim().trim_matches('"');
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn is_elektron(vendor: Option<&str>, name: &str) -> bool {
    let v = vendor.unwrap_or("").to_ascii_lowercase();
    let n = name.to_ascii_lowercase();
    v.contains("elektron") || n.contains("elektron") || overbridge_device_name(&n)
}

fn overbridge_device_name(name: &str) -> bool {
    [
        "analog heat",
        "analog rytm",
        "analog four",
        "analog keys",
        "digitakt",
        "digitone",
        "syntakt",
    ]
    .iter()
    .any(|d| name.contains(d))
}

fn names_match(a: &str, b: &str) -> bool {
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    a.contains(&b) || b.contains(&a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_substrings() {
        assert!(names_match("Analog Heat", "Elektron Analog Heat MKII"));
        assert!(names_match("Digitakt", "Digitakt"));
    }
}
