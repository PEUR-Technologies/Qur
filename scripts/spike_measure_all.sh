#!/usr/bin/env bash
# Wait for an unlocked screen, then measure both toolkit spikes back to back,
# alternating order, plus an opacity 0.85 pass for frame-time cost.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
locked() { ioreg -n Root -d1 -a | grep -A1 CGSSessionScreenIsLocked | grep -q '<true/>'; }
while locked; do sleep 10; done
echo "unlocked at $(date +%T); power: $(pmset -g batt | head -1)"
sleep 5
G="$ROOT/spikes/toolkit_gpui/target/release/toolkit_gpui"
I="$ROOT/spikes/toolkit_iced/target/release/toolkit_iced"
"$ROOT/scripts/spike_measure.sh" gpui "$G"
"$ROOT/scripts/spike_measure.sh" iced "$I"
for n in gpui iced; do
  b=$G; [ $n = iced ] && b=$I
  echo "# $n opacity 0.85"
  for m in AUTOSCROLL AUTOTYPE; do
    env QUR_SPIKE_OPACITY=0.85 QUR_SPIKE_BLUR=1 QUR_SPIKE_$m=1 perl -e 'alarm 120; exec @ARGV' "$b" 2>/dev/null | grep -E '^METRIC (scroll|type)' | sed "s/^METRIC /$n.op85./"
  done
done | tee "$ROOT/spikes/out/measure-opacity.txt"
