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
  const ideal = av + (bv - av) * t;
  let value = ideal;

  const mode = ctx.xfGrab && sliderMode !== "jump" ? sliderMode : "jump";
  const t0 = ctx.xfGrab ? ctx.xfGrab.t0 : 0;
  const g = mode === "jump" ? null : ctx.xfGrab?.per?.get(index);

  if (g) {
    const v0 = g.v0;
    if (mode === "pickup") {
      if (!g.engaged) {
        const ideal0 = av + (bv - av) * t0;
        const lo = Math.min(ideal0, ideal);
        const hi = Math.max(ideal0, ideal);
        if (v0 >= lo - EPS && v0 <= hi + EPS) g.engaged = true;
        else if (Math.abs(t - t0) > 0.05) g.engaged = true;
      }
      value = g.engaged ? ideal : v0;
    } else if (mode === "scale") {
      if (t >= t0) {
        value = t0 < 1 ? v0 + (bv - v0) * ((t - t0) / (1 - t0)) : v0;
      } else {
        value = t0 > 0 ? av + (v0 - av) * (t / t0) : v0;
      }
    }
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

/** Build grab state for pickup/scale (mutates per-map entries during morph). */
export function beginXfGrab(crossfader, scenes, ctx) {
  const per = new Map();
  for (const index of unionIndices(crossfader, scenes)) {
    const lv = ctx.liveValues.has(index) ? ctx.liveValues.get(index) : undefined;
    per.set(index, {
      v0: lv !== undefined ? lv : baseValue(index, ctx),
      engaged: false,
    });
  }
  return { t0: crossfader.pos, per };
}
