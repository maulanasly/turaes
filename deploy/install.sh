#!/usr/bin/env bash
#
# turaes installer — installs the binary, seeds /etc/turaes/turaes.env on first
# run, and manages the systemd service. Idempotent: re-run to upgrade.
#
#   sudo bash deploy/install.sh            # install/upgrade from ../target/release
#   BIN=/tmp/turaes sudo -E bash deploy/install.sh
#
# Assumes Linux + systemd. No Docker is required or used.

set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
BIN_DEST="${PREFIX}/bin/turaes"
CONFIG_DIR="/etc/turaes"
STATE_DIR="/var/lib/turaes"
ENV_FILE="${CONFIG_DIR}/turaes.env"
UNIT_DEST="/etc/systemd/system/turaes.service"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="${BIN:-${SCRIPT_DIR}/../target/release/turaes}"

log() { printf '\033[36m[turaes]\033[0m %s\n' "$*"; }
die() { printf '\033[31m[turaes] error:\033[0m %s\n' "$*" >&2; exit 1; }

[[ "$(uname -s)" == "Linux" ]] || die "this installer targets Linux"
[[ "${EUID}" -eq 0 ]] || die "run as root (sudo)"
command -v systemctl >/dev/null || die "systemd is required"

if [[ ! -x "${BIN}" ]]; then
  if command -v cargo >/dev/null; then
    log "no binary at ${BIN}; building release"
    (cd "${SCRIPT_DIR}/.." && cargo build --release)
  else
    die "no binary at ${BIN} and cargo not found; build first"
  fi
fi

log "installing ${BIN} -> ${BIN_DEST}"
install -D -m 0755 "${BIN}" "${BIN_DEST}"
mkdir -p "${CONFIG_DIR}" "${STATE_DIR}"

if [[ ! -f "${ENV_FILE}" ]]; then
  log "seeding ${ENV_FILE}"
  secret="$(openssl rand -hex 32)"
  sed "s|^TURAES_JWT_SECRET=.*|TURAES_JWT_SECRET=${secret}|" \
    "${SCRIPT_DIR}/turaes.env.example" > "${ENV_FILE}"
  chmod 0600 "${ENV_FILE}"
  log "edit ${ENV_FILE} to set GitHub OAuth credentials before signing in"
else
  log "keeping existing ${ENV_FILE}"
fi

install -m 0644 "${SCRIPT_DIR}/turaes.service" "${UNIT_DEST}"
install -m 0644 "${SCRIPT_DIR}/turaes-backup.service" /etc/systemd/system/turaes-backup.service
install -m 0644 "${SCRIPT_DIR}/turaes-backup.timer" /etc/systemd/system/turaes-backup.timer
install -m 0644 "${SCRIPT_DIR}/turaes-gc.service" /etc/systemd/system/turaes-gc.service
install -m 0644 "${SCRIPT_DIR}/turaes-gc.timer" /etc/systemd/system/turaes-gc.timer
systemctl daemon-reload
systemctl enable turaes
systemctl enable --now turaes-backup.timer
systemctl enable --now turaes-gc.timer
systemctl restart turaes

sleep 1
if systemctl is-active --quiet turaes; then
  log "turaes is running"
  systemctl status turaes --no-pager --lines=5 || true
else
  die "turaes failed to start; check: journalctl -u turaes -e"
fi
