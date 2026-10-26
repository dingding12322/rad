#!/usr/bin/env bash
#
# Regenerate the macOS Alpha app icon from assets/icons/alpha-logo.png.
#
# `alpha-logo.png` arrives flattened on opaque black (no alpha channel) at
# 1254px. The production mark's own alpha channel is the same squircle, so it
# is reused as the matte: the black corners become transparent, the Alpha badge
# stays intact. The matted art is then inset to Apple's icon grid (824px
# centered on a 1024px canvas) exactly like icon.icns, so Dock/Finder render it
# at the same visual size as native apps and as the production icon.
#
# Outputs (both checked in):
#   assets/icons/alpha-logo.icns                   dev .app icon (scripts/run-bundled.sh)
#   crates/orbit-pi/dev-assets/alpha-app-icon.png  512px mark embedded by macOS
#                                                  debug builds (cargo run,
#                                                  Settings → About). The
#                                                  dev-assets/ overlay is not
#                                                  compiled into release binaries.
#
# Re-run after editing alpha-logo.png or icon.png (the matte source).
# Requires ImageMagick (`magick`) and the system `iconutil`.
#
set -euo pipefail

cd "$(dirname "$0")/.."

SRC="assets/icons/alpha-logo.png"
MASK_SRC="assets/icons/icon.png"
ICNS="assets/icons/alpha-logo.icns"
PNG512="crates/orbit-pi/dev-assets/alpha-app-icon.png"
INSET=824 # Apple's icon grid: 824px art centered in a 1024px canvas

info() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

command -v magick >/dev/null || {
  echo "error: ImageMagick is required (brew install imagemagick)" >&2
  exit 1
}
[ -f "$SRC" ] || { echo "error: missing $SRC" >&2; exit 1; }
[ -f "$MASK_SRC" ] || { echo "error: missing $MASK_SRC" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

size="$(magick identify -format '%wx%h' "$SRC")"
info "Matting $SRC ($size) with the production alpha channel"
magick "$MASK_SRC" -alpha extract -resize "$size" -strip -depth 8 "$tmp/mask.png"
magick "$SRC" \( "$tmp/mask.png" -alpha off \) -alpha off \
  -compose CopyOpacity -composite -depth 8 "$tmp/rgba.png"

info "Insetting art to ${INSET}/1024 (Apple's icon grid)"
magick "$tmp/rgba.png" -resize "${INSET}x${INSET}" -background none \
  -gravity center -extent 1024x1024 -depth 8 "$tmp/inset.png"

info "Writing $ICNS"
rm -rf "$tmp/alpha.iconset"
mkdir -p "$tmp/alpha.iconset"
for s in 16 32 128 256 512; do
  magick "$tmp/inset.png" -resize "${s}x${s}" -depth 8 \
    "$tmp/alpha.iconset/icon_${s}x${s}.png"
  magick "$tmp/inset.png" -resize "$((s * 2))x$((s * 2))" -depth 8 \
    "$tmp/alpha.iconset/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$tmp/alpha.iconset" -o "$ICNS"

info "Writing $PNG512"
magick "$tmp/inset.png" -resize 512x512 -strip -depth 8 \
  -define png:color-type=6 -define png:compression-level=9 "$PNG512"

info "Done"
