#!/usr/bin/env bash
#
# Regenerate assets/icons/background_660x400.tiff (the DMG window background)
# from the hand-made master art, assets/icons/background_660x400.png.
#
# Why this exists
# ---------------
# Finder draws icon-view item labels in BLACK whenever the folder has a custom
# background picture or colour — it only switches labels to white when the
# window keeps the default background, which would also remove the art. That is
# documented behaviour, not a bug we can work around:
#   - DMG Canvas: "If a background image or color is specified though, the file
#     names will always remain in black. Unfortunately there is no choice to
#     have a different styles of disk images for the different system
#     appearances."
#   - DropDMG: "When a background image is specified, Finder does not change
#     the color of the filenames for Dark Mode. The text is always displayed in
#     black."
# The icon-view `icvp` record in .DS_Store has no text-colour field either, and
# neither does AppleScript's `icon view options` (see create-dmg#197,
# ganache-ui#5139, SO 10776248).
#
# So "Orbit Pi.app" / "Applications" have to stay black, and the background
# underneath them has to be light instead. This script bakes a soft frosted
# pill into the art behind each label, at the exact spot Finder draws them.
#
# Geometry (must track scripts/make-dmg.sh)
# -----------------------------------------
#   DMG_APP_X / DMG_DROP_X  180 / 480  icon centres (labels are centred on them)
#   DMG_ICON_Y              190        icon centre
#   label offset            83         Finder draws the 12pt label box 83px
#                                      below the icon centre (measured: for an
#                                      icon at y=190 the text occupies
#                                      y=267..278 on macOS 26/27 with
#                                      iconSize=128, textSize=12). The pill is
#                                      34px tall so it still covers the text if
#                                      a future Finder shifts it by a few px.
#
# Output is checked in; re-run after editing the master art.
# Requires ImageMagick (`magick`).
#
set -euo pipefail

cd "$(dirname "$0")/.."

MASTER="assets/icons/background_660x400.png"
TIFF="assets/icons/background_660x400.tiff"

# --- keep in sync with scripts/make-dmg.sh ---------------------------------
APP_X=180
DROP_X=480
ICON_Y=190
LABEL_DY=83
# ---------------------------------------------------------------------------

PILL_W=172
PILL_H=34
PILL_R=$((PILL_H / 2)) # fully rounded
PILL_FILL="#eef4ff"
PILL_FILL_ALPHA=0.72   # still frosted: the planet shows through slightly
PILL_BORDER="#ffffff8c"
PILL_GLOW_ALPHA=0.22
PILL_SHADOW_ALPHA=0.32

BG_W=660
BG_H=400
BG_DPI=72

info() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

command -v magick >/dev/null || {
  echo "error: ImageMagick is required (brew install imagemagick)" >&2
  exit 1
}
[ -f "$MASTER" ] || { echo "error: missing $MASTER" >&2; exit 1; }

size="$(magick identify -format '%wx%h' "$MASTER")"
[ "$size" = "${BG_W}x${BG_H}" ] || {
  echo "error: $MASTER is $size, expected ${BG_W}x${BG_H}" >&2
  exit 1
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

label_y=$((ICON_Y + LABEL_DY))

info "Building label plates ($PILL_W x $PILL_H at y=$label_y, centred on x=$APP_X/$DROP_X)"

# Hard-edged plate mask.
magick -size "${BG_W}x${BG_H}" xc:black -fill white \
  -draw "roundrectangle $((APP_X - PILL_W / 2)),$((label_y - PILL_H / 2)) $((APP_X + PILL_W / 2)),$((label_y + PILL_H / 2)) $PILL_R,$PILL_R" \
  -draw "roundrectangle $((DROP_X - PILL_W / 2)),$((label_y - PILL_H / 2)) $((DROP_X + PILL_W / 2)),$((label_y + PILL_H / 2)) $PILL_R,$PILL_R" \
  "$tmp/plate.png"

# Soft outer glow, so the plate sits in the art instead of looking stuck on it.
magick "$tmp/plate.png" -blur 0x9 "$tmp/glow-mask.png"
magick -size "${BG_W}x${BG_H}" xc:"$PILL_FILL" "$tmp/glow-mask.png" -alpha off \
  -compose CopyOpacity -composite \
  -channel A -evaluate multiply "$PILL_GLOW_ALPHA" +channel "$tmp/glow.png"

# The plate itself.
magick -size "${BG_W}x${BG_H}" xc:"$PILL_FILL" "$tmp/plate.png" -alpha off \
  -compose CopyOpacity -composite \
  -channel A -evaluate multiply "$PILL_FILL_ALPHA" +channel "$tmp/fill.png"

# Faint dark halo under the plate for separation from the planet glow.
magick "$tmp/plate.png" -blur 0x5 -evaluate multiply 0.30 "$tmp/shadow-mask.png"
magick -size "${BG_W}x${BG_H}" xc:'#02040c' "$tmp/shadow-mask.png" -alpha off \
  -compose CopyOpacity -composite \
  -channel A -evaluate multiply "$PILL_SHADOW_ALPHA" +channel "$tmp/shadow.png"

info "Compositing onto $MASTER"
magick "$MASTER" \
  "$tmp/shadow.png" -compose over -composite \
  "$tmp/glow.png" -compose over -composite \
  "$tmp/fill.png" -compose over -composite \
  -stroke "$PILL_BORDER" -strokewidth 1 -fill none \
  -draw "roundrectangle $((APP_X - PILL_W / 2)),$((label_y - PILL_H / 2)) $((APP_X + PILL_W / 2)),$((label_y + PILL_H / 2)) $PILL_R,$PILL_R" \
  -draw "roundrectangle $((DROP_X - PILL_W / 2)),$((label_y - PILL_H / 2)) $((DROP_X + PILL_W / 2)),$((label_y + PILL_H / 2)) $PILL_R,$PILL_R" \
  -alpha off -depth 8 -compress none -units PixelsPerInch -density "$BG_DPI" \
  "$TIFF"

info "Writing $TIFF"
magick identify "$TIFF"

# The plates exist solely so 12pt black label text is legible — assert they are
# actually light enough (0.5 luminance is roughly 5.3:1 against black).
for x in "$APP_X" "$DROP_X"; do
  lum=$(magick "$TIFF" -crop "${PILL_W}x12+$((x - PILL_W / 2))+$((label_y - 6))" +repage \
    -colorspace Gray -format '%[fx:mean]' info:)
  info "Label row under x=$x: luminance $lum"
  awk -v l="$lum" 'BEGIN { if (l + 0 < 0.5) exit 1 }' || {
    echo "error: label plate at x=$x is too dark for black text" >&2
    exit 1
  }
done

info "Done. Verify the labels read on the dark art before shipping."
