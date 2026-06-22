const PINNED_KEY = "ob-host-pinned";
const MAX_VISIBLE = 80;

let parameters = [];
let ws = null;
let pinned = JSON.parse(localStorage.getItem(PINNED_KEY) || "[]");
let draggingIndex = null;
let cardsByIndex = new Map();

const deviceDetailEl = document.getElementById("device-detail");
const paramsEl = document.getElementById("parameters");
const pinnedEl = document.getElementById("pinned-controls");
const searchEl = document.getElementById("search");
const pinnedOnlyEl = document.getElementById("pinned-only");

function api(path, options = {}) {
  return fetch(path, {
    headers: { "Content-Type": "application/json" },
    ...options,
  });
}

// Device picker + connection status live in the shared global header
// (device-header.js). It broadcasts selector data; we just keep the local
// device-detail line in sync and reload parameters when the plugin changes.
function renderDeviceDetail(data) {
  if (!deviceDetailEl) return;
  const loaded = data.loaded_plugin;

  if (!data.engine_running) {
    deviceDetailEl.innerHTML = "Start Overbridge Engine to connect hardware.";
    return;
  }

  const connected = data.connected || [];
  const selected =
    connected.find((d) => d.linked) ||
    connected.find((d) => d.suggested_plugin === loaded);
  if (selected) {
    deviceDetailEl.innerHTML = `<strong>${escapeHtml(selected.device_name)}</strong> · ${escapeHtml(selected.manufacturer)}${selected.serial ? ` · S/N ${escapeHtml(selected.serial)}` : ""} · plugin <strong>${escapeHtml(loaded)}</strong>`;
  } else if (connected.length > 0) {
    const names = connected.map((d) => escapeHtml(d.device_name)).join(", ");
    deviceDetailEl.innerHTML = `Connected: <strong>${names}</strong> · active plugin <strong>${escapeHtml(loaded)}</strong> (not linked to this device)`;
  } else {
    deviceDetailEl.innerHTML = `Active plugin: <strong>${escapeHtml(loaded)}</strong> · ${data.parameter_count} parameters · no device connected`;
  }
}

document.addEventListener("ob:selector", (ev) => {
  renderDeviceDetail(ev.detail);
});

document.addEventListener("ob:plugin-changed", async () => {
  try {
    const paramsRes = await api("/api/parameters");
    parameters = await paramsRes.json();
    pinned.length = 0;
    localStorage.setItem(PINNED_KEY, "[]");
    render();
    connectWs();
  } catch (err) {
    console.error("reload after plugin change failed", err);
  }
});

function normalize(value, min, max) {
  if (max === min) return 0;
  return (value - min) / (max - min);
}

function denormalize(t, min, max) {
  return min + t * (max - min);
}

function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function formatDisplay(p) {
  const unit = p.unit ? ` ${escapeHtml(p.unit)}` : "";
  return `${escapeHtml(p.display)}${unit}`;
}

function createParamCard(p) {
  const card = document.createElement("div");
  card.className = "param-card" + (pinned.includes(p.index) ? " pinned" : "");
  card.dataset.index = String(p.index);

  const t = normalize(p.value, p.min, p.max);

  card.innerHTML = `
    <div class="param-header">
      <span class="param-name">${escapeHtml(p.name)}</span>
      <button class="pin-btn ${pinned.includes(p.index) ? "active" : ""}" title="Pin control">★</button>
    </div>
    <div class="param-value" data-display>${formatDisplay(p)}</div>
    <input type="range" min="0" max="1000" step="1" value="${Math.round(t * 1000)}" />
  `;

  const slider = card.querySelector('input[type="range"]');
  const display = card.querySelector("[data-display]");
  const pinBtn = card.querySelector(".pin-btn");

  slider.addEventListener("pointerdown", () => {
    draggingIndex = p.index;
  });
  slider.addEventListener("pointerup", () => {
    draggingIndex = null;
  });
  slider.addEventListener("pointercancel", () => {
    draggingIndex = null;
  });

  slider.addEventListener("input", () => {
    const norm = Number(slider.value) / 1000;
    const value = denormalize(norm, p.min, p.max);
    display.textContent = value.toFixed(4);
    setParameter(p.index, value);
  });

  pinBtn.addEventListener("click", () => togglePin(p.index));

  cardsByIndex.set(p.index, { card, slider, display, meta: p });
  return card;
}

function updateParamCard(p) {
  const entry = cardsByIndex.get(p.index);
  if (!entry) return;

  entry.meta = p;
  entry.display.innerHTML = formatDisplay(p);

  if (draggingIndex === p.index) return;

  const t = normalize(p.value, p.min, p.max);
  entry.slider.value = String(Math.round(t * 1000));
}

function togglePin(index) {
  const i = pinned.indexOf(index);
  if (i >= 0) pinned.splice(i, 1);
  else pinned.push(index);
  localStorage.setItem(PINNED_KEY, JSON.stringify(pinned));
  render();
}

async function setParameter(index, value) {
  const p = parameters.find((x) => x.index === index);
  if (p) {
    p.value = value;
    p.display = value.toFixed(4);
  }

  try {
    const res = await api(`/api/parameters/${index}`, {
      method: "POST",
      body: JSON.stringify({ value }),
    });
    if (res.ok) {
      const updated = await res.json();
      if (cardsByIndex.has(index)) updateParamCard(updated);
    }
  } catch (err) {
    console.error("setParameter failed", err);
  }
}

function filteredParameters() {
  const q = searchEl.value.trim().toLowerCase();
  const pinnedOnly = pinnedOnlyEl.checked;

  return parameters.filter((p) => {
    if (pinnedOnly && !pinned.includes(p.index)) return false;
    if (q && !p.name.toLowerCase().includes(q)) return false;
    return true;
  });
}

function render() {
  cardsByIndex.clear();
  pinnedEl.innerHTML = "";
  paramsEl.innerHTML = "";

  for (const index of pinned) {
    const p = parameters.find((x) => x.index === index);
    if (p) pinnedEl.appendChild(createParamCard(p));
  }

  const filtered = filteredParameters().filter((p) => !pinned.includes(p.index));
  const visible = filtered.slice(0, MAX_VISIBLE);

  for (const p of visible) {
    paramsEl.appendChild(createParamCard(p));
  }

  if (filtered.length > MAX_VISIBLE) {
    const note = document.createElement("p");
    note.className = "hint";
    note.textContent = `Showing ${MAX_VISIBLE} of ${filtered.length} matches — refine search to find more.`;
    paramsEl.appendChild(note);
  } else if (parameters.length > 0 && !searchEl.value && !pinnedOnlyEl.checked && pinned.length === 0) {
    const note = document.createElement("p");
    note.className = "hint";
    note.textContent = `Search or pin parameters to control them. ${parameters.length} total available.`;
    paramsEl.appendChild(note);
  }
}

function applyParamUpdate(u) {
  let p = parameters.find((x) => x.index === u.index);
  if (!p) return;
  p.value = u.value;
  p.display = u.display;
  if (cardsByIndex.has(u.index)) {
    updateParamCard(p);
  }
}

function syncParameters(next) {
  parameters = next;

  for (const p of next) {
    if (cardsByIndex.has(p.index)) {
      updateParamCard(p);
    }
  }

  for (const index of pinned) {
    const p = next.find((x) => x.index === index);
    if (p && cardsByIndex.has(index)) updateParamCard(p);
  }
}

function applyParamUpdates(updates) {
  let touchedPinned = false;
  for (const u of updates) {
    applyParamUpdate(u);
    if (pinned.includes(u.index)) touchedPinned = true;
  }
  if (touchedPinned) render();
}

function connectWs() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/api/ws`);

  ws.onmessage = (ev) => {
    try {
      const msg = JSON.parse(ev.data);
      if (msg.type === "parameters") {
        syncParameters(msg.data);
      } else if (msg.type === "param_updates") {
        applyParamUpdates(msg.data);
      }
    } catch {}
  };

  ws.onclose = () => {
    setTimeout(connectWs, 2000);
  };
}

document.getElementById("refresh").addEventListener("click", async () => {
  const res = await api("/api/parameters");
  syncParameters(await res.json());
  render();
});

document.getElementById("send-cc").addEventListener("click", async () => {
  const channel = Number(document.getElementById("midi-channel").value);
  const controller = Number(document.getElementById("midi-cc").value);
  const value = Number(document.getElementById("midi-cc-value").value);
  await api("/api/midi/cc", {
    method: "POST",
    body: JSON.stringify({ channel, controller, value }),
  });
});

searchEl.addEventListener("input", render);
pinnedOnlyEl.addEventListener("change", render);

connectWs();
api("/api/parameters")
  .then((r) => r.json())
  .then((data) => {
    parameters = data;
    render();
  });
