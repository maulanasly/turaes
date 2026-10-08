#!/usr/bin/env bash
# sync-brand-assets.sh — copy the brand set from ../turaes-logo-assets into
# static/brand, generating the sizes the source lacks. Re-run mechanically on
# every asset drop; needs python3 + PIL (or sips on macOS) for the 192px
# downscale. Run from the repo root.
set -euo pipefail

SRC="${1:-../turaes-logo-assets}"
DEST="static/brand"

[[ -d "$SRC" ]] || { echo "sync-brand-assets: source not found: $SRC" >&2; exit 2; }
mkdir -p "$DEST"

# Theme-specific vectors (the per-size SVGs are pairwise identical; ship the
# 512 variant as the canonical mark).
cp "$SRC/turaes-mark-dark-512.svg" "$DEST/turaes-mark-dark.svg"
cp "$SRC/turaes-mark-light-512.svg" "$DEST/turaes-mark-light.svg"

# Theme-neutral favicons, byte-identical for both variants.
for s in 16 32 48; do
  cp "$SRC/favicon-$s.png" "$DEST/turaes-icon-dark-$s.png"
  cp "$SRC/favicon-$s.png" "$DEST/turaes-icon-light-$s.png"
done

# 192px has no source: deterministic LANCZOS downscale of favicon-256.
if command -v python3 >/dev/null && python3 -c "import PIL.Image" 2>/dev/null; then
  python3 - "$SRC/favicon-256.png" "$DEST" <<'EOF'
import sys
from PIL import Image
src, dest = sys.argv[1], sys.argv[2]
im = Image.open(src)
assert im.size == (256, 256), f"unexpected favicon-256 size: {im.size}"
for variant in ("dark", "light"):
    out = f"{dest}/turaes-icon-{variant}-192.png"
    im.resize((192, 192), Image.LANCZOS).save(out)
    print(f"generated {out}")
EOF
elif command -v sips >/dev/null; then
  for variant in dark light; do
    sips -z 192 192 -o "$DEST/turaes-icon-$variant-192.png" "$SRC/favicon-256.png" >/dev/null
  done
else
  echo "sync-brand-assets: need python3+PIL or sips for the 192px resize" >&2
  exit 2
fi

# 512px has no favicon source: use the themed mark renders.
cp "$SRC/turaes-mark-dark-512.png" "$DEST/turaes-icon-dark-512.png"
cp "$SRC/turaes-mark-light-512.png" "$DEST/turaes-icon-light-512.png"

# Legacy multi-size icon.
cp "$SRC/favicon.ico" "$DEST/favicon.ico"

echo "sync-brand-assets: done — $(ls "$DEST" | wc -l | tr -d ' ') files in $DEST"
