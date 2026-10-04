# turaes roadmap

Milestones are scope-based, not date-based; the timeline below assumes a
solo-dev cadence of roughly one milestone per 1–2 weeks. Dates are targets,
anchored at project start **2026-10-01**.

Status legend: ✅ done · 🔄 in progress · ⏳ planned

## At a glance

| # | Milestone | Status | Target window | Outcome |
|---|---|---|---|---|
| **M0** | Scaffold | ✅ | 2026-10-01 → 10-03 | Workspace, data model, runtime/proxy/monitor primitives, authenticated API, CI |
| **M1** | Deploy | 🔄 | 2026-10-04 → 10-17 | Prebuilt-binary deploy via systemd + `proc`, health checks, logs |
| **M2** | Proxy | ✅ | 2026-10-18 → 10-31 | Pingora data plane, host routing, domains, certbot TLS + reload |
| **M3** | Monitoring | ✅ | 2026-11-01 → 11-14 | metrics scrape + CPU/mem + visitors, 1-min rollups, Preact/HTM dashboard |
| **M4** | Ops | ⏳ | 2026-11-15 → 11-28 | Deploy history + rollback, env editor, WebSocket realtime |
| **M5** | Auto-deploy | ⏳ | 2026-11-29 → 12-12 | GitHub/GitLab webhooks, git-based build |
| **N0–N4** | Multi-node fleet | 🔄 | after M5 | Control plane + agents + edge — [PLAN-MULTINODE.md](./PLAN-MULTINODE.md) |
| **UX-P0–P3** | Dashboard IA redesign | 🔄 | now | [UX-REVIEW.md](./UX-REVIEW.md) — router, shell, toasts, tabs, light mode, servers |
| **T0–T1** | Multi-tenancy | 🔄 | now | orgs + role-based memberships, scoped API tokens, audit log, then quotas/isolation |

> **Live instance (2026-10-01):** turaes is deployed on `43.173.9.225`
> (Debian 13, systemd) and beruang is running under it with live health, CPU,
> memory and visitor monitoring. See [DEPLOY.md](./DEPLOY.md#live-instance).
>
> **M2 shipped (2026-10-01):** `https://turaes.rayakala.ink` serves the
> dashboard over HTTP/2 with a Let's Encrypt cert, built in GitHub Actions and
> deployed to the VPS. Rollout/fixes: [PLAN-M2-PROXY.md](./PLAN-M2-PROXY.md).
>
> **Next:** M4 ops (deploy history + rollback, env editor, live logs) and the
> multi-node fleet — [PLAN-MULTINODE.md](./PLAN-MULTINODE.md) (N0 node registry
> shipped; N1+ pending a second node).
>
> **Backlog:** ACME webroot route — done (proxy serves `/.well-known/acme-challenge/`).
>
> **Multi-tenancy (T0–T1, in progress):** turaes is moving to org/project
> tenancy with role-based memberships (`owner`/`admin`/`developer`/`viewer`),
> scoped hashed API tokens, and a durable audit log. T0 lands the schema
> (migration `007_tenancy.sql` — users, organizations, memberships, api_tokens,
> audit_log, `applications.org_id`); T1 adds authorization middleware, scoped
> routes, tokens and audit APIs. Infrastructure (`servers`) stays platform-global.
>
> **SRE reliability work:** [SRE-REVIEW.md](./SRE-REVIEW.md) ·
> [PLAN-ZERO-DOWNTIME.md](./PLAN-ZERO-DOWNTIME.md). Done: control/edge
> SO_REUSEPORT rolling upgrades + graceful drain, and **app blue/green**
> (slot A/B, health-gated cutover, drain, legacy unslotted → slot migration,
> periodic route refresh so CLI deploys propagate). **Backlog:** exposure
> (Tailscale admin + Cloudflare Tunnel/Access), backups/DR, pre-migration
> snapshots, patch automation, resource limits, platform `/metrics` + alerting.

## Timeline

```
2026-10       2026-11                    2026-12
|--M0--|
   |----M1----|
              |----M2----|
                         |----M3----|
                                    |----M4----|
                                               |----M5----|
W0   W1  W2    W3  W4     W5  W6     W7  W8     W9  W10
```

- **M0 (W0, Oct 1–3)** — scaffold and contracts. ✅
- **M1 (W1–W2, Oct 4–17)** — the first real deploy: beruang via systemd.
- **M2 (W3–W4, Oct 18–31)** — traffic through Pingora with TLS.
- **M3 (W5–W6, Nov 1–14)** — the monitoring story: CPU / memory / visitors.
- **M4 (W7–W8, Nov 15–28)** — operational polish: rollback, env, live logs.
- **M5 (W9–W10, Nov 29–Dec 12)** — hands-off: webhooks and repo builds.

## First use case (M1 → M3)

Deploy `beruang` (Rust Axum, emits tonggeret metrics with visitors) on the VPS:

1. **M1** — register `beruang`, binary `/srv/beruang/target/release/beruang-gateway`,
   port `8000`, health `/health`, metrics `/metrics`; deploy under systemd; verify
   `systemctl status beruang` and `GET /health`.
2. **M2** — route a hostname (e.g. `beruang.turaes.rayakala.ink`) →
   `127.0.0.1:8000` through Pingora; load the certbot certificate; pass
   `CF-IPCountry` so visitors get real regions. (`kalkulator.rayakala.ink` stays
   on its existing server.)
3. **M3** — scrape `http://127.0.0.1:8000/metrics`; show CPU%, memory, RPS and
   total/unique visitors on the dashboard from live data.

## Dependency graph

```
M0 ─┬─▶ M1 ─┬─▶ M2 ──▶ M3 ──▶ M4 ──▶ M5
    │       └────────────┘
    └─────────────────────────▶ (monitor primitives ready since M0)
```

M2 and M3 both depend on M1 (you must be able to run an app before routing or
monitoring it). M4 depends on M1–M3; M5 depends on M4.

## Risks & dependencies

| Risk | Impact | Mitigation |
|---|---|---|
| Pingora is a framework, not a turnkey proxy | M2 scope | data plane is feature-gated; router/TLS logic is ready and tested in M0 |
| Pingora build needs clang + perl (OpenSSL) | M2 CI | install in CI; consider rustls backend |
| Pingora is Linux tier-1 | local dev on macOS | run the platform in OrbStack/a Linux VM; apps stay Docker-less |
| systemd requires root | M1 privileges | `proc` runtime is the non-root fallback |
| Visitor headers spoofable | data quality | document best-effort; rely on proxy-set headers |
| SQLite history growth | M3/monitor | 1-minute rollups + retention compaction (M3) |

See [MILESTONES.md](./MILESTONES.md) for per-milestone deliverables, tasks and
exit criteria.
