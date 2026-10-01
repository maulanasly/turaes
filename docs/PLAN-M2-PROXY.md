# Plan — M2: Pingora proxy + TLS for `turaes.rayakala.ink`

**Goal:** serve the turaes dashboard at `https://turaes.rayakala.ink` and route
app hostnames to loopback ports, via the in-process Pingora proxy, with certs
issued by certbot. Docker-less throughout.

Status: **code merged; network verified; build moved to CI.** The Pingora data
plane compiles on Linux, inbound TCP 80/443 are reachable (Phase 0 ✅), and the
release binary is now produced by GitHub Actions (`release.yml`, Phase 1).
Remaining work: certs, config, and deploy.

---

## Phase 0 — Network access ✅ VERIFIED (2026-10-01)

The box `43.173.9.225` is a Tencent Cloud VM with **no host firewall** (`iptables`,
`nft`, `ufw` all empty). Reachability is controlled only by the **cloud security
group**.

Verified by binding temporary listeners on the host and connecting from the
public internet:

| Port | Needed for | State |
|---|---|---|
| 22 | SSH | open |
| 80 | Let's Encrypt HTTP-01 **and** HTTP→HTTPS redirect | ✅ **open** (raw payload received) |
| 443 | HTTPS dashboard | ✅ **open** (raw payload received) |
| 8787 | direct dashboard access (dev only) | blocked (control test) |
| DNS | `turaes.rayakala.ink` → `43.173.9.225`, 80/443 reachable | ✅ |

- [x] Inbound **TCP 80** confirmed from the internet (HTTP-01 will work).
- [x] Inbound **TCP 443** confirmed from the internet.
- [x] DNS resolves to this box.
- [ ] (Optional) allow **8787** if direct API access is wanted.

No owner action required — proceed to Phase 1.

---

## Phase 1 — Build in GitHub Actions (no Rust on the server)

The binary is built by CI, not on the VPS. `.github/workflows/release.yml`:

- **`build`** — on tags `v*` and manual dispatch: installs `clang perl
  pkg-config libssl-dev cmake`, runs `cargo build --release --features proxy`,
  strips, uploads the `turaes-linux-x86_64` artifact, and attaches the binary to
  a GitHub Release on tags.
- **`deploy`** — on manual dispatch with `deploy: true`: downloads the artifact
  and installs it over SSH (`install -m 0755` → `/usr/local/bin/turaes`,
  `systemctl restart turaes`).

`.github/workflows/ci.yml` now runs the proxy job as a **required** gate
(`cargo check -p turaes-proxy --features pingora`).

Required repository secrets (Settings → Secrets → Actions, or `gh secret set`):

| Secret | Value |
|---|---|
| `DEPLOY_HOST` | `43.173.9.225` |
| `DEPLOY_USER` | `root` |
| `DEPLOY_SSH_KEY` | private key whose public key is in `/root/.ssh/authorized_keys` |

- [ ] Workflows merged to `main`.
- [ ] Deploy secrets set.

Run it:

```bash
gh workflow run release.yml -f deploy=true     # build in CI + deploy to VPS
gh run watch                                    # follow the run
```

> The server no longer needs a Rust toolchain; it only runs the released
> binary. (The earlier server-side build is superseded.)

---

## Phase 2 — Certificate (certbot)

Issue **before** Pingora binds :80 (port must be free for standalone):

```bash
sudo systemctl stop turaes
sudo apt-get install -y certbot
sudo certbot certonly --standalone \
  -d turaes.rayakala.ink \
  --agree-tos -m ops@rayakala.ink --non-interactive
```

Result: `/etc/letsencrypt/live/turaes.rayakala.ink/{fullchain.pem,privkey.pem}`
(the exact layout `CertStore` expects).

**Renewals** (while Pingora owns :80): use certbot hooks so the port is free:

```bash
# /etc/letsencrypt/renewal-hooks/pre/10-stop-turaes.sh  -> systemctl stop turaes
# /etc/letsencrypt/renewal-hooks/post/10-start-turaes.sh -> systemctl start turaes
```

- [ ] Cert issued (or a temporary self-signed cert placed at the same path if
      Phase 0 is not done yet, so the proxy can start for local testing).
- [ ] Renewal hooks installed.

> Future enhancement: serve `/.well-known/acme-challenge/` from a webroot in
> Pingora's `request_filter` so renewals need no downtime.

---

## Phase 3 — Configuration

`/etc/turaes/turaes.env`:

```bash
TURAES_PROXY_ENABLED=true
TURAES_PUBLIC_URL=https://turaes.rayakala.ink   # dashboard_host derives from this
TURAES_APP_ORIGIN=https://turaes.rayakala.ink   # also fixes OAuth callback + Secure cookies
TURAES_BASE_DOMAIN=rayakala.ink                 # enables *.rayakala.ink routing
TURAES_CERT_DIR=/etc/letsencrypt/live
# TURAES_ALLOW_INSECURE_COOKIES no longer needed (https origin)
# Optional explicit: TURAES_PROXY_DASHBOARD_HOST=turaes.rayakala.ink
```

Also set GitHub OAuth so the dashboard is usable:

```bash
TURAES_GITHUB_CLIENT_ID=...
TURAES_GITHUB_CLIENT_SECRET=...
TURAES_ALLOWED_GITHUB_IDS=...
# GitHub OAuth App callback: https://turaes.rayakala.ink/auth/callback
```

- [ ] Env updated; `app_origin` is https (boot validation passes, Secure cookies on).

---

## Phase 4 — Start and route

```bash
sudo systemctl daemon-reload
sudo systemctl restart turaes
sudo turaes doctor          # expect proxy.enabled true, proxy.pingora true
```

Behavior already implemented:

- Pingora binds **:80** (`proxy.http_port`) and, when the dashboard cert exists,
  **:443** with HTTP/2 (`TlsSettings::intermediate` + `enable_h2`).
- `refresh_proxy_routes` builds the table from the DB on boot and after every
  deploy/delete:
  - dashboard host → `127.0.0.1:8787`
  - each app `domain` → `127.0.0.1:<port>`
  - wildcard `{app}.{base_domain}` resolution
- Unknown host → `404`.

- [ ] `turaes` active with proxy; `ss -tlnp` shows `:80` and `:443`.

---

## Phase 5 — Verify

```bash
# on the box
curl -s http://127.0.0.1/health
curl -sk --resolve turaes.rayakala.ink:443:127.0.0.1 https://turaes.rayakala.ink/health

# from the internet (after Phase 0)
curl -sI http://turaes.rayakala.ink/            # 200 or 301 -> https
curl -sI https://turaes.rayakala.ink/health     # 200, valid cert
```

Checklist:

- [ ] HTTP `:80` answers; HTTPS `:443` answers with a valid Let's Encrypt cert.
- [ ] Dashboard loads at `https://turaes.rayakala.ink` and OAuth login works.
- [ ] `curl -H 'Host: turaes.rayakala.ink' http://127.0.0.1/health` → ok.
- [ ] Unknown host returns 404.
- [ ] beruang (or another app) reachable via its domain once set.

> **Do not** point `kalkulator.rayakala.ink` at this box: that DNS already
> resolves to `43.173.12.145` (a different server). Give beruang-on-turaes a
> distinct hostname (e.g. `beruang.turaes.rayakala.ink` or a new domain).

---

## Phase 6 — Rollback

A broken proxy must never take down the app API:

```bash
# disable proxy only
sed -i 's/^TURAES_PROXY_ENABLED=.*/TURAES_PROXY_ENABLED=false/' /etc/turaes/turaes.env
sudo systemctl restart turaes      # API returns to :8787 only
```

Rebuild the previous binary from the last good commit and reinstall if needed.

---

## Design decisions & open questions

| Decision | Choice | Note |
|---|---|---|
| TLS backend | **OpenSSL** (system `libssl-dev`) | avoids BoringSSL `clang`/`perl`; rustls backend is experimental |
| Cert strategy | **certbot standalone + hooks** (MVP) | move to webroot-in-Pingora later for zero-downtime renewals |
| Multi-domain TLS | **single cert** (dashboard) for now | multi-cert SNI callback is the next step when apps get their own domains |
| HTTP→HTTPS | planned redirect | not yet wired in the proxy; add `request_filter` |
| Proxy process | **in-process, dedicated OS thread** | one binary; Pingora's `run_forever()` blocks and owns its runtime, so it must not run inside the Tokio runtime ("cannot start a runtime from within a runtime") |
| Proxy startup | route table from DB at boot + on deploy | `ArcSwap` publish, no locks on hot path |

### Next code steps (if not this pass)

1. HTTP→HTTPS redirect + ACME webroot route in `ProxyHttp::request_filter`.
2. Multi-cert SNI via `TlsSettings::with_callbacks` implementing `TlsAccept`,
   selecting `{cert_dir}/{sni}/{fullchain,privkey}.pem` (BoringSSL/OpenSSL only).
3. ~~CI proxy job + release binary~~ — done: `release.yml` builds with
   `--features proxy` and deploys; `ci.yml` proxy gate is required.

---

## Effort estimate

| Phase | Work | Estimate |
|---|---|---|
| 0 | open 80/443 | ✅ verified |
| 1 | CI build + artifact + deploy workflow | ~10 min + CI minutes |
| 2 | certbot issue + hooks | ~5 min |
| 3–4 | env + deploy/restart | ~5 min |
| 5 | verification | ~10 min |
| | **total hands-on** | **~30 min** + CI build time |

Phase 0 ✅ verified, so the public HTTPS path is unblocked.
