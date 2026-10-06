#!/usr/bin/env bash
# tray_test.sh [--only A|B|C|D] -- the Linux tray icon (src/linux/sni.rs) and close-to-tray, asserted in
# isolated Xvfb rigs. A mock StatusNotifierWatcher (sni_mock.py) stands in for GNOME's AppIndicator
# extension on the rig's PRIVATE bus; tray_probe.py plays the HOST (reads the item, walks the dbusmenu,
# clicks, listens for signals). No window manager on Xvfb, so this proves the app's side of the
# contract; what GNOME's extension and Mutter do with it was checked by hand (see LINUX.md, "Tray").
#
#   A  no tray: Close quits (unchanged behaviour); TRONTSNAP_TEST alone never touches the bus
#   B  with a tray: registers, menu layout, Close hides (process + port + hotkey path alive, window
#      unmapped, ~0 CPU), Activate / menu Show / second launch bring it back
#   C  recording while hidden: label flips to Stop recording, badge on the icon, NewIcon signals,
#      Activate stops it; the check items flip real settings
#   D  watcher vanishes while hidden -> window comes back, then Close quits; menu Quit exits
#
# Rig :86 / port 48786, x11 recorder (never kms), silent fake players. Logs/shots: $HARNESS_OUT/tray_test/.
# Exit 1 on any FAIL. ~3 min.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
N=${RIG_N:-86}; PORT=${RIG_PORT:-48786}
D=$OUT/tray_test; mkdir -p "$D"
RIGHOME=$OUT/homes/$N
X=("$HERE/xinput.py" -d ":$N")
export PATH="$HERE/fake_bin:$PATH"          # no shutter / ticks through the real speakers
export SNI_LOG=$D/sni.log
ONLY=${2:-}; [[ ${1:-} == --only ]] || ONLY=
fails=0
ok()  { echo "  PASS  $1${2:+  [$2]}"; }
bad() { echo "  FAIL  $1${2:+  [$2]}"; fails=$((fails + 1)); }
cond() { local n=$1 d=$2; shift 2; if "$@"; then ok "$n" "$d"; else bad "$n" "$d"; fi; }
want() { [[ -z $ONLY || $ONLY == "$1" ]]; }

probe() { "$HERE/gui_env.sh" run "$N" python3 "$HERE/tray_probe.py" "$@"; }
jget() { python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print($1)"; }
app_pid() { ss -ltnp "sport = :$PORT" 2>/dev/null | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2; }
alive() { [[ -n ${1:-} ]] && kill -0 "$1" 2>/dev/null; }
port_up() { ss -ltn "sport = :$PORT" 2>/dev/null | grep -q LISTEN; }
win() { "${X[@]}" find TrontSnap 2>/dev/null | awk '{print $3}' | head -1; }   # mapped | unmapped | ""
wait_win() { local want=$1 t=${2:-8} i; for ((i = 0; i < t * 5; i++)); do [[ $(win) == "$want" ]] && return 0; sleep 0.2; done; return 1; }
wait_gone() { local p=$1 t=${2:-8} i; for ((i = 0; i < t * 5; i++)); do alive "$p" || return 0; sleep 0.2; done; return 1; }
ticks() { awk '{print $14+$15}' "/proc/$1/stat"; }
registered() { grep -q '"op": "register"' "$SNI_LOG" 2>/dev/null; }
wait_registered() { local i; for ((i = 0; i < 100; i++)); do registered && return 0; sleep 0.2; done; return 1; }
settings() { grep -E "^$1=" "$RIGHOME/.config/trontsnap/settings.conf" 2>/dev/null | tail -1 | cut -d= -f2; }
MOCK=
start_mock() { : > "$SNI_LOG"; ( setsid "$HERE/gui_env.sh" run "$N" python3 "$HERE/sni_mock.py" > "$D/mock.log" 2>&1 < /dev/null & ); sleep 1.5
  MOCK=$(pgrep -f "[s]ni_mock.py" | head -1); }
stop_mock() { [[ -n $MOCK ]] && kill "$MOCK" 2>/dev/null; MOCK=; sleep 0.3; }
start_rig() { TRONTSNAP_TRAY=${TRAY:-on} "$HERE/gui_env.sh" start "$N" "$PORT" "$@" > "$D/rig_start.log" 2>&1 || { cat "$D/rig_start.log" >&2; bad "rig start"; return 1; }; }
stop_rig() { stop_mock; "$HERE/gui_env.sh" stop "$N" > /dev/null 2>&1; }
trap 'stop_rig' EXIT

# ---------------------------------------------------------------- A: no tray
if want A; then
  echo "== A: no tray =="
  # A1 the default test guard: TRONTSNAP_TEST must not register anything, or rec_gnome_test (real
  # session bus) would put an icon in the REAL tray.
  TRAY=unset start_rig && {
    start_mock; sleep 4
    cond "A1 TRONTSNAP_TEST alone: no registration (guard keeps test runs off the real tray)" "$(wc -l < "$SNI_LOG") log lines" \
      bash -c '! grep -q "\"op\": \"register\"" "$1"' _ "$SNI_LOG"
    stop_rig
  }
  # A2 tray enabled but no watcher: Close quits, as before.
  start_rig && {
    P=$(app_pid)
    cond "A2 no watcher: process up, window mapped" "pid $P" test "$(win)" = mapped
    "${X[@]}" wmclose TrontSnap > /dev/null; wait_gone "$P" 8
    cond "A2 no watcher: Close quits the app" "pid $P gone=$(alive "$P" && echo no || echo yes)" bash -c '! kill -0 "$1" 2>/dev/null' _ "$P"
    cond "A2 ...and frees the port" "" bash -c '! ss -ltn "sport = :$1" | grep -q LISTEN' _ "$PORT"
    stop_rig
  }
fi

# ---------------------------------------------------------------- B: registered, hide/show
if want B || want C || want D; then
  echo "== B: registered tray =="
  start_rig && start_mock
  P=$(app_pid)
  if wait_registered; then ok "B1 the item registers with the watcher (late watcher too)" "$(grep -m1 register "$SNI_LOG" | jget "d['name']")"; else bad "B1 register" "see $SNI_LOG $D/rig_start.log"; fi
  sleep 1.5
  IC=$(probe icon); echo "$IC" > "$D/icon_idle.json"
  cond "B2 icon pixmaps 16/22/24/32/48/64, Status Active" "$IC" test "$(echo "$IC" | jget "d['sizes']")" = "[16, 22, 24, 32, 48, 64]"
  cond "B2 WindowId is int32 (the spec; GNOME warns on 'u')" "" test "$(echo "$IC" | jget "d['windowid']")" = 0
  cond "B2 idle icon has no recording badge" "badge_px=$(echo "$IC" | jget "d['badge_px']")" test "$(echo "$IC" | jget "d['badge_px']")" = 0
  LY=$(probe layout); echo "$LY" > "$D/layout_idle.json"
  LABELS=$(echo "$LY" | jget "'|'.join(i['label'] or '-' for i in d['items'])")
  cond "B3 menu: Show / captures / record / OCR / checks / Quit" "$LABELS" \
    test "$LABELS" = "Show TrontSnap|-|Capture fullscreen|Capture region|Record screen|Record region|Copy text (OCR)|-|Capture cursor|Record audio|Start at login|-|Quit TrontSnap"

  "$HERE/shot.sh" "$N" "$D/1_open.png" --settle > /dev/null
  "${X[@]}" wmclose TrontSnap > /dev/null; sleep 2
  cond "B4 Close with a tray: process alive" "pid $P" alive "$P"
  cond "B4 ...command port still bound (hotkeys/record keep working)" "" port_up
  cond "B4 ...window unmapped, not destroyed" "$(win)" test "$(win)" = unmapped
  a=$(ticks "$P"); sleep 5; b=$(ticks "$P")
  cond "B5 hidden = idle (< 10 cpu ticks in 5 s)" "$((b - a)) ticks" test $((b - a)) -lt 10
  # the keybinding path while hidden: a real capture command over the port is still served
  n0=$(ls "$RIGHOME"/Pictures/TrontSnap/*.png | wc -l)
  echo -n full > "/dev/tcp/127.0.0.1/$PORT"; sleep 4
  n1=$(ls "$RIGHOME"/Pictures/TrontSnap/*.png | wc -l)
  cond "B6 'full' over the port while hidden still captures (the keybinding path)" "$n0 -> $n1 PNGs" test "$n1" -gt "$n0"

  probe activate > /dev/null; wait_win mapped 8
  cond "B7 Activate maps the window again" "$(win)" test "$(win)" = mapped
  sleep 2; "$HERE/shot.sh" "$N" "$D/2_after_activate.png" --settle > /dev/null
  "${X[@]}" wmclose TrontSnap > /dev/null; sleep 1.5
  probe click 1 > /dev/null; wait_win mapped 8
  cond "B8 menu 'Show TrontSnap' maps it again" "$(win)" test "$(win)" = mapped
  "${X[@]}" wmclose TrontSnap > /dev/null; sleep 1.5
  "$HERE/gui_env.sh" run "$N" "$REPO/target/debug/trontsnap" > /dev/null 2>&1; wait_win mapped 8
  cond "B9 a second 'trontsnap' launch maps it again" "$(win)" test "$(win)" = mapped
fi

# ---------------------------------------------------------------- C: recording + checks
if want C || want D; then
  echo "== C: recording while hidden, check items =="
  "${X[@]}" wmclose TrontSnap > /dev/null; sleep 1.5
  (probe signals 12 > "$D/signals.log" 2>&1 &)
  sleep 0.8
  echo -n record > "/dev/tcp/127.0.0.1/$PORT"
  for _ in $(seq 1 40); do
    LY=$(probe layout); [[ $(echo "$LY" | jget "d['items'][4]['label']") == "Stop recording" ]] && break; sleep 0.25
  done
  IC=$(probe icon); echo "$IC" > "$D/icon_rec.json"
  cond "C1 recording (window hidden): menu says Stop recording" "$(echo "$LY" | jget "d['items'][4]['label']")" \
    test "$(echo "$LY" | jget "d['items'][4]['label']")" = "Stop recording"
  cond "C1 ...and Record region is greyed (a press then would be a plain stop)" "" test "$(echo "$LY" | jget "d['items'][5]['enabled']")" = False
  cond "C2 recording: the icon carries the red badge" "badge_px=$(echo "$IC" | jget "d['badge_px']")" test "$(echo "$IC" | jget "d['badge_px']")" -gt 15
  cond "C2 ...tooltip says it is recording" "$(echo "$IC" | jget "d['tip']")" test "$(echo "$IC" | jget "d['tip']")" = "TrontSnap is recording"
  sleep 2
  probe activate > /dev/null            # a click while recording = stop (the badge is the cue)
  for _ in $(seq 1 60); do
    LY=$(probe layout); [[ $(echo "$LY" | jget "d['items'][4]['label']") == "Record screen" ]] && break; sleep 0.3
  done
  sleep 1; IC=$(probe icon)
  cond "C3 Activate while recording stops it: label back to Record screen" "$(echo "$LY" | jget "d['items'][4]['label']")" \
    test "$(echo "$LY" | jget "d['items'][4]['label']")" = "Record screen"
  cond "C3 ...badge gone, icon identical to idle" "$(echo "$IC" | jget "d['sha']") vs $(jget "d['sha']" < "$D/icon_idle.json")" \
    test "$(echo "$IC" | jget "d['sha']")" = "$(jget "d['sha']" < "$D/icon_idle.json")"
  sleep 4
  SIG=$(grep -c '"signal": "NewIcon"' "$D/signals.log"); LU=$(grep -c '"signal": "LayoutUpdated"' "$D/signals.log")
  cond "C4 NewIcon fired for the badge on AND off" "$SIG NewIcon, $LU LayoutUpdated" test "$SIG" -ge 2
  cond "C5 the clip was delivered to Pictures/TrontSnap" "" test "$(ls "$RIGHOME"/Pictures/TrontSnap/*.mp4 | wc -l)" -ge 2

  A0=$(settings RecordAudio); probe click 8 > /dev/null; sleep 1
  cond "C6 'Record audio' click flips the real setting" "RecordAudio $A0 -> $(settings RecordAudio)" test "$(settings RecordAudio)" != "$A0"
  cond "C6 ...and the menu check follows" "" test "$(probe layout | jget "d['items'][9]['toggle']")" = "$([[ $(settings RecordAudio) == 1 ]] && echo 1 || echo 0)"
  probe click 8 > /dev/null; sleep 1
  cond "C6 ...and back" "RecordAudio $(settings RecordAudio)" test "$(settings RecordAudio)" = "$A0"
  K0=$(settings CaptureCursor); probe click 7 > /dev/null; sleep 1
  cond "C7 'Capture cursor' click flips the real setting" "CaptureCursor ${K0:-default} -> $(settings CaptureCursor)" test "$(settings CaptureCursor)" != "$K0"
  probe click 9 > /dev/null; sleep 1
  cond "C8 'Start at login' writes the autostart entry (rig HOME)" "" test -f "$RIGHOME/.config/autostart/trontsnap.desktop"
  probe click 9 > /dev/null; sleep 1
  cond "C8 ...and removes it" "" bash -c '! test -f "$1/.config/autostart/trontsnap.desktop"' _ "$RIGHOME"
fi

# ---------------------------------------------------------------- D: watcher gone, Quit
if want D; then
  echo "== D: watcher vanishes, Quit =="
  P=$(app_pid)
  [[ $(win) == mapped ]] && { "${X[@]}" wmclose TrontSnap > /dev/null; sleep 1.5; }
  cond "D1 hidden again before the watcher goes" "$(win)" test "$(win)" = unmapped
  stop_mock
  wait_win mapped 8
  cond "D2 watcher gone while hidden: the window comes back (no stranded app)" "$(win)" test "$(win)" = mapped
  "${X[@]}" wmclose TrontSnap > /dev/null; wait_gone "$P" 8
  cond "D3 and with no tray left, Close quits again" "pid $P" bash -c '! kill -0 "$1" 2>/dev/null' _ "$P"
  stop_rig

  start_rig && start_mock; wait_registered; sleep 1.5; P=$(app_pid)
  "${X[@]}" wmclose TrontSnap > /dev/null; sleep 1.5
  probe click 10 > /dev/null; wait_gone "$P" 8
  cond "D4 menu Quit (window hidden) exits the process" "pid $P" bash -c '! kill -0 "$1" 2>/dev/null' _ "$P"
  cond "D4 ...and frees the port" "" bash -c '! ss -ltn "sport = :$1" | grep -q LISTEN' _ "$PORT"
fi

echo "  logs/screens: $D/"
(( fails == 0 )) && echo "tray_test: PASS" || echo "tray_test: $fails FAIL"
exit $(( fails > 0 ))
