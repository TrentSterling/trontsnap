#!/usr/bin/env bash
# gallery_test.sh [N] [PORT] — gallery regression checks on an isolated rig (default :91 / 48791).
# Exact state comes from the app's TRONTSNAP_TEST trace ("trontsnap-test: gallery shots= shown=
# selected= status=" lines in the rig's app.log); file-system facts (Trash, folder contents), the
# fake xdg-open log and window lookups back it up. A binary WITHOUT the trace (an older build, via
# TRONTSNAP_BIN=...) still runs every check that does not need it and SKIPs the rest.
#   - ghost tiles after Delete (key + context menu), after an external rm, Ctrl+A+Delete+confirm
#   - Esc in the search box keeps the selection, Ctrl+click twice never Opens, plain click toggles
#     once something is selected, Shift+click replaces the range
#   - Edit (annotate) opens the editor (path with spaces), Open reaps xdg-open (no zombie)
#   - Reveal stays responsive while the file manager answers slowly (fm1_mock.py)
#   - status pill: bottom-centre, expires by itself, top bar untouched
#   - 1500-file bulk delete reports chunked progress (runs off the UI thread)
# Screenshots/logs: $HARNESS_OUT/gallery_test/. Exit 1 on any FAIL. ~5 min (XINPUT_SLOW pacing).
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
N=${1:-91}; PORT=${2:-48791}
D=$OUT/gallery_test; mkdir -p "$D"
find "${D:?}" -maxdepth 1 -type f \( -name '*.png' -o -name '*.log' -o -name '*.pid' \) -delete
HOME_RIG=$OUT/homes/$N; TS=$HOME_RIG/Pictures/TrontSnap; SX=$HOME_RIG/Documents/ShareX/Screenshots
TRASH=$HOME_RIG/.local/share/Trash/files
APPLOG=$OUT/rigs/$N/app.log
X=("$HERE/xinput.py" -d ":$N")
export XINPUT_SLOW=${XINPUT_SLOW:-1}
export FAKE_OPEN_LOG=$D/open.log FAKE_NOTIFY_LOG=$D/notify.log
export PATH=$HERE/fake_bin:$PATH
: > "$FAKE_OPEN_LOG"
fails=0; skips=0; mockpid=""
ok()   { echo "  PASS  $1${2:+  [$2]}"; }
bad()  { echo "  FAIL  $1${2:+  [$2]}"; fails=$((fails + 1)); }
skip() { echo "  SKIP  $1${2:+  [$2]}"; skips=$((skips + 1)); }
cleanup() {
  [[ -n $mockpid ]] && kill "$mockpid" 2>/dev/null
  "$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1
}
trap cleanup EXIT

tx() { echo $(( 129 + 182 * $1 )); }
ty() { echo $(( 134 + 182 * $1 )); }
tile() { echo "$(tx "$1") $(ty "$2")"; }        # tile col row -> centre
park() { "${X[@]}" move 1270 700 >/dev/null; }
shot() { "$HERE/shot.sh" "$N" "$D/$1.png" "${@:2}" >/dev/null; }
last_trace() { grep 'trontsnap-test: gallery' "$APPLOG" | tail -1; }
# wait_trace REGEX [SECS] -> 0 once the LAST trace line matches; 2 when the binary has no trace
wait_trace() {
  (( have_trace )) || return 2
  local i; for (( i = 0; i < ${2:-8} * 5; i++ )); do
    [[ $(last_trace) =~ $1 ]] && return 0; sleep 0.2
  done; return 1
}
# check NAME REGEX [SECS]: a trace assertion, SKIP when the binary has none
check() {
  wait_trace "$2" "${3:-8}"; case $? in
    0) ok "$1" "$(last_trace | sed 's/.*gallery //')" ;;
    2) skip "$1" "binary has no TRONTSNAP_TEST trace" ;;
    *) bad "$1" "wanted /$2/, last: $(last_trace | sed 's/.*gallery //')" ;;
  esac
}
files() { find "$TS" "$SX" -type f 2>/dev/null | wc -l; }
trashed() { ls "$TRASH" 2>/dev/null | wc -l; }
app_pid() { cat "$OUT/rigs/$N/app.pid"; }
zombies() { ps -eo ppid,stat | awk -v p="$(app_pid)" '$1==p && $2 ~ /Z/' | wc -l; }
changed() { "$HERE/imgdiff.py" "$1" "$2" | sed -n 's/changed_pixels=\([0-9]*\).*/\1/p'; }
newest_png() { ls "$TS"/TrontSnap_*.png 2>/dev/null | tail -1; }

# === rig ===
"$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1
RIG_SEED_HOOK=$HERE/seed_gallery.sh "$HERE/gui_env.sh" start "$N" "$PORT" || { echo "  FAIL  rig start"; exit 1; }
sleep 1
have_trace=0; grep -q 'trontsnap-test: gallery' "$APPLOG" && have_trace=1
n0=$(files)
if (( have_trace )); then check "boot: gallery lists every seeded file" "shots=$n0 shown=$n0 selected=0 "
else skip "boot trace" "binary has no TRONTSNAP_TEST trace ($n0 files seeded)"; fi
park; shot 00_idle --settle

# === G3: Esc in the search box must not clear the selection ===
"${X[@]}" ctrlclick $(tile 3 0); "${X[@]}" ctrlclick $(tile 5 0)
check "G3 two tiles selected" "selected=2 "
"${X[@]}" click 728 20; sleep 0.4
"${X[@]}" key Escape; sleep 0.8
check "G3 Esc inside the search box keeps the selection" "selected=2 "
park; "${X[@]}" key Escape
check "G3 Esc outside the search box clears it" "selected=0 "

# === G12: selection gestures ===
"${X[@]}" ctrlclick $(tile 0 1)
"${X[@]}" click $(tile 1 1)
check "G12 plain click toggles once something is selected (no copy)" "selected=2 "
"${X[@]}" key Escape; sleep 0.5
"${X[@]}" ctrlclick $(tile 5 0); "${X[@]}" ctrlclick $(tile 0 1)
"${X[@]}" shiftclick $(tile 3 1)
check "G12 Shift+click replaces the selection with the range (4 tiles, the outside one dropped)" "selected=4 "
"${X[@]}" key Escape; sleep 0.5

# === G6: Ctrl+click twice, fast, never opens the file ===
before=$(wc -l < "$FAKE_OPEN_LOG")
env -u XINPUT_SLOW "${X[@]}" "ctrlclick $(tile 2 1)" "ctrlclick $(tile 2 1)"
sleep 1.5
after=$(wc -l < "$FAKE_OPEN_LOG")
if (( after == before )); then ok "G6 quick Ctrl+click x2 did not Open the file"
else bad "G6 quick Ctrl+click x2 opened the file" "$(tail -1 "$FAKE_OPEN_LOG")"; fi
check "G6 ...and toggled it back off" "selected=0 "

# === G9: Open goes through a reaped xdg-open ===
before=$(wc -l < "$FAKE_OPEN_LOG")
env -u XINPUT_SLOW "${X[@]}" "doubleclick $(tile 4 0)"
sleep 2
if (( $(wc -l < "$FAKE_OPEN_LOG") == before + 1 )); then ok "G9 double-click Open reached xdg-open once"
else bad "G9 double-click Open" "log: $(tail -2 "$FAKE_OPEN_LOG" | tr '\n' '|')"; fi
if (( $(zombies) == 0 )); then ok "G9 no zombie xdg-open left behind"
else bad "G9 zombie xdg-open children" "$(zombies)"; fi
"${X[@]}" key Escape; park

# === G2: Edit (annotate) opens the editor, also for a path with spaces ===
"${X[@]}" rightclick $(tile 3 2); sleep 0.8
shot 02_ctxmenu --settle
"${X[@]}" click $(( $(tx 3) + 60 )) $(( $(ty 2) + 121 ))
found=0; for _ in $(seq 1 40); do "${X[@]}" find "TrontSnap Edit" >/dev/null 2>&1 && { found=1; break; }; sleep 0.5; done
if (( found )); then ok "G2 Edit (annotate) opened the editor window"
else bad "G2 Edit (annotate) opened nothing" "$(grep -a 'could not open' "$APPLOG" | tail -1)"; fi
for pid in $(ps -eo pid,ppid,cmd | awk -v p="$(app_pid)" '$2==p && / edit / {print $1}'); do kill "$pid" 2>/dev/null; done
sleep 1; "${X[@]}" focus >/dev/null 2>&1; park

# === G5: the status pill: bottom-centre, expires on its own, top bar untouched ===
shot 03_pill_before --crop 400x90+390+670; shot 03_top_before --crop 1180x40+0+0
"${X[@]}" click $(tile 5 0)      # nothing selected -> copy
check "G5 'Copied' status set" 'status="Copied"'
park; sleep 0.4; shot 03_pill_during --crop 400x90+390+670; shot 03_top_during --crop 1180x40+0+0; shot 03_full_pill
c=$(changed "$D/03_pill_before.png" "$D/03_pill_during.png")
if (( ${c:-0} > 300 )); then ok "G5 pill painted bottom-centre" "$c px"; else bad "G5 pill not painted bottom-centre" "${c:-?} px"; fi
c=$(changed "$D/03_top_before.png" "$D/03_top_during.png")
if (( ${c:-0} < 60 )); then ok "G5 title bar untouched by the message (it used to draw under the window buttons)" "${c:-0} px"
else bad "G5 title bar changed" "${c:-?} px"; fi
check "G5 pill expires by itself on an idle window" 'status=""' 9
shot 03_pill_after --crop 400x90+390+670
c=$(changed "$D/03_pill_before.png" "$D/03_pill_after.png")
if (( ${c:-0} < 60 )); then ok "G5 pill gone after the timeout" "${c:-0} px"; else bad "G5 pill still painted" "${c:-?} px"; fi

# === G4: Reveal must not freeze the window while the file manager answers slowly ===
(setsid "$HERE/gui_env.sh" run "$N" python3 "$HERE/fm1_mock.py" --delay 8 --log "$D/reveal.log" > "$D/mock.out" 2>&1 < /dev/null & echo $! > "$D/mock.pid")
mockpid=$(cat "$D/mock.pid"); sleep 2
grep -q 'mock up' "$D/reveal.log" 2>/dev/null || bad "G4 mock file manager did not start" "$(cat "$D/mock.out")"
HC="172x172+$(( $(tx 2) - 86 ))+$(( $(ty 2) - 86 ))"
park; shot 04_hover_base --crop "$HC" --settle
"${X[@]}" rightclick $(tile 1 0); sleep 0.8
"${X[@]}" click $(( $(tx 1) + 60 )) $(( $(ty 0) + 155 ))     # "Show in file manager"
"${X[@]}" move $(tile 2 2); sleep 1.0
shot 04_hover_during --crop "$HC"
c=$(changed "$D/04_hover_base.png" "$D/04_hover_during.png")
if (( ${c:-0} > 300 )); then ok "G4 window still paints hover while Reveal waits on the file manager" "$c px"
else bad "G4 window frozen during Reveal" "${c:-0} px hover change"; fi
if grep -q ShowItems "$D/reveal.log" 2>/dev/null; then ok "G4 Reveal asked FileManager1.ShowItems"
else bad "G4 no ShowItems call" "$(cat "$D/reveal.log" 2>/dev/null)"; fi
for _ in $(seq 1 30); do grep -q 'Pictures/TrontSnap$' "$FAKE_OPEN_LOG" && break; sleep 0.5; done
if grep -q 'Pictures/TrontSnap$' "$FAKE_OPEN_LOG"; then ok "G4 fell back to opening the folder after the file manager timed out"
else bad "G4 no folder fallback" "$(tail -2 "$FAKE_OPEN_LOG" | tr '\n' '|')"; fi
kill "$mockpid" 2>/dev/null; mockpid=""
park

# === G1: deletes leave no ghost tiles ===
t0=$(trashed); n1=$(files)
"${X[@]}" ctrlclick $(tile 1 0); "${X[@]}" key Delete; sleep 2
if (( $(trashed) == t0 + 1 )); then ok "G1 Delete key trashed the selected file"; else bad "G1 Delete key" "trash $t0 -> $(trashed)"; fi
check "G1 no ghost tile after Delete (count dropped, selection empty)" "shots=$((n1 - 1)) shown=$((n1 - 1)) selected=0 " 8
sleep 1; check "G1 ...and it stays that way after the watcher's late events" "shots=$((n1 - 1)) shown=$((n1 - 1)) selected=0 "
shot 05_after_delete --settle
# context-menu Delete of a SELECTED tile used to leave it selected (and re-add a ghost)
"${X[@]}" ctrlclick $(tile 1 0); "${X[@]}" rightclick $(tile 1 0); sleep 0.8
"${X[@]}" click $(( $(tx 1) + 55 )) $(( $(ty 0) + 237 )); sleep 2
check "G1 context-menu Delete of a selected tile: gone and deselected" "shots=$((n1 - 2)) shown=$((n1 - 2)) selected=0 "
# a file removed outside the app leaves the grid too
rm -f -- "$(newest_png)"
check "G1 external rm removes the tile" "shots=$((n1 - 3)) shown=$((n1 - 3)) selected=0 " 6
# Ctrl+A, Delete, confirm — with a file vanishing UNDER the dialog (the batch used to abort on it)
"${X[@]}" key ctrl+a; sleep 0.5; "${X[@]}" key Delete; sleep 1
shot 06_confirm --settle
rm -f -- "$(newest_png)"
t0=$(trashed); left=$(files)
"${X[@]}" click 520 422; sleep 3
if (( $(files) == 0 )); then ok "G1 Ctrl+A + Delete + confirm emptied the folders despite a vanished path" "$left files -> 0"
else bad "G1 bulk delete left files behind" "$(files) of $left"; fi
if (( $(trashed) >= t0 + left - 1 )); then ok "G1 the rest really landed in the Trash" "+$(( $(trashed) - t0 ))"
else bad "G1 trash count" "$t0 -> $(trashed), expected +$((left - 1))"; fi
check "G1 gallery empty, status is a success message" 'shots=0 shown=0 selected=0 status="Moved [0-9]+ to the Trash"'
if grep -q 'bulk delete failed' "$APPLOG"; then bad "G1 bulk delete logged a failure" "$(grep 'bulk delete failed' "$APPLOG" | tail -1)"
else ok "G1 no 'bulk delete failed' in the app log"; fi

# === G7: a big bulk delete runs in chunks off the UI thread ===
python3 - "$TS" <<'PY'
import sys
from PIL import Image
d = sys.argv[1]
for i in range(1500):
    Image.new("RGB", (32, 24), (i % 256, (i * 7) % 256, (i * 13) % 256)).save(f"{d}/bulk_{i:04d}.png")
PY
check "G7 1500 new files spliced in live" "shots=1500 shown=1500 selected=0 " 25
"${X[@]}" key ctrl+a; sleep 0.5; "${X[@]}" key Delete; sleep 1
"${X[@]}" click 520 422
sleep 12
if (( have_trace )); then
  np=$(grep -c 'status="Moving to the Trash: [0-9]*/1500"' "$APPLOG")
  if (( np >= 1 )); then ok "G7 UI got chunked progress while the worker ran" "$np progress lines"; else bad "G7 no chunked progress" "$np lines"; fi
else skip "G7 progress" "binary has no TRONTSNAP_TEST trace"; fi
if (( $(files) == 0 )); then ok "G7 all 1500 trashed"; else bad "G7 files left" "$(files)"; fi
# (the pill has expired by now: look at the whole trace, not the last line)
if (( have_trace )); then
  if grep -q 'gallery shots=0 shown=0 selected=0 status="Moved 1500 to the Trash"' "$APPLOG"; then ok "G7 final status: Moved 1500 to the Trash"
  else bad "G7 no 'Moved 1500 to the Trash' status" "last: $(last_trace | sed 's/.*gallery //')"; fi
else skip "G7 final status" "binary has no TRONTSNAP_TEST trace"; fi

echo "  logs: $D/  (app log: $APPLOG)"
if (( fails == 0 )); then echo "gallery_test: PASS ($skips skipped)"; else echo "gallery_test: $fails FAIL ($skips skipped)"; fi
exit $(( fails > 0 ))
