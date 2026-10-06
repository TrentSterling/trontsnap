#!/usr/bin/env bash
# shot.sh N [OUT.png] [--crop WxH+X+Y] [--settle] — screenshot an X display (rig or any).
# Prints the output path. Default path: $HARNESS_OUT/shots/N_<time>.png
# --settle   re-grab every 0.5 s until two consecutive frames are byte-identical (max ~8 s):
#            thumbnails/animations on the software-GL debug build land late, and a shot taken
#            mid-load makes later pixel diffs lie.
# SHOT_MOUSE=0 hides the pointer overlay.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
n=${1:?usage: shot.sh N [out.png] [--crop WxH+X+Y]}; shift
out=""; crop=""; settle=0
while (( $# )); do
  case $1 in
    --crop) crop=$2; shift ;;
    --settle) settle=1 ;;
    *) out=$1 ;;
  esac
  shift
done
mkdir -p "$(dirname "${out:-$OUT/shots/x}")"
[[ -n $out ]] || { out="$OUT/shots/${n}_$(date +%H%M%S).png"; }
size=$(DISPLAY=":$n" xdpyinfo 2>/dev/null | awk '/dimensions:/ {print $2; exit}')
[[ -n $size ]] || { echo "shot: cannot reach display :$n" >&2; exit 1; }
vf=()
if [[ -n $crop ]]; then
  IFS='x+' read -r cw ch cx cy <<< "$crop"
  vf=(-vf "crop=$cw:$ch:$cx:$cy")
fi
grab() {
  ffmpeg -hide_banner -loglevel error -y -f x11grab -draw_mouse "${SHOT_MOUSE:-1}" -video_size "$size" \
    -i ":$n" -frames:v 1 "${vf[@]}" "$1"
}
if (( settle )); then
  prev=""
  for _ in $(seq 1 16); do
    grab "$out" || exit 1
    cur=$(md5sum < "$out")
    [[ $cur == "$prev" ]] && break
    prev=$cur; sleep 0.5
  done
else
  grab "$out" || exit 1
fi
echo "$out"
