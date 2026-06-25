// Octatrack-style Scenes + Crossfader — standalone control surface.
//
// Talks to the existing ob-host API only:
//   - GET  /api/selector        → which plugin/device is loaded (namespacing)
//   - GET  /api/parameters      → full parameter list (fallback / initial)
//   - WS   /api/ws              → live parameter values + low-latency writes
//                                 ({action:"set_parameter", index, value})
//
// Values are VST3-normalized (param.min..param.max, usually 0..1), so morphing
// is a straight linear interpolation in that space.

import {
  clamp,
  computeCrossfadeUpdates,
  beginXfGrab as beginXfGrabState,
  shouldApplyCrossfade,
  DEFAULT_QUAD_CORNERS,
  DEFAULT_QUAD_CENTER_MODE,
  DEFAULT_QUAD_RELEASE_SNAP,
  DEFAULT_AB_RELEASE_SNAP,
  DEFAULT_QUAD_RELEASE_SNAP_MS,
  normalizeCrossfader,
  quadSnapPosition,
  abSnapPosition,
  crossfaderHasAssignments,
} from "./scenes-morph.mjs";
import {
  CUSTOM_CURVES_KEY,
  DEFAULT_CURVE_ID,
  applySweepCurve,
  listCurveOptions,
} from "./sweep-curves.mjs";
import { bindXfPad, updatePadHandle, animatePadPosition, animateScalar } from "./scenes-xf-pad.mjs";

const STORE_PREFIX = "ob-scenes:v1:";
const SCENE_SLOTS = 4;
const EPS = 1e-4;
const MAX_RESULTS = 60;

// Pattern model. Elektron devices organise patterns as banks (A, B, …) of 16.
// The Digitakt VST exposes no pattern parameter, so the active pattern is chosen
// manually here (or followed from MIDI Program Change). Scenes are stored per
// pattern: each pattern keeps its own independent set of SCENE_SLOTS scenes.
const PATTERN_BANKS = 16; // A–P
const PATTERNS_PER_BANK = 16; // 1–16
const PATTERN_SEL_PREFIX = "ob-scenes:pattern:"; // remembers the chosen pattern per plugin

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

let plugin = null; // active plugin name, used to namespace stored scenes
let loadGeneration = 0; // ignore stale load() completions after plugin/pattern changes
let pattern = { bank: 0, num: 0 }; // active pattern (0-based bank + number)
let liveParams = []; // [{index,id,name,value,display,min,max,unit,...}]
const liveByIndex = new Map(); // index -> snapshot

let scenes = freshScenes(); // fixed 4 slots

function freshCrossfader() {
  return {
    mode: "ab",
    a: null,
    b: null,
    pos: 0,
    corners: { ...DEFAULT_QUAD_CORNERS },
    x: 0.5,
    y: 0.5,
    quadCenterMode: DEFAULT_QUAD_CENTER_MODE,
    quadReleaseSnap: DEFAULT_QUAD_RELEASE_SNAP,
    abReleaseSnap: DEFAULT_AB_RELEASE_SNAP,
    releaseSnapMs: DEFAULT_QUAD_RELEASE_SNAP_MS,
  };
}

let crossfader = freshCrossfader();
let baseline = new Map(); // index -> pattern baseline value, used when a crossfader side has no scene / value
let storedBaseline = null; // raw [{index,id,value}] from storage, awaiting resolution against live params
let baselineResolved = false; // true once this pattern's baseline is loaded or auto-seeded
let baselineExplicit = false; // true after Capture baseline — until then, live wins over auto-seed for empty sides
let autoBaselineOnPattern = false; // set on pattern switch → one explicit capture from live
let activeSceneId = "1"; // scene the picker adds to
let paramLearnSceneId = null; // scene id while waiting for a hardware wiggle
let paramLearnBaseline = null; // Map<index, value> snapshot at learn start

let ws = null;
const pendingApply = new Map(); // index -> value, flushed on rAF
let flushScheduled = false;
let activeSliderDrag = 0; // >0 while a scene slider is held — blocks row rebuild

// Soft-takeover anchor for the crossfader, captured when you grab the fader.
// { t0, per: Map<index, {v0, engaged}> } — v0 is each param's live value at the
// moment of grab, so Pickup/Scale reconcile against where the knobs actually are.
let xfGrab = null;
let quadSnapCancel = null;
let abSnapCancel = null;

// ---------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------

const el = {
  xfMode: document.getElementById("sc-xf-mode"),
  xfAb: document.getElementById("sc-xf-ab"),
  xfQuad: document.getElementById("sc-xf-quad"),
  xfAbFooter: document.getElementById("sc-xf-ab-footer"),
  assignA: document.getElementById("sc-assign-a"),
  assignB: document.getElementById("sc-assign-b"),
  assignTl: document.getElementById("sc-assign-tl"),
  assignTr: document.getElementById("sc-assign-tr"),
  assignBl: document.getElementById("sc-assign-bl"),
  assignBr: document.getElementById("sc-assign-br"),
  crossfader: document.getElementById("sc-crossfader"),
  percent: document.getElementById("sc-xf-percent"),
  quadReadout: document.getElementById("sc-xf-quad-readout"),
  nameA: document.getElementById("sc-xf-name-a"),
  nameB: document.getElementById("sc-xf-name-b"),
  xfPad: document.getElementById("sc-xf-pad"),
  xfPadHandle: document.getElementById("sc-xf-pad-handle"),
  xfPadLabelTl: document.getElementById("sc-xf-pad-label-tl"),
  xfPadLabelTr: document.getElementById("sc-xf-pad-label-tr"),
  xfPadLabelBl: document.getElementById("sc-xf-pad-label-bl"),
  xfPadLabelBr: document.getElementById("sc-xf-pad-label-br"),
  quadCenterMode: document.getElementById("sc-quad-center-mode"),
  quadReleaseSnap: document.getElementById("sc-quad-release-snap"),
  abReleaseSnap: document.getElementById("sc-ab-release-snap"),
  abOptionsPanel: document.getElementById("sc-ab-options-panel"),
  quadOptionsPanel: document.getElementById("sc-quad-options-panel"),
  quadOptionsMeta: document.getElementById("sc-quad-options-meta"),
  abOptionsMeta: document.getElementById("sc-ab-options-meta"),
  jumpA: document.getElementById("sc-jump-a"),
  jumpCenter: document.getElementById("sc-jump-center"),
  jumpB: document.getElementById("sc-jump-b"),
  captureBase: document.getElementById("sc-capture-base"),
  clockSlide: document.getElementById("sc-clock-slide"),
  clockBars: document.getElementById("sc-clock-bars"),
  clockSlideOnce: document.getElementById("sc-clock-slide-once"),
  clockCurve: document.getElementById("sc-clock-curve"),
  clockSlideStatus: document.getElementById("sc-clock-slide-status"),
  sliderMode: document.getElementById("sc-slider-mode"),
  midiInput: document.getElementById("sc-midi-input"),
  activeScene: document.getElementById("sc-active-scene"),
  pickerMeta: document.getElementById("sc-picker-meta"),
  search: document.getElementById("sc-param-search"),
  results: document.getElementById("sc-param-results"),
  scenes: document.getElementById("sc-scenes"),
  scenesMeta: document.getElementById("sc-scenes-meta"),
  patternBank: document.getElementById("sc-pattern-bank"),
  patternNum: document.getElementById("sc-pattern-num"),
  patternId: document.getElementById("sc-pattern-id"),
  pcFollow: document.getElementById("sc-pc-follow"),
  pcStatus: document.getElementById("sc-pc-status"),
  xfMidiInput: document.getElementById("sc-xf-midi-input"),
  midiMode: document.getElementById("sc-midi-mode"),
  midiStep: document.getElementById("sc-midi-step"),
  midiLearn: document.getElementById("sc-midi-learn"),
  midiClear: document.getElementById("sc-midi-clear"),
  midiConfigMeta: document.getElementById("sc-midi-config-meta"),
  midiMapDisplay: document.getElementById("sc-midi-map-display"),
  midiLog: document.getElementById("sc-midi-log"),
  midiLogMeta: document.getElementById("sc-midi-log-meta"),
  midiLogLines: document.getElementById("sc-midi-log-lines"),
  midiLogClear: document.getElementById("sc-midi-log-clear"),
  midiLogPause: document.getElementById("sc-midi-log-pause"),
};

// Soft-takeover mode for the crossfader — how the morph reconciles with each
// param's live (possibly knob-changed) value when you grab the fader:
//   jump      — value snaps to the morph position (classic absolute control)
//   pickup    — no change until the morph sweeps through the live value
//   scale     — value starts at the live value and scales smoothly to the
//               endpoints as the fader moves (a.k.a. "value scaling")
const SLIDER_MODE_KEY = "ob-scenes:slider-mode";
const SLIDER_MODES = ["jump", "pickup", "scale"];
let sliderMode = (() => {
  let stored = localStorage.getItem(SLIDER_MODE_KEY);
  if (stored === "interpolate" || stored === "scale-abs") stored = "scale"; // migrate old labels
  return SLIDER_MODES.includes(stored) ? stored : "jump";
})();

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function freshScenes() {
  return Array.from({ length: SCENE_SLOTS }, (_, i) => ({
    id: String(i + 1),
    name: `Scene ${i + 1}`,
    params: [], // [{index,id,name,value}]
  }));
}

function sceneById(id) {
  return scenes.find((s) => s.id === id) || null;
}

function liveValuesMap() {
  const m = new Map();
  for (const [idx, p] of liveByIndex) m.set(idx, p.value);
  return m;
}

function paramRangesMap() {
  const m = new Map();
  for (const [idx, p] of liveByIndex) {
    m.set(idx, { min: Number.isFinite(p.min) ? p.min : 0, max: Number.isFinite(p.max) ? p.max : 1 });
  }
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

function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function paramRange(index) {
  const p = liveByIndex.get(index);
  let min = p && Number.isFinite(p.min) ? p.min : 0;
  let max = p && Number.isFinite(p.max) ? p.max : 1;
  if (max === min) max = min + 1;
  return [min, max];
}

function liveValue(index) {
  const p = liveByIndex.get(index);
  return p ? p.value : undefined;
}

function fmt(v) {
  return Number(v).toFixed(3);
}

let toastTimer = null;
function toast(msg) {
  let t = document.querySelector(".sc-toast");
  if (!t) {
    t = document.createElement("div");
    t.className = "sc-toast";
    document.body.appendChild(t);
  }
  t.textContent = msg;
  requestAnimationFrame(() => t.classList.add("show"));
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove("show"), 1600);
}

// ---------------------------------------------------------------------------
// Persistence (namespaced per loaded plugin)
// ---------------------------------------------------------------------------

function bankLetter(b) {
  return String.fromCharCode(65 + clamp(b, 0, PATTERN_BANKS - 1));
}

function patternKey() {
  return bankLetter(pattern.bank) + String(pattern.num + 1).padStart(2, "0");
}

// Scenes are namespaced per plugin AND per pattern, so each pattern keeps its
// own 4 scenes.
function storeKey() {
  return STORE_PREFIX + (plugin || "default") + ":" + patternKey();
}

// Legacy (pre per-pattern) key — scenes used to be stored per plugin only. We
// migrate that data into the default pattern (A01) the first time it's opened.
function legacyStoreKey() {
  return STORE_PREFIX + (plugin || "default");
}

function scenesApiUrl() {
  const p = encodeURIComponent(plugin || "default");
  const pat = encodeURIComponent(patternKey());
  return `/api/scenes/${p}/${pat}`;
}

function activePatternApiUrl() {
  return `/api/scenes/${encodeURIComponent(plugin || "default")}/active`;
}

async function persistActivePattern() {
  if (!plugin) return;
  try {
    await fetch(activePatternApiUrl(), {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ pattern: patternKey() }),
    });
  } catch (e) {
    console.warn("active pattern save failed", e);
  }
}

function snapshotScenesPayload() {
  return {
    scenes,
    crossfader: {
      mode: crossfader.mode,
      a: crossfader.a,
      b: crossfader.b,
      corners: { ...crossfader.corners },
      x: crossfader.x,
      y: crossfader.y,
      quadCenterMode: crossfader.quadCenterMode,
      quadReleaseSnap: crossfader.quadReleaseSnap,
      abReleaseSnap: crossfader.abReleaseSnap,
      releaseSnapMs: crossfader.releaseSnapMs,
    },
    baseline: { explicit: baselineExplicit, values: serializeBaseline() },
  };
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
    crossfader.abReleaseSnap = normalized.abReleaseSnap;
    crossfader.releaseSnapMs = normalized.releaseSnapMs;
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

function readLocalScenesRaw() {
  let raw = localStorage.getItem(storeKey());
  if (!raw && pattern.bank === 0 && pattern.num === 0) {
    const legacy = localStorage.getItem(legacyStoreKey());
    if (legacy) {
      raw = legacy;
      try {
        localStorage.setItem(storeKey(), legacy);
        localStorage.removeItem(legacyStoreKey());
      } catch (e) {
        console.warn("scene migration failed", e);
      }
    }
  }
  return raw;
}

async function writeScenes() {
  const payload = snapshotScenesPayload();
  try {
    const res = await fetch(scenesApiUrl(), {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
  } catch (e) {
    console.warn("scene save to disk failed, keeping browser copy", e);
    try {
      localStorage.setItem(storeKey(), JSON.stringify(payload));
    } catch (err) {
      console.warn("scene save failed", err);
    }
  }
}

let saveTimer = null;
function save() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    writeScenes();
  }, 200);
}

async function saveNow() {
  clearTimeout(saveTimer);
  await writeScenes();
}

function flushScenesOnExit() {
  clearTimeout(saveTimer);
  const payload = JSON.stringify(snapshotScenesPayload());
  try {
    fetch(scenesApiUrl(), {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: payload,
      keepalive: true,
    });
  } catch (e) {
    try {
      localStorage.setItem(storeKey(), payload);
    } catch (_) {}
  }
}

async function load() {
  const gen = ++loadGeneration;
  scenes = freshScenes();
  crossfader = freshCrossfader();
  baseline = new Map();
  storedBaseline = null;
  baselineResolved = false;
  baselineExplicit = false;
  try {
    let data = null;
    const res = await fetch(scenesApiUrl());
    if (res.ok) {
      data = await res.json();
    } else if (res.status === 404) {
      const raw = readLocalScenesRaw();
      if (raw) {
        data = JSON.parse(raw);
        if (gen === loadGeneration) await writeScenes();
      }
    } else {
      throw new Error(`HTTP ${res.status}`);
    }
    if (gen !== loadGeneration) return;
    if (data) applyScenesPayload(data);
  } catch (e) {
    console.warn("scene load from disk failed, trying browser storage", e);
    try {
      const raw = readLocalScenesRaw();
      if (gen !== loadGeneration) return;
      if (raw) applyScenesPayload(JSON.parse(raw));
    } catch (err) {
      console.warn("scene load failed", err);
    }
  }
  if (gen !== loadGeneration) return;
  if (liveParams.length) validateScenes();
}

// Re-resolve stored param indices against the live parameter list (indices can
// shift between plugins / versions; ids and names are more stable). Mutates the
// existing param objects in place so event-handler closures (scene sliders)
// that captured them stay valid across periodic re-syncs.
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
      resolved.push(sp); // keep the same object — preserves closure identity
    }
    scene.params = resolved;
  }
  resolveBaseline();
}

// ---------------------------------------------------------------------------
// Pattern selection (scenes are stored per pattern)
// ---------------------------------------------------------------------------

function patternSelKey() {
  return PATTERN_SEL_PREFIX + (plugin || "default");
}

function persistPatternSel() {
  try {
    localStorage.setItem(patternSelKey(), JSON.stringify({ bank: pattern.bank, num: pattern.num }));
  } catch (e) {
    console.warn("pattern selection save failed", e);
  }
}

// Fetch the loaded plugin before the first scene load so disk paths match the device.
async function bootstrapPluginFromHost() {
  try {
    const res = await fetch("/api/selector");
    if (!res.ok) return;
    const data = await res.json();
    if (data.loaded_plugin) plugin = data.loaded_plugin;
  } catch (e) {
    console.warn("plugin bootstrap failed", e);
  }
}

function parsePatternKeyFromString(key) {
  const m = /^([A-P])(\d{1,2})$/i.exec(String(key || "").trim());
  if (!m) return null;
  const bank = m[1].toUpperCase().charCodeAt(0) - 65;
  const num = parseInt(m[2], 10) - 1;
  if (bank < 0 || bank >= PATTERN_BANKS || num < 0 || num >= PATTERNS_PER_BANK) return null;
  return { bank, num };
}

// Prefer the pattern last persisted by this or another client over localStorage.
async function restoreActivePatternFromServer() {
  if (!plugin) return;
  try {
    const res = await fetch(activePatternApiUrl());
    if (!res.ok) return;
    const data = await res.json();
    const parsed = parsePatternKeyFromString(data.pattern);
    if (parsed) pattern = parsed;
  } catch (e) {
    console.warn("active pattern restore failed", e);
  }
}

// Restore the last-selected pattern for the current plugin (defaults to A01).
function restorePatternSel() {
  pattern = { bank: 0, num: 0 };
  try {
    const raw = localStorage.getItem(patternSelKey());
    if (raw) {
      const d = JSON.parse(raw);
      pattern.bank = clamp(d.bank | 0, 0, PATTERN_BANKS - 1);
      pattern.num = clamp(d.num | 0, 0, PATTERNS_PER_BANK - 1);
    }
  } catch (e) {
    console.warn("pattern selection load failed", e);
  }
}

function fillPatternOptions() {
  if (!el.patternBank.options.length) {
    let banks = "";
    for (let b = 0; b < PATTERN_BANKS; b++) banks += `<option value="${b}">${bankLetter(b)}</option>`;
    el.patternBank.innerHTML = banks;
    let nums = "";
    for (let n = 0; n < PATTERNS_PER_BANK; n++) nums += `<option value="${n}">${n + 1}</option>`;
    el.patternNum.innerHTML = nums;
  }
}

function renderPattern() {
  fillPatternOptions();
  el.patternBank.value = String(pattern.bank);
  el.patternNum.value = String(pattern.num);
  el.patternId.textContent = patternKey();
}

// Switch the active pattern: persist the current pattern's scenes, then load the
// target pattern's own scene set. No values are pushed to the device on switch —
// only the editable scene set changes.
async function setPattern(bank, num, opts = {}) {
  bank = clamp(bank | 0, 0, PATTERN_BANKS - 1);
  num = clamp(num | 0, 0, PATTERNS_PER_BANK - 1);
  if (bank === pattern.bank && num === pattern.num) {
    renderPattern();
    return;
  }
  await saveNow();
  endXfGrab();
  pattern = { bank, num };
  persistPatternSel();
  void persistActivePattern();
  await load();
  autoBaselineOnPattern = true;
  tryAutoBaselineCapture();
  renderAll();
  if (opts.toast !== false) toast(`Pattern ${patternKey()}`);
}

// ---------------------------------------------------------------------------
// Crossfader morph engine (see scenes-morph.mjs)
// ---------------------------------------------------------------------------

function beginXfGrab() {
  xfGrab = beginXfGrabState(crossfader, scenes, morphCtx(), {
    ignoreStaleGrab: true,
  });
}

function cancelAbSnap() {
  if (abSnapCancel) {
    abSnapCancel();
    abSnapCancel = null;
  }
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

let xfGrabEndTimer = 0;
function scheduleEndXfGrab() {
  clearTimeout(xfGrabEndTimer);
  // Defer until after any trailing range `input` on pointer release.
  xfGrabEndTimer = setTimeout(() => {
    xfGrabEndTimer = 0;
    endXfGrab();
  }, 0);
}

function onQuadGrabEnd() {
  if (!isQuadMode()) {
    scheduleEndXfGrab();
    return;
  }

  const snap = crossfader.quadReleaseSnap ?? DEFAULT_QUAD_RELEASE_SNAP;
  const target = quadSnapPosition(snap);
  if (!target) {
    scheduleEndXfGrab();
    return;
  }

  if (
    Math.abs(crossfader.x - target.x) < EPS &&
    Math.abs(crossfader.y - target.y) < EPS
  ) {
    scheduleEndXfGrab();
    return;
  }

  cancelQuadSnap();
  const fromX = crossfader.x;
  const fromY = crossfader.y;
  const durationMs = crossfader.releaseSnapMs ?? DEFAULT_QUAD_RELEASE_SNAP_MS;

  quadSnapCancel = animatePadPosition(
    fromX,
    fromY,
    target.x,
    target.y,
    durationMs,
    (x, y) => {
      setQuadPos(x, y);
      updatePadHandle(el.xfPad, el.xfPadHandle, x, y);
      renderCrossfaderReadout();
      applyCrossfade({ force: true });
    },
    () => {
      quadSnapCancel = null;
      scheduleEndXfGrab();
      save();
    }
  );
}

function onAbGrabEnd() {
  if (isQuadMode()) {
    scheduleEndXfGrab();
    return;
  }

  const snap = crossfader.abReleaseSnap ?? DEFAULT_AB_RELEASE_SNAP;
  const target = abSnapPosition(snap);
  if (target == null) {
    scheduleEndXfGrab();
    return;
  }

  if (Math.abs(crossfader.pos - target) < EPS) {
    scheduleEndXfGrab();
    return;
  }

  cancelAbSnap();
  const from = crossfader.pos;
  const durationMs = crossfader.releaseSnapMs ?? DEFAULT_QUAD_RELEASE_SNAP_MS;

  abSnapCancel = animateScalar(
    from,
    target,
    durationMs,
    (pos) => {
      crossfader.pos = pos;
      el.crossfader.value = String(Math.round(pos * 1000));
      renderCrossfaderReadout();
      applyCrossfade({ force: true });
    },
    () => {
      abSnapCancel = null;
      scheduleEndXfGrab();
      save();
    }
  );
}

function applyCrossfade(opts = {}) {
  const force = !!opts.force;
  if (!shouldApplyCrossfade(xfGrab, sliderMode, { force })) return;
  const mode = force ? "jump" : sliderMode;
  const updates = computeCrossfadeUpdates(crossfader, scenes, morphCtx(), mode);
  for (const { index, value } of updates) {
    queueApply(index, value);
  }
  flushSoon();
}

// ---------------------------------------------------------------------------
// Pattern baseline — the neutral "home" snapshot used for any crossfader
// endpoint that has no scene assigned (None) or whose scene doesn't define a
// given parameter. Captured explicitly (button) or auto-seeded from the live
// snapshot the first time a pattern is opened, and persisted per pattern.
// ---------------------------------------------------------------------------

// Every parameter mapped in any of this pattern's 4 scenes — the only params the
// crossfader can morph, hence the only ones the baseline needs to cover.
function baselineParamIndices() {
  const set = new Set();
  for (const s of scenes) for (const p of s.params) set.add(p.index);
  return [...set];
}

function serializeBaseline() {
  const arr = [];
  for (const [index, value] of baseline) {
    const p = liveByIndex.get(index);
    arr.push({ index, id: p ? p.id : null, value });
  }
  return arr;
}

// Snapshot the current live values of every mapped param as this pattern's
// baseline, then persist. `silent` suppresses the toast (auto-seed on load).
// `explicit: true` from Capture baseline or an automatic capture on pattern switch.
function captureBaseline(opts = {}) {
  baseline = new Map();
  for (const index of baselineParamIndices()) {
    const lv = liveValue(index);
    if (lv !== undefined) baseline.set(index, lv);
  }
  baselineResolved = true;
  baselineExplicit = !!opts.explicit;
  writeScenes();
  if (!opts.silent) toast(`Baseline captured for ${patternKey()}`);
}

// After a pattern switch, snapshot live values once (same as Capture baseline).
function tryAutoBaselineCapture() {
  if (!autoBaselineOnPattern || !liveParams.length) return;
  autoBaselineOnPattern = false;
  captureBaseline({ silent: true, explicit: true });
}

// Seed a baseline value for any newly-mapped param from the current live
// snapshot, leaving existing baseline values untouched — keeps the baseline
// complete as scenes gain params without clobbering a captured home state.
function ensureBaselineCoverage() {
  let added = false;
  for (const index of baselineParamIndices()) {
    if (!baseline.has(index)) {
      const lv = liveValue(index);
      if (lv !== undefined) {
        baseline.set(index, lv);
        added = true;
      }
    }
  }
  if (added) writeScenes();
}

// Resolve the stored per-pattern baseline against the live parameter list once
// it's available; if none was ever saved for this pattern, auto-seed from the
// current snapshot. Runs from validateScenes whenever live params arrive.
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
    if (!autoBaselineOnPattern) {
      captureBaseline({ silent: true, explicit: false }); // first open → seed, live still wins
    }
  } else {
    ensureBaselineCoverage();
  }
}

// Recall a scene outright (independent of the A/B crossfader assignment).
function recallScene(scene) {
  if (!scene || !scene.params.length) return;
  for (const p of scene.params) queueApply(p.index, p.value);
  flushSoon();
  toast(`Recalled ${scene.name}`);
}

// ---------------------------------------------------------------------------
// Parameter writes (throttled, prefers WebSocket)
// ---------------------------------------------------------------------------

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
  updateLiveReadouts();
  fetch("/api/parameters/batch", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ updates }),
  }).catch(() => {});
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

function assignOptionsHtml(selected) {
  let html = `<option value="">— None —</option>`;
  for (const s of scenes) {
    const sel = s.id === selected ? " selected" : "";
    const count = s.params.length;
    html += `<option value="${s.id}"${sel}>${escapeHtml(s.name)}${count ? ` (${count})` : " (empty)"}</option>`;
  }
  return html;
}

function isQuadMode() {
  return crossfader.mode === "quad";
}

function sceneCornerRole(id) {
  if (!isQuadMode()) return null;
  for (const [corner, sceneId] of Object.entries(crossfader.corners)) {
    if (sceneId === id) return corner;
  }
  return null;
}

function cornerSceneName(corner) {
  const scene = sceneById(crossfader.corners[corner]);
  return scene ? scene.name : "Baseline";
}

function abReleaseSnapLabel(snap) {
  switch (snap) {
    case "none":
      return "Stay";
    case "a":
      return "Snap A";
    case "b":
      return "Snap B";
    default:
      return "Return center";
  }
}

function renderAbOptionsMeta() {
  if (!el.abOptionsMeta) return;
  el.abOptionsMeta.textContent = abReleaseSnapLabel(crossfader.abReleaseSnap);
}

function quadCenterModeLabel(mode) {
  return mode === "baseline" ? "Baseline" : "Scene blend";
}

function quadReleaseSnapLabel(snap) {
  switch (snap) {
    case "none":
      return "Stay";
    case "tl":
      return "Snap TL";
    case "tr":
      return "Snap TR";
    case "bl":
      return "Snap BL";
    case "br":
      return "Snap BR";
    default:
      return "Return center";
  }
}

function renderQuadOptionsMeta() {
  if (!el.quadOptionsMeta) return;
  el.quadOptionsMeta.textContent = `${quadCenterModeLabel(crossfader.quadCenterMode)} · ${quadReleaseSnapLabel(
    crossfader.quadReleaseSnap
  )}`;
}

function renderAssign() {
  el.assignA.innerHTML = assignOptionsHtml(crossfader.a);
  el.assignB.innerHTML = assignOptionsHtml(crossfader.b);
  if (el.assignTl) el.assignTl.innerHTML = assignOptionsHtml(crossfader.corners.tl);
  if (el.assignTr) el.assignTr.innerHTML = assignOptionsHtml(crossfader.corners.tr);
  if (el.assignBl) el.assignBl.innerHTML = assignOptionsHtml(crossfader.corners.bl);
  if (el.assignBr) el.assignBr.innerHTML = assignOptionsHtml(crossfader.corners.br);
  if (el.xfMode) el.xfMode.value = crossfader.mode;
  if (el.quadCenterMode) el.quadCenterMode.value = crossfader.quadCenterMode;
  if (el.quadReleaseSnap) el.quadReleaseSnap.value = crossfader.quadReleaseSnap;
  if (el.abReleaseSnap) el.abReleaseSnap.value = crossfader.abReleaseSnap;
  renderQuadOptionsMeta();
  renderAbOptionsMeta();
  el.activeScene.innerHTML = scenes
    .map(
      (s) =>
        `<option value="${s.id}"${s.id === activeSceneId ? " selected" : ""}>${escapeHtml(
          s.name
        )}</option>`
    )
    .join("");
  if (el.pickerMeta) {
    const scene = sceneById(activeSceneId);
    el.pickerMeta.textContent = scene ? scene.name : "";
  }
}

function renderCrossfaderLayout() {
  const quad = isQuadMode();
  el.xfAb?.classList.toggle("hidden", quad);
  el.xfQuad?.classList.toggle("hidden", !quad);
  el.xfAbFooter?.classList.toggle("hidden", quad);
  el.abOptionsPanel?.classList.toggle("hidden", quad);
  el.quadOptionsPanel?.classList.toggle("hidden", !quad);
}

function renderQuadPadLabels() {
  if (!el.xfPadLabelTl) return;
  el.xfPadLabelTl.textContent = cornerSceneName("tl");
  el.xfPadLabelTr.textContent = cornerSceneName("tr");
  el.xfPadLabelBl.textContent = cornerSceneName("bl");
  el.xfPadLabelBr.textContent = cornerSceneName("br");
}

function renderCrossfaderReadout() {
  renderCrossfaderLayout();
  if (isQuadMode()) {
    const xPct = Math.round(crossfader.x * 100);
    const yPct = Math.round(crossfader.y * 100);
    if (el.quadReadout) el.quadReadout.textContent = `${xPct}% · ${yPct}%`;
    renderQuadPadLabels();
    updatePadHandle(el.xfPad, el.xfPadHandle, crossfader.x, crossfader.y);
    return;
  }

  const pct = Math.round(crossfader.pos * 100);
  const a = sceneById(crossfader.a);
  const b = sceneById(crossfader.b);
  const aName = a ? a.name : "Baseline";
  const bName = b ? b.name : "Baseline";
  let side;
  if (pct <= 2) side = aName;
  else if (pct >= 98) side = bName;
  else side = `${aName} ↔ ${bName}`;
  el.percent.textContent = `${side} · ${pct}%`;
  el.nameA.textContent = aName;
  el.nameA.title = `Scene A: ${aName}`;
  el.nameB.textContent = bName;
  el.nameB.title = `Scene B: ${bName}`;
  el.crossfader.value = String(Math.round(crossfader.pos * 1000));
}

function renderScenesMeta() {
  if (!el.scenesMeta) return;
  el.scenesMeta.textContent = scenes
    .map((s) => `${s.name} (${s.params.length})`)
    .join(" · ");
}

function renderScenes() {
  // Rebuilding rows destroys the slider you're dragging (and its grab anchor).
  if (activeSliderDrag > 0) {
    updateLiveReadouts();
    return;
  }
  el.scenes.innerHTML = "";
  for (const scene of scenes) {
    el.scenes.appendChild(renderSceneCard(scene));
  }
  renderScenesMeta();
}

function renderSceneCard(scene) {
  const card = document.createElement("div");
  const isA = crossfader.a === scene.id;
  const isB = crossfader.b === scene.id;
  const corner = sceneCornerRole(scene.id);
  const cornerClass = corner ? ` assigned-${corner}` : "";
  card.className =
    "sc-scene" +
    (isA ? " assigned-a" : "") +
    (isB ? " assigned-b" : "") +
    cornerClass;

  let badge = "";
  let badgeClass = "";
  if (isQuadMode() && corner) {
    badge = corner.toUpperCase();
    badgeClass = ` corner-${corner}`;
  } else if (isA && isB) {
    badge = "AB";
    badgeClass = " a";
  } else if (isA) {
    badge = "A";
    badgeClass = " a";
  } else if (isB) {
    badge = "B";
    badgeClass = " b";
  }
  const badgeHtml = badge
    ? `<span class="sc-scene-badge ${badgeClass}">${badge}</span>`
    : `<span class="sc-scene-badge">${scene.id}</span>`;

  const head = document.createElement("div");
  head.className = "sc-scene-head";
  head.innerHTML = `
    ${badgeHtml}
    <input class="sc-scene-name" value="${escapeHtml(scene.name)}" aria-label="Scene name" />
    <span class="sc-scene-count">${scene.params.length} param${scene.params.length === 1 ? "" : "s"}</span>
  `;
  const nameInput = head.querySelector(".sc-scene-name");
  nameInput.addEventListener("change", () => {
    scene.name = nameInput.value.trim() || scene.name;
    save();
    renderAssign();
  });
  card.appendChild(head);

  const actions = document.createElement("div");
  actions.className = "sc-scene-actions";

  const recallBtn = mkBtn("Recall", "sc-btn-ghost sc-btn-sm", () => recallScene(scene));
  recallBtn.title = "Apply all of this scene's values to the device now";
  recallBtn.disabled = scene.params.length === 0;

  const snapBtn = mkBtn("Snapshot live", "sc-btn-sm", () => snapshotLive(scene));
  snapBtn.title = "Re-capture the live value of every parameter already in this scene";
  snapBtn.disabled = scene.params.length === 0;

  const learnBtn = mkBtn(
    paramLearnSceneId === scene.id ? "Learn…" : "Learn",
    "sc-btn-ghost sc-btn-sm" + (paramLearnSceneId === scene.id ? " sc-btn-active" : ""),
    () => toggleParamLearn(scene)
  );
  learnBtn.title =
    "Click, then move a hardware control — the parameter that changes is added to this scene";

  const editBtn = mkBtn("Add params ↑", "sc-btn-ghost sc-btn-sm", () => {
    cancelParamLearn();
    activeSceneId = scene.id;
    renderAssign();
    renderScenes();
    renderResults();
    el.search.focus();
    toast(`Adding to ${scene.name}`);
  });
  editBtn.title = "Make this the active scene for the parameter picker";

  const clearBtn = mkBtn("Clear", "sc-btn-danger sc-btn-sm", () => {
    if (scene.params.length === 0) return;
    scene.params = [];
    save();
    afterSceneMutation();
    toast(`Cleared ${scene.name}`);
  });
  clearBtn.disabled = scene.params.length === 0;

  actions.append(recallBtn, snapBtn, learnBtn, editBtn, clearBtn);
  card.appendChild(actions);

  const list = document.createElement("div");
  list.className = "sc-params";
  if (scene.params.length === 0) {
    const empty = document.createElement("div");
    empty.className = "sc-empty";
    empty.textContent =
      paramLearnSceneId === scene.id
        ? "Learn armed — move a hardware control."
        : "No parameters. Click Learn or use the picker above.";
    list.appendChild(empty);
  } else {
    for (const p of scene.params) list.appendChild(renderParamRow(scene, p));
  }
  card.appendChild(list);

  return card;
}

function renderParamRow(scene, p) {
  const row = document.createElement("div");
  row.className = "sc-param";
  row.dataset.index = String(p.index);

  const [min, max] = paramRange(p.index);
  const norm = (p.value - min) / (max - min);
  const lv = liveValue(p.index);

  row.innerHTML = `
    <div class="sc-param-top">
      <span class="sc-param-name" title="${escapeHtml(p.name)}">${escapeHtml(p.name)}</span>
      <button class="sc-icon-btn danger" data-del title="Remove from scene">✕</button>
    </div>
    <input type="range" min="0" max="1000" step="1" value="${Math.round(clamp(norm, 0, 1) * 1000)}" />
    <div class="sc-param-live" data-live>live: ${lv !== undefined ? fmt(lv) : "—"}</div>
  `;

  const slider = row.querySelector('input[type="range"]');

  // A scene-row slider edits this scene's stored value AND auditions it live:
  // dragging pushes the parameter straight to the device so you can dial the
  // scene in by ear. This is a direct edit, independent of the crossfader morph
  // (the morph only runs when you move the fader). We guard against the periodic
  // full-sync rebuilding the row mid-drag, which would reset the thumb.
  let dragging = false;
  function beginDrag() {
    if (dragging) return;
    dragging = true;
    activeSliderDrag++;
  }
  function endDrag() {
    if (!dragging) return;
    dragging = false;
    activeSliderDrag = Math.max(0, activeSliderDrag - 1);
  }
  slider.addEventListener("pointerdown", beginDrag, true);
  slider.addEventListener("pointerup", endDrag);
  slider.addEventListener("pointercancel", endDrag);
  slider.addEventListener("change", endDrag);

  slider.addEventListener("input", () => {
    const t = Number(slider.value) / 1000;
    // Edit the CURRENT scene param object (not the closed-over one, which a
    // periodic validateScenes() rebuild may have replaced) so the value sticks.
    const target = scene.params.find((x) => x.index === p.index) || p;
    target.value = min + t * (max - min);
    save();
    // Scene-state only: this sets the value the top crossfader morphs toward.
    // It must NOT touch the device — moving the fader applies it.
  });

  row.querySelector("[data-del]").addEventListener("click", () => {
    scene.params = scene.params.filter((x) => x.index !== p.index);
    save();
    afterSceneMutation();
  });

  return row;
}

function mkBtn(label, cls, onClick) {
  const b = document.createElement("button");
  b.className = "sc-btn " + cls;
  b.textContent = label;
  b.addEventListener("click", onClick);
  return b;
}

// Update only the "live: x" readouts + result list values without full re-render.
function updateLiveReadouts() {
  for (const row of el.scenes.querySelectorAll(".sc-param")) {
    const index = Number(row.dataset.index);
    const liveEl = row.querySelector("[data-live]");
    const lv = liveValue(index);
    if (liveEl) liveEl.textContent = `live: ${lv !== undefined ? fmt(lv) : "—"}`;
  }
  for (const row of el.results.querySelectorAll(".sc-result")) {
    const index = Number(row.dataset.index);
    const metaEl = row.querySelector(".sc-result-meta");
    const lv = liveValue(index);
    if (metaEl) metaEl.textContent = lv !== undefined ? fmt(lv) : "—";
  }
}

// ---------------------------------------------------------------------------
// Parameter picker
// ---------------------------------------------------------------------------

function renderResults() {
  const q = el.search.value.trim().toLowerCase();
  el.results.innerHTML = "";
  if (!liveParams.length) {
    el.results.innerHTML = `<div class="sc-empty">No parameters loaded yet…</div>`;
    return;
  }
  const scene = sceneById(activeSceneId);
  const inScene = new Set(scene ? scene.params.map((p) => p.index) : []);
  const matches = (q
    ? liveParams.filter((p) => p.name.toLowerCase().includes(q))
    : liveParams
  ).slice(0, MAX_RESULTS);

  for (const p of matches) {
    const row = document.createElement("div");
    row.className = "sc-result";
    row.dataset.index = String(p.index);
    const has = inScene.has(p.index);
    row.innerHTML = `
      <span class="sc-result-name">${escapeHtml(p.name)}</span>
      <div class="sc-result-right">
        <span class="sc-result-meta">${fmt(p.value)}</span>
        <button class="sc-btn sc-add-btn ${has ? "in-scene" : ""}" title="${
      has ? "Remove from scene" : "Add to scene (captures current value)"
    }">${has ? "✓" : "＋"}</button>
      </div>
    `;
    row.querySelector(".sc-add-btn").addEventListener("click", () =>
      toggleParamInActiveScene(p)
    );
    el.results.appendChild(row);
  }

  if (!q && liveParams.length > MAX_RESULTS) {
    const note = document.createElement("div");
    note.className = "sc-empty";
    note.textContent = `Showing ${MAX_RESULTS} of ${liveParams.length}. Type to filter.`;
    el.results.appendChild(note);
  }
}

function toggleParamInActiveScene(liveP) {
  const scene = sceneById(activeSceneId);
  if (!scene) return;
  const existing = scene.params.find((x) => x.index === liveP.index);
  if (existing) {
    scene.params = scene.params.filter((x) => x.index !== liveP.index);
    toast(`Removed ${liveP.name}`);
    save();
    afterSceneMutation();
  } else {
    addParamToScene(scene, liveP, { toastLabel: "Added" });
  }
}

const PARAM_LEARN_EPS = 1e-4;

function addParamToScene(scene, liveP, opts = {}) {
  const existing = scene.params.find((x) => x.index === liveP.index);
  if (existing) {
    existing.value = liveP.value;
    toast(`${opts.toastLabel || "Learned"} ${liveP.name} · updated value`);
  } else {
    scene.params.push({
      index: liveP.index,
      id: liveP.id,
      name: liveP.name,
      value: liveP.value,
    });
    toast(`${opts.toastLabel || "Learned"} ${liveP.name} → ${scene.name}`);
  }
  save();
  afterSceneMutation();
}

function toggleParamLearn(scene) {
  if (paramLearnSceneId === scene.id) {
    cancelParamLearn();
    toast("Learn cancelled");
    return;
  }
  paramLearnSceneId = scene.id;
  activeSceneId = scene.id;
  paramLearnBaseline = new Map();
  for (const p of liveParams) paramLearnBaseline.set(p.index, p.value);
  renderAssign();
  renderScenes();
  renderResults();
  toast(`Learn · move a control for ${scene.name}`);
}

function cancelParamLearn() {
  if (!paramLearnSceneId) return;
  paramLearnSceneId = null;
  paramLearnBaseline = null;
  renderScenes();
}

function tryParamLearnFromUpdates(updates) {
  if (!paramLearnSceneId || !paramLearnBaseline?.size) return;
  const scene = sceneById(paramLearnSceneId);
  if (!scene) {
    cancelParamLearn();
    return;
  }

  let bestIndex = null;
  let bestDelta = PARAM_LEARN_EPS;
  for (const u of updates) {
    if (!paramLearnBaseline.has(u.index)) continue;
    const delta = Math.abs(u.value - paramLearnBaseline.get(u.index));
    if (delta > bestDelta) {
      bestDelta = delta;
      bestIndex = u.index;
    }
  }
  if (bestIndex === null) return;

  const liveP = liveByIndex.get(bestIndex);
  if (!liveP) return;

  addParamToScene(scene, liveP);
  cancelParamLearn();
}

function snapshotLive(scene) {
  let n = 0;
  for (const p of scene.params) {
    const v = liveValue(p.index);
    if (v !== undefined) {
      p.value = v;
      n++;
    }
  }
  save();
  afterSceneMutation();
  toast(`Snapshotted ${n} live value${n === 1 ? "" : "s"}`);
}

// After any change to scene membership/values, refresh views + crossfader math.
function afterSceneMutation() {
  renderAssign();
  renderScenes();
  renderResults();
  ensureBaselineCoverage();
  if (crossfaderHasAssignments(crossfader, scenes)) {
    applyCrossfade();
  }
}

function reapplyIfAssigned(scene) {
  if (crossfader.a === scene.id || crossfader.b === scene.id) {
    applyCrossfade();
    return;
  }
  if (isQuadMode() && Object.values(crossfader.corners).includes(scene.id)) {
    applyCrossfade();
  }
}

// ---------------------------------------------------------------------------
// Crossfader interactions
// ---------------------------------------------------------------------------

el.assignA.addEventListener("change", () => {
  crossfader.a = el.assignA.value || null;
  save();
  ensureBaselineCoverage();
  renderScenes();
  renderCrossfaderReadout();
  applyCrossfade();
});

el.assignB.addEventListener("change", () => {
  crossfader.b = el.assignB.value || null;
  save();
  ensureBaselineCoverage();
  renderScenes();
  renderCrossfaderReadout();
  applyCrossfade();
});

function onCornerAssignChange(corner, selectEl) {
  crossfader.corners[corner] = selectEl.value || null;
  save();
  ensureBaselineCoverage();
  renderScenes();
  renderCrossfaderReadout();
  applyCrossfade();
}

el.assignTl?.addEventListener("change", () => onCornerAssignChange("tl", el.assignTl));
el.assignTr?.addEventListener("change", () => onCornerAssignChange("tr", el.assignTr));
el.assignBl?.addEventListener("change", () => onCornerAssignChange("bl", el.assignBl));
el.assignBr?.addEventListener("change", () => onCornerAssignChange("br", el.assignBr));

el.xfMode?.addEventListener("change", () => {
  crossfader.mode = el.xfMode.value === "quad" ? "quad" : "ab";
  save();
  renderCrossfaderReadout();
  renderScenes();
  if (crossfaderHasAssignments(crossfader, scenes)) applyCrossfade();
});

function setQuadPos(x, y) {
  crossfader.x = clamp(x, 0, 1);
  crossfader.y = clamp(y, 0, 1);
}

bindXfPad(el.xfPad, el.xfPadHandle, {
  getPos: () => ({ x: crossfader.x, y: crossfader.y }),
  setPos: (x, y) => setQuadPos(x, y),
  onGrabStart: () => {
    cancelQuadSnap();
    pauseClockSlideManual();
    beginXfGrab();
    if (sliderMode === "jump") applyCrossfade();
  },
  onGrabEnd: onQuadGrabEnd,
  onChange: () => {
    renderCrossfaderReadout();
    applyCrossfade();
  },
});

el.quadCenterMode?.addEventListener("change", () => {
  crossfader.quadCenterMode = el.quadCenterMode.value;
  renderQuadOptionsMeta();
  save();
  if (crossfaderHasAssignments(crossfader, scenes)) applyCrossfade();
});

el.quadReleaseSnap?.addEventListener("change", () => {
  crossfader.quadReleaseSnap = el.quadReleaseSnap.value;
  renderQuadOptionsMeta();
  save();
});

el.abReleaseSnap?.addEventListener("change", () => {
  crossfader.abReleaseSnap = el.abReleaseSnap.value;
  renderAbOptionsMeta();
  save();
});

function onCrossfaderGrabStart(ev) {
  cancelAbSnap();
  pauseClockSlideManual();
  ev.target.setPointerCapture?.(ev.pointerId);
  beginXfGrab();
  // Pickup/scale defer writes until the fader moves — avoid jumping to the
  // morph position on pointerdown when live differs from the slider.
  if (sliderMode === "jump") applyCrossfade();
}

el.crossfader.addEventListener("pointerdown", onCrossfaderGrabStart);
el.crossfader.addEventListener("pointerup", onAbGrabEnd);
el.crossfader.addEventListener("pointercancel", onAbGrabEnd);

el.crossfader.addEventListener("input", () => {
  pauseClockSlideManual();
  // Keyboard/programmatic moves arrive without a pointerdown — anchor on first
  // change so they still start from the live value rather than snapping.
  if (!xfGrab && !clockSlideDriving) beginXfGrab();
  crossfader.pos = Number(el.crossfader.value) / 1000;
  renderCrossfaderReadout();
  applyCrossfade();
});

function jumpTo(pos) {
  pauseClockSlideManual();
  // Jump buttons always snap, regardless of the selected takeover mode.
  clearTimeout(xfGrabEndTimer);
  xfGrabEndTimer = 0;
  cancelAbSnap();
  endXfGrab();
  crossfader.pos = pos;
  renderCrossfaderReadout();
  applyCrossfade({ force: true });
}

el.jumpA.addEventListener("click", () => jumpTo(0));
el.jumpCenter.addEventListener("click", () => jumpTo(0.5));
el.jumpB.addEventListener("click", () => jumpTo(1));

// ---------------------------------------------------------------------------
// Clock-driven crossfader slide (MIDI clock → 0..1 over N bars, then reset)
// ---------------------------------------------------------------------------

const CLOCK_SLIDE_KEY = "ob-scenes:clock-slide";
const MIDI_CLOCK_PPQ = 24;
const CLOCK_BEATS_PER_BAR = 4;

let clockSlideCfg = (() => {
  const def = { enabled: false, bars: 8, curve: DEFAULT_CURVE_ID };
  try {
    const raw = localStorage.getItem(CLOCK_SLIDE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      def.enabled = !!parsed.enabled;
      def.bars = parsed.bars ?? def.bars;
      def.curve = parsed.curve || DEFAULT_CURVE_ID;
    }
  } catch (e) {
    console.warn("clock slide cfg load failed", e);
  }
  def.bars = clamp(Math.round(def.bars) || 8, 1, 64);
  return def;
})();

function clockSlidePosition(linearT) {
  return applySweepCurve(linearT, clockSlideCfg.curve);
}

let clockState = {
  tick: 0,
  running: false,
  pausedByUser: false,
  awaitingSync: true,
  syncMode: "need_start", // need_start | next_cycle
};
let clockSlideDriving = false;
let clockSlideOneShot = {
  armed: false,
  running: false,
  originClocks: null,
};

// Keep phase while clock slide is off so re-enable can latch the next bar 1.
const TRANSPORT_ACTIVE_MS = 400;
let midiTransport = { running: false, clocks: 0, lastClockAt: 0 };

function transportPortMatch(port) {
  if (!pcCfg.inputId) return false;
  return portSelected(port, pcCfg.inputId);
}

function isTransportActive() {
  return (
    midiTransport.running &&
    performance.now() - midiTransport.lastClockAt < TRANSPORT_ACTIVE_MS
  );
}

function updateMidiTransport(port, bytes) {
  if (!bytes.length || !transportPortMatch(port)) return;

  for (let i = 0; i < bytes.length; ) {
    const status = bytes[i];
    if (status === 0xf2 && i + 2 < bytes.length) {
      const spp = (bytes[i + 2] << 7) | bytes[i + 1];
      midiTransport.clocks = Math.max(0, spp) * MIDI_CLOCK_PPQ;
      midiTransport.running = true;
      i += 3;
      continue;
    }
    if (status === 0xfa) {
      midiTransport.clocks = 0;
      midiTransport.running = true;
      i += 1;
      continue;
    }
    if (status === 0xfc) {
      midiTransport.running = false;
      i += 1;
      continue;
    }
    if (status === 0xfb) {
      midiTransport.running = true;
      i += 1;
      continue;
    }
    if (status === 0xf8) {
      midiTransport.lastClockAt = performance.now();
      if (midiTransport.running) midiTransport.clocks += 1;
      else {
        // Transport already playing (Start was missed while slide was off).
        midiTransport.running = true;
        midiTransport.clocks += 1;
      }
      i += 1;
      continue;
    }
    if (status >= 0xf0) {
      i += 1;
      continue;
    }
    break;
  }
}

function resetClockSlideAwaitingSync() {
  clockState.tick = 0;
  clockState.running = false;
  clockState.awaitingSync = true;
  clockState.syncMode = "need_start";
  renderClockSlideStatus();
}

function armClockSlideOnEnable() {
  clockState.pausedByUser = false;
  if (isTransportActive()) {
    const cycle = clockSlideCycleTicks();
    clockState.awaitingSync = true;
    clockState.syncMode = "next_cycle";
    clockState.running = true;
    if (midiTransport.clocks % cycle === 0) {
      clockState.awaitingSync = false;
      clockState.tick = 0;
      setCrossfaderPos(0);
    }
  } else {
    resetClockSlideAwaitingSync();
  }
  renderClockSlideStatus();
}

function syncClockSlideFromBeats(quarterNoteBeats) {
  const cycle = clockSlideCycleTicks();
  const clocks = Math.max(0, quarterNoteBeats) * MIDI_CLOCK_PPQ;
  midiTransport.clocks = clocks;
  midiTransport.running = true;
  clockState.tick = clocks % cycle;
  clockState.running = true;
  clockState.pausedByUser = false;
  clockState.awaitingSync = false;
  clockState.syncMode = null;
  applyClockSlidePos();
}

function applyClockSlidePos() {
  const cycle = clockSlideCycleTicks();
  const linearT = (clockState.tick % cycle) / cycle;
  setCrossfaderPos(clockSlidePosition(linearT));
  renderClockSlideStatus();
}

function onClockSlideClockTick() {
  const cycle = clockSlideCycleTicks();
  if (clockState.awaitingSync) {
    if (clockState.syncMode !== "next_cycle") {
      renderClockSlideStatus();
      return;
    }
    if (midiTransport.clocks % cycle !== 0) {
      renderClockSlideStatus();
      return;
    }
    clockState.awaitingSync = false;
    clockState.syncMode = null;
    clockState.tick = 0;
    if (clockSlideOneShot.armed) {
      beginClockSlideOneShotRun();
      renderClockSlideStatus();
      return;
    }
    setCrossfaderPos(0);
    return;
  }
  if (!clockState.running || clockState.pausedByUser) return;

  if (clockSlideOneShot.running) {
    const elapsed = midiTransport.clocks - (clockSlideOneShot.originClocks || 0);
    if (elapsed >= cycle) {
      finishClockSlideOneShot();
      return;
    }
    clockState.tick = elapsed;
    setCrossfaderPos(clockSlidePosition(elapsed / cycle));
    renderClockSlideStatus();
    return;
  }

  clockState.tick = midiTransport.clocks % cycle;
  applyClockSlidePos();
}

function resetClockSlideSequence() {
  clockState.tick = 0;
  clockState.awaitingSync = false;
  clockState.syncMode = null;
  if (clockState.running && !clockState.pausedByUser) setCrossfaderPos(0);
  else renderClockSlideStatus();
}

function saveClockSlideCfg() {
  try {
    localStorage.setItem(CLOCK_SLIDE_KEY, JSON.stringify(clockSlideCfg));
  } catch (e) {
    console.warn("clock slide cfg save failed", e);
  }
}

function clockSlideCycleTicks() {
  return clockSlideCfg.bars * CLOCK_BEATS_PER_BAR * MIDI_CLOCK_PPQ;
}

function isClockSlideEngaged() {
  return (
    clockSlideCfg.enabled ||
    clockSlideOneShot.armed ||
    clockSlideOneShot.running
  );
}

function cancelClockSlideOneShot() {
  clockSlideOneShot.armed = false;
  clockSlideOneShot.running = false;
  clockSlideOneShot.originClocks = null;
}

function beginClockSlideOneShotRun() {
  clockSlideOneShot.armed = false;
  clockSlideOneShot.running = true;
  clockSlideOneShot.originClocks = midiTransport.clocks;
  clockState.tick = 0;
  setCrossfaderPos(0);
}

function finishClockSlideOneShot() {
  const cycle = clockSlideCycleTicks();
  cancelClockSlideOneShot();
  setCrossfaderPos(1);
  if (clockSlideCfg.enabled) {
    clockState.running = true;
    clockState.pausedByUser = false;
    clockState.awaitingSync = false;
    clockState.syncMode = null;
    clockState.tick = midiTransport.clocks % cycle;
    applyClockSlidePos();
  } else {
    clockState.running = false;
    clockState.awaitingSync = true;
    clockState.syncMode = "need_start";
  }
  renderClockSlideStatus();
}

function armClockSlideOneShot() {
  if (!pcCfg.inputId) {
    toast("Select MIDI port in header");
    return;
  }
  if (clockSlideOneShot.armed || clockSlideOneShot.running) return;

  clockSlideOneShot.armed = true;
  clockState.pausedByUser = false;
  clockState.running = true;

  const cycle = clockSlideCycleTicks();
  if (isTransportActive()) {
    clockState.awaitingSync = true;
    clockState.syncMode = "next_cycle";
    if (midiTransport.clocks % cycle === 0) {
      clockState.awaitingSync = false;
      clockState.syncMode = null;
      beginClockSlideOneShotRun();
      toast(`One slide · ${clockSlideCfg.bars} bars`);
    } else {
      toast(`One slide · syncing at next bar 1`);
    }
  } else {
    clockState.awaitingSync = true;
    clockState.syncMode = "need_start";
    clockState.tick = 0;
    toast(`One slide · press Play to sync`);
  }
  renderClockSlideStatus();
}

function pauseClockSlideManual() {
  if (clockSlideOneShot.armed || clockSlideOneShot.running) {
    cancelClockSlideOneShot();
    clockState.running = false;
    clockState.awaitingSync = true;
    clockState.syncMode = "need_start";
    renderClockSlideStatus();
    return;
  }
  if (!clockSlideCfg.enabled || clockSlideDriving) return;
  clockState.pausedByUser = true;
  renderClockSlideStatus();
}

function setCrossfaderPos(pos) {
  clearTimeout(xfGrabEndTimer);
  xfGrabEndTimer = 0;
  cancelAbSnap();
  endXfGrab();
  clockSlideDriving = true;
  crossfader.pos = clamp(pos, 0, 1);
  el.crossfader.value = String(Math.round(crossfader.pos * 1000));
  renderCrossfaderReadout();
  applyCrossfade({ force: true });
  clockSlideDriving = false;
}

function renderClockSlideStatus() {
  if (!el.clockSlideStatus) return;
  if (!isClockSlideEngaged()) {
    el.clockSlideStatus.textContent = "";
    return;
  }
  if (!pcCfg.inputId) {
    el.clockSlideStatus.textContent = "select Digitakt port in header MIDI";
    return;
  }
  if (clockSlideOneShot.armed && clockState.awaitingSync) {
    if (clockState.syncMode === "next_cycle") {
      const cycle = clockSlideCycleTicks();
      const rem = cycle - (midiTransport.clocks % cycle);
      const barsLeft = Math.max(
        1,
        Math.ceil(rem / (CLOCK_BEATS_PER_BAR * MIDI_CLOCK_PPQ))
      );
      el.clockSlideStatus.textContent = `one slide · syncing at bar 1 · ${barsLeft} bar${barsLeft === 1 ? "" : "s"}`;
      return;
    }
    el.clockSlideStatus.textContent = "one slide · waiting for Start";
    return;
  }
  if (clockSlideOneShot.running) {
    const cycle = clockSlideCycleTicks();
    const elapsed = Math.max(0, midiTransport.clocks - (clockSlideOneShot.originClocks || 0));
    const t = Math.min(elapsed, cycle - 1);
    const bar = Math.floor(t / (CLOCK_BEATS_PER_BAR * MIDI_CLOCK_PPQ)) + 1;
    const beat =
      Math.floor((t % (CLOCK_BEATS_PER_BAR * MIDI_CLOCK_PPQ)) / MIDI_CLOCK_PPQ) + 1;
    el.clockSlideStatus.textContent = `one slide · bar ${bar}/${clockSlideCfg.bars} · beat ${beat}`;
    return;
  }
  if (!clockSlideCfg.enabled) {
    el.clockSlideStatus.textContent = "";
    return;
  }
  if (clockState.awaitingSync) {
    if (clockState.syncMode === "next_cycle") {
      const cycle = clockSlideCycleTicks();
      const rem = cycle - (midiTransport.clocks % cycle);
      const barsLeft = Math.max(
        1,
        Math.ceil(rem / (CLOCK_BEATS_PER_BAR * MIDI_CLOCK_PPQ))
      );
      el.clockSlideStatus.textContent = `syncing at next bar 1 · ${barsLeft} bar${barsLeft === 1 ? "" : "s"}`;
      return;
    }
    el.clockSlideStatus.textContent = "waiting for Start (press Play on the Digitakt)";
    return;
  }
  if (clockState.pausedByUser) {
    el.clockSlideStatus.textContent = "paused · manual override (Start resets)";
    return;
  }
  if (!clockState.running) {
    el.clockSlideStatus.textContent = "waiting for MIDI clock";
    return;
  }
  const cycle = clockSlideCycleTicks();
  const t = clockState.tick % cycle;
  const bar = Math.floor(t / (CLOCK_BEATS_PER_BAR * MIDI_CLOCK_PPQ)) + 1;
  const beat =
    Math.floor((t % (CLOCK_BEATS_PER_BAR * MIDI_CLOCK_PPQ)) / MIDI_CLOCK_PPQ) + 1;
  el.clockSlideStatus.textContent = `bar ${bar}/${clockSlideCfg.bars} · beat ${beat}`;
}

function handleClockSlide(port, bytes) {
  if (!bytes.length || !pcCfg.inputId) return;
  if (!portSelected(port, pcCfg.inputId)) return;
  if (!isClockSlideEngaged()) return;

  for (let i = 0; i < bytes.length; ) {
    const status = bytes[i];
    if (status === 0xf2 && i + 2 < bytes.length) {
      const spp = (bytes[i + 2] << 7) | bytes[i + 1];
      syncClockSlideFromBeats(spp);
      i += 3;
      continue;
    }
    if (status === 0xfa) {
      clockState.running = true;
      clockState.pausedByUser = false;
      clockState.awaitingSync = false;
      clockState.syncMode = null;
      clockState.tick = 0;
      if (clockSlideOneShot.armed) {
        beginClockSlideOneShotRun();
      } else {
        setCrossfaderPos(0);
      }
      renderClockSlideStatus();
      i += 1;
      continue;
    }
    if (status === 0xfc) {
      clockState.running = false;
      renderClockSlideStatus();
      i += 1;
      continue;
    }
    if (status === 0xfb) {
      clockState.running = true;
      clockState.pausedByUser = false;
      if (clockState.syncMode === "need_start") {
        clockState.awaitingSync = true;
      } else {
        clockState.awaitingSync = false;
        clockState.syncMode = null;
      }
      renderClockSlideStatus();
      i += 1;
      continue;
    }
    if (status === 0xf8) {
      onClockSlideClockTick();
      i += 1;
      continue;
    }
    if (status >= 0xf0) {
      i += 1;
      continue;
    }
    break;
  }
}

function populateClockSlideCurveSelect() {
  if (!el.clockCurve) return;
  const selected = clockSlideCfg.curve || DEFAULT_CURVE_ID;
  const options = listCurveOptions();
  el.clockCurve.replaceChildren();

  let currentGroup = null;
  for (const opt of options) {
    if (opt.group !== currentGroup) {
      currentGroup = opt.group;
      const group = document.createElement("optgroup");
      group.label = currentGroup === "custom" ? "Saved curves" : "Presets";
      el.clockCurve.append(group);
    }
    const option = document.createElement("option");
    option.value = opt.id;
    option.textContent = opt.name;
    el.clockCurve.lastElementChild.append(option);
  }

  if ([...el.clockCurve.options].some((o) => o.value === selected)) {
    el.clockCurve.value = selected;
  } else {
    clockSlideCfg.curve = DEFAULT_CURVE_ID;
    el.clockCurve.value = DEFAULT_CURVE_ID;
    saveClockSlideCfg();
  }
}

function renderClockSlideControls() {
  if (el.clockSlide) el.clockSlide.checked = clockSlideCfg.enabled;
  if (el.clockBars) el.clockBars.value = String(clockSlideCfg.bars);
  populateClockSlideCurveSelect();
  renderClockSlideStatus();
}

function renderSliderModeControl() {
  if (!el.sliderMode) return;
  el.sliderMode.value = sliderMode;
}

if (el.sliderMode) {
  el.sliderMode.addEventListener("change", () => {
    if (!SLIDER_MODES.includes(el.sliderMode.value)) return;
    sliderMode = el.sliderMode.value;
    localStorage.setItem(SLIDER_MODE_KEY, sliderMode);
    toast(`Crossfader takeover: ${sliderMode}`);
    if (xfGrab) applyCrossfade();
  });
}

if (el.clockSlide) {
  el.clockSlide.addEventListener("change", () => {
    clockSlideCfg.enabled = el.clockSlide.checked;
    if (clockSlideCfg.enabled) {
      armClockSlideOnEnable();
      toast(
        isTransportActive()
          ? `Clock slide · syncing at next bar 1 (${clockSlideCfg.bars} bars)`
          : `Clock slide · ${clockSlideCfg.bars} bars — press Play to sync`
      );
    }
    saveClockSlideCfg();
    renderClockSlideStatus();
  });
}
if (el.clockBars) {
  el.clockBars.addEventListener("change", () => {
    clockSlideCfg.bars = clamp(Math.round(Number(el.clockBars.value)) || 8, 1, 64);
    el.clockBars.value = String(clockSlideCfg.bars);
    saveClockSlideCfg();
    renderClockSlideStatus();
  });
}
if (el.clockCurve) {
  el.clockCurve.addEventListener("change", () => {
    clockSlideCfg.curve = el.clockCurve.value || DEFAULT_CURVE_ID;
    saveClockSlideCfg();
    if (isClockSlideEngaged() && clockState.running && !clockState.pausedByUser) {
      applyClockSlidePos();
    }
    renderClockSlideStatus();
  });
}
if (el.clockSlideOnce) {
  el.clockSlideOnce.addEventListener("click", () => armClockSlideOneShot());
}

window.addEventListener("storage", (e) => {
  if (e.key === CUSTOM_CURVES_KEY) populateClockSlideCurveSelect();
});

el.captureBase.addEventListener("click", () => {
  captureBaseline({ explicit: true });
});

el.activeScene.addEventListener("change", () => {
  activeSceneId = el.activeScene.value;
  renderResults();
});

el.search.addEventListener("input", renderResults);

document.addEventListener("keydown", (ev) => {
  if (ev.key === "Escape" && paramLearnSceneId) {
    cancelParamLearn();
    toast("Learn cancelled");
  }
});

// ---------------------------------------------------------------------------
// Live data: WebSocket + selector polling
// ---------------------------------------------------------------------------

function ingestParameters(list) {
  liveParams = list;
  liveByIndex.clear();
  for (const p of list) liveByIndex.set(p.index, p);
  if (paramLearnSceneId && paramLearnBaseline) {
    const updates = list
      .filter((p) => {
        const base = paramLearnBaseline.get(p.index);
        return base !== undefined && Math.abs(p.value - base) > PARAM_LEARN_EPS;
      })
      .map((p) => ({ index: p.index, value: p.value }));
    tryParamLearnFromUpdates(updates);
  }
  validateScenes();
  tryAutoBaselineCapture();
}

function applyParamUpdates(updates) {
  for (const u of updates) {
    const p = liveByIndex.get(u.index);
    if (p) {
      p.value = u.value;
      if (u.display !== undefined) p.display = u.display;
    }
  }
  tryParamLearnFromUpdates(updates);
  updateLiveReadouts();
}

// Identity of the current parameter set. The host re-broadcasts a full
// `parameters` snapshot every couple seconds; that carries fresh values but the
// same structure. We only want to rebuild the scene rows when the set actually
// changes (e.g. a plugin switch), because rebuilding mid-drag destroys the
// slider element you're holding — which wipes the Pickup/Scale grab anchor and
// resets the thumb. For value-only syncs we update readouts in place instead.
let lastParamSetSig = null;
function paramSetSignature(list) {
  if (!list || !list.length) return "0";
  return `${list.length}:${list[0].index}:${list[list.length - 1].index}`;
}

function connectWs() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  ws = new WebSocket(`${proto}://${location.host}/api/ws`);
  ws.onmessage = (ev) => {
    let msg;
    try {
      msg = JSON.parse(ev.data);
    } catch {
      return;
    }
    if (msg.type === "parameters") {
      ingestParameters(msg.data);
      const sig = paramSetSignature(liveParams);
      if (sig !== lastParamSetSig) {
        lastParamSetSig = sig;
        renderResults();
        renderScenes(); // no-ops row rebuild while a slider is held
      } else {
        updateLiveReadouts();
      }
    } else if (msg.type === "param_updates") {
      applyParamUpdates(msg.data);
    } else if (msg.type === "midi") {
      handleIncomingMidi(msg.port, msg.data, "host");
    }
  };
  ws.onclose = () => setTimeout(connectWs, 2000);
  ws.onerror = () => {};
}

// The device picker + connection status live in the shared global header
// (device-header.js). We only care which plugin is loaded, to namespace stored
// scenes. The header broadcasts selector data on every poll and on switches.
async function onSelector(data) {
  const loaded = data.loaded_plugin || null;
  if (loaded !== plugin) {
    cancelParamLearn();
    plugin = loaded;
    restorePatternSel();
    await load();
    renderAll();
  }
}

document.addEventListener("ob:selector", (ev) => onSelector(ev.detail));

async function initialParams() {
  try {
    const res = await fetch("/api/parameters");
    const data = await res.json();
    ingestParameters(data);
    renderAll();
  } catch (e) {
    console.warn("initial params failed", e);
  }
}

function renderAll() {
  renderPattern();
  renderAssign();
  renderCrossfaderReadout();
  renderScenes();
  renderResults();
}

el.patternBank.addEventListener("change", () =>
  setPattern(Number(el.patternBank.value), pattern.num)
);
el.patternNum.addEventListener("change", () =>
  setPattern(pattern.bank, Number(el.patternNum.value))
);

// ---------------------------------------------------------------------------
// MIDI — primary path is the ob-host WebSocket (system ports via midir, no
// browser permission). Optional Web MIDI fallback for localhost + Chrome/Edge.
// ---------------------------------------------------------------------------

const WEB_MIDI_KEY = "ob-scenes:web-midi";
const MIDI_KEY = "ob-scenes:midi";
let hostDebugMode = false;
let hostMidiPorts = [];
let hostMidiReady = false;
let webMidiEnabled = false;
let midiAccess = null;
let midiLearning = false;
let midiGestureActive = false;
let midiIdleTimer = null;
const MIDI_IDLE_MS = 500;

let midiCfg = (() => {
  const def = { inputId: null, channel: null, cc: null, mode: "absolute", step: 0.02 };
  try {
    const raw = localStorage.getItem(MIDI_KEY);
    if (raw) Object.assign(def, JSON.parse(raw));
  } catch (e) {
    console.warn("midi cfg load failed", e);
  }
  return def;
})();

function saveMidiCfg() {
  try {
    localStorage.setItem(MIDI_KEY, JSON.stringify(midiCfg));
  } catch (e) {
    console.warn("midi cfg save failed", e);
  }
}

const PC_KEY = "ob-scenes:pc-follow";
let lastPcKey = ""; // dedupe back-to-back identical PCs
let lastPcAt = 0;

let pcCfg = (() => {
  const def = { inputId: null, enabled: false };
  try {
    const raw = localStorage.getItem(PC_KEY);
    if (raw) Object.assign(def, JSON.parse(raw));
  } catch (e) {
    console.warn("pc cfg load failed", e);
  }
  return def;
})();

function savePcCfg() {
  try {
    localStorage.setItem(PC_KEY, JSON.stringify(pcCfg));
  } catch (e) {
    console.warn("pc cfg save failed", e);
  }
}

function setPcStatus(msg) {
  if (el.pcStatus) el.pcStatus.textContent = msg;
}

function updatePcFollowStatus() {
  if (!pcCfg.enabled) {
    setPcStatus("Program Change follow off");
    return;
  }
  if (!pcCfg.inputId) {
    setPcStatus("Select the Digitakt port in the header MIDI selector");
    return;
  }
  const label = inputLabel(pcCfg.inputId) || pcCfg.inputId;
  setPcStatus(`Following PC on ${label} · now ${patternKey()}`);
}

function hostPortLabel(id) {
  const p = hostMidiPorts.find((x) => x.id === id);
  return p ? p.name : null;
}

function webPortLabel(id) {
  if (!id?.startsWith("web:") || !midiAccess) return null;
  const inp = midiAccess.inputs.get(id.slice(4));
  return inp ? inp.name || inp.id : null;
}

function inputLabel(id) {
  if (!id) return null;
  return webPortLabel(id) || hostPortLabel(id) || id;
}

function portSelected(port, selectedId) {
  if (!selectedId || !port) return false;
  if (selectedId.startsWith("web:")) {
    if (!midiAccess) return false;
    const inp = midiAccess.inputs.get(selectedId.slice(4));
    if (!inp) return false;
    const webName = inp.name || inp.id;
    return port === webName || port === inp.id || port.includes(webName) || webName.includes(port);
  }
  return port === selectedId || port.includes(selectedId) || selectedId.includes(port);
}

function portIdFromName(port, source) {
  if (source === "web" && midiAccess) {
    for (const inp of midiAccess.inputs.values()) {
      if (port === inp.name || port === inp.id) return `web:${inp.id}`;
    }
  }
  for (const inp of hostMidiPorts) {
    if (port === inp.name || port === inp.id) return inp.id;
  }
  return null;
}

function portMatchesAnyHostPort(port) {
  if (!port) return false;
  return hostMidiPorts.some(
    (h) =>
      port === h.name ||
      port === h.id ||
      port.includes(h.name) ||
      h.name.includes(port)
  );
}

function hostPortIdForPortName(port) {
  if (!port) return null;
  const h = hostMidiPorts.find(
    (x) =>
      port === x.name ||
      port === x.id ||
      port.includes(x.name) ||
      x.name.includes(port)
  );
  return h ? h.id : null;
}

function resolveMidiPortId(selectedId) {
  if (!selectedId?.startsWith("web:")) return selectedId;
  const name = webPortLabel(selectedId);
  if (name && isRedundantWebMidi(name, "web")) {
    return hostPortIdForPortName(name) || selectedId;
  }
  return selectedId;
}

function isRedundantWebMidi(port, source) {
  return source === "web" && hostMidiReady && portMatchesAnyHostPort(port);
}

function handleIncomingMidi(port, data, source) {
  const bytes = data instanceof Uint8Array ? [...data] : [...(data || [])];
  if (isRedundantWebMidi(port, source)) return;

  if (hostDebugMode) appendMidiLogLine(`[${source}] ${port}`, bytes);

  updateMidiTransport(port, bytes);
  handleClockSlide(port, bytes);

  if (pcCfg.enabled && pcCfg.inputId && portSelected(port, pcCfg.inputId)) {
    handleProgramChange(bytes);
  }
  if ((bytes[0] & 0xf0) === 0xb0) {
    const xfPort =
      midiLearning || (midiCfg.inputId && portSelected(port, midiCfg.inputId));
    if (xfPort) handleControlChange(bytes, port, source);
  }
}

function handleProgramChange(bytes) {
  const [status, d1] = bytes;
  if ((status & 0xf0) !== 0xc0) return;
  const ch = (status & 0x0f) + 1;
  const p = d1 & 0x7f;
  const key = `${ch}:${p}`;
  const now = performance.now();
  if (key === lastPcKey && now - lastPcAt < 80) return;
  lastPcKey = key;
  lastPcAt = now;
  const bank = Math.floor(p / PATTERNS_PER_BANK) % PATTERN_BANKS;
  const num = p % PATTERNS_PER_BANK;
  setPattern(bank, num, { toast: false });
  if (clockSlideOneShot.armed || clockSlideOneShot.running) {
    cancelClockSlideOneShot();
    renderClockSlideStatus();
  }
  if (clockSlideCfg.enabled) resetClockSlideSequence();
  setPcStatus(`PC ch${ch} · ${p} → ${patternKey()}`);
}

function midiMapLabel() {
  if (midiCfg.cc === null) return "Crossfader not mapped";
  const ch = midiCfg.channel === null ? "any ch" : `ch ${midiCfg.channel + 1}`;
  return `${ch} · CC ${midiCfg.cc}`;
}

function renderMidiConfig() {
  const label = midiMapLabel();
  if (el.midiConfigMeta) el.midiConfigMeta.textContent = label;
  if (el.midiMapDisplay) {
    if (midiCfg.cc === null) {
      el.midiMapDisplay.textContent = "Not mapped — click Learn and move your fader or encoder.";
    } else {
      const inp = inputLabel(midiCfg.inputId) || "any input";
      const ch = midiCfg.channel === null ? "any" : midiCfg.channel + 1;
      el.midiMapDisplay.textContent = `${inp} · channel ${ch} · CC ${midiCfg.cc} · ${midiCfg.mode}`;
    }
  }
  if (el.midiMode) el.midiMode.value = midiCfg.mode;
  if (el.midiStep) el.midiStep.value = String(midiCfg.step);
  if (el.xfMidiInput && midiCfg.inputId) {
    const ids = [...el.xfMidiInput.options].map((o) => o.value);
    if (ids.includes(midiCfg.inputId)) el.xfMidiInput.value = midiCfg.inputId;
  }
}

function handleControlChange(bytes, port, source) {
  const [status, d1, d2] = bytes;
  const channel = status & 0x0f;

  if (midiLearning) {
    midiCfg.channel = channel;
    midiCfg.cc = d1;
    const id = portIdFromName(port, source);
    if (id) midiCfg.inputId = id;
    midiLearning = false;
    if (el.midiLearn) {
      el.midiLearn.classList.remove("sc-btn-active");
      el.midiLearn.textContent = "Learn";
    }
    saveMidiCfg();
    refreshMidiPortSelects();
    renderMidiConfig();
    toast(`Crossfader ← ${midiMapLabel()}`);
    return;
  }

  if (midiCfg.cc === null || d1 !== midiCfg.cc) return;
  if (midiCfg.channel !== null && channel !== midiCfg.channel) return;

  if (midiCfg.mode === "absolute") {
    midiApplyAbsolute(d2 / 127);
  } else {
    const delta = decodeRelative(midiCfg.mode, d2);
    if (delta !== 0) midiApplyRelative(delta * midiCfg.step);
  }
}

function decodeRelative(mode, v) {
  if (mode === "rel-signed") return v === 0 ? 0 : v < 64 ? v : v - 128;
  if (mode === "rel-offset") return v - 64;
  return 0;
}

function midiResetIdle() {
  clearTimeout(midiIdleTimer);
  midiIdleTimer = setTimeout(() => {
    if (midiGestureActive) {
      endXfGrab();
      midiGestureActive = false;
    }
  }, MIDI_IDLE_MS);
}

function midiCommitPos() {
  el.crossfader.value = String(Math.round(crossfader.pos * 1000));
  renderCrossfaderReadout();
  applyCrossfade();
}

function midiApplyAbsolute(pos) {
  pauseClockSlideManual();
  pos = clamp(pos, 0, 1);
  if (!xfGrab) {
    crossfader.pos = pos;
    beginXfGrab();
    midiGestureActive = true;
    el.crossfader.value = String(Math.round(crossfader.pos * 1000));
    renderCrossfaderReadout();
    applyCrossfade();
  } else {
    crossfader.pos = pos;
    midiCommitPos();
  }
  midiResetIdle();
}

function midiApplyRelative(step) {
  pauseClockSlideManual();
  if (!xfGrab) {
    beginXfGrab();
    midiGestureActive = true;
  }
  crossfader.pos = clamp(crossfader.pos + step, 0, 1);
  midiCommitPos();
  midiResetIdle();
}

// ---------------------------------------------------------------------------
// MIDI message log (host WebSocket + optional Web MIDI)
// ---------------------------------------------------------------------------

const MIDI_LOG_MAX = 400;
const midiLogTapped = new Map();
let midiLogCount = 0;
let midiLogPaused = false;

function midiLogActive() {
  return hostDebugMode && el.midiLog?.open && !midiLogPaused;
}

function detachWebMidiLogTaps() {
  for (const { inp, handler } of midiLogTapped.values()) {
    inp.removeEventListener("midimessage", handler);
  }
  midiLogTapped.clear();
}

function setHostDebugMode(debug) {
  hostDebugMode = !!debug;
  if (el.midiLog) el.midiLog.hidden = !hostDebugMode;
  if (!hostDebugMode) {
    clearMidiLog();
    detachWebMidiLogTaps();
  } else if (webMidiEnabled) {
    syncWebMidiLogTaps();
  }
}

async function fetchHostDebugMode() {
  try {
    const res = await fetch("/api/status");
    if (!res.ok) return;
    const data = await res.json();
    setHostDebugMode(data.debug);
  } catch (e) {
    console.warn("host status failed", e);
  }
}

function describeMidiBytes(data) {
  if (!data.length) return { kind: "other", label: "(empty)" };
  const status = data[0];
  const hi = status & 0xf0;
  const ch = (status & 0x0f) + 1;
  if (hi === 0x80 && data.length >= 3) return { kind: "other", label: `Note Off · ch ${ch} · note ${data[1]} · vel ${data[2]}` };
  if (hi === 0x90 && data.length >= 3) return { kind: "other", label: `Note On · ch ${ch} · note ${data[1]} · vel ${data[2]}` };
  if (hi === 0xa0 && data.length >= 3) return { kind: "other", label: `Poly AT · ch ${ch} · note ${data[1]} · ${data[2]}` };
  if (hi === 0xb0 && data.length >= 3) return { kind: "cc", label: `CC · ch ${ch} · cc ${data[1]} = ${data[2]}` };
  if (hi === 0xc0 && data.length >= 2) return { kind: "pc", label: `Program Change · ch ${ch} → ${data[1]}` };
  if (hi === 0xd0 && data.length >= 2) return { kind: "other", label: `Channel AT · ch ${ch} · ${data[1]}` };
  if (hi === 0xe0 && data.length >= 3) {
    const val = (data[2] << 7) | data[1];
    return { kind: "other", label: `Pitch Bend · ch ${ch} → ${val}` };
  }
  if (status === 0xf0) return { kind: "other", label: "SysEx start" };
  if (status === 0xf2) return { kind: "other", label: "Song Position" };
  if (status === 0xf8) return { kind: "other", label: "Clock" };
  if (status === 0xfa) return { kind: "other", label: "Start" };
  if (status === 0xfb) return { kind: "other", label: "Continue" };
  if (status === 0xfc) return { kind: "other", label: "Stop" };
  if (status >= 0xf0) return { kind: "other", label: `System · 0x${status.toString(16)}` };
  return { kind: "other", label: "Data" };
}

function webMidiDiagnostics() {
  const secure = window.isSecureContext;
  const api = typeof navigator.requestMIDIAccess === "function";
  const host = location.hostname;
  const local =
    host === "localhost" || host === "127.0.0.1" || host === "[::1]";
  return { secure, api, local, host };
}

function renderMidiLogMeta() {
  if (!hostDebugMode || !el.midiLogMeta) return;
  const { secure, api, local } = webMidiDiagnostics();
  const hostN = hostMidiPorts.length;
  const webN = midiAccess ? midiAccess.inputs.size : 0;
  const state = !el.midiLog?.open ? "paused (collapsed)" : midiLogPaused ? "paused" : "recording";
  const webHint = webMidiEnabled
    ? ` · Web MIDI ${webN} port${webN === 1 ? "" : "s"}`
    : !secure || !api
      ? " · Web MIDI unavailable (need localhost/HTTPS + Chrome/Edge)"
      : !local
        ? " · Web MIDI needs http://127.0.0.1 (not hostname/IP)"
        : " · Web MIDI optional";
  el.midiLogMeta.textContent = `host ${hostN} port${hostN === 1 ? "" : "s"}${webHint} · ${midiLogCount} line${midiLogCount === 1 ? "" : "s"} · ${state}`;
}

function appendMidiLogLine(portName, data) {
  if (!midiLogActive() || !el.midiLogLines) return;
  const bytes = [...data];
  const hex = bytes.map((b) => b.toString(16).padStart(2, "0")).join(" ");
  const { kind, label } = describeMidiBytes(bytes);
  const ts = new Date().toLocaleTimeString(undefined, {
    hour12: false,
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    fractionalSecondDigits: 3,
  });
  const line = document.createElement("div");
  line.className = `sc-midi-log-${kind}`;
  line.textContent = `${ts}  ${portName}  ${hex.padEnd(20)}  ${label}`;
  el.midiLogLines.appendChild(line);
  midiLogCount += 1;
  while (el.midiLogLines.childElementCount > MIDI_LOG_MAX) {
    el.midiLogLines.removeChild(el.midiLogLines.firstChild);
  }
  el.midiLogLines.scrollTop = el.midiLogLines.scrollHeight;
  renderMidiLogMeta();
}

function syncWebMidiLogTaps() {
  if (!hostDebugMode || !midiAccess) return;
  for (const inp of midiAccess.inputs.values()) {
    if (midiLogTapped.has(inp.id)) continue;
    const handler = (ev) => handleIncomingMidi(inp.name || inp.id, ev.data, "web");
    inp.addEventListener("midimessage", handler);
    midiLogTapped.set(inp.id, { inp, handler });
  }
  for (const [id, { inp, handler }] of midiLogTapped) {
    if (!midiAccess.inputs.has(id)) {
      inp.removeEventListener("midimessage", handler);
      midiLogTapped.delete(id);
    }
  }
  renderMidiLogMeta();
}

function clearMidiLog() {
  midiLogCount = 0;
  if (el.midiLogLines) el.midiLogLines.textContent = "";
  renderMidiLogMeta();
}

if (el.midiLog) {
  el.midiLog.addEventListener("toggle", renderMidiLogMeta);
}
if (el.midiLogClear) {
  el.midiLogClear.addEventListener("click", clearMidiLog);
}
if (el.midiLogPause) {
  el.midiLogPause.addEventListener("change", () => {
    midiLogPaused = el.midiLogPause.checked;
    renderMidiLogMeta();
  });
}

function fillMidiPortSelect(selectEl, selectedId, opts = {}) {
  if (!selectEl) return;
  const emptyLabel = opts.emptyLabel ?? "Off";
  selectEl.innerHTML = `<option value="">${emptyLabel}</option>`;
  const seenNames = new Set();
  for (const inp of hostMidiPorts) {
    if (seenNames.has(inp.name)) continue;
    seenNames.add(inp.name);
    const opt = document.createElement("option");
    opt.value = inp.id;
    opt.textContent = inp.name;
    selectEl.appendChild(opt);
  }
  if (webMidiEnabled && midiAccess) {
    for (const inp of midiAccess.inputs.values()) {
      const name = inp.name || inp.id;
      if (isRedundantWebMidi(name, "web")) continue;
      if (seenNames.has(name)) continue;
      seenNames.add(name);
      const opt = document.createElement("option");
      opt.value = `web:${inp.id}`;
      opt.textContent = name;
      selectEl.appendChild(opt);
    }
  }
  const ids = [...selectEl.options].map((o) => o.value);
  selectEl.value = ids.includes(selectedId) ? selectedId : "";
}

function refreshMidiPortSelects() {
  const pcId = resolveMidiPortId(pcCfg.inputId);
  const xfId = resolveMidiPortId(midiCfg.inputId);
  if (pcId !== pcCfg.inputId) {
    pcCfg.inputId = pcId;
    savePcCfg();
  }
  if (xfId !== midiCfg.inputId) {
    midiCfg.inputId = xfId;
    saveMidiCfg();
  }
  fillMidiPortSelect(el.midiInput, pcCfg.inputId);
  fillMidiPortSelect(el.xfMidiInput, midiCfg.inputId);
  renderClockSlideControls();
  renderMidiLogMeta();
  updatePcFollowStatus();
  renderMidiConfig();
}

async function initHostMidi() {
  el.pcFollow.checked = pcCfg.enabled;
  try {
    const res = await fetch("/api/midi/inputs");
    if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
    hostMidiPorts = await res.json();
    hostMidiReady = true;
    refreshMidiPortSelects();
    if (!hostMidiPorts.length) {
      setPcStatus("No MIDI inputs seen by host — restart ob-host after connecting the Digitakt");
    }
  } catch (e) {
    console.warn("host MIDI inputs failed", e);
    hostMidiReady = false;
    setPcStatus("Host MIDI unavailable — select pattern manually");
  }
  renderMidiLogMeta();
  void tryAutoWebMidi();
}

async function enableWebMidi() {
  const { secure, api, local } = webMidiDiagnostics();
  if (!secure || !api) return false;
  if (!local) return false;
  try {
    midiAccess = await navigator.requestMIDIAccess({ sysex: false });
  } catch (e) {
    console.warn("Web MIDI access denied", e);
    return false;
  }
  webMidiEnabled = true;
  try {
    localStorage.setItem(WEB_MIDI_KEY, "1");
  } catch (e) {
    console.warn("web midi pref save failed", e);
  }
  midiAccess.onstatechange = () => {
    syncWebMidiLogTaps();
    refreshMidiPortSelects();
  };
  syncWebMidiLogTaps();
  refreshMidiPortSelects();
  return true;
}

async function tryAutoWebMidi() {
  const { secure, api, local } = webMidiDiagnostics();
  if (!secure || !api || !local) return;
  let pref = false;
  try {
    pref = localStorage.getItem(WEB_MIDI_KEY) === "1";
  } catch (e) {
    console.warn("web midi pref load failed", e);
  }
  if (pref || !webMidiEnabled) await enableWebMidi();
}

el.midiInput.addEventListener("change", () => {
  pcCfg.inputId = el.midiInput.value || null;
  savePcCfg();
  updatePcFollowStatus();
  renderClockSlideStatus();
});

el.pcFollow.addEventListener("change", () => {
  pcCfg.enabled = el.pcFollow.checked;
  savePcCfg();
  updatePcFollowStatus();
});

el.xfMidiInput.addEventListener("change", () => {
  midiCfg.inputId = el.xfMidiInput.value || null;
  saveMidiCfg();
  renderMidiConfig();
});

el.midiMode.addEventListener("change", () => {
  midiCfg.mode = el.midiMode.value;
  saveMidiCfg();
  renderMidiConfig();
});

el.midiStep.addEventListener("change", () => {
  const v = parseFloat(el.midiStep.value);
  if (Number.isFinite(v) && v > 0) {
    midiCfg.step = v;
    saveMidiCfg();
    renderMidiConfig();
  }
});

el.midiLearn.addEventListener("click", () => {
  midiLearning = !midiLearning;
  if (el.midiLearn) {
    el.midiLearn.classList.toggle("sc-btn-active", midiLearning);
    el.midiLearn.textContent = midiLearning ? "Learning…" : "Learn";
  }
  if (midiLearning) {
    toast("Move the crossfader control on your MIDI device…");
  }
});

el.midiClear.addEventListener("click", () => {
  midiCfg.channel = null;
  midiCfg.cc = null;
  midiLearning = false;
  if (el.midiLearn) {
    el.midiLearn.classList.remove("sc-btn-active");
    el.midiLearn.textContent = "Learn";
  }
  saveMidiCfg();
  renderMidiConfig();
  toast("Crossfader MIDI mapping cleared");
});

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

window.addEventListener("pagehide", flushScenesOnExit);

(async () => {
  await fetchHostDebugMode();
  await bootstrapPluginFromHost();
  restorePatternSel();
  await restoreActivePatternFromServer();
  await load();
  void persistActivePattern();
  renderAll();
  initialParams();
  connectWs();
  initHostMidi();
  renderMidiConfig();
  renderClockSlideControls();
  renderSliderModeControl();
})();
