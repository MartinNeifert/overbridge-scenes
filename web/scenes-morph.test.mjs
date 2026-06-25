import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  clamp,
  unionIndices,
  bilinearWeights,
  bilinearWeightsAssigned,
  isQuadCenter,
  quadSnapPosition,
  abSnapPosition,
  computeCrossfadeUpdates,
  computeMorphValue,
  computeQuadUpdates,
  morphParamValue,
  beginXfGrab,
  shouldApplyCrossfade,
} from "./scenes-morph.mjs";

const scenes = [
  { id: "1", name: "Scene 1", params: [{ index: 0, id: 1, name: "Cutoff", value: 0.0 }] },
  { id: "2", name: "Scene 2", params: [{ index: 0, id: 1, name: "Cutoff", value: 1.0 }] },
  { id: "3", name: "Scene 3", params: [{ index: 0, id: 1, name: "Cutoff", value: 0.0 }] },
  { id: "4", name: "Scene 4", params: [{ index: 0, id: 1, name: "Cutoff", value: 1.0 }] },
  { id: "5", name: "Scene 5", params: [{ index: 1, id: 2, name: "Reso", value: 0.5 }] },
];

function ctx(overrides = {}) {
  return {
    baselineExplicit: false,
    baseline: new Map([[0, 0.0]]),
    liveValues: new Map([[0, 0.0], [1, 0.25]]),
    paramRanges: new Map([
      [0, { min: 0, max: 1 }],
      [1, { min: 0, max: 1 }],
    ]),
    xfGrab: null,
    ...overrides,
  };
}

describe("clamp", () => {
  it("clamps to range", () => {
    assert.equal(clamp(1.5, 0, 1), 1);
    assert.equal(clamp(-0.1, 0, 1), 0);
  });
});

describe("unionIndices", () => {
  it("collects params from both assigned scenes", () => {
    const indices = unionIndices({ a: "1", b: "5", pos: 0 }, scenes);
    assert.deepEqual(indices.sort(), [0, 1]);
  });

  it("collects params from all quad corners", () => {
    const indices = unionIndices(
      { mode: "quad", corners: { tl: "1", tr: "5", bl: "3", br: "4" }, x: 0.5, y: 0.5 },
      scenes
    );
    assert.deepEqual(indices.sort(), [0, 1]);
  });
});

describe("bilinearWeights", () => {
  it("sums to 1 at corners and center", () => {
    const corners = [
      bilinearWeights(0, 0),
      bilinearWeights(1, 0),
      bilinearWeights(0, 1),
      bilinearWeights(1, 1),
      bilinearWeights(0.5, 0.5),
    ];
    for (const w of corners) {
      const sum = w.tl + w.tr + w.bl + w.br;
      assert.ok(Math.abs(sum - 1) < 1e-9);
    }
    assert.equal(bilinearWeights(0, 0).tl, 1);
    assert.equal(bilinearWeights(1, 1).br, 1);
  });
});

describe("crossfader morph", () => {
  it("lerps locked params in both scenes", () => {
    const updates = computeCrossfadeUpdates(
      { a: "1", b: "2", pos: 0.5 },
      scenes,
      ctx()
    );
    const cutoff = updates.find((u) => u.index === 0);
    assert.ok(cutoff);
    assert.ok(Math.abs(cutoff.value - 0.5) < 1e-6);
  });

  it("morphs scene lock toward baseline when other side is empty", () => {
    const updates = computeCrossfadeUpdates(
      { a: "1", b: null, pos: 1.0 },
      scenes,
      ctx({ liveValues: new Map([[0, 0.0]]) })
    );
    const cutoff = updates.find((u) => u.index === 0);
    assert.ok(cutoff);
    // Scene 1 locks 0.0; empty B uses live/baseline 0.0 → at t=1 → 0.0
    assert.ok(Math.abs(cutoff.value - 0.0) < 1e-6);
  });

  it("uses explicit baseline for empty side when captured", () => {
    const updates = computeCrossfadeUpdates(
      { a: "1", b: null, pos: 0.0 },
      scenes,
      ctx({
        baselineExplicit: true,
        baseline: new Map([[0, 0.4]]),
        liveValues: new Map([[0, 0.0]]),
      })
    );
    const cutoff = updates.find((u) => u.index === 0);
    assert.ok(cutoff);
    // t=0 on A side → scene lock 0.0
    assert.ok(Math.abs(cutoff.value - 0.0) < 1e-6);
  });

  it("returns no updates when both sides are none", () => {
    const updates = computeCrossfadeUpdates(
      { a: null, b: null, pos: 0.5 },
      scenes,
      ctx()
    );
    assert.equal(updates.length, 0);
  });

  it("jump mode follows ideal morph during grab", () => {
    const crossfader = { a: "1", b: "2", pos: 0.5 };
    const grab = beginXfGrab(crossfader, scenes, ctx({ liveValues: new Map([[0, 0.9]]) }));
    const value = morphParamValue(
      0,
      0.5,
      sceneById(scenes, "1"),
      sceneById(scenes, "2"),
      { ...ctx(), xfGrab: grab },
      "jump"
    );
    assert.ok(Math.abs(value - 0.5) < 1e-6);
  });

  it("pickup holds live value until morph sweeps through it", () => {
    const crossfader = { a: "1", b: "2", pos: 0.0 };
    const grab = beginXfGrab(crossfader, scenes, ctx({ liveValues: new Map([[0, 0.6]]) }));
    const valueAtStart = morphParamValue(
      0,
      0.0,
      sceneById(scenes, "1"),
      sceneById(scenes, "2"),
      { ...ctx(), xfGrab: grab },
      "pickup"
    );
    assert.ok(Math.abs(valueAtStart - 0.6) < 1e-6);
  });

  it("pickup does not engage on a small fader move when live is outside the sweep", () => {
    const crossfader = { a: "1", b: "2", pos: 0.0 };
    const grab = beginXfGrab(crossfader, scenes, ctx({ liveValues: new Map([[0, 0.6]]) }));
    const morph = { ...ctx(), xfGrab: grab };
    const sceneA = sceneById(scenes, "1");
    const sceneB = sceneById(scenes, "2");
    const held = morphParamValue(0, 0.1, sceneA, sceneB, morph, "pickup");
    assert.ok(Math.abs(held - 0.6) < 1e-6, "still holds live after 10% move");
    assert.equal(grab.per.get(0).engaged, false);
  });

  it("pickup engages when the morph sweep passes through the live value", () => {
    const crossfader = { a: "1", b: "2", pos: 0.0 };
    const grab = beginXfGrab(crossfader, scenes, ctx({ liveValues: new Map([[0, 0.5]]) }));
    const morph = { ...ctx(), xfGrab: grab };
    const sceneA = sceneById(scenes, "1");
    const sceneB = sceneById(scenes, "2");
    const held = morphParamValue(0, 0.04, sceneA, sceneB, morph, "pickup");
    assert.ok(Math.abs(held - 0.5) < 1e-6, "holds live value before sweep");
    const engaged = morphParamValue(0, 0.5, sceneA, sceneB, morph, "pickup");
    assert.ok(Math.abs(engaged - 0.5) < 1e-6, "tracks ideal once engaged");
  });

  it("scale starts at the live value and differs from jump when live is offset", () => {
    const crossfader = { a: "1", b: "2", pos: 0.0 };
    const grab = beginXfGrab(crossfader, scenes, ctx({ liveValues: new Map([[0, 0.6]]) }));
    const morph = { ...ctx(), xfGrab: grab };
    const sceneA = sceneById(scenes, "1");
    const sceneB = sceneById(scenes, "2");
    const scaleAtStart = morphParamValue(0, 0.0, sceneA, sceneB, morph, "scale");
    const jumpAtStart = morphParamValue(0, 0.0, sceneA, sceneB, morph, "jump");
    assert.ok(Math.abs(scaleAtStart - 0.6) < 1e-6);
    assert.ok(Math.abs(jumpAtStart - 0.0) < 1e-6);
    const scaleMid = morphParamValue(0, 0.5, sceneA, sceneB, morph, "scale");
    const jumpMid = morphParamValue(0, 0.5, sceneA, sceneB, morph, "jump");
    assert.ok(Math.abs(scaleMid - 0.8) < 1e-6);
    assert.ok(Math.abs(jumpMid - 0.5) < 1e-6);
  });

  it("scale interpolates smoothly when grabbed at the B end", () => {
    const crossfader = { a: "1", b: "2", pos: 1.0 };
    const grab = beginXfGrab(crossfader, scenes, ctx({ liveValues: new Map([[0, 0.9]]) }));
    const morph = { ...ctx(), xfGrab: grab };
    const sceneA = sceneById(scenes, "1");
    const sceneB = sceneById(scenes, "2");
    assert.ok(Math.abs(morphParamValue(0, 1.0, sceneA, sceneB, morph, "scale") - 0.9) < 1e-6);
    const mid = morphParamValue(0, 0.5, sceneA, sceneB, morph, "scale");
    assert.ok(Math.abs(mid - 0.45) < 1e-6, "moves toward A, not stuck at live");
    assert.ok(Math.abs(mid - 0.5) > 1e-3, "differs from jump at midpoint");
  });

  it("scale mode without grab falls back to jump", () => {
    const updates = computeCrossfadeUpdates(
      { a: "1", b: "2", pos: 0.0 },
      scenes,
      ctx({ liveValues: new Map([[0, 0.6]]) }),
      "scale"
    );
    const cutoff = updates.find((u) => u.index === 0);
    assert.ok(cutoff);
    assert.ok(Math.abs(cutoff.value - 0.0) < 1e-6);
  });

  it("beginXfGrab ignores a stale grab in ctx when freezing empty-side endpoints", () => {
    const staleGrab = {
      t0: 0.5,
      per: new Map([[0, { v0: 0.99, av: 0.2, bv: 0.8, engaged: false }]]),
    };
    const crossfader = { a: "1", b: null, pos: 0.5 };
    const grab = beginXfGrab(
      crossfader,
      scenes,
      ctx({
        liveValues: new Map([[0, 0.4]]),
        xfGrab: staleGrab,
      }),
      { ignoreStaleGrab: true }
    );
    const entry = grab.per.get(0);
    assert.ok(entry);
    assert.ok(Math.abs(entry.bv - 0.4) < 1e-6, "empty B side uses live, not stale grab");
  });
});

describe("computeMorphValue", () => {
  it("scale is continuous through the grab point", () => {
    const atGrab = computeMorphValue({
      mode: "scale",
      t: 0.5,
      t0: 0.5,
      v0: 0.6,
      av: 0.2,
      bv: 0.8,
      engaged: false,
    });
    assert.ok(Math.abs(atGrab.value - 0.6) < 1e-6);
  });
});

describe("shouldApplyCrossfade", () => {
  it("allows jump mode without a grab", () => {
    assert.equal(shouldApplyCrossfade(null, "jump"), true);
  });

  it("blocks pickup and scale without a grab", () => {
    assert.equal(shouldApplyCrossfade(null, "pickup"), false);
    assert.equal(shouldApplyCrossfade(null, "scale"), false);
  });

  it("allows pickup and scale while a grab is active", () => {
    assert.equal(shouldApplyCrossfade({ t0: 0 }, "pickup"), true);
    assert.equal(shouldApplyCrossfade({ t0: 0 }, "scale"), true);
  });

  it("force applies even without a grab", () => {
    assert.equal(shouldApplyCrossfade(null, "pickup", { force: true }), true);
  });
});

describe("quad crossfader morph", () => {
  const quad = {
    mode: "quad",
    corners: { tl: "1", tr: "2", bl: "3", br: "4" },
    x: 0.5,
    y: 0.5,
  };

  it("interpolates bilinearly at center", () => {
    const updates = computeQuadUpdates(quad, scenes, ctx());
    const cutoff = updates.find((u) => u.index === 0);
    assert.ok(cutoff);
    assert.ok(Math.abs(cutoff.value - 0.5) < 1e-6);
  });

  it("hits corner values at corners", () => {
    const tl = computeQuadUpdates({ ...quad, x: 0, y: 0 }, scenes, ctx());
    const br = computeQuadUpdates({ ...quad, x: 1, y: 1 }, scenes, ctx());
    assert.ok(Math.abs(tl.find((u) => u.index === 0).value - 0.0) < 1e-6);
    assert.ok(Math.abs(br.find((u) => u.index === 0).value - 1.0) < 1e-6);
  });

  it("dispatches through computeCrossfadeUpdates", () => {
    const updates = computeCrossfadeUpdates(quad, scenes, ctx());
    const cutoff = updates.find((u) => u.index === 0);
    assert.ok(cutoff);
    assert.ok(Math.abs(cutoff.value - 0.5) < 1e-6);
  });

  it("returns no updates when no corners are assigned", () => {
    const updates = computeQuadUpdates(
      { mode: "quad", corners: { tl: null, tr: null, bl: null, br: null }, x: 0.5, y: 0.5 },
      scenes,
      ctx()
    );
    assert.equal(updates.length, 0);
  });

  it("interpolation mode averages assigned scenes at center when corners are missing", () => {
    const updates = computeQuadUpdates(
      {
        mode: "quad",
        quadCenterMode: "interpolation",
        corners: { tl: "1", tr: null, bl: null, br: "4" },
        x: 0.5,
        y: 0.5,
      },
      scenes,
      ctx()
    );
    const cutoff = updates.find((u) => u.index === 0);
    assert.ok(cutoff);
    assert.ok(Math.abs(cutoff.value - 0.5) < 1e-6);
  });

  it("baseline mode uses pattern baseline at center", () => {
    const updates = computeQuadUpdates(
      {
        mode: "quad",
        quadCenterMode: "baseline",
        corners: { tl: "1", tr: "2", bl: "3", br: "4" },
        x: 0.5,
        y: 0.5,
      },
      scenes,
      ctx({
        baselineExplicit: true,
        baseline: new Map([[0, 0.25]]),
        liveValues: new Map([[0, 0.9]]),
      })
    );
    const cutoff = updates.find((u) => u.index === 0);
    assert.ok(cutoff);
    assert.ok(Math.abs(cutoff.value - 0.25) < 1e-6);
  });

  it("bilinearWeightsAssigned renormalizes when corners are missing", () => {
    const w = bilinearWeightsAssigned(0.5, 0.5, { tl: true, tr: false, bl: false, br: true });
    assert.ok(Math.abs(w.tl - 0.5) < 1e-9);
    assert.ok(Math.abs(w.br - 0.5) < 1e-9);
    assert.equal(w.tr, 0);
    assert.equal(w.bl, 0);
  });

  it("isQuadCenter detects grid middle", () => {
    assert.equal(isQuadCenter(0.5, 0.5), true);
    assert.equal(isQuadCenter(0.49, 0.5), false);
  });

  it("quadSnapPosition maps release targets", () => {
    assert.deepEqual(quadSnapPosition("center"), { x: 0.5, y: 0.5 });
    assert.deepEqual(quadSnapPosition("tl"), { x: 0, y: 0 });
    assert.equal(quadSnapPosition("none"), null);
  });

  it("abSnapPosition maps release targets", () => {
    assert.equal(abSnapPosition("center"), 0.5);
    assert.equal(abSnapPosition("a"), 0);
    assert.equal(abSnapPosition("b"), 1);
    assert.equal(abSnapPosition("none"), null);
  });
});

function sceneById(scenesList, id) {
  return scenesList.find((s) => s.id === id) ?? null;
}
