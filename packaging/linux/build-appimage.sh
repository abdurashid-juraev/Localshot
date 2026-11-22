#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n1)}"
APPDIR="$ROOT_DIR/dist/AppDir"

if [ ! -f "$ROOT_DIR/target/release/localshot" ]; then
    cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"
fi

rm -rf "$APPDIR"
mkdir -p \
    "$APPDIR/usr/bin" \
    "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/scalable/apps"

install -m 755 "$ROOT_DIR/target/release/localshot" "$APPDIR/usr/bin/localshot"
install -m 644 "$ROOT_DIR/assets/localshot.desktop" "$APPDIR/usr/share/applications/localshot.desktop"
install -m 644 "$ROOT_DIR/assets/localshot.desktop" "$APPDIR/localshot.desktop"
install -m 644 "$ROOT_DIR/assets/icon.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/localshot.svg"
install -m 644 "$ROOT_DIR/assets/icon.svg" "$APPDIR/localshot.svg"

cat > "$APPDIR/AppRun" <<'EOF'
#!/usr/bin/env bash
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
exec "$HERE/usr/bin/localshot" "$@"
EOF
chmod +x "$APPDIR/AppRun"

mkdir -p "$ROOT_DIR/dist"
if command -v appimagetool >/dev/null 2>&1; then
    ARCH=x86_64 appimagetool "$APPDIR" "$ROOT_DIR/dist/LocalShot-${VERSION}-x86_64.AppImage"
    echo "Built: $ROOT_DIR/dist/LocalShot-${VERSION}-x86_64.AppImage"
else
    echo "appimagetool not found; prepared AppDir at $APPDIR"
fi
