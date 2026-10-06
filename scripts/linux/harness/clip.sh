#!/usr/bin/env bash
# clip.sh N               list the clipboard's TARGETS on display :N
# clip.sh N TYPE          write the clipboard content for TYPE to stdout (binary!)
# clip.sh N TYPE FILE     ...or to FILE
# e.g. clip.sh 99 image/png shot.png ; clip.sh 99 text/uri-list
# Uses xclip -selection clipboard. A selection only exists while its owner is alive.
set -u
n=${1:?usage: clip.sh N [type [file]]}
if [[ -z ${2:-} ]]; then
  xclip -display ":$n" -selection clipboard -t TARGETS -o
elif [[ -n ${3:-} ]]; then
  xclip -display ":$n" -selection clipboard -t "$2" -o > "$3" && echo "$3 ($(stat -c %s "$3") bytes)"
else
  xclip -display ":$n" -selection clipboard -t "$2" -o
fi
