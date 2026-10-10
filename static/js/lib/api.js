// Same-origin JSON API helper. Throws Error with `.status` on failure.
export async function api(path, options = {}) {
  const res = await fetch(path, {
    credentials: "same-origin",
    ...options,
    headers: { "content-type": "application/json", ...options.headers },
  });
  if (res.status === 401) notifyUnauthorized();
  if (res.status === 204) return null;
  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    const err = new Error(body.detail || `HTTP ${res.status}`);
    err.status = res.status;
    err.code = body.code;
    err.field = body.field;
    err.fields = body.fields;
    // Multi-error validation responses (and preflight-style reports
    // surfaced as errors) carry every problem, not just the first.
    err.errors = Array.isArray(body.errors) ? body.errors : null;
    throw err;
  }
  return body;
}

// Session-expiry fan-out: any 401 (except the bootstrap auth probe,
// which handles null itself) notifies subscribers so Shell can re-check
// the session once instead of every view faking empty data.
const unauthorizedListeners = new Set();
export function onUnauthorized(fn) {
  unauthorizedListeners.add(fn);
  return () => unauthorizedListeners.delete(fn);
}
function notifyUnauthorized() {
  for (const fn of [...unauthorizedListeners]) {
    try { fn(); } catch { /* listener errors must not break fetchers */ }
  }
}
// Tenant-scoped API: apps and everything under them live at
// `/api/v1/orgs/{org}/...`. Views call `oapi("/apps")` instead of building
// the org prefix by hand; `setOrg` is fed from `GET /api/v1/me` at sign-in.
let org = "default";
export function setOrg(slug) {
  if (slug) org = slug;
}
export function getOrg() {
  return org;
}
export function orgPath(path) {
  return `/api/v1/orgs/${org}${path}`;
}
export function oapi(path, options = {}) {
  return api(orgPath(path), options);
}
