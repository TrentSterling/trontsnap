#!/usr/bin/env bash
# seed_gallery.sh — RIG_SEED_HOOK for the gallery tests: ShareX archive + webm + odd names + a truncated PNG on top of the default seed
h=$1; TS=$h/Pictures/TrontSnap; SX=$h/Documents/ShareX/Screenshots
mkdir -p "$SX/2026-07" "$SX/2026-06"
mapfile -t pngs < <(ls "$TS"/*.png | head -6)
i=0
for p in "${pngs[@]}"; do
  i=$((i+1)); d="$SX/2026-07/ShareX shot 2026-07-0$i.png"; cp "$p" "$d"; touch -d "2026-07-0$i 12:00" "$d"
done
i=0
for p in "${pngs[@]:0:3}"; do
  i=$((i+1)); d="$SX/2026-06/Screenshot 2026-06-1$i.jpg"
  python3 -c "from PIL import Image; import sys; Image.open(sys.argv[1]).convert('RGB').save(sys.argv[2], quality=80)" "$p" "$d"
  touch -d "2026-06-1$i 09:00" "$d"
done
now=$(date +%s)
w="$TS/TrontSnap_$(date -d @$((now-300)) +%F_%H-%M-%S).webm"
ffmpeg -hide_banner -loglevel error -f lavfi -i testsrc=duration=2:size=320x240:rate=15 -c:v libvpx -b:v 300k "$w" && touch -d @$((now-300)) "$w"
o="$TS/my shot #1 100%.png"; cp "${pngs[1]}" "$o"; touch -d @$((now-400)) "$o"
t="$TS/TrontSnap_$(date -d @$((now-500)) +%F_%H-%M-%S).png"; head -c 3000 "${pngs[2]}" > "$t"; touch -d @$((now-500)) "$t"
