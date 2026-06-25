// Sweep curve presets for clock-driven crossfader slides.

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

export function listCurveOptions() {
  return Object.entries(PRESET_CURVES).map(([id, { name }]) => ({ id, name }));
}

export function applySweepCurve(t, curveId) {
  const linearT = clamp(t);
  const id = curveId?.startsWith("custom:") ? DEFAULT_CURVE_ID : curveId || DEFAULT_CURVE_ID;
  const preset = PRESET_CURVES[id];
  if (!preset) return linearT;
  return clamp(preset.fn(linearT));
}
