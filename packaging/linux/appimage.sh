#!/usr/bin/env bash
# Build TrontSnap-vX.Y.Z-x86_64.AppImage (+ .zsync) -- the Linux analog of the
# portable .exe, and the one that can upgrade itself.
#
# usage: packaging/linux/appimage.sh [--version X.Y.Z] [--out DIR] [--build]
#                                    [--appimage-tool PATH] [--require-zsync]
#
#   --version         default: [package] version in Cargo.toml
#   --out             default: <repo>/dist
#   --build           cargo build --release first (local convenience; CI builds itself)
#   --appimage-tool   appimagetool to use; also env APPIMAGETOOL. Downloaded (pinned,
#                     sha256-checked) to that path if missing. Default lives in
#                     packaging/linux/tools/, which is gitignored.
#   --require-zsync   fail instead of warn when no .zsync gets produced
#   env TRONTSNAP_BIN overrides the binary (default target/release/trontsnap).
#
# Self-update: the embedded update string makes AppImageUpdate (and the
# `appimageupdatetool` CLI) pull the newest release's .zsync. It only works when
# EVERY GitHub release carries the Linux assets: if a Windows-only release becomes
# "latest", AppImageUpdate finds no matching asset and reports no update.
#
# glibc: the AppImage bundles no libs, so it needs at least the glibc of the
# machine that built the binary. Build on the oldest Ubuntu you want to support.
#
# Runtime deps it does NOT bundle (small, universal, version-touchy): ffmpeg,
# xclip or wl-clipboard, gnome-screenshot/grim/spectacle, tesseract-ocr, and the
# XWayland/X11 + GL client libs any desktop already has.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# Pinned: the "continuous" tag moves under you and has no stable checksum.
TOOL_VERSION=1.9.1
TOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
TOOL_URL="https://github.com/AppImage/appimagetool/releases/download/$TOOL_VERSION/appimagetool-x86_64.AppImage"
UPDATE_INFO="gh-releases-zsync|TrentSterling|trontsnap|latest|TrontSnap-*x86_64.AppImage.zsync"

VERSION_ARG="" OUT="$REPO_ROOT/dist" DO_BUILD=0 REQUIRE_ZSYNC=0
TOOL="${APPIMAGETOOL:-$PKG_DIR/tools/appimagetool-x86_64.AppImage}"
while (($#)); do
    case $1 in
        --version)        VERSION_ARG="${2:?--version needs a value}"; shift 2 ;;
        --out)            OUT="${2:?--out needs a value}"; shift 2 ;;
        --appimage-tool)  TOOL="${2:?--appimage-tool needs a value}"; shift 2 ;;
        --build)          DO_BUILD=1; shift ;;
        --require-zsync)  REQUIRE_ZSYNC=1; shift ;;
        -h|--help)        sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

VERSION="$(resolve_version "$VERSION_ARG")"
((DO_BUILD)) && (cd "$REPO_ROOT" && cargo build --release)
resolve_bin
EPOCH="$(source_epoch)"

fetch_tool() {
    log "fetching appimagetool $TOOL_VERSION -> $TOOL"
    mkdir -p "$(dirname "$TOOL")"
    local part="$TOOL.part.$$"
    curl -fL --retry 3 -sS -o "$part" "$TOOL_URL" || { rm -f "$part"; die "download failed: $TOOL_URL"; }
    if [[ "$(sha256sum "$part" | cut -d' ' -f1)" != "$TOOL_SHA256" ]]; then
        rm -f "$part"
        die "appimagetool sha256 mismatch (expected $TOOL_SHA256) -- refusing to run it"
    fi
    chmod +x "$part"
    mv "$part" "$TOOL"
}
[[ -x $TOOL ]] || fetch_tool

APPDIR="$(mktemp -d)"
trap 'rm -rf "$APPDIR"' EXIT
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
NAME="TrontSnap-v${VERSION}-x86_64.AppImage"

log "staging AppDir for $VERSION"
install -Dm755 "$BIN" "$APPDIR/usr/bin/trontsnap"
install -Dm644 "$PKG_DIR/trontsnap.desktop" "$APPDIR/usr/share/applications/$DESKTOP_FILE"
install_icons "$APPDIR/usr/share/icons/hicolor" trontsnap
stamp_metainfo "$PKG_DIR/$METAINFO_FILE" "$APPDIR/metainfo.tmp" "$VERSION" "$EPOCH"
# HARD-WON: appimagetool only looks for <id>.appdata.xml; the modern .metainfo.xml
# name makes it warn "AppStream upstream metadata is missing" and skip validation.
install -Dm644 "$APPDIR/metainfo.tmp" "$APPDIR/usr/share/metainfo/$APP_ID.appdata.xml"
rm -f "$APPDIR/metainfo.tmp"
install_docs "$APPDIR/usr/share/doc/trontsnap"
# appimagetool wants the desktop file + icon at the AppDir root.
cp "$APPDIR/usr/share/applications/$DESKTOP_FILE" "$APPDIR/$DESKTOP_FILE"
cp "$APPDIR/usr/share/icons/hicolor/256x256/apps/trontsnap.png" "$APPDIR/trontsnap.png"
ln -s trontsnap.png "$APPDIR/.DirIcon"

# HARD-WON: TrontSnap writes its GNOME keybindings and autostart entry from
# current_exe(), and inside an AppImage that is /tmp/.mount_TrontSxxxxxx/usr/bin/
# trontsnap -- a different path every launch, gone when the app exits. Both would
# be dead on arrival. So AppRun parks a copy of the binary at a fixed path and
# runs THAT; the copy is refreshed (atomic rename, safe while running) whenever
# the AppImage carries a different build, which is how AppImageUpdate upgrades
# reach the keybindings too. A one-line src fix (prefer $APPIMAGE) would also
# work, but then every Print keypress would mount the AppImage first.
cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
BIN="$HERE/usr/bin/trontsnap"
if [ -z "${TRONTSNAP_NO_STABLE_COPY:-}" ] && [ -n "${HOME:-}" ]; then
    DEST_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/trontsnap/bin"
    DEST="$DEST_DIR/trontsnap"
    if ! cmp -s "$BIN" "$DEST" 2>/dev/null; then
        if mkdir -p "$DEST_DIR" 2>/dev/null \
            && cp "$BIN" "$DEST.new.$$" 2>/dev/null \
            && chmod 755 "$DEST.new.$$" \
            && mv -f "$DEST.new.$$" "$DEST"; then
            :
        else
            rm -f "$DEST.new.$$" 2>/dev/null
        fi
    fi
    [ -x "$DEST" ] && BIN="$DEST"
fi
exec "$BIN" "$@"
APPRUN
chmod 755 "$APPDIR/AppRun"
normalize_tree "$APPDIR" "$EPOCH"

# HARD-WON: --appimage-extract-and-run so appimagetool itself needs no FUSE (CI
# containers and some laptops have none). appimagetool drops the .zsync in its
# cwd, hence the cd.
log "running appimagetool -> $OUT/$NAME"
rm -f "$OUT/$NAME" "$OUT/$NAME.zsync"
(cd "$OUT" && ARCH=x86_64 "$TOOL" --appimage-extract-and-run -u "$UPDATE_INFO" "$APPDIR" "$NAME")
[[ -f $OUT/$NAME ]] || die "appimagetool produced no $NAME"

# Prove the update string made it in; it is what self-update hangs on.
GOT="$("$OUT/$NAME" --appimage-updateinformation 2>/dev/null || true)"
[[ $GOT == "$UPDATE_INFO" ]] || die "update information not embedded (got: '${GOT:-nothing}')"

if [[ ! -f $OUT/$NAME.zsync ]] && command -v zsyncmake >/dev/null 2>&1; then
    (cd "$OUT" && zsyncmake -u "$NAME" -o "$NAME.zsync" "$NAME")
fi
if [[ -f $OUT/$NAME.zsync ]]; then
    log "built $OUT/$NAME (+ .zsync)"
else
    # appimagetool 1.9.x bundles zsyncmake; only an older --appimage-tool needs the package.
    MSG="no $NAME.zsync produced, so AppImageUpdate CANNOT upgrade this AppImage (older appimagetool? apt install zsync)."
    ((REQUIRE_ZSYNC)) && die "$MSG"
    warn "$MSG"
    log "built $OUT/$NAME (NO .zsync)"
fi
