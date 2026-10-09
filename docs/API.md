# turaes HTTP API

Base path: `/` on the configured port (default `8787`). All `/api/*` routes
require a session cookie except `/health` and the OAuth handshake. Errors are
JSON `{"detail": "...", "code": "..."}` — switch on the stable `code`
(`config`, `not_found`, `unauthorized`, `forbidden`, `bad_request`,
`conflict`, `db`, `io`, `internal`), never on the human `detail` text. App
validation/conflict responses include `field` when the issue maps to one input
(for example, a reserved port); non-field errors omit it. The preflight
endpoint below additionally returns an `errors` array of
`{field, code, detail}` objects describing every problem at once, plus
`warnings` (`{field?, detail}`) and per-check `pass`/`fail`/`unknown`
outcomes — use it to validate a draft before writing anything.

Status codes: `200` ok · `201` created · `204` no content · `401` unauthenticated ·
`403` not on allowlist · `404` unknown id · `409` resource conflict · `422` bad input ·
`500` internal.

Conventions: every list endpoint accepts `?limit` (client cap, clamped to
1..=200; absent returns everything). Timestamps are UTC text
(`YYYY-MM-DD HH:MM:SS`, from SQLite `datetime('now')`); the dashboard parses
them as UTC.

## Authentication

GitHub OAuth (authorization code) with an allowlist of numeric GitHub user ids.
The session is an HttpOnly, SameSite=Lax JWT cookie.

| Method | Path | Description |
|---|---|---|
| `GET` | `/auth/login` | Redirects to GitHub (or to `/` if already signed in); sets a short-lived CSRF `state` cookie |
| `GET` | `/auth/callback?code=&state=` | Exchanges the code, enforces the allowlist, sets the session cookie, redirects to `/`; failures redirect to `/?login_error=<msg>` |
| `GET` | `/auth/me` | Current user (`401` if not signed in) |
| `POST` | `/auth/logout` | Clears the session cookie (`204`) |

`GET /auth/me` →

```json
{ "id": 12345678, "login": "maulanasly", "name": "Maulana" }
```

> Debug builds honor `AUTH_DISABLED=1` (never in release): every request runs as
> `{ "id": 0, "login": "dev", "name": "Dev mode" }`.

## Health

| Method | Path | Auth | Description |
|---|---|---|---|
| `GET` | `/health` | no | Liveness + capabilities |

```json
{ "status": "ok", "version": "0.1.0", "runtime": "systemd", "proxy": false }
```

## Identity

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/me` | Resolved tenant principal: user, organizations and roles |

`GET /api/v1/me` →

```json
{
  "id": "c3b2a1d0-…",
  "github_id": 5284227,
  "login": "maulanasly",
  "name": "Maulana",
  "orgs": [{ "org_id": "default", "slug": "default", "name": "Default", "role": "owner" }]
}
```

Roles, weakest first: `viewer` (read) › `developer` (deploy, lifecycle, env,
domains) › `admin` (apps, servers, tokens) › `owner` (members, org). The first
user to sign in becomes `owner` of the `default` organization; later users
start as `viewer`.

## API tokens

Programmatic/CI access without SSH or a browser. Send
`Authorization: Bearer <token>` instead of the session cookie; a bearer token
always wins when both are present. Tokens are bound to one organization with
one hierarchical scope — `read` (viewer floor), `deploy` (developer floor) or
`admin` (admin floor) — and can never reach `owner`. The effective role is
capped by the creator's *current* membership, so demoting the creator
attenuates the token. Only the SHA-256 hash is stored; the plaintext (returned
exactly once) looks like `turaes_…`.

| Method | Path | Role | Description |
|---|---|---|---|
| `GET` | `/api/v1/orgs/{org}/tokens` | admin | List live tokens (hashes never included) |
| `POST` | `/api/v1/orgs/{org}/tokens` | admin | Mint a token (`201` returns `{token, plaintext}`) |
| `DELETE` | `/api/v1/orgs/{org}/tokens/{id}` | admin | Revoke a token (`204`) |

```bash
curl -X POST localhost:8787/api/v1/orgs/default/tokens \
  -H 'content-type: application/json' \
  -d '{"name": "ci-deploy", "scopes": "deploy"}'
# → { "token": { "id": "…", "scopes": "deploy", … }, "plaintext": "turaes_…" }

curl localhost:8787/api/v1/orgs/default/apps \
  -H 'Authorization: Bearer turaes_…'
```

## Organizations

Any signed-in user may create an organization and becomes its `owner`.
Managing members requires `owner`; the member roster is visible to `viewer`
and up. The last owner can neither be demoted nor removed. Invites accept a
GitHub login (resolved via the public GitHub users API) or a raw numeric
GitHub id.

| Method | Path | Role | Description |
|---|---|---|---|
| `GET` | `/api/v1/orgs` | any | The caller's organizations, with roles |
| `POST` | `/api/v1/orgs` | any | Create an organization (`201`; caller becomes `owner`) |
| `GET` | `/api/v1/orgs/{org}/members` | viewer | Roster with roles |
| `POST` | `/api/v1/orgs/{org}/members` | owner | Invite (`{login}` or `{github_id}`, optional `{role}`) |
| `PATCH` | `/api/v1/orgs/{org}/members/{user_id}` | owner | Change a member's role |
| `DELETE` | `/api/v1/orgs/{org}/members/{user_id}` | owner | Remove a member |

## Applications

All app routes are nested under `/api/v1/orgs/{org}/…`, where `{org}` is an
organization id or slug. Unknown orgs yield `404`; apps outside the caller's
org yield `404` (never `403`, so tenants cannot probe each other); insufficient
roles yield `403`.

| Method | Path | Role | Description |
|---|---|---|---|
| `GET` | `/api/v1/orgs/{org}/apps` | viewer | List the org's applications |
| `POST` | `/api/v1/orgs/{org}/apps` | admin | Create an application (`201`) |
| `POST` | `/api/v1/orgs/{org}/apps/preflight` | admin | Validate a draft without writing (`200` always; `ok:false` when `errors` is non-empty; pass `app_id` to preview an edit) |
| `GET` | `/api/v1/orgs/{org}/apps/{id}` | viewer | Fetch one |
| `PATCH` | `/api/v1/orgs/{org}/apps/{id}` | admin | Edit fields + placement |
| `DELETE` | `/api/v1/orgs/{org}/apps/{id}` | admin | Remove app + stop it (`204`) |
| `POST` | `/api/v1/orgs/{org}/apps/{id}/deploy` | developer | Apply a workload version (binary, static files, or worker) using slots |
| `POST` | `/api/v1/orgs/{org}/apps/{id}/rollback` | developer | Roll back a binary build or previous static slot; command apps do not retain prior argv for rollback |
| `POST` | `/api/v1/orgs/{org}/apps/{id}/{stop,start,restart}` | developer | Lifecycle (local apps) |
| `POST` | `/api/v1/orgs/{org}/apps/{id}/maintenance` | developer | Toggle maintenance page (`{"enabled":bool}`); units keep running |
| `GET/POST` | `/api/v1/orgs/{org}/apps/{id}/domains` | viewer / developer | List / add domain aliases |
| `DELETE` | `/api/v1/orgs/{org}/apps/{id}/domains/{domain}` | developer | Remove an alias |
| `GET` | `/api/v1/orgs/{org}/apps/{id}/stats?hours=1` | viewer | CPU/memory samples (max 720h) |
| `GET` | `/api/v1/orgs/{org}/apps/{id}/visitors?hours=24` | viewer | Visitor rows per region (max 720h) |
| `GET` | `/api/v1/orgs/{org}/apps/{id}/deployments?limit=20` | viewer | Recent deployments (log truncated to 2000 chars) |
| `GET` | `/api/v1/orgs/{org}/deployments/{id}` | viewer | Full deployment record incl. log |
| `GET` | `/api/v1/orgs/{org}/audit?app=&action=&limit=50` | viewer | Newest-first audit timeline (actor login resolved; `system` actions have none) |
| `GET` | `/api/v1/orgs/{org}/apps/{id}/logs` | developer | **WebSocket** live logs (`journalctl -f` / `tail -f`; local apps only) |
| `GET` | `/api/v1/orgs/{org}/apps/{id}/env` | developer | List env var **keys** (values are never returned) |
| `PUT` | `/api/v1/orgs/{org}/apps/{id}/env/{key}` | developer | Create (`201`) or replace (`204`) an env var (AES-256-GCM sealed) |
| `DELETE` | `/api/v1/orgs/{org}/apps/{id}/env/{key}` | developer | Remove an env var |
| `GET` | `/api/v1/artifacts/{hash}` | developer | Download a stored artifact by `sha256:<hex>` — only when the hash backs a deployment of an app in one of the caller's orgs (`404` otherwise) |

Loopback ports are a host-global resource: creating or re-porting an app to a
port (or its blue/green pair) already claimed by *any* application yields
`409`. App names stay globally unique because they become systemd unit names.

Deploys first store the binary in the content-addressed artifact store
(`{artifact_dir}/sha256/{hex}`, mode preserved) and install from that copy, so
rollback and (later) remote agents can reuse it. `POST .../rollback` redeploys
the most recent **different** artifact; `422` if there is no previous one.

`server_id` (see [Servers](#servers)) places the app on a node; it defaults to
`local`. Deploy to a non-local server is rejected until N1.

### Create

```bash
curl -X POST localhost:8787/api/v1/orgs/default/apps \
  -H 'content-type: application/json' \
  -d '{
    "name": "beruang",
    "binary_path": "/srv/beruang/target/release/beruang-gateway",
    "port": 8000,
    "domain": "kalkulator.rayakala.ink",
    "health_path": "/health",
    "metrics_path": "/metrics",
    "runtime": "systemd",
    "auto_restart": true
  }'
```

| Field | Required | Notes |
|---|---|---|
| `name` | yes | lowercase slug `a-z0-9-`, ≤ 64 chars; becomes the unit + install name |
| `binary_path` | service/binary | absolute path to a prebuilt binary on the server (server uses `command[0]` / `publish_dir` when those are set) |
| `command` | service+command / worker | exec argv array, e.g. `["/opt/venv/bin/python", "worker.py"]` (mutually exclusive with `args`; no shell) |
| `workdir` | no | `WorkingDirectory` override (defaults to the state dir) |
| `publish_dir` | static only | source directory synced per deploy |
| `kind` | no | `service` (default), `static`, or `worker`; immutable after create |
| `port` | service/static | loopback port the process binds (workers pass `0` or omit) |
| `description` | no | free text |
| `args` | no | extra `ExecStart` arguments |
| `health_path` | no | default `/health` |
| `metrics_path` | no | default `/metrics` (service only) |
| `domain` | no | primary hostname routed by the proxy (service/static) |
| `runtime` | no | `systemd` (default) or `proc` |
| `auto_restart` | no | default `true` |
| `mem_limit_mb` | no | memory ceiling 16–65536 (`systemd` only; takes effect on next deploy/restart) |
| `cpu_quota_pct` | no | CPU ceiling 1–6400 (% of one core; `systemd` only) |

`PATCH` accepts the same optional fields; `mem_limit_mb`/`cpu_quota_pct` and
`command`/`workdir`/`publish_dir` are tri-state (absent keeps, `null` clears,
a value sets). `kind` is immutable after create. Limits with the `proc`
runtime are rejected (`422`) rather than silently ignored.

Primary domains and aliases share one namespace: claiming a domain already
used as another app's primary or alias yields `409` on the `domain` field,
as does aliasing an app's own primary. Hostnames are normalized to lowercase
before comparison. The dashboard exposes the full launch model — prebuilt
binary, explicit argv `command` (one argument per line, no shell), and
`workdir` — in both the create wizard (Advanced launch) and Settings, with
the same tri-state semantics as `PATCH`.

## Quotas

Per-organization budgets. Quota accounting sums explicit limits only —
unlimited apps count toward `max_apps` but not the memory/CPU budgets.

| Method | Path | Role | Description |
|---|---|---|---|
| `GET` | `/api/v1/orgs/{org}/quota` | viewer | Ceilings plus current usage |
| `PUT` | `/api/v1/orgs/{org}/quota` | owner | Replace ceilings (omitted fields keep) |

Exceeding a budget on create/update/domain-claim yields `409`.

## Alerts

The monitor evaluates alert rules every tick (unhealthy apps, recent failed
deploys, stale backups) and notifies a webhook once on fire and once on
resolve. With no webhook configured, alerts are still recorded for the
dashboard banner. Deduplication is by key — one firing row per key, ever.

| Method | Path | Role | Description |
|---|---|---|---|
| `GET` | `/api/v1/orgs/{org}/alerts?status=firing&limit=50` | viewer | Newest-first timeline (platform alerts included for admins) |
| `POST` | `/api/v1/orgs/{org}/alerts/{id}/resolve` | developer | Manually resolve (platform alerts need admin) |

Set `TURAES_ALERT_WEBHOOK_URL` to a generic JSON receiver (Discord/Slack
incoming webhooks both accept the `text`/`content` payload).

Response (`201`):

```json
{
  "application": {
    "id": "5cffb37d-…",
    "name": "beruang",
    "binary_path": "/srv/beruang/target/release/beruang-gateway",
    "port": 8000,
    "health_path": "/health",
    "metrics_path": "/metrics",
    "domain": "kalkulator.rayakala.ink",
    "runtime": "systemd",
    "auto_restart": true,
    "status": "stopped",
    "created_at": "2026-10-01 15:54:24",
    "updated_at": "2026-10-01 15:54:24"
  }
}
```

### Deploy

```bash
curl -X POST localhost:8787/api/v1/orgs/default/apps/5cffb37d-…/deploy
```

```json
{
  "deployment_id": "…",
  "state": "running",
  "artifact_hash": "sha256:…",
  "log": "artifact sha256:…\napplied runtime config\nstarted service\nstate: running\n"
}
```

The deploy is idempotent. On failure the app is marked `failed` and the error is
`{"detail": ...}`; the deployment row keeps the log.

### Stats / visitors

`GET /api/v1/orgs/{org}/apps/{id}/stats?hours=1` →

```json
{ "metrics": [ { "id": "…", "application_id": "…", "cpu_pct": 12.4, "mem_bytes": 15728640, "recorded_at": "…" } ] }
```

`GET /api/v1/orgs/{org}/apps/{id}/visitors?hours=24` →

```json
{ "visitors": [ { "id": "…", "application_id": "…", "region": "ID", "visits": 128, "uniques": 41, "recorded_at": "…" } ] }
```

> Stats/visitors are populated by the monitoring loop (M3); in M0 the arrays are
> empty until that loop lands.

## Catalog

Curated starter templates for one-click app creation, plus the serving
platform version. Templates are platform-global (identical for every org);
the route is org-scoped only so membership is enforced like every other
read. Per-app release roll-ups join this response in P1.

| Method | Path | Role | Description |
|---|---|---|---|
| `GET` | `/api/v1/orgs/{org}/catalog` | viewer | `{templates: [...], platform: {version}}` |

Each template carries `slug`, `name`, `description`, `kind` and a `defaults`
object whose keys map onto wizard draft fields (`#/apps?template=<slug>`
opens the create form prefilled). Unknown keys are ignored.

## Servers

Nodes are platform-global infrastructure, so these routes stay outside the org
nest and require an operator (`admin` or above in any organization).

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/servers` | List nodes (local first); each carries a `capacity` sample or `null` |
| `POST` | `/api/v1/servers` | Register a node (`name`, `address`, optional `ssh_*`) |
| `GET` | `/api/v1/servers/{id}` | Fetch one |
| `GET` | `/api/v1/servers/{id}/stats?hours=` | Host CPU/memory history (minutes; default 24h, max 720h) |
| `DELETE` | `/api/v1/servers/{id}` | Remove a node (not `local`; refuses if apps are placed) |
| `POST` | `/api/v1/servers/{id}/validate` | Reachability (TCP to SSH for remote) |
| `POST` | `/api/v1/servers/{id}/bootstrap` | SSH: install + start the turaes agent on the node |

SSH keys are sealed at rest and never returned. Capacity is the control plane
sampling its own host (`cpu_pct` normalised 0–100, `mem_bytes`/`mem_total_bytes`);
remote nodes report no host stats yet, so their capacity reads back as `null`.
See [PLAN-MULTINODE.md](./PLAN-MULTINODE.md).

## Conventions

- Timestamps are RFC3339-ish UTC text (`datetime('now')`).
- Booleans in JSON are real booleans; in SQLite they are `0/1`.
- The API never returns decrypted environment variables.
