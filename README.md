# Qur

**See what your agents see.**

Qur is a lightweight, native macOS code editor for running and steering AI coding agents. Each agent works in its own git worktree. The file tree glows as a live heatmap while agents read and edit. When an agent is done, you review its changes and merge them.

It's written in Rust on [GPUI](https://github.com/zed-industries/zed) (Zed's UI framework): no Electron, everything local, and agents use your existing Claude Code login (never an API key).

> **Status: early development (pre-release), macOS only.** M0–M6 (agents in worktrees, Live + heatmap, Changes + Merge + Git panel, terminal agents, search + alerts + Vim) and Browser + Inspect are done. See [HANDOFF.md](HANDOFF.md) for the current state.

![Live heatmap: files glow as the agent writes them; Live follows the file](docs/images/heatmap-live.png)

## Contents

- [Quick start](#quick-start)
- [The window](#the-window)
- [Features](#features)
- [Everyday keys](#everyday-keys)
- [Configuration](#configuration)
- [Lightness](#lightness)
- [How Qur compares](#how-qur-compares)
- [Development](#development)
- [License and credits](#license-and-credits)

## Quick start

### Requirements

- macOS (Apple Silicon tested; macOS 27).
- **Rust** stable, via [rustup](https://rustup.rs).
- **Xcode** with the Metal toolchain (GPUI compiles Metal shaders). If the build complains, run `xcodebuild -downloadComponent MetalToolchain` once.
- **git** 2.38 or newer (Merge uses `git merge-tree --write-tree`).
- For agents:
  - [Claude Code](https://claude.com/claude-code), logged in with your subscription (`claude` CLI).
  - The ACP adapter, pinned: `npm i -g @agentclientprotocol/claude-agent-acp@0.94.0` (don't use `npx` at runtime).

### Build and run

```sh
git clone https://github.com/PEUR-Technologies/Qur.git && cd Qur
cargo build --release -p qur_app
target/release/qur ~/path/to/your/repo   # a folder or a file; no argument reopens the last project
```

Press `Cmd+Shift+T` for an Agent Terminal (Claude in its own worktree), or `Cmd+Shift+N` for a structured agent task. Describe the task and watch.

## The window

One window is one project. There's no permanent editor area: the window is for agents and files.

- **Left column**
  - **AGENTS**: one button per agent. A button fills with the agent's colour while it's working, and the selected agent has a coloured outline. Click one to open its session.
  - **TERMINALS**: plain shells, shown only when you have some.
  - **SOURCE CONTROL**: branch and change count; opens the Git panel.
  - **FILES**: the file tree, and the only collapsible section. It shows the selected agent's worktree, or your checkout when no agent is selected.
- **Stage** (the rest of the window): the selected session.
  - An Agent Terminal or a shell fills the stage as a real terminal.
  - A structured (ACP) agent shows Live, its to-do plan, chat, a permission bar and the prompt.
- **Code modal**: opening a file shows it in a modal with full editing. `Esc` closes it.

## Features

### Agents

- **Isolated worktrees.** Each task gets a git worktree outside your repo, on branch `qur/<agent>/<task>`, so your checkout stays untouched. Run several agents at once.
- **Agent Terminals** (`Cmd+Shift+T`): the interactive `claude` CLI in its own worktree, with the sidebar button, heatmap, review and merge like any other task. Any other CLI agent works too: add it to `agents.toml` with `mode = "terminal"`.
- **Agents name their own work.** A button shows what the agent says it's doing (Claude Code's terminal title), unless you rename it (Agent > Rename Agent…, or double-click).
- **Terminal comforts:** Find (`Cmd+F`, regex, through the scrollback) and `Cmd`-click to open URLs and `file:line` paths.
- **Structured sessions** over the [Agent Client Protocol](https://agentclientprotocol.com):
  - Streaming tool calls, the agent's to-do plan, and a permission bar (Allow once / Allow for this task / Deny).
  - Stop at any time. Messages sent mid-turn are queued; `@` mentions a file and `/` runs agent commands.
- **Unread markers** on agents that finished or need you while you were elsewhere.
- **Turn-end checks.** Set `[checks] command` per project, and Qur runs it in the worktree after each agent turn that changed files. A failure marks the agent's button `✕ checks` and lands in the prompt (or the Agent Terminal's input line) for you to send back. Agent > Run Checks Now runs it any time.

### Watching

- **Live file-tree heatmap.**
  - An outline on files waiting for your approval, a pulsing glow while editing, a fading glow while reading, and "new" or `+n −m` on changed files.
  - Heat deepens with how often a file was touched, and folders roll up their contents.
  - On your main checkout, dots show which files agents changed in their worktrees.
- **Live view** (structured agents): the file the agent is working in, as it types. Read-only while the agent edits, with **Take over** to step in. It follows the agent across files but never jumps within 5 s of your own scroll or click.
- **Browser with Inspect.** Open your dev server next to the agent, pick elements or text, and ask the agent to adjust them. It gets the selector, HTML, key CSS, the source location when available, and a screenshot. Uses the system WebKit, freed when idle.
- **Run Project in Worktree.** Starts your dev server in the agent's worktree on its own port and opens it in that agent's Browser, so parallel agents never collide. Localhost only; stopped on Discard, Merge cleanup or quit.

![Browser + Inspect on peur.ph: pick an element, ask the agent](docs/images/browser-inspect.jpg)

### Review and merge

- Review the task's diff and comment on lines; comments go back to the agent as one message.
- A pre-check warns about conflicts. **Merge** never touches your staged changes or stash.
- Resolve conflicts in-app (Accept Ours / Theirs / Both, then Finish or Abort; also in Agent > Merge Conflicts), and stage and commit in the Git panel.
- Discard removes the task's worktree and branch.

### Search and alerts

- **Project search** (`Cmd+Shift+F`): the whole tree you're looking at (an agent's worktree or your checkout) in about 50 ms for 5,000 files, with case, whole-word and regex options and Replace All.
- **Alerts:** when an agent finishes, errors or needs approval while you're elsewhere: a notification (click it to jump to the agent), a Dock badge and a soft chime. Each can be turned off in `config.toml` (`[alerts]`).

### Editor and looks

- Code modal with tree-sitter highlighting for 13 languages, multi-cursor, regex find/replace, soft wrap and VS Code keybindings, plus optional Vim mode (Qur > Vim Mode).
- A first-run picker for agents, layout, keys and theme (Qur > Setup… reopens it).
- 17 built-in themes plus Auto (follows macOS), a theme per project, and window transparency with blur. Every theme passes a contrast test.
- Every command is in the macOS menu bar and the command palette (`Cmd+Shift+P`). No toolbars or settings screens.

## Everyday keys

| Keys | |
|---|---|
| `Cmd+Shift+T` / `Cmd+T` | New Agent Terminal / new shell |
| `Cmd+Shift+N` | New structured agent task |
| `Ctrl+1..9`, `Ctrl+Opt+↑/↓` | Switch sessions |
| `Cmd+I`, `Esc` / `Cmd+.` | Focus the prompt, stop the agent |
| `Cmd+P`, `Cmd+Shift+P` | Go to file, command palette |
| `Cmd+K Cmd+L`, `Cmd+1..5` | Structured agents: cycle layouts; Live / To-do / Chat / Changes / Browser |
| `Cmd+Shift+C` | Browser: inspect element |
| `Ctrl+Shift+G` | Source control |
| `Cmd+K Cmd+T` | Theme (this project or all projects) |

Full list: [docs/keymap.md](docs/keymap.md). Menu bar: [docs/menus.md](docs/menus.md).

## Configuration

Everything lives in `~/Library/Application Support/Qur/`. Qur never writes into your repo; only agents do, in their worktrees.

| File | What |
|---|---|
| `config.toml` | Theme, window opacity/blur, editor font, soft wrap, peek layout, `[keys] vim`, `[alerts]`. Qur > Settings opens it; saved changes apply right away. |
| `agents.toml` | Which agents Qur can start (ACP or terminal mode). Written on first launch. |
| `projects/<id>.toml` | Per-project theme, opacity, `[worktree] setup = "pnpm install"` (runs in each new worktree before the agent starts), `[checks] command = "cargo check"` (runs after each agent turn that changed files), and `[run] command` / `url` with `$PORT` (Run Project). |
| `projects/<id>.tasks.toml` | Tasks, so they survive restarts and resume their sessions. |
| `worktrees/<id>/<task>/` | The task worktrees. |

In your repo, an optional **`.worktreeinclude`** file lists one path per line (e.g. `.env`, `node_modules`). Qur copies those into each new worktree with APFS copy-on-write: instant, no extra disk.

## Lightness

Measured on a MacBook Air M5 with `scripts/budget.sh` ([docs/budget.md](docs/budget.md)):

| | Target | Qur |
|---|---|---|
| App binary | < 40 MB | ~25 MB, including 13 tree-sitter grammars (~11 MB) |
| Idle memory, project + small file (physical footprint) | < 80 MB | ~71 MB |
| Memory with 3 Claude agents active (Qur's process) | < 150 MB | 81 MB |
| Start to first frame | < 250 ms | ~110–130 ms |
| Open a 10,000-line file, fully highlighted | < 100 ms | ~86 ms |
| Agent write → glow on screen (Qur unfocused) | < 100 ms | 81–84 ms |
| Keystroke handled and drawn (CPU side, p99) | ≤ 8.3 ms | ~3.5 ms |

## How Qur compares

**Zed.** Qur shares Zed's GPUI and native feel, but it isn't a general-purpose editor with an agent panel. In Qur the agents are the main view; code appears in the heatmap, Live, the code modal and the review diff.

**Orca** ([stablyai/orca](https://github.com/stablyai/orca)) is the closest peer. We studied it ([docs/research/orca.md](docs/research/orca.md)) and adopted several ideas: worktree setup commands, `.worktreeinclude`, review comments sent back to the agent, and Design-Mode-style inspect.

| | Qur | Orca |
|---|---|---|
| Tech | Rust + GPUI, native; ~25 MB | Electron + React; ~254 MB app |
| Agents in isolated worktrees | Always, outside the repo | Yes |
| Agent link | Terminal or ACP (structured) | Terminal by default; structured chat experimental |
| Live view of the agent's current file, with Take over | Yes | — |
| File-tree heatmap of agent activity | Yes | — |
| Permissions | Asked in a permission bar by default | Permission-bypass flags by default |
| Shipping changes | Local Merge into your branch (index- and stash-safe) | Push and pull request |
| Browser + element inspect | System WebKit, freed when idle | Bundled Chromium |
| Telemetry / background daemon | None / none | Opt-out telemetry / keeps terminals alive after quit |

Based on our reading of Orca's repository and docs in October 2026; corrections welcome.

## Development

Read [HANDOFF.md](HANDOFF.md) and [docs/decisions.md](docs/decisions.md) first.

```sh
cargo test --release --workspace   # includes the theme contrast test and ACP tests against a fake agent
scripts/budget.sh 5                # weight budget; needs an unlocked screen (AC power recommended)
```

House rules:
- Lightness is the product: measure with `scripts/budget.sh`, don't estimate.
- No UI that isn't in the spec or decisions.
- Agents only ever use their own CLI logins; Qur never sets `ANTHROPIC_API_KEY`.

### Project layout

```
crates/
  qur_app       the app (binary `qur`): window, sidebar, stage, Live, Browser, Changes, Git panel, menus
  qur_acp       hand-rolled ACP v1 client (JSON-RPC over stdio, no async runtime)
  qur_agents    agent roster (agents.toml) and task store
  qur_worktree  task worktrees, .worktreeinclude, setup command
  qur_watch     FSEvents file watcher (debounced, .gitignore-aware)
  qur_heat      heatmap model (states, heat levels, diff stats)
  qur_git       diffs, merge (index- and stash-safe), conflicts, status/stage/commit
  qur_browser   system WKWebView embedding + element inspector
  qur_editor    UI-free editor model (rope, multi-cursor, undo, find, soft wrap)
  qur_syntax    tree-sitter highlighting for 13 languages
  qur_theme     17 themes + contrast tests
  qur_config    settings files
  qur_tree      lazy, gitignore-aware file tree
docs/           spec (QUR_HANDOFF.md), owner decisions, keymap, menus, budget, research
spikes/         M0 spikes and test fixtures
```

## License and credits

[GPL-3.0-or-later](LICENSE). © PEUR Technologies.

Themes are ported from their upstream projects under their licenses ([docs/THEME_CREDITS.md](docs/THEME_CREDITS.md)). Fonts, grammars and other components are listed in [docs/THIRD_PARTY.md](docs/THIRD_PARTY.md). The UI font is Inter (SIL OFL).

**Trademark:** "Qur" has not been through trademark clearance. Don't register or rely on the name commercially until it has.
