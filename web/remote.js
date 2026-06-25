// Mobile crossfader — read-only scenes client.
// Uses the shared morph engine from scenes-morph.mjs.

import {
  clamp,
  computeCrossfadeUpdates,
  DEFAULT_QUAD_CORNERS,
  DEFAULT_QUAD_RELEASE_SNAP,
  DEFAULT_QUAD_RELEASE_SNAP_MS,
  normalizeCrossfader,
  quadSnapPosition,
} from "./scenes-morph.mjs";
import { bindXfPad, updatePadHandle, animatePadPosition } from "./scenes-xf-pad.mjs";

const EPS = 1e-4;

const SCENE_SLOTS = 4;

let plugin = null;
let pattern = { bank: 0, num: 0 };
let liveParams = [];
const liveByIndex = new Map();

let scenes = freshScenes();
let crossfader = freshCrossfader();
let baseline = new Map();
let storedBaseline = null;
let baselineResolved = false;
let baselineExplicit = false;

let ws = null;
const pendingApply = new Map();
let flushScheduled = false;
let xfGrab = null;
let quadSnapCancel = null;

const el = {
  ab: document.getElementById("remote-ab"),
  quad: document.getElementById("remote-quad"),
  slider: document.getElementById("remote-slider"),
  readout: document.getElementById("remote-readout"),
  quadReadout: document.getElementById("remote-quad-readout"),
  meta: document.getElementById("remote-meta"),
  status: document.getElementById("remote-status"),
  nameA: document.getElementById("remote-name-a"),
  nameB: document.getElementById("remote-name-b"),
  jumpA: document.getElementById("remote-jump-a"),
  jumpCenter: document.getElementById("remote-jump-center"),
  jumpB: document.getElementById("remote-jump-b"),
  pad: document.getElementById("remote-pad"),
  padHandle: document.getElementById("remote-pad-handle"),
  padLabelTl: document.getElementById("remote-pad-label-tl"),
  padLabelTr: document.getElementById("remote-pad-label-tr"),
  padLabelBl: document.getElementById("remote-pad-label-bl"),
  padLabelBr: document.getElementById("remote-pad-label-br"),
};

function freshScenes() {
  return Array.from({ length: SCENE_SLOTS }, (_, i) => ({
    id: String(i + 1),
    name: `Scene ${i + 1}`,
    params: [],
  }));
}

function freshCrossfader() {
  return {
    mode: "ab",
    a: null,
    b: null,
    pos: 0,
    corners: { ...DEFAULT_QUAD_CORNERS },
    x: 0.5,
    y: 0.5,
    quadCenterMode: "interpolation",
    quadReleaseSnap: DEFAULT_QUAD_RELEASE_SNAP,
    quadReleaseSnapMs: DEFAULT_QUAD_RELEASE_SNAP_MS,
  };
}

function isQuadMode() {
  return crossfader.mode === "quad";
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

function sceneById(id) {
  return scenes.find((s) => s.id === id) || null;
}

function cornerSceneName(corner) {
  const scene = sceneById(crossfader.corners[corner]);
  return scene ? scene.name : "Baseline";
}

function liveValue(index) {
  const p = liveByIndex.get(index);
  return p ? p.value : undefined;
}

function liveValuesMap() {
  const m = new Map();
  for (const p of liveParams) m.set(p.index, p.value);
  return m;
}

function paramRangesMap() {
  const m = new Map();
  for (const p of liveParams) m.set(p.index, { min: p.min, max: p.max });
  return m;
}

function morphCtx() {
  return {
    baselineExplicit,
    baseline,
    liveValues: liveValuesMap(),
    paramRanges: paramRangesMap(),
    xfGrab,
  };
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
    const normalized = normalizeCrossfader(data.crossfader);
    crossfader.mode = normalized.mode;
    crossfader.a = normalized.a;
    crossfader.b = normalized.b;
    crossfader.corners = { ...normalized.corners };
    crossfader.x = normalized.x;
    crossfader.y = normalized.y;
    crossfader.quadCenterMode = normalized.quadCenterMode;
    crossfader.quadReleaseSnap = normalized.quadReleaseSnap;
    crossfader.quadReleaseSnapMs = normalized.quadReleaseSnapMs;
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

function beginXfGrab() {
  xfGrab = null;
}

function cancelQuadSnap() {
  if (quadSnapCancel) {
    quadSnapCancel();
    quadSnapCancel = null;
  }
}

function endXfGrab() {
  xfGrab = null;
}

function onQuadGrabEnd() {
  endXfGrab();
  if (!isQuadMode()) return;

  const snap = crossfader.quadReleaseSnap ?? DEFAULT_QUAD_RELEASE_SNAP;
  const target = quadSnapPosition(snap);
  if (!target) return;

  if (
    Math.abs(crossfader.x - target.x) < EPS &&
    Math.abs(crossfader.y - target.y) < EPS
  ) {
    return;
  }

  cancelQuadSnap();
  const fromX = crossfader.x;
  const fromY = crossfader.y;
  const durationMs = crossfader.quadReleaseSnapMs ?? DEFAULT_QUAD_RELEASE_SNAP_MS;

  quadSnapCancel = animatePadPosition(
    fromX,
    fromY,
    target.x,
    target.y,
    durationMs,
    (x, y) => {
      setQuadPos(x, y);
      updatePadHandle(el.pad, el.padHandle, x, y);
      renderReadout();
      applyCrossfade();
    },
    () => {
      quadSnapCancel = null;
    }
  );
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
  const updates = computeCrossfadeUpdates(crossfader, scenes, morphCtx(), "jump");
  for (const { index, value } of updates) {
    queueApply(index, value);
  }
  flushSoon();
}

function renderLayout() {
  const quad = isQuadMode();
  el.ab?.classList.toggle("hidden", quad);
  el.quad?.classList.toggle("hidden", !quad);
}

function renderQuadPadLabels() {
  if (!el.padLabelTl) return;
  el.padLabelTl.textContent = cornerSceneName("tl");
  el.padLabelTr.textContent = cornerSceneName("tr");
  el.padLabelBl.textContent = cornerSceneName("bl");
  el.padLabelBr.textContent = cornerSceneName("br");
}

function renderReadout() {
  renderLayout();
  if (isQuadMode()) {
    const xPct = Math.round(crossfader.x * 100);
    const yPct = Math.round(crossfader.y * 100);
    if (el.quadReadout) el.quadReadout.textContent = `${xPct}% · ${yPct}%`;
    renderQuadPadLabels();
    updatePadHandle(el.pad, el.padHandle, crossfader.x, crossfader.y);
    return;
  }

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
  crossfader = freshCrossfader();
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

function setQuadPos(x, y) {
  crossfader.x = clamp(x, 0, 1);
  crossfader.y = clamp(y, 0, 1);
}

el.slider?.addEventListener("pointerdown", beginXfGrab);
el.slider?.addEventListener("pointerup", endXfGrab);
el.slider?.addEventListener("pointercancel", endXfGrab);

el.slider?.addEventListener("input", () => {
  if (!xfGrab) beginXfGrab();
  crossfader.pos = Number(el.slider.value) / 1000;
  renderReadout();
  applyCrossfade();
});

el.jumpA?.addEventListener("click", () => jumpTo(0));
el.jumpCenter?.addEventListener("click", () => jumpTo(0.5));
el.jumpB?.addEventListener("click", () => jumpTo(1));

bindXfPad(el.pad, el.padHandle, {
  getPos: () => ({ x: crossfader.x, y: crossfader.y }),
  setPos: (x, y) => setQuadPos(x, y),
  onGrabStart: () => {
    cancelQuadSnap();
    beginXfGrab();
  },
  onGrabEnd: onQuadGrabEnd,
  onChange: () => {
    renderReadout();
    applyCrossfade();
  },
});

(async () => {
  try {
    const statusRes = await fetch("/api/status");
    if (!statusRes.ok) throw new Error(`HTTP ${statusRes.status}`);
    const status = await statusRes.json();
    plugin = status.plugin || "default";
    setMeta([status.plugin || "Host", patternKey()]);

    const patKey = await resolvePatternKey();
    await loadScenesForPattern(patKey);
    const modeLabel = isQuadMode() ? "4-scene grid" : "A/B";
    setMeta([status.plugin || "Host", patternKey(), modeLabel]);
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
