import { test } from "node:test";
import assert from "node:assert/strict";
import {
  KINDS, STEPS, emptyDraft, kindInfo, stepError, buildPayload, reviewGroups,
  createConsequences, deployConsequences, restartConsequences, stopConsequences,
  startConsequences, rollbackConsequences,
} from "../../static/js/lib/appForm.js";

test("kinds cover the three workload shapes", () => {
  assert.deepEqual(KINDS.map((k) => k.id), ["service", "static", "worker"]);
  assert.equal(kindInfo("bogus").id, "service");
  for (const k of KINDS) {
    assert.ok(k.label);
    assert.ok(k.blurb);
    assert.ok(k.constraints.length >= 2);
  }
  assert.deepEqual(STEPS, ["Workload", "Process", "Placement", "Review"]);
});

test("emptyDraft defaults to a local service", () => {
  const d = emptyDraft();
  assert.equal(d.kind, "service");
  assert.equal(d.server_id, "local");
  assert.equal(d.runtime, "systemd");
  assert.equal(d.auto_restart, true);
  assert.equal(emptyDraft("s9").server_id, "s9");
});

test("stepError enforces name and per-kind required fields", () => {
  const d = emptyDraft();
  assert.equal(stepError(d, 0), null);
  assert.match(stepError(d, 1), /Name/);

  d.name = "Beruang";
  assert.match(stepError(d, 1), /lowercase slug/);
  d.name = "beruang";
  assert.match(stepError(d, 1), /binary path/);

  d.binary_path = "/srv/beruang/app";
  assert.match(stepError(d, 1), /port/);
  d.port = "8000";
  assert.equal(stepError(d, 1), null);

  // Static needs a directory, not a binary.
  const s = { ...emptyDraft(), kind: "static", name: "docs", port: "8080" };
  assert.match(stepError(s, 1), /source directory/);
  s.publish_dir = "/srv/docs/dist";
  assert.equal(stepError(s, 1), null);

  // Workers need no port.
  const w = { ...emptyDraft(), kind: "worker", name: "jobs", binary_path: "/srv/jobs/run" };
  assert.equal(stepError(w, 1), null);
});

test("buildPayload sends only kind-relevant fields", () => {
  const service = buildPayload({ ...emptyDraft(), name: "app", binary_path: "/bin/app", port: "8000", domain: "a.example", args: " --x ", health_path: "/healthz" });
  assert.deepEqual(service, {
    kind: "service", name: "app", binary_path: "/bin/app", args: "--x",
    port: 8000, health_path: "/healthz", domain: "a.example",
    server_id: "local", runtime: "systemd", auto_restart: true,
  });

  const worker = buildPayload({ ...emptyDraft(), kind: "worker", name: "jobs", binary_path: "/bin/jobs", port: "9999", domain: "x.example" });
  assert.equal(worker.port, undefined);
  assert.equal(worker.domain, undefined);
  assert.equal(worker.binary_path, "/bin/jobs");

  const staticSite = buildPayload({ ...emptyDraft(), kind: "static", name: "docs", publish_dir: "/srv/docs", port: "8080", binary_path: "/ignored" });
  assert.equal(staticSite.publish_dir, "/srv/docs");
  assert.equal(staticSite.binary_path, undefined);

  const limited = buildPayload({ ...emptyDraft(), name: "l", binary_path: "/bin/l", port: "1", mem_limit_mb: "256", cpu_quota_pct: "50" });
  assert.equal(limited.mem_limit_mb, 256);
  assert.equal(limited.cpu_quota_pct, 50);
});

test("reviewGroups summarises identity, process and placement", () => {
  const d = { ...emptyDraft(), name: "app", binary_path: "/bin/app", port: "8000", domain: "a.example", auto_restart: false };
  const groups = reviewGroups(d, "worker-1");
  const titles = groups.map((g) => g.title);
  assert.deepEqual(titles, ["Workload", "Identity", "Process", "Placement"]);
  const placement = groups[3].rows;
  assert.deepEqual(placement[0], ["Server", "worker-1"]);
  assert.deepEqual(placement.find(([l]) => l === "Restart when unhealthy"), ["Restart when unhealthy", "no"]);

  // Workers hide port/domain rows.
  const w = { ...emptyDraft(), kind: "worker", name: "jobs", binary_path: "/bin/jobs" };
  const wg = reviewGroups(w, "local");
  assert.ok(!wg[2].rows.some(([l]) => l === "Port"));
  assert.ok(!wg[3].rows.some(([l]) => l === "Domain"));
});

test("lifecycle consequences are always explained", () => {
  assert.ok(createConsequences("service").length >= 2);
  assert.match(createConsequences("worker").join(" "), /no port or route/);
  assert.ok(deployConsequences({ domain: "a.example" }).join(" ").includes("no downtime"));
  assert.ok(restartConsequences().join(" ").includes("interruption"));
  assert.ok(stopConsequences().join(" ").includes("immediately"));
  assert.ok(startConsequences().join(" ").includes("health check"));
  assert.ok(rollbackConsequences().join(" ").includes("previous build"));
  assert.ok(rollbackConsequences("deadbeef").join(" ").includes("deadbeef"));
});
