#!/usr/bin/env bash
# Build TrontSnap-x86_64.AppImage — the Linux analog of the portable .exe.
#
# Usage: packaging/linux/appimage.sh         (expects target/release/trontsnap built)
#        packaging/linux/appimage.sh --build (cargo build --release first)
#
# Downloads appimagetool on first run (cached beside this script). Runs it with
# --appimage-extract-and-run so no system FUSE is needed.
#
# Runtime deps the AppImage does NOT bundle (tiny, universal, and version-touchy):
#   wl-clipboard or xclip   clipboard writes (wl-copy daemonizes to own the selection)
#   gnome-screenshot/grim/spectacle   Wayland grab fallback when xcap can't
#   ffmpeg                  recording + video thumbnails + GIF export
#   tesseract-ocr           region OCR (optional)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BIN="$ROOT/target/release/trontsnap"
APPDIR="$ROOT/target/TrontSnap.AppDir"
TOOL="$HERE/appimagetool-x86_64.AppImage"

if [[ "${1:-}" == "--build" ]]; then
    (cd "$ROOT" && cargo build --release)
fi
[[ -x "$BIN" ]] || { echo "build target/release/trontsnap first (or pass --build)"; exit 1; }

if [[ ! -x "$TOOL" ]]; then
    echo "fetching appimagetool..."
    curl -fL -o "$TOOL" \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$TOOL"
fi

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/icons/hicolor/256x256/apps"
cp "$BIN" "$APPDIR/usr/bin/trontsnap"
cp "$HERE/trontsnap.desktop" "$APPDIR/trontsnap.desktop"
# The embedded rainbow icon, exported for the desktop entry.
if command -v convert >/dev/null; then
    convert "$ROOT/assets/trontsnap.png" -resize 256x256 "$APPDIR/trontsnap.png"
else
    cp "$ROOT/assets/trontsnap.png" "$APPDIR/trontsnap.png"
fi
cp "$APPDIR/trontsnap.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/trontsnap.png"
cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
exec "$(dirname "$0")/usr/bin/trontsnap" "$@"
EOF
chmod +x "$APPDIR/AppRun"

VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
OUT="$ROOT/target/TrontSnap-v${VERSION}-x86_64.AppImage"
ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$APPDIR" "$OUT"
echo "built: $OUT"
