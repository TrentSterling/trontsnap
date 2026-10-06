#!/usr/bin/env bash
# selftest.sh [N] [PORT] — smoke-test the GUI rig + input driver + screenshots +
# clipboard end to end. Starts its own rig (default :97 / 48797), stops it after.
# Screenshots go to $HARNESS_OUT/selftest/ (Read them: pixel diffs only say "changed").
# Exit 0 = all PASS.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
N=${1:-97}; PORT=${2:-48797}
S="$OUT/selftest"; mkdir -p "$S"
X=("$HERE/xinput.py" -d ":$N")
fails=0
ok() { echo "  PASS  $1${2:+  [$2]}"; }
bad() { echo "  FAIL  $1${2:+  [$2]}"; fails=$((fails + 1)); }
changed() { "$HERE/imgdiff.py" "$1" "$2" | sed -n 's/changed_pixels=\([0-9]*\).*/\1/p'; }
park() { "${X[@]}" move 1270 700; sleep 0.8; }   # off the tiles, off the toast area

# HARD-WON: the debug app on this 4-core box drops/lags input once other agents load the machine
# (load avg 7+: "ctrl+a  0 px" and "1 selected" instead of 2). Paced input makes the selftest load-proof.
export XINPUT_SLOW=${XINPUT_SLOW:-1}
trap '"$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1' EXIT
"$HERE/gui_env.sh" start "$N" "$PORT" || { echo "  FAIL  rig start"; exit 1; }
ok "rig started" ":$N port $PORT"

park; "$HERE/shot.sh" "$N" "$S/1_idle.png" --settle >/dev/null
"${X[@]}" ctrlclick 310 134; sleep 0.3; "${X[@]}" ctrlclick 492 134; park
"$HERE/shot.sh" "$N" "$S/2_two_selected.png" --settle >/dev/null
c=$(changed "$S/1_idle.png" "$S/2_two_selected.png")
(( ${c:-0} > 20000 )) && ok "ctrl+click x2 changed the gallery (selection highlight)" "$c px" \
  || bad "ctrl+click x2 changed the gallery" "${c:-?} px (Read $S/2_two_selected.png)"

"${X[@]}" key ctrl+a; park; "$HERE/shot.sh" "$N" "$S/3_all.png" --settle >/dev/null
c=$(changed "$S/2_two_selected.png" "$S/3_all.png")
(( ${c:-0} > 20000 )) && ok "ctrl+a selected everything" "$c px vs two-selected" || bad "ctrl+a" "${c:-?} px"

"${X[@]}" key Escape; park; "$HERE/shot.sh" "$N" "$S/4_cleared.png" --settle >/dev/null
c=$(changed "$S/1_idle.png" "$S/4_cleared.png")
(( ${c:-99999} < 2000 )) && ok "Escape cleared the selection (back to idle)" "$c px vs idle" || bad "Escape" "${c:-?} px vs idle"

# Plain click = the gallery's "copy to clipboard" gesture -> xclip must see image/png.
"${X[@]}" click 310 134; sleep 1.5
t=$("$HERE/clip.sh" "$N" 2>&1 | tr '\n' ' ')
if [[ $t == *image/png* ]]; then
  "$HERE/clip.sh" "$N" image/png "$S/5_clip.png" >/dev/null
  dim=$(python3 -c "from PIL import Image; print('x'.join(map(str, Image.open('$S/5_clip.png').size)))" 2>/dev/null)
  ok "plain click copied a PNG to the rig clipboard" "TARGETS: $t dims $dim"
else
  bad "plain click clipboard" "$t"
fi

echo "  screenshots: $S/"
(( fails == 0 )) && echo "selftest: PASS" || echo "selftest: $fails FAIL"
exit $(( fails > 0 ))
