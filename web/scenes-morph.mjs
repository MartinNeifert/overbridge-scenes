/**
 * Pure crossfader morph math (shared by scenes.js, remote.js, and Node unit tests).
 *
 * Values are VST3-normalized (usually 0..1). Morphing is linear interpolation
 * in that space unless pickup/scale takeover is active during a fader grab.
 *
 * Modes:
 *   ab   — 1D crossfade between scenes A and B (default)
 *   quad — 2D bilinear blend across four corner scenes (TL, TR, BL, BR)
 */

export const EPS = 1e-4;

export const DEFAULT_QUAD_CORNERS = { tl: "1", tr: "2", bl: "3", br: "4" };

/** What the grid center (0.5, 0.5) morphs toward. */
export const QUAD_CENTER_MODES = ["interpolation", "baseline"];
/** Where the pad handle moves after pointer release (`none` = stay put). */
export const QUAD_RELEASE_SNAPS = ["none", "center", "tl", "tr", "bl", "br"];

export const DEFAULT_QUAD_CENTER_MODE = "interpolation";
export const DEFAULT_QUAD_RELEASE_SNAP = "center";
export const DEFAULT_QUAD_RELEASE_SNAP_MS = 400;

export function clamp(v, lo, hi) {
  return Math.min(hi, Math.max(lo, v));
}

export function sceneById(scenes, id) {
  if (id == null) return null;
  return scenes.find((s) => s.id === id) ?? null;
}

export function crossfaderMode(crossfader) {
  return crossfader?.mode === "quad" ? "quad" : "ab";
}

export function normalizeQuadCenterMode(mode) {
  return mode === "baseline" ? "baseline" : DEFAULT_QUAD_CENTER_MODE;
}

export function normalizeQuadReleaseSnap(snap) {
  return QUAD_RELEASE_SNAPS.includes(snap) ? snap : DEFAULT_QUAD_RELEASE_SNAP;
}

export function normalizeCrossfader(crossfader = {}) {
  const mode = crossfaderMode(crossfader);
  const corners = { ...DEFAULT_QUAD_CORNERS, ...(crossfader.corners || {}) };
  const snapMs = Number(crossfader.quadReleaseSnapMs);
  return {
    mode,
    a: crossfader.a ?? null,
    b: crossfader.b ?? null,
    pos: Number.isFinite(crossfader.pos) ? crossfader.pos : 0,
    corners,
    x: Number.isFinite(crossfader.x) ? crossfader.x : 0.5,
    y: Number.isFinite(crossfader.y) ? crossfader.y : 0.5,
    quadCenterMode: normalizeQuadCenterMode(crossfader.quadCenterMode),
    quadReleaseSnap: normalizeQuadReleaseSnap(crossfader.quadReleaseSnap),
    quadReleaseSnapMs: Number.isFinite(snapMs) && snapMs >= 0 ? snapMs : DEFAULT_QUAD_RELEASE_SNAP_MS,
  };
}

export function unionIndices(crossfader, scenes) {
  const set = new Set();
  if (crossfaderMode(crossfader) === "quad") {
    const corners = { ...DEFAULT_QUAD_CORNERS, ...(crossfader.corners || {}) };
    for (const id of Object.values(corners)) {
      const scene = sceneById(scenes, id);
      if (scene) for (const p of scene.params) set.add(p.index);
    }
    return [...set];
  }
  const a = sceneById(scenes, crossfader.a);
  const b = sceneById(scenes, crossfader.b);
  if (a) for (const p of a.params) set.add(p.index);
  if (b) for (const p of b.params) set.add(p.index);
  return [...set];
}

export function bilinearWeights(x, y) {
  const x1 = clamp(x, 0, 1);
  const y1 = clamp(y, 0, 1);
  return {
    tl: (1 - x1) * (1 - y1),
    tr: x1 * (1 - y1),
    bl: (1 - x1) * y1,
    br: x1 * y1,
  };
}

export function isQuadCenter(x, y) {
  return Math.abs(x - 0.5) < EPS && Math.abs(y - 0.5) < EPS;
}

/** Bilinear weights among assigned corners only (renormalized to sum 1). */
export function bilinearWeightsAssigned(x, y, cornerAssigned) {
  const raw = bilinearWeights(x, y);
  let sum = 0;
  const w = { tl: 0, tr: 0, bl: 0, br: 0 };
  for (const corner of ["tl", "tr", "bl", "br"]) {
    if (cornerAssigned[corner]) {
      w[corner] = raw[corner];
      sum += raw[corner];
    }
  }
  if (sum <= EPS) return w;
  for (const corner of ["tl", "tr", "bl", "br"]) {
    w[corner] /= sum;
  }
  return w;
}

/** Pad position for a release-snap target. */
export function quadSnapPosition(target) {
  switch (target) {
    case "tl":
      return { x: 0, y: 0 };
    case "tr":
      return { x: 1, y: 0 };
    case "bl":
      return { x: 0, y: 1 };
    case "br":
      return { x: 1, y: 1 };
    case "center":
      return { x: 0.5, y: 0.5 };
    default:
      return null;
  }
}

/**
 * @param {object} ctx
 * @param {boolean} ctx.baselineExplicit
 * @param {Map<number, number>} ctx.baseline
 * @param {Map<number, number>} ctx.liveValues
 */
export function baseValue(index, ctx) {
  if (ctx.baselineExplicit && ctx.baseline.has(index)) {
    return ctx.baseline.get(index);
  }
  if (ctx.liveValues.has(index)) return ctx.liveValues.get(index);
  if (ctx.baseline.has(index)) return ctx.baseline.get(index);
  return 0;
}

export function emptySideValue(index, ctx) {
  if (ctx.baselineExplicit && ctx.baseline.has(index)) {
    return ctx.baseline.get(index);
  }
  if (ctx.xfGrab?.per?.has(index)) return ctx.xfGrab.per.get(index).v0;
  if (ctx.liveValues.has(index)) return ctx.liveValues.get(index);
  if (ctx.baseline.has(index)) return ctx.baseline.get(index);
  return 0;
}

export function endpointValue(scene, index, ctx) {
  if (scene) {
    const p = scene.params.find((x) => x.index === index);
    if (p) return p.value;
  }
  return emptySideValue(index, ctx);
}

/**
 * Morph one parameter at crossfader position `t` (jump / pickup / scale).
 * During a grab, A/B endpoints are frozen at grab time — see `beginXfGrab`.
 */
export function computeMorphValue({ mode, t, t0, v0, av, bv, engaged }) {
  const ideal = av + (bv - av) * t;
  if (mode === "jump") return { value: ideal, engaged };

  if (mode === "pickup") {
    let nextEngaged = engaged;
    if (!nextEngaged) {
      const ideal0 = av + (bv - av) * t0;
      const lo = Math.min(ideal0, ideal);
      const hi = Math.max(ideal0, ideal);
      if (v0 >= lo - EPS && v0 <= hi + EPS) nextEngaged = true;
      else if (Math.abs(t - t0) > 0.05) nextEngaged = true;
    }
    return { value: nextEngaged ? ideal : v0, engaged: nextEngaged };
  }

  // Scale: piecewise linear through (0, av) → (t0, v0) → (1, bv).
  let value;
  if (t0 <= EPS) {
    value = v0 + (bv - v0) * t;
  } else if (t0 >= 1 - EPS) {
    value = t <= t0 ? av + (v0 - av) * (t / t0) : v0;
  } else if (t <= t0) {
    value = av + (v0 - av) * (t / t0);
  } else {
    value = v0 + (bv - v0) * ((t - t0) / (1 - t0));
  }
  return { value, engaged };
}

/**
 * Morph one parameter at crossfader position `t`.
 *
 * @param {string} sliderMode - jump | pickup | scale
 */
export function morphParamValue(
  index,
  t,
  sceneA,
  sceneB,
  ctx,
  sliderMode = "jump"
) {
  const av = endpointValue(sceneA, index, ctx);
  const bv = endpointValue(sceneB, index, ctx);

  const mode = ctx.xfGrab && sliderMode !== "jump" ? sliderMode : "jump";
  const t0 = ctx.xfGrab ? ctx.xfGrab.t0 : 0;
  const g = mode === "jump" ? null : ctx.xfGrab?.per?.get(index);

  let value;
  if (g) {
    const result = computeMorphValue({
      mode,
      t,
      t0,
      v0: g.v0,
      av: g.av,
      bv: g.bv,
      engaged: g.engaged,
    });
    g.engaged = result.engaged;
    value = result.value;
  } else {
    value = av + (bv - av) * t;
  }

  const [min, max] = paramRange(index, ctx.paramRanges);
  return clamp(value, Math.min(min, max), Math.max(min, max));
}

export function morphQuadParamValue(index, x, y, cornerScenes, ctx, centerMode = DEFAULT_QUAD_CENTER_MODE) {
  const mode = normalizeQuadCenterMode(centerMode);
  const [min, max] = paramRange(index, ctx.paramRanges);
  const clamped = (v) => clamp(v, Math.min(min, max), Math.max(min, max));

  if (mode === "baseline" && isQuadCenter(x, y)) {
    return clamped(baseValue(index, ctx));
  }

  const cornerAssigned = {
    tl: !!cornerScenes.tl,
    tr: !!cornerScenes.tr,
    bl: !!cornerScenes.bl,
    br: !!cornerScenes.br,
  };

  const w =
    mode === "interpolation"
      ? bilinearWeightsAssigned(x, y, cornerAssigned)
      : bilinearWeights(x, y);

  const weightSum = w.tl + w.tr + w.bl + w.br;
  if (weightSum <= EPS) return clamped(baseValue(index, ctx));

  let value = 0;
  value += w.tl * endpointValue(cornerScenes.tl, index, ctx);
  value += w.tr * endpointValue(cornerScenes.tr, index, ctx);
  value += w.bl * endpointValue(cornerScenes.bl, index, ctx);
  value += w.br * endpointValue(cornerScenes.br, index, ctx);
  return clamped(value);
}

export function paramRange(index, paramRanges) {
  const r = paramRanges?.get(index);
  let min = r?.min ?? 0;
  let max = r?.max ?? 1;
  if (max === min) max = min + 1;
  return [min, max];
}

function hasAssignedAbSides(crossfader, scenes) {
  return !!(sceneById(scenes, crossfader.a) || sceneById(scenes, crossfader.b));
}

function hasAssignedQuadCorners(crossfader, scenes) {
  const corners = { ...DEFAULT_QUAD_CORNERS, ...(crossfader.corners || {}) };
  return Object.values(corners).some((id) => sceneById(scenes, id));
}

/**
 * Compute all parameter writes for the current crossfader position.
 * Dispatches to 1D A/B or 2D quad bilinear blend based on `crossfader.mode`.
 */
export function computeCrossfadeUpdates(crossfader, scenes, ctx, sliderMode = "jump") {
  if (crossfaderMode(crossfader) === "quad") {
    return computeQuadUpdates(crossfader, scenes, ctx);
  }
  const sceneA = sceneById(scenes, crossfader.a);
  const sceneB = sceneById(scenes, crossfader.b);
  if (!sceneA && !sceneB) return [];

  const t = crossfader.pos;
  const updates = [];
  for (const index of unionIndices(crossfader, scenes)) {
    updates.push({
      index,
      value: morphParamValue(index, t, sceneA, sceneB, ctx, sliderMode),
    });
  }
  return updates;
}

export function computeQuadUpdates(crossfader, scenes, ctx) {
  if (!hasAssignedQuadCorners(crossfader, scenes)) return [];
  const corners = { ...DEFAULT_QUAD_CORNERS, ...(crossfader.corners || {}) };
  const cornerScenes = {
    tl: sceneById(scenes, corners.tl),
    tr: sceneById(scenes, corners.tr),
    bl: sceneById(scenes, corners.bl),
    br: sceneById(scenes, corners.br),
  };
  const x = crossfader.x ?? 0.5;
  const y = crossfader.y ?? 0.5;
  const centerMode = normalizeQuadCenterMode(crossfader.quadCenterMode);
  const updates = [];
  for (const index of unionIndices(crossfader, scenes)) {
    updates.push({
      index,
      value: morphQuadParamValue(index, x, y, cornerScenes, ctx, centerMode),
    });
  }
  return updates;
}

export function crossfaderHasAssignments(crossfader, scenes) {
  if (crossfaderMode(crossfader) === "quad") {
    return hasAssignedQuadCorners(crossfader, scenes);
  }
  return hasAssignedAbSides(crossfader, scenes);
}

/**
 * Build grab state for pickup/scale (mutates per-map entries during morph).
 * Freezes each param's live value and A/B endpoints at grab time so the morph
 * trajectory stays fixed while pickup/scale reconcile against knob position.
 *
 * Pass `{ ignoreStaleGrab: true }` when replacing an existing grab so empty-side
 * endpoints are frozen from live values, not the previous grab's v0.
 */
export function beginXfGrab(crossfader, scenes, ctx, opts = {}) {
  const grabCtx =
    opts.ignoreStaleGrab && ctx.xfGrab ? { ...ctx, xfGrab: null } : ctx;

  if (crossfaderMode(crossfader) === "quad") {
    const corners = { ...DEFAULT_QUAD_CORNERS, ...(crossfader.corners || {}) };
    const cornerScenes = {
      tl: sceneById(scenes, corners.tl),
      tr: sceneById(scenes, corners.tr),
      bl: sceneById(scenes, corners.bl),
      br: sceneById(scenes, corners.br),
    };
    const per = new Map();
    for (const index of unionIndices(crossfader, scenes)) {
      const lv = grabCtx.liveValues.has(index)
        ? grabCtx.liveValues.get(index)
        : undefined;
      per.set(index, {
        v0: lv !== undefined ? lv : baseValue(index, grabCtx),
        tl: endpointValue(cornerScenes.tl, index, grabCtx),
        tr: endpointValue(cornerScenes.tr, index, grabCtx),
        bl: endpointValue(cornerScenes.bl, index, grabCtx),
        br: endpointValue(cornerScenes.br, index, grabCtx),
        engaged: false,
      });
    }
    return { mode: "quad", x0: crossfader.x ?? 0.5, y0: crossfader.y ?? 0.5, per };
  }

  const sceneA = sceneById(scenes, crossfader.a);
  const sceneB = sceneById(scenes, crossfader.b);
  const per = new Map();
  for (const index of unionIndices(crossfader, scenes)) {
    const lv = grabCtx.liveValues.has(index)
      ? grabCtx.liveValues.get(index)
      : undefined;
    per.set(index, {
      v0: lv !== undefined ? lv : baseValue(index, grabCtx),
      av: endpointValue(sceneA, index, grabCtx),
      bv: endpointValue(sceneB, index, grabCtx),
      engaged: false,
    });
  }
  return { mode: "ab", t0: crossfader.pos, per };
}

/** Whether a crossfade write should run (pickup/scale defer until grab). */
export function shouldApplyCrossfade(xfGrab, sliderMode, opts = {}) {
  if (opts.force) return true;
  if (!xfGrab && sliderMode !== "jump") return false;
  return true;
}
