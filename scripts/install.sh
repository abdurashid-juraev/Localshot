#!/usr/bin/env bash
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
APP_DIR="$PREFIX/share/applications"
ICON_DIR="$PREFIX/share/icons/hicolor/scalable/apps"
AUTOSTART_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"

echo "Installing LocalShot to $PREFIX..."

if ! command -v cargo >/dev/null 2>&1; then
    if [ -f "$HOME/.cargo/bin/cargo" ]; then
        export PATH="$HOME/.cargo/bin:$PATH"
    fi
fi

echo "Building release binary..."
cargo build --release --manifest-path "$DIR/Cargo.toml"

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR" "$AUTOSTART_DIR"

install -m 755 "$DIR/target/release/localshot" "$BIN_DIR/localshot"
install -m 644 "$DIR/assets/icon.svg" "$ICON_DIR/localshot.svg"
install -m 644 "$DIR/assets/localshot.desktop" "$APP_DIR/localshot.desktop"
install -m 644 "$DIR/assets/localshot-autostart.desktop" "$AUTOSTART_DIR/localshot.desktop"

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APP_DIR" 2>/dev/null || true
fi

echo "LocalShot installed successfully!"
echo "  Binary:    $BIN_DIR/localshot"
echo "  Desktop:   $APP_DIR/localshot.desktop"
echo "  Autostart: $AUTOSTART_DIR/localshot.desktop"
echo "  Icon:      $ICON_DIR/localshot.svg"
