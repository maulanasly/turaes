#!/usr/bin/env sh
# Syntax-check every frontend ES module without a bundler.
# `node --check` validates syntax without resolving imports, so we copy each
# `.js` (ESM) to a temp `.mjs` so Node parses it as a module.
set -e
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
find static/js -name '*.js' -not -path '*/vendor/*' | while read -r f; do
  cp "$f" "$tmp/mod.mjs"
  if ! node --check "$tmp/mod.mjs" 2>/dev/null; then
    echo "syntax error: $f" >&2
    node --check "$tmp/mod.mjs"
    exit 1
  fi
done
echo "frontend modules OK"
