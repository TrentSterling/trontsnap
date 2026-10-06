#!/usr/bin/env bash
# ff_paste_case.sh N WINID NAME -- <command that sets the clipboard on :N, backgrounded by this script>
# Sets the clipboard, pastes (ctrl+v) into the Firefox paste_page.html contenteditable on the
# rig, screenshots the page. Reload first so each case starts from "(nothing pasted yet)".
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
n=$1; win=$2; name=$3; shift 4
OUT=${HARNESS_OUT:-$HERE/../../../harness_out/$(date +%F)}/audit-paste
"$HERE/xinput.py" -d ":$n" focus "$win" >/dev/null
"$HERE/xinput.py" -d ":$n" key ctrl+r; sleep 3
( "$@" > "$OUT/ffcase_$name.owner.log" 2>&1 & )
sleep 3
echo "[$name] TARGETS: $("$HERE/clip.sh" "$n" | tr '\n' ' ')"
"$HERE/xinput.py" -d ":$n" move 600 330; sleep 0.5
"$HERE/xinput.py" -d ":$n" click 600 330; sleep 0.8
"$HERE/xinput.py" -d ":$n" key ctrl+v; sleep 3
"$HERE/shot.sh" "$n" "$OUT/ffcase_$name.png" --settle --crop 1239x470+0+85 >/dev/null
echo "[$name] shot: $OUT/ffcase_$name.png"
