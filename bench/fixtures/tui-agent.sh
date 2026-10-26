#!/bin/sh
set -eu

usage="usage: bench/fixtures/tui-agent.sh <seconds>"
duration=${1:-}
case "$duration" in
  '' | *[!0-9]*)
    echo "$usage" >&2
    exit 2
    ;;
esac

rows=$(stty size 2>/dev/null | cut -d ' ' -f 1 || true)
case "$rows" in
  '' | *[!0-9]*) rows=24 ;;
esac
if [ "$rows" -lt 4 ]; then
  rows=4
fi
status=$((rows - 2))
tokens=$((rows - 1))

printf '\033[2J\033[H'
line=1
while [ "$line" -le 40 ]; do
  printf 'tui fixture line %02d: the transcript an agent keeps above its animated status\r\n' "$line"
  line=$((line + 1))
done

frames=$((duration * 20))
frame=0
while [ "$frame" -lt "$frames" ]; do
  case $((frame % 4)) in
    0) glyph='|' ;;
    1) glyph='/' ;;
    2) glyph='-' ;;
    *) glyph='\' ;;
  esac
  printf '\033[?2026h\033[%d;1H\033[2K%s Thinking for %ds (esc to interrupt)\033[%d;1H\033[2K  %d tokens\033[%d;1H\033[2K> \033[?2026l' \
    "$status" "$glyph" $((frame / 20)) "$tokens" $((frame * 7)) "$rows"
  if [ $((frame % 2)) -eq 0 ]; then
    case $((frame / 2 % 4)) in
      0) title_glyph='\342\240\213' ;;
      1) title_glyph='\342\240\231' ;;
      2) title_glyph='\342\240\271' ;;
      *) title_glyph='\342\240\270' ;;
    esac
    printf '\033]2;%b tui fixture\007' "$title_glyph"
  fi
  sleep 0.05
  frame=$((frame + 1))
done
printf '\r\ntui fixture done\r\n'
