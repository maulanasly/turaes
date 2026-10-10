# Agent Workflow

turaes: a lightweight, self-hosted deployment platform ("Coolify but leaner")
that runs apps as **native processes** — no Docker, no containers. Inspired by
[`nusendra/ployer`](https://github.com/nusendra/ployer), with monitoring wired
to the fleet's [`tonggeret`](https://github.com/maulanasly/tonggeret) telemetry.

Rust (Axum + SQLite) + embedded zero-build UI. The proxy is
[Pingora](https://github.com/cloudflare/pingora); TLS certs come from certbot.

## Environment

- **Rust**: 1.85+ (workspace `rust-version`)
- **SQLite**: bundled via `sqlx`
- **Proxy**: Pingora (optional `pingora` feature; Linux for the data plane)
- **GitHub**: `gh` CLI authenticated as `maulanasly`
- No Python/Node toolchains required (frontend is zero-build)

## Repository

- **URL**: github.com/maulanasly/turaes
- **Main branch**: `main`
- **Workflow**: feature branch → tests → PR → merge

## Communication

- Always interact in English — chat, commits, PR titles/bodies, code comments.
- App UI strings and user-provided Indonesian domain terms stay as-is.

## AI guide rail (anti-slop)

- This repo's AI output filter is [anti-slop](https://github.com/miqdadbadjuber/anti-slop)
  (MIT): rules that reject generic AI-generated UI, filler copy, and AI-shaped
  code. It is a **filter, not a style guide** — direction stays here
  (`docs/UX-REVIEW.md`, palette tokens, zero-build UI) and in this file.
- Load it per task, not wholesale: `antislop` (always), `antislop-code` (Rust
  comments — keep what explains why, drop what restates the code),
  `antislop-ui` / `antislop-copywriting` / `antislop-human` /
  `antislop-layoutmobile` (dashboard static/ work only).
- Single-file fallback (no install): `curl -o antislop.md https://raw.githubusercontent.com/miqdadbadjuber/anti-slop/main/antislop.md`.
  Full install: `npx antislop-ai` (or `npx skills add miqdadbadjuber/anti-slop).
- Close every change with its Delivery Gate report (PASS/FAIL) alongside
  `make verify` in the PR body.

## Feature / Bug Fix Workflow (MANDATORY)

1. **Read context first** — run `graphify query "..."` (or read the docs below)
   before changing code.
2. **Branch off `main`**:
   ```bash
   git fetch origin && git switch main
   git switch -c feature/<name>   # or fix/<name>, chore/<name>, docs/<name>, test/<name>
   ```
3. **Implement + test** — write unit tests with the code; integration tests under
   `src/tests.rs` for the HTTP surface.
4. **Run `make verify`** — must be green before commit.
5. **Commit** using Conventional Commits (`feat:`, `fix:`, `test:`, `chore:`,
   `docs:`, `refactor:`, `perf:`).
6. **Push + PR to `main`**:
   ```bash
   git push -u origin feature/<name>
   gh pr create --base main --title "feat: ..." --body "..."
   ```

## Verification (the gate)

| Command | What |
|---|---|
| `make lint` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `make fmt-check` | `cargo fmt --all -- --check` |
| `make test-all` | `cargo test --workspace --all-targets` |
| `make certs-check` | `bash scripts/check-certs.sh` (TLS issuance fixture test) |
| `make verify` | all of the above — **run before every commit** |
| `make proxy-check` | type-check the Pingora data plane (Linux; `--features pingora`) |

## Structure

```
crates/turaes-core/     config, db + migrations, models, error, crypto
crates/turaes-runtime/  Runtime trait; systemd.rs, proc.rs, deploy.rs
crates/turaes-proxy/    router (ArcSwap), tls (certbot), pingora data plane
crates/turaes-monitor/  health, scrape (Prometheus), stats (cgroup/proc), rollup
.github/workflows/     ci.yml (gate), release.yml (build + deploy), configure.yml (OAuth secrets)
proto/control.proto     gRPC Control service (agent register/heartbeat)
src/                    CLI (serve/migrate/doctor/app/server/agent/edge), Axum app, OAuth, routes, gRPC, embedded UI
migrations/             SQLite schema (embedded via sqlx::migrate!)
static/                 zero-build UI compiled into the binary (rust-embed)
deploy/                 systemd unit, env template, install script
docs/                   ARCHITECTURE, ROADMAP, MILESTONES, API, DEPLOY
```

## Conventions

- **Errors**: JSON `{"detail": ...}` — 422 bad input · 401 unauth · 403 forbidden ·
  404 not found · 409 conflict · 500 internal (matches the fleet).
- **Dockerless**: never add a container runtime. Apps are prebuilt binaries
  supervised by systemd (default) or the embedded supervisor (`proc`).
- **Monitoring** reads tonggeret's Prometheus text: `http_requests_total`,
  `visitors_total{region}` (counter) and `unique_visitors_estimate{region}`
  (gauge). Never `sum()` the uniques gauge — take the latest per region.
- **CPU/memory** come from cgroup v2 (`system.slice/{app}.service`) and fall
  back to `/proc`; the proxy never samples resources itself.
- **Monitoring is local-only**: the control-plane monitor supervises apps with
  `server_id = 'local'`; remote apps are reconciled/reported by their node agent
  (cross-node metrics land in N2).
- **TLS**: certbot issues; turaes loads certs. Do not implement ACME in-process.
- **Secrets**: app env vars are AES-256-GCM sealed at rest; never log them.
- **Frontend**: zero-build, embedded; no npm/CDN.
- Update the graph after code changes: `graphify update .`

## Domain invariants

- An app's `name` is a lowercase slug and is the systemd unit + install name.
- `Runtime` is the only abstraction allowed to touch processes; handlers never
  spawn directly.
- The routing table is immutable behind `ArcSwap`; deploy rebuilds and publishes
  a new snapshot — never mutate in place.
- Visitor series are best-effort (headers are spoofable); never store raw
  identifiers.
