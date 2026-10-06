#!/usr/bin/env bash
# Build trontsnap-X.Y.Z-linux-x86_64.tar.gz: binary + desktop entry + icons +
# docs + install.sh/uninstall.sh (installs to ~/.local, no root).
#
# usage: packaging/linux/build-tarball.sh [--version X.Y.Z] [--out DIR]
#   --version  default: [package] version in Cargo.toml
#   --out      default: <repo>/dist
#   env TRONTSNAP_BIN (default target/release/trontsnap), SOURCE_DATE_EPOCH
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

VERSION="$(resolve_version "$VERSION_ARG")"
resolve_bin
EPOCH="$(source_epoch)"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
TOP="trontsnap-${VERSION}-linux-x86_64"
TARBALL="$OUT/$TOP.tar.gz"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
R="$STAGE/$TOP"
log "staging $TOP"

install -Dm755 "$BIN" "$R/bin/trontsnap"
install -Dm644 "$PKG_DIR/trontsnap.desktop" "$R/share/applications/$DESKTOP_FILE"
install_icons "$R/share/icons/hicolor" trontsnap
install_docs "$R/share/doc/trontsnap"
install -m644 "$PKG_DIR/tarball/README.txt" "$R/README.txt"
install -m755 "$PKG_DIR/tarball/install.sh" "$PKG_DIR/tarball/uninstall.sh" "$R/"

find "$R" -type d -exec chmod 755 {} +
normalize_tree "$STAGE" "$EPOCH"

# Fixed order, owner and mtime; gzip -n drops the name/time header. Same inputs
# give a byte-identical tarball.
rm -f "$TARBALL"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$EPOCH" \
    -C "$STAGE" -cf - "$TOP" | gzip -9n > "$TARBALL"
log "built $TARBALL"
