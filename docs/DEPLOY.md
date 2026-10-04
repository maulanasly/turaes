# Deploying turaes

turaes targets a single Linux VPS (Ubuntu 22.04+/Debian 12+) and runs the apps
it manages as **native processes**. Nothing about the managed apps uses Docker;
turaes itself is a single binary with an embedded UI.

> **Dev note (macOS):** the API, runtime (`proc`) and monitor primitives build and
> run on macOS, but the Pingora data plane is Linux tier-1. Run the proxy inside
> OrbStack/a Linux VM. See [ARCHITECTURE.md](./ARCHITECTURE.md).

## Live instance

| | |
|---|---|
| Host | `43.173.9.225` (Debian 13, systemd, 2 vCPU / 3.6 GB) |
| Dashboard | **https://turaes.rayakala.ink** (Pingora :80/:443, certbot TLS) |
| API (localhost) | `http://127.0.0.1:8787` (external 8787 blocked by the cloud SG) |
| Binary | `/usr/local/bin/turaes` — built in GitHub Actions (no Rust on the box) |
| Config | `/etc/turaes/turaes.env` (mode 0600) |
| Data | `/var/lib/turaes/turaes.db` |
| Cert | `/etc/letsencrypt/live/turaes.rayakala.ink/` (webroot renewals; proxy reloads on change) |
| App | `beruang` → `beruang.service`, loopback `:8000` |

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
(`TURAES_GITHUB_CLIENT_ID`, `SECRET`, `ALLOWED_GITHUB_IDS`) and are applied to
`/etc/turaes/turaes.env` by the `Configure` workflow (`.github/workflows/configure.yml`):

```bash
gh workflow run configure.yml      # writes env from secrets + restarts turaes
```

The OAuth App's **Authorization callback URL must be exactly**
`https://turaes.rayakala.ink/auth/callback`. Allowed ids are numeric GitHub
user ids (`gh api user -q .id`).

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
`DEPLOY_HOST`, `DEPLOY_USER`, `DEPLOY_SSH_KEY`.

Local build (optional, Linux; needs the same deps):

```bash
cargo build --release --features proxy      # or: make proxy-build
```

## 2. Install

```bash
sudo install -m 0755 target/release/turaes /usr/local/bin/turaes
sudo mkdir -p /etc/turaes /var/lib/turaes
sudo install -m 0644 deploy/turaes.service /etc/systemd/system/turaes.service
sudo install -m 0600 deploy/turaes.env.example /etc/turaes/turaes.env
sudo $EDITOR /etc/turaes/turaes.env      # set secrets (below)
sudo systemctl daemon-reload
sudo systemctl enable --now turaes
```

`deploy/install.sh` automates the above (download-or-build, generate JWT secret,
write the env file, enable the service).

## 3. Configuration

Layering, lowest → highest precedence: embedded defaults → file
(`TURAES_CONFIG`, default `/etc/turaes/turaes.toml`) → `TURAES_*` env vars.

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
`https://<your-host>/auth/callback` (plus `http://localhost:8787/auth/callback`
for dev). Find your numeric id with
`curl -H "Authorization: Bearer <token>" https://api.github.com/user`.

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

Use the **webroot** plugin — the proxy serves
`/.well-known/acme-challenge/<token>` from `proxy.acme_webroot`
(`TURAES_ACME_WEBROOT`, default `/var/lib/turaes/acme`), so no port needs to be
freed:

```bash
sudo mkdir -p /var/lib/turaes/acme
sudo certbot certonly --webroot -w /var/lib/turaes/acme \
  -d turaes.rayakala.ink --agree-tos -m ops@rayakala.ink --non-interactive
```

Renewal (`systemctl enable --now certbot.timer`) rewrites the files; the proxy
reloads a certificate automatically when its file mtime changes, and the edge
pulls updated certs from the control plane — **no restart and no downtime**.
(For multiple hosts, add a `[[webroot_map]]` entry per domain or pass `-d` per
cert.)

## 5. First app: beruang

```bash
# 1) build the app (on the server or upload the artifact)
cd /srv/beruang && cargo build --release

# 2) register + deploy via the API (session cookie required)
curl -X POST https://turaes.rayakala.ink/api/v1/apps \
  -H 'content-type: application/json' --cookie "$SESSION" \
  -d '{"name":"beruang",
       "binary_path":"/srv/beruang/target/release/beruang-gateway",
       "port":8000,"domain":"kalkulator.rayakala.ink",
       "health_path":"/health","metrics_path":"/metrics"}'

curl -X POST https://turaes.rayakala.ink/api/v1/apps/<id>/deploy --cookie "$SESSION"
```

Verify:

```bash
systemctl status beruang
curl -s 127.0.0.1:8000/health
curl -s 127.0.0.1:8000/metrics | head
```

The generated unit matches the fleet's hardened shape (`ProtectSystem=strict`,
`NoNewPrivileges`, `Restart=always`, `ReadWritePaths=/var/lib/beruang`).

## 6. Operations

```bash
systemctl status turaes
journalctl -u turaes -f          # dashboard/API logs
journalctl -u beruang -f         # an app's logs (systemd runtime)
turaes doctor                    # resolved config + capability report
turaes migrate                   # apply migrations (also runs on boot)
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

Until M4 ships one-click rollback, restore the previous binary path recorded in
the `deployments.previous_artifact` column and re-deploy.

## 7. Troubleshooting

| Symptom | Check |
|---|---|
| Dashboard 401 | GitHub id missing from the allowlist, or `APP_ORIGIN`/callback mismatch |
| Cookies not set | https origin required unless `TURAES_ALLOW_INSECURE_COOKIES=true` |
| Proxy not listening | built without `--features pingora` (`turaes doctor` shows `proxy.pingora`) |
| App won't start | `journalctl -u <app>`; verify `ExecStart` path and `PORT` |
| Visitors all `unknown` | proxy not forwarding a CDN country header |
| `database is locked` | another process holds the DB; SQLite WAL expects a single writer |
