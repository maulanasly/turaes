# turaes architecture

> A Docker-less, self-hosted PaaS. One Rust binary owns the API, the dashboard,
> the process runtime and the reverse proxy; SQLite is the only datastore.

## Goals & non-goals

**Goals**

- Deploy a prebuilt binary to a server and make it reachable by hostname with TLS.
- Supervise native processes (no container runtime) with health-based restarts.
- Surface per-app CPU, memory and visitor metrics scraped from tonggeret.
- Stay small: single binary, embedded UI, SQLite, no external services.

**Non-goals (for now)**

- Building from source (M0 takes a prebuilt binary path; repo builds later).
- Multi-node orchestration.
- In-process ACME (certbot owns issuance).

## Component map

```
                       ┌──────────────────────────────────────────┐
   Internet ──80/443──▶│ Pingora proxy (turaes-proxy)             │
                       │  · Host → upstream (ArcSwap RouteTable)   │
                       │  · TLS: certbot certs, graceful reload    │
                       └───────────────▲──────────────────────────┘
                                       │ embedded
┌──────────────────────────────────────┴───────────────────────────────────┐
│ turaes (single binary)                                                    │
│                                                                           │
│  Axum API ── auth (GitHub OAuth allowlist, HttpOnly JWT session)          │
│     │                                                                     │
│  routes/apps ── deploy ─▶ Deployer ─▶ Runtime trait                       │
│                                     ├── SystemdRuntime  (units + cgroups) │
│                                     └── ProcRuntime     (spawn + pidfile) │
│                                                                           │
│  Monitor (M3) ── health polls, /metrics scrape, cgroup//proc sampling     │
│                                                                           │
│  SQLite (WAL) ── applications, deployments, env_vars, domains,            │
│                  health_*, app_metrics, visit_metrics, events             │
│  Embedded UI (rust-embed, zero-build)                                     │
└───────────────────────────────────────────────────────────────────────────┘
```

| Crate | Responsibility |
|---|---|
| `turaes-core` | `Config` (toml + `TURAES_*` env), `db` (pool + embedded migrations), `models`, `Error`, `crypto` (JWT, AES-256-GCM) |
| `turaes-runtime` | `Runtime`/`AppSpec`/`RunState`, `SystemdRuntime`, `ProcRuntime`, `Deployer` + artifact hashing |
| `turaes-proxy` | `Router`/`RouteTable` (ArcSwap), `CertStore` (certbot layout), Pingora data plane (feature-gated) |
| `turaes-monitor` | Prometheus text parser, visitor folding, HTTP health `Threshold`, cgroup/`/proc` stats, rollup bucketing |
| `src/` | CLI (`serve`/`migrate`/`doctor`/`backup`/`restore`/`secrets`/`gc`/`app`/`server`/`apply`/`serve-static`, …), Axum router, OAuth, handlers, embedded static UI |

## Data flow

### Deploy (prebuilt binary)

```
POST /api/v1/apps/{id}/deploy
  → load app + decrypt env_vars
  → build AppSpec (installed_path, state_dir, env_file, PORT)
  → Deployer::deploy
       1. artifact_hash = sha256(binary_path)
       2. Runtime::apply   (copy binary, write env file, render unit/pid)
       3. Runtime::restart
       4. Runtime::status
  → persist deployment row + app.status
  → (M2) publish new RouteTable snapshot
```

`AppSpec` is the only contract between the control plane and the runtime. A
deploy is idempotent: re-running it re-installs and restarts.

Application kinds share the slot machinery with kind-specific steps:
`service` (binary or argv, health-gated), `static` (publish dir synced per
slot, served by `turaes serve-static`, always-200 gate), `worker` (no port,
no gate, no route; supervision + restart only). Command apps execute argv in
place (nothing is installed or stored; history records a `cmd:` identity
hash); static history records a `dir:` content hash and rolls back by cutting
to the surviving slot.

### Monitoring (the tonggeret path)

```
every interval_secs:
  health:  GET  http(s)://127.0.0.1:<port><health_path>  → Threshold → restart on breach
  scrape:  GET  http://127.0.0.1:<port>/metrics           → parse_prometheus
                                                     → visitor_samples (per region)
  stats:   cgroup v2  system.slice/{app}.service/{memory.current,cpu.stat}
           or /proc/<pid>/stat + smaps_rollup            → cpu%, RSS
  → 1-minute rollups into app_metrics / visit_metrics
```

Visitor semantics follow tonggeret exactly:

- `visitors_total{region}` is a **counter** — sum across scrapes for totals.
- `unique_visitors_estimate{region}` is a **gauge** (HyperLogLog) — take the
  **latest** value per region, never `sum()`.
- Region comes from CDN country headers passed through the proxy
  (`CF-IPCountry` → `X-Vercel-IP-Country` → `CloudFront-Viewer-Country`), else
  `unknown`. Infra paths (`/metrics`, `/health`) are excluded from visits.

### Proxy routing

```
Host: kalkulator.rayakala.ink
  → RouteTable.resolve: exact match, else strip ".{base_domain}" and match app
  → Upstream { 127.0.0.1:8000, tls }
  → HttpPeer::new(addr, tls, sni)
```

The table is built from `applications.domain` + `domains` rows and published
atomically after each deploy/domain change. Unknown hosts return 404.

## Docker-less runtime design

`Runtime` is deliberately small so both supervisors behave identically.
Nothing in it is Rust-specific: any executable honoring `$PORT` and a 2xx
health path deploys — Go/Zig/C binaries, `deno compile` output, shebang
scripts, or explicit interpreter argv (`command`, no shell ever).

```rust
#[async_trait]
pub trait Runtime {
    async fn apply(&self, spec: &AppSpec, env: &BTreeMap<String, String>) -> Result<()>;
    async fn start(&self, spec: &AppSpec) -> Result<()>;
    async fn stop(&self, spec: &AppSpec) -> Result<()>;
    async fn restart(&self, spec: &AppSpec) -> Result<()>;
    async fn remove(&self, spec: &AppSpec) -> Result<()>;
    async fn status(&self, spec: &AppSpec) -> Result<RunState>;
    async fn logs(&self, spec: &AppSpec, lines: usize) -> Result<String>;
}
```

**SystemdRuntime** renders a hardened unit (mirroring the fleet's existing
`beruang.service` / `monthly-logs.service`):

```ini
[Service]
Type=simple
User={app}
Group={app}
WorkingDirectory=/var/lib/{app}
EnvironmentFile=/etc/{app}.env
Environment=PORT={port}
ExecStart=/usr/local/bin/{app} {args}
Restart=always
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=/var/lib/{app}
MemoryMax={mem_limit_mb}M    # only when the app sets a limit
CPUQuota={cpu_quota_pct}%    # only when the app sets a limit
TasksMax=512
```

Per-org quotas (`org_quotas`: apps, summed memory/CPU limits, hostnames) are
enforced at the API on create/update/domain-claim; limits render into the unit
on the next deploy/restart.

**ProcRuntime** spawns `installed_path`, redirects stdout/stderr to
`state_dir/{app}.log`, tracks the pid in `state_dir/{app}.pid`, and signals via
`kill`. It needs no root — used for development and as a fallback.

## Tenancy model

turaes is multi-tenant. Every application is owned by an **organization**;
users join organizations with a **role**. Infrastructure (`servers`) stays
platform-global — operators own the fleet, tenants own apps.

```
users ──< memberships(role) >── organizations ──< applications ──< deployments
                                      │                              └─ env_vars, domains
                                      ├──< api_tokens (scoped)
                                      └──< audit_log
```

- Roles, highest first: `owner` (org settings + members) › `admin` (apps,
  servers, tokens) › `developer` (deploy, lifecycle, env, domains) › `viewer`
  (read-only).
- `api_tokens` are hashed (SHA-256), scoped (`read`/`deploy`/`admin`) and
  org-bound, for CI/programmatic access without SSH.
- `audit_log` records every mutating action with actor, org and target.
- Migration `007_tenancy.sql` seeds a `default` organization and assigns all
  pre-existing applications to it.

> Status: schema + models land first (migration 007); authorization middleware,
> scoped routes and the audit API follow. See `ROADMAP.md`.

## Security posture

- **Auth**: GitHub OAuth authorization-code; only numeric ids on
  `allowed_github_ids` may sign in. Session is an HttpOnly, SameSite=Lax JWT
  cookie (Secure when the origin is https). Debug builds can run `AUTH_DISABLED=1`.
- **Perimeter**: the dashboard/API binds loopback by default and is exposed
  through the proxy; responses carry `nosniff`/`DENY`/same-origin-referrer
  (+HSTS on https origins, no `includeSubDomains`, no CSP — the UI boots from
  an inline script by design); cookie-authed mutations require a JSON content
  type (no CORS layer exists to grant preflights); global rate buckets cover
  `/auth/*` (60/min) and mutating `/api/*` (600/min); handlers time out at
  120s; the systemd unit sandboxes fs/devices/namespaces.
- **Secrets at rest**: app env vars (and sealed agent/SSH material) use
  AES-256-GCM with a key derived from `jwt_secret` via HKDF-SHA256 under a
  dedicated info string — independent of the JWT signing key. Blobs carry a
  `v1$` prefix; unprefixed pre-HKDF blobs (key = SHA-256 of the secret) still
  decrypt until `turaes secrets reseal` migrates them. Rotation: set the new
  secret, keep the old one in `TURAES_SECRET_PREVIOUS`, reseal, verify, unset.
  Release builds refuse the placeholder secret and an empty sign-in allowlist.
- **Isolation**: systemd units run as a dedicated user with `ProtectSystem=strict`
  and a single writable path. This is *process* isolation, not container
  isolation — do not run untrusted code.
- **Trust boundary**: `X-Forwarded-For` / country headers are proxy-set and
  spoofable; visitor numbers are best-effort.

## Validation flow

App drafts are validated in one shared core (`src/routes/app_validation.rs`)
reused by create, update, preflight, and manifest apply: name, workload
shape, runtime, placement, port availability, limits, quotas, domain format,
budget and global availability, plus advisory source-path and no-route
warnings. Handlers stay authoritative and fail fast with a single `field`
error; `POST /apps/preflight` instead returns every problem at once
(`errors[]`), warnings, and per-check `pass`/`fail`/`unknown` outcomes without
writing anything. The dashboard Review step runs preflight on entry and
rechecks immediately before submit.

## Testing strategy

- **Unit**: pure functions — unit-file rendering, env-file parsing, Prometheus
  parsing, cgroup/`/proc` parsers, rollup math, JWT/seal round-trips, router
  resolution, cert path discovery.
- **Integration** (`src/tests.rs`): the real Axum router against a temporary
  SQLite database via `tower::ServiceExt::oneshot`.
- **Frontend** (`tests/frontend`, `node --test`, zero-build): pure form
  helpers, API error-shape preservation, workflow simulation with a stubbed
  `fetch` layer, contrast tokens, and static markup audits.
- **Gate**: `make verify` (clippy `-D warnings` + fmt check + all tests).

## Related projects

- [`nusendra/ployer`](https://github.com/nusendra/ployer) — architecture and API
  shape; container layer replaced by `Runtime`.
- [`cloudflare/pingora`](https://github.com/cloudflare/pingora) — the proxy engine.
- [`maulanasly/tonggeret`](https://github.com/maulanasly/tonggeret) — the metrics
  contract turaes monitors.
- [`maulanasly/tonggeret-dashboard`](https://github.com/maulanasly/tonggeret-dashboard) —
  scrape + history patterns reused here.
