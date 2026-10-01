# Plan — M2: Pingora proxy + TLS for `turaes.rayakala.ink`

**Goal:** serve the turaes dashboard at `https://turaes.rayakala.ink` and route
app hostnames to loopback ports, via the in-process Pingora proxy, with certs
issued by certbot. Docker-less throughout.

Status: **code written, not yet built/deployed on the server.** The data plane
compiles on Linux with `--features proxy` (verified via `cargo check`); the
remaining work is build, certs, config, and cloud-firewall access.

---

## Phase 0 — Unblock (owner action)

The box `43.173.9.225` is a Tencent Cloud VM with **no host firewall** (`iptables`,
`nft`, `ufw` all empty). Reachability is controlled only by the **cloud security
group**.

| Port | Needed for | State |
|---|---|---|
| 22 | SSH | open |
| 80 | Let's Encrypt HTTP-01 **and** HTTP→HTTPS redirect | open the SYNs — confirm |
| 443 | HTTPS dashboard | **must be opened** |
| 8787 | direct dashboard access (dev only) | optional; currently rejected |

- [ ] In the Tencent console, allow inbound **TCP 80** and **TCP 443** from
      `0.0.0.0/0` on this instance's security group.
- [ ] (Optional) allow **8787** for direct API access before DNS/TLS land.

> Why this matters: certbot's HTTP-01 challenge must be reachable from the
> public internet on port 80; users need 443. No host-side firewall changes
> are required.

---

## Phase 1 — Build with the proxy enabled

On the server (Linux; Pingora is Linux tier-1):

```bash
sudo apt-get install -y build-essential pkg-config libssl-dev cmake   # already present
cd /srv/turaes && . /root/.cargo/env
cargo build --release --features proxy        # or: make proxy-build
```

- Build deps: `libssl-dev` (OpenSSL backend links system OpenSSL), `cmake`
  (`libz-ng-sys`, pulled transitively). `clang`/`perl` only needed for the
  BoringSSL backend — not used here.
- Output: `/srv/turaes/target/release/turaes` (with Pingora compiled in;
  `turaes doctor` will show `proxy.pingora true`).

- [ ] Release binary built with `--features proxy`.

```bash
sudo install -m 0755 /srv/turaes/target/release/turaes /usr/local/bin/turaes
```

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
| Proxy process | **in-process** (`tokio::spawn`) alongside the API | one binary; restart on redeploy |
| Proxy startup | route table from DB at boot + on deploy | `ArcSwap` publish, no locks on hot path |

### Next code steps (if not this pass)

1. HTTP→HTTPS redirect + ACME webroot route in `ProxyHttp::request_filter`.
2. Multi-cert SNI via `TlsSettings::with_callbacks` implementing `TlsAccept`,
   selecting `{cert_dir}/{sni}/{fullchain,privkey}.pem` (BoringSSL/OpenSSL only).
3. CI: flip the `proxy` job from `continue-on-error` to required once this
   build is stable; publish a release binary so the server needs no Rust.

---

## Effort estimate

| Phase | Work | Estimate |
|---|---|---|
| 0 | open 80/443 (owner) | minutes |
| 1 | build `--features proxy` on server | ~5–10 min build |
| 2 | certbot issue + hooks | ~5 min |
| 3–4 | env + restart | ~5 min |
| 5 | verification | ~10 min |
| | **total hands-on** | **~30–40 min** + build time |

Blocked on **Phase 0** for public HTTPS; Phases 1–5 can proceed against
`127.0.0.1` with a temporary self-signed cert in the meantime.
