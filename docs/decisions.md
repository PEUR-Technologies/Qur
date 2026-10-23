# Owner decisions after the handoff

These amend `docs/QUR_HANDOFF.md`. Newest last.

## 2026-10-23

1. **Unified top bar (like Zed).** Transparent macOS titlebar with full-size content. Only the traffic lights remain, inset into Qur's own top bar, which uses the window background with a 1 px hairline below. The bar holds the Qur symbol and the project switcher, drags the window, and zooms on double-click.
2. **Compact layout.** Sidebar 220 px with truncation ("…") instead of wrapping. Tight editor gutter and margins.
3. **Soft wrap** of editor lines at the editor width, on by default. Continuation rows have no line number.
4. **Terminal shortcuts.** The bottom panel holds several terminals in a slim tab strip (terminal tabs only; still no file tabs).
   - `Cmd+T`: new terminal (login `$SHELL`, cwd = the project, or the selected agent's worktree).
   - `Cmd+Shift+T`: new terminal with `claude` already running (shell stays after it exits). Uses the CLI's own login; API keys are never set.
   - `Cmd+W`: close the focused terminal (kills its process group).
   - These override VS Code's `Cmd+T` (go to symbol) and `Cmd+Shift+T` (reopen closed editor). Neither exists in Qur v1 (no LSP, no tabs), so nothing is lost.
5. **Terminal pulled into the M0 spike.** The terminal panel (`alacritty_terminal`) is part of the M0 toolkit comparison, not deferred to M5.
6. **No Codex for now.** Codex is out of scope until the owner says otherwise. M0's acceptance becomes "Claude Code works on its subscription login". Do not ask for `codex login`. The roster and ACP client stay vendor-neutral, so Codex can be added back as config. M2's acceptance becomes three parallel Claude tasks.
7. **Full VS Code-style keymap + command palette.** Canonical list: `docs/keymap.md`. Every shortcut is also a palette command and, where macOS expects one, a native menu item. 
8. **Window transparency** (like the owner's Apple Terminal "Pro" profile). Config `[window] opacity` (0.5 to 1.0, default 1.0) and `blur` (default on when opacity < 1). Set from a small "Window: Transparency…" popover with a live slider (palette and View menu). This is an approved exception to "no settings UI beyond the pickers". Only backgrounds go translucent; text, cursors, selections, borders and the logo stay opaque. The contrast test checks themes at opacity 1.0 and at the 0.5 floor over both white and black desktops.
9. **Everything in the native menu bar, like Zed** (`docs/menus.md`). No toolbars, gear icons or settings screens in the window. Every command is in a menu and the palette. Unavailable items are disabled, never hidden.
10. **Peek-layout cycle moved to `Cmd+K Cmd+L`** (handoff section 8 said `Cmd+Shift+L`). `Cmd+Shift+L` stays Select All Matches, as in VS Code.
11. **Codex packages uninstalled** (`@openai/codex`, `@agentclientprotocol/codex-acp`). Only `@agentclientprotocol/claude-agent-acp@0.94.0` stays installed.
12. **`Cmd+Shift+T` creates a full Claude agent task**, not a plain terminal. Decision 4 is amended for this key.
    - Qur creates a worktree outside the repo on branch `qur/claude/term-<n>`, from the main checkout's HEAD (same lifecycle as `+` in handoff section 10).
    - It runs the interactive `claude` CLI in a PTY in that worktree; this is terminal mode, not ACP.
    - The task appears in the agents sidebar with an accent and status. The terminal is its Chat view and also shows as a tab in the bottom panel. Its edits drive the heatmap through the file watcher. Review, Merge and Discard work as for any task.
    - The title starts as "claude terminal <n>" and becomes the first line the user types. Closing the tab stops the process; the task and worktree remain until Merge or Discard.
    - If the project isn't a git repo, the key shows a one-line message ("Claude tasks need a git repo") and does nothing else.
    - `Cmd+T` stays a plain shell in the project, or in the selected agent's worktree.
13. **Theme per project**, so windows for different projects look different at a glance.
    - **Storage, nothing in the user's repo:** `~/Library/Application Support/Qur/projects/<repo-id>.toml`, with `theme = "Tokyo Night"` and optionally `opacity`. `<repo-id>` = the first 12 hex chars of the root commit, as for worktrees, so the setting follows the project across clones, moves and renames. A folder that isn't a git repo is keyed by a hash of its absolute path.
    - **Resolution order:** project theme, then global `config.toml` theme, then Auto. Agent task worktrees inherit their project's theme, so a peeked agent never looks like a different project.
    - **Setting it:** the theme picker (`Cmd+K Cmd+T`, Qur > Theme…) gets a scope toggle at the top: "This project" (default once a project is open) / "All projects". Qur > Theme also gets "Use Default Theme for This Project", which clears the override.
    - **Per window:** each window is one project, so two projects open side by side show two themes. Switching the project in the top-bar switcher swaps the theme instantly with no state loss (decision rules from handoff 12a still apply).
    - **Identity cue:** the project name in the top bar uses the theme's primary text color; nothing else is added. The logo stays black or white by background luminance (never tinted).
    - Opacity (decision 8) can also be set per project with the same scope toggle in the Transparency popover.
14. **Toolkit: GPUI** (Zed git rev pinned in the workspace; upgrade on purpose, at most once per milestone, re-running the budget). Evidence: `docs/spikes/toolkit.md`.
15. **Idle memory budget is measured as physical footprint** (Activity Monitor "Memory", `footprint <pid>`), with RSS reported alongside. Same targets: idle < 80 MB, 3 agents < 150 MB.
16. **Section 15 defaults accepted:** repo at `~/qur`; no file tabs (not even as an option for now); "by PEUR Technologies" only in About. The 6-accent palettes are chosen in M1 and shown to the owner with the M1 report.
17. **"New Claude Task" is now called "New Agent Terminal"** (`Cmd+Shift+T`) in menus, the command palette and the empty-editor hints. Behaviour is unchanged (decision 12).
18. **No permanent code editor area** (amends handoff sections 7–8). The window is for **files and agent sessions**. Code opens in a **modal** over the window, which keeps full editing, find and save. **Esc** closes it, stepping out one layer at a time: close find, then collapse multi-cursor, then close the modal. Unsaved edits stay in memory: the file shows a dirty dot in the tree, and Cmd+S saves, also from inside the modal. Quitting with unsaved edits still asks first.
19. **M2 window layout.**
    - **Left column (resizable, 280 px default):**
      - AGENTS: ACP tasks and Agent Terminals.
      - TERMINALS: plain shells, shown only when there are any.
      - FILES: expanded by default and taking all remaining height. This replaces the earlier "collapsed dropdown".
      - The tree shows the selected agent's worktree, or the main checkout when no agent is selected.
    - **Stage (rest of the window):** the selected session.
      - ACP agent: header (name, status, branch), To-do strip, Chat stream, permission bar, prompt bar.
      - Agent Terminal or shell: the terminal itself.
    - The bottom panel is gone (Cmd+J has nothing to toggle and is disabled). Shells live in the stage like agents.
20. **Automated agent tests use a tiny prompt** to save the owner's Claude usage. The M2 parallel check is three tasks with a one-line prompt.
21. **Take Orca's ideas where they fit** (owner, 2026-10-23: "take a ton of features from orca"). The inventory is in `docs/research/orca.md`: 125 features, each marked Adopt, Adopt adapted, or Conflicts. Orca is MIT (GPL-compatible), but it's TypeScript/Electron, so we take ideas, not code. Features that conflict with a constraint (browser, hosted-git integrations, cloud/SSH, telemetry, a terminal daemon that outlives the app, push) are **not** adopted unless the owner says so per item.
22. **Orca features adopted in M2:**
    - `.worktreeinclude`. A repo-root file the user writes (Qur only reads it) with one gitignore-style path per line. Matching files and folders from the main checkout, like `.env` and `node_modules`, are copied into each new worktree with APFS copy-on-write (`cp -c`): instant and no extra disk.
    - A per-project setup command: `[worktree] setup = "pnpm install"` in `projects/<id>.toml`. It runs in the new worktree through the login shell before the agent starts. Its output shows in the chat, and the agent starts only on success.
    - Sidebar rows show the agent's last message, and an unread marker on agents that finished or need you while not selected.
    - Composer: `/` opens the agent's slash commands (from ACP `available_commands_update`). Messages sent during a turn are queued and sent when it ends.
