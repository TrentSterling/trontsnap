#!/usr/bin/env bash
# Release gate: tag == root Cargo.toml == hotkeyd/Cargo.toml == Cargo.lock, and
# CHANGELOG.md has a '## vX.Y.Z' section, which becomes the release notes.
#
# Usage (from the repo root): .github/scripts/verify-release.sh vX.Y.Z [notes-out]
# Reports every mismatch before exiting nonzero so one run shows all the problems.
set -euo pipefail

TAG="${1:-}"
NOTES_OUT="${2:-release-notes.md}"
ERRS=0

err() {
    ERRS=$((ERRS + 1))
    if [ -n "${GITHUB_ACTIONS:-}" ]; then
        echo "::error title=Release verify failed::$*"
    else
        echo "ERROR: $*" >&2
    fi
}

# HARD-WON: both Cargo.toml files start with a UTF-8 BOM. hotkeyd's '[package]'
# header is line 1, so without the strip it never matches and the version reads
# back empty (which then reads as "no version", not "BOM").
toml_version() {
    sed '1s/^\xEF\xBB\xBF//' "$1" | tr -d '\r' | awk '
        /^\[/ { inpkg = ($0 ~ /^\[package\][ \t]*(#.*)?$/); next }
        inpkg && /^version[ \t]*=/ {
            v = $0; sub(/^version[ \t]*=[ \t]*"/, "", v); sub(/".*$/, "", v); print v; exit
        }'
}

# Cargo.lock lists a package as name = "x" then version = "y" in one block.
lock_version() {
    tr -d '\r' < Cargo.lock | awk -v want="$1" '
        /^\[\[package\]\]/ { name = ""; next }
        /^name = / { n = $0; sub(/^name = "/, "", n); sub(/".*$/, "", n); name = n; next }
        /^version = / && name == want {
            v = $0; sub(/^version = "/, "", v); sub(/".*$/, "", v); print v; exit
        }'
}

if [[ ! "$TAG" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
    err "tag '${TAG}' is not vMAJOR.MINOR.PATCH (e.g. v0.26.0)"
    exit 1
fi
VERSION="${BASH_REMATCH[1]}"

for f in Cargo.toml hotkeyd/Cargo.toml Cargo.lock CHANGELOG.md; do
    if [ ! -f "$f" ]; then
        err "$f not found; run this from the repo root"
        exit 1
    fi
done

check() { # label, actual
    if [ "$2" = "$VERSION" ]; then
        echo "ok: $1 = $2"
    else
        err "$1 is '${2:-<not found>}' but the tag says $VERSION"
    fi
}
check "Cargo.toml [package] version"          "$(toml_version Cargo.toml)"
check "hotkeyd/Cargo.toml [package] version"  "$(toml_version hotkeyd/Cargo.toml)"
check "Cargo.lock trontsnap"                  "$(lock_version trontsnap)"
check "Cargo.lock trontsnap-hotkeys"          "$(lock_version trontsnap-hotkeys)"

# Section = everything under '## vX.Y.Z' up to the next '## ' heading, blank
# edges trimmed. String compare, not regex: the dots in the tag are literal.
mkdir -p "$(dirname "$NOTES_OUT")"
if tr -d '\r' < CHANGELOG.md | awk -v tag="$TAG" '
        BEGIN { h = "## " tag }
        /^## / {
            if (found) exit
            if ($0 == h || index($0, h " ") == 1) found = 1
            next
        }
        found { lines[++n] = $0 }
        END {
            if (!found) exit 3
            s = 1; while (s <= n && lines[s] ~ /^[ \t]*$/) s++
            e = n; while (e >= s && lines[e] ~ /^[ \t]*$/) e--
            for (i = s; i <= e; i++) print lines[i]
        }' > "$NOTES_OUT"; then
    if [ -s "$NOTES_OUT" ]; then
        echo "ok: CHANGELOG.md has '## $TAG' ($(wc -l < "$NOTES_OUT") lines -> $NOTES_OUT)"
    else
        err "CHANGELOG.md has a '## $TAG' heading but the section is empty"
    fi
else
    err "CHANGELOG.md has no '## $TAG' section"
fi

if [ "$ERRS" -ne 0 ]; then
    echo "Release verify FAILED ($ERRS problem(s)). Bump the version in Cargo.toml and hotkeyd/Cargo.toml," >&2
    echo "refresh Cargo.lock, add '## $TAG' to CHANGELOG.md, commit, then re-tag." >&2
    exit 1
fi

if [ -n "${GITHUB_OUTPUT:-}" ]; then
    echo "version=$VERSION" >> "$GITHUB_OUTPUT"
fi
echo "Release verify OK: $TAG"
