// The vendored figures keep the skill-validated shape: a meta export with a
// one-way range and a mount function. The adaptation from skill source to
// production module is mechanical (HL import + meta export); this test pins
// the contract the Hairline wrapper relies on.
import { test } from "node:test";
import assert from "node:assert/strict";
import HL from "../../static/js/vendor/hairline/kernel.js";
import { mount as mountBeacon, meta as metaBeacon } from "../../static/js/vendor/hairline/beacon.js";
import { mount as mountSlots, meta as metaSlots } from "../../static/js/vendor/hairline/slots.js";

test("kernel exposes the engine surface figures need", () => {
  for (const name of ["inject", "mk", "pointer", "register", "disposer", "solid", "put", "prism"]) {
    assert.equal(typeof HL[name], "function", `HL.${name}`);
  }
});

for (const [name, mount, meta] of [["beacon", mountBeacon, metaBeacon], ["slots", mountSlots, metaSlots]]) {
  test(`${name}: mount is a function and meta names the figure`, () => {
    assert.equal(typeof mount, "function");
    assert.equal(meta.name, name);
    assert.ok(typeof meta.means === "string" && meta.means.length > 0 && meta.means.length <= 140);
  });

  test(`${name}: range holds three numbers moving one way`, () => {
    assert.equal(meta.range.length, 3);
    const [lo, mid, hi] = meta.range;
    assert.ok([lo, mid, hi].every(Number.isFinite));
    assert.ok((mid - lo) * (hi - mid) >= 0 && lo !== hi);
  });
}
