import { test } from "node:test";
import assert from "node:assert/strict";
import { api, orgPath, setOrg } from "../../static/js/lib/api.js";

test("orgPath prefixes tenant-scoped routes with the active org", () => {
  setOrg("default");
  assert.equal(orgPath("/apps"), "/api/v1/orgs/default/apps");
  assert.equal(
    orgPath("/apps/x/deployments?limit=15"),
    "/api/v1/orgs/default/apps/x/deployments?limit=15"
  );
});

test("setOrg switches the prefix; empty values are ignored", () => {
  setOrg("acme");
  assert.equal(orgPath("/apps"), "/api/v1/orgs/acme/apps");
  setOrg("");
  assert.equal(orgPath("/apps"), "/api/v1/orgs/acme/apps");
  setOrg(undefined);
  assert.equal(orgPath("/apps"), "/api/v1/orgs/acme/apps");
});

test("api preserves structured field errors for forms", async () => {
  const previousFetch = globalThis.fetch;
  globalThis.fetch = async () => new Response(JSON.stringify({
    detail: "conflict: port is already claimed",
    code: "conflict",
    field: "port",
  }), { status: 409, headers: { "content-type": "application/json" } });
  try {
    await assert.rejects(api("/api/v1/test"), (err) => {
      assert.equal(err.status, 409);
      assert.equal(err.code, "conflict");
      assert.equal(err.field, "port");
      assert.equal(err.message, "conflict: port is already claimed");
      return true;
    });
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("api preserves multi-error arrays alongside the first field", async () => {
  const previousFetch = globalThis.fetch;
  const errors = [
    { field: "port", code: "conflict", detail: "claimed" },
    { field: "domain", code: "conflict", detail: "taken" },
  ];
  globalThis.fetch = async () => new Response(JSON.stringify({
    detail: "invalid input: claimed",
    code: "validation",
    field: "port",
    errors,
  }), { status: 422, headers: { "content-type": "application/json" } });
  try {
    await assert.rejects(api("/api/v1/test"), (err) => {
      assert.equal(err.status, 422);
      assert.equal(err.field, "port");
      assert.deepEqual(err.errors, errors);
      return true;
    });
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("api merges caller headers without dropping the JSON default", async () => {
  const previousFetch = globalThis.fetch;
  let seen;
  globalThis.fetch = async (url, options) => {
    seen = options;
    return new Response(JSON.stringify({ ok: true }), { status: 200 });
  };
  try {
    await api("/api/v1/test", { headers: { "x-extra": "1" } });
    assert.equal(seen.headers["content-type"], "application/json");
    assert.equal(seen.headers["x-extra"], "1");
    assert.equal(seen.credentials, "same-origin");
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("api notifies unauthorized subscribers on 401 only", async () => {
  const { onUnauthorized } = await import("../../static/js/lib/api.js");
  const previousFetch = globalThis.fetch;
  let calls = 0;
  const off = onUnauthorized(() => { calls += 1; });
  try {
    globalThis.fetch = async () => new Response("{}", { status: 401 });
    await assert.rejects(api("/api/v1/test"), (err) => err.status === 401);
    assert.equal(calls, 1);
    globalThis.fetch = async () => new Response("{}", { status: 403 });
    await assert.rejects(api("/api/v1/test"));
    assert.equal(calls, 1);
    off();
    globalThis.fetch = async () => new Response("{}", { status: 401 });
    await assert.rejects(api("/api/v1/test"));
    assert.equal(calls, 1);
  } finally {
    globalThis.fetch = previousFetch;
  }
});
