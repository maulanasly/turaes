#!/usr/bin/env bash
# ensure-app-certs.sh — idempotent TLS issuance for turaes-managed domains.
#
# Collects hostnames from the turaes database (applications.domain + the
# domains table) and runs `certbot certonly --webroot` for each one missing a
# live cert under $CERT_DIR. Existing valid certs are skipped; renewals stay
# with certbot's own timer. The Pingora proxy reloads certs on file-mtime
# change, so no turaes restart is needed afterwards.
#
# Environment (all optional except EMAIL when issuance is needed):
#   TURAES_DB            SQLite file (default /var/lib/turaes/turaes.db).
#                        TURAES_DATABASE_URL (sqlite:///path?...) is honored too.
#   TURAES_CERT_DIR      certbot live dir (default /etc/letsencrypt/live).
#   TURAES_ACME_WEBROOT  webroot served at /.well-known/acme-challenge/
#                        (default /var/lib/turaes/acme; must match the proxy).
#   TURAES_CERT_EMAIL    contact for new certbot registrations (required when
#                        at least one domain needs a cert).
#   DRY_RUN=1            pass --dry-run to certbot (staging validation only).
set -euo pipefail

DB="${TURAES_DB:-}"
if [[ -z "$DB" && -n "${TURAES_DATABASE_URL:-}" ]]; then
    # sqlite:///var/lib/turaes/turaes.db?mode=rwc -> /var/lib/turaes/turaes.db
    DB="${TURAES_DATABASE_URL#sqlite://}"
    DB="${DB%%\?*}"
fi
DB="${DB:-/var/lib/turaes/turaes.db}"
CERT_DIR="${TURAES_CERT_DIR:-/etc/letsencrypt/live}"
WEBROOT="${TURAES_ACME_WEBROOT:-/var/lib/turaes/acme}"
EMAIL="${TURAES_CERT_EMAIL:-}"
DRY_RUN="${DRY_RUN:-0}"

command -v sqlite3 >/dev/null || { echo "ensure-app-certs: sqlite3 not found" >&2; exit 2; }
[[ -f "$DB" ]] || { echo "ensure-app-certs: database not found: $DB" >&2; exit 2; }
command -v openssl >/dev/null || { echo "ensure-app-certs: openssl not found" >&2; exit 2; }

# Portable (bash 3.2 has no mapfile).
DOMAINS=()
while IFS= read -r line; do
    [[ -n "$line" ]] && DOMAINS+=("$line")
done < <(sqlite3 -noheader -list "$DB" \
    "SELECT domain FROM applications WHERE domain IS NOT NULL AND domain != '' UNION SELECT domain FROM domains ORDER BY 1;" \
    | tr -d '\r' | sort -u)

# Let's Encrypt only issues for public DNS names: require a dot, legal
# hostname chars, and skip local-only suffixes.
is_public_name() {
    local d="$1"
    [[ "$d" == *.* ]] || return 1
    [[ "$d" =~ ^[A-Za-z0-9.-]+$ ]] || return 1
    [[ "$d" == *.local || "$d" == localhost ]] && return 1
    return 0
}

cert_ok() {
    local pem="$CERT_DIR/$1/fullchain.pem"
    [[ -f "$pem" ]] && openssl x509 -in "$pem" -noout -checkend 0 >/dev/null 2>&1
}

issued=0
skipped=0
failed=0

for d in ${DOMAINS[@]+"${DOMAINS[@]}"}; do
    if ! is_public_name "$d"; then
        echo "SKIP (not a public name): $d"
        skipped=$((skipped + 1))
        continue
    fi
    if cert_ok "$d"; then
        echo "OK (valid cert): $d"
        skipped=$((skipped + 1))
        continue
    fi
    if [[ -z "$EMAIL" ]]; then
        echo "FAIL (no cert, set TURAES_CERT_EMAIL): $d" >&2
        failed=$((failed + 1))
        continue
    fi
    mkdir -p "$WEBROOT"
    args=(certonly --webroot -w "$WEBROOT" -d "$d" --cert-name "$d"
        --agree-tos -m "$EMAIL" --non-interactive --keep-until-expiring)
    if [[ "$DRY_RUN" == "1" ]]; then
        args+=(--dry-run)
    fi
    # DRY_RUN without certbot installed is still a useful listing; real runs
    # require certbot.
    if [[ "$DRY_RUN" == "1" ]] && ! command -v certbot >/dev/null; then
        echo "WOULD ISSUE (dry-run, no certbot): $d"
        issued=$((issued + 1))
        continue
    fi
    command -v certbot >/dev/null || { echo "ensure-app-certs: certbot not found" >&2; exit 2; }
    if certbot "${args[@]}"; then
        echo "ISSUED: $d"
        issued=$((issued + 1))
    else
        echo "FAIL (certbot error): $d" >&2
        failed=$((failed + 1))
    fi
done

echo "ensure-app-certs: issued=$issued skipped=$skipped failed=$failed"
[[ "$failed" == "0" ]]
