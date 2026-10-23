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
- `Cmd+Shift+T` = a full Claude agent task (own worktree, sidebar row, heatmap), shown as a terminal.
- Peek-layout cycle is `Cmd+K Cmd+L`; `Cmd+Shift+L` stays Select All Matches.

## Status

**Milestone: M0 (spikes), work done; waiting on the owner's answers.** Committed as "M0: spikes".

| Spike | Result | Where |
|---|---|---|
| ACP (Claude Code) | **Done.** Subscription login, no API keys; prompt, permissions, cancel, orphan-free shutdown | `spikes/acp_spike/`, `docs/spikes/acp-worktree.md` |
| ACP (Codex) | Deferred by the owner; packages uninstalled | n/a |
| Worktree + merge | **Done.** Merge into a dirty checkout keeps index and stash intact; safe refusal on overlap | `spikes/worktree_spike.sh` |
| Toolkit | **Done. Recommendation: GPUI.** Footprint 71 vs 120 MB, first frame 148 vs 211 ms, binary 8.5 vs 10.2 MB | `docs/spikes/toolkit.md`, `docs/budget.md` |

M0 acceptance: Claude works on its subscription login ✅. Toolkit chosen with numbers: recommendation made, owner to confirm. Owner has seen the report: pending.

## Key findings so far

- Both ACP adapters speak **ACP v1** (v2 is alpha). Qur targets v1.
- A hand-rolled ACP client (serde_json + std threads) is 548 KB, versus 806 KB / 63 crates for the official crate's types alone. Plan: hand-roll the transport in `qur_acp`.
- Claude's `tool_call` arrives with `kind` but **empty `locations`**; the path comes 0.5 to 0.9 s later in a `tool_call_update`. The heatmap must track `toolCallId` state. Status goes `pending` to `completed` with no `in_progress`.
- Spawn agents with `process_group(0)` and signal the group: no orphans.
- Plain `git merge` refuses when the user has staged changes. Use `merge-tree --write-tree`, then `commit-tree`, then `read-tree -m -u`, then `update-ref`. Needs git 2.38 or newer.
- Highlighting the whole 10k-line file with tree-sitter takes 50 to 90 ms. M1 needs incremental parsing and viewport-first highlighting to keep file open under 100 ms.
- The owner's Mac (MacBook Air M5, Mac17,3) has a **60 Hz** panel. The "one frame at 120 Hz" typing target is judged on CPU-side key-to-draw time (8.3 ms or less), not on observed vsync.
- Idle RSS counts about 40 MB of shared macOS framework pages. Physical footprint: GPUI 71 MB, iced 120 MB (iced's GPU window buffers alone are 59 MB). Proposed: budget on footprint.
- GPUI throttles unfocused or occluded windows by default. Qur must configure this so the heatmap updates while the user is in another app; check it in M3.
- Running on battery capped the display at 60 Hz in these runs; on AC, frames paced at 120 Hz.
- GPUI is pinned to Zed git rev `9e84b066`, and building it needs the Xcode Metal toolchain (installed). Zed source for reference: `~/src-ref/zed` (sparse clone).

## Repo layout

```
HANDOFF.md            this file
docs/                 QUR_HANDOFF.md, decisions.md, keymap.md, menus.md, spikes/
assets/brand/         logo kit
scripts/              spike_measure.sh (toolkit metrics), winid.swift (window screenshot helper)
spikes/
  fixtures/gen.py     generates big-repo/ (5,000 files) and large.ts (10,000 lines); outputs gitignored
  spike_common/       shared highlighter, file-tree model, metrics, Qur Dark palette
  toolkit_gpui/       GPUI prototype
  toolkit_iced/       iced prototype
  acp_spike/          ACP v1 client spike
  worktree_spike.sh   end-to-end worktree + agent + merge test
  out/                run logs, screenshots (gitignored)
```

The real Cargo workspace (`crates/qur_*`, handoff section 6) starts in M1.

## Machine setup

- Rust stable via rustup (1.99.1); run `source ~/.cargo/env` in shells that don't have it yet.
- Xcode is installed (needed for GPUI's Metal shaders and `swiftc`).
- `npm i -g @agentclientprotocol/claude-agent-acp@0.94.0` (pinned; never `npx` at runtime).
- Claude Code CLI logged in via claude.ai. Never set `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`.
- Codex packages were uninstalled (Codex deferred). The owner's pre-existing `~/.codex/` folder is not ours; leave it alone.

## How to run things

```sh
python3 spikes/fixtures/gen.py                          # fixtures (once)
spikes/worktree_spike.sh claude claude-agent-acp        # ACP + worktree + merge, prints PASS
spikes/worktree_spike.sh claude claude-agent-acp --cancel-after-first-tool-call
(cd spikes/toolkit_gpui && cargo build --release)
scripts/spike_measure.sh gpui spikes/toolkit_gpui/target/release/toolkit_gpui
scripts/spike_measure.sh iced spikes/toolkit_iced/target/release/toolkit_iced
```

Spike env vars: `QUR_SPIKE_EXIT_AFTER_FIRST_FRAME`, `QUR_SPIKE_AUTOSCROLL`, `QUR_SPIKE_AUTOTYPE`, `QUR_SPIKE_EXPAND_ALL`, `QUR_SPIKE_OPACITY=0.85`, `QUR_SPIKE_ALLOW_SAVE` (otherwise Cmd+S never writes the fixture), `QUR_SPIKE_ROOT`, `QUR_SPIKE_FILE`.

## Next

1. The owner confirms the toolkit (GPUI recommended) and answers the section 15 questions plus the open questions below.
2. Optionally, the owner opens both prototypes to compare text quality by eye. Automated screenshots need Screen Recording permission for the terminal app.
3. Start M1 in the chosen toolkit: create the `crates/qur_*` workspace (handoff section 6), with themes (incl. per-project, decision 13), the editor shell, the keymap from `docs/keymap.md` and menus from `docs/menus.md`. Lift working pieces from `spikes/toolkit_<chosen>`.
4. Write `scripts/budget.sh` (from `spike_measure.sh` plus footprint) and record M1 in `docs/budget.md`.

## Open questions for the owner

- Trademark clearance for "Qur" hasn't been checked (brand README). It needs flagging in the public README at M7.
- Toolkit: confirm GPUI?
- Idle memory budget: measure physical footprint instead of RSS?
- Section 15 defaults:
  - repo location `~/qur`;
  - file tabs later: no;
  - "by PEUR Technologies" in About only;
  - 6-accent palette and theme licensing, reviewed at the start of M1.
