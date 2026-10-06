#!/usr/bin/env bash
# usage: check-assets.sh DIR NAME_OR_GLOB...
# Each argument must match exactly ONE non-empty file in DIR. Reports every miss, then
# exits 1. A glob (not a fixed name) is for names with a part that build scripts own,
# e.g. the .deb's Debian revision: trontsnap_X.Y.Z-1_amd64.deb.
#
# HARD-WON: the release workflow once hard-coded trontsnap_X.Y.Z_amd64.deb while
# build-all.sh writes ...-1_amd64.deb, so the Linux job went red after packaging worked.
set -euo pipefail

dir="${1:?usage: check-assets.sh DIR NAME_OR_GLOB...}"
shift

bad=0
for pat in "$@"; do
    n=0
    # compgen -G prints the matches (and fails on none); process substitution keeps
    # the counter in this shell.
    while IFS= read -r m; do
        if [ -s "$m" ]; then n=$((n + 1)); fi
    done < <(compgen -G "$dir/$pat" || true)
    if [ "$n" -ne 1 ]; then
        bad=1
        msg="$n non-empty files match $dir/$pat (want exactly 1)"
        if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::error::$msg"; else echo "ERROR: $msg" >&2; fi
    fi
done
exit "$bad"
