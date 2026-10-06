#!/usr/bin/env bash
# hotkey_wl_test.sh — the GNOME keybindings PRESSED, not just installed. A private headless GNOME Shell
# (wl_env.sh: own bus, temp HOME, software GL, no video decode) gets a private gsd-media-keys (the daemon that
# owns `custom-keybindings`), the app installs its five bindings into that session's dconf, and real key
# events go in through mutter's RemoteDesktop API (wl_input.py): Print, Ctrl+Shift+Print, Ctrl+Print,
# Ctrl+Alt+Print, Ctrl+Shift+Alt+Print. Nothing touches the real session's gsettings or keys.
# Recording uses the hook backend (a software lavfi clip), so nothing records the screen or touches the GPU.
# Shots + logs: $HARNESS_OUT/hotkey_wl/ (READ them). Needs tesseract ABSENT for the OCR-notice step.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
export HARNESS_OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
export WL_PORT=${RIG_PORT:-48795}
BIN=${TRONTSNAP_BIN:-$REPO/target/debug/trontsnap}
D=$HARNESS_OUT/hotkey_wl; mkdir -p "$D"
W=$HERE/wl_env.sh
WH=$HARNESS_OUT/wl/home
PICS=$WH/Pictures/TrontSnap
fails=0
ok()  { echo "  PASS  $1${2:+  [$2]}"; }
bad() { echo "  FAIL  $1${2:+  [$2]}"; fails=$((fails + 1)); }
pids=()
cleanup() {
  for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done
  "$W" stop >/dev/null 2>&1
}
trap cleanup EXIT
[[ -x $BIN ]] || { echo "no $BIN"; exit 1; }
"$W" start | tail -1
journalctl -k -b --since "-4 min" | grep -i -E "uvd|vce|radeon.*error|hung" && { echo "  FAIL  GPU errors in the kernel log: STOP"; exit 1; }
sleep 3
banner_px() { python3 - "$1" <<'PY'
import sys
from PIL import Image
im = Image.open(sys.argv[1]).convert("RGB").crop((420, 36, 950, 140))
print(sum(1 for p in im.getdata() if 50 < min(p) and max(p) < 125 and max(p) - min(p) < 8))
PY
}
for _ in 1 2 3 4; do
  "$W" input key Escape >/dev/null 2>&1; sleep 1.5
  "$W" shot "$D/0_base.png" >/dev/null; base=$(banner_px "$D/0_base.png")
  (( base < 1000 )) && break
done

# --- the keybinding daemon, in the private session ---
(setsid "$W" run /usr/libexec/gsd-media-keys > "$D/gsd.log" 2>&1 < /dev/null & echo $! > "$D/gsd.pid")
pids+=("$(cat "$D/gsd.pid")")
# --- the hook recorder: a software lavfi clip until SIGINT ---
mkdir -p "$WH/.config/trontsnap"
cat > "$WH/.config/trontsnap/record-start.sh" <<'HOOK'
#!/bin/sh
exec ffmpeg -re -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=320x180:rate=15 -pix_fmt yuv420p -c:v libx264 -preset ultrafast "$1"
HOOK
chmod +x "$WH/.config/trontsnap/record-start.sh"
# --- the app WITHOUT the TEST guard: it installs its bindings into this session's dconf ---
(setsid "$W" run env -u TRONTSNAP_TEST TRONTSNAP_RECORDER=hook "$BIN" --startup > "$D/app.log" 2>&1 < /dev/null & echo $! > "$D/app.pid")
pids+=("$(cat "$D/app.pid")")
for _ in $(seq 1 60); do ss -ltn "sport = :$WL_PORT" 2>/dev/null | grep -q LISTEN && break; sleep 0.5; done
sleep 4   # install_gnome_binds runs a few gsettings calls
list=$("$W" run gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings 2>&1)
n=$(grep -o "custom[0-9]*" <<< "$list" | wc -l)
(( n >= 5 )) && ok "the app installed its custom keybindings in the private session" "$n entries" || bad "bindings not installed" "$list"
sleep 3
if grep -qi "error\|failed" "$D/gsd.log"; then echo "  NOTE  gsd-media-keys said: $(head -3 "$D/gsd.log" | tr '\n' '|')"; fi

press() { "$W" input key "$1" >/dev/null 2>&1; }
newest() { ls -t "$PICS" 2>/dev/null | head -1; }

# === Print -> trontsnap full ===
n0=$(ls "$PICS" 2>/dev/null | wc -l)
press Print
for _ in $(seq 1 60); do (( $(ls "$PICS" 2>/dev/null | wc -l) > n0 )) && break; sleep 0.5; done
f=$PICS/$(newest)
if (( $(ls "$PICS" | wc -l) > n0 )) && file "$f" | grep -q "PNG image data, 1366 x 768"; then ok "Print ran \`trontsnap full\`: a 1366x768 PNG landed" "$(basename "$f")"
else bad "Print did not produce a screenshot" "$(ls -t "$PICS" | head -3 | tr '\n' ' ') gsd: $(head -2 "$D/gsd.log" | tr '\n' '|')"; fi

# === Ctrl+Shift+Print twice -> record start, record stop ===
n0=$(ls "$PICS" | grep -c '\.mp4$')
press ctrl+shift+Print; sleep 4; press ctrl+shift+Print
for _ in $(seq 1 60); do (( $(ls "$PICS" | grep -c '\.mp4$') > n0 )) && break; sleep 0.5; done
if (( $(ls "$PICS" | grep -c '\.mp4$') > n0 )); then
  m=$PICS/$(ls -t "$PICS" | grep '\.mp4$' | head -1)
  dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$m" 2>/dev/null)
  ok "Ctrl+Shift+Print toggled the recorder: a clip was delivered" "$(basename "$m") ${dur:-?} s"
else bad "Ctrl+Shift+Print twice delivered no clip" "$(tail -3 "$D/app.log" | tr '\n' '|')"; fi

# === Ctrl+Shift+Alt+Print while a recording runs STOPS it (no picker): the real binding, end to end ===
n0=$(ls "$PICS" | grep -c '\.mp4$')
press ctrl+shift+Print; sleep 3
press ctrl+shift+alt+Print; sleep 3
if [[ -z $(pgrep -f "[t]rontsnap record region" | head -1) ]]; then ok "Ctrl+Shift+Alt+Print during a recording opened NO picker"; else bad "a picker opened during a recording"; fi
for _ in $(seq 1 40); do (( $(ls "$PICS" | grep -c '\.mp4$') > n0 )) && break; sleep 0.5; done
(( $(ls "$PICS" | grep -c '\.mp4$') > n0 )) && ok "...and it stopped the recording: the clip was delivered" || bad "the recording was not stopped by Ctrl+Shift+Alt+Print"
pkill_picker() { for p in $(pgrep -f "[t]rontsnap record region"); do kill "$p" 2>/dev/null; done; }; pkill_picker

# === Ctrl+Alt+Print -> trontsnap ocr (no tesseract: the notice) ===
if command -v tesseract >/dev/null; then echo "  SKIP  Ctrl+Alt+Print: tesseract is installed"; else
  "$W" shot "$D/1_before_ocr.png" >/dev/null
  press ctrl+alt+Print; sleep 2.5; "$W" shot "$D/2_ocr_notice.png" >/dev/null
  a=$(banner_px "$D/2_ocr_notice.png")
  (( a > base + 20000 )) && ok "Ctrl+Alt+Print ran \`trontsnap ocr\`: the OCR notice is on screen" "$a px" || bad "no OCR notice after Ctrl+Alt+Print" "$a px"
fi

# === Ctrl+Print / Ctrl+Shift+Alt+Print -> the region picker opens (a process); a right click cancels it ===
# (Escape is NOT used: nothing in this headless shell gets keyboard focus without a click - zenity ignores Escape
# here too - so a key press cannot reach any freshly mapped window; pointer events do.)
picker() { pgrep -f "[t]rontsnap $1" | head -1; }
cancel() { "$W" input abs 700 400 wait 0.3 rclick >/dev/null 2>&1; sleep 2; }
press ctrl+Print; sleep 6
if [[ -n $(picker region) ]]; then ok "Ctrl+Print started \`trontsnap region\` (the picker)"; "$W" shot "$D/3_picker.png" >/dev/null; cancel
else bad "Ctrl+Print started no picker"; fi
[[ -z $(picker region) ]] && ok "a right click closed the picker" || bad "the picker is still running after a right click"
press ctrl+shift+alt+Print; sleep 6
if [[ -n $(picker "record region") ]]; then ok "Ctrl+Shift+Alt+Print started \`trontsnap record region\`"; cancel
else bad "Ctrl+Shift+Alt+Print started no record-region picker"; fi

echo "  shots/logs: $D/"
(( fails == 0 )) && echo "hotkey_wl_test: PASS" || echo "hotkey_wl_test: $fails FAIL"
exit $(( fails > 0 ))
