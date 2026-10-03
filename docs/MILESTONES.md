# turaes milestones

Detailed scope, deliverables and **exit criteria** for each milestone. A
milestone is complete only when every exit-criterion box is checked and
`make verify` is green. Status mirrors [ROADMAP.md](./ROADMAP.md).

---

## M0 — Scaffold ✅ (2026-10-01 → 10-03)

**Goal**: every architectural contract exists, compiles and is tested, without
committing to the full feature set.

### Deliverables

- [x] Cargo workspace: `turaes-core`, `turaes-runtime`, `turaes-proxy`, `turaes-monitor`, bin `turaes`.
- [x] `Config` (embedded `config/default.toml` + file + `TURAES_*` env) with validation.
- [x] SQLite schema + embedded migrations (`applications`, `deployments`, `env_vars`,
      `domains`, `health_checks`, `health_results`, `app_metrics`, `visit_metrics`, `events`).
- [x] `Error` → JSON `{"detail"}` mapping (422/401/403/404/409/500).
- [x] `crypto`: session JWT (HS256) + AES-256-GCM secret sealing.
- [x] `Runtime` trait + `SystemdRuntime` (hardened unit rendering) + `ProcRuntime`
      + `Deployer` with artifact hashing.
- [x] `Router`/`RouteTable` (ArcSwap, wildcard base domain) + `CertStore` (certbot layout).
- [x] Monitor primitives: Prometheus parser, visitor folding, health `Threshold`,
      cgroup/`/proc` parsers, rollup bucketing.
- [x] Axum app: GitHub OAuth allowlist auth, apps CRUD + deploy + stats/visitors,
      public `/health`, `/auth/*`, embedded UI.
- [x] CLI: `serve`, `migrate`, `doctor`.
- [x] CI workflow + `make verify` gate.
- [~] `pingora` data plane: interfaces + feature gate present; real proxy body is M2.

### Exit criteria

- [x] `make verify` green (clippy `-D warnings`, fmt check, all tests).
- [x] `turaes serve` boots, `/health` returns ok, apps can be created and listed.
- [x] No container/Docker dependency anywhere in the tree.

---

## M1 — Deploy 🔄 (2026-10-04 → 10-17)

**Goal**: take a prebuilt binary and run it reliably on the server.

### Deliverables

- [x] Deploy starts/stops/restarts via `SystemdRuntime`; `proc` fallback for non-root/dev.
- [x] Deployments persisted with status transitions (`queued → installing → starting → running|failed`).
- [x] Health checks run on an interval with thresholds and auto-restart.
- [x] Per-app state dir + env file written, with service user auto-created and ownership set.
- [x] One-shot CLI (`turaes app add|deploy|list|show`) for bootstrap without OAuth.
- [ ] `GET /apps/{id}/logs` streams journald / supervisor log tail.
- [ ] Rollback to the previous artifact on failed start (best-effort).

### Exit criteria

- [x] `beruang` deploys on the VPS and answers `GET /health` on its port.
- [x] `systemctl status beruang` is healthy; unit has `Restart=always`.
- [ ] Integration test covers deploy failure → app marked `failed`.
- [ ] Logs endpoint returns recent lines for both runtimes.

---

## M2 — Proxy ✅ (2026-10-18 → 10-31)

**Goal**: route hostnames through the embedded Pingora proxy with TLS.

Execution plan: [PLAN-M2-PROXY.md](./PLAN-M2-PROXY.md).

### Deliverables

- [x] `ProxyHttp` impl resolves `Host` → upstream (verified compiling on Linux).
- [x] Route table rebuilt from DB and published on boot + deploy/delete.
- [x] Wildcard `*.{base_domain}` routing for multitenant apps.
- [x] TLS settings loaded from certbot `fullchain.pem`/`privkey.pem` (single cert).
- [x] Unknown host → 404.
- [x] `turaes-proxy` builds with `--features proxy` in CI (`release.yml`); proxy check is a required gate.
- [ ] Multi-cert SNI selection (per-app domains).
- [~] HTTP→HTTPS redirect done; ACME webroot route for renewals still pending.
- [~] Forward `X-Forwarded-For/Proto`, `X-Real-IP`, and CDN country headers explicitly.
- [ ] Graceful cert reload on renewal (currently restart via certbot hooks).

### Exit criteria

- [x] `https://turaes.rayakala.ink` serves the dashboard with a valid cert (HTTP/2).
- [ ] Adding a domain takes effect without a process restart.
- [ ] beruang reachable through the proxy on its own hostname.
- [ ] A proxy integration test (curl through the proxy) passes in CI.

**Network verified (Phase 0 ✅)**: inbound TCP 80/443 reach the box from the
internet; `turaes.rayakala.ink` resolves here. Note `kalkulator.rayakala.ink`
must **not** be pointed here (it resolves to another server).

---

## M3 — Monitoring 🔄 (2026-11-01 → 11-14)

**Goal**: the headline feature — CPU, memory, visitors — from live data.

### Deliverables

- [x] Background monitor loop: health + `/metrics` scrape + resource sampling.
- [x] CPU% from `cpu.stat` deltas; memory from `memory.current`; `/proc` fallback.
- [x] Visitors folded from `visitors_total` + `unique_visitors_estimate` per region.
- [x] Interval rollups written to `app_metrics` / `visit_metrics`; retention compaction.
- [x] `GET /apps/{id}/stats` and `/visitors` serve rolled-up history.
- [x] 1-minute downsampling (minute-bucket upserts; one row per app/region per minute).
- [x] Dashboard (zero-build Preact/HTM): per-app cards + time-series charts (SVG, no chart lib).
- [ ] App's own `/metrics` re-export (dogfood via tonggeret).

### Exit criteria

- [x] beruang reports CPU%, memory and visitor totals from live scrapes (`turaes app show beruang`).
- [x] Unique visitors are latest-per-region, never summed (verified by test).
- [ ] A 24h window renders without gaps when the app is up.
- [x] Retention deletes samples older than `retention_days`.

---

## M4 — Ops ⏳ (2026-11-15 → 11-28)

**Goal**: make day-to-day operation safe and pleasant.

### Deliverables

- [~] Rollback API + CLI + UI button done; deployment history API + table done; live logs/WebSocket pending.
- [x] Environment variable editor (sealed at rest; API returns keys only; UI add/remove).
- [~] Live log streaming over WebSocket done; deploy-progress/health realtime still pending.
- [ ] Domain management (add/remove/verify/primary) end to end.
- [ ] Audit `events` timeline per app.

### Exit criteria

- [ ] Rollback restores the prior binary and status.
- [ ] Uploaded secrets are never returned in plaintext by the API.
- [ ] Live logs stream over WebSocket without polling.
- [ ] Events timeline reflects deploys, restarts and health flips.

---

## M5 — Auto-deploy ⏳ (2026-11-29 → 12-12)

**Goal**: push-to-deploy and (optionally) multi-server.

### Deliverables

- [ ] GitHub + GitLab webhooks with signature/token validation.
- [ ] Branch filter + auto-deploy on matching push; delivery log.
- [ ] Git-based build strategy (clone + configurable command, e.g. `cargo build --release`).
- [ ] Deploy keys for private repos.
- [ ] (Optional) multiple servers with SSH transport.

### Exit criteria

- [ ] A push to `main` triggers a deploy and records a delivery.
- [ ] A `cargo` project builds from Git and deploys without a manual artifact.
- [ ] Webhook signature failures are rejected with 401 and never deploy.

---

## N0–N4 — Multi-node fleet 🔄 (after M5)

Full design: [PLAN-MULTINODE.md](./PLAN-MULTINODE.md). Decisions: fleet-first,
same Tencent VPC, hybrid SSH bootstrap + gRPC agent, separate Pingora edge.

| Phase | Scope | Status |
|---|---|---|
| **N0** | `servers` table + `applications.server_id`, `/api/v1/servers`, `turaes server …`, UI panel | ✅ |
| **N1** | ArtifactStore + SSH bootstrap + `turaes agent` over gRPC + `AgentRuntime` | ✅ (artifact store, register/heartbeat, poll/report reconcile, SSH bootstrap) |
| **N2** | agent-pushed CPU/mem + visitor metrics + health transitions; per-node view | ✅ |
| **N3** | `turaes edge` role (gRPC route/cert stream); Pingora off the control plane | ✅ (`turaes edge` + routes + cert distribution + multi-cert SNI) |
| **N4** | DNS-01 multi-cert/wildcard SNI + replicas/LB + control-plane HA | 🔄 (placements + LB + aliases done; DNS-01/HA pending) |

**Backlog:** ACME webroot route — ✅ done (the proxy serves `/.well-known/acme-challenge/`).

---

## Cross-milestone definition of done

1. `make verify` green.
2. New public APIs documented in [API.md](./API.md); architecture changes in
   [ARCHITECTURE.md](./ARCHITECTURE.md).
3. Tests cover the new behavior (unit + integration where applicable).
4. No Docker/container runtime introduced.
5. Graph refreshed (`graphify update .`).
