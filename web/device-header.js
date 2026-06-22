// Shared global header: device selector + live connection status.
//
// Included by BOTH the classic control surface (index.html) and the scenes
// surface (scenes.html) so the device picker and connection indicator are
// identical everywhere. Mounts into <div id="ob-device-header"></div>.
//
// Responsibilities:
//   - poll GET /api/selector
//   - drive the device <select> and switch plugins via POST /api/select-plugin
//   - color the status pill GREEN only when a hardware device is actually
//     connected (data.connected.length > 0) — engine-up / plugin-loaded alone
//     is NOT green.
//
// Broadcasts on `document` so each page can refresh its own data:
//   - "ob:selector"        detail: full selector payload (every successful poll)
//   - "ob:plugin-changed"  detail: { plugin } (whenever the loaded plugin changes)

(function () {
  const mount = document.getElementById("ob-device-header");
  if (!mount) return;

  mount.classList.add("obdh");
  mount.innerHTML = `
    <label class="obdh-select-wrap">
      <span class="obdh-label">Device</span>
      <select class="obdh-select" id="device-select" aria-label="Select Overbridge device or plugin">
        <option value="">Loading…</option>
      </select>
    </label>
    <span class="obdh-status" id="device-status" role="status">Connecting…</span>
  `;

  const selectEl = mount.querySelector("#device-select");
  const statusEl = mount.querySelector("#device-status");

  let data = null;
  let switching = false;
  let lastPlugin = null;

  // Resilience: a single slow/failed poll shouldn't flip the pill to
  // "unavailable". Keep the last-known state until several polls in a row miss.
  let inflight = false;
  let consecutiveFailures = 0;
  const FAILURE_TOLERANCE = 3;
  const POLL_TIMEOUT_MS = 4000;

  function setStatus(text, state) {
    statusEl.textContent = text;
    statusEl.className = "obdh-status" + (state ? " " + state : "");
  }

  let lastSelectSig = null;

  function render() {
    renderSelect();
    renderStatus();
  }

  // Signature of everything the <select> renders, so we only rebuild its DOM
  // when something actually changed (rebuilding every poll causes a visible
  // flicker and closes the dropdown if it's open).
  function selectSignature() {
    if (!data.engine_running) return "engine-off";
    const conn = (data.connected || [])
      .map((d) => `${d.device_name}:${d.suggested_plugin || ""}:${d.linked ? 1 : 0}`)
      .join(",");
    const plugs = (data.plugins || [])
      .map((p) => `${p.name}:${p.loaded ? 1 : 0}:${p.connected ? 1 : 0}`)
      .join(",");
    return `${data.loaded_plugin}|${switching ? 1 : 0}|${conn}|${plugs}`;
  }

  function renderSelect() {
    const sig = selectSignature();
    if (sig === lastSelectSig) return; // nothing changed → no DOM churn
    // Don't rebuild while the user is interacting with the dropdown.
    if (document.activeElement === selectEl) return;
    lastSelectSig = sig;

    const loaded = data.loaded_plugin;
    selectEl.innerHTML = "";

    if (!data.engine_running) {
      selectEl.disabled = true;
      selectEl.innerHTML =
        '<option value="">Overbridge Engine not running</option>';
      return;
    }
    selectEl.disabled = switching;

    const connected = data.connected || [];
    if (connected.length > 0) {
      const group = document.createElement("optgroup");
      group.label = "Connected";
      for (const d of connected) {
        const plugin = d.suggested_plugin || loaded;
        const opt = document.createElement("option");
        opt.value = plugin;
        opt.textContent = d.device_name + (d.linked ? " · linked" : "");
        opt.dataset.device = d.device_name;
        if (plugin === loaded) opt.selected = true;
        group.appendChild(opt);
      }
      selectEl.appendChild(group);
    }

    const all = document.createElement("optgroup");
    all.label = "All plugins";
    for (const p of data.plugins) {
      const opt = document.createElement("option");
      opt.value = p.name;
      const suffix = p.loaded ? " · active" : p.connected ? " · connected" : "";
      opt.textContent = p.name + suffix;
      if (p.loaded) opt.selected = true;
      all.appendChild(opt);
    }
    selectEl.appendChild(all);

    // Always reflect the actually-loaded plugin.
    if ([...selectEl.options].some((o) => o.value === loaded)) {
      selectEl.value = loaded;
    }
  }

  function renderStatus() {
    if (!data.engine_running) {
      setStatus("Engine offline", "warn");
      return;
    }
    const connected = data.connected || [];
    if (connected.length === 0) {
      // Plugin may be loaded, but no hardware is actually connected → not green.
      setStatus(`${data.loaded_plugin} · no device`, "idle");
      return;
    }
    // A hardware device is actually connected → green.
    const linked = connected.find((d) => d.linked);
    if (linked) {
      setStatus(`${linked.device_name} · linked`, "connected");
    } else {
      const names = connected.map((d) => d.device_name).join(", ");
      setStatus(`${names} · connected`, "connected");
    }
  }

  async function refresh() {
    if (switching || inflight) return;
    inflight = true;
    let next;
    try {
      const ctrl = new AbortController();
      const timer = setTimeout(() => ctrl.abort(), POLL_TIMEOUT_MS);
      try {
        const res = await fetch("/api/selector", {
          headers: { "Content-Type": "application/json" },
          signal: ctrl.signal,
        });
        if (!res.ok) throw new Error("HTTP " + res.status);
        next = await res.json();
      } finally {
        clearTimeout(timer);
      }
    } catch {
      inflight = false;
      consecutiveFailures += 1;
      // Tolerate transient misses (server busy during a switch / device scan):
      // keep showing the last-known state until repeated failures.
      if (consecutiveFailures >= FAILURE_TOLERANCE) {
        setStatus("Host unavailable", "error");
        selectEl.disabled = true;
      }
      return;
    }
    inflight = false;
    consecutiveFailures = 0;
    data = next;
    render();
    document.dispatchEvent(new CustomEvent("ob:selector", { detail: data }));

    if (data.loaded_plugin !== lastPlugin) {
      const prev = lastPlugin;
      lastPlugin = data.loaded_plugin;
      if (prev !== null) {
        document.dispatchEvent(
          new CustomEvent("ob:plugin-changed", {
            detail: { plugin: data.loaded_plugin },
          })
        );
      }
    }
  }

  async function switchTo(plugin) {
    if (!plugin || switching || plugin === data?.loaded_plugin) return;

    switching = true;
    selectEl.disabled = true;
    setStatus(`Loading ${plugin}…`, "idle");

    try {
      const res = await fetch("/api/select-plugin", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ plugin }),
      });
      if (!res.ok) throw new Error("switch failed");
      data = await res.json();
      lastPlugin = data.loaded_plugin;
      render();
      document.dispatchEvent(new CustomEvent("ob:selector", { detail: data }));
      document.dispatchEvent(
        new CustomEvent("ob:plugin-changed", {
          detail: { plugin: data.loaded_plugin },
        })
      );
    } catch (e) {
      console.error(e);
      setStatus("Plugin switch failed", "error");
    } finally {
      switching = false;
      selectEl.disabled = false;
    }
  }

  function onChange() {
    switchTo(selectEl.value);
  }

  selectEl.addEventListener("change", onChange);

  // Expose a manual refresh for pages that want to re-poll after an action.
  window.OBDeviceHeader = { refresh };

  refresh();
  setInterval(refresh, 3000);
})();
