// End-to-end workflow simulation with pure helpers plus a stubbed HTTP
// layer: draft -> client gates -> payload -> preflight report -> field
// mapping, mirroring exactly what the wizard Review step and submit do.
import { test } from "node:test";
import assert from "node:assert/strict";
import { oapi, setOrg } from "../../static/js/lib/api.js";
import {
  emptyDraft, stepErrors, buildPayload, reviewGroups, mapIssuesToFields,
  FIELD_STEPS, hasPendingDeployment,
} from "../../static/js/lib/appForm.js";

test("create wizard: a clean draft passes every gate to review", () => {
  const draft = {
    ...emptyDraft(),
    name: "web",
    binary_path: "/srv/web/app",
    port: "8000",
    domain: "web.test",
  };
  for (const step of [0, 1, 2]) {
    assert.deepEqual(stepErrors(draft, step), {}, `step ${step} should pass`);
  }
  const payload = buildPayload(draft);
  assert.equal(payload.port, 8000);
  assert.equal(payload.domain, "web.test");
  const titles = reviewGroups(draft, "local").map((g) => g.title);
  assert.deepEqual(titles, ["Workload", "Identity", "Process", "Placement"]);
  const process = reviewGroups(draft, "local")[2].rows;
  assert.deepEqual(process[0], ["Launch", "Prebuilt binary"]);
});

test("create wizard: preflight problems land on the earliest step", async () => {
  const previousFetch = globalThis.fetch;
  const seen = [];
  globalThis.fetch = async (url, options) => {
    seen.push([url, JSON.parse(options.body)]);
    return new Response(JSON.stringify({
      ok: false,
      errors: [
        { field: "port", code: "conflict", detail: "port 8000 (or its blue/green pair) is already claimed by 'web'" },
        { field: "domain", code: "conflict", detail: "domain 'a.test' is already used by 'web'" },
      ],
      warnings: [
        { field: null, detail: "Source '/srv/app' was not found on this host; it must exist before deploy." },
      ],
      checks: { port: "fail", domain: "fail", source: "unknown" },
    }), { status: 200, headers: { "content-type": "application/json" } });
  };
  try {
    setOrg("default");
    const draft = {
      ...emptyDraft(), name: "web2", binary_path: "/srv/app", port: "8000", domain: "a.test",
    };
    assert.deepEqual(stepErrors(draft, 1), {});
    const report = await oapi("/apps/preflight", {
      method: "POST",
      body: JSON.stringify(buildPayload(draft)),
    });
    assert.equal(report.ok, false);
    assert.equal(seen[0][0], "/api/v1/orgs/default/apps/preflight");
    assert.equal(seen[0][1].port, 8000);
    // The view jumps to the earliest failing step and focuses its field.
    const { fieldErrors, firstField, step } = mapIssuesToFields(report.errors);
    assert.deepEqual(fieldErrors, {
      port: "port 8000 (or its blue/green pair) is already claimed by 'web'",
      domain: "domain 'a.test' is already used by 'web'",
    });
    assert.equal(firstField, "port");
    assert.equal(step, FIELD_STEPS.port);
    assert.equal(report.warnings.length, 1);
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("create wizard: single-field submit errors map like preflight issues", () => {
  // A 409 without an errors array still lands on its field (backward compat).
  const mapped = mapIssuesToFields([
    { field: "mem_limit_mb", code: "conflict", detail: "memory quota exceeded" },
  ]);
  assert.equal(mapped.firstField, "mem_limit_mb");
  assert.equal(mapped.step, FIELD_STEPS.mem_limit_mb);
});

test("detail view: queued or installing deployments keep the list live", () => {
  assert.equal(hasPendingDeployment(null), false);
  assert.equal(hasPendingDeployment([]), false);
  assert.equal(hasPendingDeployment([{ status: "running" }]), false);
  assert.equal(hasPendingDeployment([{ status: "running" }, { status: "queued" }]), true);
  assert.equal(hasPendingDeployment([{ status: "installing" }]), true);
  assert.equal(hasPendingDeployment([{ status: "failed" }]), false);
});
