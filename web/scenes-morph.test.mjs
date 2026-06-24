import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  clamp,
  unionIndices,
  computeCrossfadeUpdates,
  morphParamValue,
  beginXfGrab,
} from "./scenes-morph.mjs";

const scenes = [
  { id: "1", name: "Scene 1", params: [{ index: 0, id: 1, name: "Cutoff", value: 0.2 }] },
  { id: "2", name: "Scene 2", params: [{ index: 0, id: 1, name: "Cutoff", value: 0.8 }] },
  { id: "3", name: "Scene 3", params: [{ index: 1, id: 2, name: "Reso", value: 0.5 }] },
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
    const indices = unionIndices({ a: "1", b: "3", pos: 0 }, scenes);
    assert.deepEqual(indices.sort(), [0, 1]);
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
    // Scene 1 locks 0.2; empty B uses live/baseline 0.0 → at t=1 → 0.0
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
    // t=0 on A side → scene lock 0.2
    assert.ok(Math.abs(cutoff.value - 0.2) < 1e-6);
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
});

function sceneById(scenesList, id) {
  return scenesList.find((s) => s.id === id) ?? null;
}
