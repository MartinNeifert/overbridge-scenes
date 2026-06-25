const statusEl = document.getElementById("home-status");

async function refreshStatus() {
  try {
    const res = await fetch("/api/status");
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json();
    const plugin = data.plugin || "no plugin loaded";
    const port = data.api_port ?? "7780";
    const engine = data.engine_running ? "Engine on" : "Engine off";
    statusEl.textContent = `Host on port ${port} · ${plugin} · ${engine}`;
    statusEl.className = "home-hint ok";
  } catch {
    statusEl.textContent = "Host not reachable — start ob-host and reload.";
    statusEl.className = "home-hint err";
  }
}

refreshStatus();
