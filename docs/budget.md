# Weight budget log

Targets: handoff section 4. One row per milestone measurement; never relax a target without the owner.

## M0 (2026-10-23, toolkit spikes, MacBook Air M5, on battery)

| Metric | Target | GPUI spike | iced spike |
|---|---|---|---|
| Release binary | < 40 MB | 8.5 MB ✅ | 10.2 MB ✅ |
| Idle memory: physical footprint | < 80 MB (proposed metric) | 71 MB ✅ | 120 MB ❌ |
| Idle memory: RSS (current metric) | < 80 MB | 109–121 MB ❌ | 118–126 MB ❌ |
| Cold start to first frame | < 250 ms | 148 ms ✅ | 211 ms ✅ |
| Keystroke to pixel (CPU side) | ≤ 8.3 ms | 0.36 p50 / 0.98 max ✅ | 0.16 p50 draw ✅ |
| Open 10k-line file | < 100 ms | 49 ms ✅ | 49 ms ✅ |
| RSS with 3 agents | < 150 MB | M2 | |
| Agent event to glow | < 100 ms | M3 | |

Approved (decision 15): idle memory is measured as physical footprint, RSS reported alongside.

## M1 (2026-10-23, editor shell, MacBook Air M5, on battery, `scripts/budget.sh 5`)

| Metric | Target | M1 | |
|---|---|---|---|
| Release binary | < 40 MB (excl. grammars) | 22.3 MB incl. all 13 grammars (~11 MB without) | ✅ |
| Idle footprint: project open, no file | < 80 MB | 66 MB (RSS 97) | ✅ |
| Idle footprint: project + small file | < 80 MB | 70 MB (RSS 104) | ✅ |
| Idle footprint: project + 10k-line file | (info) | 83 MB (RSS 117) | |
| Cold start to first frame (median of 5) | < 250 ms | 108 ms | ✅ |
| Open 10k-line file: text on screen | < 100 ms | load 1 ms, drawn next frame | ✅ |
| Open 10k-line file: fully highlighted | < 100 ms | 84–86 ms | ✅ |
| Keystroke to frame (CPU side) p50 / p99 / max | ≤ 8.3 ms | 0.32 / 0.51 / 1.81 ms | ✅ |
| Scroll frame interval p50 / p99 (60 Hz on battery) | | 16.65 / 19.7 ms | |
| Theme switch, apply to next frame p50 / max (34 switches) | instant | 16.7 / 53 ms | ✅ |

Notes:
- Grammar weight: all 13 grammars ≈ 11.3 MB; Kotlin alone ≈ 5.5 MB (the biggest). See `docs/THIRD_PARTY.md`.
- Startup work that was moved off the critical path in M1: full parse (now background, ~70 ms after text shows), building the menu bar (after first frame), code-font detection (direct lookup instead of listing every installed font, which cost ~40 ms and ~7 MB).

## M2 (2026-10-23, agents in worktrees, MacBook Air M5, AC power)

| Metric | Target | M2 | |
|---|---|---|---|
| Footprint with 3 Claude agents active (Qur process only) | < 150 MB | 81 MB | ✅ |
| Release binary | < 40 MB (excl. grammars) | 23.1 MB incl. grammars | ✅ |
| Idle footprint, project + small file | < 80 MB | 70 MB | ✅ |
| Cold start to first frame | < 250 ms | 113 ms | ✅ |
| Open 10k-line file, fully highlighted | < 100 ms | 88 ms | ✅ |
| Keystroke (CPU side) p50 / max | ≤ 8.3 ms | 0.45 / 1.86 ms | ✅ |

M2 acceptance:
- **Three Claude tasks in parallel:** each in its own worktree, with the tiny prompt "Reply with just the word ok." All three replied "ok" (screenshot `spikes/out/m2-three.png`).
- **Quit leaves no orphans:** after Quit, `ps` shows no `claude-agent-acp` process.
- **Permission flow:** "Create hi.txt" → permission bar → Allow once → file written.
- **Resume:** relaunch → select the task → `session/load` replays the history without a new prompt.

## M3 (2026-10-23, Live + heatmap, MacBook Air M5, AC power)

| Metric | Target | M3 | |
|---|---|---|---|
| Agent edit → glow on screen (`QUR_BENCH=glow`, 20 writes; Qur visible, **not focused**) | < 100 ms | p50 81 ms, max 84 ms, 0 missed | ✅ |
| Live follows the agent across files | yes | followed all 20 probe files | ✅ |
| Release binary | < 40 MB | 23.6 MB incl. grammars (watcher +0.1 MB) | ✅ |
| Idle footprint, project + small file | < 80 MB | 71 MB | ✅ |
| Cold start to first frame | < 250 ms | 108–159 ms | ✅ |
| Open 10k-line file, fully highlighted | < 100 ms | 86 ms (after moving the root watcher start past the first frame) | ✅ |
| Keystroke (CPU side) p50 / p99 / max | ≤ 8.3 ms | 1.6 / 3.5 / 8.5 ms | ✅ p99 |

Notes:
- **Typing is slower than in M2's table (0.45 ms p50), but it's the machine, not M3.** Rebuilding the M2 commit and running both builds back to back gave the same numbers: M2 1.6–1.7 ms p50 and 8–11 ms max; M3 1.6 ms p50 and 8.3–8.5 ms max. An occasional worst-case keystroke goes slightly over 8.3 ms in both builds.
- **macOS stops drawing a window that is fully covered.** GPUI stops the display link when the window is occluded. The model keeps updating, and the window shows the current state as soon as any part of it is visible. Unattended test runs therefore float the window (`QUR_BENCH`).
- Bugs found and fixed:
  - A tree rebuild on every heat update when the agent's focus file didn't exist yet.
  - A 5 ms repaint timer while a recompute was overdue. Heat timers now repaint at most every 33 ms, and an edit tool's completion schedules its recompute.
