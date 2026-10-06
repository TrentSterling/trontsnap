#!/usr/bin/env bash
# Build trontsnap_X.Y.Z-1_amd64.deb with plain dpkg-deb (no cargo-deb, no debhelper).
#
# usage: packaging/linux/build-deb.sh [--version X.Y.Z] [--out DIR]
#   --version  default: [package] version in Cargo.toml (0.26.0-rc1 -> 0.26.0~rc1-1)
#   --out      default: <repo>/dist
#   env TRONTSNAP_BIN       binary to package (default target/release/trontsnap)
#       DEB_REVISION        Debian revision, default 1 (bump for packaging-only fixes)
#       DEB_MAINTAINER      default: the repo author
#       SOURCE_DATE_EPOCH   pins mtimes (default: HEAD commit time)
#
# Installed layout:  /usr/bin/trontsnap  (the path the app bakes into its GNOME
# keybindings + autostart entry, so it must not move between releases)
#                    /usr/share/{applications,icons/hicolor,metainfo,doc/trontsnap}
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

VERSION_ARG="" OUT="$REPO_ROOT/dist"
while (($#)); do
    case $1 in
        --version) VERSION_ARG="${2:?--version needs a value}"; shift 2 ;;
        --out)     OUT="${2:?--out needs a value}"; shift 2 ;;
        -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

command -v dpkg-deb >/dev/null 2>&1 || die "dpkg-deb not found (apt install dpkg)"
VERSION="$(resolve_version "$VERSION_ARG")"
resolve_bin
EPOCH="$(source_epoch)"
export SOURCE_DATE_EPOCH="$EPOCH"   # dpkg-deb clamps member mtimes to this
DEB_VERSION="${VERSION//-/\~}-${DEB_REVISION:-1}"
MAINTAINER="${DEB_MAINTAINER:-Trent Sterling <trentster222@gmail.com>}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
DEB="$OUT/trontsnap_${DEB_VERSION}_amd64.deb"

ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
log "staging $DEB_VERSION"

install -Dm755 "$BIN" "$ROOT/usr/bin/trontsnap"
install -Dm644 "$PKG_DIR/trontsnap.desktop" "$ROOT/usr/share/applications/$DESKTOP_FILE"
install_icons "$ROOT/usr/share/icons/hicolor" trontsnap
stamp_metainfo "$PKG_DIR/$METAINFO_FILE" "$ROOT/metainfo.tmp" "$VERSION" "$EPOCH"
install -Dm644 "$ROOT/metainfo.tmp" "$ROOT/usr/share/metainfo/$METAINFO_FILE"
rm -f "$ROOT/metainfo.tmp"
DOC="$ROOT/usr/share/doc/trontsnap"
install_docs "$DOC"
{
    printf 'trontsnap (%s) unstable; urgency=medium\n\n' "$DEB_VERSION"
    printf '  * Packaged from upstream release v%s.\n' "$VERSION"
    printf '    Release notes: %s/releases/tag/v%s\n\n' "$REPO_URL" "$VERSION"
    printf ' -- %s  %s\n' "$MAINTAINER" "$(LC_ALL=C date -u -d "@$EPOCH" '+%a, %d %b %Y %H:%M:%S +0000')"
} | gzip -9n > "$DOC/changelog.Debian.gz"

# === control ===
# HARD-WON: dpkg-shlibdeps can't see dlopen()ed libs, and winit/glutin/xcursor
# dlopen almost everything (found by strings on the binary: libwayland-client,
# libwayland-egl, libxkbcommon(-x11), libXcursor, libXrender, libX11-xcb, EGL/GL).
# Without them listed the app dies at window creation on a minimal install.
GLIBC="$(glibc_min "$BIN")"
LIBC_DEP="libc6${GLIBC:+ (>= $GLIBC)}"
[[ -n $GLIBC ]] || warn "couldn't read the glibc floor from $BIN; libc6 dependency is unversioned"
INSTALLED_KB="$(find "$ROOT" -type f -printf '%s\n' | awk '{s += $1} END {print int((s + 1023) / 1024)}')"
mkdir -p "$ROOT/DEBIAN"
cat > "$ROOT/DEBIAN/control" <<CONTROL
Package: trontsnap
Version: $DEB_VERSION
Architecture: amd64
Maintainer: $MAINTAINER
Installed-Size: $INSTALLED_KB
Depends: $LIBC_DEP, libgcc-s1, libxcb1, libx11-6, libx11-xcb1, libxcursor1, libxrender1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-egl1, libegl1, libgl1, ffmpeg, xclip | wl-clipboard
Recommends: gnome-screenshot | grim | spectacle, tesseract-ocr, wl-clipboard
Suggests: libnotify-bin
Section: graphics
Priority: optional
Homepage: $REPO_URL
Description: fast screenshot tool with a searchable capture gallery
 TrontSnap grabs the screen or a dragged region straight to the clipboard,
 saves a PNG, and keeps a live gallery over your whole capture history.
 .
 It also has an annotation editor with real redaction, offline OCR (copy the
 text out of any region, needs tesseract-ocr), and screen recording. On GNOME
 it installs Print / Ctrl+Print / Ctrl+Shift+Print / Ctrl+Alt+Print
 keybindings on first launch.
CONTROL
for s in postinst prerm postrm; do install -m755 "$PKG_DIR/deb/$s" "$ROOT/DEBIAN/$s"; done

# === normalise ===
find "$ROOT" -type d -exec chmod 755 {} +
find "$ROOT" -type f ! -perm /111 -exec chmod 644 {} +
chmod 644 "$ROOT/DEBIAN/control"
normalize_tree "$ROOT" "$EPOCH"

rm -f "$DEB"
dpkg-deb --root-owner-group -Zxz --build "$ROOT" "$DEB" >/dev/null
log "built $DEB"
dpkg-deb -f "$DEB" Package Version Architecture Depends >&2

if command -v lintian >/dev/null 2>&1; then
    lintian --no-tag-display-limit "$DEB" >&2 || warn "lintian reported problems (informational: this is a third-party deb)"
else
    log "lintian not installed -- skipped"
fi
