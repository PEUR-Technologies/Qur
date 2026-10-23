# Qur: working handoff

Read this first when picking the project up (person or Claude session). It says where things stand, what was decided, and what to do next. Keep it current: update the **Status** and **Next** sections at the end of every work session.

Last updated: 2026-10-23

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

**M0, M1 and M2 are done.** Commits: "M0: spikes", "M1: editor shell", "M2: agents in worktrees". Next is M3 (peek + heatmap).

M2 delivered:
- **New crates:**
  - `qur_acp`: hand-rolled ACP v1 client, with a fake-agent protocol test suite and one ignored real-Claude test.
  - `qur_worktree`: create and remove worktrees, slugs, `.worktreeinclude` copies with APFS clones, and the setup command.
  - `qur_agents`: `agents.toml` roster, login-shell `PATH`, and a per-project task store.
- **App:**
  - Left column: AGENTS (ACP tasks and Agent Terminals) with accent, status glyph, last-message preview and unread marker; TERMINALS; and FILES, which follows the selected worktree, is resizable and drags into the prompt.
  - Stage: agent view (header, To-do strip, Chat with tool calls and diff counts, permission bar, prompt bar with `@` mentions, `/` commands and queueing) or a terminal.
  - Code modal: Esc steps out of find, then extra cursors, then the modal.
  - New Agent Task dialog.
  - Stop (Esc / Cmd+.), Discard, Reveal and Worktree terminal.
  - Tasks persist and resume through `session/load`.
- **Acceptance** (see `docs/budget.md`, M2):
  - three Claude tasks ran in parallel, footprint 81 MB;
  - quit leaves no orphans;
  - the permission flow and resume were verified with tiny prompts.

Tested by script plus screenshots, not by hand: typing in the prompt, drag-to-prompt, `/` commands with a real command list, Discard, and resizing the sidebar. The owner should try these.

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
- GPUI throttles unfocused or occluded windows by default. Qur sets `inactive_frame_interval` to 16 ms; verify in M3 that a glow lands within 100 ms while the user is in another app.
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
cargo test --release --workspace                 # 84 tests, incl. the theme contrast test
scripts/budget.sh 5                              # needs an unlocked screen; prefer AC power
QUR_DEBUG_ACTIONS="qur::ThemePicker" scripts/shot.sh out.png <file>   # scripted screenshot
python3 spikes/fixtures/gen.py                   # fixtures (once)
spikes/worktree_spike.sh claude claude-agent-acp # M0 ACP + worktree + merge check
```

Env: `QUR_HOME` (settings folder; **always set it to a scratch dir in tests**, so the owner's real settings stay untouched), `QUR_BENCH=first_frame|scroll|type|theme`, `QUR_DEBUG_ACTIONS` (comma-separated action names, `wait:<ms>`).

## Next

1. Owner reviews M2 by hand: `target/release/qur ~/some/repo`, then Cmd+Shift+N.
2. **M3: peek + heatmap.** Planned:
   - The Live view (follow the agent's current file, Take over), probably as the code modal in "follow" mode, since there is no code area now (decision 18).
   - The peek layouts reinterpreted for the stage, plus the heatmap states in the Files tree (via the FSEvents watcher plus the ACP `toolCallId` state from M0), folder dots, heat levels, and overlap dots on the main tree.
   - Orca items for M3: never overwrite unseen disk changes; "needs you" ordering options.
   - Accept: a glow within 100 ms, Live follows across files, and state survives layout switches. Also check the GPUI inactive-window throttle with Qur unfocused.
3. Known gaps:
   - IME marked text isn't drawn.
   - End and Cmd+Right go to the line end, not the visual-row end.
   - No file watcher yet; trees refresh when the window is activated.
   - Agent Terminals don't pre-trust the worktree. Claude's "trust this folder?" prompt can appear; see the open question below.

## Open questions for the owner

- Trademark clearance for "Qur" hasn't been checked (brand README). It needs flagging in the public README at M7.
- Approve the accent palettes (Qur Dark: #ff7a45 #3ddc97 #5b9bff #f062c0 #f5c04a #a77bff; Qur Light: #d9480f #0e9f6e #2563eb #c026d3 #0891b2 #db2777).
- Kotlin adds about 5.5 MB to the binary. Keep it, or load it lazily later?
- Orca item: pre-trust new worktrees for Claude CLI terminals. That means Qur writing to `~/.claude.json`, which is another app's config. OK or not?
- Which of Orca's "conflicts" items, if any, should override a non-goal? See the table in `docs/research/orca.md`.
