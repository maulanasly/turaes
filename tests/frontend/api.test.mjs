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
