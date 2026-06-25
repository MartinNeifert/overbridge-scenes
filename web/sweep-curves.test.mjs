import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { applySweepCurve, listCurveOptions } from "./sweep-curves.mjs";

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

  it("falls back to linear for removed custom curve ids", () => {
    assert.equal(applySweepCurve(0.4, "custom:Demo"), 0.4);
  });
});

describe("listCurveOptions", () => {
  it("lists preset curves only", () => {
    const options = listCurveOptions();
    assert.ok(options.length > 3);
    assert.ok(options.every((o) => !o.id.startsWith("custom:")));
    assert.ok(options.some((o) => o.id === "linear"));
  });
});
