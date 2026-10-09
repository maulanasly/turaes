import { test } from "node:test";
import assert from "node:assert/strict";
import { parseRoute, pathFor, APP_TABS, RANGES, DEFAULT_RANGE, normalizeRange, rangeToHours } from "../../static/js/lib/route.js";

test("default and empty hash -> apps", () => {
  assert.deepEqual(parseRoute(""), { view: "apps" });
  assert.deepEqual(parseRoute("#/apps"), { view: "apps" });
  assert.deepEqual(parseRoute("#"), { view: "apps" });
});

test("app detail with tab", () => {
  assert.deepEqual(parseRoute("#/apps/abc/overview"), { view: "app", id: "abc", tab: "overview" });
  assert.deepEqual(parseRoute("#/apps/abc/logs"), { view: "app", id: "abc", tab: "logs" });
});

test("unknown tab passes through for the detail view to name", () => {
  assert.deepEqual(parseRoute("#/apps/abc/bogus"), { view: "app", id: "abc", tab: "bogus" });
});

test("unknown routes go to notfound instead of silently aliasing", () => {
  assert.deepEqual(parseRoute("#/nope"), { view: "notfound", path: "/nope" });
  assert.deepEqual(parseRoute("#/apps/abc/overview/extra"), { view: "app", id: "abc", tab: "overview" });
  assert.equal(pathFor({ view: "notfound", path: "/nope" }), "#/apps");
});

test("servers and server detail", () => {
  assert.deepEqual(parseRoute("#/servers"), { view: "servers" });
  assert.deepEqual(parseRoute("#/servers/s1"), { view: "server", id: "s1" });
});

test("catalog", () => {
  assert.deepEqual(parseRoute("#/catalog"), { view: "catalog" });
  assert.equal(pathFor({ view: "catalog" }), "#/catalog");
  assert.deepEqual(parseRoute(pathFor({ view: "catalog" })), { view: "catalog" });
});

test("about view", () => {
  assert.deepEqual(parseRoute("#/about"), { view: "about" });
  assert.equal(pathFor({ view: "about" }), "#/about");
  assert.deepEqual(parseRoute(pathFor({ view: "about" })), { view: "about" });
});

test("pathFor round-trips", () => {
  assert.equal(pathFor({ view: "apps" }), "#/apps");
  assert.equal(pathFor({ view: "app", id: "x", tab: "logs" }), "#/apps/x/logs");
  assert.equal(pathFor({ view: "server", id: "s" }), "#/servers/s");
  assert.equal(pathFor({ view: "tokens" }), "#/tokens");
  assert.equal(pathFor({ view: "org" }), "#/org");
  for (const tab of APP_TABS) {
    assert.deepEqual(parseRoute(pathFor({ view: "app", id: "id", tab })), { view: "app", id: "id", tab });
  }
});

test("range presets and hours mapping", () => {
  assert.deepEqual(RANGES, ["1h", "6h", "24h", "7d", "30d"]);
  assert.equal(DEFAULT_RANGE, "24h");
  assert.equal(rangeToHours("1h"), 1);
  assert.equal(rangeToHours("6h"), 6);
  assert.equal(rangeToHours("24h"), 24);
  assert.equal(rangeToHours("7d"), 168);
  assert.equal(rangeToHours("30d"), 720);
});

test("normalizeRange falls back to default", () => {
  assert.equal(normalizeRange("7d"), "7d");
  assert.equal(normalizeRange("bogus"), DEFAULT_RANGE);
  assert.equal(normalizeRange(undefined), DEFAULT_RANGE);
});

test("parseRoute carries the range on app overview", () => {
  assert.deepEqual(parseRoute("#/apps/abc/overview?range=7d"), { view: "app", id: "abc", tab: "overview", range: "7d" });
  assert.deepEqual(parseRoute("#/apps/abc?range=1h"), { view: "app", id: "abc", tab: "overview", range: "1h" });
});

test("parseRoute drops unknown ranges and explicit defaults", () => {
  assert.deepEqual(parseRoute("#/apps/abc/overview?range=bogus"), { view: "app", id: "abc", tab: "overview" });
  assert.deepEqual(parseRoute("#/apps/abc/overview?range=24h"), { view: "app", id: "abc", tab: "overview" });
  // Non-app views ignore the range param entirely.
  assert.deepEqual(parseRoute("#/servers?range=7d"), { view: "servers" });
});

test("pathFor serializes only non-default ranges", () => {
  assert.equal(pathFor({ view: "app", id: "x", tab: "overview", range: "7d" }), "#/apps/x/overview?range=7d");
  assert.equal(pathFor({ view: "app", id: "x", tab: "overview", range: "24h" }), "#/apps/x/overview");
  assert.equal(pathFor({ view: "app", id: "x", tab: "overview", range: "bogus" }), "#/apps/x/overview");
  // Round-trip: pathFor(parseRoute(h)) == h for non-default ranges.
  assert.equal(pathFor(parseRoute("#/apps/x/overview?range=30d")), "#/apps/x/overview?range=30d");
});
