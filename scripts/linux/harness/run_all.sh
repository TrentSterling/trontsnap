#!/usr/bin/env bash
# run_all.sh — every harness suite, one after another, ONE rig at a time (6 GB box: never two Xvfb rigs
# or two cargo builds), then a kernel-log check and a summary file.
#
#   run_all.sh [--only selftest,paste,gallery,parity,tray,notice,hotkey,recfake] [--skip x,y]
#   RIG_N=95 RIG_PORT=48795 run_all.sh      # display/port every suite uses (default 95 / 48795)
#
# Suites (each stops its own rig): selftest (~25 s), paste (~80 s), gallery (~6 min), parity (X rig +
# a private headless GNOME Shell), tray (mock watcher), notice (banners really expire in a private GNOME
# Shell), hotkey (the five GNOME keybindings PRESSED in a private shell), recfake (the kms recorder against a FAKE root
# helper: no GPU, no real screen). NOT included: rec_test.sh (records the REAL screen on the VCE, capped
# at a few clips per agent, needs the live app) and the --gpu/real-session probes: run those by hand.
# MACHINE SAFETY: nothing here decodes video on the GPU. Exit 1 if any suite failed or the kernel log
# shows uvd/vce/radeon errors since the start.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
export HARNESS_OUT=${HARNESS_OUT:-$REPO/harness_out/$(date +%F)}
export RIG_N=${RIG_N:-95} RIG_PORT=${RIG_PORT:-48795}
D=$HARNESS_OUT/run_all; mkdir -p "$D"
ONLY=""; SKIP=""
while (( $# )); do
  case $1 in
    --only) ONLY=,$2,; shift ;;
    --skip) SKIP=,$2,; shift ;;
    -h|--help) sed -n '2,15p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "run_all: unknown option $1" >&2; exit 2 ;;
  esac
  shift
done
want() { [[ -z $ONLY || $ONLY == *,$1,* ]] && [[ $SKIP != *,$1,* ]]; }

# A rig already on our display/port would make every suite fail at "rig start": say so once, up front.
if [[ -e /tmp/.X11-unix/X$RIG_N ]] || ss -ltn "sport = :$RIG_PORT" 2>/dev/null | grep -q LISTEN; then
  echo "run_all: display :$RIG_N or port $RIG_PORT is busy ($HERE/gui_env.sh status; stop it, or set RIG_N/RIG_PORT)" >&2
  exit 2
fi
[[ -x $REPO/target/debug/trontsnap ]] || { echo "run_all: no target/debug/trontsnap (cargo build first; run_all never builds)" >&2; exit 2; }
echo "load: $(cut -d' ' -f1-3 /proc/loadavg)  (a loaded box slows input pacing; other agents' GPU work shows here)"
t_start=$(date +%s)
declare -A RC
run() {  # run NAME CMD...
  local name=$1; shift
  want "$name" || { echo "--- $name: skipped"; return; }
  echo "--- $name: $*"
  local t0=$(date +%s)
  "$@" > "$D/$name.log" 2>&1; RC[$name]=$?
  tail -n 3 "$D/$name.log" | sed 's/^/    /'
  echo "--- $name: rc=${RC[$name]} in $(( $(date +%s) - t0 )) s (log: $D/$name.log)"
}
run selftest "$HERE/selftest.sh" "$RIG_N" "$RIG_PORT"
run paste    "$HERE/paste_test.sh" "$RIG_N" "$RIG_PORT"
run gallery  "$HERE/gallery_test.sh" "$RIG_N" "$RIG_PORT"
run parity   "$HERE/parity_test.sh"
run tray     "$HERE/tray_test.sh"
run notice   "$HERE/notice_wl_test.sh"
run hotkey   "$HERE/hotkey_wl_test.sh"
run recfake  python3 "$HERE/rec_fake_test.py"

# kernel log since we started: the radeon UVD/VCE hang that hard-reset this box once shows up here first
mins=$(( ( $(date +%s) - t_start ) / 60 + 2 ))
klog=$(journalctl -k -b --since "-${mins} min" 2>/dev/null | grep -i -E "uvd|vce|radeon.*error|hung" | head -5)
{
  echo "run_all $(date -Is) display :$RIG_N port $RIG_PORT"
  for k in selftest paste gallery parity tray notice hotkey recfake; do
    want "$k" && echo "$k: $([[ ${RC[$k]:-1} == 0 ]] && echo PASS || echo "FAIL (rc ${RC[$k]:-?})")"
  done
  echo "kernel log (uvd|vce|radeon error|hung) since start: ${klog:-clean}"
} | tee "$D/summary.txt"
fail=0; for k in "${!RC[@]}"; do [[ ${RC[$k]} == 0 ]] || fail=1; done
[[ -n $klog ]] && fail=1
exit $fail
