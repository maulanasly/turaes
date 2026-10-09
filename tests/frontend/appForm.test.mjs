import { test } from "node:test";
import assert from "node:assert/strict";
import {
  KINDS, STEPS, FIELD_STEPS, emptyDraft, applyTemplate, kindInfo, stepError, stepErrors, buildPayload, reviewGroups,
  createConsequences, deployConsequences, restartConsequences, stopConsequences,
  startConsequences, rollbackConsequences, lifecycleSummary, mapIssuesToFields,
  validateCommandLines, parseCommandArgv, textPatch, argvPatch,
} from "../../static/js/lib/appForm.js";

test("kinds cover the three workload shapes", () => {
  assert.deepEqual(KINDS.map((k) => k.id), ["service", "static", "worker"]);
  assert.equal(kindInfo("bogus").id, "service");
  for (const k of KINDS) {
    assert.ok(k.label);
    assert.ok(k.blurb);
    assert.ok(k.constraints.length >= 2);
  }
  assert.match(kindInfo("service").constraints.join(" "), /turaes\.yaml/);
  assert.match(kindInfo("worker").constraints.join(" "), /Advanced launch/);
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
  assert.match(stepError(d, 1), /lowercase letters/);
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

test("placement validation points to invalid limits and runtime combinations", () => {
  const d = {
    ...emptyDraft(),
    mem_limit_mb: "8",
    cpu_quota_pct: "6401",
  };
  assert.match(stepErrors(d, 2).mem_limit_mb, /16 to 65536/);
  assert.match(stepErrors(d, 2).cpu_quota_pct, /1 to 6400/);

  d.mem_limit_mb = "256";
  d.cpu_quota_pct = "50";
  d.runtime = "proc";
  assert.match(stepErrors(d, 2).runtime, /require systemd/);

  d.runtime = "systemd";
  assert.deepEqual(stepErrors(d, 2), {});
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

test("explicit argv commands validate like the API", () => {
  assert.deepEqual(validateCommandLines(""), { lines: [], error: "Enter the executable path, one argument per line." });
  assert.match(validateCommandLines("/opt/my prog\n--x").error, /cannot contain spaces/);
  assert.deepEqual(validateCommandLines("/opt/venv/bin/python\nworker.py --queue default"), {
    lines: ["/opt/venv/bin/python", "worker.py --queue default"],
    error: null,
  });

  // Command mode requires argv and forbids flat args; binary mode is untouched.
  const cmd = { ...emptyDraft(), name: "jobs", launchMode: "command", command: "", port: "8000" };
  assert.match(stepErrors(cmd, 1).command, /executable path/);
  cmd.command = "/bin/sleep\n60";
  cmd.args = "--x";
  assert.match(stepErrors(cmd, 1).args, /cannot be combined/);
  cmd.args = "";
  assert.deepEqual(stepErrors(cmd, 1), {});

  const payload = buildPayload({ ...cmd, workdir: "/srv/jobs " });
  assert.deepEqual(payload.command, ["/bin/sleep", "60"]);
  assert.equal(payload.binary_path, undefined);
  assert.equal(payload.args, undefined);
  assert.equal(payload.workdir, "/srv/jobs");
  assert.equal(payload.port, 8000);
});

test("field issues map to steps with the earliest problem first", () => {
  assert.equal(FIELD_STEPS.port, 1);
  assert.equal(FIELD_STEPS.mem_limit_mb, 2);
  const mapped = mapIssuesToFields([
    { field: "domain", code: "conflict", detail: "taken" },
    { field: "port", code: "conflict", detail: "claimed" },
    { field: "mystery", code: "bad_request", detail: "?" },
  ]);
  assert.deepEqual(mapped.fieldErrors, { domain: "taken", port: "claimed", mystery: "?" });
  assert.equal(mapped.firstField, "domain");
  assert.equal(mapped.step, 1);
  assert.deepEqual(mapIssuesToFields([]), { fieldErrors: {}, firstField: null, step: 3 });
  assert.deepEqual(mapIssuesToFields(null), { fieldErrors: {}, firstField: null, step: 3 });
});

test("tri-state patches distinguish unchanged, cleared and set values", () => {
  assert.deepEqual(textPatch("/srv/a", "/srv/a"), { present: false });
  assert.deepEqual(textPatch(null, ""), { present: false });
  assert.deepEqual(textPatch("/srv/a", ""), { present: true, value: null });
  assert.deepEqual(textPatch(null, "/srv/b"), { present: true, value: "/srv/b" });
  assert.deepEqual(argvPatch(null, []), { present: false });
  assert.deepEqual(argvPatch(["a"], ["a"]), { present: false });
  assert.deepEqual(argvPatch(["a"], []), { present: true, value: null });
  assert.deepEqual(argvPatch(null, ["a"]), { present: true, value: ["a"] });
  assert.deepEqual(parseCommandArgv(null), null);
  assert.deepEqual(parseCommandArgv('["/bin/sleep","60"]'), ["/bin/sleep", "60"]);
  assert.equal(parseCommandArgv("nope"), null);
});

test("review reflects the launch mode actually submitted", () => {
  const d = { ...emptyDraft(), name: "jobs", launchMode: "command", command: "/bin/sleep\n60", workdir: "/srv/jobs" };
  const process = reviewGroups(d, "local")[2].rows;
  assert.deepEqual(process[0], ["Launch", "Explicit argv command"]);
  assert.deepEqual(process[1], ["Command", "/bin/sleep 60"]);
  assert.deepEqual(process.find(([l]) => l === "Working directory"), ["Working directory", "/srv/jobs"]);
  assert.ok(!process.some(([l]) => l === "Binary"));
});

test("lifecycle consequences are always explained", () => {
  assert.ok(createConsequences("service").length >= 2);
  assert.match(createConsequences("service").join(" "), /no public domain route/);
  assert.match(createConsequences("worker").join(" "), /no port or route/);
  assert.ok(deployConsequences({ kind: "service", domain: "a.example" }).join(" ").includes("health check"));
  assert.match(deployConsequences({ kind: "static" }).join(" "), /source directory/);
  assert.match(deployConsequences({ kind: "worker" }).join(" "), /no HTTP port or health check/);
  assert.ok(restartConsequences().join(" ").includes("interruption"));
  assert.ok(stopConsequences().join(" ").includes("route remains"));
  assert.ok(startConsequences().join(" ").includes("listening"));
  assert.ok(rollbackConsequences({ kind: "service" }).join(" ").includes("previous build"));
  assert.ok(rollbackConsequences({ kind: "service" }, "deadbeef").join(" ").includes("deadbeef"));
  assert.match(rollbackConsequences({ kind: "worker" }).join(" "), /no HTTP health check/);
  assert.match(lifecycleSummary({ kind: "worker" }), /no HTTP route/);
});

test("applyTemplate overlays known draft fields and validates kind", () => {
  const d = emptyDraft("s1");
  const applied = applyTemplate(d, { kind: "static", port: 8001, description: "Docs", bogus: 1 });
  assert.equal(d.kind, "static");
  assert.equal(d.port, 8001);
  assert.equal(d.description, "Docs");
  assert.equal(d.server_id, "s1");
  assert.ok(!("bogus" in d));
  assert.ok(applied.includes("kind") && applied.includes("port"));
});

test("applyTemplate ignores bad kinds and non-objects", () => {
  const d = emptyDraft();
  assert.deepEqual(applyTemplate(d, { kind: "bogus" }), []);
  assert.equal(d.kind, "service");
  assert.deepEqual(applyTemplate(d, null), []);
  assert.deepEqual(applyTemplate(d, "x"), []);
  const d2 = emptyDraft();
  applyTemplate(d2, { server_id: "elsewhere", port: "8000" });
  assert.equal(d2.server_id, "local");
  assert.equal(d2.port, "8000");
});
