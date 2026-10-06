#!/usr/bin/env bash
# notice_wl_test.sh — do one-shot notices really go away on a REAL GNOME Shell? Private headless shell
# (wl_env.sh, software GL, no video decode, own bus): GNOME ignores notify-send -t and holds a banner until
# the user next touches the mouse/keyboard, so the control banner must still be up 9 s later while
# `trontsnap ocr`'s notice (explicit CloseNotification from a detached `notice-close`) must be gone.
# Needs tesseract to be ABSENT (the OCR notice is what is under test). Shots in $HARNESS_OUT/notice_wl/ (READ them).
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
export HARNESS_OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
D=$HARNESS_OUT/notice_wl; mkdir -p "$D"
W=$HERE/wl_env.sh
fails=0
ok()  { echo "  PASS  $1${2:+  [$2]}"; }
bad() { echo "  FAIL  $1${2:+  [$2]}"; fails=$((fails + 1)); }
command -v tesseract >/dev/null && { echo "  SKIP  tesseract is installed: the OCR notice is not raised"; exit 0; }
trap '"$W" stop >/dev/null 2>&1' EXIT
"$W" start | tail -1
klog() { journalctl -k -b --since "-4 min" | grep -i -E "uvd|vce|radeon.*error|hung"; }
[[ -z $(klog) ]] || { echo "  FAIL  kernel log shows GPU errors after the shell started: STOP"; klog; exit 1; }

# banner = grey card (~76 grey) in the top-centre; the blue wallpaper and the black bar do not match
banner_px() {
  python3 - "$1" <<'PY'
import sys
from PIL import Image
im = Image.open(sys.argv[1]).convert("RGB").crop((420, 36, 950, 140))
print(sum(1 for p in im.getdata() if 50 < min(p) and max(p) < 125 and max(p) - min(p) < 8))
PY
}
# HARD-WON: the shell enters the Activities overview a few seconds AFTER it is READY; an Escape sent earlier is
# lost, and the overview's grey widgets then look like a banner to the pixel count. Escape until the base
# shot is the empty desktop.
sleep 3
for _ in 1 2 3 4; do
  "$W" input key Escape >/dev/null 2>&1; sleep 1.5
  "$W" shot "$D/0_base.png" >/dev/null
  base=$(banner_px "$D/0_base.png")
  (( base < 1000 )) && break
done
(( base < 1000 )) || { bad "could not leave the Activities overview (base $base px): Read $D/0_base.png"; exit 1; }

"$W" run notify-send -a OldStyle -t 2000 -h int:transient:1 "old style banner" "notify-send -t 2000, transient"
sleep 1.2; "$W" shot "$D/1_old_1s.png" >/dev/null; sleep 7.5; "$W" shot "$D/2_old_9s.png" >/dev/null
a=$(banner_px "$D/1_old_1s.png"); b=$(banner_px "$D/2_old_9s.png")
(( a > base + 20000 )) && ok "control: a plain notify-send banner shows" "$a px" || bad "control banner not shown" "$a px"
(( b > base + 20000 )) && ok "control: ...and is STILL up 9 s later although -t 2000 (the GNOME behaviour this fixes)" "$b px" \
  || echo "  NOTE  control banner expired by itself: this shell honours -t now, the explicit close is merely belt and braces"
# dismiss the control (first notification on this bus)
busctl --address="$(cat "$HARNESS_OUT/wl/dbus.addr")" call org.freedesktop.Notifications /org/freedesktop/Notifications \
  org.freedesktop.Notifications CloseNotification u 1 >/dev/null 2>&1; sleep 1.5

"$W" run target/debug/trontsnap ocr >/dev/null 2>&1
sleep 1.0; "$W" shot "$D/4_new_1s.png" >/dev/null; sleep 8.5; "$W" shot "$D/5_new_9s.png" >/dev/null
a=$(banner_px "$D/4_new_1s.png"); b=$(banner_px "$D/5_new_9s.png")
(( a > base + 20000 )) && ok "trontsnap ocr: the notice shows" "$a px" || bad "trontsnap ocr: no notice on screen" "$a px"
(( b <= base + 500 )) && ok "trontsnap ocr: the notice is GONE 9 s later, with nobody touching the shell" "$b px" || bad "trontsnap ocr: the notice is still up" "$b px"
echo "  screenshots: $D/ (4_new_1s.png, 5_new_9s.png, 2_old_9s.png)"
(( fails == 0 )) && echo "notice_wl_test: PASS" || echo "notice_wl_test: $fails FAIL"
exit $(( fails > 0 ))
