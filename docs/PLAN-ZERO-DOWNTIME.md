# Plan — zero-downtime deploys

Make control, edge, and app releases happen without dropping traffic.

Status: **P3 + P4 implemented.** Control drains on SIGTERM; edge upgrades roll
across `turaes-edge@a/@b` via SO_REUSEPORT; app deploys are blue/green
(`active_port`) with a health gate and automatic fallback to the previous slot.

## Root causes

- One process binds :80/:443 **and** serves the API; `systemctl restart` drops
  traffic; `axum::serve` has no graceful shutdown.
- App units restart with a gap (old stopped before new is up).
- Migrations run at boot.

## Decisions

| Decision | Choice |
|---|---|
| Edge upgrade | **SO_REUSEPORT rolling** (two template instances) — Pingora's `add_tls_with_settings` accepts `Option<TcpSocketOptions>`, so this covers TLS too |
| Control API | **graceful shutdown** (drain on SIGTERM) |
| Slots | derived `port` / `port + runtime.slot_offset` + `applications.active_port` |
| Scope | **systemd local apps first**, `proc` best-effort, agent/remote deferred |
| Drain window | `runtime.drain_secs` (default 10s) |

## Phase 3 — control + edge

**3.1 Control graceful shutdown.** `axum::serve(...).with_graceful_shutdown()`
on SIGTERM/SIGINT; systemd `Restart=always` unaffected.

**3.2 Split roles.** Run the edge as its own service (`deploy/turaes-edge.service`)
and set the control's `TURAES_PROXY_ENABLED=false` in prod. Control restarts no
longer touch traffic; the edge serves its last routes/certs.

**3.3 Edge rolling replace (SO_REUSEPORT).** `proxy.reuse_port=true` makes the
edge bind :80/:443 with `SO_REUSEPORT`, so `deploy/turaes-edge@.service`
instances can coexist. Upgrade: install the new binary → start the idle
instance (`@b`) → confirm active → stop the previous instance (`@a`) → record
the active slot in `/var/lib/turaes/edge-active`. The kernel load-balances
across both during the overlap, so no request is dropped.

**3.4 Deploy.** Atomic install with a `.previous` copy; health-gate `/health`
(control) + a proxy probe; auto-rollback on failure (`scripts/` / `release.yml`).

## Phase 4 — app blue/green

**4.1 Model (migration 006).** `applications.active_port INTEGER NULL`; slot A =
`port`, slot B = `port + slot_offset`. Routes use `active_port.unwrap_or(port)`.

**4.2 Runtime.** `AppSpec.slot`; slot-scoped unit name (`{name}-a|b.service`),
env `PORT`, state dir, pid/log.

**4.3 Flow.** deploy → start **inactive** slot → health-gate on its port →
publish routes (`ArcSwap`) → after `drain_secs` stop the old slot (kept for
rollback). On failure keep the blue slot, mark failed.

**4.4 Rollback.** Flip `active_port` back, or reinstall the previous artifact and
cut over.

## Verification

- Unit tests for slot unit rendering + route selection.
- Local blue/green smoke: deploy v1 → active A; deploy v2 → B healthy →
  `active_port` flips, old stopped; failure keeps blue; traffic through the
  cutover has **zero failed requests**.
- `make verify`, then CI → `release.yml`.

## Backlog (deferred)

Exposure (Tailscale admin + Cloudflare Tunnel/Access), backups/DR,
pre-migration snapshot, patching, resource limits, `/metrics`+alerting.
