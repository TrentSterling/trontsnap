#!/usr/bin/env bash
# RIG_SEED_HOOK for paste_test.sh: a recording whose name needs URI escaping
# (space # ( ) % and a non-ASCII byte), newest in the gallery so it is tile 1.
h=$1
src=$(ls -S "$h"/Pictures/TrontSnap/*.mp4 | head -1)
dst="$h/Pictures/TrontSnap/Screen Rec #1 (final) 100%é.mp4"
cp "$src" "$dst"; touch -d "@$(( $(date +%s) - 60 ))" "$dst"
