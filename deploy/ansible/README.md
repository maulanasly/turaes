# turaes — Ansible provisioning

Automates bringing a fresh Linux VPS (Ubuntu 22.04+/Debian 12+) to a **usable
turaes deployment platform**: base hardening, the turaes binary + config, the
systemd units and timers, TLS via certbot, and an optional first app. Also
provisions **worker (agent) nodes** and **edge nodes** for a multi-node fleet
on a shared VPC.

What it cannot do (documented manual prerequisites, §Prerequisites):
creating the GitHub OAuth app, pointing DNS, and tagging a release with the
`turaes-linux-x86_64` asset (built by CI with `--features proxy`).

## Layout

```
ansible.cfg                  # inventory, ssh settings, default stdout callback
requirements.yml             # galaxy collections (community.general, community.crypto, ansible.posix)
requirements.txt             # controller python packages (ansible-core, ansible-lint)
.ansible-lint                # lint profile + skips
inventory/hosts.yml          # groups: control / workers / edges
playbooks/
  provision.yml              # control plane: base → turaes → tls → first-app (tags)
  agent.yml                  # worker prep: base (hardening) + agent (fleet key)
  join.yml                   # control: `turaes server add` + `bootstrap` workers
  edge.yml                   # edge node: base + turaes_binary + edge
  group_vars/                # ansible-playbook loads group vars next to the playbook:
    workers.yml              #   per-group files (workers, edges)
    edges.yml
    vault.yml.example        #   OAuth + join-token template
    all/                     #   the `all` group uses the subdirectory form:
      config.yml             #     shared configuration
      vault.yml              #     encrypted secrets (gitignored)
templates/turaes.env.j2      # /etc/turaes/turaes.env (0600)
roles/
  base/                      # apt, admin user, UFW (gated 80/443, VPC subnet), SSH hardening
  turaes_binary/             # shared: pinned release binary install (control + edge)
  turaes/                    # control: env seed, units/timers, fleet channel, health gate
  tls/                       # DNS assert → certbot webroot → renewal timer
  first_app/                 # declarative manifest → turaes apply → deploy
  agent/                     # worker prep for the control-plane SSH bootstrap
  edge/                      # edge.env + turaes-edge@a on :80/:443
```

## Prerequisites

Controller:

```bash
python3 -m pip install -r deploy/ansible/requirements.txt
cd deploy/ansible && ansible-galaxy collection install -r requirements.yml
```

Target VPS (manual, one-time):

1. **DNS**: `A` records for `turaes_domain` (and the first-app domain, if used)
   pointing at the host's public IP.
2. **GitHub OAuth app** at github.com/settings/developers with callback
   `https://<turaes_domain>/auth/callback`. Your numeric id: `gh api user -q .id`.
3. **Tagged release** with the `turaes-linux-x86_64` asset; copy its sha256 into
   `turaes_release_sha256`.

## Configure

The repository carries **no real host data or secrets** — the inventory and
vault are gitignored. Copy both examples and fill them in:

```bash
cd deploy/ansible
mkdir -p playbooks/group_vars/all
cp playbooks/group_vars/vault.yml.example playbooks/group_vars/all/vault.yml
cp inventory/hosts.example.yml inventory/hosts.yml
ansible-vault edit playbooks/group_vars/all/vault.yml  # OAuth id/secret, allowed ids, join token
$EDITOR inventory/hosts.yml               # real IPs, users, key paths; per-deployment
                                          # overrides (domain, cert email, control-plane
                                          # address, admin pubkey) go in host_vars here
$EDITOR playbooks/group_vars/all/config.yml  # only if you must change shared defaults
```

Secrets flow only through `playbooks/group_vars/all/vault.yml` (gitignored,
ansible-vault). Deployment-specific values (real domains, email, private IPs)
go in the gitignored `inventory/hosts.yml` host_vars or `host_vars/<host>.yml`
— they override the committed `playbooks/group_vars/all/config.yml` defaults.
Never commit real values. The JWT secret is generated on the host at first
seed and stored only in the 0600 `/etc/turaes/turaes.env`.

CI also runs a **gitleaks** secret scan (`.github/workflows/secrets.yml`) as a
backstop on every push/PR.

## Run

```bash
make ansible-verify       # lint + syntax-check + --check dry run (needs vault)
make ansible-provision    # control plane end-to-end
make ansible-agent        # prep worker nodes (SSH key + hardening)
make ansible-join         # control: register + bootstrap workers
make ansible-edge         # edge node (Pingora :80/:443)
make ansible-first-app    # add + deploy the first app only
```

`--check` connects to the host but changes nothing; run it before the real pass.
Tags let you compose: `--skip-tags first-app`, or run roles individually.

## Fleet topology

```
 Internet ─▶ edge (:80/:443) ─ VPC ─▶ workers (:app ports)      control :8787/:9443
   │             ▲  gRPC routes/certs (outbound)                    ▲
   └─────────────┴──────────────────────────────────────────────────┘
             agents/edges dial control; control SSHes workers (bootstrap)
```

- **control**: UFW `22/80/443` + `9443` (gRPC) and `8787` (artifact HTTP) from
  `turaes_vpc_subnet`. Set `turaes_control_address` (its private IP) and, so
  agents can fetch artifacts, `turaes_host` (e.g. `0.0.0.0` — keep the cloud SG
  scoped to the VPC).
- **worker**: UFW `22` + `turaes_vpc_subnet`. The **agent role generates the
  control-plane fleet SSH key** (`/etc/turaes/ssh/fleet-key`) and authorizes it
  for `turaes_node_ssh_user` (default `root` — the bootstrap script needs root
  writes and never uses sudo).
- **edge**: UFW `22/80/443`; `edge.env` + `turaes-edge@a` serve the fleet and
  pull routes/certs from the control plane with the same join token.

Run order for a new worker: `make ansible-agent` (prep), then `make ansible-join`
(register + bootstrap). `join.yml` is idempotent: it skips already-registered
names and re-bootstraps only nodes not `online`. Edges: `make ansible-edge`.

Note: the control plane must be provisioned **with** `turaes_grpc_enabled`,
`turaes_agent_join_token` and `turaes_control_address` for join/edge to work.
Env seeding is one-shot — an already-live single-host control plane needs those
vars added to `/etc/turaes/turaes.env` by hand before joining workers.

## Safety notes

- **SSH hardening is two-phase**: the admin key is installed and key login is
  proven (`wait_for_connection`) *before* `PasswordAuthentication no` is applied.
  If the play fails after an sshd reload, recover via the cloud/console serial
  console.
- **UFW ordering**: allow rules (22/80/443 + VPC) are added before UFW is
  enabled, so the SSH session is never dropped. The cloud security group is the
  outer layer; UFW is defense-in-depth.
- **Env seeding is one-shot**: `/etc/turaes/turaes.env` is written only when
  absent, matching `deploy/install.sh`. Change OAuth later with
  `sudo bash deploy/set-github-oauth.sh …` or by editing the env file and
  restarting turaes.
- **CLI env**: bare `turaes` CLI invocations on the control plane (apply,
  server add/bootstrap) read only `TURAES_*` env vars — the roles source
  `/etc/turaes/turaes.env` into the process environment so they hit the right
  database and sealing key.
- **TLS order**: the proxy must be up (it is — the turaes role restarts the
  service with `TURAES_PROXY_ENABLED=true`) before certbot runs, because the
  challenge path is served by Pingora.

## Upgrades

Bump `turaes_release_tag` (and sha256), re-run `make ansible-provision` (and
`make ansible-edge`). The previous binary is kept at `/usr/local/bin/turaes.previous`
for rollback.
