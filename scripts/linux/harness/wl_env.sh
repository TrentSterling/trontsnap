#!/usr/bin/env bash
# wl_env.sh — REAL native-Wayland test rig: a private headless GNOME Shell (mutter 46)
# with its own dbus session, XDG_RUNTIME_DIR, temp HOME and Wayland socket. TrontSnap
# runs in it as a genuine Wayland client (winit-wayland), which Xvfb cannot show you:
# winit's Wayland backend makes set_visible a no-op, ignores set_minimized(false),
# focus_window is empty, and sends NO app_id. All of that is invisible under Xvfb.
#
#   wl_env.sh start [SEED_HOME]   start the shell; SEED_HOME = a gui_env temp home to copy
#                                 (Pictures + settings) into the rig HOME
#   wl_env.sh stop                stop shell + clients by process group, drop its X sockets
#   wl_env.sh status
#   wl_env.sh run CMD...          run CMD as a native-Wayland client (TRONTSNAP_TEST=1, port
#                                 $WL_PORT default 48794, recorder x11, temp HOME)
#   wl_env.sh shot OUT.png        screenshot the shell (gnome-screenshot over the private bus)
#   wl_env.sh input ACTION...     pointer/key injection, see wl_input.py
#
# SAFETY (this laptop: legacy radeon, a UVD hang already hard-reset it once):
#  - headless mutter opens /dev/dri/card1 render-only ("using no mode setting") and the env
#    forces LIBGL_ALWAYS_SOFTWARE=1 (llvmpipe), so no GPU command submission and NO video
#    decode. Still: run `journalctl -k -b --since "-5 min" | grep -i -E "uvd|vce|radeon.*error|hung"`
#    after start, and stop at the first hit.
#  - ~300 MB RSS; not alongside a cargo build + two rigs (6 GB RAM).
#  - XWayland clients do NOT work in it ("Error starting X11 services": no systemd --user on
#    the private bus), so the toast (which strips WAYLAND_DISPLAY) can't be tested here.
#  - The shell starts in the Activities overview: `input key Escape` once, and never pin the
#    pointer to the top-left corner (hot corner reopens the overview; wl_input.py pins bottom-right).
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
W=$OUT/wl
BIN=${TRONTSNAP_BIN:-$REPO/target/debug/trontsnap}
PORT=${WL_PORT:-48794}
die() { echo "wl_env: $*" >&2; exit 1; }
alive() { [[ -n ${1:-} ]] && kill -0 "$1" 2>/dev/null; }

cmd_start() {
  alive "$(cat "$W/pgid" 2>/dev/null)" && die "already running (wl_env.sh stop first)"
  case $W in "$OUT"/wl) ;; *) die "refusing to wipe $W" ;; esac
  rm -rf "$W"; mkdir -p "$W/home/.config/trontsnap" "$W/run"; chmod 700 "$W/run"
  if [[ -n ${1:-} && -d $1 ]]; then cp -a "$1/." "$W/home/"; rm -rf "$W/home/.config/autostart"; fi
  # user-dirs must point at THIS home or captures land in the seed home's folders.
  printf 'XDG_PICTURES_DIR="%s/Pictures"\nXDG_VIDEOS_DIR="%s/Videos"\nXDG_DOCUMENTS_DIR="%s/Documents"\n' \
    "$W/home" "$W/home" "$W/home" > "$W/home/.config/user-dirs.dirs"
  mkdir -p "$W/home/Pictures/TrontSnap" "$W/home/Videos"
  [[ -f $W/home/.config/trontsnap/settings.conf ]] || printf 'HasRun=1\nAutostartInit=1\nRecordAudio=0\n' > "$W/home/.config/trontsnap/settings.conf"
  setsid env -i PATH="$PATH" HOME="$W/home" XDG_RUNTIME_DIR="$W/run" XDG_SESSION_TYPE=wayland \
    XDG_CURRENT_DESKTOP=GNOME XDG_CONFIG_HOME="$W/home/.config" XDG_CACHE_HOME="$W/home/.cache" \
    XDG_DATA_HOME="$W/home/.local/share" LANG=en_US.UTF-8 LIBGL_ALWAYS_SOFTWARE=1 \
    MUTTER_DEBUG_DUMMY_MODE_SPECS=1366x768 \
    dbus-run-session -- bash -c 'echo "$DBUS_SESSION_BUS_ADDRESS" > "$1/dbus.addr"
      exec nice -n 5 gnome-shell --headless --wayland --wayland-display wl-audit --virtual-monitor 1366x768' _ "$W" \
    > "$W/shell.log" 2>&1 < /dev/null &
  echo $! > "$W/pgid"   # setsid leader: its pid is the process-group id
  for _ in $(seq 1 100); do [[ -S $W/run/wl-audit && -s $W/dbus.addr ]] && break; sleep 0.2; done
  [[ -S $W/run/wl-audit ]] || { tail -5 "$W/shell.log" >&2; die "shell did not come up"; }
  sleep 2
  echo "READY wayland=wl-audit dir=$W  (now: journalctl -k -b --since '-2 min' | grep -i -E 'uvd|vce|radeon.*error|hung')"
}

cmd_stop() {
  local pg; pg=$(cat "$W/pgid" 2>/dev/null) || { echo "not running"; return 0; }
  alive "$pg" && kill -TERM -- "-$pg" 2>/dev/null
  for _ in $(seq 1 40); do alive "$pg" || break; sleep 0.1; done
  alive "$pg" && kill -KILL -- "-$pg" 2>/dev/null
  # the shell's own XWayland sockets/locks (numbers are in its log) — only if no live owner
  local n
  for n in $(grep -o -E 'X11 display :[0-9]+|using :[0-9]+ for managed' "$W/shell.log" 2>/dev/null | grep -o -E ':[0-9]+' | tr -d ':'); do
    local lp; lp=$(cat "/tmp/.X$n-lock" 2>/dev/null | tr -d ' ')
    alive "$lp" || rm -f "/tmp/.X11-unix/X$n" "/tmp/.X$n-lock"
  done
  rm -f "$W/pgid"; echo "stopped"
}

cmd_status() { alive "$(cat "$W/pgid" 2>/dev/null)" && echo "up (pgid $(cat "$W/pgid"))" || echo "down"; }

cmd_run() {
  [[ -s $W/dbus.addr ]] || die "not started"
  exec env -i PATH="$PATH" HOME="$W/home" XDG_RUNTIME_DIR="$W/run" WAYLAND_DISPLAY=wl-audit \
    DBUS_SESSION_BUS_ADDRESS="$(cat "$W/dbus.addr")" XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=GNOME \
    XDG_CONFIG_HOME="$W/home/.config" XDG_CACHE_HOME="$W/home/.cache" XDG_DATA_HOME="$W/home/.local/share" \
    LANG=en_US.UTF-8 LIBGL_ALWAYS_SOFTWARE=1 RUST_BACKTRACE=1 TRONTSNAP_TEST=1 TRONTSNAP_PORT="$PORT" \
    TRONTSNAP_RECORDER=x11 "$@"
}

case ${1:-} in
  start)  shift; cmd_start "$@" ;;
  stop)   cmd_stop ;;
  status) cmd_status ;;
  run)    shift; [[ ${1:-} == target/* ]] && set -- "$REPO/$1" "${@:2}"; cmd_run "$@" ;;
  shot)   shift; [[ -n ${1:-} ]] || die "usage: shot OUT.png"; out=$(realpath -m "$1")
          "$0" run gnome-screenshot -f "$out" && echo "$out" ;;
  input)  shift; exec python3 "$HERE/wl_input.py" "$W" "$@" ;;
  *) sed -n '2,25p' "${BASH_SOURCE[0]}"; exit 2 ;;
esac
