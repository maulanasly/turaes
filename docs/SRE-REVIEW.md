# turaes — SRE review

Principal-SRE review of turaes as deployed on `43.173.9.225`. Grounded in the
repo and the live box. Scope: reliability, security, operability, data
durability, change management, observability, capacity.

## Architecture & failure domains

Single VPS = **control plane + SQLite + artifact store + in-process Pingora
proxy (:80/:443) + monitor**, one root process. Managed apps are separate
systemd services on the same host. Multi-node is built but dormant
(`grpc.enabled false`).

- host loss ⇒ total outage + data loss (no backups)
- turaes crash ⇒ dashboard + proxied app hostnames down (apps keep running)
- turaes runs as **root and is internet-facing** (`systemd-analyze security` 9.4)

## Findings (evidence)

### Sev-1
- **No backups / no DR** — `/var/backups/turaes` empty; destructive migrations
  (`migrations/002_downsample.sql` `DROP TABLE`) run at boot.
- **Internet-exposed root service**, minimal sandboxing (`ProtectSystem=no`,
  `PrivateTmp=no`, 9.4 UNSAFE).
- **Deploy runs migrations with no pre-deploy snapshot, single-shot restart**, no
  rollback (`release.yml`).
- **(When enabled) plaintext gRPC** carrying join tokens + certificate private
  keys; no mTLS.

### Sev-2
- No automated security patching (84 pending; `unattended-upgrades` disabled).
- ~~No resource limits, no swap (`MemoryMax=infinity` on a 3.6 GB box)~~ — done: per-app `MemoryMax`/`CPUQuota`/`TasksMax`, per-org quotas.
- No platform `/metrics`/alerting; nothing pages.
- Proxy logs ERROR for scanner 404/TLS noise (~45% of recent lines); no rate limit.
- Not zero-downtime (`systemctl restart` drops :80/:443 + API).
- ~~Artifact store unbounded (no GC)~~ — done: monthly `turaes gc` + `--dry-run`.
- ~~Shared secret for JWT signing **and** env sealing; no rotation~~ — done: HKDF-separated sealing key, `turaes secrets reseal`, rotation runbook.
- ~~API/apps bind `0.0.0.0` (loopback suffices)~~ — done: control binds loopback by default, fronted by the proxy.

### Sev-3
- ~~Control unit not hardened~~ — done: `ProtectSystem=strict` + device/namespace
  lockdown (see `deploy/turaes.service`); irreversible migrations; root SSH login
  permitted (key only); cert renewal depends on turaes serving :80; no
  SLOs/runbook; journald uncapped; vendor `tat_agent` present.

## Remediation status

| Item | Status |
|---|---|
| Zero-downtime deploys (control/edge, app blue/green) | **in progress** — [PLAN-ZERO-DOWNTIME.md](./PLAN-ZERO-DOWNTIME.md) |
| Exposure reduction (Tailscale admin + Cloudflare Tunnel/Access, SG lockdown) | **backlog** |
| Backups/DR + pre-migration snapshot | **done (local)** — nightly timer + fail-closed boot snapshot + restore; offsite copies still backlog |
| Scheduler patching (unattended-upgrades), `/metrics`+alerting, log hygiene | backlog (secret separation, backups, quotas, artifact GC done) |

## Reliability lens

Availability rests on `Restart=always` + one node. MTTD ≈ ∞ (no alerts);
MTTR is manual with no backups/rollback. Adequate for a single-operator
platform; not for an availability target until the backlog above lands.
