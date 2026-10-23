# Orca feature inventory, mapped onto Qur

Research date: 2026-10-23. Subject: [stablyai/orca](https://github.com/stablyai/orca) at release v1.4.237 (2026-10-21), shallow clone of `main`, plus the in-repo docs site (`docs/site/content/docs/**.mdx`, published at https://www.onorca.dev/docs) and recent release notes.

Paths below are relative to the Orca repo root. "Docs" means `docs/site/content/docs/<page>.mdx`. Where a feature appears only in docs, or only in source, the row says so.

## 1. What Orca is

- **Pitch:** "The AI Orchestrator for 100x builders. Run Codex, Claude Code, OpenCode or Pi side by side, each in its own worktree, tracked in one place" (`README.md`). It is a desktop IDE for macOS, Windows and Linux, with an iOS/Android companion app.
- **Stack:** Electron 43, React 19, Monaco editor and xterm.js (`package.json`: `electron 43.7.8`, `monaco-editor ^0.55.1`, `@xterm/xterm 6.1.0-beta`). Main process code is in `src/main/`, UI in `src/renderer/src/components/`, the CLI in `src/cli/`, the mobile app in `mobile/`, and the relay service in `cloud/`.
- **Size:** `orca-macos-arm64.dmg` is 254 MB (v1.4.237 release assets). Qur's binary budget is under 40 MB.
- **Agent model:** the agent's own TUI runs in a PTY by default. A structured "Chat UI" is experimental (docs `agents/native-chat`). Orca has an ACP client in `src/main/acp/` with dialects for Grok, OpenCode, Antigravity and OMP (`src/main/acp/acp-dialects/`). Claude's native chat goes through the Claude Agent SDK stream (`src/main/claude/claude-agent-sdk-*.ts`), not ACP.
- **Defaults that differ from Qur:** agents launch in full-bypass mode by default (`--dangerously-skip-permissions` and similar; docs `model/agents-sessions`, `agents/supported`). Anonymous telemetry is on unless you opt out (docs `telemetry`). A background daemon keeps PTYs alive after the app quits (docs `model/session-restore`, `src/main/daemon/`).

## 2. License and code reuse

- **SPDX: `MIT`** (`LICENSE`, "Copyright (c) 2026 Lovecast Inc."; GitHub reports `mit`).
- **Can Qur reuse the code (Qur is GPL-3.0-or-later)?** Yes. MIT is GPL-compatible, so MIT code can go into a GPL-3.0-or-later work. The one condition is keeping Orca's copyright notice and the MIT permission text, for example as an entry in `docs/THIRD_PARTY.md` and a header on any ported file.
- **In practice, little will be copied verbatim.** Orca is TypeScript, Electron and React, and Qur is Rust and GPUI. What carries over:
  - algorithms and edge-case handling (worktree shared paths and copy budgets, agent idle detection, transcript scanning, workspace trust pre-seeding);
  - prompt templates (commit message, PR details, review-notes batch);
  - config conventions (`orca.yaml`, `.worktreeinclude`).
- **Exclusions to check before copying any asset:**
  - third-party agent logos and icons (`src/shared/agent-icons/`) are other companies' trademarks;
  - the docs site has its own `docs/site/THIRD_PARTY_NOTICES.md`;
  - bundled dependencies keep their own licenses.

## 3. How to read the tables

- **Verdict:**
  - **Adopt**: fits Qur as is.
  - **Adopt, adapted**: fits once changed to respect lightness, native-only and local-only.
  - **Conflicts**: breaks a stated constraint or v1 non-goal. These rows are still listed so the owner can choose to override.
- **Milestone:** M2 agents in worktrees, M3 peek and heatmap, M4 changes, merge and git panel, M5 terminal, M6 search, vim, first-run and alerts, M7 polish, or "new" for a new milestone.
- **Effort:** S is a few days or less, M is about a week or two, L is more.
- "(planned)" means Qur's spec already calls for it, and Orca serves as a reference implementation.

---

## 4. Inventory

### 4.1 Worktrees and the task lifecycle

| ID | Feature | Orca evidence | Verdict | MS | Effort | Notes for Qur |
|---|---|---|---|---|---|---|
| W1 | A real git worktree per task, with a per-repo **base ref** (default `origin/main`) | docs `model/worktrees`; `src/main/git/worktree-add.ts` | Adopt | M2 | S | (planned) Qur branches from HEAD. A per-repo base-ref setting is the only addition. |
| W2 | **Start-from picker**: base ref, local branch (stacking), commit SHA or remote branch | docs `model/worktrees` "Start-from picker" | Adopt | M2 | S | Small field in the `+` new-task flow; defaults to HEAD. |
| W3 | Worktree creation runs **in the background**: the dialog closes at once, the sidebar shows a progress row, and creation can be cancelled or retried | docs `model/worktrees` "Creation runs in the background"; `src/renderer/src/components/worktree-creation/` | Adopt | M2 | S | The agent row appears immediately in a "preparing" state. |
| W4 | **Auto-naming**: a random name when the task name is blank, then the branch, title and folder are renamed after the agent's first work | docs `settings` (Git: "Auto-Rename Branch From Work"); `src/main/agent-hooks/first-work-branch-rename.ts`, `first-work-workspace-title-rename.ts`, `src/main/text-generation/conversation-name-generation-request.ts` | Adopt, adapted | M2 | S | Slug the first prompt. A nicer title would need a one-shot call through the user's own agent CLI, never an API key. |
| W5 | **Shared gitignored paths**: listed paths (e.g. `node_modules`, `.cache`) are put into each new worktree as an APFS clone-copy or a symlink | docs `model/worktrees`, `model/orca-yaml` (table "Sharing versus copying") | Adopt | M2 | M | Without this a fresh worktree has no deps. `clonefile(2)` is cheap on APFS. |
| W6 | **`.worktreeinclude`**: gitignored files such as `.env` are copied privately into each worktree, with a 2 GiB / 50k-entry budget | docs `model/orca-yaml`; `src/main/ipc/worktree-include-copy-budget.ts` | Adopt | M2 | S | Read the same filename so repos that already have one work unchanged. |
| W7 | **Repo setup script** that runs in the new worktree, with `wait-for-setup` so the agent starts only after it succeeds; an **archive script** runs before removal | docs `model/orca-yaml`; `src/main/hooks.ts` | Adopt, adapted | M2 | M | Keep it in Qur's per-project config outside the repo, and optionally read `orca.yaml` `scripts.setup` for compatibility. Show output in a Setup terminal. |
| W8 | Setup env vars `ORCA_ROOT_PATH`, `ORCA_WORKTREE_PATH`, `ORCA_WORKSPACE_NAME`, plus `CONDUCTOR_ROOT_PATH` compatibility aliases | docs `model/orca-yaml` | Adopt | M2 | S | Expose `QUR_*` and the same aliases. |
| W9 | `defaultTabs`: terminals with titles, commands and colours opened automatically for a new worktree | docs `model/orca-yaml` | Adopt, adapted | M5 | S | Qur has no tabs, so make these default bottom-panel terminals per task (e.g. a dev server). |
| W10 | Pin, sleep, archive and delete a worktree (delete removes the branch); multi-select bulk actions; `Cmd-Shift-Backspace` delete with confirmation | docs `model/worktrees` "Sidebar layout" | Adopt | M4 / M7 | S | Delete is Qur's "Discard task" (M4). Pin and bulk actions are polish (M7). |
| W11 | **Preserved branches**: when git refuses to drop an unmerged branch, a "Review N branches" toast lets you force-delete some | docs `model/worktrees` "Preserved branches" | Adopt | M4 | S | Safety net for Discard. |
| W12 | **Workspace cleanup** ("Resource Manager → Clean up workspaces", "Space"): lists each workspace's size, git state, recent activity and linked review, and lets you inspect changed files before removing | docs `model/worktrees`; `src/renderer/src/components/workspace-cleanup/`, `workspace-space/`; v1.4.232 notes | Adopt, adapted | M7 | M | Disk use of `~/Library/Application Support/Qur/worktrees` grows quickly; a native list view is enough. |
| W13 | **Show external worktrees** (made with `git worktree add`, Claude Code's own worktrees, GSD) from a "Non-Orca worktrees" dialog | docs `model/worktrees` "Using plain git" | Adopt | new | M | Lets Qur adopt a worktree the user or Claude created elsewhere. |
| W14 | **Parent and child worktrees** shown nested in the sidebar, with "Sleep/Delete with descendants" | docs `model/worktrees` | Adopt, adapted | new | M | Only useful once Qur has sub-tasks or orchestration. |
| W15 | **Multi-repo project groups and folder workspaces** | docs `model/worktrees` "Multi-repo project groups"; `src/main/project-groups/` | Adopt, adapted | new | L | Qur is one project per window; defer. |
| W16 | **Sparse-checkout presets** when creating a worktree | Source only: `src/renderer/src/components/sparse/` (`SparsePresetChooser.tsx`, ...). Not in docs. | Adopt, adapted | new | M | Valuable for monorepos. Behaviour not verified beyond file names. |
| W17 | Emoji workspace names with `:shortcode:` picker | docs `model/worktrees` "Emoji workspace names" | Adopt | M7 | S | Low value. |
| W18 | **Workspace status** (todo, in-progress, in-review, completed) and a **Workspace Board** kanban | docs `cli/worktree-checkpoints`, `settings` ("Toggle Workspace Board"); `src/renderer/src/components/sidebar/use-workspace-kanban-board-projection.ts` | Adopt, adapted | M7 | M | Qur could derive the state (working, done, reviewed, merged) instead of asking the user to set it. |
| W19 | Linked GitHub, Linear, Jira or GitLab item on a worktree; create a worktree from an issue or PR | docs `model/worktrees`, `review/github`, `review/linear`, `review/jira` | **Conflicts** | – | M | Non-goal (GitHub/Linear integrations) and network. A cheap local alternative is a free-text "link" field. |
| W20 | Run a worktree on an SSH host, a Remote Orca Server or a Cloud VM ("Run on") | docs `ways-to-run`, `ssh`, `remote-servers` | **Conflicts** | – | L | Non-goal: remote/SSH and cloud. |

### 4.2 Agents and sessions

| ID | Feature | Orca evidence | Verdict | MS | Effort | Notes for Qur |
|---|---|---|---|---|---|---|
| G1 | Built-in catalog of about 40 agent CLIs, auto-detected, with default launch args; agents can be enabled or disabled | docs `agents/supported`; per-agent dirs in `src/main/` (`claude/`, `codex/`, `gemini/`, `opencode/`, ...) | Adopt, adapted | M6 | S | Ship a small table (binary name, ACP adapter or not, resume flag) for the first-run picker to write `agents.toml`. |
| G2 | Status glyph set: spinner (working), amber `?` (needs you), green (done), red (failed or interrupted), grey (idle), plus done-but-unread | docs `model/agents-sessions` "State indicators"; `src/renderer/src/components/sidebar/StatusIndicator.tsx` | Adopt | M2 | S | (planned) Adds "unread done" as a separate state, which matters when you come back to many agents. |
| G3 | **Status detection for TUI agents** from OSC title sequences and Orca-managed **agent hooks**; the hook endpoint is re-read so sessions survive app restarts | docs `model/agents-sessions`, `agents/hooks-memory`; `src/main/agent-hooks/` (`agent-transition-detector.ts`, `managed-agent-hook-registry.ts`), `src/main/agent-turn-idle-verdict.ts` | Adopt, adapted | M5 | M | For Qur's terminal-mode agents: a local Unix-socket hook (no TCP), installed only with consent. Never silently rewrite `~/.claude/settings.json`. OSC title alone is the zero-touch fallback. |
| G4 | **Agent Dashboard**: kanban of Needs You, Working, Done and Idle (idle hidden by default), with search, filters and a pop-out window | docs `model/agents-sessions` "Agent dashboard"; `src/renderer/src/components/dashboard/`, `dashboard-popout/` | Adopt, adapted | M3 | S | Qur's agents list already does this job. Borrow the grouping or sort (Needs You first) and hiding of idle agents, not a separate kanban. |
| G5 | **Preview line** on each agent card: last user or agent message, or the pending question; Needs-You cards tinted | docs `model/agents-sessions` "Cards" | Adopt | M2 | S | A second line under the task title in the sidebar row. |
| G6 | **Agents feed**: threaded, chronological feed of turn completions, blocking questions and worktree creation, with response previews, unread badge and `Cmd-F` filter | docs `activity`; `src/renderer/src/components/activity/` | Adopt, adapted | M6 | M | A catch-up surface after being away. Could share a model with the alerts inbox (A2). |
| G7 | **Subagents and Agent Teams** shown as expandable child rows under the lead agent | docs `agents/claude-code` "Subagents and teams"; `src/main/claude/claude-background-task-*.ts` | Adopt, adapted | M3 | M | Derive from ACP `Task` tool calls and nest them under the agent row. |
| G8 | **Restart chip** when an agent process exits; reopens the same agent in the same cwd | docs `model/agents-sessions` "Restart chip" | Adopt | M2 | S | |
| G9 | **Permission mode** Manual, Auto or Yolo (global, per-agent overrides). Orca's shipped default is Yolo. | docs `agents/supported`, `settings` (Agents); v1.4.232 notes | Adopt, adapted | M2 | S | Keep Manual as the default with Qur's permission bar, and offer Auto or Yolo per task. Note that Orca defaults to bypass; Qur should not. |
| G10 | **Workspace-trust pre-seeding**: Orca writes the agent's "trusted folder" record so the "Do you trust this folder?" prompt does not block in a fresh worktree | Source: `src/main/agent-trust-presets.ts`, `src/main/agent-workspace-trust.ts`, `src/main/claude/claude-folder-trust-file*`, `src/main/codex/config-toml-trust*` | Adopt | M2 / M5 | S | Qur's worktrees live outside the repo, so every task is a new folder. Check whether the ACP adapters hit this prompt; terminal-mode Claude will. |
| G11 | **Agent session history**: scans `~/.claude`, `~/.codex` and other CLI transcripts; search; scope to Workspace, Project or All; show the full first prompt; Resume, copy resume command, open log | docs `agents/session-history`; `src/main/ai-vault/`, `src/main/ai-vault-search/` | Adopt | new | M | Read-only local file parsing. Resume through ACP `session/load` or `claude --resume <id>` in a PTY. |
| G12 | **Agent hibernation**: stops finished, untouched background agents after N minutes (default 30) and resumes them with `--resume` when reopened | docs `agents/hibernation` | Adopt, adapted | M7 | M | Fits the idle-memory budget well. Only for agents with resumable sessions. |
| G13 | **Session restore**: worktrees, layout, scrollback and focused tab return on relaunch | docs `model/session-restore` | Adopt, adapted | M7 | M | (planned "tasks persist and can be resumed") Restore the task list, then resume each agent session on demand, not all at launch. |
| G14 | **Background PTY daemon**: agents keep running after Cmd-Q or an app crash; warm reattach on next launch | docs `model/session-restore`; `src/main/daemon/` (about 250 files) | **Conflicts** | – | L | Breaks Qur's "no orphaned processes; on quit agents stop" rule and adds a resident process. G12 and G13 cover most of the value. |
| G15 | **Usage and rate-limit readout**: plan use, 5-hour, daily and weekly reset times, an 80% warning chip, and a status-bar popover listing every provider | docs `agents/usage-tracking`; `src/main/claude-usage/`, `codex-usage/`, `src/main/rate-limits/` | Adopt, adapted | M7 | M | The docs say it reads local CLI state with "no API calls". However `src/main/rate-limits/service/service-fetch-policy.ts` suggests some fetching (**unverified**). Qur should only read local files. |
| G16 | **Multi-account hot-swap** for Claude, Codex and others, using isolated per-account homes | docs `agents/codex-hot-swap`, `agents/claude-code`; `src/main/claude-accounts/`, `codex-accounts/`, `grok-accounts/` | Adopt, adapted | new | M | Local and subscription-only. Niche, and touches CLI auth dirs, so it needs care. |
| G17 | **Stats**: per-provider daily usage charts, token-activity heatmap, inferred cost | docs `agents/usage-tracking` "Estimated cost"; `src/renderer/src/components/stats/` | Adopt, adapted | new | M | Local only, low priority. |
| G18 | **Keep the computer awake** while an agent works (On, Agent or Off; "Caffeinate" in the status bar) | docs `settings` (Agents); `src/main/agent-awake-service.ts` | Adopt | M7 | S | An `IOPMAssertion` while any agent is working. |
| G19 | **Voice dictation** with on-device models (Parakeet, Whisper Tiny, SenseVoice) or cloud OpenAI with an API key | docs `settings` "Voice"; `src/main/speech/`, `src/renderer/src/components/dictation/` | Adopt, adapted | new | L | macOS system dictation works for free if the GPUI text input supports it. Downloadable on-device models would hurt the weight budget. The cloud option **conflicts** (API key, network). |
| G20 | **Pet overlay**: an animated sprite that reacts to agent state | Source only: `src/renderer/src/components/pet/` | Adopt, adapted | M7 | S | Off by default if done at all; low value. |
| G21 | Per-agent launch args and env, editable, with a Reset button | docs `model/agents-sessions` "Launch defaults" | Adopt | M6 | S | Already implied by `agents.toml`; needs a reset-to-default path. |
| G22 | **Claude Agent Teams** launched with native panes per teammate (`orca claude-teams`) | docs `agents/supported` | Adopt, adapted | new | L | Defer. |

### 4.3 Structured chat (Qur's ACP stage)

| ID | Feature | Orca evidence | Verdict | MS | Effort | Notes for Qur |
|---|---|---|---|---|---|---|
| C1 | Structured chat transcript and composer (experimental in Orca) | docs `agents/native-chat`; `src/main/native-chat/`, `src/main/acp/`, `src/renderer/src/components/native-chat/` | Adopt | M2 | – | (planned) This is Qur's core and Orca's experimental one, so Qur is ahead here. |
| C2 | **Question cards** (Claude `AskUserQuestion`) rendered in the transcript and answered inline with Submit | docs `agents/native-chat` "Questions from the agent" | Adopt | M2 | S | Pairs with the permission bar. |
| C3 | **Model, effort and mode pills**; Claude's model list probed from the installed CLI, not hardcoded | docs `agents/native-chat` "Composer"; `src/main/acp/acp-model-catalog-probe.ts` | Adopt | M2 | S | Use the ACP session modes and config options the adapter advertises. |
| C4 | Each conversation remembers its own model, effort and "Fast" settings | v1.4.235 notes | Adopt | M2 | S | |
| C5 | `/` slash commands and skills catalog, per agent (`$` for Codex skills) | docs `agents/native-chat`; v1.4.232 notes | Adopt | M2 | S | ACP `available_commands_update`. Qur's prompt bar already has `@file`. |
| C6 | **Attachments**: files and images, paste copied files, drag files or images from the tree into the prompt | docs `agents/native-chat`, `editing/file-explorer`; v1.4.230 notes | Adopt | M3 | M | (partly planned: drag from tree) Images are ACP image content blocks. |
| C7 | **Queued messages** while a turn runs, editable before they send | v1.4.230, v1.4.235 notes | Adopt | M2 | S | |
| C8 | **Rewind to here**: edit an earlier prompt and continue from that point | v1.4.235 notes | Adopt, adapted | new | L | Depends on adapter support; ACP has no general rewind (**uncertain**). |
| C9 | **Continue in terminal / continue in chat**: copy a conversation between the structured chat and the CLI TUI | docs `agents/native-chat`; `src/main/acp/acp-conversation-copy.ts` | Adopt, adapted | M5 | M | Qur version: open `claude --resume <session-id>` in a PTY in the same worktree. |
| C10 | **Show full output** for truncated tool results and messages | v1.4.235 notes | Adopt | M2 | S | |
| C11 | Context-usage meter, goals, and a usage-limit-reset notice with a **Continue** button | v1.4.229, v1.4.232 notes; `src/main/acp/acp-context-usage.ts` | Adopt | M2 / M7 | S | |
| C12 | **Turn-by-turn edits reviewed inline** in the chat | v1.4.229 notes | Adopt | M4 | M | Natural in Qur: each turn's diff links to the Changes view. |
| C13 | Stop ends the agent's whole process tree, including background commands and subagents | v1.4.220 notes; `src/main/claude/claude-child-root-termination.ts` | Adopt | M2 | S | (planned) Qur already uses process groups. |
| C14 | Install a missing agent from the chat itself | v1.4.230 notes | Adopt, adapted | M6 | S | Show the install command; never `npx` at runtime (Qur rule). |

### 4.4 Review, diff and git

| ID | Feature | Orca evidence | Verdict | MS | Effort | Notes for Qur |
|---|---|---|---|---|---|---|
| R1 | **Combined diff** against the start-from ref (staged, unstaged and untracked), with a switchable compare base | docs `review/diff-viewer`; `src/renderer/src/components/editor/DiffViewer.tsx`, `combined-diff/` | Adopt | M4 | M | (planned) |
| R2 | **Annotate AI Diff**: `+` in the gutter or `c` adds a line comment (markdown, `Cmd-Enter` saves); comments follow the line; **Send to agent** batches all notes into one prompt; resolve; unresolved notes ride the next batch | docs `review/annotate-ai-diff`; `src/renderer/src/components/diff-comments/`, `editor/DiffNotesSendMenu.tsx` | Adopt | M4 | M | The strongest review-loop idea in Orca. Send goes to the task's own agent through the prompt bar. |
| R3 | **AI attribution**: gutter marker on lines an agent wrote; a human edit flips it back | docs `review/attribution` only; **no implementation found in source** (uncertain) | Adopt, adapted | M4 | M | Qur already tracks ACP tool-call locations for the heatmap, so it can tint AI-written hunks in the Changes view. |
| R4 | Diff file tree (resizable), word-wrap toggle, line numbers, `F7`/`Shift-F7` change navigation, hide unchanged regions | docs `review/diff-viewer`; `editor/diff-editor-*-options.ts` | Adopt | M4 | S | |
| R5 | **Image diff**: side-by-side, swipe and onion-skin | docs `review/diff-viewer` | Adopt | M7 | M | |
| R6 | **Three-way merge-conflict UI** with inline resolution and a conflict file tree | docs `review/diff-viewer`; `editor/ConflictComponents.tsx`, `ConflictReviewFileTree.tsx` | Adopt | M4 | M | (planned: ours, theirs, both) |
| R7 | **"…with AI" repair actions**: Resolve with AI (conflicts), Fix with AI (failed commit hook, given the hook output and staged list) | docs `review/commit-push`; `src/main/text-generation/source-control-agent-*.ts` | Adopt, adapted | M4 | S | Send a structured prompt to the task's agent. The "Fix broken checks" (CI) variant is part of R12. |
| R8 | **AI commit message and PR details**, generated by running an agent CLI, with editable per-repo **action recipes** (agent, args, template variables such as `{stagedPatch}` and `{branch}`) | docs `review/commit-push` "Per-repo AI action recipes"; `src/main/text-generation/commit-message-*.ts` | Adopt, adapted | M4 | M | One-shot through the user's subscription CLI (e.g. `claude -p`), never an API key. A Merge commit message for Qur would be a good first use. |
| R9 | **Source Control panel**: stage, discard, commit, amend (explicit); abort merge or rebase; the primary button changes with state (Stage, then Commit, then Push) | docs `review/commit-push` "Source control panel"; `src/renderer/src/components/source-control/` | Adopt | M4 | M | (planned Git panel) Leave out push and pull (R11). |
| R10 | **Branch → base row** with a `+n −m` chip measured from the merge-base, hover breakdown into Source, Tests and Generated (path heuristics), and "Committed on branch" | docs `review/commit-push` | Adopt | M4 | S | Fits the agent row and the Changes header. |
| R11 | **Push, Pull, Sync** and **force-push with lease** | docs `review/commit-push` "Push" | **Conflicts** | – | S | Spec: "Qur never force-pushes". Push and pull is network beyond agent-to-vendor traffic. The owner may allow plain push and pull as user-initiated git. |
| R12 | Hosted reviews: create PR/MR (GitHub, GitLab, Bitbucket, Azure DevOps, Gitea), checks and Actions logs, review threads and reactions, auto-merge or merge queue, stacked PRs, Fix broken checks, issue drawer, GitHub Projects "Tasks", Linear, Jira | docs `review/github`, `review/linear`, `review/jira`; `src/main/github/`, `gitlab/`, `bitbucket/`, `azure-devops/`, `gitea/`, `linear/`, `jira/`; `src/renderer/src/components/task-page/`, `pull-request-page/` | **Conflicts** | – | L | Non-goal (GitHub/Linear integrations), network and accounts. |
| R13 | **Changes view mode** on an open file: toggles HEAD vs working tree in place | docs `editing/monaco` "Changes view mode"; `editor/ChangesModeView.tsx` | Adopt | M4 | S | Good inside Qur's code modal. |
| R14 | Review notes on rendered Markdown | docs `editing/markdown` "Review annotations" | Adopt, adapted | new | M | Depends on a Markdown preview (E3). |
| R15 | **Conflict prediction** against the base using `git merge-tree` before shipping | Source: `src/main/github/conflict-summary.ts` (in Orca it is tied to PR flow) | Adopt, adapted | M4 | S | Qur already plans `merge-tree --write-tree` for Merge. Run it as a dry run to show "will conflict" on the agent row before the user clicks Merge. |

### 4.5 Terminal

| ID | Feature | Orca evidence | Verdict | MS | Effort | Notes for Qur |
|---|---|---|---|---|---|---|
| T1 | Find in scrollback (`Cmd-F`, case, regex, match navigation) | docs `terminal` "Search" | Adopt | M5 | S | |
| T2 | Terminal themes, **Ghostty config import** (theme, font, cursor), Warp theme import | docs `terminal`; `src/main/ghostty/`, `src/main/warp-themes/` | Adopt, adapted | M7 | S | Import font and cursor only. User-imported themes are a v1 non-goal. |
| T3 | **Split panes**: drag a tab to an edge to split, nested; layout saved per worktree; watch several agents at once | docs `model/tabs-panes-splits` | Adopt, adapted | new | L | Qur has no tabs. The useful part is a "watch wall": several agents' peeks or terminals tiled at once. |
| T4 | **Floating terminal**: a global shell one chord away (`Cmd-Opt-A`), with its own tabs, not tied to a worktree | docs `terminal` "Floating terminal"; `src/renderer/src/components/floating-terminal/` | Adopt | M5 | S | |
| T5 | **Quick Commands**: saved commands and reusable agent prompts, global or per project; run in a new terminal or insert | docs `terminal` "Quick Commands"; `src/renderer/src/components/terminal-quick-commands/` | Adopt | M6 | S | Prompt presets also fit Qur's prompt bar. |
| T6 | OSC 52 clipboard writes (on by default), **kitty keyboard protocol** (real `Shift+Enter` for agent TUIs), OSC 8 hyperlinks, a link-action popover | docs `terminal` | Adopt | M5 | S | Check `alacritty_terminal` support for each. |
| T7 | Drop files on a terminal to paste their paths | docs `editing/file-explorer` | Adopt | M5 | S | |
| T8 | **Copy Context**: copies a bounded transcript of recent pane output | docs `terminal` | Adopt | M5 | S | |
| T9 | `$SHELL` as a login shell by default; custom shell and args | docs `terminal` "macOS and Linux shell" | Adopt | M5 | S | |
| T10 | **Listening-port detection** per workspace (which dev server belongs to which worktree), plus port forwarding for remote hosts | docs `ssh` "Port forwarding"; `src/main/ports/` (`local-workspace-port-scanner.ts`, `local-workspace-port-attribution.ts`) | Adopt, adapted | new | M | Local detection only; forwarding is SSH (**conflicts**). |
| T11 | **Resource Manager**: CPU, memory and sessions per terminal; End a PTY; daemon controls; disk scans | docs `terminal` "Resource Manager terminals", `settings` (Appearance); `src/main/memory/` | Adopt, adapted | M7 | M | Fits Qur's lightness story: show each agent's RSS. Leave out the daemon parts. |

### 4.6 Navigation, keyboard and settings

| ID | Feature | Orca evidence | Verdict | MS | Effort | Notes for Qur |
|---|---|---|---|---|---|---|
| N1 | **`Cmd-J` jump palette**: with an empty query it lists up to six recent chats ranked needs-you, then done, then idle, frozen while open, with `Cmd-1..6` to jump; also worktrees, projects, PR `#123`, open tabs; Shift-Enter opens in a split; creates a worktree from the query when nothing matches | docs `model/quick-open` "Worktree Jump Palette"; `src/renderer/src/components/cmd-j/` | Adopt, adapted | M6 | M | An agents and tasks palette. The frozen "needs you first" list is the key idea. |
| N2 | Quick Open (`Cmd-P`) shows gitignored files as a second pass after tracked matches | docs `model/quick-open` | Adopt | M6 | S | Qur already has `Cmd-P`; add the second pass. |
| N3 | New-tab omnibox (tabs, files, URLs, agents, web search) | docs `model/quick-open` | **Conflicts** | – | M | Built around the embedded browser and tabs. |
| N4 | **Remappable keymap** in `~/.orca/keybindings.json`; many actions ship unbound | docs `settings` "Shortcuts"; `src/main/keybindings/` | Adopt | M7 | M | A user keymap file next to `config.toml`. |
| N5 | Full searchable Settings UI (`Cmd-,`) | docs `settings` | **Conflicts** | – | L | Spec non-goal: no settings UI beyond the first-run and theme pickers in v1. Config files instead. |
| N6 | "Open in" an external editor menu (VS Code and others) | docs `settings` (General) | Adopt | M7 | S | |
| N7 | UI zoom, accent colour, density, app-icon variants | docs `settings` (General, Appearance) | Adopt, adapted | M7 | S | Accent colour conflicts with the monochrome Qur themes; zoom is fine. |
| N8 | Interface in 6 languages | docs `settings` "Language"; `src/main/i18n/`, `src/renderer/src/i18n/` | Adopt, adapted | new | L | |
| N9 | Onboarding feature wall, contextual tours, feature tips | `src/renderer/src/components/onboarding/`, `contextual-tours/`, `feature-tips/` | Adopt, adapted | M6 | S | Fold into Qur's first-run picker; no tours. |

### 4.7 Alerts

| ID | Feature | Orca evidence | Verdict | MS | Effort | Notes for Qur |
|---|---|---|---|---|---|---|
| A1 | Agent-finished and needs-input notification: system notification, sound, chip; Dock badge with the unread count | docs `notifications`; `src/main/notifications/`, `src/main/dock/unread-badge.ts` | Adopt | M6 | S | (planned) |
| A2 | **Inbox (header bell)**: unread notifications across worktrees, click to jump to the pane, right-click to **mark unread**; unread worktrees shown in bold | docs `notifications` "Persistent bell", "Mark unread" | Adopt | M6 | M | "Mark unread" is a cheap triage tool when many agents run. |
| A3 | Per-category toggles (system, sound, chip-only) and a **custom sound file** with volume | docs `notifications` "Tuning", "Custom sounds" | Adopt | M6 | S | |
| A4 | Mute notifications from hidden or sleeping workspaces; delivery depends on away or present state | Source: `src/renderer/src/attention/hidden-workspace-notification-mute.ts`, `src/main/notifications/desktop-away-state.ts` | Adopt | M6 | S | (planned "only alert when not focused on that agent") |
| A5 | **Menu-bar (tray) icon** that changes when an agent needs attention | Source: `src/main/tray/system-tray.ts`, `tray-attention-icon.ts` | Adopt, adapted | M7 | S | An `NSStatusItem`; optional. |
| A6 | Notifications for PR check failures and available updates | docs `settings` "Notifications" | **Conflicts** | – | S | GitHub integration and auto-update are out of scope for v1. |

### 4.8 Editor and viewers

| ID | Feature | Orca evidence | Verdict | MS | Effort | Notes for Qur |
|---|---|---|---|---|---|---|
| E1 | **Autosave** on blur and after short idle; no dirty dot | docs `editing/monaco` "Autosave"; `editor/editor-autosave-controller.ts` | Adopt | M7 | S | |
| E2 | **Never save over unseen on-disk changes**: refuses to save over changes made on disk that the user hasn't seen; Discard pauses autosave of that file | v1.4.233, v1.4.237 notes; `editor/editor-external-change-telemetry.ts` | Adopt | M3 | S | Critical in Qur, where agents write the same files the user opens in the modal. Goes with "Take over". |
| E3 | **Rich Markdown** editor and preview: slash menu, `[[` links, Mermaid, TOC, front matter, tables | docs `editing/markdown` | Adopt, adapted | new | L | A read-only native rendered preview at most. A WYSIWYG editor is out of proportion. |
| E4 | HTML preview in a sandboxed browser tab | docs `editing/viewers` "HTML" | **Conflicts** | – | M | Needs a webview. |
| E5 | PDF viewer (scroll, zoom, select, remembers position) | docs `editing/viewers` "PDF" | Adopt, adapted | new | M | Would need PDFKit or a Rust renderer inside GPUI (**uncertain feasibility**). |
| E6 | Image viewer (png, jpg, svg, webp, gif) | docs `editing/viewers` "Images" | Adopt | M7 | S | |
| E7 | CSV/TSV table viewer with sortable columns and search | docs `editing/viewers`; `editor/csv/` | Adopt, adapted | new | M | |
| E8 | Jupyter notebook viewer and editor (beta) | docs `editing/viewers`; `src/main/notebook/` | Adopt, adapted | new | L | |
| E9 | File explorer: live disk updates, git-status colours, natural numeric sort, Finder drag-in, Copy Path or Relative Path, Find in Folder | docs `editing/file-explorer` | Adopt | M3 | S | Feeds the heatmap tree. Natural sort and git colours are cheap. |
| E10 | Go-to-definition and language features (through Monaco) | docs `editing/monaco` | **Conflicts** | – | L | LSP is a v1 non-goal. |
| E11 | Minimap (off by default) | docs `editing/monaco` "Minimap" | Adopt | M7 | M | Optional. |

### 4.9 Platform, automation and integrations

| ID | Feature | Orca evidence | Verdict | MS | Effort | Notes for Qur |
|---|---|---|---|---|---|---|
| P1 | **`orca` CLI** that scripts the running app: `worktree create/set/current`, `terminal send`, `terminal wait --for tui-idle`, open a file or diff as a tab | docs `cli/overview`, `cli/reference`; `src/cli/` | Adopt, adapted | new | M | A `qur` CLI over a local Unix socket, no TCP. Lets agents open files in Qur's peek. Extends the planned `qur doctor`. |
| P2 | **Worktree checkpoints**: the agent sets a free-text status comment and workspace status through the CLI | docs `cli/worktree-checkpoints` | Adopt, adapted | new | S | Qur gets most of this from the ACP plan (To-do). A CLI hook helps terminal agents. |
| P3 | **Orchestration**: runs, tasks (DAG), dispatches, supervised workers, ask/decision gates, worker heartbeats | docs `cli/orchestration`; `src/cli/handlers/` | Adopt, adapted | new | L | Heavy; later, if ever. |
| P4 | **Skills page**: find, install, update and delete agent skills in `~/.claude`, Codex and OMP skill dirs; share skills by link | docs `cli/skills`; `src/main/skills/`, `src/renderer/src/components/skills/` | Adopt, adapted | new | M | The local list and update parts fit. Sharing by link needs an Orca account and cloud (**conflicts**). |
| P5 | MCP servers settings | docs `settings` (Integrations: "MCP servers"), details not documented | Adopt, adapted | new | M | **Uncertain** scope. Qur could show the agent's configured MCP servers read-only. |
| P6 | **Scheduled automations** (cron prompts against a repo or workspace, prechecks, reuse session) | docs `cli/automations`; `src/main/automations/` | **Conflicts** | – | L | Non-goal. |
| P7 | **Computer use** (`orca computer`: accessibility tree, click, type, screenshots) | docs `cli/computer-use`; `src/main/computer/` | **Conflicts** | – | L | Non-goal. |
| P8 | **Embedded per-worktree browser** (Chromium), **Design Mode** (click an element to send its DOM, CSS and screenshot to the agent), profiles and cookie import, viewport emulation, browser automation CLI | docs `browser/overview`, `browser/design-mode`, `browser/profiles`; `src/main/browser/`, `src/renderer/src/components/browser-pane/` | **Conflicts** | – | L | Non-goal; needs a webview. |
| P9 | iOS Simulator and Android emulator panes, driven by agents | docs `cli/overview` "Mobile emulator", `cli/skills`; `src/main/emulator/`, `src/renderer/src/components/emulator-pane/` | **Conflicts** | – | L | Same territory as computer use and the browser. |
| P10 | **Artifacts**: publish Markdown or HTML as public links through an Orca account | docs `editing/markdown` "Share as artifact", `settings` "Artifacts"; `src/main/artifacts/` | **Conflicts** | – | M | Account and cloud. |
| P11 | **Mobile companion** (iOS/Android) and relay | docs `mobile`; `mobile/`, `cloud/` | **Conflicts** | – | L | Non-goal. |
| P12 | **SSH worktrees**, Remote Orca Server, headless `orca serve`, web client | docs `ssh`, `remote-servers`; `src/main/ssh/`, `src/main/server/`, `src/main/orcad/` | **Conflicts** | – | L | Non-goal. |
| P13 | **Cloud VM / per-workspace environment recipes** (cloud sandboxes, VMs, local Docker) | docs `ways-to-run`, `model/orca-yaml` (`environmentRecipes`); `src/main/ephemeral-vm/` | **Conflicts** | – | L | Cloud. A local-Docker-only variant would be "new milestone". |
| P14 | **Plugin system and git marketplaces** (experimental) | docs `settings` "Plugins"; `src/main/plugins/`, `src/renderer/src/components/plugin-catalog/` | **Conflicts** | – | L | Non-goal (extension marketplace). |
| P15 | Anonymous telemetry (opt-out), crash reporting, "star the repo" nag | docs `telemetry`; `src/main/telemetry/`, `src/main/crash-reporting/`, `src/main/star-nag/` | **Conflicts** | – | – | Local-only, no telemetry. |
| P16 | Auto-updater with RC, perf and local-build channels | docs `settings` (General, Updates); `src/main/updater/` | **Conflicts** | – | M | No auto-update in v1. |
| P17 | Windows, Linux and WSL builds | docs `terminal`, `install`; `src/main/wsl/`, `src/wsl-guest/` | **Conflicts** | – | L | Non-goal (keep code portable). |

---

## 5. Counts

125 rows in all:

| Verdict | Rows |
|---|---|
| **Adopt** | 55 |
| **Adopt, adapted** | 48 |
| **Conflicts** | 22 |

The 22 Conflicts rows are W19, W20, G14, R11, R12, N3, N5, A6, E4, E10, and P6 to P17. Four Adopt-adapted rows contain a part that conflicts and is noted in the row: G19 cloud speech, T10 port forwarding, P4 skill sharing, and P13's local Docker idea.

## 6. Top 15 to take, ranked for an agent-watching editor

1. **R2 Annotate the agent's diff and send the notes back as one batch** (M4, M). This turns Review into steering, which is Qur's core loop.
2. **W5 and W6: shared gitignored dirs (APFS clone or symlink) and `.worktreeinclude` for `.env`** (M2, M). Without them every new task worktree is missing deps and secrets, and agents waste turns reinstalling.
3. **W7 Setup script with "wait for setup" before the agent starts** (M2, M).
4. **G10 Workspace-trust pre-seeding** (M2/M5, S). Every Qur task is a new folder outside the repo, so the CLI trust prompt would otherwise stall terminal agents.
5. **E2 Never save over unseen agent edits; Discard pauses autosave** (M3, S). User and agent share files.
6. **G4 and G5: Needs-You-first ordering, unread-done state and a last-message preview on each agent row** (M2/M3, S).
7. **A2 Inbox with jump-to and "mark unread"**, plus A3 and A4 tuning and muting (M6, S/M).
8. **N1 Agent jump palette**: recent chats ranked needs-you first, frozen order, `Cmd-1..6` (M6, M).
9. **G3 Reliable status for terminal-mode agents** through OSC title and an opt-in local hook (M5, M).
10. **C2, C3, C5, C7, C10: chat composer essentials**: inline question cards, model, effort and mode pills, `/` commands and skills, queued messages, show full output (M2, S each).
11. **R15 and R10: conflict prediction with `merge-tree`, plus a `+n −m` chip** on each agent row before Merge (M4, S).
12. **R7 and R8: "Resolve or Fix with AI" and a commit message generated through the user's own agent CLI** (M4, S/M).
13. **G11 Session history with resume** (new milestone, M). Local transcript scan; resume through `session/load` or `--resume`.
14. **G12 and G13: hibernate idle finished agents and restore tasks on relaunch** (M7, M). Directly serves the idle-memory budget.
15. **G15 Local usage and rate-limit readout with reset times** (M7, M). Subscription users hit limits; read local files only.

Close runners-up: T11 per-agent memory/CPU readout, R3 AI-attribution tint, T5 Quick Commands and prompt presets, T4 floating terminal, W3 background worktree creation.

## 7. What Qur does that Orca does not

These are based on a search of Orca's repo and docs. "Not found" means a search for it came up empty, not proof that it is absent.

- **Native and light.** Orca is Electron 43 with React, Monaco and xterm.js, and its macOS dmg is 254 MB. Qur targets under 40 MB and under 80 MB idle, with no JS or webviews.
- **Live file-tree heatmap** of what each agent reads and edits, with per-agent accent glows and heat levels. No equivalent was found. Orca's file explorer just reflects disk changes, and the only "heatmap" in its source is a token-activity chart in Stats (`src/renderer/src/components/stats/usage-overview-sections.tsx`).
- **Live "follow the agent's cursor" view with Take over.** Not found in Orca.
- **Overlap dots between agents** on the main checkout tree, before merging. Not found.
- **ACP-first structured chat as the default for Claude**, with plan/To-do, permission bar and Stop. Orca's chat UI is experimental and the terminal is its default. Its Claude chat uses the Claude Agent SDK stream; ACP is used for Grok, OpenCode, Antigravity and OMP.
- **Safe-by-default permissions.** Qur shows an inline permission bar that names and outlines the file. Orca launches agents with bypass flags (`--dangerously-skip-permissions` and similar) by default.
- **A local Merge button** that merges the task branch into the user's current branch with `merge-tree`, `commit-tree` and `read-tree`, without touching the user's index. Orca's ship flow is commit, push, then a hosted PR or MR; it uses `merge-tree` only to predict PR conflicts. No local merge-into-my-branch action was found.
- **No telemetry, accounts or background daemon**, and no orphaned processes after quit. Orca has opt-out telemetry, an optional account (Artifacts, skill sharing, relay) and a PTY daemon that outlives the app.
- **Optional Vim mode.** None found in Orca (no `monaco-vim` or vim-mode references).
