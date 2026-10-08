// Unit tests for the hairline figure state mappings (pure, no DOM).
import { test } from "node:test";
import assert from "node:assert/strict";
import { beaconValue, slotsValue, slotsInput } from "../../static/js/lib/figureState.js";

test("beaconValue maps every known status, unknown falls back mid-low", () => {
  assert.equal(beaconValue("running"), 0.15);
  assert.equal(beaconValue("stopped"), 0.2);
  assert.equal(beaconValue("unknown"), 0.3);
  assert.equal(beaconValue("deploying"), 0.55);
  assert.equal(beaconValue("unhealthy"), 0.7);
  assert.equal(beaconValue("failed"), 0.9);
  assert.equal(beaconValue("bogus"), 0.3);
  assert.equal(beaconValue(undefined), 0.3);
});

test("beaconValue orders severity monotonically", () => {
  const order = ["running", "stopped", "unknown", "deploying", "unhealthy", "failed"];
  const values = order.map(beaconValue);
  const sorted = [...values].sort((a, b) => a - b);
  assert.deepEqual(values, sorted);
});

test("slotsValue encodes the live slot in the whole part, phase in the fraction", () => {
  assert.equal(slotsValue("A", "empty"), 0.05);
  assert.equal(slotsValue("A", "idle"), 0.3);
  assert.equal(slotsValue("A", "progress"), 0.8);
  assert.equal(slotsValue("B", "idle"), 1.3);
  assert.equal(slotsValue("B", "progress"), 1.8);
  assert.equal(slotsValue("B", "bogus"), 1.3);
});

test("slotsInput derives the live slot from active_port (only two slots exist)", () => {
  const app = { port: 8000, active_port: null };
  assert.equal(slotsInput(app, []).active, "A");
  assert.equal(slotsInput({ ...app, active_port: 8000 }, []).active, "A");
  assert.equal(slotsInput({ ...app, active_port: 18000 }, []).active, "B");
  assert.equal(slotsInput(null, []).active, "A");
});

test("slotsInput derives empty/progress/idle phases", () => {
  const app = { port: 8000, active_port: null };
  assert.equal(slotsInput(app, []).phase, "empty");
  assert.equal(slotsInput(app, null).phase, "empty");
  assert.equal(slotsInput(app, [{ status: "queued" }]).phase, "progress");
  assert.equal(slotsInput(app, [{ status: "installing" }]).phase, "progress");
  assert.equal(slotsInput(app, [{ status: "running" }]).phase, "idle");
  assert.equal(slotsInput(app, [{ status: "failed" }]).phase, "idle");
});

test("slotsInput reports a retained slot for rollback", () => {
  const app = { port: 8000, active_port: 18000 };
  assert.equal(slotsInput(app, [{ status: "running" }]).retained, false);
  assert.equal(slotsInput(app, [{ status: "running" }, { status: "running" }]).retained, true);
  assert.equal(
    slotsInput(app, [{ status: "running", previous_artifact: "abc" }]).retained,
    true,
  );
});
