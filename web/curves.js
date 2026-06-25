import {
  CUSTOM_CURVES_KEY,
  PRESET_CURVES,
  loadCustomCurves,
  normalizeCurvePoints,
  presetCurvePoints,
  resamplePolyline,
  saveCustomCurves,
} from "./sweep-curves.mjs";

const el = {
  canvas: document.getElementById("cv-canvas"),
  name: document.getElementById("cv-name"),
  save: document.getElementById("cv-save"),
  clear: document.getElementById("cv-clear"),
  resetLinear: document.getElementById("cv-reset-linear"),
  status: document.getElementById("cv-status"),
  presets: document.getElementById("cv-presets"),
  saved: document.getElementById("cv-saved"),
};

const ctx = el.canvas.getContext("2d");
let points = presetCurvePoints("linear");
let stroke = [];
let drawing = false;
let activeSavedName = null;

function setStatus(msg) {
  el.status.textContent = msg || "";
}

function drawCurveOnContext(targetCtx, width, height, curvePoints, { grid = true, strokeStyle } = {}) {
  targetCtx.clearRect(0, 0, width, height);
  targetCtx.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--bg").trim() || "#121418";
  targetCtx.fillRect(0, 0, width, height);

  if (grid) {
    targetCtx.strokeStyle = getComputedStyle(document.documentElement).getPropertyValue("--border").trim() || "#2e333b";
    targetCtx.lineWidth = 1;
    targetCtx.beginPath();
    for (let i = 1; i < 4; i += 1) {
      const x = (width * i) / 4;
      const y = (height * i) / 4;
      targetCtx.moveTo(x, 0);
      targetCtx.lineTo(x, height);
      targetCtx.moveTo(0, y);
      targetCtx.lineTo(width, y);
    }
    targetCtx.stroke();
  }

  const pts = normalizeCurvePoints(curvePoints);
  if (!pts.length) return;

  targetCtx.strokeStyle =
    strokeStyle ||
    getComputedStyle(document.documentElement).getPropertyValue("--accent").trim() ||
    "#6b9fff";
  targetCtx.lineWidth = grid ? 2.5 : 1.5;
  targetCtx.lineJoin = "round";
  targetCtx.lineCap = "round";
  targetCtx.beginPath();
  for (let i = 0; i < pts.length; i += 1) {
    const px = pts[i].x * width;
    const py = height - pts[i].y * height;
    if (i === 0) targetCtx.moveTo(px, py);
    else targetCtx.lineTo(px, py);
  }
  targetCtx.stroke();

  if (grid) {
    targetCtx.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--muted").trim() || "#8a919c";
    targetCtx.font = "11px system-ui, sans-serif";
    targetCtx.fillText("0", 4, height - 4);
    targetCtx.fillText("1", width - 10, height - 4);
    targetCtx.fillText("1", 4, 12);
  }
}

function renderMainCanvas() {
  drawCurveOnContext(ctx, el.canvas.width, el.canvas.height, points);
}

function canvasToNormalized(clientX, clientY) {
  const rect = el.canvas.getBoundingClientRect();
  const x = (clientX - rect.left) / rect.width;
  const y = 1 - (clientY - rect.top) / rect.height;
  return { x: Math.max(0, Math.min(1, x)), y: Math.max(0, Math.min(1, y)) };
}

function beginStroke(clientX, clientY) {
  drawing = true;
  activeSavedName = null;
  stroke = [canvasToNormalized(clientX, clientY)];
  renderSavedList();
}

function extendStroke(clientX, clientY) {
  if (!drawing) return;
  const p = canvasToNormalized(clientX, clientY);
  const last = stroke[stroke.length - 1];
  if (!last || Math.hypot(p.x - last.x, p.y - last.y) > 0.004) {
    stroke.push(p);
  }
  points = resamplePolyline(stroke);
  renderMainCanvas();
}

function endStroke() {
  if (!drawing) return;
  drawing = false;
  if (stroke.length >= 2) {
    points = resamplePolyline(stroke);
  } else if (stroke.length === 1) {
    points = normalizeCurvePoints([
      { x: 0, y: stroke[0].y },
      { x: 1, y: stroke[0].y },
    ]);
  }
  renderMainCanvas();
}

function loadPoints(nextPoints, { name = null, status = "" } = {}) {
  points = normalizeCurvePoints(nextPoints);
  stroke = [...points];
  activeSavedName = name;
  if (name) el.name.value = name;
  renderMainCanvas();
  renderSavedList();
  setStatus(status);
}

function renderPresetButtons() {
  el.presets.replaceChildren();
  for (const [id, { name }] of Object.entries(PRESET_CURVES)) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "cv-preset-btn";
    btn.title = `Load ${name}`;

    const mini = document.createElement("canvas");
    mini.width = 96;
    mini.height = 40;
    drawCurveOnContext(mini.getContext("2d"), mini.width, mini.height, presetCurvePoints(id, 32), {
      grid: false,
    });

    const label = document.createElement("span");
    label.className = "cv-preset-label";
    label.textContent = name;

    btn.append(mini, label);
    btn.addEventListener("click", () => {
      loadPoints(presetCurvePoints(id), { status: `Loaded preset: ${name}` });
      el.name.value = "";
      activeSavedName = null;
      renderSavedList();
    });
    el.presets.append(btn);
  }
}

function renderSavedList() {
  const curves = loadCustomCurves();
  const names = Object.keys(curves).sort((a, b) => a.localeCompare(b));
  el.saved.replaceChildren();

  if (!names.length) {
    const empty = document.createElement("p");
    empty.className = "cv-empty";
    empty.textContent = "No saved curves yet.";
    el.saved.append(empty);
    return;
  }

  for (const name of names) {
    const li = document.createElement("li");
    li.className = "cv-saved-item";
    if (name === activeSavedName) li.classList.add("is-active");

    const preview = document.createElement("canvas");
    preview.className = "cv-saved-preview";
    preview.width = 72;
    preview.height = 32;
    drawCurveOnContext(preview.getContext("2d"), preview.width, preview.height, curves[name], {
      grid: false,
    });

    const label = document.createElement("span");
    label.className = "cv-saved-name";
    label.textContent = name;

    const actions = document.createElement("div");
    actions.className = "cv-saved-actions";

    const loadBtn = document.createElement("button");
    loadBtn.type = "button";
    loadBtn.className = "cv-btn";
    loadBtn.textContent = "Load";
    loadBtn.addEventListener("click", () => {
      loadPoints(curves[name], { name, status: `Loaded "${name}"` });
    });

    const delBtn = document.createElement("button");
    delBtn.type = "button";
    delBtn.className = "cv-btn";
    delBtn.textContent = "Delete";
    delBtn.addEventListener("click", () => {
      const next = loadCustomCurves();
      delete next[name];
      saveCustomCurves(next);
      if (activeSavedName === name) activeSavedName = null;
      renderSavedList();
      setStatus(`Deleted "${name}"`);
    });

    actions.append(loadBtn, delBtn);
    li.append(preview, label, actions);
    el.saved.append(li);
  }
}

function saveCurve() {
  const name = el.name.value.trim();
  if (!name) {
    setStatus("Enter a name for the curve.");
    return;
  }
  const curves = loadCustomCurves();
  curves[name] = normalizeCurvePoints(points);
  saveCustomCurves(curves);
  activeSavedName = name;
  renderSavedList();
  setStatus(`Saved "${name}" — available in Clock slide on the scenes page.`);
}

el.canvas.addEventListener("pointerdown", (e) => {
  el.canvas.setPointerCapture(e.pointerId);
  beginStroke(e.clientX, e.clientY);
});
el.canvas.addEventListener("pointermove", (e) => extendStroke(e.clientX, e.clientY));
el.canvas.addEventListener("pointerup", endStroke);
el.canvas.addEventListener("pointercancel", endStroke);
el.canvas.addEventListener("pointerleave", endStroke);

el.save.addEventListener("click", saveCurve);
el.clear.addEventListener("click", () => {
  stroke = [];
  points = [];
  activeSavedName = null;
  renderMainCanvas();
  setStatus("Canvas cleared.");
});
el.resetLinear.addEventListener("click", () => {
  loadPoints(presetCurvePoints("linear"), { status: "Reset to linear." });
  el.name.value = "";
  activeSavedName = null;
});

window.addEventListener("storage", (e) => {
  if (e.key === CUSTOM_CURVES_KEY) renderSavedList();
});

renderPresetButtons();
renderSavedList();
renderMainCanvas();
