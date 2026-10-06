#!/usr/bin/env bash
# Build every Linux release artifact from an already-built binary.
#
# usage: packaging/linux/build-all.sh --version X.Y.Z --out dist/ [--appimage-tool PATH]
#                                     [--require-zsync] [--allow-version-mismatch]
#
# Expects target/release/trontsnap (or $TRONTSNAP_BIN) -- it never runs cargo.
# Writes into --out (default <repo>/dist):
#   TrontSnap-vX.Y.Z-x86_64.AppImage           (+ .zsync, so existing AppImages self-update)
#   trontsnap_X.Y.Z-1_amd64.deb
#   trontsnap-X.Y.Z-linux-x86_64.tar.gz
#   SHA256SUMS                                 (covers exactly the files above)
#
# --version must equal Cargo.toml's version: the binary has that baked in, and a
# release whose file names disagree with `trontsnap --version`-style reality is
# the kind of mistake you only notice after the upgrade path breaks.
# --require-zsync turns a missing .zsync into a failure (CI should pass it).
# --appimage-tool PATH is passed through to appimage.sh (downloaded there if absent).
#
# CI notes: needs curl (appimagetool download), dpkg-deb, tar, gzip, python3 for
# icons (or ImageMagick). Build the binary on the OLDEST Ubuntu you mean to
# support: the .deb's libc6 floor and the AppImage's reach both follow the
# builder's glibc.
set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

VERSION_ARG="" OUT="$REPO_ROOT/dist" TOOL_ARGS=() ZSYNC_ARGS=() ALLOW_MISMATCH=0
while (($#)); do
    case $1 in
        --version)       VERSION_ARG="${2:?--version needs a value}"; shift 2 ;;
        --out)           OUT="${2:?--out needs a value}"; shift 2 ;;
        --appimage-tool) TOOL_ARGS=(--appimage-tool "${2:?--appimage-tool needs a value}"); shift 2 ;;
        --require-zsync) ZSYNC_ARGS=(--require-zsync); shift ;;
        --allow-version-mismatch) ALLOW_MISMATCH=1; shift ;;
        -h|--help)       sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

VERSION="$(resolve_version "$VERSION_ARG")"
CARGO_VERSION="$(cargo_version)"
if [[ $VERSION != "$CARGO_VERSION" ]] && ((!ALLOW_MISMATCH)); then
    die "--version $VERSION != Cargo.toml $CARGO_VERSION (bump Cargo.toml and rebuild, or pass --allow-version-mismatch)"
fi
resolve_bin
EPOCH="$(source_epoch)"
export SOURCE_DATE_EPOCH="$EPOCH"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

# === metadata gates (hard failures; "skipped" when the validator isn't installed) ===
log "validating desktop entry + AppStream metadata"
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$PKG_DIR/trontsnap.desktop"
else
    warn "desktop-file-validate not installed -- desktop entry NOT validated (apt install desktop-file-utils)"
fi
if command -v appstreamcli >/dev/null 2>&1; then
    CHECK="$(mktemp -d)"
    stamp_metainfo "$PKG_DIR/$METAINFO_FILE" "$CHECK/$METAINFO_FILE" "$VERSION" "$EPOCH"
    appstreamcli validate --no-net "$CHECK/$METAINFO_FILE"
    rm -rf "$CHECK"
else
    warn "appstreamcli not installed -- metainfo NOT validated (apt install appstream)"
fi

# === artifacts ===
"$PKG_DIR/appimage.sh" --version "$VERSION" --out "$OUT" ${TOOL_ARGS[@]+"${TOOL_ARGS[@]}"} ${ZSYNC_ARGS[@]+"${ZSYNC_ARGS[@]}"}
"$PKG_DIR/build-deb.sh" --version "$VERSION" --out "$OUT"
"$PKG_DIR/build-tarball.sh" --version "$VERSION" --out "$OUT"

# === checksums ===
ARTIFACTS=()
for f in "TrontSnap-v${VERSION}-x86_64.AppImage" "TrontSnap-v${VERSION}-x86_64.AppImage.zsync" \
    "trontsnap_${VERSION//-/\~}-${DEB_REVISION:-1}_amd64.deb" "trontsnap-${VERSION}-linux-x86_64.tar.gz"; do
    [[ -f $OUT/$f ]] && ARTIFACTS+=("$f")
done
(cd "$OUT" && LC_ALL=C sha256sum "${ARTIFACTS[@]}" > SHA256SUMS)
log "done: $OUT"
cat "$OUT/SHA256SUMS" >&2
[[ -f $OUT/TrontSnap-v${VERSION}-x86_64.AppImage.zsync ]] \
    || warn "no .zsync in the output: AppImage self-update will NOT work for this release"
