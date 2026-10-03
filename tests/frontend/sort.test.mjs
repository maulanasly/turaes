import { test } from "node:test";
import assert from "node:assert/strict";
import { sortApps } from "../../static/js/lib/sort.js";

const apps = [
  { name: "zeta", status: "running", server_id: "s2", port: 9000, updated_at: "2026-01-03" },
  { name: "alpha", status: "stopped", server_id: "s1", port: 8000, updated_at: "2026-01-01" },
  { name: "mid", status: "failed", server_id: "s2", port: 8500, updated_at: "2026-01-02" },
];
const names = (id) => ({ s1: "worker-1", s2: "worker-2" }[id]);

test("sort by name", () => {
  assert.deepEqual(sortApps(apps, "name").map((a) => a.name), ["alpha", "mid", "zeta"]);
});

test("sort by port is numeric", () => {
  assert.deepEqual(sortApps(apps, "port").map((a) => a.port), [8000, 8500, 9000]);
});

test("sort by server name", () => {
  assert.deepEqual(sortApps(apps, "server", names).map((a) => a.server_id), ["s1", "s2", "s2"]);
});

test("recent is newest first", () => {
  assert.deepEqual(sortApps(apps, "recent").map((a) => a.name), ["zeta", "mid", "alpha"]);
});

test("does not mutate input", () => {
  const copy = [...apps];
  sortApps(apps, "name");
  assert.deepEqual(apps, copy);
});
