# turaes.yaml — declarative app manifests

One file declares one app. `turaes apply` creates or updates the application
row plus placement, health checks, domains, env and secrets — **config only**.
Deploying (the blue/green cutover) stays an explicit separate step
(`turaes app deploy`, the dashboard, or the API).

```bash
turaes apply -f turaes.yaml [--org acme] [--dry-run]   # -f defaults to ./turaes.yaml
```

## Kinds

| `kind` | What runs | Port | Health gate | Proxy route |
|---|---|---|---|---|
| `service` (default) | one long-lived process (binary or argv) | required | yes (2xx) | yes |
| `static` | `turaes serve-static` on a synced directory | required | yes (always 200) | yes |
| `worker` | supervised background process | none (omit or `0`) | no (clean start wins) | no |

Any language works: Go/Zig/C binaries, `deno compile` / `bun build --compile`
output, shebang scripts, or explicit interpreter argv. Nothing assumes Rust —
the health gate is a plain HTTP 2xx probe. Interpreters live on the host at
absolute paths (a venv unpacked under the app's state dir works); turaes
provisions no toolchains.

## Examples

Service from a binary (Go, Rust, C, compiled Deno/Bun):

```yaml
name: beruang
binary: /srv/beruang/target/release/beruang-gateway
port: 8000
domain: beruang.example.com
aliases: [www.example.com]
health:
  path: /health
  interval: 15s
resources:
  memory_mb: 512
env:
  LOG_LEVEL: info
```

Service from an interpreter (no shell involved — argv, not a command line):

```yaml
name: pyapi
command: [/opt/venv/bin/python, -m, gunicorn, -b, 127.0.0.1:8000, main:app]
workdir: /srv/pyapi
port: 8000
```

Static site (SPA fallback included):

```yaml
name: docs
kind: static
port: 8001
publish_dir: ./dist
domain: docs.example.com
```

Worker (no HTTP surface at all):

```yaml
name: mailer
kind: worker
command: [/opt/venv/bin/python, worker.py]
workdir: /srv/mailer
```

## Schema reference

| Field | Required | Notes |
|---|---|---|
| `name` | yes | lowercase slug `a-z0-9-`, ≤ 64 chars; the unit + install name |
| `binary` | service/binary only | absolute path to the prebuilt binary |
| `command` | service+command / worker | exec argv array (mutually exclusive with `binary` and `args`-style fields); `command[0]` must exist, no spaces (no shell) |
| `workdir` | no | `WorkingDirectory` override (defaults to the state dir) |
| `port` | service/static | loopback port; host-globally unique with its blue/green pair |
| `domain` / `aliases` | no | primary hostname + extra hostnames (service/static) |
| `kind` | no | `service` (default), `static`, `worker`; immutable after create |
| `runtime` | no | `systemd` (default) or `proc` |
| `server` | no | placement id or name (default `local`) |
| `auto_restart` | no | default `true` |
| `health` | no | `path` (default `/health`), `interval`/`timeout` (`15s`/`5m`/`1h`/bare seconds), `healthy_threshold` (2), `unhealthy_threshold` (3); service only |
| `metrics_path` | no | default `/metrics`; service only |
| `resources` | no | `memory_mb` 16–65536, `cpu_percent` 1–6400 (`systemd` only) |
| `publish_dir` | static only | source directory synced per deploy (relative paths anchor at the manifest file) |
| `env` | no | plaintext, committable environment |
| `secrets` | no | key names **only** — values are never written here |

Unknown fields are rejected so typos fail loudly with a line number.
Relative `binary`/`publish_dir`/`workdir` paths resolve against the manifest
file's directory. `kind` cannot change after creation (recreate the app).

The dashboard and the manifest share one launch model: the create wizard and
Settings expose the same prebuilt-binary / explicit-argv-`command` /
`workdir` options (with the same mutual-exclusion and tri-state rules), so a
`turaes.yaml` round-trip and a dashboard edit cannot disagree about what an
app runs.

## Secrets (names only, values out-of-band)

```bash
turaes secrets set myapp DATABASE_URL          # value as arg…
export DATABASE_URL=… && turaes secrets set myapp DATABASE_URL   # …or $KEY…
printenv DATABASE_URL | turaes secrets set myapp DATABASE_URL    # …or stdin
turaes secrets unset myapp OLD_KEY
turaes secrets list myapp                      # key names only, never values
```

Values are AES-256-GCM sealed at rest, indistinguishable from dashboard-set
env vars. Or declare self-minted values:

```yaml
secrets:
  - DATABASE_URL
  - key: INTERNAL_TOKEN
    generate: true
```

`apply` resolves every name **before writing anything** and aborts listing the
exact `secrets set` commands when a value is missing. First-time flow for a
new app: `apply` creates the shell, reports the missing secrets, you set them,
re-`apply` completes (re-running is idempotent). `generate: true` mints once
and never rotates — rotate with `secrets set` like any other value.

## Declarative semantics

Update re-applies the whole file: changed fields are patched (config takes
effect on the next deploy/restart), missing aliases and env keys are
**pruned**, `--dry-run` previews everything including prunes. Rollback keeps
working per kind: binaries via the artifact store, commands via fresh restart
cutover, static by cutting back to the surviving slot directory.

## Limits of v1

No on-host builds (prebuilt only; build-from-source stays M5 git strategy),
no `cron` schedules, no multi-process `web+worker` in one app (run two apps),
no remote-agent deploys for `static`/`command` apps (clear error, local only).
