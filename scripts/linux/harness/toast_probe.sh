#!/usr/bin/env bash
# toast_probe.sh N | --live — where does the capture toast window actually land?
#   toast_probe.sh 99      toast inside rig :99 (Xvfb 1366x768, no WM)
#   toast_probe.sh --live  toast on the REAL session display (~3 s popup on screen)
# Spawns `trontsnap toast <png>` like toast::launch (WAYLAND_DISPLAY dropped so it
# runs on X/XWayland), polls the window's absolute geometry, and prints it next to
# the intended bottom-right dock (screen - 340x188 - (24,48)).
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
BIN=${TRONTSNAP_BIN:-$REPO/target/debug/trontsnap}
mode=${1:?usage: toast_probe.sh N | --live}
mkdir -p "$OUT/shots"
png="$OUT/toast_probe_src.png"
src=$(ls "$HOME"/Pictures/TrontSnap/*.png 2>/dev/null | head -1)
[[ -n $src ]] && cp "$src" "$png" || { echo "toast_probe: no source png" >&2; exit 1; }

if [[ $mode == --live ]]; then
  disp=${DISPLAY:-:0}
  launch=(env -u WAYLAND_DISPLAY DISPLAY="$disp" "$BIN" toast "$png")
  shotn=""
else
  disp=":$mode"
  # run inside the rig's env; the toast subprocess is a fresh process, so also
  # drop WAYLAND_DISPLAY exactly as the real launcher does (rig has none anyway).
  launch=("$HERE/gui_env.sh" run "$mode" "$BIN" toast "$png")
  shotn=$mode
fi

# Park the pointer top-left: a pointer ON the toast pins it open (hover pauses the
# dismiss countdown) and the probe then never exits. Rig only; live = your mouse.
[[ -n $shotn ]] && "$HERE/xinput.py" -d "$disp" move 3 3
size=$(DISPLAY=$disp xdpyinfo 2>/dev/null | awk '/dimensions:/ {print $2; exit}')
sw=${size%x*}; sh=${size#*x}
"${launch[@]}" > "$OUT/toast_probe.log" 2>&1 &
tpid=$!
found=""
for i in $(seq 1 60); do
  found=$("$HERE/xinput.py" -d "$disp" find "TrontSnap Toast" 2>/dev/null | head -1)
  [[ -n $found ]] && break
  sleep 0.1
done
if [[ -z $found ]]; then
  echo "toast_probe: toast window never appeared (log: $OUT/toast_probe.log)"; kill "$tpid" 2>/dev/null; exit 1
fi
sleep 0.8   # let OuterPosition (sent on the first frames) take effect
found=$("$HERE/xinput.py" -d "$disp" find "TrontSnap Toast" 2>/dev/null | head -1)
[[ -n $shotn ]] && SHOT_MOUSE=0 "$HERE/shot.sh" "$shotn" "$OUT/shots/toast_probe_$shotn.png" >/dev/null
echo "screen : ${sw}x${sh} on $disp"
echo "toast  : $found"
geo=$(awk '{print $2}' <<< "$found")      # WxH+X+Y
IFS='x+' read -r tw th tx ty <<< "$geo"
ex=$(( sw - 340 - 24 )); ey=$(( sh - 188 - 48 ))
echo "intended: ${tw}x${th}+${ex}+${ey} (bottom-right dock, in logical px at scale 1)"
echo "delta  : dx=$(( tx - ex )) dy=$(( ty - ey ))"
[[ -n $shotn ]] && echo "shot   : $OUT/shots/toast_probe_$shotn.png"
wait "$tpid" 2>/dev/null
