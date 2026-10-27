#!/usr/bin/env bash
# Terminal output throughput, run *inside* the terminal under test.
# usage: scripts/termbench.sh <result-file>
# Writes "name=value" lines to <result-file>, "done" last. QUR_BENCH=term runs it in Qur; for another
# terminal: open -na Ghostty.app --args --window-width=136 --window-height=50 -e bash scripts/termbench.sh out.txt
set -u
OUT=$1
DIR=$(cd "$(dirname "$0")/.." && pwd)/spikes/out/termbench
mkdir -p "$DIR"
# ~260 MB plain text, ~65 MB dense SGR color: built once.
[ -f "$DIR/ascii.txt" ] || seq 1 5000000 | awk '{print $1, "the quick brown fox jumps over the lazy dog"}' >"$DIR/ascii.txt"
[ -f "$DIR/color.txt" ] || seq 1 1200000 | awk '{printf "\033[38;5;%dm%07d \033[1;48;5;%dmcolor\033[0m %s\n", $1%256, $1, ($1*7)%256, "abcdefghijklmnop"}' >"$DIR/color.txt"
ms() { perl -MTime::HiRes=time -e 'printf "%.0f\n", time*1000'; }
: >"$OUT"
echo "size=$(stty size | tr ' ' x)" >>"$OUT"
for w in ascii color; do
  for i in 1 2 3; do
    a=$(ms); cat "$DIR/$w.txt"; b=$(ms)
    echo "${w}_ms_$i=$((b - a))" >>"$OUT"
  done
  echo "${w}_mb=$(( $(stat -f %z "$DIR/$w.txt") / 1048576 ))" >>"$OUT"
done
echo done >>"$OUT"
