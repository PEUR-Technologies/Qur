#!/usr/bin/env bash
# Memory regression checks: open and close things in Qur and check that its
# physical footprint comes back to idle (docs/budget.md, "Memory checks").
#
# usage: scripts/memcheck.sh [-n runs] [check ...]   (default: every check, 3 runs; -l lists)
# env:   QUR_BIN (default target/release/qur, or $CARGO_TARGET_DIR/release/qur)
#        MC_DIR  scratch folder (default spikes/out/memcheck)
#        MC_STRICT=1  known failures (xfail) fail the run too
# Needs an unlocked screen: the window has to draw (QUR_BENCH floats it). No
# agent, no network. Leave the mouse off the window while it runs.
#
# Each run: Qur starts on a scratch repo, settles, `mem` (the QUR_DEBUG_ACTIONS
# step) marks a sample, the check does its thing, Qur settles again, another
# sample. A sample is `footprint -v` of the process. Two things in it aren't
# Qur's to keep or free, so the check's number leaves them out:
#  - the CAMetalLayer's drawables (2 or 3 window-sized IOSurfaces, ~16 MB each
#    at 1280x820 pt on a 2x screen): GPUI asks for up to 3 and Core Animation
#    keeps 2 or 3 at random, which alone moves footprint by 16 MB between runs;
#  - Metal's command-buffer memory (~16 x 2 MB, "Owned physical footprint
#    (unmapped) (graphics)"): dirty while frames are submitted, marked
#    reclaimable (and out of phys_footprint) 3-5 s after the last one. The
#    settle time is long enough for that, so it shows up only if Qur is still
#    drawing, which is worth failing on anyway.
# The check passes when the median over the runs of (after - before) is under
# its threshold. Plain phys_footprint deltas are printed alongside.
set -uo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${QUR_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/release/qur}
MC=${MC_DIR:-$ROOT/spikes/out/memcheck}
RUNS=3
SETTLE=12000   # ms of quiet before a sample

# name | threshold MB | expect (pass or xfail) | what
CHECKS="
code|12|pass|open and close the code modal on 10 files (closed buffers stay cached today)
viewer|30|pass|open and close the file viewer on 3 images and a binary file (images are decoded at drawn size and freed on close)
viewer_file|6|pass|open and close the file viewer on a binary file, 5 times
terminals|8|pass|3 terminals with filled scrollback, then closed
"
# terminals also has a ceiling while the 3 are open (TERM_PEAK).
TERM_PEAK=110

while getopts "n:l" o; do
  case $o in
    n) RUNS=$OPTARG ;;
    l) echo "$CHECKS" | grep . | awk -F'|' '{printf "%-12s %4s MB  %-5s %s\n", $1, $2, $3, $4}'; exit 0 ;;
    *) exit 2 ;;
  esac
done
shift $((OPTIND - 1))
[ -x "$BIN" ] || { echo "no binary at $BIN (cargo build --release -p qur_app)"; exit 2; }
WANT=("$@")

# --- fixture -----------------------------------------------------------------
REPO="$MC/repo"
fixture() {
  rm -rf "${MC:?}/repo"; mkdir -p "$REPO"
  local f
  for f in script doctor mem files look dock terminal alerts review live; do cp "$ROOT/crates/qur_app/src/$f.rs" "$REPO/"; done
  sips -z 3065 1600 "$ROOT/docs/images/heatmap-live.png" --out "$REPO/big.png" >/dev/null
  sips -z 1200 1600 "$ROOT/assets/brand/qur-brand-board.png" --out "$REPO/board.png" >/dev/null
  sips -z 800 800 "$ROOT/assets/brand/3d/qur-symbol-3d-white.png" --out "$REPO/sym.png" >/dev/null
  head -c 200000 /dev/urandom > "$REPO/blob.bin"
  git -C "$REPO" init -q
  git -C "$REPO" -c user.name=mc -c user.email=mc@localhost -c commit.gpgsign=false add -A
  git -C "$REPO" -c user.name=mc -c user.email=mc@localhost -c commit.gpgsign=false commit -qm init
}

# --- steps -------------------------------------------------------------------
sample() { echo "wait:$SETTLE,mem,wait:3000"; }   # footprint -v takes ~1 s; nothing runs meanwhile
open_close() { echo "key:cmd-p,type:$1\\n,wait:${2:-700},key:escape,wait:400"; }
steps_code() {
  local s; s="$(sample)"
  for f in script doctor mem files look dock terminal alerts review live; do s="$s,$(open_close "$f.rs")"; done
  echo "$s,$(sample),qur::Quit"
}
steps_viewer() {
  local s; s="$(sample)"
  for f in big.png board.png sym.png blob.bin; do s="$s,$(open_close "$f" 1500)"; done
  echo "$s,$(sample),qur::Quit"
}
steps_viewer_file() {
  local s; s="$(sample)"
  for _ in 1 2 3 4 5; do s="$s,$(open_close blob.bin)"; done
  echo "$s,$(sample),qur::Quit"
}
steps_terminals() {
  local s; s="$(sample)"
  for _ in 1 2 3; do s="$s,qur::NewTerminal,wait:1500,type:seq -f 'line %g of the scrollback check' 1 20000\\n,wait:3000"; done
  s="$s,$(sample)"
  for _ in 1 2 3; do s="$s,key:cmd-w,wait:1000"; done
  echo "$s,state,$(sample),qur::Quit"
}

# --- one run -----------------------------------------------------------------
# footprint -v → "drawables_mb gfx_dirty_mb" (DRT, the dirty pages, is field 6).
# phys_footprint itself comes from the MEM line (to 0.1 MB; footprint rounds to 1 MB).
measure() {
  footprint -v -p "$1" 2>/dev/null | awk '
    /bytes per page/ { if (match($0, /[0-9]+ bytes per page/)) page = substr($0, RSTART, RLENGTH) + 0 }
    /CAMetalLayer Display Drawable/ { draw += $6 }
    /^ *unmapped - +unmapped .*Owned physical footprint \(unmapped\) \(graphics\)/ { gfx += $6 }
    END { if (!page) page = 16384; printf "%.1f %.1f\n", draw * page / 1048576, gfx * page / 1048576 }'
}

# run <check> <n>: prints "owned_delta phys_delta [peak_owned_delta]" or "error ..."
run() {
  local name=$1 n=$2 dir="$MC/$1/run$2"
  rm -rf "$dir"; mkdir -p "$dir/home"
  printf '[[agent]]\nid = "claude-terminal"\nlabel = "Test shell agent"\nmode = "terminal"\ncommand = "/bin/sh"\n' > "$dir/home/agents.toml"
  local steps; steps=$("steps_$name")
  QUR_HOME="$dir/home" QUR_BENCH=memcheck QUR_DEBUG_ACTIONS="$steps" perl -e 'alarm shift; exec @ARGV' 300 "$BIN" "$REPO" </dev/null >"$dir/log" 2>&1 &
  local pid=$! seen=0 c
  : > "$dir/samples"
  while kill -0 "$pid" 2>/dev/null; do
    c=$(grep -c '^MEM' "$dir/log")
    if [ "$c" -gt "$seen" ]; then
      seen=$c
      measure "$pid" >> "$dir/samples"
    fi
    sleep 0.2
  done
  wait "$pid" 2>/dev/null
  grep '^MEM' "$dir/log" | sed -E 's/.*footprint_mb=([0-9.]+).*/\1/' | paste -d' ' - "$dir/samples" | awk '
    { phys[NR] = $1; own[NR] = $1 - $2; gfx[NR] = $3 }
    END {
      if (NR < 2) { print "error: " NR " samples"; exit }
      d = own[NR] - own[1]; p = phys[NR] - phys[1]
      out = sprintf("%.1f %.1f", d, p)
      if (NR >= 3) out = out sprintf(" %.1f", own[2] - own[1])
      if (gfx[NR] > 8) out = out " (still drawing: " gfx[NR] " MB of command buffers)"
      print out
    }'
}

median() { sort -n | awk '{a[NR]=$1} END {if (NR) print (NR%2 ? a[(NR+1)/2] : (a[NR/2]+a[NR/2+1])/2)}'; }

# --- main --------------------------------------------------------------------
mkdir -p "$MC"
fixture
FAILS=0
echo "# Qur memory checks  $(date '+%Y-%m-%d %H:%M')  $(sysctl -n hw.model)  runs: $RUNS  (MB; owned = footprint minus the window's drawables)"
while IFS='|' read -r name limit expect what; do
  [ -n "$name" ] || continue
  if [ ${#WANT[@]} -gt 0 ] && [[ ! " ${WANT[*]} " == *" $name "* ]]; then continue; fi
  echo "$name: $what"
  owned=(); phys=(); peaks=()
  for i in $(seq 1 "$RUNS"); do
    r=$(run "$name" "$i")
    echo "    run $i: owned +${r%% *}  (phys_footprint $(echo "$r" | awk '{print "+" $2}'))$(echo "$r" | awk 'NF >= 3 && $3 ~ /^-?[0-9.]+$/ {print "  peak owned +" $3}')$(echo "$r" | grep -o '(still.*')"
    case $r in error*) continue ;; esac
    owned+=("$(echo "$r" | awk '{print $1}')"); phys+=("$(echo "$r" | awk '{print $2}')")
    peaks+=("$(echo "$r" | awk '{print $3}')")
  done
  if [ ${#owned[@]} -eq 0 ]; then echo "  ✕ no runs finished"; FAILS=$((FAILS + 1)); continue; fi
  m=$(printf '%s\n' "${owned[@]}" | median); mp=$(printf '%s\n' "${phys[@]}" | median)
  ok=$(echo "$m < $limit" | bc)
  if [ "$name" = terminals ]; then
    pk=$(printf '%s\n' "${peaks[@]}" | median)
    echo "    with 3 terminals open: median owned +$pk MB (ceiling $TERM_PEAK)"
    [ "$(echo "$pk < $TERM_PEAK" | bc)" = 1 ] || ok=0
  fi
  line="median owned +$m MB (phys_footprint +$mp), threshold $limit MB"
  if [ "$ok" = 1 ] && [ "$expect" = pass ]; then echo "  ✓ $line"
  elif [ "$ok" = 1 ]; then echo "  ✓ $line: XPASS, a known failure passes now (make it 'pass' in CHECKS)"
  elif [ "$expect" = xfail ]; then echo "  ! $line: XFAIL (known failure)"; [ "${MC_STRICT:-}" = 1 ] && FAILS=$((FAILS + 1))
  else echo "  ✕ $line"; FAILS=$((FAILS + 1)); fi
done <<< "$CHECKS"
[ "$FAILS" -eq 0 ] && echo "OK" || { echo "$FAILS check(s) failed"; exit 1; }
