/**
 * Pure crossfader morph math (shared by scenes.js and Node unit tests).
 *
 * Values are VST3-normalized (usually 0..1). Morphing is linear interpolation
 * in that space unless pickup/scale takeover is active during a fader grab.
 */

export const EPS = 1e-4;

export function clamp(v, lo, hi) {
  return Math.min(hi, Math.max(lo, v));
}

export function sceneById(scenes, id) {
  if (id == null) return null;
  return scenes.find((s) => s.id === id) ?? null;
}

export function unionIndices(crossfader, scenes) {
  const a = sceneById(scenes, crossfader.a);
  const b = sceneById(scenes, crossfader.b);
  const set = new Set();
  if (a) for (const p of a.params) set.add(p.index);
  if (b) for (const p of b.params) set.add(p.index);
  return [...set];
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

export function paramRange(index, paramRanges) {
  const r = paramRanges?.get(index);
  let min = r?.min ?? 0;
  let max = r?.max ?? 1;
  if (max === min) max = min + 1;
  return [min, max];
}

/**
 * Compute all parameter writes for the current crossfader position.
 * Returns [] when both sides are unassigned.
 */
export function computeCrossfadeUpdates(crossfader, scenes, ctx, sliderMode = "jump") {
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
  return { t0: crossfader.pos, per };
}

/** Whether a crossfade write should run (pickup/scale defer until grab). */
export function shouldApplyCrossfade(xfGrab, sliderMode, opts = {}) {
  if (opts.force) return true;
  if (!xfGrab && sliderMode !== "jump") return false;
  return true;
}
