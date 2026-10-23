#!/usr/bin/env bash
# Measure one M0 toolkit spike binary the same way for every candidate.
#
# usage: scripts/spike_measure.sh <name> <binary> [runs]
# Writes spikes/out/measure-<name>.txt and spikes/out/<name>-retina.png
set -uo pipefail

NAME=${1:?name}
BIN=${2:?binary}
RUNS=${3:-5}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/spikes/out"
mkdir -p "$OUT"
REPORT="$OUT/measure-$NAME.txt"
WINID="$OUT/winid"
[ -x "$WINID" ] || xcrun swiftc -O "$ROOT/scripts/winid.swift" -o "$WINID" 2>/dev/null

guard() { perl -e 'alarm shift; exec @ARGV' "$@"; }   # timeout without coreutils
metric() { grep -E "^METRIC $1=" | tail -1 | cut -d= -f2; }
median() { sort -n | awk '{a[NR]=$1} END {print (NR%2 ? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2)}'; }

{
echo "# $NAME  ($(date '+%Y-%m-%d %H:%M'), $(sysctl -n machdep.cpu.brand_string), macOS $(sw_vers -productVersion))"
echo "binary_bytes=$(stat -f %z "$BIN")"
echo "binary_mb=$(echo "scale=2; $(stat -f %z "$BIN")/1048576" | bc)"

# Cold start: internal first-frame and external spawn->exit wall time.
# Run 1 is discarded as a warm-up (dyld/shader caches); report the median of the rest.
ff=(); wall=(); open=()
for i in $(seq 0 "$RUNS"); do
  t0=$(perl -MTime::HiRes=time -e 'print time')
  o=$(QUR_SPIKE_EXIT_AFTER_FIRST_FRAME=1 guard 60 "$BIN" 2>/dev/null)
  t1=$(perl -MTime::HiRes=time -e 'print time')
  [ "$i" -eq 0 ] && continue
  ff+=("$(echo "$o" | metric first_frame_ms)")
  open+=("$(echo "$o" | metric open_10k_ms)")
  wall+=("$(perl -e "printf '%.0f', ($t1-$t0)*1000")")
done
echo "first_frame_ms_runs=${ff[*]}"
echo "first_frame_ms_median=$(printf '%s\n' "${ff[@]}" | median)"
echo "spawn_to_exit_ms_median=$(printf '%s\n' "${wall[@]}" | median)"
echo "open_10k_ms_median=$(printf '%s\n' "${open[@]}" | median)"

o=$(QUR_SPIKE_EXPAND_ALL=1 QUR_SPIKE_EXIT_AFTER_FIRST_FRAME=1 guard 60 "$BIN" 2>/dev/null)
echo "expand_all_first_frame_ms=$(echo "$o" | metric first_frame_ms)"
echo "expand_all_visible_rows=$(echo "$o" | metric tree_visible_rows)"

# Idle RSS after 5 s, plus a screenshot of the window for text-quality review.
"$BIN" >/dev/null 2>&1 &
PID=$!
sleep 5
echo "idle_rss_mb=$(echo "scale=1; $(ps -o rss= -p $PID)/1024" | bc)"
if W=$("$WINID" "$PID"); then
  screencapture -x -o -l"$W" "$OUT/$NAME-retina.png" && echo "screenshot=$OUT/$NAME-retina.png"
else
  echo "screenshot=unavailable (no window id; Screen Recording permission?)"
fi
kill $PID 2>/dev/null; wait $PID 2>/dev/null

# Scroll and typing.
QUR_SPIKE_AUTOSCROLL=1 guard 120 "$BIN" 2>/dev/null | grep '^METRIC scroll_' | sed 's/^METRIC //'
QUR_SPIKE_AUTOTYPE=1 guard 120 "$BIN" 2>/dev/null | grep '^METRIC type_' | sed 's/^METRIC //'
} | tee "$REPORT"
