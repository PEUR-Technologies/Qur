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

## Browser + Inspect (2026-10-23, decision 26, MacBook Air M5, AC power)

| Metric | Result |
|---|---|
| Binary | 24.9 MB incl. grammars (`qur_browser` + objc2-web-kit ≈ +0.14 MB; links the system WebKit.framework) |
| Qur footprint, one agent + Browser open on a small page | 77 MB (idle without the browser: ~70 MB) |
| WebKit processes while the page is open | GPU 14 MB, WebContent 17 MB, Networking (small). They are WebKit's own processes and not counted in Qur's budget. |
| After closing the page (idle release 5 min) or quitting | the page's WebContent exits on release; every helper is gone after quit |

End-to-end check, with a tiny prompt:
1. The task's Browser opened `index.html`.
2. Inspect picked `#ship`; the chip "button#ship 'Ship it'" went to Claude with the element's context and a screenshot, plus "Change this button's text to Deploy."
3. Claude edited `index.html` in its worktree (permission allowed once).
4. The Browser reloaded at the end of the turn and showed "Deploy". No agent process was left after quit.

## M4 (2026-10-23, Changes + Merge + Git panel, MacBook Air M5)

| Metric | Result |
|---|---|
| `task_diff` on big-repo (51 changed files) | cold 241 ms; settled median 62 ms (min 58, max 155); `diff_totals` 62 ms |
| Footprint, one Agent Terminal on a big-repo clone, window drawing (floated) | 78 MB |
| Same, with Changes open on 50 changed files | 94–101 MB (Changes adds about 16–23 MB). No M4 target; within the 3-agent budget of 150 MB |

Footprint with the window covered (not drawing) was 75 MB in both cases, which is why these runs float the window.

M4 acceptance, scripted on scratch repos (`QUR_DEBUG_ACTIONS` + `QUR_DEBUG_PROMPT`):
- **Merge:** an Agent Terminal with a file written into its worktree → Merge → the file is on `main` in a merge commit, and the checkout is clean. A real ACP agent with a one-line prompt (Allow Once, `end_turn`, then Merge) did the same.
- **Conflict:** the same line changed on `main` and in the task → Merge → "Resolve in Qur" → Accept Both → Finish Merge → a merge commit keeping both lines, with no `MERGE_HEAD` (5 of 5 runs). Abort Merge → `HEAD` unchanged, the file as it was, no `MERGE_HEAD` (2 of 2).
- **Discard:** removes the worktree, the `qur/…` branch and the task record.
- **Git panel:** Stage All and Unstage All from the palette update the index; Commit with an empty message focuses the message field. Per-file stage/unstage, the file diff and a typed commit still need a check by hand (synthetic typing would go to whatever app is in front).
- **Harness finding:** macOS stops drawing a covered window, so a view focused since the last frame (the code modal) isn't in GPUI's dispatch tree, and a scripted action dispatched to it went nowhere (2 of 4 conflict runs before the fix). The `QUR_DEBUG_ACTIONS` driver now falls back to the workspace's own node in that case. A window you're looking at keeps drawing, so users aren't affected.

## M5 (2026-10-23, terminal agents, MacBook Air M5)

| Metric | Target | M5 | |
|---|---|---|---|
| Terminal agent's write → glow on screen (`QUR_BENCH=glow`, 20 writes, Qur not focused) | < 100 ms | p50 80 ms, max 84 ms, 0 missed (2 runs) | ✅ |
| Footprint, 3 Agent Terminals on a big-repo clone (Qur's process; the CLIs are separate) | < 150 MB | 101–133 MB (RSS 115–118 MB), 3 runs | ✅ |
| Release binary | < 40 MB | 24.4 MB | ✅ |

M5 acceptance, scripted on scratch repos with a fake CLI agent (a bash script; no model, no network):
- **A non-ACP CLI agent runs in its worktree and its edits light up the tree:** the `agents.toml` agent `mode = "terminal"` received the New Agent Task text (typed in once it went quiet, and with `prompt = "--message"`), wrote its file in its own worktree, and the glow bench above measures the heatmap path for terminal tasks. Quit leaves no agent process.
- **Setup:** `[worktree] setup` runs first in the agent's terminal; the agent starts only if it succeeds.
- **Turns:** with `[checks] terminal = true`, a check ran exactly once after the agent's edit (startup and setup don't count; 2 of 2 runs).
- **Names:** a title set by the agent (OSC 0) became the task name and was saved; generic titles ("Claude Code", shells, paths) are ignored. The real Claude CLI with a one-line prompt named its task too.
- Find in Terminal and Cmd-click links are covered by unit tests against a real `Term`; typing into the find field and clicking need a check by hand.
