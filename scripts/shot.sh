#!/usr/bin/env bash
# Dev: screenshot Qur's window. usage: scripts/shot.sh out.png [qur args...]
# Env passes through (QUR_DEBUG_ACTIONS, QUR_HOME, ...). Needs an unlocked screen.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=$1; shift
WINS="$ROOT/spikes/out/wins"
[ -x "$WINS" ] || xcrun swiftc -O "$ROOT/scripts/wins.swift" -o "$WINS" 2>/dev/null
"$ROOT/target/release/qur" "$@" </dev/null >/dev/null 2>&1 & P=$!
sleep "${SHOT_DELAY:-4}"
best=""; bestsz=0
for id in $("$WINS" $P | awk '/^[0-9]/{print $1}'); do
  screencapture -x -o -l"$id" "$OUT.tmp.png" 2>/dev/null || continue
  sz=$(stat -f %z "$OUT.tmp.png"); if [ "$sz" -gt "$bestsz" ]; then bestsz=$sz; mv "$OUT.tmp.png" "$OUT"; fi
done
rm -f "$OUT.tmp.png"; kill $P 2>/dev/null; wait $P 2>/dev/null; echo "$OUT"
