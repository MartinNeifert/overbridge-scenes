import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  applySweepCurve,
  normalizeCurvePoints,
  presetCurvePoints,
  resamplePolyline,
  sampleCurvePoints,
} from "./sweep-curves.mjs";

describe("applySweepCurve", () => {
  it("returns linear time for linear preset", () => {
    assert.equal(applySweepCurve(0.25, "linear"), 0.25);
    assert.equal(applySweepCurve(0.75, "linear"), 0.75);
  });

  it("applies ease-in below linear midpoint", () => {
    const out = applySweepCurve(0.5, "ease-in");
    assert.ok(out < 0.5);
    assert.ok(out > 0);
  });

  it("applies ease-out above linear midpoint", () => {
    const out = applySweepCurve(0.5, "ease-out");
    assert.ok(out > 0.5);
    assert.ok(out < 1);
  });

  it("clamps out-of-range time", () => {
    assert.equal(applySweepCurve(-1, "ease-in"), 0);
    assert.equal(applySweepCurve(2, "ease-out"), 1);
  });

  it("interpolates custom curves", () => {
    const custom = {
      Demo: [
        { x: 0, y: 0 },
        { x: 0.5, y: 0.25 },
        { x: 1, y: 1 },
      ],
    };
    const mid = applySweepCurve(0.5, "custom:Demo", custom);
    assert.ok(Math.abs(mid - 0.25) < 1e-6);
  });
});

describe("normalizeCurvePoints", () => {
  it("forces endpoints at 0 and 1", () => {
    const out = normalizeCurvePoints([{ x: 0.2, y: 0.3 }, { x: 0.8, y: 0.9 }]);
    assert.equal(out[0].x, 0);
    assert.equal(out[out.length - 1].x, 1);
  });
});

describe("resamplePolyline", () => {
  it("returns normalized endpoints", () => {
    const out = resamplePolyline(
      [
        { x: 0, y: 0 },
        { x: 0.5, y: 0.5 },
        { x: 1, y: 1 },
      ],
      8
    );
    assert.equal(out[0].x, 0);
    assert.equal(out[0].y, 0);
    assert.equal(out[out.length - 1].x, 1);
    assert.equal(out[out.length - 1].y, 1);
  });
});

describe("presetCurvePoints", () => {
  it("samples preset curves across 0..1", () => {
    const pts = presetCurvePoints("sine", 5);
    assert.equal(pts.length, 5);
    assert.equal(pts[0].x, 0);
    assert.equal(pts[pts.length - 1].x, 1);
  });
});

describe("sampleCurvePoints", () => {
  it("returns endpoint values", () => {
    const pts = [
      { x: 0, y: 0.1 },
      { x: 1, y: 0.9 },
    ];
    assert.equal(sampleCurvePoints(0, pts), 0.1);
    assert.equal(sampleCurvePoints(1, pts), 0.9);
  });
});
