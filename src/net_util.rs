//! Small helpers for advertising the control API on the local network.

fn scutil_get(key: &str) -> Option<String> {
    let output = std::process::Command::new("scutil")
        .arg("--get")
        .arg(key)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// macOS Bonjour host label, e.g. `My-Mac` → `http://My-Mac.local:7780/…`
pub fn local_hostname() -> Option<String> {
    scutil_get("LocalHostName")
}

/// Best-effort LAN IPv4 address (Wi‑Fi / Ethernet on macOS).
pub fn local_lan_ip() -> Option<String> {
    for iface in ["en0", "en1", "en2"] {
        let output = std::process::Command::new("ipconfig")
            .args(["getifaddr", iface])
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        let ip = String::from_utf8(output.stdout).ok()?;
        let ip = ip.trim();
        if !ip.is_empty() {
            return Some(ip.to_string());
        }
    }
    None
}
