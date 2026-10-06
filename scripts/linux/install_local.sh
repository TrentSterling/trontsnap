#!/usr/bin/env bash
# install_local.sh — stable per-user install: target/release/trontsnap -> ~/.local/bin/trontsnap,
# then restart the LIVE app on it. Build first (cargo build --release); this never builds.
#
#   scripts/linux/install_local.sh [--no-restart]
#
# Why: the GNOME keybindings and the autostart entry point at whatever binary the app was started
# from, and every `cargo build` replaces target/debug/trontsnap under them. Installed path = stable.
# Env: TRONTSNAP_INSTALL (dest, default ~/.local/bin/trontsnap), TRONTSNAP_PORT (live port, 48761).
set -eu
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
SRC=$REPO/target/release/trontsnap
DST=${TRONTSNAP_INSTALL:-$HOME/.local/bin/trontsnap}
PORT=${TRONTSNAP_PORT:-48761}
LOG=$HOME/.cache/trontsnap/app.log
die() { echo "install_local: $*" >&2; exit 1; }
[[ -x $SRC ]] || die "no $SRC (cargo build --release first)"

# A live recording owns a kmsgrab + audio sidecar: stopping the app under it orphans both.
if [[ -e $HOME/.cache/trontsnap/rec/session.marker ]]; then die "a recording is in progress; stop it first"; fi

# === Copy ===
mkdir -p "$(dirname "$DST")" "$(dirname "$LOG")"
if [[ -e $DST ]]; then
  bak=$DST.$(date +%Y%m%d-%H%M%S).bak
  cp -p "$DST" "$bak" && echo "backup: $bak"
fi
# HARD-WON: rename, not cp-over: a running binary can't be opened for writing (ETXTBSY), a rename can.
install -m755 "$SRC" "$DST.new" && mv -f "$DST.new" "$DST"
echo "installed: $DST ($(stat -c %s "$DST") bytes)"

# === Autostart ===
# HARD-WON: the app writes the autostart entry exactly once (AutostartInit), unlike the keybindings it
# rewrites every launch, so an old entry keeps pointing at the old binary until someone fixes it.
AUTO=$HOME/.config/autostart/trontsnap.desktop
if [[ -f $AUTO ]] && ! grep -qF "\"$DST\"" "$AUTO"; then
  sed -i "s|^Exec=.*|Exec=\"$DST\" --startup|" "$AUTO" && echo "autostart: $AUTO -> $DST"
fi

[[ ${1:-} == --no-restart ]] && exit 0

# === Restart ===
# The live app is whoever listens on the command port (a harness rig never uses 48761).
pid=$(ss -ltnp "sport = :$PORT" 2>/dev/null | sed -n 's/.*pid=\([0-9]*\).*/\1/p' | head -1)
if [[ -n $pid ]]; then
  kill -TERM "$pid"
  for _ in $(seq 1 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  kill -0 "$pid" 2>/dev/null && die "pid $pid ignored SIGTERM; not touching it further"
fi
cd "$HOME"
setsid "$DST" --startup > "$LOG" 2>&1 < /dev/null &
for _ in $(seq 1 100); do ss -ltn "sport = :$PORT" 2>/dev/null | grep -q LISTEN && break; sleep 0.2; done
ss -ltn "sport = :$PORT" 2>/dev/null | grep -q LISTEN || die "app did not come back on port $PORT (log: $LOG)"
echo "live app restarted from $DST (was pid ${pid:-none}, log $LOG)"
