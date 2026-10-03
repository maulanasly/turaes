# turaes HTTP API

Base path: `/` on the configured port (default `8787`). All `/api/*` routes
require a session cookie except `/health` and the OAuth handshake. Errors are
JSON `{"detail": "..."}`.

Status codes: `200` ok · `201` created · `204` no content · `401` unauthenticated ·
`403` not on allowlist · `404` unknown id · `409` name conflict · `422` bad input ·
`500` internal.

## Authentication

GitHub OAuth (authorization code) with an allowlist of numeric GitHub user ids.
The session is an HttpOnly, SameSite=Lax JWT cookie.

| Method | Path | Description |
|---|---|---|
| `GET` | `/auth/login` | Redirects to GitHub; sets a short-lived CSRF `state` cookie |
| `GET` | `/auth/callback?code=&state=` | Exchanges the code, enforces the allowlist, sets the session cookie, redirects to `/` |
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

## Applications

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/apps` | List applications |
| `POST` | `/api/v1/apps` | Create an application (`201`) |
| `GET` | `/api/v1/apps/{id}` | Fetch one |
| `DELETE` | `/api/v1/apps/{id}` | Remove app + stop it (`204`) |
| `POST` | `/api/v1/apps/{id}/deploy` | Install + restart the prebuilt binary |
| `POST` | `/api/v1/apps/{id}/rollback` | Redeploy the previous artifact |
| `GET` | `/api/v1/apps/{id}/stats?hours=1` | CPU/memory samples (max 720h) |
| `GET` | `/api/v1/apps/{id}/visitors?hours=24` | Visitor rows per region (max 720h) |
| `GET` | `/api/v1/apps/{id}/deployments?limit=20` | Recent deployments (log truncated to 2000 chars) |
| `GET` | `/api/v1/deployments/{id}` | Full deployment record incl. log |
| `GET` | `/api/v1/artifacts/{hash}` | Download a stored artifact by `sha256:<hex>` |

Deploys first store the binary in the content-addressed artifact store
(`{artifact_dir}/sha256/{hex}`, mode preserved) and install from that copy, so
rollback and (later) remote agents can reuse it. `POST .../rollback` redeploys
the most recent **different** artifact; `422` if there is no previous one.

`server_id` (see [Servers](#servers)) places the app on a node; it defaults to
`local`. Deploy to a non-local server is rejected until N1.

### Create

```bash
curl -X POST localhost:8787/api/v1/apps \
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
| `binary_path` | yes | absolute path to a prebuilt binary on the server |
| `port` | yes | loopback port the process binds |
| `description` | no | free text |
| `args` | no | extra `ExecStart` arguments |
| `health_path` | no | default `/health` |
| `metrics_path` | no | default `/metrics` |
| `domain` | no | primary hostname routed by the proxy |
| `runtime` | no | `systemd` (default) or `proc` |
| `auto_restart` | no | default `true` |

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
curl -X POST localhost:8787/api/v1/apps/5cffb37d-…/deploy
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

`GET /api/v1/apps/{id}/stats?hours=1` →

```json
{ "metrics": [ { "id": "…", "application_id": "…", "cpu_pct": 12.4, "mem_bytes": 15728640, "recorded_at": "…" } ] }
```

`GET /api/v1/apps/{id}/visitors?hours=24` →

```json
{ "visitors": [ { "id": "…", "application_id": "…", "region": "ID", "visits": 128, "uniques": 41, "recorded_at": "…" } ] }
```

> Stats/visitors are populated by the monitoring loop (M3); in M0 the arrays are
> empty until that loop lands.

## Servers

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/servers` | List nodes (local first) |
| `POST` | `/api/v1/servers` | Register a node (`name`, `address`, optional `ssh_*`) |
| `GET` | `/api/v1/servers/{id}` | Fetch one |
| `DELETE` | `/api/v1/servers/{id}` | Remove a node (not `local`; refuses if apps are placed) |
| `POST` | `/api/v1/servers/{id}/validate` | Reachability (TCP to SSH for remote) |

SSH keys are sealed at rest and never returned. See
[PLAN-MULTINODE.md](./PLAN-MULTINODE.md).

## Conventions

- Timestamps are RFC3339-ish UTC text (`datetime('now')`).
- Booleans in JSON are real booleans; in SQLite they are `0/1`.
- The API never returns decrypted environment variables.
