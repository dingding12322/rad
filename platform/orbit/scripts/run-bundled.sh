#!/usr/bin/env bash
#
# Build orbit-pi (debug) and launch it from a real .app bundle.
#
# Notifications need an app bundle identity: without one macOS attributes
# banners to Script Editor, shows no Orbit icon, and never delivers the
# click that routes back to a session. `cargo run` (and bacon) runs a bare
# binary, so use this script when testing notifications.
#
# This builds the **Alpha** development app: `alpha-logo.icns`, the separate
# `dev.orbit.pi.alpha` identity, and the debug build's embedded Alpha dock mark
# (see `crates/orbit-pi/src/app_icon.rs`). The separate bundle id lets it run
# beside the installed production app without claiming its notifications,
# permissions, or activation. The shipping bundle (`dev.orbit.pi`,
# `icon.icns`) is what `scripts/make-dmg.sh` builds.
#
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="Orbit Pi Alpha"
EXEC_NAME="orbit-pi"
BUNDLE_ID="dev.orbit.pi.alpha"
ICON="assets/icons/alpha-logo.icns"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/orbit-pi/Cargo.toml | head -1)"
APP="target/package/Orbit Pi Alpha.app"

info() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

info "Building orbit-pi (debug)…"
cargo build -p orbit-pi

info "Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/debug/$EXEC_NAME" "$APP/Contents/MacOS/$EXEC_NAME"
cp "$ICON" "$APP/Contents/Resources/icon.icns"
printf 'APPL????' > "$APP/Contents/PkgInfo"
chmod +x "$APP/Contents/MacOS/$EXEC_NAME"

cat > "$APP/Contents/Info.plist" <<EOF
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

# Ad-hoc signing gives the bundle its own identity — the identifier is what
# macOS keys notification attribution and permission to.
info "Ad-hoc signing as $BUNDLE_ID"
codesign --force --sign - --identifier "$BUNDLE_ID" "$APP"

# `open` would just activate an already-running Orbit; stop it so the fresh
# bundle is what launches.
pkill -x "$EXEC_NAME" 2>/dev/null || true

info "Opening $APP"
open "$APP"
