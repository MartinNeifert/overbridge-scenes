// Mobile crossfader — read-only scenes client (slider only).
// Uses the same morph engine as scenes.html; does not edit scenes.

const EPS = 1e-4;
const SCENE_SLOTS = 4;

let plugin = null;
let pattern = { bank: 0, num: 0 };
let liveParams = [];
const liveByIndex = new Map();

let scenes = freshScenes();
let crossfader = { a: null, b: null, pos: 0 };
let baseline = new Map();
let storedBaseline = null;
let baselineResolved = false;
let baselineExplicit = false;

let ws = null;
const pendingApply = new Map();
let flushScheduled = false;
let xfGrab = null;

const el = {
  slider: document.getElementById("remote-slider"),
  readout: document.getElementById("remote-readout"),
  meta: document.getElementById("remote-meta"),
  status: document.getElementById("remote-status"),
  nameA: document.getElementById("remote-name-a"),
  nameB: document.getElementById("remote-name-b"),
  jumpA: document.getElementById("remote-jump-a"),
  jumpCenter: document.getElementById("remote-jump-center"),
  jumpB: document.getElementById("remote-jump-b"),
};

function clamp(v, lo, hi) {
  return Math.min(hi, Math.max(lo, v));
}

function bankLetter(b) {
  return String.fromCharCode(65 + b);
}

function patternKey() {
  return bankLetter(pattern.bank) + String(pattern.num + 1).padStart(2, "0");
}

function parsePatternKey(key) {
  const m = /^([A-P])(\d{1,2})$/i.exec(String(key || "").trim());
  if (!m) return null;
  const bank = m[1].toUpperCase().charCodeAt(0) - 65;
  const num = Number(m[2]) - 1;
  if (bank < 0 || bank > 15 || num < 0 || num > 15) return null;
  return { bank, num };
}

function freshScenes() {
  return Array.from({ length: SCENE_SLOTS }, (_, i) => ({
    id: String(i + 1),
    name: `Scene ${i + 1}`,
    params: [],
  }));
}

function sceneById(id) {
  return scenes.find((s) => s.id === id) || null;
}

function liveValue(index) {
  const p = liveByIndex.get(index);
  return p ? p.value : undefined;
}

function paramRange(index) {
  const p = liveByIndex.get(index);
  if (p && Number.isFinite(p.min) && Number.isFinite(p.max)) return [p.min, p.max];
  return [0, 1];
}

function scenesApiUrl(pat) {
  const p = encodeURIComponent(plugin || "default");
  const pk = encodeURIComponent(pat || patternKey());
  return `/api/scenes/${p}/${pk}`;
}

function activePatternApiUrl() {
  return `/api/scenes/${encodeURIComponent(plugin || "default")}/active`;
}

function applyScenesPayload(data) {
  if (Array.isArray(data.scenes)) {
    for (const s of data.scenes) {
      const slot = sceneById(s.id);
      if (!slot) continue;
      if (typeof s.name === "string") slot.name = s.name;
      if (Array.isArray(s.params)) {
        slot.params = s.params
          .filter((p) => p && Number.isFinite(p.value))
          .map((p) => ({
            index: p.index,
            id: p.id,
            name: p.name || "",
            value: p.value,
          }));
      }
    }
  }
  if (data.crossfader) {
    crossfader.a = data.crossfader.a ?? null;
    crossfader.b = data.crossfader.b ?? null;
  }
  if (data.baseline) {
    if (Array.isArray(data.baseline)) {
      storedBaseline = data.baseline;
      baselineExplicit = true;
    } else if (Array.isArray(data.baseline.values)) {
      storedBaseline = data.baseline.values;
      baselineExplicit = !!data.baseline.explicit;
    }
  }
}

function validateScenes() {
  const byId = new Map();
  const byName = new Map();
  for (const p of liveParams) {
    byId.set(p.id, p);
    byName.set(p.name.toLowerCase(), p);
  }
  for (const scene of scenes) {
    const resolved = [];
    for (const sp of scene.params) {
      let live = sp.id != null ? byId.get(sp.id) : undefined;
      if (!live && sp.name) live = byName.get(sp.name.toLowerCase());
      if (!live && liveByIndex.has(sp.index)) live = liveByIndex.get(sp.index);
      if (!live) continue;
      sp.index = live.index;
      sp.id = live.id;
      sp.name = live.name;
      resolved.push(sp);
    }
    scene.params = resolved;
  }
  resolveBaseline();
}

function baselineParamIndices() {
  const set = new Set();
  for (const s of scenes) for (const p of s.params) set.add(p.index);
  return [...set];
}

function resolveBaseline() {
  if (!liveParams.length) return;
  if (storedBaseline) {
    const byId = new Map(liveParams.map((p) => [p.id, p]));
    baseline = new Map();
    for (const b of storedBaseline) {
      let live = b.id != null ? byId.get(b.id) : null;
      if (!live && liveByIndex.has(b.index)) live = liveByIndex.get(b.index);
      if (live && Number.isFinite(b.value)) baseline.set(live.index, b.value);
    }
    storedBaseline = null;
    baselineResolved = true;
  }
  if (!baselineResolved) {
    baseline = new Map();
    for (const index of baselineParamIndices()) {
      const lv = liveValue(index);
      if (lv !== undefined) baseline.set(index, lv);
    }
    baselineResolved = true;
    baselineExplicit = false;
  }
}

function unionIndices() {
  const a = sceneById(crossfader.a);
  const b = sceneById(crossfader.b);
  const set = new Set();
  if (a) for (const p of a.params) set.add(p.index);
  if (b) for (const p of b.params) set.add(p.index);
  return [...set];
}

function emptySideValue(index) {
  if (baselineExplicit && baseline.has(index)) return baseline.get(index);
  if (xfGrab && xfGrab.per.has(index)) return xfGrab.per.get(index).v0;
  const lv = liveValue(index);
  if (lv !== undefined) return lv;
  if (baseline.has(index)) return baseline.get(index);
  return 0;
}

function endpointValue(scene, index) {
  if (scene) {
    const p = scene.params.find((x) => x.index === index);
    if (p) return p.value;
  }
  return emptySideValue(index);
}

function beginXfGrab() {
  const per = new Map();
  for (const index of unionIndices()) {
    const lv = liveValue(index);
    per.set(index, {
      v0: lv !== undefined ? lv : emptySideValue(index),
      engaged: false,
    });
  }
  xfGrab = { t0: crossfader.pos, per };
}

function endXfGrab() {
  xfGrab = null;
}

function queueApply(index, value) {
  pendingApply.set(index, value);
}

function flushSoon() {
  if (flushScheduled) return;
  flushScheduled = true;
  requestAnimationFrame(flushApply);
}

function flushApply() {
  flushScheduled = false;
  if (pendingApply.size === 0) return;
  const updates = [];
  for (const [index, value] of pendingApply) {
    updates.push({ index, value });
  }
  pendingApply.clear();
  fetch("/api/parameters/batch", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ updates }),
  }).catch(() => setStatus("Could not send parameter update", true));
}

function applyCrossfade() {
  const a = sceneById(crossfader.a);
  const b = sceneById(crossfader.b);
  if (!a && !b) return;
  const t = crossfader.pos;

  for (const index of unionIndices()) {
    const av = endpointValue(a, index);
    const bv = endpointValue(b, index);
    const ideal = av + (bv - av) * t;
    const [min, max] = paramRange(index);
    queueApply(index, clamp(ideal, Math.min(min, max), Math.max(min, max)));
  }
  flushSoon();
}

function renderReadout() {
  const pct = Math.round(crossfader.pos * 100);
  const a = sceneById(crossfader.a);
  const b = sceneById(crossfader.b);
  const aName = a ? a.name : "Baseline";
  const bName = b ? b.name : "Baseline";
  el.nameA.textContent = aName;
  el.nameB.textContent = bName;
  el.readout.textContent = `${pct}%`;
  el.slider.value = String(Math.round(crossfader.pos * 1000));
}

function setStatus(msg, isError = false) {
  if (!el.status) return;
  el.status.textContent = msg || "";
  el.status.classList.toggle("error", !!isError);
}

function setMeta(lines) {
  if (el.meta) el.meta.textContent = lines.filter(Boolean).join(" · ");
}

async function loadScenesForPattern(patKey) {
  const parsed = parsePatternKey(patKey);
  if (parsed) pattern = parsed;

  scenes = freshScenes();
  crossfader = { a: null, b: null, pos: 0 };
  baseline = new Map();
  storedBaseline = null;
  baselineResolved = false;
  baselineExplicit = false;

  try {
    const res = await fetch(scenesApiUrl(patKey));
    if (res.ok) {
      applyScenesPayload(await res.json());
    } else if (res.status !== 404) {
      throw new Error(`HTTP ${res.status}`);
    }
  } catch (e) {
    console.warn("scene load failed", e);
    setStatus("Could not load scenes — is the host running?", true);
    return;
  }

  if (liveParams.length) validateScenes();
  renderReadout();
  setStatus("");
}

async function resolvePatternKey() {
  const fromUrl = new URLSearchParams(location.search).get("pattern");
  if (fromUrl && parsePatternKey(fromUrl)) return fromUrl.toUpperCase();

  try {
    const res = await fetch(activePatternApiUrl());
    if (res.ok) {
      const data = await res.json();
      if (data.pattern && parsePatternKey(data.pattern)) return data.pattern.toUpperCase();
    }
  } catch (e) {
    console.warn("active pattern load failed", e);
  }
  return "A01";
}

function ingestParameters(list) {
  liveParams = list;
  liveByIndex.clear();
  for (const p of list) liveByIndex.set(p.index, p);
  validateScenes();
}

function connectWs() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/api/ws`);
  ws.onopen = () => setStatus("");
  ws.onmessage = (ev) => {
    let msg;
    try {
      msg = JSON.parse(ev.data);
    } catch {
      return;
    }
    if (msg.type === "parameters") {
      ingestParameters(msg.data);
    } else if (msg.type === "param_updates") {
      for (const u of msg.data) {
        const p = liveByIndex.get(u.index);
        if (p) p.value = u.value;
      }
    }
  };
  ws.onclose = () => {
    setStatus("Disconnected — reconnecting…", true);
    setTimeout(connectWs, 2000);
  };
  ws.onerror = () => {};
}

function jumpTo(pos) {
  endXfGrab();
  crossfader.pos = pos;
  renderReadout();
  applyCrossfade();
}

el.slider.addEventListener("pointerdown", beginXfGrab);
el.slider.addEventListener("pointerup", endXfGrab);
el.slider.addEventListener("pointercancel", endXfGrab);

el.slider.addEventListener("input", () => {
  if (!xfGrab) beginXfGrab();
  crossfader.pos = Number(el.slider.value) / 1000;
  renderReadout();
  applyCrossfade();
});

el.jumpA.addEventListener("click", () => jumpTo(0));
el.jumpCenter.addEventListener("click", () => jumpTo(0.5));
el.jumpB.addEventListener("click", () => jumpTo(1));

(async () => {
  try {
    const statusRes = await fetch("/api/status");
    if (!statusRes.ok) throw new Error(`HTTP ${statusRes.status}`);
    const status = await statusRes.json();
    plugin = status.plugin || "default";
    setMeta([status.plugin || "Host", patternKey()]);

    const patKey = await resolvePatternKey();
    await loadScenesForPattern(patKey);
    setMeta([status.plugin || "Host", patternKey()]);
    if (
      status.lan_ip &&
      (location.hostname === "localhost" || location.hostname === "127.0.0.1")
    ) {
      const host = status.lan_hostname
        ? `http://${status.lan_hostname}.local:${status.api_port}/remote.html`
        : null;
      const ip = `http://${status.lan_ip}:${status.api_port}/remote.html`;
      setStatus(host ? `On your phone: ${host} (or ${ip})` : `On your phone: ${ip}`);
    }
    connectWs();
  } catch (e) {
    console.warn("boot failed", e);
    setMeta("Not connected");
    setStatus("Cannot reach ob-host on this address", true);
  }
})();
