# turaes — Ansible provisioning

Automates bringing a fresh Linux VPS (Ubuntu 22.04+/Debian 12+) to a **usable
turaes deployment platform**: base hardening, the turaes binary + config, the
systemd units and timers, TLS via certbot, and an optional first app.

What it cannot do (documented manual prerequisites, §Prerequisites):
creating the GitHub OAuth app, pointing DNS, and tagging a release with the
`turaes-linux-x86_64` asset (built by CI with `--features proxy`).

## Layout

```
ansible.cfg                  # inventory, ssh settings, yaml stdout
requirements.yml             # galaxy collections (community.general, ansible.posix)
requirements.txt             # controller python packages (ansible-core, ansible-lint, yamllint)
.ansible-lint                # lint profile + skips
.yamllint
inventory/hosts.yml          # target host(s); ssh key path only — no passwords
group_vars/all.yml           # domains, release tag, ports, first-app vars
group_vars/vault.yml.example # GitHub OAuth template (copy → encrypt as vault.yml)
playbooks/provision.yml      # ordered: base → turaes → tls → first-app (tags)
templates/turaes.env.j2      # /etc/turaes/turaes.env (0600)
roles/
  base/                      # apt, admin user, UFW, SSH hardening (default on)
  turaes/                    # release binary, env seed, systemd units/timers, health gate
  tls/                       # DNS assert → certbot webroot → renewal timer
  first_app/                 # declarative manifest → turaes apply → deploy
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

```bash
cd deploy/ansible
cp group_vars/vault.yml.example group_vars/vault.yml
ansible-vault edit group_vars/vault.yml          # OAuth client id/secret + allowed ids
$EDITOR group_vars/all.yml                        # domain, release tag, admin pubkey, first app
```

Secrets flow only through `group_vars/vault.yml` (gitignored, ansible-vault).
Never commit real OAuth values. The JWT secret is generated on the host at first
seed and stored only in the 0600 `/etc/turaes/turaes.env`.

## Run

```bash
make ansible-verify       # lint + syntax-check + --check dry run (needs vault)
make ansible-provision    # full provisioning run
make ansible-first-app    # add + deploy the first app only
```

`--check` connects to the host but changes nothing; run it before the real pass.
Tags let you compose: `--skip-tags first-app`, or run roles individually.

## Safety notes

- **SSH hardening is two-phase**: the admin key is installed and key login is
  proven (`wait_for_connection`) *before* `PasswordAuthentication no` is applied.
  If the play fails after an sshd reload, recover via the cloud/console serial
  console.
- **UFW ordering**: allow rules (22/80/443) are added before UFW is enabled, so
  the SSH session is never dropped. The cloud security group is the outer layer;
  UFW is defense-in-depth.
- **Env seeding is one-shot**: `/etc/turaes/turaes.env` is written only when
  absent, matching `deploy/install.sh`. Change OAuth later with
  `sudo bash deploy/set-github-oauth.sh …` or by editing the env file and
  restarting turaes.
- **TLS order**: the proxy must be up (it is — the turaes role restarts the
  service with `TURAES_PROXY_ENABLED=true`) before certbot runs, because the
  challenge path is served by Pingora.

## Upgrades

Bump `turaes_release_tag` (and sha256), re-run `make ansible-provision`. The
previous binary is kept at `/usr/local/bin/turaes.previous` for rollback.
