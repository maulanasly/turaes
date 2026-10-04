import { test } from "node:test";
import assert from "node:assert/strict";
import { orgPath, setOrg } from "../../static/js/lib/api.js";

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
