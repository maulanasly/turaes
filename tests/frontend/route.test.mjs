import { test } from "node:test";
import assert from "node:assert/strict";
import { parseRoute, pathFor, APP_TABS } from "../../static/js/lib/route.js";

test("default and empty hash -> apps", () => {
  assert.deepEqual(parseRoute(""), { view: "apps" });
  assert.deepEqual(parseRoute("#/apps"), { view: "apps" });
  assert.deepEqual(parseRoute("#"), { view: "apps" });
});

test("app detail with tab", () => {
  assert.deepEqual(parseRoute("#/apps/abc/overview"), { view: "app", id: "abc", tab: "overview" });
  assert.deepEqual(parseRoute("#/apps/abc/logs"), { view: "app", id: "abc", tab: "logs" });
});

test("unknown tab falls back to overview", () => {
  assert.deepEqual(parseRoute("#/apps/abc/bogus"), { view: "app", id: "abc", tab: "overview" });
});

test("servers and server detail", () => {
  assert.deepEqual(parseRoute("#/servers"), { view: "servers" });
  assert.deepEqual(parseRoute("#/servers/s1"), { view: "server", id: "s1" });
});

test("pathFor round-trips", () => {
  assert.equal(pathFor({ view: "apps" }), "#/apps");
  assert.equal(pathFor({ view: "app", id: "x", tab: "logs" }), "#/apps/x/logs");
  assert.equal(pathFor({ view: "server", id: "s" }), "#/servers/s");
  for (const tab of APP_TABS) {
    assert.deepEqual(parseRoute(pathFor({ view: "app", id: "id", tab })), { view: "app", id: "id", tab });
  }
});
