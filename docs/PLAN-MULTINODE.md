# Plan — Multi-node fleet (control plane + agents + edge)

Status: **planned.** N0 (node registry) is implemented; N1+ are not. No second
node exists yet, so this is the design of record.

## Decisions (locked)

| Decision | Choice |
|---|---|
| First goal | **Fleet management** — one dashboard, many nodes |
| Network | **Same Tencent VPC** — private IPs between edge/control/workers |
| Transport | **Hybrid**: SSH for bootstrap, **gRPC (tonic)** for steady state |
| Proxy | **Separate edge host** running Pingora; control plane has no :80/:443 |

## Roles

| Role | Host | Responsibility |
|---|---|---|
| **control** | VM | Axum API + UI, SQLite, ArtifactStore, monitor, gRPC server |
| **agent** | worker VM(s) | reconcile assigned apps; push cgroup CPU/mem, health, visitors, logs |
| **edge** | VM | Pingora only: TLS/SNI + routing to worker private IPs (no DB) |

The edge is a **stateless subscriber**: the control plane streams the route table
and certificate material to it. This keeps the edge thin and restartable, at the
cost of a route/cert sync protocol.

```
                         Internet
                            │ 80/443 (TLS/SNI)
              ┌─────────────▼──────────────┐
              │ edge (Pingora)              │  host → worker private IP:port
              └───────┬────────────┬────────┘
            VPC 10.x  │            │  VPC 10.x
              ┌───────▼───┐   ┌────▼──────┐
              │ worker A   │   │ worker B  │  turaes-agent + systemd apps
              └────────────┘   └───────────┘
                     ▲  gRPC (mTLS, outbound)
         ┌───────────┴───────────────┐
         │ control plane              │  API · UI · SQLite · ArtifactStore · monitor
         └────────────────────────────┘
```

## gRPC surface (tonic + prost, rustls; mTLS for agent/edge)

```
service Control {
  rpc Register(RegisterRequest)        returns (AgentCredentials);   // join token → bound token
  rpc AgentSession(stream AgentMsg)    returns (stream ControlMsg);  // desired state ↔ heartbeat/metrics/logs
  rpc EdgeSession(stream EdgeMsg)      returns (stream ControlMsg);  // routes + cert bundles ↔ edge health
  rpc FetchArtifact(ArtifactRequest)   returns (stream Chunk);       // content-addressed binary
}
```

Build note: `protoc` is required (`apt install protobuf-compiler`) or use
`protoc-bin-vendored`.

## Data model (control plane)

- `servers(id, name, address, ssh_host, ssh_port, ssh_user, ssh_key_enc, is_local,
  status, last_seen_at, agent_version, created_at)` — **N0 done**.
- `applications.server_id` — **N0 done**.
- later: `server_id` on `app_metrics`/`visit_metrics`/`events`; `artifacts(hash, size, created_at)`.
- Agent/edge credentials sealed with the existing AES-256-GCM key.

## Artifact distribution

Content-addressed store on the control plane: `/var/lib/turaes/artifacts/sha256/{hash}`.
Deploys reference `artifact_ref` (hash); nodes pull via `FetchArtifact` (or SSH
during bootstrap). Rollback = redeploy a previous hash. `AppSpec` splits
`artifact_ref` from per-node `installed_path`.

## API additions

- `GET/POST /api/v1/servers`, `GET/DELETE /api/v1/servers/{id}`,
  `POST /api/v1/servers/{id}/validate` — **N0 done**.
- `POST /api/v1/apps` accepts `server_id` — **N0 done** (deploy to non-local is
  rejected until N1).
- later: `/agent/*` gRPC + artifact fetch.

## Phases

| Phase | Scope | Exit |
|---|---|---|
| **N0** ✅ | `servers` + `applications.server_id`, `/api/v1/servers`, `turaes server …`, UI panel | local node auto-registered; no regression |
| **N1** | ArtifactStore + SSH bootstrap + `turaes agent` over gRPC + `AgentRuntime` | deploy an app to a worker |
| **N2** | agent pushes cgroup CPU/mem; control plane scrapes health/`/metrics` over VPC; per-node dashboard | worker shows CPU/mem/health/visitors |
| **N3** | `turaes edge` role + `EdgeSession` route/cert stream; move Pingora off control | edge serves all apps; control has no :80/:443 |
| **N4** | DNS-01 multi-cert/wildcard SNI + replicas + Pingora LB + control-plane HA | TLS per app; N replicas behind edge |

## Risks

- Wildcard TLS needs certbot **DNS-01** (Tencent DNS creds); dynamic SNI is
  OpenSSL/BoringSSL-only.
- Split edge adds a route/cert sync protocol and a second host to secure.
- tonic adds `protoc` and a second HTTP/2 stack to the build.
- Agent auth: join token → bound token (rotate), sha256 integrity, mTLS.
- SQLite stays single-writer on control — fine at fleet scale.
