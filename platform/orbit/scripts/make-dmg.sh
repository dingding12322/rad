#!/usr/bin/env bash
#
# Build and package Orbit as a macOS .app bundle, wrap it in a DMG, and emit
# the .tar.gz the in-app updater installs from.
#
# Usage:
#   ./scripts/make-dmg.sh            # arm64 + universal DMGs
#   ./scripts/make-dmg.sh arm64      # Apple Silicon DMG only
#   ./scripts/make-dmg.sh universal  # universal (arm64 + x86_64) DMG only
#   ./scripts/make-dmg.sh both       # both (default)
#
# Overrides (env):
#   SIGN_ID         codesign identity (default: Developer ID Application)
#   SIGNING=0       ad-hoc sign instead (no Developer ID needed)
#   VERSION         bundle version (default: read from Cargo.toml)
#   NOTARY_PROFILE  notarytool keychain profile; when set, each .app and DMG
#                   is notarized and stapled (otherwise notarization is
#                   skipped with a warning and Gatekeeper warns on other Macs)
#
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

TARGET="${1:-both}"
VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' crates/orbit-pi/Cargo.toml | head -1)}"
APP_NAME="Orbit Pi"
EXEC_NAME="orbit-pi"
BUNDLE_ID="dev.orbit.pi"
ICON="assets/icons/icon.icns"
# DMG window styling. The background art is 660x400 and Finder draws a folder
# background at natural size from the top-left of the content area, so the
# window frame has to be one title bar taller than the art to avoid trimming
# its bottom edge.
# The TIFF has two frosted label plates baked in behind "Orbit Pi.app" and
# "Applications": Finder draws icon labels in black whenever a background
# picture is set (it only adapts to Dark Mode for the default background), so
# the art has to carry the light area the labels sit on. Regenerate it with
# scripts/make-dmg-background.sh — the plate positions must track the icon
# coordinates below.
DMG_BG="assets/icons/background_660x400.tiff"
DMG_BG_NAME="background.tiff"
DMG_CONTENT_W=660
DMG_CONTENT_H=400
DMG_TITLEBAR_H=28
DMG_ICON_SIZE=128
DMG_APP_X=180
DMG_DROP_X=480
DMG_ICON_Y=190
# `SIGNING=0` ad-hoc signs the bundle — what CI builds when the Apple secrets
# are absent. Otherwise default to the maintainer's Developer ID identity.
if [ "${SIGNING:-1}" = "0" ]; then
  SIGN_ID="-"
else
  SIGN_ID="${SIGN_ID:-Developer ID Application: One Man Wireless Inc. (RFBXG4V45C)}"
fi

BUILD_ROOT="$ROOT/target/package"
STAGE="$BUILD_ROOT/stage"
DIST="$ROOT/dist"

info()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn()  { printf '\033[1;33m!!>\033[0m %s\n' "$*"; }
die()   { printf '\033[1;31mXX>\033[0m %s\n' "$*" >&2; exit 1; }

# --- create the .app bundle from an already-built binary -------------------
make_app() {
  local binary="$1"
  local out="$2"

  info "Assembling .app: $out"
  rm -rf "$out"
  mkdir -p "$out/Contents/MacOS" "$out/Contents/Resources"

  cp "$binary" "$out/Contents/MacOS/$EXEC_NAME"
  cp "$ICON" "$out/Contents/Resources/icon.icns"
  printf 'APPL????' > "$out/Contents/PkgInfo"

  cat > "$out/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleLocalizations</key>
    <array>
        <string>en</string>
        <string>zh-CN</string>
        <string>ja</string>
        <string>ko</string>
        <string>es</string>
        <string>fr</string>
        <string>de</string>
        <string>pt-BR</string>
        <string>ru</string>
        <string>it</string>
    </array>
    <key>CFBundleExecutable</key>
    <string>${EXEC_NAME}</string>
    <key>CFBundleIconFile</key>
    <string>icon</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

  chmod +x "$out/Contents/MacOS/$EXEC_NAME"
  info "Signing .app with: $SIGN_ID"
  # No `--deep`: the bundle carries no nested code, and `--deep` is deprecated
  # and can stall `codesign` for a long time on a large universal binary.
  local args=(--force --sign "$SIGN_ID")
  # Hardened runtime and a secure timestamp need a real Developer ID; an
  # ad-hoc signature ("-") rejects them.
  if [ "$SIGN_ID" != "-" ]; then
    args+=(--options runtime --timestamp)
  fi
  codesign "${args[@]}" "$out"
}

# --- wrap a signed .app into a DMG -----------------------------------------
make_dmg() {
  local app="$1"
  local dmg="$2"

  info "Creating DMG: $dmg"
  rm -f "$dmg"
  local dmg_stage="$BUILD_ROOT/dmg-stage"
  local rw="$BUILD_ROOT/rw.dmg"
  local mnt="/Volumes/$APP_NAME"
  rm -rf "$dmg_stage" "$rw"
  mkdir -p "$dmg_stage"
  cp -R "$app" "$dmg_stage/"
  # Drag-and-drop target.
  ln -s /Applications "$dmg_stage/Applications"
  # Keep the background inside the image so the layout travels with it.
  mkdir -p "$dmg_stage/.background"
  cp "$DMG_BG" "$dmg_stage/.background/$DMG_BG_NAME"

  # Finder can only lay out a writable HFS+ image; compress it afterwards.
  hdiutil create \
    -volname "$APP_NAME" \
    -srcfolder "$dmg_stage" \
    -fs HFS+ \
    -format UDRW \
    -ov \
    "$rw" >/dev/null
  # A volume of the same name from a previous run would shadow this one.
  if [ -d "$mnt" ]; then hdiutil detach "$mnt" >/dev/null 2>&1 || true; fi
  hdiutil attach "$rw" -readwrite -noverify -noautoopen >/dev/null
  sleep 1

  info "Laying out the DMG window"
  # Finder writes the icon positions and background into the volume's
  # .DS_Store. Setting the background has historically been flaky (and can
  # fail on runners without a GUI session), so it is best-effort: without it
  # the DMG is still valid, just unstyled.
  osascript <<EOF || warn "Finder layout failed — shipping an unstyled DMG"
tell application "Finder"
  tell disk "$APP_NAME"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {200, 120, $((200 + DMG_CONTENT_W)), $((120 + DMG_CONTENT_H + DMG_TITLEBAR_H))}
    set opts to the icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to $DMG_ICON_SIZE
    set position of item "$APP_NAME.app" of container window to {$DMG_APP_X, $DMG_ICON_Y}
    set position of item "Applications" of container window to {$DMG_DROP_X, $DMG_ICON_Y}
    try
      set background picture of opts to file ".background:$DMG_BG_NAME"
    on error errMsg
      log "background not applied: " & errMsg
    end try
    update without registering applications
    delay 1
  end tell
end tell
EOF

  sync
  rm -rf "$mnt/.fseventsd" "$mnt/.Trashes" 2>/dev/null || true
  hdiutil detach "$mnt" >/dev/null
  hdiutil convert "$rw" -format UDZO -ov -o "$dmg" >/dev/null
  rm -f "$rw"

  # Sign the disk image itself so Gatekeeper accepts it as a package
  # (`spctl -a -t open --context context:primary-signature`) and notarytool
  # has a signature to match the ticket against. Must happen before
  # notarizing and stapling.
  if [ "$SIGN_ID" != "-" ]; then
    info "Signing DMG with: $SIGN_ID"
    codesign --force --sign "$SIGN_ID" --timestamp "$dmg"
  fi
  echo "  -> $dmg"
}

# --- notarization (opt-in) -------------------------------------------------
# A Developer-ID-signed artifact still triggers Gatekeeper until it is
# notarized and stapled. Set NOTARY_PROFILE to a `notarytool` keychain profile
# (`xcrun notarytool store-credentials`) to enable it; without one the build
# still succeeds and prints what is missing.
notary_enabled() { [[ -n "${NOTARY_PROFILE:-}" ]]; }

notarize_app() {
  local app="$1"
  if ! notary_enabled; then
    warn "NOTARY_PROFILE unset — skipping notarization (Gatekeeper will warn on other Macs)"
    return 0
  fi
  local zip="$app.zip"
  info "Notarizing .app (this can take a few minutes)…"
  rm -f "$zip"
  ditto -c -k --keepParent "$app" "$zip"
  xcrun notarytool submit "$zip" --keychain-profile "$NOTARY_PROFILE" --wait
  info "Stapling .app…"
  xcrun stapler staple "$app"
}

notarize_dmg() {
  local dmg="$1"
  notary_enabled || return 0
  info "Notarizing DMG…"
  xcrun notarytool submit "$dmg" --keychain-profile "$NOTARY_PROFILE" --wait
  info "Stapling DMG…"
  xcrun stapler staple "$dmg"
}

build_and_package() {
  local label="$1"   # e.g. arm64
  local arch="$2"    # rust target triple
  local binary="$3"  # path to the release binary for this arch

  # The bundle is always named "Orbit Pi.app" — the label only distinguishes
  # the DMG/tarball downloads. A per-label directory keeps the two builds from
  # clobbering each other without putting the label in the app's Finder name.
  local app="$BUILD_ROOT/${label}/${APP_NAME}.app"
  make_app "$binary" "$app"
  notarize_app "$app"

  local dmg="$DIST/${APP_NAME}-${VERSION}-${label}.dmg"
  make_dmg "$app" "$dmg"
  notarize_dmg "$dmg"

  # Updater payload: a .tar.gz whose single top-level entry is the .app. The
  # in-app updater extracts it and swaps the bundle in place. Named without
  # spaces so the appcast enclosure URL stays clean.
  local tarball="$DIST/Orbit-Pi-${VERSION}-${label}.tar.gz"
  local payload="$BUILD_ROOT/updater-${label}"
  rm -rf "$payload"
  mkdir -p "$payload"
  cp -R "$app" "$payload/${APP_NAME}.app"
  info "Creating updater archive: $tarball"
  tar -C "$payload" -czf "$tarball" "${APP_NAME}.app"
}

# --- build the release binaries -------------------------------------------
mkdir -p "$BUILD_ROOT" "$DIST"

# arm64 (Apple Silicon)
if [[ "$TARGET" == "arm64" || "$TARGET" == "both" ]]; then
  info "Building release (aarch64-apple-darwin)…"
  cargo build --release -p orbit-pi --target aarch64-apple-darwin
  build_and_package "arm64" "aarch64-apple-darwin" \
    "$ROOT/target/aarch64-apple-darwin/release/$EXEC_NAME"
fi

# universal (arm64 + x86_64)
if [[ "$TARGET" == "universal" || "$TARGET" == "both" ]]; then
  # lipo needs an arm64 slice; build it unless the arm64 DMG already did.
  if [[ ! -f "$ROOT/target/aarch64-apple-darwin/release/$EXEC_NAME" ]]; then
    info "Building release (aarch64-apple-darwin)…"
    cargo build --release -p orbit-pi --target aarch64-apple-darwin
  fi

  info "Building release (x86_64-apple-darwin)…"
  cargo build --release -p orbit-pi --target x86_64-apple-darwin

  info "Combining universal binary with lipo…"
  uni="$BUILD_ROOT/${EXEC_NAME}-universal"
  lipo -create \
    "$ROOT/target/aarch64-apple-darwin/release/$EXEC_NAME" \
    "$ROOT/target/x86_64-apple-darwin/release/$EXEC_NAME" \
    -output "$uni"

  build_and_package "universal" "universal" "$uni"
fi

info "Done. DMGs in: $DIST"
