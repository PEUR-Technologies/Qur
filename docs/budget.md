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

## M6 (2026-10-23, search, alerts, first-run picker, Vim; MacBook Air M5)

| Metric | Target | M6 | |
|---|---|---|---|
| Project search over big-repo (5,000 files, `qur_search`, query "file": 5,000 matches) | < 300 ms | settled median 51 ms, cold 113–162 ms (`big_repo_search_timing`) | ✅ |
| Same, from the app 1 s after launch (Search view, results shown) | < 300 ms | 200–205 ms (competes with startup) | ✅ |
| Idle footprint, big-repo clone (window covered) | < 80 MB | 67–69 MB | ✅ |
| With 5,000 search results open | | 71 MB | |
| Release binary | < 40 MB | 24.8 MB (+0.4 MB for search, Vim, alerts and the chime) | ✅ |
| Alert chime | < 50 KB | 18.5 KB (0.42 s, generated for Qur) | ✅ |

No new crates: search uses `ignore` (already in qur_tree) and `regex` (already linked by GPUI); alerts use GPUI's notifications and the `objc2-app-kit` crate GPUI and qur_browser already link.

M6 acceptance:
- **Search:** under 300 ms over 5,000 files (above). Replace All is covered by `qur_search` tests (literal `$` kept, regex `$1` expanded).
- **Vim:** 8 unit tests drive keys through `qur_editor::vim` (motions with counts, i/a/I/A/o/O, x/X/D/C/J/r, d/c/y/>/< with motions and doubled, p/P, u/Ctrl-R, `.`, visual and visual line). The code modal routes typed keys to it in normal/visual mode; Qur > Vim Mode saved `[keys] vim = true`.
- **First-run picker writes config:** Enter in the picker saved `theme`, `peek_layout`, `keys.vim` to config.toml, the found agents to agents.toml, and `setup_done` to state.toml.
- **Alerts only when appropriate:** with Qur in the background, two real agents (one-line prompts) gave exactly one alert per new state (finished; needs approval, then finished) and the badge counted 1 then 2. Selecting an agent clears it. Notifications are skipped in the unbundled dev build (GPUI's guard); they need the M7 `.app`.
- Not yet checked by hand (the screen was locked): the Search view and picker on screen, typing Vim keys, the Check button, the chime being audible, and "no alert while you're on that agent".

## M7 (2026-10-23, packaging and acceptance, MacBook Air M5, battery)

`scripts/budget.sh 3` with real agents (spikes/out/budget-20261023-2333.txt):

| Metric | Target | M7 | |
|---|---|---|---|
| Release binary | < 40 MB | 24.8 MB; Qur.app 25 MB (icon 207 KB) | ✅ |
| Cold start to first frame | < 250 ms | 113 ms (109–123) | ✅ |
| Open 10k-line file, fully highlighted | < 100 ms | 84 ms | ✅ |
| Idle footprint, project + small file | < 80 MB | 73 MB (project only 70, large file 86) | ✅ |
| Keystroke (CPU side) p50 / p99 | ≤ 8.3 ms | 1.4 / 2.0 ms | ✅ |
| Agent write → glow (Qur not focused) | < 100 ms | p50 81 ms, max 84.5 ms, 0 missed | ✅ |
| Footprint with 3 Claude agents (Qur's process) | < 150 MB | 129 MB (2 of the 3 adapters were still alive at the sample) | ✅ |
| Agent processes left after quit | 0 | 0 | ✅ |

On battery the display runs at 60 Hz, so scroll and theme frames pace at ~16.7 ms (scroll max 309 ms was a single first-frame outlier).

**No outbound connections from Qur itself:** `lsof -nP -a -i -p <qur pid>` listed no sockets at 5, 12 and 19 s, with an ACP agent working, an Agent Terminal open and project search running. Agents and the Browser's WebKit are separate processes.

**`qur doctor`** on the owner's Mac: macOS 27.0, git 2.54, settings folder writable, config.toml and agents.toml fine, Claude Code over ACP (`@agentclientprotocol/claude-agent-acp 0.94.0`, ACP v1, adapter pinned 0.94.0); exit 0 in 1.4 s.

## v0.1.6 (2026-10-26, MacBook Air M5)

| Metric | Target | v0.1.6 | |
|---|---|---|---|
| Release binary / `Qur.app` | < 40 MB | 30.9 MB / 29 MB (zip 9.0 MB) | ✅ |
| Cold start to first frame (3 runs) | < 250 ms | 67–71 ms | ✅ |
| Idle footprint, small project (3 runs) | < 80 MB | 70 MB | ✅ |
| File viewer on a 1600×3065 PNG: open / after closing | | 98 MB / 92–93 MB (2 of 3 runs; one 127); v0.1.6 before the fix: 142 / ~140 | |
| Claude agent processes, 3 agents done → released after `idle_release` | | 669 MB → 0 MB (outside Qur's process) | |

`budget.sh 3` wasn't rerun: it starts three tasks with the same prompt, so two fail on the same worktree name and the "3 agents" figure measures one agent. The fix is on the unreleased memory-checks branch.

## Memory checks and readout

`scripts/memcheck.sh` opens and closes the code modal, the file viewer (images, a binary file) and terminals with filled scrollback, and fails when physical footprint doesn't come back (median of 3 runs; thresholds in the script). It needs a window, so it isn't in CI: run it by hand on an unlocked screen. About and `qur doctor` show Qur's own footprint plus its agents' and terminals' (sampled on demand, never polled). About 33 MB of "Owned physical footprint (unmapped) (graphics)" stays dirty after closing any modal (Metal command buffers); not investigated.

## v0.1.11 (2026-10-28, MacBook Air M5, AC power)

`scripts/budget.sh 3` on v0.1.10, then the one fix. Built v0.1.6 alongside: first frame with large.ts and its idle footprint were the same (124–156 ms vs 125–149 ms; 75–83 MB vs 66–76 MB), so nothing regressed since.

| Metric | Target | v0.1.10 | v0.1.11 | |
|---|---|---|---|---|
| Release binary | < 40 MB | 30.6 MB | | ✅ |
| Cold start to first frame, large.ts | < 250 ms | 123 ms (121–124) | | ✅ |
| Cold start to first frame, no folder (empty window with a shell) | < 250 ms | 347–382 ms | 106–121 ms | ✅ |
| Open 10k-line file, fully highlighted | < 100 ms | 66 ms | | ✅ |
| Idle footprint, project only / small file / large file | < 80 MB | 67 / 72 / 97 MB (large file 66–76 on rerun) | | ✅ |
| Keystroke p50 / p99 | ≤ 8.3 ms | 0.52 / 1.63 ms | | ✅ |
| Agent write → glow | < 100 ms | p50 80.8, max 84.9, 0 missed | | ✅ |
| Footprint with 3 Claude agents | < 150 MB | 85 MB | | ✅ |
| Agent processes left after quit | 0 | 0 | | ✅ |

The empty window's shell waited on the UI thread for `qur_agents::login_path()` (an interactive login shell, ~250 ms). A plain terminal is `$SHELL -l` and builds that PATH itself, so only agent commands and a custom `[terminal] shell` wait for it now.

