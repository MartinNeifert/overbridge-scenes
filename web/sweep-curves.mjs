// Sweep curve presets + custom curve storage for clock-driven crossfader slides.

export const CUSTOM_CURVES_KEY = "ob-sweep-curves:custom";
export const DEFAULT_CURVE_ID = "linear";

export function clamp(t, lo = 0, hi = 1) {
  return Math.min(hi, Math.max(lo, t));
}

const PRESET_FNS = {
  linear: (t) => t,
  "ease-in": (t) => t * t,
  "ease-out": (t) => t * (2 - t),
  "ease-in-out": (t) => (t < 0.5 ? 2 * t * t : -1 + (4 - 2 * t) * t),
  "ease-in-cubic": (t) => t * t * t,
  "ease-out-cubic": (t) => 1 - (1 - t) ** 3,
  "ease-in-out-cubic": (t) =>
    t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2,
  "exp-in": (t) => (t <= 0 ? 0 : 2 ** (10 * (t - 1))),
  "exp-out": (t) => (t >= 1 ? 1 : 1 - 2 ** (-10 * t)),
  "log-in": (t) => Math.log10(1 + 9 * t),
  "log-out": (t) => 1 - Math.log10(1 + 9 * (1 - t)),
  sine: (t) => Math.sin((t * Math.PI) / 2),
  smoothstep: (t) => t * t * (3 - 2 * t),
  bounce: (t) => {
    const n1 = 7.5625;
    const d1 = 2.75;
    if (t < 1 / d1) return n1 * t * t;
    if (t < 2 / d1) return n1 * (t -= 1.5 / d1) * t + 0.75;
    if (t < 2.5 / d1) return n1 * (t -= 2.25 / d1) * t + 0.9375;
    return n1 * (t -= 2.625 / d1) * t + 0.984375;
  },
  elastic: (t) => {
    if (t <= 0) return 0;
    if (t >= 1) return 1;
    const p = 0.35;
    return 2 ** (-10 * t) * Math.sin(((t - p / 4) * (2 * Math.PI)) / p) + 1;
  },
  "hold-then-sweep": (t) => (t < 0.5 ? 0 : (t - 0.5) * 2),
  "sweep-then-hold": (t) => (t < 0.5 ? t * 2 : 1),
};

export const PRESET_CURVES = {
  linear: { name: "Linear", fn: PRESET_FNS.linear },
  "ease-in": { name: "Ease in", fn: (t) => clamp(PRESET_FNS["ease-in"](t)) },
  "ease-out": { name: "Ease out", fn: (t) => clamp(PRESET_FNS["ease-out"](t)) },
  "ease-in-out": { name: "Ease in-out", fn: (t) => clamp(PRESET_FNS["ease-in-out"](t)) },
  "ease-in-cubic": { name: "Ease in (cubic)", fn: (t) => clamp(PRESET_FNS["ease-in-cubic"](t)) },
  "ease-out-cubic": { name: "Ease out (cubic)", fn: (t) => clamp(PRESET_FNS["ease-out-cubic"](t)) },
  "ease-in-out-cubic": {
    name: "Ease in-out (cubic)",
    fn: (t) => clamp(PRESET_FNS["ease-in-out-cubic"](t)),
  },
  "exp-in": { name: "Exponential in", fn: (t) => clamp(PRESET_FNS["exp-in"](t)) },
  "exp-out": { name: "Exponential out", fn: (t) => clamp(PRESET_FNS["exp-out"](t)) },
  "log-in": { name: "Logarithmic in", fn: (t) => clamp(PRESET_FNS["log-in"](t)) },
  "log-out": { name: "Logarithmic out", fn: (t) => clamp(PRESET_FNS["log-out"](t)) },
  sine: { name: "Sine", fn: (t) => clamp(PRESET_FNS.sine(t)) },
  smoothstep: { name: "Smoothstep", fn: (t) => clamp(PRESET_FNS.smoothstep(t)) },
  bounce: { name: "Bounce", fn: (t) => clamp(PRESET_FNS.bounce(t)) },
  elastic: { name: "Elastic", fn: (t) => clamp(PRESET_FNS.elastic(t)) },
  "hold-then-sweep": {
    name: "Hold then sweep",
    fn: (t) => clamp(PRESET_FNS["hold-then-sweep"](t)),
  },
  "sweep-then-hold": {
    name: "Sweep then hold",
    fn: (t) => clamp(PRESET_FNS["sweep-then-hold"](t)),
  },
};

export function loadCustomCurves() {
  try {
    const raw = localStorage.getItem(CUSTOM_CURVES_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

export function saveCustomCurves(curves) {
  localStorage.setItem(CUSTOM_CURVES_KEY, JSON.stringify(curves));
}

export function listCurveOptions(customCurves = null) {
  const customs = customCurves ?? loadCustomCurves();
  const presets = Object.entries(PRESET_CURVES).map(([id, { name }]) => ({
    id,
    name,
    group: "preset",
  }));
  const customList = Object.keys(customs)
    .sort((a, b) => a.localeCompare(b))
    .map((name) => ({ id: `custom:${name}`, name, group: "custom" }));
  return [...presets, ...customList];
}

export function sampleCurvePoints(t, points) {
  const x = clamp(t);
  if (!points?.length) return x;
  const sorted = normalizeCurvePoints(points);
  if (sorted.length === 1) return clamp(sorted[0].y);

  if (x <= sorted[0].x) return clamp(sorted[0].y);
  const last = sorted[sorted.length - 1];
  if (x >= last.x) return clamp(last.y);

  for (let i = 0; i < sorted.length - 1; i += 1) {
    const a = sorted[i];
    const b = sorted[i + 1];
    if (x < a.x || x > b.x) continue;
    const span = b.x - a.x;
    if (span <= 0) return clamp(a.y);
    const u = (x - a.x) / span;
    return clamp(a.y + (b.y - a.y) * u);
  }
  return x;
}

export function normalizeCurvePoints(points) {
  if (!Array.isArray(points) || !points.length) return [];

  const cleaned = points
    .map((p) => ({ x: clamp(Number(p.x)), y: clamp(Number(p.y)) }))
    .filter((p) => Number.isFinite(p.x) && Number.isFinite(p.y))
    .sort((a, b) => a.x - b.x);

  if (!cleaned.length) return [];

  const monotonic = [];
  let maxX = -Infinity;
  for (const p of cleaned) {
    const x = Math.max(p.x, maxX);
    maxX = x;
    monotonic.push({ x, y: p.y });
  }

  if (monotonic[0].x > 0) monotonic.unshift({ x: 0, y: monotonic[0].y });
  else monotonic[0] = { ...monotonic[0], x: 0 };

  const tail = monotonic[monotonic.length - 1];
  if (tail.x < 1) monotonic.push({ x: 1, y: tail.y });
  else monotonic[monotonic.length - 1] = { ...tail, x: 1 };

  monotonic[0] = { x: 0, y: clamp(monotonic[0].y) };
  monotonic[monotonic.length - 1] = {
    x: 1,
    y: clamp(monotonic[monotonic.length - 1].y),
  };

  return monotonic;
}

export function applySweepCurve(t, curveId, customCurves = null) {
  const linearT = clamp(t);
  const id = curveId || DEFAULT_CURVE_ID;

  if (id.startsWith("custom:")) {
    const name = id.slice("custom:".length);
    const curves = customCurves ?? loadCustomCurves();
    const points = curves[name];
    if (!points) return linearT;
    return sampleCurvePoints(linearT, points);
  }

  const preset = PRESET_CURVES[id];
  if (!preset) return linearT;
  return clamp(preset.fn(linearT));
}

export function resamplePolyline(rawPoints, sampleCount = 64) {
  if (!rawPoints?.length) return [];
  if (rawPoints.length === 1) {
    return normalizeCurvePoints([
      { x: 0, y: rawPoints[0].y },
      { x: 1, y: rawPoints[0].y },
    ]);
  }

  const lengths = [0];
  let total = 0;
  for (let i = 1; i < rawPoints.length; i += 1) {
    const dx = rawPoints[i].x - rawPoints[i - 1].x;
    const dy = rawPoints[i].y - rawPoints[i - 1].y;
    total += Math.hypot(dx, dy);
    lengths.push(total);
  }
  if (total <= 0) {
    return normalizeCurvePoints([
      { x: 0, y: rawPoints[0].y },
      { x: 1, y: rawPoints[rawPoints.length - 1].y },
    ]);
  }

  const samples = [];
  for (let i = 0; i < sampleCount; i += 1) {
    const target = (total * i) / (sampleCount - 1);
    let seg = 0;
    while (seg < lengths.length - 2 && lengths[seg + 1] < target) seg += 1;
    const segStart = lengths[seg];
    const segEnd = lengths[seg + 1];
    const span = segEnd - segStart || 1;
    const u = (target - segStart) / span;
    const a = rawPoints[seg];
    const b = rawPoints[seg + 1];
    samples.push({
      x: a.x + (b.x - a.x) * u,
      y: a.y + (b.y - a.y) * u,
    });
  }

  return normalizeCurvePoints(samples);
}

export function presetCurvePoints(curveId, sampleCount = 64) {
  const fn = PRESET_CURVES[curveId]?.fn ?? PRESET_FNS.linear;
  const points = [];
  for (let i = 0; i < sampleCount; i += 1) {
    const x = i / (sampleCount - 1);
    points.push({ x, y: clamp(fn(x)) });
  }
  return points;
}
