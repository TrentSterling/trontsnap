#!/usr/bin/env bash
# Install TrontSnap for the current user (no root). Upgrade = unpack the newer
# tarball and run this again.
#
#   ./install.sh                 -> ~/.local/bin/trontsnap + menu entry + icons
#   PREFIX=/opt/ts ./install.sh  -> somewhere else
#
# HARD-WON: the binary lands at a FIXED path on purpose. TrontSnap writes its
# GNOME keybindings and autostart entry from its own exe path, so running it
# straight out of the unpacked folder (or /tmp) would leave Print pointing at a
# file that later disappears.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX_GIVEN="${PREFIX:-}"
PREFIX="${PREFIX:-$HOME/.local}"
if [[ $(id -u) -eq 0 && -z $PREFIX_GIVEN ]]; then
    echo "Run this as your normal user (it installs to ~/.local), or set PREFIX=/usr/local." >&2
    exit 1
fi
[[ -x $HERE/bin/trontsnap ]] || { echo "bin/trontsnap missing: run install.sh from the unpacked folder." >&2; exit 1; }

BIN="$PREFIX/bin/trontsnap"
APP_ID=xyz.tront.TrontSnap

# Temp name + rename: replacing a binary that is currently running must not hit
# "Text file busy", and a failed copy must not leave half a binary behind.
install -d "$PREFIX/bin"
install -m755 "$HERE/bin/trontsnap" "$PREFIX/bin/.trontsnap.new.$$"
mv -f "$PREFIX/bin/.trontsnap.new.$$" "$BIN"

# Absolute, quoted Exec: ~/.local/bin is often not on the desktop session's PATH
# until the next login, and paths may contain spaces.
install -d "$PREFIX/share/applications"
while IFS= read -r line; do
    if [[ $line == Exec=trontsnap* ]]; then
        line="Exec=\"$BIN\"${line#Exec=trontsnap}"
    fi
    printf '%s\n' "$line"
done < "$HERE/share/applications/$APP_ID.desktop" > "$PREFIX/share/applications/$APP_ID.desktop"
chmod 644 "$PREFIX/share/applications/$APP_ID.desktop"

while IFS= read -r icon; do
    install -Dm644 "$HERE/share/icons/$icon" "$PREFIX/share/icons/$icon"
done < <(cd "$HERE/share/icons" && find hicolor -name '*.png' | sort)

install -d "$PREFIX/share/doc/trontsnap" "$PREFIX/share/trontsnap"
install -m644 "$HERE"/share/doc/trontsnap/* "$HERE/README.txt" "$PREFIX/share/doc/trontsnap/"
install -m755 "$HERE/uninstall.sh" "$PREFIX/share/trontsnap/uninstall.sh"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f "$PREFIX/share/icons/hicolor" 2>/dev/null || true
fi

echo "Installed TrontSnap to $BIN"

# === what the app shells out to ===
missing=()
command -v ffmpeg >/dev/null 2>&1 || missing+=("ffmpeg (recording, GIF export, video thumbnails)")
command -v xclip >/dev/null 2>&1 || command -v wl-copy >/dev/null 2>&1 \
    || missing+=("xclip or wl-clipboard (clipboard)")
command -v tesseract >/dev/null 2>&1 || missing+=("tesseract-ocr (optional: copy text from a region)")
if ! command -v gnome-screenshot >/dev/null 2>&1 && ! command -v grim >/dev/null 2>&1 \
    && ! command -v spectacle >/dev/null 2>&1; then
    missing+=("gnome-screenshot, grim or spectacle (Wayland capture fallback)")
fi
if ((${#missing[@]})); then
    echo
    echo "Not found on this machine (install with your package manager, e.g. apt):"
    printf '  - %s\n' "${missing[@]}"
fi

case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) printf '\nNote: %s is not on your PATH; launch from the app menu or run %s\n' "$PREFIX/bin" "$BIN" ;;
esac
cat <<MSG

Start it from the app menu, or run: $BIN
A TrontSnap that is already running keeps the old version until you restart it.
To remove it later: ${PREFIX_GIVEN:+PREFIX=$PREFIX }$PREFIX/share/trontsnap/uninstall.sh
MSG
