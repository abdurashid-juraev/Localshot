#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n1)}"
ARCH="${ARCH:-amd64}"
PKG_DIR="$ROOT_DIR/dist/deb/localshot_${VERSION}_${ARCH}"

if [ ! -f "$ROOT_DIR/target/release/localshot" ]; then
    cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"
fi

rm -rf "$PKG_DIR"
mkdir -p \
    "$PKG_DIR/DEBIAN" \
    "$PKG_DIR/usr/bin" \
    "$PKG_DIR/usr/share/applications" \
    "$PKG_DIR/usr/share/icons/hicolor/scalable/apps" \
    "$PKG_DIR/etc/xdg/autostart"

install -m 755 "$ROOT_DIR/target/release/localshot" "$PKG_DIR/usr/bin/localshot"
install -m 644 "$ROOT_DIR/assets/localshot.desktop" "$PKG_DIR/usr/share/applications/localshot.desktop"
install -m 644 "$ROOT_DIR/assets/icon.svg" "$PKG_DIR/usr/share/icons/hicolor/scalable/apps/localshot.svg"
install -m 644 "$ROOT_DIR/assets/localshot-autostart.desktop" "$PKG_DIR/etc/xdg/autostart/localshot.desktop"

cat > "$PKG_DIR/DEBIAN/control" <<EOF
Package: localshot
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${ARCH}
Maintainer: LocalShot Team <noreply@localshot.app>
Depends: libc6, libgtk-3-0 | libgtk-3-0t64, libayatana-appindicator3-1 | libappindicator3-1, libx11-6, libxcb1, libxkbcommon0, libwayland-client0, libdbus-1-3
Recommends: xdg-desktop-portal, libnotify-bin
Description: 100% offline, privacy-first screenshot annotation tool
 LocalShot is a fast, offline Lightshot alternative built with Rust and Slint.
EOF

mkdir -p "$ROOT_DIR/dist"
dpkg-deb --build --root-owner-group "$PKG_DIR" "$ROOT_DIR/dist/localshot_${VERSION}_${ARCH}.deb"
echo "Built: $ROOT_DIR/dist/localshot_${VERSION}_${ARCH}.deb"
