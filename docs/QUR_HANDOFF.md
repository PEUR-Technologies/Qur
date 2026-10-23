# Qur: Handoff to Claude Code

**What:** Qur, a lightweight agentic code editor for macOS, written in Rust. A Zed / VS Code replacement built around watching and steering AI coding agents.
**Owner:** Yobi (Tracy Angelo Posadas), PEUR Technologies
**License:** GPL-3.0-or-later (open source; this lets us reuse Zed's GPL crates where useful)
**Platform (v1):** macOS only. Keep code portable so Linux can follow.
**Brand:** see `brand/` next to this file and section 12.
**Date:** 2026-10-23

---

## 0. Rules for you (Claude Code)

1. Read this whole file and `brand/README.md` before writing code.
2. Build milestone by milestone (section 14). Do not start one until the previous one passes its acceptance criteria. Commit at the end of each.
3. **Lightness is the product.** Every dependency, crate and feature must justify its weight. Measure (section 4), never estimate.
4. **No clutter.** If a UI element is not described here, do not add it. Ask the owner first.
5. Ask the owner before: adding a heavyweight crate, adding a network call that is not an agent talking to its own vendor, or changing any decision in section 2.
6. ACP, adapter package names, and GPUI APIs move fast. Verify against live sources before relying on anything written here:
   - ACP spec: https://agentclientprotocol.com
   - Zed source (reference ACP client, GPUI, terminal): https://github.com/zed-industries/zed
   - Peers for UX ideas only: https://github.com/stablyai/orca , https://github.com/superset-sh/superset , https://superset.sh , https://delta.dev

---

## 1. The idea in one paragraph

Qur is a code editor where **agents are first-class, not a side panel.** The left sidebar lists your agents instead of your files. Click an agent and you **peek** at what it is doing right now: the file it is typing into (live), its to-do list, its chat, and its changes. Every agent works in its own git worktree, and the file tree acts as a **live heatmap**: files glow in the agent's accent color as it reads and edits them. When an agent is done, you review its changes and press **Merge**. Underneath, it is still a real editor with syntax highlighting, VS Code keybindings and optional Vim mode, so you never need to leave it. Everything is local, uses your existing Claude and Codex subscriptions, and stays small and fast.

**Tagline direction (brand story):** the Qur mark is a finder eye, the one shape every scanner looks for. In Qur, it is the eye you keep on your agents: *"See what your agents see."* Write product copy in that spirit. (The brand kit was made for an earlier QR-code project; replace any QR-specific copy in it.)

---

## 2. Decisions already made (do not change without asking)

| Area | Decision |
|---|---|
| Language | Rust |
| UI toolkit | **Whichever is lightest** that still renders code text well. Decide by spike in M0 (section 5). |
| Editing depth | Real editing + tree-sitter syntax highlighting + find/replace. **No LSP in v1.** |
| Agents | Claude Code, Codex CLI, any ACP agent via config, several at once (including several of the same vendor). |
| Agent link | **Both:** ACP (structured) by default, raw terminal (PTY) fallback for agents without ACP. |
| Isolation | **Always worktrees.** Every agent task gets its own git worktree and branch. |
| Merge | Review the agent's changes, then a **Merge** button merges its branch into yours. Conflicts open in the editor. |
| Sidebar | **Agents on top. File tree below as a collapsed dropdown** (one click to open). No clutter. |
| Peek layout | **User picks** between three layouts (section 8). Default chosen in the first-run picker. |
| Visualizer | **Live file-tree heatmap** (section 9). |
| Panels | Integrated terminal, Git (stage/commit), project search. All optional, chosen in the **first-run picker**. |
| Keys | VS Code keybindings by default; optional Vim mode. |
| Themes | **Multiple themes from day one** (section 12a): Qur Light / Qur Dark (default, brand black and white), popular ports, and high contrast. A theme changes the whole UI and code. Pick in the first-run picker or with `Cmd+K Cmd+T`. "Auto" follows macOS light/dark using Qur Light / Qur Dark. |
| Agent colors | Each agent gets one accent color (dot, glow, cursor), defined per theme so they stay readable. In the Qur themes the rest of the UI is monochrome. The logo is never recolored. |
| Alerts | macOS notification + Dock badge + soft sound when an agent finishes or needs approval. |
| Billing | Agents run on the owner's **existing CLI subscriptions**. Qur never asks for or stores API keys. |
| Network | Local only. The only outbound traffic is each agent reaching its own vendor. No telemetry, no accounts, no auto-update in v1. |

---

## 3. Non-goals for v1

- LSP (autocomplete, go-to-definition, diagnostics). Plan the editor so LSP can be added later, but do not build it.
- Extensions/plugin marketplace, user-made or imported themes (the theme format should make this easy later, but v1 ships only the built-in set), settings UI beyond the first-run picker and the theme picker.
- Collaboration, multiplayer, cloud, mobile companion, remote/SSH workspaces.
- Embedded browser, design mode, computer use, GitHub/Linear integrations, scheduled automations (Orca/Superset have these; we do not).
- Windows and Linux builds (keep code portable, do not ship).
- Electron, webviews, or any JavaScript in the app itself.

---

## 4. Weight budget (targets, measured in `scripts/budget.sh`)

| Metric | Target |
|---|---|
| Release app bundle (stripped, excluding bundled grammars) | under 40 MB |
| Idle RSS, one small project open, no agents | under 80 MB |
| RSS with 3 agents active (Qur process only, not agent processes) | under 150 MB |
| Cold start to first interactive frame | under 250 ms |
| Keystroke to pixel in the editor | one frame at 120 Hz |
| Agent event to heatmap glow on screen | under 100 ms |
| Open a 10,000-line file | under 100 ms |

Run the budget script at the end of every milestone and record results in `docs/budget.md`. If a target looks unreachable, tell the owner with numbers instead of quietly relaxing it.

---

## 5. Toolkit choice (M0 spike)

The owner wants "whichever is lightest". Build the **same tiny prototype** in two candidates, then pick by measurement:

- **GPUI** (Zed's GPU UI framework; check its crate license, believed Apache-2.0, and its current packaging/version). Proven for code editors; lets us reuse Zed crates (rope, text, tree-sitter integration, `alacritty_terminal` usage) under our GPL license. Also look at `gpui-component` (third party) for a ready tree view and code editor component, but only if it does not bloat the build.
- **One lighter alternative**: `iced` or `egui`. Smaller, but check text shaping, ligatures, crisp Retina rendering, and large-file scrolling.

Prototype: a window with a sidebar list, a tree view of a 5,000-file repo, and an editable 10,000-line TypeScript file with tree-sitter highlighting.

Measure for each: release binary size, idle RSS, cold start, scroll smoothness on the 10k-line file, typing latency, text quality on Retina. **Pick the lightest one that does not compromise code text quality or typing latency.** Report the table to the owner before M1.

---

## 6. Architecture

Cargo workspace:

```
qur/
  crates/
    qur_app/        # window, layout, sidebar, peek, first-run picker, keymaps
    qur_editor/     # buffer (rope), cursor/selection, undo, find/replace, vim mode
    qur_syntax/     # tree-sitter highlighting, bundled grammars
    qur_tree/       # file tree model + heatmap state
    qur_agents/     # roster, lifecycle, ACP client, PTY fallback
    qur_acp/        # ACP JSON-RPC over stdio (official Rust crate if light, else hand-rolled)
    qur_worktree/   # create/remove worktrees, branches, merge
    qur_git/        # status, diff, stage, commit (git CLI or gix, pick lighter)
    qur_term/       # PTY + terminal emulation (alacritty_terminal)
    qur_search/     # project search (ripgrep crates: grep-searcher, ignore)
    qur_notify/     # macOS notifications, dock badge, sound
    qur_config/     # config files, first-run state
  assets/brand/     # copied from the handoff brand/ folder
  scripts/budget.sh
  docs/
```

Rules:
- Async runtime: use whatever the chosen UI toolkit already uses; do not add a second one.
- File watching: `notify` crate (FSEvents on macOS), debounced at 50 ms per path.
- Persisted state (agents, tasks, layout choice): small TOML/JSON files under `~/Library/Application Support/Qur/`. No database.
- Nothing is written into the user's repo except what agents write and the worktrees Qur creates (which live **outside** the repo, see section 10).

---

## 7. Window layout

```
+--------------------+-------------------------------------------+
| [Qur symbol]  kliper v               (project switcher)        |
+--------------------+-------------------------------------------+
| AGENTS          +  |                                           |
|  * claude-1   ~~   |          PEEK AREA / EDITOR               |
|    fix navbar      |                                           |
|  * codex-1    ok   |   (one of three layouts, section 8)       |
|    add tests       |                                           |
|  * claude-2   !    |                                           |
|    needs approval  |                                           |
|                    |                                           |
| > Files            |                                           |
|   (collapsed)      |                                           |
+--------------------+-------------------------------------------+
| > ask claude-1...                                     Stop     |
+----------------------------------------------------------------+
|  [optional bottom panel: Terminal | Git | Search]              |
+----------------------------------------------------------------+
```

- **Agents list:** one row per agent task: accent dot, name, one-line task title, status glyph (working, done, needs you, error). `+` starts a new agent task (pick agent, type the task; Qur creates its worktree).
- **Files dropdown:** collapsed by default. When open, it shows the tree of whatever is selected: the main checkout if no agent is selected, or **the selected agent's worktree**. This tree is the heatmap (section 9).
- **Prompt bar:** always at the bottom, targets the selected agent. Enter sends, Shift+Enter newline, Esc or Stop cancels. Supports `@file` mentions (fuzzy) and dragging files from the tree.
- **Bottom panel:** only the panels the user enabled at first run. Toggle with VS Code shortcuts (`Ctrl+\`` terminal, `Cmd+Shift+F` search, `Ctrl+Shift+G` git).
- **No file tabs.** Opening a file replaces what is in the editor area; `Cmd+P` quick-open and `Ctrl+Tab` recent files cover switching. (If the owner later wants tabs, add them as an option, off by default.)
- **Permission requests** appear as a slim bar above the prompt: what the agent wants, which file (outlined in the tree), Allow once / Allow for this task / Deny. Also triggers an alert (section 11).

---

## 8. Peek: three layouts, user picks

Clicking an agent shows four things: **Live** (the file it is typing into, scrolling with its cursor), **To-do** (its plan with the current step), **Chat** (messages and tool calls streaming), **Changes** (diff of everything it changed in its worktree).

The user chooses one layout in the first-run picker and can switch any time with `Cmd+Shift+L` (cycles) or the View menu:

**A. Watch + side notes**
Editor area shows Live. A right strip shows To-do on top and Chat below. Changes opens with one click (replaces Live until closed).

**B. One view at a time**
Four small segmented buttons at the top of the editor area: Live / To-do / Chat / Changes. `Cmd+1..4` switches.

**C. Watch + pull-up drawer**
Live fills the area. A thin drawer at the bottom shows the current step and latest chat line; drag or click to expand it into To-do / Chat / Changes.

Live view rules:
- Read-only while the agent is actively editing that file, with a small banner "claude-1 is editing" and a **Take over** button (pauses the agent's turn via `session/cancel`, then makes the buffer editable).
- The agent's "cursor" is drawn in its accent color at the last edited range.
- If the agent touches a different file, Live follows it, unless the user scrolled or clicked in the last 5 seconds (do not fight the user). A "Follow" toggle sits in the peek header, on by default.
- Data source: ACP `tool_call` / `tool_call_update` locations plus file-watcher events on the worktree. For terminal-mode agents (no ACP), Live follows the most recently modified file in the worktree; To-do and Chat show "not available for terminal agents" and the terminal itself is shown instead.

---

## 9. Visualizer: live file-tree heatmap

The Files dropdown is the visualizer. Each file row can be in one state (priority order):

| State | Trigger | Look |
|---|---|---|
| needs-approval | permission request names the file | outline in agent accent, steady |
| editing | edit tool call in progress, or write event during a turn | pulsing glow in agent accent |
| reading | read/search tool call in progress | faint glow, fades 1.5 s after it ends |
| created | file did not exist at task start | small dot + "new" |
| deleted | file removed | struck through, faded |
| changed | differs from the task's base commit | `+n -m` right-aligned, subtle tint |
| idle | none | plain |

Heat over time: a file's tint gets slightly stronger the more times the agent has touched it in this task (cap at 4 levels), so a glance shows where the agent spent its effort.

Folders show a dot in the strongest state of anything inside, so activity is visible when collapsed. When a deep file starts `editing` and Follow is on, auto-expand its path and scroll it into view (same 5-second "do not fight the user" rule).

When **no agent is selected**, the main checkout tree shows a small accent dot on files that any active agent has changed in its worktree, so you can see overlap between agents before merging.

Merge two sources of truth: ACP tool calls (intent, arrive first) and the file watcher on the worktree (catches shell commands, formatters, codegen). Recompute `+n -m` for a file 150 ms after its last write, never the whole tree on every event. Render only expanded rows.

Never apply glows or effects to the logo; glows are only for file rows and agent dots.

---

## 10. Agents, worktrees, merge

### Roster config
`~/Library/Application Support/Qur/agents.toml`, written by the first-run picker after detecting installed CLIs:

```toml
[[agent]]
id = "claude"
label = "Claude Code"
mode = "acp"
command = "claude-agent-acp"   # verify current package/binary name
# accent = "#FF7A45"           # optional override of the theme slot

[[agent]]
id = "codex"
label = "Codex"
mode = "acp"
command = "codex-acp"          # verify current package/binary name
# accent = "#3DDC97"           # optional override of the theme slot

[[agent]]
id = "aider"
label = "Aider"
mode = "terminal"              # PTY fallback
command = "aider"
# accent = "#5B9BFF"           # optional override of the theme slot
```

Adapters (as of Oct 2026, **verify**): Claude Code via `@agentclientprotocol/claude-agent-acp` (older name `@zed-industries/claude-code-acp`); Codex via `@agentclientprotocol/codex-acp` (older name `@zed-industries/codex-acp`, supports ChatGPT subscription login, `CODEX_PATH` to use the installed `codex`). Install globally and pinned; never `npx` at runtime.

Accent palette: agents take accent slots 1 to 6 in order from the active theme's palette (section 12a). The `accent` value in `agents.toml` is an optional override; if omitted, the theme slot is used.

### Task lifecycle
1. User presses `+`, picks an agent, types the task.
2. Qur creates a worktree **outside the repo**: `~/Library/Application Support/Qur/worktrees/<repo-id>/<task-slug>/`, on branch `qur/<agent>/<task-slug>`, based on the current HEAD of the main checkout.
3. ACP mode: spawn the adapter with `cwd` = worktree; `initialize` (minimal client capabilities first; add `fs` / `terminal` only if an adapter requires them); `session/new`; `session/prompt`. Stream `session/update` into Chat, To-do (plan updates), Live and the heatmap. Answer `session/request_permission` via the permission bar. `session/cancel` on Stop or Take over.
   Terminal mode: spawn the CLI in a PTY in the worktree, show it as the agent's Chat view; heatmap from the watcher only.
4. When a turn ends: update status, fire alerts (section 11).
5. **Review then Merge:** the Changes view shows the diff of the task branch versus its base. Buttons: **Merge** (merge `qur/<agent>/<task>` into the main checkout's current branch; if the main checkout has uncommitted changes, ask first; on conflict, open conflicted files in the editor with simple ours/theirs/both actions), **Discard task** (confirm; remove worktree and branch), **Keep working** (send another prompt).
6. Removing a task kills the agent process (cancel, close stdin, SIGTERM, SIGKILL after 2 s) and cleans up the worktree. On app quit, agents stop; tasks and worktrees persist and can be resumed.

Several agents can run at once, including two of the same vendor, each in its own worktree.

### Hard constraints
- Never prompt for, store, or set `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`. If an adapter only works with an API key, stop and tell the owner.
- No process Qur spawns may be left orphaned (verify with `pgrep` in tests).
- Qur never force-pushes, rewrites history, or touches the main checkout except on an explicit Merge or a user edit.

---

## 11. Alerts

When an agent finishes a turn, errors, or requests permission:
- **macOS notification** (UserNotifications framework; requires a bundled, signed app; in dev builds fall back gracefully). Clicking it focuses Qur on that agent.
- **Dock badge** with the count of agents that need the user.
- **Soft sound** (one short, subtle system-style chime; bundled, under 50 KB).
- Only alert when Qur is not focused on that agent. Respect macOS Focus modes (the system does this for notifications).

---

## 12. Brand and look

The brand kit is in `brand/` beside this file. Copy it to `assets/brand/`. Read `brand/README.md` and `brand/qur-logo-usage-guide.pdf`.

- **Colors (Qur Light / Qur Dark themes):** Black `#000000`, White `#FFFFFF`. Qur Light = white UI, black text. Qur Dark = black UI, white text. A few neutral greys for borders and secondary text only. The only color is agent accents, syntax, and diff green/red. Other themes follow section 12a.
- **Logo on other themes:** use the black logo on light backgrounds and the white logo on dark ones (pick by the theme's background luminance). Never tint the logo to match a theme.
- **Typography:** UI in **Inter** (SIL OFL, bundle only the weights used, subset if possible). Code in a monospace font: default to the system's SF Mono / Menlo so nothing extra ships; user can change in config.
- **Logo use:**
  - Top-left of the window: the symbol (`svg/qur-symbol-*.svg`, or `qur-symbol-small-*` below 48 px), black or white to match theme.
  - First-run picker and About window: horizontal lockup (`svg/qur-horizontal-*.svg`), with "by PEUR Technologies" in small text below.
  - App icon: build the `.icns` from `icons/app-icon-3d-1024.png` (3D render is allowed at icon sizes 96 px and up); use the flat small symbol for any size below 96 px.
  - Never recolor the logo (including in agent accents), never detach or rotate the tail, never stretch, never add outlines, glows or effects to the flat logo. Keep 2x clear space (x = ring thickness).
- **Shape language:** the mark is rounded squares. Use the same soft rounded-square radius for agent dots, buttons and the peek header. No gradients, no shadows beyond a 1 px hairline, no decorative illustration.
- **Copy:** short, plain, lowercase-friendly. Story: the finder eye is how you keep an eye on your agents. Tagline direction: "See what your agents see." Replace any QR-code wording from the old kit.
- Trademark clearance for "Qur" has not been checked (per the kit README). Flag this in the README; do not register anything.

---

## 12a. Themes

### Built-in set (all ship in v1)

| Group | Themes |
|---|---|
| Qur (default) | Qur Light, Qur Dark |
| Popular ports | One Dark, One Light, Catppuccin (Latte, Mocha), Tokyo Night (Night, Day), Gruvbox (Dark, Light), Nord, Solarized (Dark, Light), GitHub (Dark, Light) |
| Accessibility | High Contrast Dark, High Contrast Light |

Plus **Auto**: follows macOS appearance, using Qur Light in light mode and Qur Dark in dark mode. Auto is the default.

Before porting, verify each theme's license (most of these are MIT) and record attribution in `docs/THEME_CREDITS.md` and the About window. If a theme's license does not allow redistribution, drop it and tell the owner.

### What a theme controls
The **whole UI and code**:
- UI: backgrounds (window, sidebar, editor, panels, drawer), borders, text (primary, secondary, muted), selection, focus ring, buttons, permission bar, notifications-in-app.
- Editor: syntax token colors (mapped from tree-sitter highlight names), cursor, current line, matching brackets, search matches, gutter.
- Diff: added, removed, modified (lines and inline words).
- Terminal: the 16 ANSI colors plus foreground, background and cursor.
- **Agents: a palette of 6 accent colors**, plus heatmap tints derived from them (4 heat levels). Each theme must define its own 6 accents so agent dots, glows and cursors stay distinguishable on that theme's background.

### Rules
- **One format, embedded:** every theme is a small TOML file in `crates/qur_app/themes/`, compiled into the binary (`include_str!`). Parse once at startup; switching themes must not allocate per frame or re-read files. All 20 themes together should add well under 200 KB.
- Every theme must pass a contrast check in CI: body text at least 4.5:1 against its background; high contrast themes at least 7:1; each agent accent at least 3:1 against the sidebar and editor backgrounds and distinguishable from the other five (check pairwise difference in a perceptual space such as OKLab or CIEDE2000). Write this as a `cargo test`.
- A missing token in a theme falls back to the closest Qur theme (light or dark by background luminance), never to a hard-coded color in UI code. UI code reads colors only through theme tokens.
- Switching themes is instant and keeps all state (open file, scroll, agents, peek layout).

### Picking a theme
- **First-run picker:** a grid of small live previews (a sidebar with two agents, a few lines of highlighted code, one glowing file row) for each theme; Auto preselected.
- **`Cmd+K Cmd+T`:** VS Code-style quick picker. Arrow keys preview themes live; Enter keeps, Esc reverts.
- Choice is saved in `config.toml`.

---

## 13. First-run picker

Shown once on first launch (re-openable from the menu: Qur > Setup...). One screen, brand lockup on top, then:

1. **Agents found:** detected CLIs/adapters with checkboxes (Claude Code, Codex, others found on PATH). A "Check" button runs a quick ACP `initialize` to confirm each works on the user's existing login.
2. **Panels:** Terminal, Git, Search (checkboxes, all on by default).
3. **Peek layout:** A / B / C with small schematic previews.
4. **Keys:** VS Code (default) / VS Code + Vim mode.
5. **Theme:** grid of live previews of all built-in themes, Auto preselected (section 12a).

Saved to `config.toml`. Everything here can also be edited by hand; there is no other settings UI.

---

## 14. Milestones and acceptance criteria

### M0: Spikes (prove the risky parts)
- Toolkit spike (section 5): report the comparison table, pick one.
- ACP spike: spawn both adapters in a scratch repo, run a full prompt, dump every `session/update` to a file, document the mapping from each adapter's tool calls to heatmap states and Live-follow.
- Worktree spike: create a worktree outside the repo, let an agent edit it, merge back, verify the main checkout's index and stash are untouched.
- **Accept:** both agents work on subscription logins with no API keys in the environment; toolkit chosen with numbers; owner has seen the report.

### M1: Editor shell
Window, theme system with all built-in themes, Auto mode, `Cmd+K Cmd+T` picker and the contrast test (section 12a), brand symbol, project switcher, sidebar with collapsed Files dropdown, editor with tree-sitter highlighting (TS/TSX, JS, Rust, Python, JSON, CSS, HTML, Markdown, TOML, YAML, Kotlin, Dart), find/replace, `Cmd+P`, VS Code keymap, no tabs.
**Accept:** budget targets for size, idle RSS, cold start and large-file open pass; all themes pass the contrast test; switching themes is instant and keeps state.

### M2: Agents in worktrees
Roster config, `+` new task, worktree creation, ACP client, prompt bar, Chat and To-do, permission bar, Stop, process cleanup.
**Accept:** two Claude tasks and one Codex task run in parallel in separate worktrees; Stop and quit leave no orphans.

### M3: Peek + heatmap
Live view with follow and Take over, the three peek layouts with `Cmd+Shift+L`, heatmap states, folder dots, heat levels, overlap dots on the main tree.
**Accept:** an edit glows within 100 ms; Live follows the agent across files; switching layouts keeps state.

### M4: Changes + Merge + Git panel
Changes diff view, Merge with conflict handling, Discard task, Git panel (status, stage, commit on the main checkout).
**Accept:** a merged task's commits appear on the user's branch; a conflict is resolvable in-app; Discard removes worktree and branch.

### M5: Terminal
Bottom terminal panel (`alacritty_terminal`), and terminal-mode agents (PTY fallback) with watcher-driven heatmap.
**Accept:** a non-ACP CLI agent runs in its worktree and its edits light up the tree.

### M6: Search, Vim, first-run picker, alerts
**Accept:** project search over 5,000 files returns in under 300 ms; Vim mode covers normal/insert/visual basics; first-run picker writes config; notifications, dock badge and sound fire only when appropriate.

### M7: Polish and packaging
`.app` bundle with `.icns`, signing notes, README (install, adapters, config, screenshots or GIF of the heatmap, budget numbers, "how Qur compares to Zed, Orca and Superset", trademark note), `qur doctor` CLI subcommand (checks git, adapters, ACP initialize).
**Accept:** `scripts/budget.sh` passes on the owner's Mac with real agents; `lsof -i -p <qur pid>` shows no outbound connections from Qur itself.

---

## 15. Ask the owner after M0 (one message, defaults included)

1. Toolkit result and recommendation (from the spike table).
2. Repo location (default `~/qur`).
3. Accent palette approval for Qur Light / Qur Dark (show the 6 swatches on black and white), and any theme dropped for licensing.
4. Whether file tabs should exist as an off-by-default option later (default: no).
5. Whether "by PEUR Technologies" appears in the title bar or only in About (default: About only).
