# Deploying turaes

turaes targets a single Linux VPS (Ubuntu 22.04+/Debian 12+) and runs the apps
it manages as **native processes**. Nothing about the managed apps uses Docker;
turaes itself is a single binary with an embedded UI.

> **Dev note (macOS):** the API, runtime (`proc`) and monitor primitives build and
> run on macOS, but the Pingora data plane is Linux tier-1. Run the proxy inside
> OrbStack/a Linux VM. See [ARCHITECTURE.md](./ARCHITECTURE.md).

## Live instance

> The repo never carries identifying host details. The current production
> host's IP, sizing and layout live in the **gitignored** ops sheet
> `docs/ops/live.md` (create it from the template below). Replace `<vps-ip>`
> below with that value.

| | |
|---|---|
| Host | `<vps-ip>` (Debian, systemd) — see `docs/ops/live.md` |
| Dashboard | **https://turaes.rayakala.ink** (Pingora :80/:443, certbot TLS) |
| API (localhost) | `http://127.0.0.1:8787` (external 8787 blocked by the cloud SG) |
| Binary | `/usr/local/bin/turaes` — built in GitHub Actions (no Rust on the box) |
| Config | `/etc/turaes/turaes.env` (mode 0600) |
| Data | `/var/lib/turaes/turaes.db` |
| Cert | `/etc/letsencrypt/live/turaes.rayakala.ink/` (webroot renewals; proxy reloads on change) |
| App | `beruang` → `beruang.service`, loopback `:8000` |

Ops sheet template (`docs/ops/live.md`, not committed):

```markdown
# Live instance (ops-only — do not commit)

- Host: <vps-ip> (Debian 13, systemd, 2 vCPU / 3.6 GB)
- Dashboard: https://turaes.rayakala.ink
- SSH: <user>@<vps-ip> (key-only)
```

```bash
sudo systemctl status turaes beruang
turaes app list
turaes app show beruang        # status + cpu/mem/visitors/health
curl -sI https://turaes.rayakala.ink/health
```

**Deploy/update the platform:**

```bash
gh workflow run release.yml -f deploy=true     # CI builds --features proxy + deploys
```

**GitHub OAuth:** configured. Credentials live as repo secrets
(`TURAES_GITHUB_CLIENT_ID`, `TURAES_GITHUB_CLIENT_SECRET`,
`TURAES_ALLOWED_GITHUB_IDS`) and are applied to `/etc/turaes/turaes.env` by
the `Configure` workflow (`.github/workflows/configure.yml`):

```bash
gh workflow run configure.yml      # writes env from secrets + restarts turaes
```

(The workflow also accepts the legacy aliases `SECRET` and
`ALLOWED_GITHUB_IDS`; prefer the `TURAES_*` names for new setups, or run
`sudo bash deploy/set-github-oauth.sh --client-id … --client-secret … --allowed-ids …`
directly on the box.)

The OAuth App's **Authorization callback URL must be exactly**
`https://turaes.rayakala.ink/auth/callback`. Allowed ids are numeric GitHub
user ids (`gh api user -q .id`). Set `TURAES_APP_ORIGIN` to the dashboard
origin **before** signing in — the callback URL is derived from it, and
session cookies are bound to it (mismatches fail boot in release, or silently
drop cookies).

**Outstanding on this host**

- HTTP (`:80`) serves the dashboard directly; an HTTP→HTTPS redirect is a planned
  proxy enhancement.
- beruang is localhost-only until given its own hostname (do **not** use
  `kalkulator.rayakala.ink`; it points at another server).
- Edit `/etc/turaes/turaes.env` then `sudo systemctl restart turaes`.

## 1. Build (in GitHub Actions)

The binary is built by CI — the server does **not** need a Rust toolchain.

```bash
gh workflow run release.yml -f deploy=true   # build with --features proxy + deploy
gh run watch
```

`.github/workflows/release.yml` builds `turaes` on Ubuntu with `clang perl
pkg-config libssl-dev cmake`, uploads the `turaes-linux-x86_64` artifact,
attaches it to a GitHub Release on tags, and (when `deploy: true`) installs it
on the VPS over SSH and restarts the service. Required secrets:
`DEPLOY_HOST`, `DEPLOY_USER`, `DEPLOY_SSH_KEY`. The deploy job only runs for
the `production` environment. Its post-deploy gate checks
`http://127.0.0.1:8787/health` only — confirm the proxy route and app health
separately (see §5).

Local build (optional, Linux; needs the same deps):

```bash
cargo build --release --features proxy      # or: make proxy-build
```

## 2. Install

> **Automated option:** `deploy/ansible/` provisions a fresh VPS end-to-end
> (base hardening + UFW/SSH, binary install, env seed, systemd units/timers,
> certbot TLS, optional first app). See `deploy/ansible/README.md` — manual
> prerequisites are DNS pointing at the host, a GitHub OAuth app, and a tagged
> release with the `turaes-linux-x86_64` asset. Run with `make ansible-provision`.

Prefer `deploy/install.sh` — it installs the binary, generates the JWT secret,
seeds the env file, and enables the service **and** the backup/GC timers:

```bash
sudo bash deploy/install.sh            # from the repo root (builds if needed)
```

Manual equivalent (the script does all of this, plus the timers):

```bash
sudo install -m 0755 target/release/turaes /usr/local/bin/turaes
sudo mkdir -p /etc/turaes /var/lib/turaes
sudo install -m 0644 deploy/turaes.service /etc/systemd/system/turaes.service
sudo install -m 0600 deploy/turaes.env.example /etc/turaes/turaes.env
sudo $EDITOR /etc/turaes/turaes.env      # set secrets (below)
sudo systemctl daemon-reload
sudo systemctl enable --now turaes
```

`install.sh` does not cover OAuth, DNS or TLS — continue below. Never copy
the example env verbatim in production: its placeholder secret fails the
release boot gates (the script generates a real one).

## 3. Configuration

Layering, lowest → highest precedence: embedded defaults → file
(`--config` / `TURAES_CONFIG` when set; no path is probed by default, so pass
one explicitly or rely on env) → `TURAES_*` env vars. In production the env
file (`/etc/turaes/turaes.env`, loaded via the unit's `EnvironmentFile`) is
the configuration.

### Required secrets (`/etc/turaes/turaes.env`)

```bash
TURAES_JWT_SECRET=$(openssl rand -hex 32)
TURAES_GITHUB_CLIENT_ID=...
TURAES_GITHUB_CLIENT_SECRET=...
TURAES_ALLOWED_GITHUB_IDS=12345678     # numeric ids, comma-separated
TURAES_APP_ORIGIN=https://turaes.rayakala.ink
TURAES_DATABASE_URL=sqlite:///var/lib/turaes/turaes.db?mode=rwc
```

Register the GitHub OAuth app with callback
`https://<your-host>/auth/callback` (GitHub allows several callback URLs, so
also add `http://localhost:8787/auth/callback` for dev). Find your numeric id
with `gh api user -q .id`.

### Proxy

```bash
TURAES_PROXY_ENABLED=true
TURAES_PROXY_HTTP_PORT=80
TURAES_PROXY_HTTPS_PORT=443
TURAES_CERT_DIR=/etc/letsencrypt/live
```

Pingora owns 80/443. If nginx already holds those ports (as on the current
fleet), retire the app vhosts there — turaes replaces them — or front Pingora
with nginx during migration.

### Runtime

```bash
TURAES_RUNTIME_DRIVER=systemd      # or proc
TURAES_UNIT_DIR=/etc/systemd/system
TURAES_BIN_DIR=/usr/local/bin
TURAES_STATE_DIR=/var/lib
TURAES_ENV_DIR=/etc
```

Use `proc` to run turaes as a non-root user (no systemd unit generation).

## 4. TLS with certbot

turaes does **not** do ACME itself; it loads certs issued by certbot into the
standard layout `{CERT_DIR}/{domain}/{fullchain.pem,privkey.pem}`.

Order matters, because the proxy is **off** by default (`TURAES_PROXY_ENABLED`
ships `false`): point DNS at the host first, then enable the proxy and restart
turaes, *then* issue the cert — the challenge path only exists once the proxy
serves it.

Use the **webroot** plugin — the proxy serves
`/.well-known/acme-challenge/<token>` from `proxy.acme_webroot`
(`TURAES_ACME_WEBROOT`, default `/var/lib/turaes/acme`), so no port needs to be
freed:

```bash
sudo mkdir -p /var/lib/turaes/acme
sudo certbot certonly --webroot -w /var/lib/turaes/acme \
  -d turaes.rayakala.ink --agree-tos -m <ops-email> --non-interactive
```

Renewal (`systemctl enable --now certbot.timer`) rewrites the files; the proxy
reloads a certificate automatically when its file mtime changes, and the edge
pulls updated certs from the control plane — **no restart and no downtime**.
(For multiple hosts, add a `[[webroot_map]]` entry per domain or pass `-d` per
cert.)

### Automatic issuance for app domains

`install.sh` enables the `turaes-certs.timer` (weekly), which runs
`deploy/ensure-app-certs.sh`: it reads every hostname from the turaes database
(`applications.domain` + the `domains` table) and issues any missing cert via
the same webroot flow. Existing valid certs are skipped; renewals stay with
`certbot.timer`. Set the contact address once in `/etc/turaes/turaes.env`
(otherwise new domains fail loudly instead of issuing):

```bash
TURAES_CERT_EMAIL=ops@example.com
```

Manual run (also useful with `DRY_RUN=1` to validate against staging first):

```bash
sudo /usr/local/bin/ensure-app-certs.sh
sudo DRY_RUN=1 /usr/local/bin/ensure-app-certs.sh   # staging only, issues nothing
```

## 5. First app: beruang

```bash
# 1) build the app (on the server or upload the artifact)
cd /srv/beruang && cargo build --release
```

Pick one of three ways to register and deploy. **No OAuth needed:** the CLI talks
to the local database directly (fastest for bootstrap):

```bash
turaes app add --name beruang \
  --binary /srv/beruang/target/release/beruang-gateway \
  --port 8000 --domain beruang.turaes.rayakala.ink
turaes app deploy beruang
```

Or declaratively from a file (see [MANIFEST.md](./MANIFEST.md) — any language,
plus static sites and workers):

```bash
turaes apply -f turaes.yaml
turaes app deploy beruang
```

**Via the API** (session cookie or API token required). Sign in to the
dashboard once in a browser first — the first user to sign in becomes `owner`
of the seeded `default` organization. Then either export the session cookie
from your browser's devtools as `$SESSION`, or mint a token headlessly:

```bash
# headless auth: mint a deploy-scoped token (paste a session cookie once,
# or do this in the Tokens dashboard page and skip the cookie entirely)
curl -X POST https://turaes.rayakala.ink/api/v1/orgs/default/tokens \
  -H 'content-type: application/json' --cookie "$SESSION" \
  -d '{"name":"bootstrap","scopes":"deploy"}'
# → { "plaintext": "turaes_…" } — shown once, store it as $TOKEN
```

```bash
curl -X POST https://turaes.rayakala.ink/api/v1/orgs/default/apps \
  -H 'content-type: application/json' -H "Authorization: Bearer $TOKEN" \
  -d '{"name":"beruang",
       "binary_path":"/srv/beruang/target/release/beruang-gateway",
       "port":8000,"domain":"beruang.turaes.rayakala.ink",
       "health_path":"/health","metrics_path":"/metrics"}'

curl -X POST https://turaes.rayakala.ink/api/v1/orgs/default/apps/<id>/deploy \
  -H "Authorization: Bearer $TOKEN"
```

Notes:

- Loopback ports are host-global: a port (or its blue/green pair) claimed by
  any app yields `409`. Point the domain's DNS at this host *before* adding
  it, or the proxy returns `404` for unknown hosts.
- Optional fields: `server_id` (default `local`), `runtime` (`systemd` default,
  `proc` fallback), `mem_limit_mb` / `cpu_quota_pct` (systemd only), `args`,
  `description`, `auto_restart`.
- Deploys are blue/green with a health gate: the new slot must answer its
  health path before traffic cuts over.

Verify:

```bash
systemctl status beruang-a beruang-b   # slots; one is active
turaes app show beruang                # status + cpu/mem/visitors/health
curl -s 127.0.0.1:8000/health         # base port (slot A when active)
curl -sk https://beruang.turaes.rayakala.ink/health
```

The generated unit matches the fleet's hardened shape (`ProtectSystem=strict`,
`NoNewPrivileges`, `Restart=always`, `ReadWritePaths=/var/lib/beruang`).

## 6. Operations

```bash
systemctl status turaes
journalctl -u turaes -f          # dashboard/API logs
journalctl -u beruang -f         # an app's logs (systemd runtime)
turaes doctor                    # resolved config + capability report (exits non-zero on failure)
turaes doctor --json             # same report as JSON (for monitoring/scripts)
turaes migrate                   # apply migrations (also runs on boot)
```

Local app commands talk to the database directly (no OAuth) and take `--org`
to scope by organization (default `default`; names stay globally unique):

```bash
turaes app list [--org acme] [--json]
turaes app show beruang [--json]
turaes app remove beruang        # stops its units, then deletes it
```

### Backup

Snapshots are automatic: `turaes-backup.timer` takes a nightly snapshot, and
every boot takes a `pre-migration-*.db` snapshot *before* migrations run (boot
aborts if the snapshot fails). Only the newest `TURAES_BACKUP_RETAIN` (default
14) snapshots are kept, in `TURAES_BACKUP_DIR` (default
`/var/lib/turaes/backups`):

```bash
turaes backup                    # snapshot now (+ prune)
turaes doctor                    # shows newest snapshot + age
tar czf /var/backups/turaes-state-$(date +%F).tgz /var/lib/turaes  # state root
```

Copy snapshots off the box for real DR (a host loss still takes everything
with it). To restore:

```bash
systemctl stop turaes
turaes restore /var/lib/turaes/backups/turaes-<timestamp>.db --force
systemctl start turaes
```

### Rotate the platform secret

`TURAES_JWT_SECRET` signs sessions *and* derives the at-rest sealing key
(HKDF-SHA256, domain-separated). Rotate it without losing stored env vars:

```bash
# 1. Set the new secret, keep the old one for decryption.
#    In /etc/turaes/turaes.env:
#      TURAES_JWT_SECRET=<new openssl rand -hex 32>
#      TURAES_SECRET_PREVIOUS=<old secret>
systemctl restart turaes
# 2. Re-seal everything with the new primary (migrates legacy blobs too).
turaes secrets reseal
# 3. Verify: deploy an app, confirm env-dependent behavior, check
#    `turaes doctor` no longer reports a staged rotation.
# 4. Remove TURAES_SECRET_PREVIOUS from the env file and restart.
```

All sessions invalidate on rotation (users sign in again) — that is expected.

### Artifact garbage collection

Every deploy stores the binary in the content-addressed store, so it grows
forever without collection. `turaes-gc.timer` runs monthly; anything not
named by a deployment row is unreachable (rollback can never address it) and
is deleted, along with stale crashed uploads. Files younger than an hour are
always spared (a deploy stores the blob before writing its row):

```bash
turaes gc --dry-run   # report reclaimable blobs without deleting
turaes gc             # collect (audited as `artifacts.gc`)
turaes doctor          # shows store file count + size
```

### Perimeter

The control plane binds loopback by default (`server.host`, override with
`TURAES_HOST=0.0.0.0` only behind other access controls) and is exposed through
the proxy's dashboard route. Every response carries `nosniff` / `DENY` /
same-origin-referrer headers (+HSTS on https origins); cookie-authed mutations
require a JSON content type (CSRF); `/auth/*` is capped at 60 req/min and
mutating `/api/*` at 600 req/min globally (`429` + `Retry-After`); handlers
time out at 120s. The systemd unit confines the filesystem to
`/var/lib/turaes`, `/etc` and `/usr/local/bin` with device/namespace lockdown
— extend `ReadWritePaths` if the `[runtime]` dirs move.

### Upgrade

```bash
sudo install -m 0755 target/release/turaes /usr/local/bin/turaes
sudo systemctl restart turaes      # migrations run on boot
```

### Rollback an app

```bash
turaes app rollback beruang                    # previous build
curl -X POST https://turaes.rayakala.ink/api/v1/orgs/default/apps/<id>/rollback \
  -H "Authorization: Bearer $TOKEN"            # previous build
curl -X POST https://turaes.rayakala.ink/api/v1/orgs/default/apps/<id>/rollback \
  -H 'content-type: application/json' -H "Authorization: Bearer $TOKEN" \
  -d '{"artifact_hash":"sha256:<hex>"}'        # a specific build (see Deployments tab)
```

Rollback replays the stored artifact through the same blue/green,
health-gated path as a deploy. It needs at least two distinct builds in
history, otherwise it answers `422`.

## 7. Troubleshooting

| Symptom | Check |
|---|---|
| Dashboard 401 | GitHub id missing from the allowlist, or `APP_ORIGIN`/callback mismatch |
| Refuses to boot: placeholder secret | release builds reject the shipped default — set a real `TURAES_JWT_SECRET` (`openssl rand -hex 32`) |
| Refuses to boot: empty allowlist | release builds require `TURAES_ALLOWED_GITHUB_IDS`, or explicitly opt into open sign-in with `TURAES_ALLOW_OPEN_SIGNIN=true` |
| Cookies not set | https origin required unless `TURAES_ALLOW_INSECURE_COOKIES=true`; `TURAES_APP_ORIGIN` must match the URL in the browser |
| Proxy not listening | built without `--features proxy` (`turaes doctor` shows `proxy.pingora`); proxy off by default — set `TURAES_PROXY_ENABLED=true` and restart before certbot |
| App won't start | `journalctl -u <app>`; verify `ExecStart` path and `PORT` |
| API `409` on create | port (or its blue/green pair) already claimed by another app; pick a free loopback port |
| API `422` on rollback | needs at least two distinct builds in history |
| Visitors all `unknown` | proxy not forwarding a CDN country header |
| `database is locked` | another process holds the DB; SQLite WAL expects a single writer |
| Config change ignored | unparseable `TURAES_*` values are only a startup warning, never fatal — check `journalctl -u turaes` for `ignoring unparseable` |
