const statusEl = document.getElementById("home-status");

async function refreshStatus() {
  try {
    const res = await fetch("/api/status");
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const plugin = data.plugin_loaded ? data.plugin_name || "plugin loaded" : "no plugin loaded";
    const port = data.port ?? "7780";
    statusEl.textContent = `Host running on port ${port} · ${plugin}`;
    statusEl.className = "home-hint ok";
  } catch {
    statusEl.textContent = "Host not reachable — start ob-host and reload.";
    statusEl.className = "home-hint err";
  }
}

refreshStatus();
