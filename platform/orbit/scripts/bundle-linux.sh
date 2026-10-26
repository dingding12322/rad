#!/usr/bin/env bash
#
# Package the Linux build as a relocatable tarball and a native Debian package:
# the self-contained `orbit-pi` binary (assets + bundled pi extensions are
# compiled in), a desktop entry, the app icon, and the license. The tarball is
# what the in-app updater consumes; the .deb is for `apt install ./orbit-pi*.deb`.
#
# Usage:
#   ./scripts/bundle-linux.sh
#
# Env:
#   VERSION   bundle version (default: crates/orbit-pi/Cargo.toml)
#
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

version="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' crates/orbit-pi/Cargo.toml | head -1)}"
triple="$(rustc -vV | sed -n 's/^host: //p')"
package="orbit-pi-${version}-${triple}"
archive="dist/${package}.tar.gz"

staging="$(mktemp -d)"
trap 'rm -rf -- "$staging"' EXIT

cargo build --locked --release -p orbit-pi

dir="$staging/$package"
install -Dm755 target/release/orbit-pi "$dir/bin/orbit-pi"
install -Dm644 assets/icons/icon.png \
  "$dir/share/icons/hicolor/256x256/apps/dev.orbit.pi.png"
install -Dm644 LICENSE "$dir/share/licenses/orbit-pi/LICENSE"

mkdir -p "$dir/share/applications"
cat > "$dir/share/applications/dev.orbit.pi.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Orbit Pi
Comment=Native workbench for the pi coding agent
Exec=orbit-pi
Icon=dev.orbit.pi
Categories=Development;
Terminal=false
EOF

mkdir -p dist
tar -C "$staging" -czf "$archive" "$package"
printf 'Created %s\n' "$archive"

# --- Debian package --------------------------------------------------------
# A native .deb built with dpkg-deb, which ships with dpkg on every Debian or
# Ubuntu host (including the CI runner) — no cargo-deb or extra tooling needed.
# Best-effort: a host without dpkg-deb still produces the tarball above.
if command -v dpkg-deb >/dev/null 2>&1; then
  case "$triple" in
    x86_64-*)  deb_arch=amd64 ;;
    aarch64-*) deb_arch=arm64 ;;
    *)         deb_arch="$(dpkg --print-architecture)" ;;
  esac

  deb="$staging/deb"
  mkdir -p "$deb/DEBIAN" \
    "$deb/usr/bin" \
    "$deb/usr/share/applications" \
    "$deb/usr/share/icons/hicolor/256x256/apps" \
    "$deb/usr/share/doc/orbit-pi"
  install -Dm755 target/release/orbit-pi "$deb/usr/bin/orbit-pi"
  install -Dm644 assets/icons/icon.png \
    "$deb/usr/share/icons/hicolor/256x256/apps/dev.orbit.pi.png"
  install -Dm644 LICENSE "$deb/usr/share/doc/orbit-pi/copyright"
  install -Dm644 "$dir/share/applications/dev.orbit.pi.desktop" \
    "$deb/usr/share/applications/dev.orbit.pi.desktop"

  # Runtime libraries. dpkg-shlibdeps derives an exact Depends from the linked
  # binary; since dpkg 1.22 it insists on reading a debian/control, so give it
  # a throwaway one (the repo deliberately carries no debian/ tree). The static
  # list is the fallback for hosts without dpkg-dev, and the runtime
  # counterpart of the build dependencies above.
  depends=""
  if command -v dpkg-shlibdeps >/dev/null 2>&1; then
    shlib_work="$staging/shlibdeps"
    mkdir -p "$shlib_work/debian"
    cat > "$shlib_work/debian/control" <<'SHLIBS'
Source: orbit-pi
Section: devel
Priority: optional
Maintainer: Orbit Pi maintainers <noreply@github.com>
Standards-Version: 4.6.0

Package: orbit-pi
Architecture: any
Depends: ${shlibs:Depends}
Description: Native workbench for the pi coding agent
SHLIBS
    depends="$(cd "$shlib_work" && dpkg-shlibdeps -O "$deb/usr/bin/orbit-pi" 2>/dev/null \
      | sed -n 's/^shlibs:Depends=//p')" || true
  fi
  if [ -z "$depends" ]; then
    depends="libc6, libgcc-s1, libfontconfig1, libvulkan1, libwayland-client0, libx11-6, libx11-xcb1, libxcb1, libxkbcommon-x11-0, libxkbcommon0, zlib1g"
  fi

  cat > "$deb/DEBIAN/control" <<EOF
Package: orbit-pi
Version: ${version}
Section: devel
Priority: optional
Architecture: ${deb_arch}
Maintainer: Orbit Pi maintainers <noreply@github.com>
Depends: ${depends}
Homepage: https://github.com/imrj05/orbit
Description: Native workbench for the pi coding agent
 A GPUI desktop client that drives the pi coding agent over its RPC
 protocol. Assets and bundled pi extensions are compiled into the binary.
EOF

  deb_file="dist/orbit-pi_${version}_${deb_arch}.deb"
  # Fixed mtimes so identical build inputs produce a byte-identical package.
  find "$deb" -exec touch -d '1980-01-01 00:00:00 UTC' {} +
  dpkg-deb --root-owner-group --build "$deb" "$deb_file"
  printf 'Created %s\n' "$deb_file"
else
  printf 'dpkg-deb not found; skipping the .deb package\n' >&2
fi
