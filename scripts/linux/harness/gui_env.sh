#!/usr/bin/env bash
# gui_env.sh — isolated TrontSnap GUI rig: own Xvfb, own dbus session, own temp
# HOME, own command port. Nothing it does can reach the live session.
#
#   gui_env.sh start  N PORT [--no-guard] [--bare] [--recorder x11|gnome|kms|hook]
#   gui_env.sh stop   N
#   gui_env.sh status [N]
#   gui_env.sh run    N CMD...      run CMD inside the rig's env (DISPLAY, HOME, dbus)
#   gui_env.sh reseed N             re-seed the rig's Pictures/TrontSnap (app must be stopped)
#
# N >= 10 (display :N; :0/:1 are the real session). PORT != 48761 (the live app).
# --no-guard : don't set TRONTSNAP_TEST=1 (used once to prove the private dbus +
#              temp HOME isolate gsettings/autostart even without the guard).
# --bare     : empty Pictures/TrontSnap instead of the seeded 12 PNG + 1 mp4.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
BIN=${TRONTSNAP_BIN:-$REPO/target/debug/trontsnap}
SCREEN=${RIG_SCREEN:-1366x768x24}

die() { echo "gui_env: $*" >&2; exit 1; }

check_n() {
  [[ ${1:-} =~ ^[0-9]+$ ]] || die "display number N required"
  (( $1 >= 10 )) || die "N must be >= 10 (low numbers are the real session)"
}
rdir() { echo "$OUT/rigs/$1"; }
hdir() { echo "$OUT/homes/$1"; }
alive() { [[ -n ${1:-} ]] && kill -0 "$1" 2>/dev/null; }
port_listening() { ss -ltn "sport = :$1" 2>/dev/null | grep -q LISTEN; }

# === Seed ===
# ~12 real PNG copies (cycled from ~/Pictures/TrontSnap, mtimes spread 90 min
# apart so the timeline has several groups) + one real mp4.
seed_home() {
  local h; h=$(hdir "$1")
  case $h in "$OUT"/homes/*) ;; *) die "refusing to wipe $h" ;; esac
  rm -rf "$h"
  mkdir -p "$h/Pictures/TrontSnap" "$h/Videos" "$h/.config/trontsnap" "$h/.cache"
  # HARD-WON: dirs::picture_dir() reads ONLY user-dirs.dirs on Linux; without it
  # captures fall back to $HOME/TrontSnap and the seed folder is never watched.
  printf 'XDG_PICTURES_DIR="%s/Pictures"\nXDG_VIDEOS_DIR="%s/Videos"\nXDG_DOCUMENTS_DIR="%s/Documents"\n' "$h" "$h" "$h" \
    > "$h/.config/user-dirs.dirs"
  # HasRun=1 opens on Gallery instead of the first-run About tab.
  printf 'HasRun=1\nAutostartInit=1\nRecordAudio=0\n' > "$h/.config/trontsnap/settings.conf"
  [[ ${BARE:-0} == 1 ]] && return 0
  local pngs=() p i t now dst
  while IFS= read -r p; do pngs+=("$p"); done < <(find "$HOME/Pictures/TrontSnap" -maxdepth 1 -name '*.png' | sort)
  (( ${#pngs[@]} > 0 )) || die "no source PNGs in ~/Pictures/TrontSnap to seed from"
  now=$(date +%s)
  for i in $(seq 1 12); do
    t=$(( now - i * 5400 ))
    dst="$h/Pictures/TrontSnap/TrontSnap_$(date -d "@$t" +%F_%H-%M-%S).png"
    cp "${pngs[$(( (i - 1) % ${#pngs[@]} ))]}" "$dst"
    touch -d "@$t" "$dst"
  done
  local mp4
  mp4=$(ls -S "$HOME"/Pictures/TrontSnap/*.mp4 "$HOME"/Videos/captures/*.mp4 2>/dev/null \
        | while read -r f; do (( $(stat -c %s "$f") < 3000000 )) && { echo "$f"; break; }; done)
  if [[ -n $mp4 ]]; then
    t=$(( now - 600 ))
    dst="$h/Pictures/TrontSnap/TrontSnap_$(date -d "@$t" +%F_%H-%M-%S).mp4"
    cp "$mp4" "$dst"; touch -d "@$t" "$dst"
  else
    echo "gui_env: WARNING no mp4 under 3MB to seed" >&2
  fi
}

cmd_start() {
  local n=${1:-}; local port=${2:-}; shift 2 2>/dev/null
  check_n "$n"
  [[ $port =~ ^[0-9]+$ ]] || die "PORT required (usage: start N PORT)"
  [[ $port != 48761 ]] || die "48761 is the live app's port"
  local guard=1 recorder=""
  BARE=0
  while (( $# )); do
    case $1 in
      --no-guard) guard=0 ;;
      --bare) BARE=1 ;;
      --recorder) recorder=$2; shift ;;
      *) die "unknown option $1" ;;
    esac
    shift
  done
  [[ -x $BIN ]] || die "no binary at $BIN (cargo build first)"
  local r; r=$(rdir "$n"); mkdir -p "$r"
  if alive "$(cat "$r/app.pid" 2>/dev/null)" || alive "$(cat "$r/xvfb.pid" 2>/dev/null)"; then
    die "rig :$n is already running (gui_env.sh stop $n first)"
  fi
  port_listening "$port" && die "port $port is already in use"
  [[ -e /tmp/.X11-unix/X$n ]] && die "display :$n is taken (stale socket? stop it or pick another N)"
  rm -f "$r"/*.pid "$r"/*.log "$r/dbus.addr"
  echo "$port" > "$r/port"

  # --- Xvfb ---
  setsid Xvfb ":$n" -screen 0 "$SCREEN" -nolisten tcp -noreset > "$r/xvfb.log" 2>&1 < /dev/null &
  echo $! > "$r/xvfb.pid"
  for _ in $(seq 1 50); do [[ -S /tmp/.X11-unix/X$n ]] && break; sleep 0.1; done
  [[ -S /tmp/.X11-unix/X$n ]] || { cat "$r/xvfb.log" >&2; die "Xvfb :$n did not come up"; }

  seed_home "$n"
  local h; h=$(hdir "$n")

  # --- App, under its own dbus session. The inner bash records the REAL app pid
  # (exec replaces it) and the private bus address so `run` can join the session.
  local envs=(DISPLAY=":$n" HOME="$h" TRONTSNAP_PORT="$port" XDG_SESSION_TYPE=x11
              LIBGL_ALWAYS_SOFTWARE=1 RUST_BACKTRACE=1)
  (( guard )) && envs+=(TRONTSNAP_TEST=1)
  [[ -n $recorder ]] && envs+=(TRONTSNAP_RECORDER="$recorder")
  setsid env -u WAYLAND_DISPLAY -u XAUTHORITY -u XDG_CONFIG_HOME -u XDG_CACHE_HOME -u XDG_DATA_HOME \
      -u SESSION_MANAGER -u GNOME_DESKTOP_SESSION_ID "${envs[@]}" \
      dbus-run-session -- bash -c '
        echo $$ > "$1/app.pid"; echo "$DBUS_SESSION_BUS_ADDRESS" > "$1/dbus.addr"
        cd "$2"; exec "$3"' _ "$r" "$h" "$BIN" \
      > "$r/app.log" 2>&1 < /dev/null &
  echo $! > "$r/session.pid"

  # --- Ready: port bound AND the window mapped (debug build + llvmpipe is slow).
  for _ in $(seq 1 300); do
    port_listening "$port" && DISPLAY=":$n" xwininfo -root -tree 2>/dev/null | grep -q '"TrontSnap"' && break
    # app.pid appears a beat after launch (dbus-run-session starts first).
    if [[ -s $r/app.pid ]] && ! alive "$(cat "$r/app.pid")"; then
      tail -20 "$r/app.log" >&2; die "app died during startup (log: $r/app.log)"
    fi
    sleep 0.2
  done
  port_listening "$port" || { tail -20 "$r/app.log" >&2; die "app never bound port $port"; }
  sleep 1.5   # first frame + gallery scan
  # HARD-WON: no WM on Xvfb = nothing is ever focused, and egui then drops keys and
  # Ctrl/Shift state. Focus the window once here (xinput.py also self-heals).
  "$HERE/xinput.py" -d ":$n" focus >/dev/null 2>&1 || echo "gui_env: WARNING could not focus the TrontSnap window" >&2
  # Thumbnails decode lazily; READY means the first screen is fully painted (shots
  # taken earlier lie in pixel diffs). Pointer parked off the tiles first.
  "$HERE/xinput.py" -d ":$n" move 1270 700 >/dev/null 2>&1
  "$HERE/shot.sh" "$n" "$r/ready.png" --settle >/dev/null 2>&1
  echo "READY display=:$n port=$port app_pid=$(cat "$r/app.pid") home=$h log=$r/app.log"
}

cmd_stop() {
  check_n "${1:-}"
  local n=$1 r; r=$(rdir "$n")
  local sp app xv
  sp=$(cat "$r/session.pid" 2>/dev/null); app=$(cat "$r/app.pid" 2>/dev/null); xv=$(cat "$r/xvfb.pid" 2>/dev/null)
  # The setsid'd session leader's pid is its process-group id: TERM the whole
  # group (app + private dbus-daemon). Exact pids only, never pkill -f.
  alive "$sp" && kill -TERM -- "-$sp" 2>/dev/null
  alive "$app" && kill -TERM "$app" 2>/dev/null
  for _ in $(seq 1 30); do alive "$app" || break; sleep 0.1; done
  alive "$app" && kill -KILL "$app" 2>/dev/null
  alive "$sp" && kill -KILL -- "-$sp" 2>/dev/null
  alive "$xv" && kill -TERM "$xv" 2>/dev/null
  for _ in $(seq 1 30); do alive "$xv" || break; sleep 0.1; done
  alive "$xv" && kill -KILL "$xv" 2>/dev/null
  rm -f "/tmp/.X11-unix/X$n" "/tmp/.X$n-lock"
  rm -f "$r"/*.pid
  echo "stopped :$n"
}

cmd_status() {
  local dirs=()
  if [[ -n ${1:-} ]]; then dirs=("$(rdir "$1")"); else
    shopt -s nullglob; dirs=("$OUT"/rigs/*); fi
  (( ${#dirs[@]} )) || { echo "no rigs"; return 0; }
  local d n
  for d in "${dirs[@]}"; do
    n=$(basename "$d")
    printf ':%s xvfb=%s app=%s (pid %s) home=%s\n' "$n" \
      "$(alive "$(cat "$d/xvfb.pid" 2>/dev/null)" && echo up || echo down)" \
      "$(alive "$(cat "$d/app.pid" 2>/dev/null)" && echo up || echo down)" \
      "$(cat "$d/app.pid" 2>/dev/null || echo -)" "$(hdir "$n")"
  done
}

cmd_run() {
  check_n "${1:-}"; local n=$1; shift
  local r h; r=$(rdir "$n"); h=$(hdir "$n")
  [[ -f $r/dbus.addr ]] || die "rig :$n has no session (start it first)"
  local port; port=$(cat "$r/port" 2>/dev/null)
  exec env -u WAYLAND_DISPLAY -u XAUTHORITY -u XDG_CONFIG_HOME -u XDG_CACHE_HOME -u XDG_DATA_HOME \
    DISPLAY=":$n" HOME="$h" XDG_SESSION_TYPE=x11 LIBGL_ALWAYS_SOFTWARE=1 TRONTSNAP_TEST=1 \
    TRONTSNAP_PORT="${port:-0}" DBUS_SESSION_BUS_ADDRESS="$(cat "$r/dbus.addr")" "$@"
}

cmd_reseed() {
  check_n "${1:-}"
  alive "$(cat "$(rdir "$1")/app.pid" 2>/dev/null)" && die "stop the rig before reseeding"
  BARE=0; seed_home "$1"; echo "reseeded $(hdir "$1")"
}

case ${1:-} in
  start)  shift; cmd_start "$@" ;;
  stop)   shift; cmd_stop "$@" ;;
  status) shift; cmd_status "$@" ;;
  run)    shift; cmd_run "$@" ;;
  reseed) shift; cmd_reseed "$@" ;;
  *) sed -n '2,15p' "${BASH_SOURCE[0]}"; exit 2 ;;
esac
