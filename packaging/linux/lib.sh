# shellcheck shell=bash
# Shared helpers for the packaging scripts. Source it, don't run it.
#
# Env knobs every script honors:
#   TRONTSNAP_BIN      binary to package (default: target/release/trontsnap). Tests
#                      point this at a stand-in so nothing here ever needs cargo.
#   SOURCE_DATE_EPOCH  mtime for everything staged (default: HEAD commit time).

PKG_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$PKG_DIR/../.." && pwd)"
REPO_URL="https://github.com/TrentSterling/trontsnap"
APP_ID="xyz.tront.TrontSnap"
# shellcheck disable=SC2034  # read by the scripts that source this file
DESKTOP_FILE="$APP_ID.desktop"
# shellcheck disable=SC2034
METAINFO_FILE="$APP_ID.metainfo.xml"

log()  { printf '==> %s\n' "$*" >&2; }
warn() { printf '!!  WARNING: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

# Version of the [package] table. The root Cargo.toml opens with a [workspace]
# table and a BOM, so a bare `grep ^version` is a trap; read the right table.
cargo_version() {
    awk '/^\[package\]/ {p=1; next} /^\[/ {p=0}
         p && /^version[ \t]*=/ {sub(/^version[ \t]*=[ \t]*"/, ""); sub(/".*/, ""); print; exit}' \
        "$REPO_ROOT/Cargo.toml"
}

# resolve_version [REQUESTED] -> prints X.Y.Z (a leading "v" is accepted)
resolve_version() {
    local v="${1:-}"
    [[ -n $v ]] || v="$(cargo_version)"
    v="${v#v}"
    [[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+~.][0-9A-Za-z.+~-]+)?$ ]] \
        || die "unusable version '$v' (want X.Y.Z)"
    printf '%s' "$v"
}

# Sets BIN. Refuses a non-x86-64 ELF; a non-ELF is allowed (test stand-ins) but loud.
resolve_bin() {
    BIN="${TRONTSNAP_BIN:-$REPO_ROOT/target/release/trontsnap}"
    [[ -f $BIN && -x $BIN ]] \
        || die "no executable at $BIN -- run 'cargo build --release' first (or set TRONTSNAP_BIN)"
    if command -v readelf >/dev/null 2>&1 && readelf -h "$BIN" >/dev/null 2>&1; then
        readelf -h "$BIN" | grep -q 'X86-64' || die "$BIN is not an x86-64 ELF"
    else
        warn "$BIN is not an ELF binary (stand-in?) -- fine for tests, never ship this"
    fi
}

source_epoch() {
    if [[ -n ${SOURCE_DATE_EPOCH:-} ]]; then
        printf '%s' "$SOURCE_DATE_EPOCH"
    elif git -C "$REPO_ROOT" log -1 --format=%ct >/dev/null 2>&1; then
        git -C "$REPO_ROOT" log -1 --format=%ct
    else
        date +%s
    fi
}

# Highest GLIBC_x.y symbol version the binary needs, e.g. "2.39". Empty if it
# can't tell. This is the real floor for "which distro can run this".
glibc_min() {
    command -v readelf >/dev/null 2>&1 || return 0
    { readelf -sW --dyn-syms "$1" 2>/dev/null | grep -o 'GLIBC_[0-9][0-9.]*' \
        | sed 's/^GLIBC_//' | sort -uV | tail -n1; } || true
}

# install_icons HICOLOR_ROOT NAME -> HICOLOR_ROOT/<n>x<n>/apps/NAME.png
# Source of truth is assets/icon.ico (PNG frames, 16..256), so ImageMagick is
# optional. Fallbacks keep the build alive but say what they cost.
install_icons() {
    local root=$1 name=$2 tmp sz f
    tmp="$(mktemp -d)"
    if command -v python3 >/dev/null 2>&1 \
        && python3 -I "$PKG_DIR/ico-extract.py" "$REPO_ROOT/assets/icon.ico" "$tmp"; then
        :
    elif command -v convert >/dev/null 2>&1; then
        for sz in 16 24 32 48 64 128 256; do
            convert "$REPO_ROOT/assets/trontsnap.png" -resize "${sz}x${sz}" "$tmp/$sz.png"
        done
    else
        warn "no python3 or ImageMagick: shipping the 256px icon only"
        cp "$REPO_ROOT/assets/trontsnap.png" "$tmp/256.png"
    fi
    for f in "$tmp"/*.png; do
        sz="$(basename "$f" .png)"
        install -Dm644 "$f" "$root/${sz}x${sz}/apps/$name.png"
    done
    rm -rf "$tmp"
}

# stamp_metainfo SRC DST VERSION EPOCH
# Adds a <release> for VERSION when the file doesn't list it yet, so a release
# build never ships metainfo that claims a stale latest version.
stamp_metainfo() {
    local src=$1 dst=$2 ver=$3 day
    day="$(date -u -d "@$4" +%F)"
    if grep -q "<release version=\"$ver\"" "$src"; then
        cp "$src" "$dst"
    else
        awk -v v="$ver" -v d="$day" -v url="$REPO_URL" '
            { print }
            /^[ \t]*<releases>[ \t]*$/ && !n {
                printf "    <release version=\"%s\" date=\"%s\">\n", v, d
                printf "      <url type=\"details\">%s/releases/tag/v%s</url>\n", url, v
                printf "    </release>\n"
                n = 1
            }' "$src" > "$dst"
    fi
}

# normalize_tree DIR EPOCH: one mtime for every file/dir (reproducible archives).
normalize_tree() {
    find "$1" -exec touch -h -d "@$2" {} +
}

# Shared payload: license/notice files every artifact carries.
install_docs() {
    local dest=$1
    install -Dm644 "$PKG_DIR/copyright" "$dest/copyright"
    install -Dm644 "$REPO_ROOT/assets/fonts/OFL.txt" "$dest/Rajdhani-OFL.txt"
}
