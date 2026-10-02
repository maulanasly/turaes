#!/usr/bin/env bash
#
# Set GitHub OAuth credentials in /etc/turaes/turaes.env and restart turaes.
#
# Get your numeric id with:  gh api user -q .id
# Create an OAuth App at https://github.com/settings/developers with callback
# {APP_ORIGIN}/auth/callback, then copy the Client ID and a generated secret.
#
# Usage:
#   sudo bash deploy/set-github-oauth.sh \
#     --client-id <CLIENT_ID> --client-secret <CLIENT_SECRET> --allowed-ids 5284227
#
# Any flag may be omitted to leave that value unchanged.

set -euo pipefail

ENV_FILE="${TURAES_ENV_FILE:-/etc/turaes/turaes.env}"
CLIENT_ID=""
CLIENT_SECRET=""
ALLOWED=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --client-id) CLIENT_ID="${2:-}"; shift 2 ;;
    --client-secret) CLIENT_SECRET="${2:-}"; shift 2 ;;
    --allowed-ids) ALLOWED="${2:-}"; shift 2 ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
done

[[ -f "$ENV_FILE" ]] || { echo "error: $ENV_FILE not found" >&2; exit 1; }

# Replace or append a KEY=VALUE without sed delimiter pitfalls (secrets may
# contain '/', '&', etc.).
set_kv() {
  local key="$1" val="$2"
  [[ -z "$val" ]] && return 0
  python3 - "$ENV_FILE" "$key" "$val" <<'PY'
import sys
path, key, val = sys.argv[1], sys.argv[2], sys.argv[3]
lines = open(path).read().splitlines()
out, found = [], False
for line in lines:
    if line.startswith(key + "="):
        out.append(f"{key}={val}")
        found = True
    else:
        out.append(line)
if not found:
    out.append(f"{key}={val}")
open(path, "w").write("\n".join(out) + "\n")
PY
}

set_kv TURAES_GITHUB_CLIENT_ID "$CLIENT_ID"
set_kv TURAES_GITHUB_CLIENT_SECRET "$CLIENT_SECRET"
set_kv TURAES_ALLOWED_GITHUB_IDS "$ALLOWED"

chmod 0600 "$ENV_FILE"
systemctl restart turaes
sleep 1
systemctl is-active turaes
echo "updated GitHub OAuth config in $ENV_FILE (values redacted):"
grep -E '^TURAES_(GITHUB_CLIENT_ID|ALLOWED_GITHUB_IDS)=' "$ENV_FILE" | sed 's/=.*/=<set>/'
