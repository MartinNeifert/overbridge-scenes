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
let pattern = { bank: 0, num: 0 }; // active pattern (0-based bank + number)
let liveParams = []; // [{index,id,name,value,display,min,max,unit,...}]
const liveByIndex = new Map(); // index -> snapshot

let scenes = freshScenes(); // fixed 4 slots
let crossfader = { a: null, b: null, pos: 0 }; // a/b = scene id | null, pos 0..1
let baseline = new Map(); // index -> neutral value used when a side is empty
let baselineArmed = false;
let activeSceneId = "1"; // scene the picker adds to

let ws = null;
const pendingApply = new Map(); // index -> value, flushed on rAF
let flushScheduled = false;
const lastSent = new Map(); // index -> last value we sent (dedupe)
let activeSliderDrag = 0; // >0 while a scene slider is held — blocks row rebuild

// Soft-takeover anchor for the crossfader, captured when you grab the fader.
// { t0, per: Map<index, {v0, engaged}> } — v0 is each param's live value at the
// moment of grab, so Pickup/Scale reconcile against where the knobs actually are.
let xfGrab = null;

// ---------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------

const el = {
  assignA: document.getElementById("sc-assign-a"),
  assignB: document.getElementById("sc-assign-b"),
  crossfader: document.getElementById("sc-crossfader"),
  percent: document.getElementById("sc-xf-percent"),
  nameA: document.getElementById("sc-xf-name-a"),
  nameB: document.getElementById("sc-xf-name-b"),
  jumpA: document.getElementById("sc-jump-a"),
  jumpCenter: document.getElementById("sc-jump-center"),
  jumpB: document.getElementById("sc-jump-b"),
  captureBase: document.getElementById("sc-capture-base"),
  activeScene: document.getElementById("sc-active-scene"),
  sliderMode: document.getElementById("sc-slider-mode"),
  search: document.getElementById("sc-param-search"),
  results: document.getElementById("sc-param-results"),
  scenes: document.getElementById("sc-scenes"),
  midiInput: document.getElementById("sc-midi-input"),
  midiMode: document.getElementById("sc-midi-mode"),
  midiStep: document.getElementById("sc-midi-step"),
  midiLearn: document.getElementById("sc-midi-learn"),
  midiStatus: document.getElementById("sc-midi-status"),
  patternBank: document.getElementById("sc-pattern-bank"),
  patternNum: document.getElementById("sc-pattern-num"),
  patternId: document.getElementById("sc-pattern-id"),
  pcFollow: document.getElementById("sc-pc-follow"),
  pcInput: document.getElementById("sc-pc-input"),
  pcStatus: document.getElementById("sc-pc-status"),
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

function clamp(v, lo, hi) {
  return Math.min(hi, Math.max(lo, v));
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

function writeScenes() {
  try {
    localStorage.setItem(
      storeKey(),
      JSON.stringify({ scenes, crossfader: { a: crossfader.a, b: crossfader.b } })
    );
  } catch (e) {
    console.warn("scene save failed", e);
  }
}

let saveTimer = null;
function save() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(writeScenes, 200);
}

// Flush a pending debounced save immediately — used before switching patterns so
// the current pattern's edits aren't lost to the still-pending timer.
function saveNow() {
  clearTimeout(saveTimer);
  writeScenes();
}

function load() {
  scenes = freshScenes();
  crossfader = { a: null, b: null, pos: 0 };
  baseline = new Map();
  baselineArmed = false;
  try {
    let raw = localStorage.getItem(storeKey());
    // One-time migration: pre-pattern scenes land in the default pattern (A01).
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
    if (raw) {
      const data = JSON.parse(raw);
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
    }
  } catch (e) {
    console.warn("scene load failed", e);
  }
  if (liveParams.length) validateScenes();
}

// Re-resolve stored param indices against the live parameter list (indices can
// shift between plugins / versions; ids and names are more stable).
function validateScenes() {
  const byId = new Map();
  const byName = new Map();
  for (const p of liveParams) {
    byId.set(p.id, p);
    byName.set(p.name.toLowerCase(), p);
  }
  for (const scene of scenes) {
    scene.params = scene.params
      .map((sp) => {
        let live = sp.id != null ? byId.get(sp.id) : undefined;
        if (!live && sp.name) live = byName.get(sp.name.toLowerCase());
        if (!live && liveByIndex.has(sp.index)) live = liveByIndex.get(sp.index);
        if (!live) return null;
        return { index: live.index, id: live.id, name: live.name, value: sp.value };
      })
      .filter(Boolean);
  }
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
function setPattern(bank, num, opts = {}) {
  bank = clamp(bank | 0, 0, PATTERN_BANKS - 1);
  num = clamp(num | 0, 0, PATTERNS_PER_BANK - 1);
  if (bank === pattern.bank && num === pattern.num) {
    renderPattern();
    return;
  }
  saveNow();
  endXfGrab();
  pattern = { bank, num };
  persistPatternSel();
  load();
  renderAll();
  if (opts.toast !== false) toast(`Pattern ${patternKey()}`);
}

// ---------------------------------------------------------------------------
// Crossfader morph engine
// ---------------------------------------------------------------------------

function unionIndices() {
  const a = sceneById(crossfader.a);
  const b = sceneById(crossfader.b);
  const set = new Set();
  if (a) for (const p of a.params) set.add(p.index);
  if (b) for (const p of b.params) set.add(p.index);
  return [...set];
}

function baseValue(index) {
  if (baseline.has(index)) return baseline.get(index);
  const lv = liveValue(index);
  return lv !== undefined ? lv : 0;
}

// Endpoint value for one side: explicit scene lock wins, otherwise neutral base.
function endpointValue(scene, index) {
  if (scene) {
    const p = scene.params.find((x) => x.index === index);
    if (p) return p.value;
  }
  return baseValue(index);
}

function captureBaseline() {
  baseline = new Map();
  for (const index of unionIndices()) {
    const lv = liveValue(index);
    baseline.set(index, lv !== undefined ? lv : endpointValue(sceneById(crossfader.a), index));
  }
  baselineArmed = true;
}

// Captured when the crossfader is grabbed. Records each param's live value at
// the moment of grab so Pickup/Scale can reconcile against where the knobs
// actually are. We deliberately do NOT refresh the baseline here: the morph
// trajectory (A→B) must stay fixed so Pickup has a path to sweep through the
// live value and Scale has real endpoints to land on.
function beginXfGrab() {
  if (!baselineArmed) captureBaseline();
  const per = new Map();
  for (const index of unionIndices()) {
    const lv = liveValue(index);
    per.set(index, {
      v0: lv !== undefined ? lv : baseValue(index),
      engaged: false,
    });
  }
  xfGrab = { t0: crossfader.pos, per };
}

function endXfGrab() {
  xfGrab = null;
}

function applyCrossfade() {
  const a = sceneById(crossfader.a);
  const b = sceneById(crossfader.b);
  if (!a && !b) return;
  const t = crossfader.pos;
  // Modes only apply during a live grab; programmatic moves (jump buttons,
  // assignment changes) snap straight to the morph.
  const mode = xfGrab ? sliderMode : "jump";
  const t0 = xfGrab ? xfGrab.t0 : 0;

  for (const index of unionIndices()) {
    const av = endpointValue(a, index);
    const bv = endpointValue(b, index);
    const ideal = av + (bv - av) * t; // absolute morph value at this position
    let value = ideal;

    const g = mode === "jump" ? null : xfGrab.per.get(index);
    if (g) {
      const v0 = g.v0;
      if (mode === "pickup") {
        // Hold the live value until the swept morph range [grab → now] reaches
        // it, then take over. A range test (not an exact == ) tolerates the
        // fader's discrete steps so it reliably engages on the way through.
        if (!g.engaged) {
          const ideal0 = av + (bv - av) * t0;
          const lo = Math.min(ideal0, ideal);
          const hi = Math.max(ideal0, ideal);
          if (v0 >= lo - EPS && v0 <= hi + EPS) g.engaged = true;
        }
        value = g.engaged ? ideal : v0;
      } else {
        // scale: piecewise-linear through (t0, v0) so the value starts at the
        // live value and still reaches each endpoint exactly.
        if (t >= t0) {
          value = t0 < 1 ? v0 + (bv - v0) * ((t - t0) / (1 - t0)) : v0;
        } else {
          value = t0 > 0 ? av + (v0 - av) * (t / t0) : v0;
        }
      }
    }

    const [min, max] = paramRange(index);
    value = clamp(value, Math.min(min, max), Math.max(min, max));
    queueApply(index, value);
  }
  flushSoon();
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
  const open = ws && ws.readyState === WebSocket.OPEN;
  for (const [index, value] of pendingApply) {
    const prev = lastSent.get(index);
    if (prev !== undefined && Math.abs(prev - value) < EPS) continue;
    lastSent.set(index, value);
    // optimistic local update so capture/readouts feel instant
    const p = liveByIndex.get(index);
    if (p) p.value = value;
    if (open) {
      ws.send(JSON.stringify({ action: "set_parameter", index, value }));
    } else {
      fetch(`/api/parameters/${index}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ value }),
      }).catch(() => {});
    }
  }
  pendingApply.clear();
  updateLiveReadouts();
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

function renderAssign() {
  el.assignA.innerHTML = assignOptionsHtml(crossfader.a);
  el.assignB.innerHTML = assignOptionsHtml(crossfader.b);
  el.activeScene.innerHTML = scenes
    .map(
      (s) =>
        `<option value="${s.id}"${s.id === activeSceneId ? " selected" : ""}>${escapeHtml(
          s.name
        )}</option>`
    )
    .join("");
}

function renderCrossfaderReadout() {
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
}

function renderSceneCard(scene) {
  const card = document.createElement("div");
  const isA = crossfader.a === scene.id;
  const isB = crossfader.b === scene.id;
  card.className =
    "sc-scene" + (isA ? " assigned-a" : "") + (isB ? " assigned-b" : "");

  const badge = isA && isB ? "AB" : isA ? "A" : isB ? "B" : "";
  const badgeClass = isA ? "a" : isB ? "b" : "";
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

  const editBtn = mkBtn("Add params ↑", "sc-btn-ghost sc-btn-sm", () => {
    activeSceneId = scene.id;
    renderAssign();
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

  actions.append(recallBtn, snapBtn, editBtn, clearBtn);
  card.appendChild(actions);

  const list = document.createElement("div");
  list.className = "sc-params";
  if (scene.params.length === 0) {
    const empty = document.createElement("div");
    empty.className = "sc-empty";
    empty.textContent = "No parameters. Use the picker above to map some.";
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
      <div class="sc-param-tools">
        <span class="sc-param-val" data-val>= ${fmt(p.value)}</span>
        <button class="sc-icon-btn" data-map title="Map: save current live value here">⤓</button>
        <button class="sc-icon-btn danger" data-del title="Remove from scene">✕</button>
      </div>
    </div>
    <input type="range" min="0" max="1000" step="1" value="${Math.round(clamp(norm, 0, 1) * 1000)}" />
    <div class="sc-param-live" data-live>live: ${lv !== undefined ? fmt(lv) : "—"}</div>
  `;

  const slider = row.querySelector('input[type="range"]');
  const valEl = row.querySelector("[data-val]");

  // A scene-row slider just edits this scene's stored value. (Soft takeover —
  // Jump/Pickup/Scale — lives on the crossfader, not here.) We only guard
  // against the periodic full-sync rebuilding the row mid-drag, which would
  // otherwise reset the thumb under the user's finger.
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
    p.value = min + t * (max - min);
    valEl.textContent = `= ${fmt(p.value)}`;
    save();
    reapplyIfAssigned(scene);
  });

  row.querySelector("[data-map]").addEventListener("click", () => {
    const v = liveValue(p.index);
    if (v === undefined) {
      toast("No live value yet");
      return;
    }
    p.value = v;
    save();
    const t = (p.value - min) / (max - min);
    slider.value = String(Math.round(clamp(t, 0, 1) * 1000));
    valEl.textContent = `= ${fmt(p.value)}`;
    toast(`Mapped ${p.name}`);
    reapplyIfAssigned(scene);
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
  } else {
    scene.params.push({
      index: liveP.index,
      id: liveP.id,
      name: liveP.name,
      value: liveP.value, // capture current live value
    });
    toast(`Mapped ${liveP.name} → ${scene.name}`);
  }
  save();
  afterSceneMutation();
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
  if (crossfader.a || crossfader.b) {
    captureBaseline();
    applyCrossfade();
  }
}

function reapplyIfAssigned(scene) {
  if (crossfader.a === scene.id || crossfader.b === scene.id) applyCrossfade();
}

// ---------------------------------------------------------------------------
// Crossfader interactions
// ---------------------------------------------------------------------------

el.assignA.addEventListener("change", () => {
  crossfader.a = el.assignA.value || null;
  save();
  captureBaseline();
  renderScenes();
  renderCrossfaderReadout();
  applyCrossfade();
});

el.assignB.addEventListener("change", () => {
  crossfader.b = el.assignB.value || null;
  save();
  captureBaseline();
  renderScenes();
  renderCrossfaderReadout();
  applyCrossfade();
});

el.crossfader.addEventListener("pointerdown", beginXfGrab);
el.crossfader.addEventListener("pointerup", endXfGrab);
el.crossfader.addEventListener("pointercancel", endXfGrab);

el.crossfader.addEventListener("input", () => {
  // Keyboard/programmatic moves arrive without a pointerdown — anchor on first
  // change so they still start from the live value rather than snapping.
  if (!xfGrab) beginXfGrab();
  crossfader.pos = Number(el.crossfader.value) / 1000;
  renderCrossfaderReadout();
  applyCrossfade();
});

function jumpTo(pos) {
  // Jump buttons always snap, regardless of the selected takeover mode.
  endXfGrab();
  if (!baselineArmed) captureBaseline();
  crossfader.pos = pos;
  renderCrossfaderReadout();
  applyCrossfade();
}

el.jumpA.addEventListener("click", () => jumpTo(0));
el.jumpCenter.addEventListener("click", () => jumpTo(0.5));
el.jumpB.addEventListener("click", () => jumpTo(1));

el.captureBase.addEventListener("click", () => {
  captureBaseline();
  toast("Baseline captured from live");
});

el.activeScene.addEventListener("change", () => {
  activeSceneId = el.activeScene.value;
  renderResults();
});

el.search.addEventListener("input", renderResults);

el.sliderMode.value = sliderMode;
el.sliderMode.addEventListener("change", () => {
  sliderMode = el.sliderMode.value;
  localStorage.setItem(SLIDER_MODE_KEY, sliderMode);
  toast(`Crossfader takeover: ${sliderMode}`);
});

// ---------------------------------------------------------------------------
// Live data: WebSocket + selector polling
// ---------------------------------------------------------------------------

function ingestParameters(list) {
  liveParams = list;
  liveByIndex.clear();
  for (const p of list) liveByIndex.set(p.index, p);
  validateScenes();
}

function applyParamUpdates(updates) {
  for (const u of updates) {
    const p = liveByIndex.get(u.index);
    if (p) {
      p.value = u.value;
      if (u.display !== undefined) p.display = u.display;
    }
  }
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
    }
  };
  ws.onclose = () => setTimeout(connectWs, 2000);
  ws.onerror = () => {};
}

// The device picker + connection status live in the shared global header
// (device-header.js). We only care which plugin is loaded, to namespace stored
// scenes. The header broadcasts selector data on every poll and on switches.
function onSelector(data) {
  const loaded = data.loaded_plugin || null;
  if (loaded !== plugin) {
    plugin = loaded;
    restorePatternSel(); // remember the pattern last edited for this plugin
    load(); // load scenes for this plugin + pattern namespace
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
// MIDI control of the crossfader (Web MIDI API)
//
// Supports two controller styles:
//   absolute   — a normal 0–127 fader/knob; value maps straight to fader pos
//   rel-signed — endless encoder, two's-complement (1..63 = +, 127..65 = −)
//   rel-offset — endless encoder, 64-centred (65 = +1, 63 = −1)
// MIDI moves drive the same morph engine + takeover modes as the on-screen
// fader: a turn/move starts a grab (anchoring the live values) and ends after a
// short idle, so Pickup/Scale reconcile exactly as they do for mouse drags.
// ---------------------------------------------------------------------------

const MIDI_KEY = "ob-scenes:midi";
let midiAccess = null;
let midiInput = null; // currently connected MIDIInput
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

// Program Change follow — the device's only live signal of the active pattern.
// Elektron devices send a Program Change (0–127) when the pattern changes
// (requires "Program Change Send" enabled on the device). PC n maps to bank
// floor(n/16) + pattern (n mod 16): PC 0 → A01, PC 16 → B01, … PC 127 → H16.
const PC_KEY = "ob-scenes:pc-follow";
let pcInput = null; // currently connected MIDIInput for program change

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

function midiMapLabel() {
  if (midiCfg.cc === null) return "no control mapped";
  const ch = midiCfg.channel === null ? "any" : midiCfg.channel + 1;
  return `CC ${midiCfg.cc} · ch ${ch}`;
}

function setMidiStatus(msg) {
  if (el.midiStatus) el.midiStatus.textContent = msg;
}

async function initMidi() {
  el.midiMode.value = midiCfg.mode;
  el.midiStep.value = String(midiCfg.step);
  el.pcFollow.checked = pcCfg.enabled;
  if (!navigator.requestMIDIAccess) {
    setMidiStatus("Web MIDI not supported in this browser");
    el.midiInput.disabled = true;
    el.midiLearn.disabled = true;
    el.pcInput.disabled = true;
    el.pcFollow.disabled = true;
    setPcStatus("Web MIDI not supported — select pattern manually");
    return;
  }
  try {
    midiAccess = await navigator.requestMIDIAccess({ sysex: false });
  } catch (e) {
    console.warn("MIDI access denied", e);
    setMidiStatus("MIDI access denied");
    return;
  }
  midiAccess.onstatechange = populateMidiInputs;
  populateMidiInputs();
  if (midiCfg.inputId) connectMidiInput(midiCfg.inputId);
  if (pcCfg.enabled && pcCfg.inputId) connectPcInput(pcCfg.inputId);
}

function populateMidiInputs() {
  const inputs = midiAccess ? [...midiAccess.inputs.values()] : [];
  const current = midiCfg.inputId || "";
  el.midiInput.innerHTML = '<option value="">Off</option>';
  for (const inp of inputs) {
    const opt = document.createElement("option");
    opt.value = inp.id;
    opt.textContent = inp.name || inp.id;
    el.midiInput.appendChild(opt);
  }
  // keep the saved selection if the device is still present
  el.midiInput.value = inputs.some((i) => i.id === current) ? current : "";
  if (midiCfg.inputId && el.midiInput.value !== midiCfg.inputId) {
    setMidiStatus("Saved MIDI device not connected");
  } else if (!midiInput) {
    setMidiStatus(inputs.length ? "MIDI off" : "No MIDI inputs found");
  }

  // Same input list drives the Program Change follow selector.
  const pcCurrent = pcCfg.inputId || "";
  el.pcInput.innerHTML = '<option value="">Off</option>';
  for (const inp of inputs) {
    const opt = document.createElement("option");
    opt.value = inp.id;
    opt.textContent = inp.name || inp.id;
    el.pcInput.appendChild(opt);
  }
  el.pcInput.value = inputs.some((i) => i.id === pcCurrent) ? pcCurrent : "";
  if (pcCfg.enabled && pcCfg.inputId && el.pcInput.value === pcCfg.inputId) {
    connectPcInput(pcCfg.inputId);
  }
}

function connectMidiInput(id) {
  if (midiInput) {
    midiInput.onmidimessage = null;
    midiInput = null;
  }
  if (!id || !midiAccess) {
    midiCfg.inputId = null;
    saveMidiCfg();
    setMidiStatus("MIDI off");
    return;
  }
  const inp = midiAccess.inputs.get(id);
  if (!inp) {
    setMidiStatus("MIDI device unavailable");
    return;
  }
  midiInput = inp;
  midiInput.onmidimessage = onMidiMessage;
  midiCfg.inputId = id;
  saveMidiCfg();
  setMidiStatus(`${inp.name || "MIDI"} · ${midiMapLabel()}`);
}

function connectPcInput(id) {
  if (pcInput) {
    pcInput.onmidimessage = null;
    pcInput = null;
  }
  if (!id || !midiAccess || !pcCfg.enabled) {
    if (!pcCfg.enabled) setPcStatus("Program Change follow off");
    return;
  }
  const inp = midiAccess.inputs.get(id);
  if (!inp) {
    setPcStatus("MIDI device unavailable");
    return;
  }
  pcInput = inp;
  pcInput.onmidimessage = onPcMessage;
  setPcStatus(`Following PC on ${inp.name || "MIDI"} · now ${patternKey()}`);
}

function onPcMessage(ev) {
  const [status, d1] = ev.data;
  if ((status & 0xf0) !== 0xc0) return; // program change only
  if (!pcCfg.enabled) return;
  const p = d1 & 0x7f;
  const bank = Math.floor(p / PATTERNS_PER_BANK) % PATTERN_BANKS;
  const num = p % PATTERNS_PER_BANK;
  setPattern(bank, num, { toast: false });
  setPcStatus(`PC ${p} → ${patternKey()}`);
}

function onMidiMessage(ev) {
  const [status, d1, d2] = ev.data;
  if ((status & 0xf0) !== 0xb0) return; // control change only
  const channel = status & 0x0f;

  if (midiLearning) {
    midiCfg.channel = channel;
    midiCfg.cc = d1;
    midiLearning = false;
    el.midiLearn.classList.remove("sc-btn-active");
    el.midiLearn.textContent = "Learn";
    saveMidiCfg();
    setMidiStatus(`Mapped ${midiMapLabel()}`);
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

// Absolute fader: the first message of a gesture only syncs the anchor (so an
// out-of-sync physical fader doesn't lurch), then subsequent moves morph.
function midiApplyAbsolute(pos) {
  pos = clamp(pos, 0, 1);
  if (!xfGrab) {
    crossfader.pos = pos;
    beginXfGrab();
    midiGestureActive = true;
    el.crossfader.value = String(Math.round(crossfader.pos * 1000));
    renderCrossfaderReadout();
  } else {
    crossfader.pos = pos;
    midiCommitPos();
  }
  midiResetIdle();
}

// Endless encoder: nudge the fader from its current position by the decoded step.
function midiApplyRelative(step) {
  if (!xfGrab) {
    beginXfGrab();
    midiGestureActive = true;
  }
  crossfader.pos = clamp(crossfader.pos + step, 0, 1);
  midiCommitPos();
  midiResetIdle();
}

el.midiInput.addEventListener("change", () => connectMidiInput(el.midiInput.value));

el.pcInput.addEventListener("change", () => {
  pcCfg.inputId = el.pcInput.value || null;
  savePcCfg();
  connectPcInput(pcCfg.inputId);
});

el.pcFollow.addEventListener("change", () => {
  pcCfg.enabled = el.pcFollow.checked;
  savePcCfg();
  if (pcCfg.enabled) {
    if (!navigator.requestMIDIAccess) {
      setPcStatus("Web MIDI not supported — select pattern manually");
    } else if (pcCfg.inputId) {
      connectPcInput(pcCfg.inputId);
    } else {
      setPcStatus("Select the device's MIDI input above");
    }
  } else {
    connectPcInput(null);
  }
});

el.midiMode.addEventListener("change", () => {
  midiCfg.mode = el.midiMode.value;
  saveMidiCfg();
});

el.midiStep.addEventListener("change", () => {
  const v = parseFloat(el.midiStep.value);
  if (Number.isFinite(v) && v > 0) {
    midiCfg.step = v;
    saveMidiCfg();
  }
});

el.midiLearn.addEventListener("click", () => {
  if (!midiInput) {
    setMidiStatus("Select a MIDI input first");
    return;
  }
  midiLearning = !midiLearning;
  el.midiLearn.classList.toggle("sc-btn-active", midiLearning);
  el.midiLearn.textContent = midiLearning ? "Learning…" : "Learn";
  setMidiStatus(midiLearning ? "Move the controller to map it…" : `Mapped ${midiMapLabel()}`);
});

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

restorePatternSel();
load();
renderAll();
initialParams();
connectWs();
initMidi();
