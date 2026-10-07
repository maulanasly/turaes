#!/usr/bin/env bash
# check-certs.sh — fixture test for deploy/ensure-app-certs.sh.
# Builds a temp sqlite DB (apps + alias domains), a cert dir holding one
# valid self-signed cert, and a stub certbot that records its invocations.
# Asserts: valid certs skipped, missing public domains issued, local names
# filtered, missing email fails. Needs: bash, sqlite3, openssl.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

command -v sqlite3 >/dev/null || { echo "check-certs: sqlite3 not found" >&2; exit 2; }
command -v openssl >/dev/null || { echo "check-certs: openssl not found" >&2; exit 2; }

export TURAES_DB="$WORK/turaes.db"
export TURAES_CERT_DIR="$WORK/live"
export TURAES_ACME_WEBROOT="$WORK/acme"
export TURAES_CERT_EMAIL="ops@example.com"
export DRY_RUN=0
mkdir -p "$TURAES_CERT_DIR" "$WORK/bin"
cat >"$WORK/bin/certbot" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$STUB_LOG"
exit 0
EOF
chmod +x "$WORK/bin/certbot"
export STUB_LOG="$WORK/calls.log"
export PATH="$WORK/bin:$PATH"

sqlite3 "$TURAES_DB" \
    "CREATE TABLE applications (id TEXT PRIMARY KEY, domain TEXT);
     CREATE TABLE domains (id TEXT PRIMARY KEY, application_id TEXT, domain TEXT NOT NULL);
     INSERT INTO applications VALUES ('a1','ok.example.com'),('a2','missing.example.com'),('a3','localhost'),('a4',NULL);
     INSERT INTO domains VALUES ('d1','a1','alias.example.com'),('d2','a1','thing.local');"

# Self-signed, currently valid cert for ok.example.com only.
mkdir -p "$TURAES_CERT_DIR/ok.example.com"
openssl req -x509 -newkey rsa:2048 -keyout "$WORK/key.pem" \
    -out "$TURAES_CERT_DIR/ok.example.com/fullchain.pem" \
    -days 30 -nodes -subj "/CN=ok.example.com" >/dev/null 2>&1

out="$(bash "$SCRIPT_DIR/deploy/ensure-app-certs.sh")"
echo "$out"

grep -q "OK (valid cert): ok.example.com" <<<"$out"
grep -q "SKIP (not a public name): localhost" <<<"$out"
grep -q "SKIP (not a public name): thing.local" <<<"$out"
grep -q "ISSUED: missing.example.com" <<<"$out"
grep -q "ISSUED: alias.example.com" <<<"$out"
grep -q "issued=2 skipped=3 failed=0" <<<"$out"
# Stub saw exactly the two missing public domains, nothing else.
grep -c "^" "$STUB_LOG" | grep -q "^2$"
grep -q "\-d missing.example.com" "$STUB_LOG"
grep -q "\-d alias.example.com" "$STUB_LOG"
! grep -q "ok.example.com" "$STUB_LOG"
! grep -q "localhost" "$STUB_LOG"

# Missing email with a missing domain must fail loudly.
: >"$STUB_LOG"
if TURAES_CERT_EMAIL="" bash "$SCRIPT_DIR/deploy/ensure-app-certs.sh" >/dev/null 2>&1; then
    echo "check-certs: expected failure without email" >&2
    exit 1
fi

echo "check-certs: ok"
