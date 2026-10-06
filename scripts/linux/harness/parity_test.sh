#!/usr/bin/env bash
# parity_test.sh [--no-wayland] [--only X|W] — window lifecycle, hotkey plumbing, picker loupe and
# cursor checks (the 2026-10-05 parity audit, PAR-02..13). Two halves, run one after the other
# because both use port 48794:
#   X  an Xvfb rig (:94): `--startup` stays unmapped, a second launch maps it, the title-bar X quits,
#      WM_CLASS carries the app_id, stored hotkeys become GNOME accelerators, the PrintScreen opt-out
#      survives a relaunch, the Settings tab spawns ~no gsettings processes, the picker paints a loupe.
#   W  a private headless GNOME Shell (wl_env.sh, real native Wayland): `--startup` minimizes and sends
#      set_app_id, the X button quits, a second launch asks for attention (xdg-activation) when the
#      window is buried, the wheel resizes (and clamps) the loupe, "Capture cursor" draws the pointer.
# Screenshots/logs: $HARNESS_OUT/parity_test/. LOOK at the PNGs; exit 1 on any FAIL. ~6 min.
# MACHINE SAFETY: no video decode anywhere; the Wayland half is software GL (see wl_env.sh), and the
# script greps the kernel log for UVD/VCE/radeon trouble before and after it.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
BIN=${TRONTSNAP_BIN:-$REPO/target/debug/trontsnap}
N=${RIG_N:-94}; PORT=${RIG_PORT:-48794}; export WL_PORT=$PORT
D=$OUT/parity_test; mkdir -p "$D"
find "${D:?}" -maxdepth 1 -type f \( -name '*.png' -o -name '*.log' -o -name '*.dbg' \) -delete
RIGHOME=$OUT/homes/$N
X=("$HERE/xinput.py" -d ":$N")
export XINPUT_SLOW=${XINPUT_SLOW:-1}
RUN_X=1; RUN_W=1
for a in "$@"; do case $a in --no-wayland) RUN_W=0 ;; --only) ;; X) RUN_W=0 ;; W) RUN_X=0 ;; esac; done
fails=0
ok()  { echo "  PASS  $1${2:+  [$2]}"; }
bad() { echo "  FAIL  $1${2:+  [$2]}"; fails=$((fails + 1)); }
# cond NAME DETAIL <test command...>: PASS when the command succeeds
cond() { local n=$1 d=$2; shift 2; if "$@"; then ok "$n" "$d"; else bad "$n" "$d"; fi; }
gpid() { ss -ltnp 2>/dev/null | grep ":$PORT " | grep -o 'pid=[0-9]*' | head -1 | cut -d= -f2; }
port_free() { ! ss -ltn 2>/dev/null | grep -q ":$PORT "; }
kernel_check() { journalctl -k -b --since "-3 min" 2>/dev/null | grep -i -E "uvd|vce|radeon.*error|hung" | head -3; }
# Kill the trontsnap processes of THIS test's port by exact pid (pgrep -x: never the harness wrapper,
# and never the live app, whose environment has no TRONTSNAP_PORT).
killrig() {
  local p
  for p in $(pgrep -x trontsnap); do
    tr '\0' '\n' < "/proc/$p/environ" 2>/dev/null | grep -q "^TRONTSNAP_PORT=$PORT\$" && kill "$p" 2>/dev/null
  done
  return 0
}
cleanup() {
  killrig
  "$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1
  "$HERE/wl_env.sh" stop >/dev/null 2>&1
}
trap cleanup EXIT
shot() { "$HERE/shot.sh" "$N" "$D/$1.png" "${@:2}" >/dev/null; }
park() { "${X[@]}" move 1270 700 >/dev/null; }
K=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings
bind() { "$HERE/gui_env.sh" run "$N" gsettings get "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$K/trontsnap-$1/" binding 2>/dev/null | tr -d "'"; }
# gsettings from inside the rig's private dbus + dconf (never the real session)
rg() { "$HERE/gui_env.sh" run "$N" gsettings "$@" 2>/dev/null; }
# a seed hook that writes settings.conf lines into the rig's temp HOME
mkseed() { # mkseed FILE lines...
  local f=$1; shift
  { echo '#!/bin/sh'; echo 'mkdir -p "$1/.config/trontsnap"'; echo 'cat >> "$1/.config/trontsnap/settings.conf" <<EOF'; printf '%s\n' "$@"; echo 'EOF'; } > "$f"
  chmod +x "$f"
}

pre=$(kernel_check); [[ -n $pre ]] && { echo "kernel log already shows GPU trouble, not starting:"; echo "$pre"; exit 1; }

# ============================================================ X: Xvfb rig
if (( RUN_X )); then
echo "== X: Xvfb rig"
"$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1

# --- X1 --startup: unmapped; second launch maps; X quits
RIG_APP_ARGS=--startup "$HERE/gui_env.sh" start "$N" "$PORT" >/dev/null 2>&1 || { bad "X1 rig start"; exit 1; }
st=$("${X[@]}" find TrontSnap | awk '{print $3}')
cond "X1 --startup leaves the window unmapped" "state=$st" test "$st" = unmapped
wid=$("${X[@]}" find TrontSnap | awk '{print $1}')
cls=$(xprop -display ":$N" -id "$wid" WM_CLASS 2>/dev/null)
cond "X2 window carries the app_id as WM_CLASS" "$cls" grep -q '"trontsnap"' <<< "$cls"
"$HERE/gui_env.sh" run "$N" "$BIN" >/dev/null 2>&1; sleep 3
st=$("${X[@]}" find TrontSnap | awk '{print $3}')
cond "X1 a second launch maps it" "state=$st" test "$st" = mapped
shot x1_second_launch --settle
pid=$(gpid)
"${X[@]}" click 1156 20; sleep 3
if kill -0 "$pid" 2>/dev/null; then bad "X1 title-bar X quits the app" "pid $pid still alive"; else ok "X1 title-bar X quits the app" "pid $pid gone"; fi
cond "X1 command port released" "" port_free
"$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1

# --- X3 stored hotkeys -> GNOME accelerators (private dconf); X5 opt-out
mkseed "$D/seed_hotkeys.sh" HasRun=1 HotkeyFullMods=2 HotkeyFullVk=123 HotkeyRegionMods=8 HotkeyRegionVk=44 \
  HotkeyRecordMods=3 HotkeyRecordVk=122 HotkeyOcrMods=6 HotkeyOcrVk=36
RIG_SEED_HOOK=$D/seed_hotkeys.sh "$HERE/gui_env.sh" start "$N" "$PORT" --no-guard >/dev/null 2>&1 || { bad "X3 rig start"; exit 1; }
sleep 3
f=$(bind full); r=$(bind region); c=$(bind record); rr=$(bind record-region); o=$(bind ocr)
cond "X3 Ctrl+F12 -> full"          "$f"  test "$f" = '<Ctrl>F12'
cond "X3 Super+PrtSc -> region"     "$r"  test "$r" = '<Super>Print'
cond "X3 Ctrl+Alt+F11 -> record"    "$c"  test "$c" = '<Ctrl><Alt>F11'
cond "X3 record-region disabled when Record already has Alt" "'$rr'" test "$rr" = ''
cond "X3 Ctrl+Shift+Home -> ocr"    "$o"  test "$o" = '<Ctrl><Shift>Home'
sc=$(rg get org.gnome.shell.keybindings show-screenshot-ui)
cond "X5 default: GNOME's own Print UI is freed" "$sc" grep -q '\[\]' <<< "$sc"
"$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1

mkseed "$D/seed_optout.sh" HasRun=1 PrintKeyOptOut=1
RIG_SEED_HOOK=$D/seed_optout.sh "$HERE/gui_env.sh" start "$N" "$PORT" --no-guard >/dev/null 2>&1 || { bad "X5 rig start"; exit 1; }
sleep 3
sc=$(rg get org.gnome.shell.keybindings show-screenshot-ui); f=$(bind full)
cond "X5 opted out: launch leaves GNOME's Print UI alone" "$sc" grep -q "Print" <<< "$sc"
cond "X5 opted out: our bare-Print binding is blanked" "'$f'" test "$f" = ''

# --- X4 the Settings tab must not spawn gsettings per frame (counting shim in front of the real one)
"$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1
mkdir -p "$D/shim"
cat > "$D/shim/gsettings" <<EOS
#!/bin/sh
echo "\$*" >> "$D/gsettings_calls.log"
exec /usr/bin/gsettings "\$@"
EOS
chmod +x "$D/shim/gsettings"; : > "$D/gsettings_calls.log"
PATH=$D/shim:$PATH "$HERE/gui_env.sh" start "$N" "$PORT" >/dev/null 2>&1 || { bad "X4 rig start"; exit 1; }
sleep 2; "${X[@]}" click 358 20; sleep 2
base=$(wc -l < "$D/gsettings_calls.log")
for i in $(seq 1 20); do "${X[@]}" move $((300 + i * 25)) $((200 + i * 10)); done
sleep 1
delta=$(( $(wc -l < "$D/gsettings_calls.log") - base ))
# before the fix: +36 over 20 pointer moves (every frame asked gsettings). One 5 s refresh is allowed.
cond "X4 Settings tab: 20 pointer moves cause <= 2 gsettings spawns" "delta=$delta (total $(wc -l < "$D/gsettings_calls.log"))" test "$delta" -le 2
"$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1

# --- X6 picker loupe (Xvfb: no WM, so the picker window is 800x600 at +0+0)
"$HERE/gui_env.sh" start "$N" "$PORT" >/dev/null 2>&1 || { bad "X6 rig start"; exit 1; }
# Pointer outside the 800x600 picker BEFORE it starts: no hover yet, so no loupe in the first shot.
"${X[@]}" move 1200 700; sleep 0.5
(setsid "$HERE/gui_env.sh" run "$N" "$BIN" region > "$D/x6_region.log" 2>&1 < /dev/null &)
sleep 9
export SHOT_MOUSE=0   # x11grab would paint the pointer into the shots and the diff would measure IT
shot x6_no_loupe --settle
"${X[@]}" move 300 200; sleep 0.5; "${X[@]}" move 301 201; sleep 1; shot x6_loupe --settle
unset SHOT_MOUSE
bb=$(python3 "$HERE/imgdiff.py" "$D/x6_no_loupe.png" "$D/x6_loupe.png" 2>&1 | sed -n 's/.*bbox=(\(.*\)).*/\1/p')
echo "      loupe diff bbox: ${bb:-none}"
# loupe box = pointer + 26, 132 wide (+ stroke): expect the changed area inside ~ (320..470, 220..370)
cond "X6 loupe appears beside the pointer (and only there)" "bbox=$bb" python3 - "$bb" <<'PY'
import sys
b=[int(v) for v in sys.argv[1].replace(' ','').split(',') if v] if len(sys.argv)>1 else []
sys.exit(0 if len(b)==4 and b[0]>=290 and b[1]>=190 and b[2]<=470 and b[3]<=372 and (b[2]-b[0])>=120 else 1)
PY
killrig
"$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1

# --- X7 "Capture cursor" on an X11 session: xcap has no pointer, XFixes supplies it
"$HERE/gui_env.sh" start "$N" "$PORT" >/dev/null 2>&1 || { bad "X7 rig start"; exit 1; }
PIC=$RIGHOME/Pictures/TrontSnap
newest() { ls -t "$PIC"/TrontSnap_*.png 2>/dev/null | head -1; }
"${X[@]}" move 900 650; sleep 1       # empty part of the window: no tile hover, no tooltip
setcursor() { sed -i '/^CaptureCursor=/d' "$RIGHOME/.config/trontsnap/settings.conf"; echo "CaptureCursor=$1" >> "$RIGHOME/.config/trontsnap/settings.conf"; }
setcursor 1; "$HERE/gui_env.sh" run "$N" "$BIN" full > "$D/x7_on.log" 2>&1; cp "$(newest)" "$D/x7_cursor_on.png"; sleep 2
setcursor 0; "$HERE/gui_env.sh" run "$N" "$BIN" full > "$D/x7_off.log" 2>&1; cp "$(newest)" "$D/x7_cursor_off.png"
# The first capture adds a tile to the live gallery (everything shifts), so compare only the empty
# strip around the pointer, not the whole frame.
bb=$(python3 - "$D/x7_cursor_on.png" "$D/x7_cursor_off.png" <<'PY'
import sys
from PIL import Image, ImageChops
box = (850, 615, 950, 690)
a = Image.open(sys.argv[1]).convert("RGB").crop(box); b = Image.open(sys.argv[2]).convert("RGB").crop(box)
bbox = ImageChops.difference(a, b).getbbox()
print("" if bbox is None else f"{bbox[0]+box[0]}, {bbox[1]+box[1]}, {bbox[2]+box[0]}, {bbox[3]+box[1]}")
PY
)
echo "      pointer diff bbox (strip around 900,650): ${bb:-none}"
cond "X7 CaptureCursor=1 differs from =0 by the pointer, at the pointer" "bbox=$bb" python3 - "$bb" <<'PY'
import sys
b=[int(v) for v in sys.argv[1].replace(' ','').split(',') if v]
# the cursor image is drawn with its hotspot at (900,650): the changed box must contain/touch it
sys.exit(0 if len(b)==4 and b[0]<=902 and b[2]>=898 and b[1]<=652 and b[3]>=648 and (b[2]-b[0])<=64 and (b[3]-b[1])<=64 else 1)
PY
"$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1
fi

# ============================================================ W: native Wayland (private GNOME Shell)
if (( RUN_W )); then
echo "== W: native Wayland (private headless GNOME Shell)"
W=$OUT/wl
WL=("$HERE/wl_env.sh")
"${WL[@]}" start >/dev/null 2>&1 || { bad "W rig start"; exit 1; }
k=$(kernel_check); [[ -n $k ]] && { echo "kernel log shows GPU trouble after the shell start; stopping:"; echo "$k"; exit 1; }
python3 - "$W/home/Pictures/TrontSnap/wl_probe.png" <<'PY'
import sys
from PIL import Image
im = Image.new("RGB", (800, 500), (30, 40, 60)); im.save(sys.argv[1])
PY
wshot() { "${WL[@]}" shot "$D/$1.png" >/dev/null 2>&1; }
# HARD-WON: the headless shell opens the Activities overview some seconds AFTER its socket is up (an
# early Escape is lost) and every click/shot afterwards lands in the overview. Escape until the
# dock-height strip is the plain desktop, never before the shell has had time to show it.
leave_overview() {
  local i
  sleep 7
  for i in 1 2 3 4; do
    wshot overview_probe
    pxok "$D/overview_probe.png" 683 712 'not (abs(r - b) < 20 and r > 25)' && return 0
    "${WL[@]}" input key Escape; sleep 2
  done
  return 1
}
pxok() { python3 - "$@" <<'PY'
import sys
from PIL import Image
im = Image.open(sys.argv[1]).convert("RGB"); x, y = int(sys.argv[2]), int(sys.argv[3])
p = im.getpixel((x, y)); sys.exit(0 if eval(sys.argv[4], {"r": p[0], "g": p[1], "b": p[2]}) else 1)
PY
}

leave_overview || bad "W0 could not leave the Activities overview"
# --- W1 --startup: minimized + app_id; the desktop shows through
(setsid "${WL[@]}" run env WAYLAND_DEBUG=1 "$BIN" --startup > "$D/w1_startup.dbg" 2>&1 < /dev/null &)
sleep 12
cond "W1 xdg_toplevel.set_app_id(\"trontsnap\") sent" "" grep -q 'set_app_id("trontsnap")' "$D/w1_startup.dbg"
cond "W1 --startup minimizes the window" "" grep -q 'set_minimized' "$D/w1_startup.dbg"
wshot w1_startup
cond "W1 nothing of the app is on screen (desktop blue at the centre)" "" pxok "$D/w1_startup.png" 683 400 'b > 100 and r < 40'
pid=$(gpid); [[ -n $pid ]] && kill "$pid" 2>/dev/null; sleep 1

# --- W2 buried window + second launch -> xdg-activation request
(setsid "${WL[@]}" run env WAYLAND_DEBUG=1 "$BIN" > "$D/w2_main.dbg" 2>&1 < /dev/null &)
sleep 10
(setsid "${WL[@]}" run "$BIN" edit "$W/home/Pictures/TrontSnap/wl_probe.png" > "$D/w2_edit.log" 2>&1 < /dev/null &)
sleep 9
before=$(grep -c 'get_activation_token' "$D/w2_main.dbg")
"${WL[@]}" run "$BIN" > /dev/null 2>&1; sleep 3
after=$(grep -c 'get_activation_token' "$D/w2_main.dbg")
cond "W2 second launch asks GNOME to raise the unfocused window" "get_activation_token $before -> $after" test "$after" -gt "$before"
wshot w2_second_launch
for p in $(pgrep -x trontsnap); do
  tr '\0' '\n' < "/proc/$p/cmdline" | grep -qx edit && kill "$p" 2>/dev/null
done
sleep 1

# --- W3 the X button quits (it was a silent no-op on Wayland)
wshot w3_before_X
pid=$(gpid)
"${WL[@]}" input abs 1342 52 wait 0.6 click; sleep 3
if kill -0 "$pid" 2>/dev/null; then bad "W3 title-bar X quits on native Wayland" "pid $pid alive"; else ok "W3 title-bar X quits on native Wayland" "pid $pid gone"; fi
cond "W3 command port released" "" port_free

# --- W4 "Capture cursor" draws the pointer into `trontsnap full`
"${WL[@]}" input abs 600 350 wait 1; sleep 1
PIC=$W/home/Pictures/TrontSnap
newest() { ls -t "$PIC"/TrontSnap_*.png 2>/dev/null | head -1; }
printf 'HasRun=1\nAutostartInit=1\nRecordAudio=0\nCaptureCursor=1\n' > "$W/home/.config/trontsnap/settings.conf"
"${WL[@]}" run "$BIN" full > "$D/w4_full_on.log" 2>&1; cp "$(newest)" "$D/w4_cursor_on.png"; sleep 2
printf 'HasRun=1\nAutostartInit=1\nRecordAudio=0\nCaptureCursor=0\n' > "$W/home/.config/trontsnap/settings.conf"
"${WL[@]}" run "$BIN" full > "$D/w4_full_off.log" 2>&1; cp "$(newest)" "$D/w4_cursor_off.png"
# Only the strip around the pointer: the shell's top-bar clock and notifications change between shots.
bb=$(python3 - "$D/w4_cursor_on.png" "$D/w4_cursor_off.png" <<'PY'
import sys
from PIL import Image, ImageChops
box = (540, 300, 680, 420)
a = Image.open(sys.argv[1]).convert("RGB").crop(box); b = Image.open(sys.argv[2]).convert("RGB").crop(box)
bbox = ImageChops.difference(a, b).getbbox()
print("" if bbox is None else f"{bbox[0]+box[0]}, {bbox[1]+box[1]}, {bbox[2]+box[0]}, {bbox[3]+box[1]}")
PY
)
echo "      pointer diff bbox (strip around 600,350): ${bb:-none}"
cond "W4 CaptureCursor=1 differs from =0 by exactly the pointer sprite" "bbox=$bb" python3 - "$bb" <<'PY'
import sys
b=[int(v) for v in sys.argv[1].replace(' ','').split(',') if v]
sys.exit(0 if len(b)==4 and 590<=b[0]<=612 and 340<=b[1]<=360 and 6<=b[2]-b[0]<=40 and 10<=b[3]-b[1]<=40 else 1)
PY

# --- W5 loupe wheel: +48 per notch, clamped to 96..528, persisted
printf 'HasRun=1\nAutostartInit=1\nRecordAudio=0\nCaptureCursor=0\nLoupeSize=132\n' > "$W/home/.config/trontsnap/settings.conf"
(setsid "${WL[@]}" run "$BIN" region > "$D/w5_region.log" 2>&1 < /dev/null &)
sleep 9
lsz() { grep '^LoupeSize=' "$W/home/.config/trontsnap/settings.conf" | cut -d= -f2; }
"${WL[@]}" input abs 700 400 wait 0.4 wheel 1 wait 0.6
cond "W5 one notch forward grows the loupe 132 -> 180" "LoupeSize=$(lsz)" test "$(lsz)" = 180
wshot w5_loupe_180
w=$(python3 - "$D/w5_loupe_180.png" <<'PY'
import sys
from PIL import Image
im = Image.open(sys.argv[1]).convert("RGB")
xs = [x for x in range(700, 1000) if (lambda p: p[2] > 200 and p[1] > 150 and p[0] < 140)(im.getpixel((x, 470)))]
print(xs[-1] - xs[0] + 1 if xs else 0)
PY
)
cond "W5 the painted loupe box is 180 px wide" "measured $w" test "$w" -ge 178 -a "$w" -le 184
"${WL[@]}" input abs 700 400 wait 0.3 wheel -1 wait 0.4 wheel -1 wait 0.4 wheel -1 wait 0.6
cond "W5 shrinking clamps at 96" "LoupeSize=$(lsz)" test "$(lsz)" = 96
"${WL[@]}" input abs 700 400 wait 0.3 $(for i in $(seq 1 11); do printf 'wheel 1 wait 0.4 '; done)
cond "W5 growing clamps at 528" "LoupeSize=$(lsz)" test "$(lsz)" = 528
killrig
"${WL[@]}" stop >/dev/null 2>&1
k=$(kernel_check); if [[ -n $k ]]; then bad "kernel log after the Wayland half" "$k"; else ok "kernel log clean (no uvd/vce/radeon errors/hung)"; fi
fi

echo
(( fails == 0 )) && echo "parity_test: ALL PASS" || echo "parity_test: $fails FAIL"
exit $(( fails > 0 ))
