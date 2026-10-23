# OpenADE and PandaCode feature inventory, mapped onto Qur

Research date: 2026-10-23. Subjects: [bearlyai/openade](https://github.com/bearlyai/openade) and [aicodeking/pandacode](https://github.com/aicodeking/pandacode). Shallow clones of `main` (in `research/openade/repo` and `research/pandacode/repo` next to this file). I only read the files. Nothing was installed, built or run.

Paths are relative to each repo's root. Qur's status comes from `README.md`, `HANDOFF.md`, `docs/decisions.md`, `docs/menus.md` and a grep of `crates/`. Where a feature is already a row in `docs/research/orca.md`, the row cites that ID (for example "= Orca C3"), so we don't count the same idea twice.

## How to read the tables

- **Qur:** **Has** means it's built on `main` or `m4-wip`. **Missing** means it isn't built. "Missing (Orca X)" means it isn't built but is already listed in the Orca inventory.
- **Verdict:**
  - **Adopt:** fits Qur as is.
  - **Adopt, adapted:** fits once changed to stay light, native and local.
  - **Conflicts:** breaks a stated constraint, with the reason given.
- **Effort:** S is a few days or less, M is about a week or two, L is more.

---

## 1. OpenADE

### 1.1 What it is

- **Pitch:** "Agentic Development Environment for Devs Who Don't Ship Slop." The core workflow is **Plan → Revise → Execute**: you have the agent write a plan, comment on it inline, then run it. Its declared anti-feature is that **you can't edit code** in the app.
- **Stack:** Electron, React, MobX, Yjs persistence, xterm.js and `@pierre/diffs`. It's a monorepo of 12 TypeScript packages (`projects/web`, `electron`, `harness`, `runtime*`, `openade-module`, `mobile`, `landing`). Agents are driven by `projects/harness`, which spawns the `claude` CLI (`-p`, stream-json, `--resume`) and Codex. That means it uses the CLI's own login, despite the README's "your own Claude Code or Codex API access" wording.
- **Maturity:**
  - About 390 stars. Created 2026-02, last push 2026-07-24 (v0.81.2). No activity for about 3 months.
  - Windows is "experimental". There is an iOS companion app (Capacitor) with a pairing and remote-control API.
- **License:**
  - The README badge and footer say **MIT © Bearly AI**, but the repo has **no LICENSE file**, and GitHub reports no license.
  - Strictly speaking, that makes code reuse legally unclear. Take ideas only, unless they add a LICENSE file.
- **Defaults that differ from Qur:**
  - Optional anonymous telemetry plus Sentry (`web/src/analytics/`, `electron/src/modules/sentry.ts`).
  - A settings modal.
  - MCP connectors with OAuth to Linear, Notion, Stripe and others.
  - Push and PR creation.
- **Honest take:** much of it overlaps with Qur (worktrees, setup script, diff comments sent back to the agent, terminal, notifications, themes). The new and useful parts are the **turn modes** (Plan, Ask, Revise, Run plan), the **second-opinion Review**, **HyperPlan**, **per-turn patch snapshots**, **Repeat** and the **processes file**. The prompt templates in `projects/openade-module/src/review.ts` and `promptBuilder.ts` are well thought out.

### 1.2 Features

| ID | Feature | What it does (from source) | Qur | Verdict | Effort | Notes for Qur |
|---|---|---|---|---|---|---|
| OA1 | **Turn modes: Plan / Ask / Revise / Run plan / Do** | Each action is a turn type (`openade-module/src/types.ts`). It gets its own system block (`promptBuilder.ts`: `<current_operating_mode mode="plan">`, plus a reminder in the user message). Plan, Ask and Revise run **read-only**. | Missing (Orca C3 covers mode pills only) | Adopt, adapted | M | Use the ACP adapter's own session modes (`session/set_mode`; claude-agent-acp advertises default, acceptEdits, plan and bypassPermissions) instead of prompt text. `qur_acp` already parses `current_mode_update` but never sends `set_mode`. |
| OA2 | **Read-only enforcement for terminal Claude** | The harness maps "read-only" to `--permission-mode dontAsk`, plus an `--allowedTools` whitelist of read tools and read-only Bash patterns, plus `--disallowed-tools Edit,Write,NotebookEdit` (`harness/src/harnesses/claude-code/args.ts`). They avoid `plan` mode because it injects plan prompts. | Missing | Adopt | S | A useful recipe for read-only Agent Terminals (reviewers, Ask). These are CLI flags only, with no API key. |
| OA3 | **Comment on the plan, then Revise** | Pending comments attach to plan lines, agent text, diffs, patches, files and bash output (`web/src/types.ts` comment sources: `plan`, `llm_output`, `assistant_text`, `bash_output`, `patch`, ...). The next action includes them, and they become read-only once "consumed" (`includesCommentIds`). | Partial: Qur has diff line comments (M4) | Adopt, adapted | M | Reuse the `changes_view.rs` comment model on the plan text and on chat messages. "Consumed" marking avoids sending notes twice. |
| OA4 | **Second-opinion Review** | "Review" starts an independent **read-only** run with a picked harness and model, on the plan or on recent work. Findings use `Location, Issue, Criticality: N/10, Suggestion`, with a "things that might be intentional" section. The result is then **handed back** to the main agent, which must answer each finding with Agree/Disagree, Why and Fix, then ask before proceeding (`openade-module/src/review.ts`). | Missing | **Adopt, adapted** | S–M | Strong fit: it turns Review into steering. Same vendor only (no Codex), so the "second opinion" is a different Claude model. See shortlist #1. |
| OA5 | **HyperPlan** | DAG strategies over plan, review, reconcile and revise steps: Standard, Peer Review, Ensemble (N planners + reconciler), Cross-Review (`web/src/hyperplan/strategies.ts`, `openade-module/src/hyperplan.ts`). Sub-runs persist, Stop cancels all of them, and context is capped at 240 KB. | Missing | Adopt, adapted | M–L | Doable as several read-only ACP sessions in one worktree. It multiplies subscription usage, so make it opt-in and later. |
| OA6 | **Per-turn patch snapshots** | After each plan/do/ask turn, the worktree diff against the turn's starting commit is saved as `~/.openade/data/snapshots/{id}.patch` plus an index, and shown collapsed in the timeline. Opening it loads only the selected file's slice, and you can comment on it. | Missing (Orca C12) | Adopt, adapted | M | The README says "rollback to any point with a click", but **I found no rollback code**: snapshots are view-only. Qur can do per-turn trees plus Rewind with plain git. See shortlist #2. |
| OA7 | **Repeat** | Sends the same prompt over and over, with an optional **stop-on-text** match and a max-runs cap (default 100) (`web/src/store/managers/InputManager.ts`). | Missing | Adopt, adapted | S | Handy for "keep fixing until tests pass". Cap it low (for example 10) to protect the subscription. Agent menu: "Repeat Prompt…". |
| OA8 | **Processes file** (`openade.toml` `[[process]]`) | Named processes with `type = setup / daemon / task / check`, a `command`, a `work_dir` and an optional `url`. A Processes tray starts and stops them, with start-all/stop-all (`electron/src/modules/code/procs/parse.ts`). It lives in the repo. | Partial: Qur has a per-project setup command | Adopt, adapted | S | Keep it in `projects/<id>.toml`, never in the repo. Pair `url` with Qur's Browser. See shortlist #5. |
| OA9 | **Cron jobs** | `[[cron]]` schedule, prompt and isolation; fires with catch-up; can reuse a thread. | Missing | **Conflicts** | – | = Orca P6 (scheduled automations are a non-goal). |
| OA10 | Queue follow-ups, **interrupt and send a replacement now** | Queued Do/Ask turns show in the input bar and can be cancelled. "Interrupt" sends a replacement immediately. | Has the queue; interrupt-and-send is missing | Adopt | S | "Send Now" (`Cmd+Shift+Enter`) = ACP `session/cancel`, then send. |
| OA11 | **Export the thread as Markdown** | Copy or download, with toggles for tool calls, results and thinking. | Missing | Adopt | S | Palette: "Copy Chat as Markdown". |
| OA12 | Task thread serializer | Turns the task into XML for handoffs (reviewer and sub-planner context), capped by a byte budget. | Missing | Adopt | S | Needed by OA4 and OA5. |
| OA13 | Clickable `path:line` in agent output | Paths with fuzzy and line-numbered matching open the file viewer. | Missing | Adopt | S | Open in the code modal at that line. The prompt also tells the agent how to write links. |
| OA14 | Frecency file favourites | Quick Open ranks recently and often used files first, and prunes stale entries. | Missing | Adopt | S | `Cmd+P` ranking; store it in `state.toml`. |
| OA15 | Git Log tray | Commit history with per-commit diffs. | Missing | Adopt, adapted | M | A "Log" segment in the Git panel. |
| OA16 | Global env vars for every subprocess | Set in Settings and merged into PTYs, git and processes. | Missing | Adopt, adapted | S | `[env]` in `config.toml`. Never allow `ANTHROPIC_API_KEY` there (refuse that key). |
| OA17 | Shortcut hints while Cmd is held | Shortcut badges appear on actions. | Missing | Adopt, adapted | S | Ephemeral, so not clutter. Low priority. |
| OA18 | Stash prompt drafts | Drafts (with mentions and images) move to a side rail and come back later. | Missing | Adopt, adapted | S | Simpler for Qur: keep an unsent draft per task across switches and restarts. |
| OA19 | Scratchpad tray | Quick notes per workspace. | Missing | Conflicts | – | New UI surface with no clear tie to agents (the no-clutter rule). |
| OA20 | MCP connectors (Linear, Notion, Stripe, ...) with OAuth, plus per-task selection | Presets with remote MCP URLs. | Missing | **Conflicts** | – | Network, accounts and settings UI. Agents already load their own MCP config. |
| OA21 | Usage stats, cost and share cards | Throughput, runtime and cost per period, plus an image card. | Missing (Orca G15/G17) | Adopt, adapted | M | Local only; drop the share card. |
| OA22 | Commit & Push, PR with a generated title | | Missing | Conflicts (push) | – | = Orca R8/R11/R12. |
| OA23 | Mobile companion, remote-control API | | Missing | **Conflicts** | – | = Orca P11. |
| OA24 | Telemetry and Sentry | | – | **Conflicts** | – | Qur: no telemetry. |
| OA25 | Fast mode and thinking-level picker | | Missing (Orca C3) | Adopt | S | Through ACP config options when the adapter offers them. |
| OA26 | Image attachments (paste, drop, resize to 1568 px) | | Partial: Browser picks send PNG blocks | Adopt | S | = Orca C6. The 1568 px / 1.15 MP resize constraint is a useful number to reuse. |
| OA27 | "No code editing" anti-feature | | n/a | Conflicts | – | Qur keeps a real editor (code modal, Take over). |

---

## 2. PandaCode

### 2.1 What it is

- **Pitch:** "A small, native desktop workspace for coding agents." It runs about 15 coding CLIs side by side: Claude Code, Codex, Kimi, OpenCode, Copilot CLI, Grok, Pi, Muse and more, through each one's native protocol, ACP, Pi RPC or MSP.
- **Stack:**
  - Tauri 2 (system webview), Rust core, SQLite (`rusqlite` bundled) and a vanilla-TypeScript renderer.
  - Rust deps include `tokio` full, `reqwest`, `tokio-tungstenite`, `rustls`, x25519 and chacha20 (for the remote relay), `portable-pty` and the Tauri updater.
  - About 4.6 MB of Rust source and 3 MB of TS.
- **Maturity:**
  - Repo **created 2026-10-18** (5 days ago), about 560 stars, at v0.3.31 with 3 releases today. A very fast cadence.
  - The surface is huge for its age, which suggests it is largely agent-written.
  - Several docs openly say a feature **isn't wired in yet**: `docs/recipes.md` and `docs/jj.md` have a "Wiring: these files are outside the worker's scope. Apply them to turn the feature on" section. Treat docs as intent, not proof.
- **License:** the "PandaCode License" = **MIT plus a fork-attribution clause**. Any fork that is released publicly must say "This is a fork of PandaCode" in its README *and* in a visible place such as the About screen.
  - That is an extra condition. GPL-3.0 §7(b) may allow attribution requirements, but "a fork must state…" in the UI is murky, and Qur wouldn't want it anyway.
  - **Take ideas, not code.**
- **Defaults that differ from Qur:**
  - New tasks start in **Full access** (bypass permissions) by default (`docs/approvals.md`).
  - Auto-update is on.
  - Background version checks run every 6 h against brew, claude.ai, GitHub and kimi.
  - It fetches a daily LiteLLM price list.
  - It has a GitHub view, deploys, bots (Telegram, Slack, ...), cloud sandboxes, VMs and simulators.
  - Daily anime wallpapers.
- **Honest take:** it's a "kitchen sink" with many items Qur has ruled out. Still, a handful of small, well-bounded ideas are worth taking: **turn-end checks**, **turn-change attribution**, the **run script → preview** loop, **Approve all**, **find in thread**, **tool-call grouping**, **steer** and **fork/btw**. Its docs are good at stating resource limits, and that's a habit worth copying.

### 2.2 Features

| ID | Feature | What it does (from docs and source) | Qur | Verdict | Effort | Notes for Qur |
|---|---|---|---|---|---|---|
| PC1 | **Turn-end project checks** | Opt-in. After a *clean* turn, the app runs one fast check that the checkout's own manifests declare (the npm `test` script, otherwise `cargo check`). On failure, the output tail goes into the **message queue as an editable repair request** (`src-tauri/src/checks.rs`). | Missing | **Adopt, adapted** | S | Strong fit and fully local. Use an explicit `[checks] command` in `projects/<id>.toml`, plus optional detection. See shortlist #3. |
| PC2 | **Turn changes (attributed per turn)** | Every completed turn shows a file summary, `+n −m` and **Open diff** on immutable saved patches. Ownership comes from a pre-write `pandacode_track_files` handshake tool and the edit events. Bounded: 200 paths, 1 MiB per file, 2 MiB per turn, 128 MiB retention (`docs/turn-changes.md`). | Missing (Orca C12) | Adopt, adapted | M | PandaCode needs the handshake because tasks can share a checkout. Qur's worktrees are isolated, so a git tree at each turn boundary is exact and simpler. See shortlist #2. |
| PC3 | **Run script → preview** | If the checkout has `pandacode-run.sh` (with an optional `# pandacode-preview: http://…` comment), a **Run** button starts it in a managed terminal, waits for the URL and opens the in-app browser. Agents are asked to maintain the script (`docs/run-project.md`). | Missing | Adopt, adapted | S | Combine with OA8. Config lives outside the repo (or read the repo file if present). See shortlist #5. |
| PC4 | **Approvals bar with "Approve all"** | Pending requests wait one line each above the prompt and are grouped, with Approve all when there are several. | Partial: Qur's permission bar shows one request | Adopt | S | "Allow All Pending (n)" in the Agent menu and the bar. Never make it the default. |
| PC5 | **Find in thread** | `Cmd+F` searches user and agent messages with highlights; `Cmd+G` goes to the next match. | Missing | Adopt | S | `Cmd+F` while the ACP stage has focus. Reuse the `qur_editor` find logic on the chat text. |
| PC6 | **Tool-call grouping, "Worked for …"** | Consecutive tool calls fold into groups of up to 5, with counts, categories and running/failed counts. Narration splits the groups, and final answers stay visible. The live preview is capped at 30 entries (`docs/conversation-presentation.md`). | Missing (Qur's `ChatItem::Tool` is one line per call) | Adopt | S | Makes the chat stream much denser and easier to scan. |
| PC7 | **Steer** (`Cmd+Enter`) | Sends a message into the running turn where the harness supports it, otherwise as the next turn of the same session (`src-tauri/src/message_queue.rs`). The queue is durable, reorderable and editable, and survives restarts. | Has the queue (not durable) | Adopt, adapted | S | ACP has no in-turn steer, so "Send Now" = cancel + send (same as OA10). Persist the queue in `tasks.toml`. |
| PC8 | **Fork / side question** (`/fork`, `/btw`) | A new task with the transcript copied up to a chosen message, in the same checkout (`conversation_fork.rs`). | Missing | Adopt, adapted | M | Qur version: "Fork Task" = a new worktree from the task's current branch plus a fresh session seeded with a serialized thread (OA12). "Ask Aside" = a read-only side session (OA2), so the main agent's context stays clean. |
| PC9 | **Task board** (Pending → In progress → Review → Completed; queue a task without starting it; auto-archive 7 days after Completed) | | Missing (= Orca W18/G4) | Adopt, adapted | M | The new idea: **queue a task without starting it**, so you can write the next tasks while agents run. A "pending" agent button. |
| PC10 | **Architect and workers** | One architect delegates to up to N workers on mixed harnesses. Every assignment must declare a write `files` scope, and **overlapping write scopes are serialized**; read-only workers run in parallel. There's a stall watchdog (20 min) and a "Continuing the plan" nudge (`docs/architect-workers.md`). | Missing (= Orca P3/G22) | Adopt, adapted | L | Write-scope locking is an interesting fit with the heatmap, but it's heavy. Defer. |
| PC11 | **Agent usage panel** | Limits and reset times per harness; estimated API cost from local `~/.claude/projects` logs (deduplicated by message and request ID, incremental offsets, prompt text dropped while parsing). For Claude limits it calls **Claude's OAuth usage endpoint with the CLI's local token**. | Missing (= Orca G15) | Adopt, adapted | M | The local log parsing is fine. The OAuth endpoint call and the LiteLLM price fetch are **new network calls** and need the owner's OK; bundle a price table instead. |
| PC12 | Workflow recipes | `.pandacode/recipes/*.json`: steps of `prompt`, `slash`, `check` and `approval`. A dry run marks each step supported, fallback, manual or blocked, and Run queues one message per step (`docs/recipes.md`). | Missing (≈ Orca T5) | Adopt, adapted | S | **Not wired in upstream.** Qur version: prompt presets in `projects/<id>.toml`, run through the queue. Low priority. |
| PC13 | Agent-suggested titles | The working agent proposes a title through an optional metadata tool. Manual renames win. | Missing (≈ Orca W4) | Adopt, adapted | S | ACP gives Qur no tool injection without an MCP server; keep Orca W4's "slug the first prompt". |
| PC14 | Quick overlay | A global shortcut (`Cmd+Shift+Space`) opens a launcher for new and running tasks from any app. | Missing | Adopt, adapted | M | A global hotkey (Carbon `RegisterEventHotKey`, no crate) that opens Qur's palette. Ask first: it's a new system-wide surface. |
| PC15 | Agents exit after each turn; resume by session ID; pre-warm the next Claude process | | Missing (≈ Orca G12) | Adopt, adapted | M | Agent processes aren't in Qur's budget, but freeing them helps the machine. The ACP adapter can `session/load`. |
| PC16 | Context bar (project, Local/Worktree, branch chip); "Worktree missing → Recreate" | | Partial: the header shows the branch | Adopt | S | "Recreate Worktree" for a task whose folder was deleted is a cheap robustness win. |
| PC17 | Projectless chats | A chat with its own folder under `~/PandaCode Chats/`. | Missing | Conflicts | – | Qur is one project per window and agent-in-repo; low value. |
| PC18 | Jujutsu (jj) panel | Status, diff, describe, new and commit. | Missing | Adopt, adapted | M | **Not wired in upstream.** Niche; later. |
| PC19 | Skills and rules library | Writes skills into `~/.claude/skills` or `<checkout>/.claude/skills`, and rule blocks into `CLAUDE.md`, with hash guards. | Missing (≈ Orca P4) | Conflicts (project scope) / Adopt, adapted (user scope) | M | Project scope writes into the user's repo, which Qur never does. |
| PC20 | Token saver (RTK, per-task response-style packs) | Installs `rtk` through Homebrew. | Missing | Conflicts | – | Installs tools and adds prompt packs; outside Qur's job. |
| PC21 | Attachments: PDFs, folders, **mockup sketcher** | | Missing (= Orca C6) | Adopt, adapted | M | The sketcher is a new canvas; skip it. |
| PC22 | Default permission **Full access** | | – | **Conflicts** | – | Qur keeps the permission bar by default. |
| PC23 | Many harnesses (Kimi, GLM, MiMo, DeepSeek, ...), some with **API keys** in the keychain | | Has (any ACP agent through `agents.toml`) | Conflicts (keys, Codex) | – | Qur never stores API keys; no Codex for now. |
| PC24 | Bots (Telegram, Slack, Discord, WhatsApp, web chat) | | – | **Conflicts** | – | Network and remote. |
| PC25 | Cloud agents, cloud sandboxes, local VMs (Tart) | | – | **Conflicts** | – | Cloud non-goal. |
| PC26 | iOS and Android simulators, Deploys, GitHub view, remote access relay | | – | **Conflicts** | – | = Orca P9/R12/P11. |
| PC27 | Agent CLI update checks and one-click updates; app auto-update | Checks every 6 h. | – | **Conflicts** | – | Network; no auto-update in v1. |
| PC28 | 21 anime wallpapers; VS Code theme import | | – | **Conflicts** | – | Weight; user themes are a v1 non-goal. |
| PC29 | Codex `/goal` (persistent objectives with token budgets) | | – | **Conflicts** | – | Codex only. |
| PC30 | Stated resource limits per feature | Every doc lists its caps (bytes, counts, timeouts). | – | Adopt | – | A doc habit: give each new Qur feature a "Limits" line in `docs/budget.md`. |

---

## 3. Counts

| | Adopt | Adopt, adapted | Conflicts |
|---|---|---|---|
| OpenADE (27 rows) | 9 | 11 | 7 |
| PandaCode (30 rows) | 6 | 12 | 12 (PC19 counted as Conflicts) |

About a third of the Adopt rows are already in the Orca inventory (marked "= Orca …"). The genuinely new ideas are OA1–OA8, OA10–OA13, PC1–PC8, PC14 and PC16.

---

## 4. Ranked shortlist for Qur now

### 1. Second-opinion Review with handoff (OA4 + OA2 + OA12)

- **Why:** Qur's loop is "watch, then review, then merge". An independent read-only reviewer on a different Claude model, with scored findings (Criticality N/10) that are then **handed to the task's agent**, catches bugs before Merge at the cost of one extra session. It strengthens the review step that M4 is building, and needs no new surface beyond a row.
- **UI sketch:**
  - **Start:** Agent menu and palette: "Review Agent's Work…" and "Review Agent's Plan…". A small picker asks for the model (from the ACP adapter's model list), with an optional instruction line.
  - **Session:** a second ACP session starts in the **same worktree**, read-only. Mode is `plan` when advertised, and Qur also auto-denies any edit or execute permission request (the permission bar never shows for it). The `review.ts` prompt shape (findings format, sensitivity rules) is the template.
  - **Sidebar:** the reviewer shows as an indented child button under the agent ("↳ review · opus"), with the same fill and unread states. Clicking it shows its stream in the stage.
  - **Handoff:** when it ends, its chat shows **Send Findings to Agent**. This puts the handoff prompt (Agree/Disagree, Why, Fix per finding, then "ask before proceeding") into the main agent's prompt, editable and queued if the agent is busy.
- **Touches:**
  - `qur_app/src/agent_view.rs`: a reviewer session as a child of the task, and auto-deny.
  - `qur_app/src/workspace.rs`: child row in AGENTS.
  - `qur_app/src/commands.rs`: menu and palette items.
  - `qur_acp`: `session/set_mode` and the model list from `session/new`.
  - `qur_agents`: store the reviewer link in `tasks.toml`.
  - A new small module that serializes the thread (OA12) under a byte cap.
- **Size:** M (S for a first cut that reviews `git diff` only, with no thread context).
- **Constraints:**
  - Doubles usage for that task. Run it only on request; the automated test must use a tiny prompt (decision 20).
  - Same vendor only (no Codex), so the "second opinion" is another Claude model.
  - Read-only is enforced by Qur's deny plus the mode, not by an OS sandbox. Say so in the UI copy.

### 2. Turn checkpoints: per-turn changes plus Rewind (OA6 + PC2, = Orca C12)

- **Why:**
  - Today Qur shows the whole task diff. Seeing **what this turn changed** answers "wait, what did it just do?".
  - **Rewind to before this turn** is the safety net both projects lack: OpenADE advertises rollback, but its snapshots are view-only.
  - Because Qur's worktrees are isolated, plain git trees are exact, with no attribution handshake.
- **UI sketch:**
  - **Snapshots:** at each ACP prompt send and at `end_turn`, `qur_git` writes a tree of the worktree through a temp index (the same technique `task_diff` uses) and pins it under `refs/qur/turns/<task>/<n>`, so gc can't prune it.
  - **Chat line:** each finished turn ends with one compact line: "3 files · +12 −4 · View · Rewind".
  - **View:** opens the Changes tab filtered to that turn (`diff-tree start end`), and you can comment as usual.
  - **Rewind:** asks for confirmation, stops the agent if it's running, then does `read-tree -u --reset <start tree>` in the worktree (worktree only, never the user's checkout), and adds a Notice to the chat for the agent's next turn.
  - **Heatmap:** can optionally tint only "this turn's" files.
  - **Menu:** Agent menu: "Show Last Turn's Changes", "Rewind Last Turn…".
- **Touches:**
  - `qur_git`: new `turns.rs` with `snapshot_tree`, `tree_diff` and `restore_tree`, plus tests that the user's index and stash stay untouched.
  - `qur_agents`: turn list in `tasks.toml`.
  - `qur_app/src/agent_view.rs` and `qur_app/src/changes_view.rs`: turn filter.
  - `qur_app/src/commands.rs`.
- **Size:** M.
- **Constraints:**
  - Agent Terminals have no turn boundaries. Start with ACP only, and maybe use the "idle for 2 s" signal later.
  - Rewinding files the user changed by hand in the worktree: warn first.
  - The extra refs must be deleted on Discard or Merge cleanup.

### 3. Turn-end checks (PC1)

- **Why:** a cheap, local, high-value loop. After an agent turn that changed files, Qur runs the project's check (for example `cargo check` or `pnpm test --run`) in the worktree. A failure comes back to the agent as an **editable** repair message. That cuts the "it said done but it doesn't build" round trips, and it works with any agent.
- **UI sketch:**
  - **Config:** `[checks] command = "cargo check"` (plus an optional `timeout = 120`) in `projects/<id>.toml`, next to `[worktree] setup`. This is opt-in; manifest detection could be offered by the first-run picker.
  - **Run:** it runs through the login shell, like setup. Output streams as a `ChatItem::Output` block, collapsed after it finishes.
  - **Result:** the sidebar button gets the existing `✕` marker on failure and `✓` on a pass.
  - **Failure:** the prompt bar is prefilled with "Checks failed (`cargo check`):" plus the last ~4 KB of output. It's not auto-sent unless `auto_send = true`.
  - **Menu:** Agent menu: "Run Checks Now".
- **Touches:**
  - `qur_config`: `ProjectSettings`.
  - `qur_worktree`: reuse the setup-command runner.
  - `qur_app/src/agent_view.rs`: turn-end hook and prefill.
  - `qur_app/src/workspace.rs`: marker.
  - `qur_app/src/commands.rs`.
- **Size:** S.
- **Constraints:**
  - CPU and time spikes from the check process; it runs outside Qur's own footprint.
  - Only run it after turns that changed files, and never while another check for the same task is running.

### 4. Plan first, comment, then Run (OA1 + OA3, extends Orca C3)

- **Why:** OpenADE's main thesis, and it maps directly onto ACP session modes that Claude's adapter already advertises. Qur already has line comments on diffs. Commenting on a plan is the same steering idea, applied *before* any code is written. That's cheaper than reviewing a wrong diff.
- **UI sketch:**
  - **Mode menu:** Agent menu: "Mode ▸ Ask Before Edits · Accept Edits · Plan" (a radio group). `Shift+Tab` cycles it, as in the Claude CLI. The current mode shows as one word in the stage header; nothing else is added. Bypass is not offered, or sits behind a confirmation, to keep Qur's safe default.
  - **Plan document:** when a plan-mode turn ends with a plan (ExitPlanMode tool content or the final message), the stage shows it as a document in the Chat or To-do area, with the same `+` line comments as `changes_view.rs`.
  - **Actions:** two actions in the prompt bar and the Agent menu. **Revise** sends the pending comments (marked as sent afterwards). **Run Plan** switches the mode to default or accept-edits and sends "Implement the approved plan".
- **Touches:**
  - `qur_acp`: `set_mode` request, and the modes from `session/new`, which are already partly in `types.rs`.
  - `qur_app/src/agent_view.rs`.
  - A comment component shared with `changes_view.rs`.
  - `qur_app/src/commands.rs` and `docs/menus.md`.
- **Size:** M.
- **Constraints:**
  - Verify that claude-agent-acp@0.94.0 advertises the modes and how it surfaces ExitPlanMode.
  - Agent Terminals get the Claude CLI's own `Shift+Tab`, so nothing is needed there.

### 5. Run Project in the task's worktree, then Browser (OA8 + PC3)

- **Why:** Qur already has Browser + Inspect per agent, but you have to start the dev server for each worktree by hand. One command that starts it in the agent's worktree and opens the Browser on its URL closes the loop: watch, then inspect, then ask the agent.
- **UI sketch:**
  - **Config:** `[run] command = "pnpm dev --port $PORT"`, `url = "http://localhost:$PORT"` in `projects/<id>.toml`. Qur picks a free `PORT` per task, so parallel worktrees don't collide.
  - **Start:** Agent menu and palette: "Run Project in Worktree". This creates a shell session under TERMINALS named "run · <task>", polls the URL for up to about 10 s, then shows the agent's Browser (`Cmd+5`) on it.
  - **Repeat runs:** running it again reuses the live session.
  - **Cleanup:** Discard or Merge cleanup stops it (process group kill).
  - Optionally, read a repo `pandacode-run.sh` or `openade.toml` `[[process]]` with `url` as a fallback. Qur only reads it, and never runs anything without the user's command.
- **Touches:**
  - `qur_config`: `ProjectSettings`.
  - `qur_app/src/terminal.rs`: a named session with env.
  - `qur_app/src/browser_view.rs`: open URL.
  - `qur_app/src/workspace.rs` and `qur_app/src/commands.rs`.
- **Size:** S.
- **Constraints:** localhost only, so no new outbound network. Dev servers cost memory outside Qur, so stop them when the task goes away.

### 6. Chat-stream polish batch (PC4, PC5, PC6, OA10/PC7, OA11)

- **Why:** five small things that make the ACP stage feel finished, each with no new surface:
  - **Find in chat:** `Cmd+F` while the stage has focus.
  - **Grouped tool calls:** "Worked for 2m · 14 tools ▸", expandable.
  - **Allow All Pending (n):** in the permission bar when several requests are waiting.
  - **Send Now:** `Cmd+Shift+Enter` cancels the turn and sends.
  - **Copy Chat as Markdown:** in the palette.
- **Touches:**
  - `qur_app/src/agent_view.rs`: `ChatItem::Tool` grouping and find.
  - `qur_app/src/prompt.rs`: Send Now.
  - `qur_app/src/commands.rs` and `docs/keymap.md`.
- **Size:** S each, M for the batch.
- **Constraints:** none. Allow All must stay explicit and per task, never sticky.

### Runners-up

- OA7 **Repeat until** (stop-on-text, low cap).
- PC8 **Fork Task / Ask Aside**.
- PC9 **queue a task without starting it** (a "pending" agent button).
- PC16 **Recreate a missing worktree**.
- OA13 clickable `path:line` in chat.
- OA5 **HyperPlan** (later; it multiplies usage).
