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
