#!/usr/bin/env bash
# wincheck.sh — compile-check the WINDOWS side of this tree from the Linux host.
#
#   wincheck.sh                      # = --workspace --bins --tests (trontsnap + trontsnap-hotkeys)
#   wincheck.sh -p trontsnap --bins  # any `cargo check` args replace the default set
#
# How it works: `cargo check --target x86_64-pc-windows-gnu` (rust-std for it is
# `rustup target add x86_64-pc-windows-gnu`; NO mingw/linker needed, check never links).
# build.rs/hotkeyd/build.rs call winresource, which shells out to windres + ar: a fake
# windres (an empty .o) and the host ar satisfy it, so no build.rs edit is needed.
# Own CARGO_TARGET_DIR so it never takes the lock on target/ (Trent's live binary).
# Proven non-vacuous 2026-10-05: an error injected into a cfg(windows) fn is caught
# (src/app.rs restore_and_foreground), one injected into a cfg(not(windows)) fn is not seen.
# Checks that it COMPILES; it cannot run anything, and it is the gnu ABI not msvc.
# HARD-WON: ~1.3 GB target dir, ~10 min cold / ~1.5 min warm on the G50-45. One cargo at a time (6 GB RAM).
set -eu
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
mkdir -p "$OUT"
FAKE=$OUT/fake_windres.sh
cat > "$FAKE" <<'EOS'
#!/bin/sh
# last arg = output .o; an empty file is enough for a check-only build
for a; do last="$a"; done
: > "$last"
EOS
chmod +x "$FAKE"
export WINDRES=$FAKE AR=/usr/bin/ar
export CARGO_TARGET_DIR=${WINCHECK_TARGET:-$OUT/wincheck-target}
cd "$REPO"
(( $# )) || set -- --workspace --bins --tests
exec nice -n 19 cargo check --offline --locked --target x86_64-pc-windows-gnu -j2 "$@"
