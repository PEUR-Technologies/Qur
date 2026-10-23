#!/usr/bin/env bash
# Measure Qur against the weight budget (handoff section 4, decision 15).
# usage: scripts/budget.sh [runs]       (needs an unlocked screen; AC power preferred)
# Prints a table and writes spikes/out/budget-<date>.txt. Uses a scratch QUR_HOME.
set -uo pipefail
RUNS=${1:-5}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN="$ROOT/target/release/qur"
FIX="$ROOT/spikes/fixtures"
export QUR_HOME="$ROOT/spikes/out/qur-home-budget"
mkdir -p "$ROOT/spikes/out"
OUT="$ROOT/spikes/out/budget-$(date +%Y%m%d-%H%M).txt"
[ -f "$FIX/large.ts" ] || python3 "$FIX/gen.py" >/dev/null

guard() { perl -e 'alarm shift; exec @ARGV' "$@"; }
metric() { grep -E "^METRIC $1=" | tail -1 | cut -d= -f2; }
median() { sort -n | awk '{a[NR]=$1} END {print (NR%2 ? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2)}'; }
footprint_mb() {  # $@ = qur args; prints "footprint rss" in MB after settling
  "$BIN" "$@" >/dev/null 2>&1 & local p=$!
  sleep 8
  local fp; fp=$(footprint "$p" 2>/dev/null | sed -nE 's/.*Footprint: ([0-9.]+) MB.*/\1/p' | head -1)
  local rss=$(( $(ps -o rss= -p "$p") / 1024 ))
  kill "$p" 2>/dev/null; wait "$p" 2>/dev/null
  echo "$fp $rss"
}

{
echo "# Qur budget  $(date '+%Y-%m-%d %H:%M')  $(sysctl -n hw.model) macOS $(sw_vers -productVersion)  power: $(pmset -g batt | head -1 | sed -E "s/.*'(.*)'.*/\1/")"
echo "binary_mb=$(echo "scale=1; $(stat -f %z "$BIN")/1048576" | bc)  (all 13 grammars included)"

ff=(); op=()
for i in $(seq 0 "$RUNS"); do
  o=$(QUR_BENCH=first_frame guard 60 "$BIN" "$FIX/large.ts" 2>/dev/null)
  [ "$i" -eq 0 ] && continue   # warm-up
  ff+=("$(echo "$o" | metric first_frame_ms)"); op+=("$(echo "$o" | metric open_ms)")
done
echo "first_frame_ms_median=$(printf '%s\n' "${ff[@]}" | median)  runs: ${ff[*]}"
echo "open_10k_load_ms_median=$(printf '%s\n' "${op[@]}" | median)  (text on screen next frame)"
hl=()
for i in 1 2 3; do
  o=$(QUR_BENCH=x QUR_DEBUG_ACTIONS="wait:1500,qur::Quit" guard 30 "$BIN" "$FIX/large.ts" 2>/dev/null)
  a=$(echo "$o" | awk '$1=="MARK" && $2=="window_open"{print $3}'); b=$(echo "$o" | awk '$1=="MARK" && $2=="highlighted"{print $3}')
  hl+=("$(echo "$b - $a" | bc)")
done
echo "open_10k_to_highlighted_ms_median=$(printf '%s\n' "${hl[@]}" | median)"

for label in "project_only:$FIX/big-repo" "small_file:$ROOT/crates/qur_config/src/lib.rs" "large_file:$FIX/large.ts"; do
  name=${label%%:*}; arg=${label#*:}
  fps=(); rsss=()
  for i in 1 2 3; do read -r fp rss < <(footprint_mb "$arg"); fps+=("$fp"); rsss+=("$rss"); done
  echo "idle_${name}_footprint_mb=$(printf '%s\n' "${fps[@]}" | median)  rss_mb=$(printf '%s\n' "${rsss[@]}" | median)"
done

QUR_BENCH=type guard 120 "$BIN" "$FIX/large.ts" 2>/dev/null | grep '^METRIC type_' | sed 's/^METRIC //' | tr '\n' ' '; echo
QUR_BENCH=scroll guard 120 "$BIN" "$FIX/large.ts" 2>/dev/null | grep '^METRIC scroll_' | sed 's/^METRIC //' | tr '\n' ' '; echo
QUR_BENCH=theme guard 120 "$BIN" "$FIX/large.ts" 2>/dev/null | grep '^METRIC theme_' | sed 's/^METRIC //' | tr '\n' ' '; echo
} | tee "$OUT"
