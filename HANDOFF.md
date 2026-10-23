# Qur: working handoff

Read this first when picking the project up (person or Claude session). It says where things stand, what was decided, and what to do next. Keep it current: update the **Status** and **Next** sections at the end of every work session.

Last updated: 2026-10-23 (M4 done)

## What Qur is

A lightweight agentic code editor for macOS in Rust, by PEUR Technologies (owner: Yobi). Agents are first-class: the sidebar lists agents, you peek at what each one is doing, every agent works in its own git worktree, the file tree glows as a live heatmap, and you review then Merge. GPL-3.0-or-later.

## Source of truth (in order)

1. `docs/decisions.md`: owner decisions made after the original handoff. **These win** over everything below.
2. `docs/QUR_HANDOFF.md`: the original spec (product, architecture, budgets, milestones M0 to M7). Its section 0 rules still apply: milestone by milestone, measure don't estimate, no unrequested UI, ask before heavy crates or new network calls.
3. `docs/keymap.md`: every shortcut and command (VS Code base + Qur's own).
4. `docs/menus.md`: the native macOS menu bar, which holds every command.
5. `assets/brand/`: logo kit (copied from the handoff). Read `assets/brand/README.md`. Never recolor or add effects to the logo.

Summary of the owner decisions so far:
- Zed-style unified top bar (transparent titlebar, traffic lights inset).
- Compact layout: 220 px sidebar, tight margins, soft wrap on.
- Terminals in the bottom panel: `Cmd+T` new shell, `Cmd+W` close.
- Full VS Code-style keymap plus a command palette.
- Everything lives in the native menu bar, like Zed.
- Window transparency slider (0.5 to 1.0) with optional blur.
- **No Codex for now.**
- Theme (and opacity) per project, stored outside the repo, keyed by root commit (decision 13).
- `Cmd+Shift+T` = **New Agent Terminal**: a Claude agent task in its own worktree (sidebar row, heatmap), shown as a terminal.
- **No permanent code editor area:** code opens in a modal (Esc closes). The window is files + agent sessions: a left column (AGENTS, TERMINALS, FILES) and a stage (decisions 18–19).
- Orca's features are adopted where they fit (`docs/research/orca.md`, decisions 21–22). Conflicting ones (browser, hosted-git, cloud, telemetry, push) need the owner's per-item OK.
- Automated agent tests use a tiny prompt to save the owner's usage (decision 20).
- Toolkit is **GPUI**; idle memory is budgeted as physical footprint; repo at `~/qur`; no file tabs; "by PEUR Technologies" in About only.
- Peek-layout cycle is `Cmd+K Cmd+L`; `Cmd+Shift+L` stays Select All Matches.

## Status

**M0–M3 and Browser + Inspect are done** and on `main`. Commits: "M0: spikes", "M1: editor shell", "M2: agents in worktrees", "M3: Live view + heatmap", "Browser + Inspect", "Add README". The repo lives at https://github.com/PEUR-Technologies/Qur (private).

**M4 (Changes + Merge + Git panel) is done** and merged into `main` (2026-10-23). All 144 workspace tests pass (qur_git: 16 passed, 1 timing test ignored). Merge, conflicts (Finish and Abort), Discard, and the Git panel's Stage All / Unstage All were verified end to end with scripted runs; see `docs/budget.md` (M4). Still to check by hand: per-file stage/unstage, the file diff and a typed commit in the Git panel.

**After M4 (decision 29, ideas from OpenADE and PandaCode, `docs/research/openade-pandacode.md`):** turn-end checks (`[checks]`, Run Checks Now) and Run Project in Worktree (`[run]`, localhost only), both verified with scripted runs. Binary 24.4 MB; no crates added. 146 tests pass.

M4 delivered:
- **`qur_git` crate** (std + git CLI only):
  - `task_diff` and `diff_totals` (temp index copy with intent-to-add for untracked files; renames via `-M`; settled at about 60 ms on big-repo).
  - `commit_all`, `precheck`, and `merge_clean` → `Merged | UpToDate | Conflicts | Refused`. Merge uses merge-tree → commit-tree → read-tree → update-ref, so the user's index, stash and untracked files stay byte-identical (tested). If HEAD moves mid-merge it rolls back and refuses.
  - The conflict path: `start_conflict_merge`, `find_conflicts`, `resolve` (Ours/Theirs/Both, diff3-aware), `finish_merge`, `abort_merge`.
  - The Git panel backend: `status` (with ahead/behind), `stage`, `unstage`, `commit` (runs the user's hooks), `discard_unstaged`, `file_diff`.
- **App:**
  - `changes_view.rs`: the task's diff with word emphasis, line comments sent back to the agent as one prompt, a conflict pre-check, and Merge / Keep working / Discard.
  - `git_panel.rs` (`Ctrl+Shift+G`, SOURCE CONTROL row in the sidebar): status, stage and unstage, file diff, commit with `Cmd+Enter`, and Finish/Abort merge.
  - The editor conflict bar (Prev, Next, Accept ours/theirs/both). Esc steps out of find, then multi-cursor, then closes.
  - `+n −m ⚠` on agent rows, the Changes tab in the agent stage, and Merge into My Branch, Review Changes and Keep Working in the menus and palette.
  - After a merge, Qur offers to remove the task (the branch is kept as a safety net).

Browser + Inspect delivered (decision 26):
- `qur_browser`: a system WKWebView embedded in GPUI's NSView, plus an injected inspector. It reports picks (selector, HTML, styles, rect, React/`data-qur-source` source), page text, SPA navigation and load errors, and takes snapshots.
- `BrowserPane` per agent task:
  - URL bar with back, forward and reload; the URL is saved on the task.
  - Inspect (`Cmd+Shift+C`): picks become prompt chips, sent as context text plus a PNG image block when the agent supports images.
  - Created when first shown, hidden under Qur overlays, freed after 5 idle minutes.
  - Reloads after a turn that changed files.
- In the stage: a "Live | Browser" switch in layouts A/C, a Browser segment in B, and `Cmd+5`.

M3 delivered:
- **New crates:**
  - `qur_watch`: FSEvents watcher via `notify` 8.2, debounced per path at 50 ms, filtering `.git` and `.gitignore`.
  - `qur_heat`: the heatmap model (states and priorities, heat levels, fades, folder rollup, diff stats via git, focus).
- **App:**
  - `TaskMonitor`, one per task: watcher, heatmap, recompute 150 ms after writes, and repaint timers.
  - The Files tree as heatmap: approval outline, pulsing edit glow (30 fps cap), fading read glow, "new", +n −m tinted by heat level, folder dots, auto-reveal (never within 5 s of user input), and overlap dots on the main tree.
  - `LivePane`: follows the agent's file; read-only while it edits, with "<agent> is editing · Take over"; Follow toggle; accent mark on changed lines.
  - Peek layouts A/B/C in the stage (decision 25), cycled with Cmd+K Cmd+L, with Cmd+1..4 in layout B.
  - The main checkout is watched too, with "changed on disk" protection for unsaved edits.
  - Pre-trusting worktrees for Claude CLI terminals (decision 24).
- **Acceptance** (`docs/budget.md`, M3): a glow 81–84 ms after a write while Qur is unfocused, and Live followed every file.

## Key findings so far

- Both ACP adapters speak **ACP v1** (v2 is alpha). Qur targets v1.
- A hand-rolled ACP client (serde_json + std threads) is 548 KB, versus 806 KB / 63 crates for the official crate's types alone. Plan: hand-roll the transport in `qur_acp`.
- Claude's `tool_call` arrives with `kind` but **empty `locations`**; the path comes 0.5 to 0.9 s later in a `tool_call_update`. The heatmap must track `toolCallId` state. Status goes `pending` to `completed` with no `in_progress`.
- Spawn agents with `process_group(0)` and signal the group: no orphans.
- Plain `git merge` refuses when the user has staged changes. Use `merge-tree --write-tree`, then `commit-tree`, then `read-tree -m -u`, then `update-ref`. Needs git 2.38 or newer.
- M1 startup: first frame about 108 ms. The full parse runs in the background (colours about 85 ms after open). Menus are built after the first frame. Never call `text_system().all_font_names()` at startup: it costs about 40 ms and 7 MB.
- GPUI needs `gpui_platform`'s `font-kit` feature, or no text renders at all (zero fonts, no error).
- The owner's Mac (MacBook Air M5, Mac17,3) has a **60 Hz** panel. The "one frame at 120 Hz" typing target is judged on CPU-side key-to-draw time (8.3 ms or less), not on observed vsync.
- Idle RSS counts about 40 MB of shared macOS framework pages, so memory is budgeted as physical footprint (decision 15).
- Kotlin's grammar is about 5.5 MB of the about 11 MB that grammars add to the binary.
- GPUI throttles unfocused windows (Qur sets `inactive_frame_interval` to 16 ms) and stops drawing fully covered ones. Verified in M3: a glow lands within 84 ms while Qur is unfocused but visible. Unattended tests float the window.
- Screenshots of a background window can lag by a few frames; wait 6–7 s or bring it to the front.
- Running on battery capped the display at 60 Hz in these runs; on AC, frames paced at 120 Hz.
- GPUI is pinned to Zed git rev `9e84b066`, and building it needs the Xcode Metal toolchain (installed). Zed source for reference: `~/src-ref/zed` (sparse clone).

## Repo layout

```
HANDOFF.md            this file
Cargo.toml            workspace; [workspace.dependencies] is APPEND-ONLY (GPUI pinned rev)
crates/               qur_app (bin `qur`), qur_editor, qur_syntax, qur_theme, qur_config, qur_tree
assets/               brand/ (logo kit), fonts/ (Inter subset + OFL), icons/ (UI SVGs)
docs/                 QUR_HANDOFF.md, decisions.md, keymap.md, menus.md, budget.md,
                      THEME_CREDITS.md, THIRD_PARTY.md, spikes/
scripts/              budget.sh (weight budget), shot.sh (window screenshot), spike_measure*.sh, wins.swift
spikes/               M0 spikes (acp_spike, worktree_spike.sh, toolkit_gpui, toolkit_iced, spike_common)
  fixtures/gen.py     big-repo/ (5,000 files, a git repo) and large.ts (10,000 lines); outputs gitignored
  out/                run logs, screenshots, scratch QUR_HOMEs (gitignored)
```

## Machine setup

- Rust stable via rustup (1.99.1); run `source ~/.cargo/env` in shells that don't have it yet.
- Xcode is installed (needed for GPUI's Metal shaders and `swiftc`).
- `npm i -g @agentclientprotocol/claude-agent-acp@0.94.0` (pinned; never `npx` at runtime).
- Claude Code CLI logged in via claude.ai. Never set `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`.
- Codex packages were uninstalled (Codex deferred). The owner's pre-existing `~/.codex/` folder is not ours; leave it alone.

## How to run things

```sh
cargo build --release -p qur_app && target/release/qur ~/some/project   # or a file; no arg = last project
cargo test --release --workspace                 # 144 tests, incl. the theme contrast test and qur_git
scripts/budget.sh 5                              # needs an unlocked screen; prefer AC power
QUR_DEBUG_ACTIONS="qur::ThemePicker" scripts/shot.sh out.png <file>   # scripted screenshot
python3 spikes/fixtures/gen.py                   # fixtures (once)
spikes/worktree_spike.sh claude claude-agent-acp # M0 ACP + worktree + merge check
```

Env:
- `QUR_HOME`: the settings folder. **Always set it to a scratch dir in tests**, so the owner's real settings stay untouched.
- `QUR_BENCH=first_frame|scroll|type|theme|glow`: benchmarks; it also floats the window so it keeps drawing.
- `QUR_DEBUG_ACTIONS`: comma-separated action names, plus `wait:<ms>`.
- `QUR_DEBUG_PROMPT="0,1"`: answer native dialogs in order with these button indexes (logged as `PROMPT …`). With `QUR_DEBUG_ACTIONS` set, top-bar messages are logged as `FLASH …`.
- `QUR_DEBUG_TASKS="prompt1|prompt2"`: start tasks on launch. Use tiny prompts.
- `QUR_DEBUG_BROWSER_URL` (`{slug}` is replaced), `QUR_DEBUG_PICK=<css selector>`, `QUR_DEBUG_SEND`: drive the browser and inspector.
- `QUR_ACP_LOG=<dir>`: log ACP traffic. `QUR_WEB_DEVTOOLS=1`: enable the WebKit inspector.

Test fixtures: `spikes/fixtures/big-repo` must be left clean afterwards (no `qur/*` branches or worktrees).

## Next

1. **M4 follow-ups:**
   - Owner check by hand: in the Git panel, per-file stage/unstage, the file diff, and a commit with a typed message.
   - Scripted runs: set `QUR_DEBUG_PROMPT="0,1"` to answer native dialogs in order (button index). The `QUR_DEBUG_ACTIONS` driver sends actions to the workspace when the focused view isn't drawn yet (covered window).
   - Known qur_git gaps:
     - `root` must be the top of the checkout.
     - `diff --cc` entries are skipped.
     - A change that only adds or removes the final newline shows no marker.
     - Submodules and fsmonitor are untested.
     - A gpgsign passphrase prompt could block commits.
     - The HEAD-moved rollback path has no test.
2. **Next from the research shortlist** (`docs/research/openade-pandacode.md` section 4):
   - Turn checkpoints with Rewind (ACP tasks first): per-turn trees under `refs/qur/turns/<task>/<n>`, "View · Rewind" on each turn. Size M, no crates.
   - Needs the owner's OK because they cost Claude usage or depend on the pinned adapter: a second-opinion review (a second read-only session), and Plan mode with commentable plans (check what claude-agent-acp 0.94.0 advertises).
   - Chat polish batch: find in chat, grouped tool calls, Allow All Pending, Copy Chat as Markdown.
3. Browser follow-ups (known gaps):
   - Iframes and closed shadow roots can't be inspected; selectors aren't scoped inside shadow roots.
   - React 19 has no `_debugSource`; `data-qur-source` attributes work everywhere.
   - Keyboard text selection isn't captured.
   - Page JavaScript can see `window.__qur`.
4. Known gaps:
   - IME marked text isn't drawn.
   - End and Cmd+Right go to the line end, not the visual-row end.
   - The watcher honours only the root `.gitignore`.
   - Deleted files don't appear as tree rows.

## Open questions for the owner

- Trademark clearance for "Qur" hasn't been checked (brand README). It needs flagging in the public README at M7.
- Approve the accent palettes (Qur Dark: #ff7a45 #3ddc97 #5b9bff #f062c0 #f5c04a #a77bff; Qur Light: #d9480f #0e9f6e #2563eb #c026d3 #0891b2 #db2777).
- Kotlin adds about 5.5 MB to the binary. Keep it, or load it lazily later?
- Which of Orca's "conflicts" items, if any, should override a non-goal? See the table in `docs/research/orca.md`.
