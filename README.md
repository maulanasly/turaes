# turaes

A lightweight, self-hosted deployment platform — **no Docker**. Deploy prebuilt
binaries from Git, route them by hostname through an in-process
[Pingora](https://github.com/cloudflare/pingora) proxy, and watch CPU, memory
and visitor counts on one dashboard.

> Think Coolify/Ployer, but leaner and container-less. Monitoring is wired to
> the fleet's [tonggeret](https://github.com/maulanasly/tonggeret) Prometheus
> telemetry.

## Why

The existing fleet (beruang, monthly-logs, …) already deploys as **native Rust
binaries behind systemd + a reverse proxy**. turaes turns that hand-rolled
pattern into a product: register an app, point at a binary, deploy, and get
health checks, host-based routing, TLS and metrics — without introducing a
container layer.

## Status

**M0 — scaffold.** The workspace, data model, runtime abstraction, proxy
routing, monitoring primitives and authenticated API all build and are tested.
The Pingora data plane and the live monitoring loop land in M2/M3. See
[docs/ROADMAP.md](./docs/ROADMAP.md) and [docs/MILESTONES.md](./docs/MILESTONES.md).

## Quickstart

```bash
make run     # serve on :8787 using config/default.toml
make dev     # AUTH_DISABLED=1 + debug logging (debug builds only)
make verify  # clippy + fmt + all tests (the gate)
```

```bash
curl -s localhost:8787/health
# {"proxy":false,"runtime":"systemd","status":"ok","version":"0.1.0"}
```

## How it works

```
Internet ─▶ Pingora (80/443) ─ host routing ─▶ 127.0.0.1:<app port>
                 │  TLS from certbot, graceful reload
                 └─ embedded in ─┐
turaes binary (Axum API + embedded UI + SQLite)
   ├── Runtime ── systemd (primary) | proc (embedded supervisor)
   ├── Monitor ── /metrics scrape + health + cgroup/proc CPU & memory
   └── Proxy   ── ArcSwap route table, host + *.base_domain
```

- **Runtime** — `Runtime` trait with `SystemdRuntime` (generated, hardened
  units; cgroup accounting; journald) and `ProcRuntime` (spawn + pid/log files,
  no root). Chosen per app.
- **Deploy** — the prebuilt binary is copied to `bin_dir`, an env file is
  written, the unit is (re)rendered, and the service is (re)started. Artifact
  SHA-256 is recorded for dedupe/rollback.
- **Monitoring** — health polls with thresholds; a Prometheus scrape of each
  app's `/metrics`; cgroup v2 (`memory.current`, `cpu.stat`) or `/proc`
  sampling; `visitors_total` / `unique_visitors_estimate` folded per region.
- **Proxy** — host header resolves to a loopback upstream; certbot certs are
  loaded from `/etc/letsencrypt/live`. No ACME in-process.

## Documentation

- [ARCHITECTURE.md](./docs/ARCHITECTURE.md) — components and data flow
- [ROADMAP.md](./docs/ROADMAP.md) — milestones and timeline
- [MILESTONES.md](./docs/MILESTONES.md) — detailed scope + exit criteria
- [API.md](./docs/API.md) — HTTP API
- [DEPLOY.md](./docs/DEPLOY.md) — VPS install and operations
- [AGENTS.md](./AGENTS.md) — contributor/agent workflow

## License

MIT — see [LICENSE](./LICENSE).
