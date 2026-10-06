#!/usr/bin/env bash
# paste_test.sh [N] [PORT] — what actually lands on the clipboard, asserted. Own rig
# (default :92 / 48792), stopped after. Covers the paste audit fixes:
#   A video tile (special name) -> Nautilus-style file set, RFC 3986 percent-encoded URIs, CRLF
#   B still tile                -> image/png + the saved file (uri-list + gnome-copied-files)
#   C Ctrl+C, 2 selected        -> both files (egui-winit swallows Key::C; we read Event::Copy)
#   D Ctrl+C, 1 selected        -> that shot's image
#   E Ctrl+C in the search box  -> does NOT copy files
#   F `trontsnap full` (xcap path) -> image/png byte-identical to the saved file + the file set
#   G TRONTSNAP_CLIPSERVE=off   -> single-target fallback still works and logs why
#   H 10 MB PNG through INCR    -> 3 identical reads, owner exits when replaced
#   I `trontsnap ocr` w/o tesseract -> visible notification, clipboard untouched
#   J "Copy 2 files" / "Copy 2 paths" menu items; paths survive the app being killed
#   K 20000 paths through clip-serve's stdin (argv would hit ARG_MAX); a 36 MB copy then a small one end in click order (guards the FIFO; cannot reproduce the old race)
# Screenshots/logs in $HARNESS_OUT/paste_test/. Exit 0 = all PASS.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
N=${1:-92}; PORT=${2:-48792}
S="$OUT/paste_test"; mkdir -p "$S"
X=("$HERE/xinput.py" -d ":$N")
BIN=${TRONTSNAP_BIN:-$REPO/target/debug/trontsnap}
FAKE="$HERE/fake_bin"
HOME_N="$OUT/homes/$N"
fails=0
ok() { echo "  PASS  $1${2:+  [$2]}"; }
bad() { echo "  FAIL  $1${2:+  [$2]}"; fails=$((fails + 1)); }
check() { if eval "$2"; then ok "$1" "${3:-}"; else bad "$1" "${3:-} cond: $2"; fi; }
export XINPUT_SLOW=${XINPUT_SLOW:-1}
tgts() { "$HERE/clip.sh" "$N" 2>/dev/null | tr '\n' ' '; }
has() { [[ " $1 " == *" $2 "* ]]; }
park() { "${X[@]}" move 1270 700; sleep 0.6; }
setsent() { printf SENTINEL | xclip -display ":$N" -selection clipboard -i; sleep 0.4; }
# owner pids for this display's clip-serve children (exact: they carry no rig marker, so
# list all and let the caller compare before/after).
serve_pids() { pgrep -f '[c]lip-serve (image|files)' | sort | tr '\n' ' '; }

trap '"$HERE/gui_env.sh" stop "$N" >/dev/null 2>&1' EXIT
RIG_SEED_HOOK="$HERE/paste_seed.sh" "$HERE/gui_env.sh" start "$N" "$PORT" >/dev/null || { echo "  FAIL  rig start"; exit 1; }
ok "rig started" ":$N port $PORT"
park

# === A: video tile with an awkward name ===
setsent
"${X[@]}" click 128 134; sleep 2
t=$(tgts)
for want in text/uri-list x-special/gnome-copied-files UTF8_STRING text/plain\;charset=utf-8; do
  check "A: video copy offers $want" 'has "$t" "$want"' "$t"
done
check "A: video copy offers no image/png" '! has "$t" image/png'
uri=$("$HERE/clip.sh" "$N" text/uri-list | tr -d '\r\n')
want_uri=$(python3 -c 'import sys,urllib.parse as u,os; p=sys.argv[1]+"/Pictures/TrontSnap/Screen Rec #1 (final) 100%é.mp4"; print("file://"+u.quote(p,safe="/-._~"),end="")' "$HOME_N")
check "A: uri-list is the percent-encoded URI" '[[ $uri == "$want_uri" ]]' "$uri"
check "A: no raw # or space in the URI" '[[ $uri != *"#"* && $uri != *" "* ]]'
crlf=$("$HERE/clip.sh" "$N" text/uri-list | od -An -c | tr -d ' \n')
check "A: uri-list line ends CRLF" '[[ $crlf == *\\r\\n ]]' "$crlf"
dec=$(python3 -c 'import sys,urllib.parse as u; print(u.unquote(sys.argv[1][7:]),end="")' "$uri")
check "A: decoded URI is the real file" '[[ -f $dec ]]' "$dec"
gcf=$("$HERE/clip.sh" "$N" x-special/gnome-copied-files)
check "A: gnome-copied-files = copy + LF + URI, no CR" '[[ $gcf == "copy"$'"'"'\n'"'"'"$uri" ]]'
txt=$("$HERE/clip.sh" "$N" UTF8_STRING)
check "A: bare path text for terminals" '[[ $txt == "$dec" ]]' "$txt"

# === B: still tile ===
setsent
"${X[@]}" click 492 134; sleep 2.5
t=$(tgts)
for want in image/png text/uri-list x-special/gnome-copied-files; do
  check "B: still copy offers $want" 'has "$t" "$want"' "$t"
done
check "B: still copy offers no text/plain (path text could beat the picture in rich fields)" \
  '! has "$t" UTF8_STRING && ! has "$t" text/plain'
f=$(python3 -c 'import sys,urllib.parse as u; print(u.unquote(sys.argv[1][7:]),end="")' \
    "$("$HERE/clip.sh" "$N" text/uri-list | tr -d '\r\n')")
"$HERE/clip.sh" "$N" image/png "$S/B_clip.png" >/dev/null
check "B: image/png byte-identical to the file the URI names" \
  '[[ $(sha1sum < "$S/B_clip.png") == $(sha1sum < "$f") ]]' "$f"

# === C: Ctrl+C with 2 selected ===
park; "${X[@]}" key Escape; sleep 0.5
"${X[@]}" ctrlclick 492 134; "${X[@]}" ctrlclick 674 134; park
setsent
"${X[@]}" key ctrl+c; sleep 2.5
t=$(tgts)
check "C: ctrl+c replaced SENTINEL with a file set" 'has "$t" text/uri-list' "$t"
nuri=$("$HERE/clip.sh" "$N" text/uri-list | grep -c '^file://')
check "C: uri-list names 2 files" '(( nuri == 2 ))' "$nuri"
"$HERE/shot.sh" "$N" "$S/C_two_selected.png" --settle >/dev/null

# === E: Ctrl+C in the search box must not copy the selection's files ===
"${X[@]}" click 728 20; sleep 0.8
setsent
"${X[@]}" key ctrl+c; sleep 1.5
t=$(tgts)
check "E: ctrl+c in the search box did not copy files" '! has "$t" text/uri-list' "$t"
"${X[@]}" key Escape; park

# === D: Ctrl+C with 1 selected -> the image ===
"${X[@]}" key Escape; sleep 0.5
"${X[@]}" click 1160 400; sleep 0.5   # blank grid area: drop search focus
"${X[@]}" key Escape; sleep 0.5
"${X[@]}" ctrlclick 856 134; park
setsent
"${X[@]}" key ctrl+c; sleep 2.5
t=$(tgts)
check "D: ctrl+c, 1 selected -> image/png" 'has "$t" image/png' "$t"

# === F/G: one-shot capture path (xcap under Xvfb) ===
P="$FAKE:$PATH"
newest() { ls -t "$HOME_N"/Pictures/TrontSnap/TrontSnap_*.png 2>/dev/null | head -1; }
before=$(newest)
s=$(date +%s.%N)
"$HERE/gui_env.sh" run "$N" env PATH="$P" "$BIN" full > "$S/F_full.out" 2>&1
e=$(date +%s.%N)
sleep 1.5
t=$(tgts)
after=$(newest)
check "F: trontsnap full saved a new shot" '[[ -n $after && $after != "$before" ]]' "$after"
for want in image/png text/uri-list x-special/gnome-copied-files; do check "F: full offers $want" 'has "$t" "$want"' "$t"; done
"$HERE/clip.sh" "$N" image/png "$S/F_clip.png" >/dev/null
check "F: clipboard image/png == saved file (no re-encode)" '[[ $(sha1sum < "$S/F_clip.png") == $(sha1sum < "$after") ]]'
echo "        (trontsnap full in the rig took $(python3 -c "print(round($e-$s,2))") s)"

before=$(newest); sleep 1.1   # timestamped name has 1 s resolution
"$HERE/gui_env.sh" run "$N" env PATH="$P" TRONTSNAP_CLIPSERVE=off "$BIN" full > "$S/G_full.out" 2>&1
sleep 1.5
t=$(tgts)
check "G: clip-serve off -> single-target image/png fallback" 'has "$t" image/png && ! has "$t" text/uri-list' "$t"
check "G: ...and the reason is in trontsnap.log" 'grep -q "clip-serve failed" "$HOME_N/.local/share/trontsnap/trontsnap.log"'

# === H: INCR + owner lifetime ===
python3 - "$HOME_N/big_noise.png" <<'PY'
import os, sys
from PIL import Image
w, h = 2400, 1500
Image.frombytes("RGB", (w, h), os.urandom(w * h * 3)).save(sys.argv[1])
PY
big="$HOME_N/big_noise.png"
(setsid "$HERE/gui_env.sh" run "$N" "$BIN" clip-serve image "$big" < "$big" > "$S/H_child.out" 2>&1 &)
for _ in $(seq 1 30); do grep -q READY "$S/H_child.out" 2>/dev/null && break; sleep 0.2; done
check "H: clip-serve reported READY" 'grep -q READY "$S/H_child.out"'
h1=$("$HERE/clip.sh" "$N" image/png | sha1sum); h2=$("$HERE/clip.sh" "$N" image/png | sha1sum); h3=$("$HERE/clip.sh" "$N" image/png | sha1sum)
want=$(sha1sum < "$big")
check "H: 10 MB PNG (INCR) read back identical x3" '[[ $h1 == "$want" && $h2 == "$want" && $h3 == "$want" ]]' "$(stat -c %s "$big") bytes"
before_pids=$(serve_pids)
setsent
sleep 1
check "H: replaced owner exits (no pile-up)" '[[ -z $(pgrep -f "[c]lip-serve image $big") ]]'
rm -f "$big"

# === I: OCR without tesseract tells the user ===
if command -v tesseract >/dev/null; then
  echo "  SKIP  I: tesseract is installed here"
else
  : > "$S/I_notify.log"; setsent
  timeout 20 "$HERE/gui_env.sh" run "$N" env PATH="$P" FAKE_NOTIFY_LOG="$S/I_notify.log" "$BIN" ocr > "$S/I_ocr.out" 2>&1
  sleep 1
  check "I: ocr without tesseract raised a notification" 'grep -q "OCR unavailable" "$S/I_notify.log"' "$(cat "$S/I_notify.log")"
  check "I: clipboard untouched" '[[ $("$HERE/clip.sh" "$N" UTF8_STRING 2>/dev/null) == SENTINEL ]]'
fi


# === K: a huge file list rides stdin; two quick copies land in click order ===
# HARD-WON: the file list used to be argv. ~17k paths is near ARG_MAX; the spawn failed with E2BIG and the
# writer silently fell back to xclip with only text/uri-list.
python3 - "$S/K_paths.bin" "$HOME_N" <<'PY'
import sys
out, home = sys.argv[1:3]
with open(out, "wb") as f:
    for i in range(20000):
        f.write(f"{home}/Pictures/TrontSnap/k_{i:05d}_{'x' * 60}.png".encode() + b"\0")
PY
argv_bytes=$(tr '\0' ' ' < "$S/K_paths.bin" | wc -c)
(setsid "$HERE/gui_env.sh" run "$N" "$BIN" clip-serve files < "$S/K_paths.bin" > "$S/K_child.out" 2>&1 &)
for _ in $(seq 1 50); do grep -q READY "$S/K_child.out" 2>/dev/null && break; sleep 0.2; done
check "K: clip-serve files (20000 paths on stdin, $argv_bytes bytes: more than ARG_MAX) reported READY" 'grep -q READY "$S/K_child.out"' "$(cat "$S/K_child.out")"
nuri=$("$HERE/clip.sh" "$N" text/uri-list | grep -c '^file://')
check "K: all 20000 URIs are on the clipboard" '(( nuri == 20000 ))' "$nuri"
npath=$("$HERE/clip.sh" "$N" UTF8_STRING | grep -c 'Pictures/TrontSnap/k_')
check "K: ...and all 20000 bare paths (the terminal flavour)" '(( npath == 20000 ))' "$npath"
setsent; sleep 1
check "K: replaced owner exits" '[[ -z $(pgrep -f "[c]lip-serve files") ]]'
rm -f "$S/K_paths.bin"

# Two plain clicks 'copy' two shots; the LAST click must own the clipboard (a single FIFO worker now; it was
# a thread per copy, where whichever owner claimed CLIPBOARD last won). HONEST LIMIT: the pre-fix binary also
# PASSES this (checked): the race needs two clicks inside ~100 ms and XTest pacing on this box can't produce
# that, so this guards the FIFO against breaking order or deadlocking, it does not reproduce the old race.
park; "${X[@]}" key Escape; sleep 0.5
bigf="$HOME_N/Pictures/TrontSnap/TrontSnap_$(date +%F_%H-%M-%S).png"
python3 - "$bigf" <<'PY'
import os, sys
from PIL import Image
Image.frombytes("RGB", (4000, 3000), os.urandom(4000 * 3000 * 3)).save(sys.argv[1], compress_level=0)
PY
sleep 8   # the watcher splices it in as the NEWEST tile: tile 1 is now the big one
setsent; "${X[@]}" click 310 134; sleep 3        # what tile 2 alone puts on the clipboard = the expected end state
want_uri=$("$HERE/clip.sh" "$N" text/uri-list | tr -d '\r')
dist=0
for rep in 1 2; do
  setsent
  "${X[@]}" click 128 134                         # the big one
  env -u XINPUT_SLOW "${X[@]}" move 310 134; sleep 0.3
  env -u XINPUT_SLOW "${X[@]}" click 310 134      # then the small one, while the big hand-over is still going
  sleep 7
  got=$("$HERE/clip.sh" "$N" text/uri-list | tr -d '\r')
  [[ $got == "$want_uri" ]] || dist=$((dist + 1))
  park; sleep 0.5
done
check "K: big copy then small copy x2: the clipboard always ends as the LAST click's file" '(( dist == 0 ))' "$dist of 2 ended on the big one ($(stat -c %s "$bigf") bytes)"
rm -f "$bigf"; sleep 3   # let the watcher drop the tile again: J expects the old layout

# === J: menu items + survival after the app is gone ===
# Tiles 1+2 are the two newest STILLS here (F/G just added them), so the menu has the
# still layout: pointer + (45,285) = "Copy N files", + (49,319) = "Copy N paths".
"${X[@]}" click 1160 400; "${X[@]}" key Escape; sleep 0.5
"${X[@]}" ctrlclick 128 134; "${X[@]}" ctrlclick 310 134; "${X[@]}" rightclick 128 134; sleep 1
"$HERE/shot.sh" "$N" "$S/J_menu.png" --settle >/dev/null
setsent
"${X[@]}" click 173 419; sleep 2.5     # "Copy 2 files"
t=$(tgts)
check "J: 'Copy 2 files' -> uri-list" 'has "$t" text/uri-list' "$t"
nuri=$("$HERE/clip.sh" "$N" text/uri-list | grep -c '^file://')
check "J: 'Copy 2 files' names 2 files" '(( nuri == 2 ))' "$nuri"
"${X[@]}" rightclick 128 134; sleep 1
setsent
"${X[@]}" click 177 453; sleep 2       # "Copy 2 paths"
check "J: 'Copy 2 paths' -> text with 2 lines" '[[ $("$HERE/clip.sh" "$N" UTF8_STRING 2>/dev/null | grep -c Pictures/TrontSnap) == 2 ]]' "$(tgts)"
"$HERE/shot.sh" "$N" "$S/J_after_paths.png" --settle >/dev/null
kill -TERM "$(cat "$OUT/rigs/$N/app.pid")" 2>/dev/null; sleep 2
check "J: app is gone" '! kill -0 "$(cat "$OUT/rigs/$N/app.pid")" 2>/dev/null'
check "J: copied paths survive the app exiting" '[[ $("$HERE/clip.sh" "$N" UTF8_STRING 2>/dev/null | grep -c Pictures/TrontSnap) == 2 ]]' "$(tgts)"

echo "  screenshots/logs: $S/"
(( fails == 0 )) && echo "paste_test: PASS" || echo "paste_test: $fails FAIL"
exit $(( fails > 0 ))
