#!/usr/bin/env bash
set -e

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "🚀 Installing LocalShot to system..."

# Ensure cargo is accessible
if ! command -v cargo >/dev/null 2>&1; then
    if [ -f "$HOME/.cargo/bin/cargo" ]; then
        export PATH="$HOME/.cargo/bin:$PATH"
    fi
fi

# Build release if not already present
if [ ! -f "$DIR/target/release/localshot" ]; then
    echo "📦 Building release binary..."
    cargo build --release
fi

# Ensure user directories exist
mkdir -p "$HOME/.local/bin"
mkdir -p "$HOME/.local/share/applications"
mkdir -p "$HOME/.local/share/icons/hicolor/scalable/apps"
mkdir -p "$HOME/Desktop"

# Copy binary & icons
echo "🔧 Installing binary and icons..."
cp "$DIR/target/release/localshot" "$HOME/.local/bin/localshot"
chmod +x "$HOME/.local/bin/localshot"

cp "$DIR/assets/icon.svg" "$HOME/.local/share/icons/localshot.svg"
cp "$DIR/assets/icon.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/localshot.svg"

# Copy desktop entries
echo "🖥️ Creating desktop launchers..."
cp "$DIR/assets/localshot.desktop" "$HOME/.local/share/applications/localshot.desktop"
cp "$DIR/assets/localshot.desktop" "$HOME/Desktop/LocalShot.desktop"

chmod +x "$HOME/.local/share/applications/localshot.desktop"
chmod +x "$HOME/Desktop/LocalShot.desktop"

# Allow launching in GNOME Desktop
if command -v gio >/dev/null 2>&1; then
    gio set "$HOME/Desktop/LocalShot.desktop" metadata::trusted true 2>/dev/null || true
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
fi

echo "✅ LocalShot installed successfully!"
echo "📍 Desktop shortcut: ~/Desktop/LocalShot.desktop"
echo "📍 Application menu: Super key -> type 'LocalShot'"
echo "📍 Binary: ~/.local/bin/localshot"
