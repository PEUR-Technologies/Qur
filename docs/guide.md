# Qur guide

Everything the [README](../README.md) leaves out: requirements, every feature, configuration, measurements, comparisons and development.

## Install

### Requirements

- macOS 13 or newer (developed on macOS 27, Apple Silicon).
- **git** 2.38 or newer (Merge uses `git merge-tree --write-tree`).
- For agents, on your own logins (Qur never sets an API key):
  - [Claude Code](https://claude.com/claude-code), logged in with your subscription (`claude` CLI). That's all Agent Terminals need.
  - For structured (ACP) agents, the adapter, pinned: `npm i -g @agentclientprotocol/claude-agent-acp@0.94.0` (don't use `npx` at runtime).
  - Gemini CLI, opencode, GitHub Copilot and Cursor over ACP, Aider in a terminal: Settings > Agents lists them once installed. Any other CLI agent: Settings > Agents > **Add agent** (name, command, Structured or Terminal), or by hand in `agents.toml`.

### The app

```sh
git clone https://github.com/PEUR-Technologies/Qur.git && cd Qur
scripts/bundle.sh                      # -> target/release/Qur.app (~29 MB, ad-hoc signed for this Mac)
cp -R target/release/Qur.app /Applications/
ln -sf /Applications/Qur.app/Contents/MacOS/qur /usr/local/bin/qur   # optional: `qur <folder>` from a terminal
qur doctor                             # checks git, the agents and their adapters, then a real ACP initialize
```

Building needs **Rust** stable ([rustup](https://rustup.rs)) and **Xcode** with the Metal toolchain (`xcodebuild -downloadComponent MetalToolchain` once if the build asks). Release signing and notarization: [docs/signing.md](signing.md). Notifications need the app bundle; everything else also runs straight from `target/release/qur`.

On first launch the settings screen opens as a welcome: connect an AI account (install and sign in, decision 45), then pick the theme, accent, layout and keys if you like. **Qur > Settings…** (`Cmd+,`) brings it back any time.

Then open a project (File > Open Folder, or `qur ~/path/to/repo`) and press `Cmd+Shift+T` for an Agent Terminal (Claude in its own worktree), or `Cmd+Shift+N` for a structured agent task. Describe the task and watch.

### From source, without bundling

```sh
cargo build --release -p qur_app
target/release/qur ~/path/to/your/repo   # a folder or a file; no argument reopens the last project
```

## The window

One window is one project. There's no permanent editor area: the window is for agents and files.

- **Left column**
  - **AGENTS**: one button per agent. A button fills with the agent's colour while it's working, and the selected agent has a coloured outline. Click one to open its session.
  - **TERMINALS**: plain shells, shown only when you have some. Split one with `Cmd+D` (right) or `Cmd+Shift+D` (down); the new shell starts in the project folder and shares the row. `Cmd+Opt+Arrows` or a click moves between panes, drag the divider to resize, `Cmd+W` closes the focused pane. Agent Terminals split the same way.
  - **SOURCE CONTROL**: branch and change count; opens the Git panel.
  - **FILES**: the file tree, and the only collapsible section. It shows the selected agent's worktree, or your checkout when no agent is selected.
- **Stage** (the rest of the window): the selected session, in panes you arrange like VS Code.
  - An Agent Terminal fills the stage as a real terminal; add Live, Browser or Changes beside it with the pane's **+** (or `Cmd+5` for the Browser). Inspect picks are pasted into its terminal, with a screenshot file path.
  - A structured (ACP) agent shows Live, Browser and Changes next to its Chat (with the permission bar and the prompt) and To-do.
  - Drag a tab, or a whole pane by its tab strip (the agent's own pane too), over another pane: a rounded preview shows where it will land, as macOS window tiling does. Near an edge it goes beside the pane on that side; on the middle or the tab strip it joins the pane. Let go and the panes glide into their new places. Drag the dividers to resize; × tucks a tab away and the pane's + menu brings it back. Your arrangement is saved. Split terminals glide the same way when you split (`Cmd+D`, `Cmd+Shift+D`), lay them out as a grid (`Cmd+Enter`) or close one.
- **Code modal**: opening a file shows it in a modal with full editing. `Esc` closes it. In an agent's worktree the gutter marks the agent's changes (yours against HEAD in your checkout): Next/Previous Change opens a peek with the removed lines, and Revert undoes one change at a time (undoable; the index is never touched).
- **Files of any kind**: the modal detects the encoding (UTF-8, UTF-16, Windows-1252) and line endings, shows them in its header and saves them back; File > Reopen with Encoding…, Convert to Encoding… and Convert Line Endings change them. Images open in the file viewer; other binaries in a hex view (Open as Text shows them as text anyway). Files over 50 MB (`[editor] large_file_mb`) open read-only and paged from disk, so a 1 GB log costs almost no memory. View > Show Whitespace. A file changed on disk reloads by itself, or asks if you have unsaved edits. File > Open File… (`⌘O`) opens anything, in or outside the project. Closed files are dropped from memory (unsaved ones never are) and reopen where you left them.
- **Files tree menu**: right-click a file for Open, Open in Default App, Reveal in Finder, Copy Path and Copy Relative Path (also in the File menu for the open file).
- **No project**: opening Qur with no folder (from Spotlight or the Dock) is a terminal: the empty window with a shell in your home folder. `cd` into a git repository, or pick a folder (Open Folder…, a recent project), and that window becomes the project's, terminals and all. Closing the last project shows the same window with recent projects, Open Folder… and terminals. A folder without git opens with git features off.
- **File viewer**: an image (PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, SVG) opens in a modal of its own, fitted to it and never enlarged, with its size in pixels and bytes. A file that isn't text gets its name and size instead of an error. Both offer **Open in Default App** and **Reveal in Finder**; `Esc` or a click outside closes it.
- **Window gaps** (View > Toggle Gaps, or Qur > Appearance > Gaps… to adjust; also a row in Setup): like Hyprland, the top bar, the sidebar, every dock pane and every split terminal float as their own tiles with clear gaps between them and around the window's edge, so Qur looks like a stack of separate windows. The popover sets the gap (2–24 pt), rounded corners and their radius (0–16 pt), and a hairline border per tile; it previews as you drag and saves when it closes. Drag a gap between panes (or between the sidebar and the stage) to resize. The gaps show your desktop even at 100% opacity; below 100% with blur on they're blurred too, because macOS blurs the whole window. With zen mode on too, the top bar's tile slides down at the top edge and pushes the tiles below it down.
- **Sidebar** (View > Sidebar `Cmd+B`, View > Auto-hide Sidebar, View > Sidebar on Right; also the Sidebar rows in Settings > Appearance): always shown, auto-hidden, or hidden until `Cmd+B`. Auto-hidden, it slides in when the mouse reaches its edge of the window (pushing the stage over) and away once the mouse leaves it, like Chrome's vertical tabs. It can sit on the left or the right. Drag a section header (AGENTS, TERMINALS, SOURCE CONTROL, FILES) onto another to reorder the sections. Saved as `[ui] sidebar`, `sidebar_side` and `sidebar_sections`.

## Features

### Agents

- **Isolated worktrees.** Each task gets a git worktree outside your repo, on branch `qur/<agent>/<task>`, so your checkout stays untouched. Run several agents at once.
- **Agent Terminals** (`Cmd+Shift+T`): the interactive `claude` CLI in its own worktree, with the sidebar button, heatmap, review and merge like any other task. Any other CLI agent works too: add it in Settings > Agents (**Add agent**, Terminal) or in `agents.toml` with `mode = "terminal"`.
- **Agents name their own work.** A button shows what the agent says it's doing (Claude Code's terminal title), unless you rename it (Agent > Rename Agent…, or double-click).
- **Terminal comforts:** Find (`Cmd+F`, regex, through the scrollback) and `Cmd`-click to open URLs and `file:line` paths.
- **Structured sessions** over the [Agent Client Protocol](https://agentclientprotocol.com):
  - Streaming tool calls, the agent's to-do plan, and a permission bar (Allow once / Allow for this task / Deny). When the agent asks several things at once, the bar shows them one at a time ("1 of 3 requests waiting") and **Allow all 3 once** (also Agent > Permission: Allow All Pending) answers them all "allow once", never "always".
  - Tool calls in a row fold into one line, like "Read 4 files, ran 2 commands ▸"; click it to see each call. A call that's running, failed or waiting for your permission stays in view.
  - **Find in Chat** (`Cmd+F` in the agent's view) searches your messages, the agent's replies, thoughts, and tool titles and output, ignoring case. `↩` goes to the older match, `⇧↩` the newer one, `Esc` closes; a folded thought, tool call or group opens to show the match.
  - **Copy Chat as Markdown** (Agent menu, the palette, or right-click the agent): the whole chat on the clipboard, with your messages quoted, the agent's Markdown as it wrote it, one line per tool call with ✓ / ✕, and thoughts left out.
  - Stop at any time. Messages sent mid-turn are queued; `@` mentions a file and `/` runs agent commands.
  - **Files and images:** paste (`Cmd+V`) files copied in Finder, or drop them on the prompt, and they become chips above it; a pasted screenshot or copied image does too. Images go to the agent as images (scaled down when over the 5 MB limit; TIFF and BMP sent as PNG), any other file or folder as a link the agent reads itself. Click a chip, or a thumbnail or file under a sent message, to see it in the file viewer (text files in the code modal); × removes a chip. In the New Agent Task dialog, attached files become mentions.
- **Plan mode** (Agent > Plan Mode `⌥⌘P`, or "Plan first" in New Agent Task): the agent plans before it edits. Its plan opens in place of the chat with numbered lines; click a line to comment, then **Send Comments** (it revises) or **Approve** (back to the mode you were in).
- **Usage:** the agent's header shows tokens and cost when the adapter reports them ("121.2k tokens · $0.12"; on a subscription the cost is the adapter's figure).
- **Idle agents let go of their memory.** A structured agent you haven't looked at, idle for 10 minutes, has its processes stopped (about 220 MB each for Claude); select it or send it a message and the session resumes where it was. Set `[agents] idle_release` in minutes, 0 for never. Agents that are working, waiting for you or have queued messages are never stopped, nor are Agent Terminals.
- **Chats are kept.** A structured agent's chat survives a restart; only its last few hundred rows stay in memory, and **Show earlier messages** brings back the rest from disk.
- **Unread markers** on agents that finished or need you while you were elsewhere.
- **Turn-end checks.** Set `[checks] command` per project, and Qur runs it in the worktree after each agent turn that changed files. A failure marks the agent's button `✕ checks` and lands in the prompt (or the Agent Terminal's input line) for you to send back. Agent > Run Checks Now runs it any time.

### Watching

- **Live file-tree heatmap.**
  - An outline on files waiting for your approval, a pulsing glow while editing, a fading glow while reading, and "new" or `+n −m` on changed files.
  - Heat deepens with how often a file was touched, and folders roll up their contents.
  - Files the agent deleted stay in the tree, struck through and dimmed with their glow, until they come back or you merge or discard the task.
  - It follows the same ignore rules as the tree: `.gitignore` and `.ignore` files in any folder, `.git/info/exclude` and your global git excludes.
  - Your own saves from the code modal and Replace All never glow as an agent's edit.
  - An agent working in the project folder (the first Agent Terminal) shares the folder with you, so only files it touched glow: writes count as its own while it's busy (an Agent Terminal printing output, or a structured agent running a tool), and your uncommitted work stays quiet.
  - On your main checkout, dots show which files agents changed in their worktrees.
- **Live view** (structured agents): the file the agent is working in, as it types. Read-only while the agent edits, with **Take over** to step in. It follows the agent across files but never jumps within 5 s of your own scroll or click.
- **Browser with Inspect.** Open your dev server next to the agent, pick elements or text, and ask the agent to adjust them. It gets the selector, HTML, key CSS, the source location when available, and a screenshot. Uses the system WebKit, freed when idle.
- **Run Project in Worktree.** Starts your dev server in the agent's worktree on its own port and opens it in that agent's Browser, so parallel agents never collide. Localhost only; stopped on Discard, Merge cleanup or quit.

### Review and merge

- Review the task's diff and comment on lines; comments go back to the agent as one message.
- **Turns** (structured agents): each turn that changed files ends with a line like "3 files · +12 −4 · View · Rewind". **View** shows just that turn's changes in Changes (comment as usual; **All changes** goes back). **Rewind** puts the worktree's files back to how they were before that turn, after asking: it stops the agent first if it's working, warns if files changed since the turn ended, and tells the agent with your next message. Commits stay. Also Agent > Show Last Turn's Changes and Rewind Last Turn…. Qur keeps a snapshot at the start and end of each turn (a git tree in the task's worktree, pinned under `refs/qur/turns/<task>/`, deleted on Discard or after Merge); your own checkout, index and stash are never touched. Agent Terminals get turns too: each busy-then-quiet stretch that changes files is one, listed in their Changes tab; Rewind waits until the agent is quiet and types a one-line note into its input line (not sent).
- **Second opinion** (structured agents): Agent > **Review Agent's Work…** (or **Review Agent's Plan…** when the agent has a plan) starts a reviewer on the task. Pick who reviews (Up/Down or click): another agent from your roster, such as Codex reviewing a Claude task or the reverse, or another model of the same agent. The default is the other account when it's installed and signed in, else another model. An optional line tells it what to look at. The reviewer is a second session in the same worktree; it gets the task's diff and a summary of its chat (both capped in size) and answers with findings, each with a **Criticality N/10**. It shows as `↳ review · codex` under the agent in the sidebar; click it to watch it in the chat panel. It's **read-only by Qur's rule, not by an OS sandbox**: Qur denies every edit or command it asks to run (no permission bar; the chat says what was denied) and starts it in its plan or read-only mode. When it's done, **Send Findings to Agent** puts a reply request into the agent's prompt (per finding: Agree/Disagree, Why, Fix, then ask before changing anything), for you to edit and send; it queues if the agent is busy. A review costs one more session on the reviewer's own account. The reviewer is saved with the task (click it after a restart to see its findings again) and goes away on Close, Discard or Merge.
- A pre-check warns about conflicts. **Merge** never touches your staged changes or stash.
- **Push and Open Pull Request** (Agent menu, or next to Merge): pushes the task's branch with your git and opens a pull request with your `gh` (or GitHub's compare page without it). The link stays on the task; remote branches are never deleted.
- Resolve conflicts in-app (Accept Ours / Theirs / Both, then Finish or Abort; also in Agent > Merge Conflicts), and stage and commit in the Git panel.
- Discard removes the task's worktree and branch.
- **Commit messages** are written from the agent's turns: the Git panel's box is prefilled, and Merge uses it.
- **Clean Up Worktrees…** (Agent menu) lists every worktree and `qur/*` branch with its size; leftovers are preselected, and anything with unmerged commits or unsaved changes asks twice.

### Search and alerts

- **Project search** (`Cmd+Shift+F`): the whole tree you're looking at (an agent's worktree or your checkout) in about 50 ms for 5,000 files, with case, whole-word and regex options and Replace All.
- **Alerts:** when an agent finishes, errors or needs approval while you're elsewhere: a notification (click it to jump to the agent), a Dock badge and a soft chime. Each can be turned off in `config.toml` (`[alerts]`).
- **Crash reports** stay on your Mac (`~/Library/Logs/Qur/crash-*.txt`, the newest 10). After a crash the next launch offers View Report, Report a Problem (opens the issue page; nothing is uploaded) or Dismiss.
- **Memory:** About and `qur doctor` show what Qur, its agents and terminals use, and how much disk the worktrees take.

### Editor and looks

- Code modal with tree-sitter highlighting for 13 languages, multi-cursor, regex find/replace, soft wrap and VS Code keybindings, plus optional Vim mode (Edit > Vim Mode, or Settings).
- **Go to Symbol in File** (`Cmd+Shift+O`): functions, methods, types, classes, headings, keys… from the same tree-sitter parse as the colors (no language server). Arrow keys preview each symbol; Esc puts the cursor back.
- **Expand / Shrink Selection** (`Ctrl+Shift+Cmd+→` / `←`): grows every cursor's selection to the enclosing syntax node (word, call, statement, block, function…), and shrinks back one step at a time. Files Qur doesn't parse grow by word, line, then the whole file.
- With soft wrap on, `End` / `Cmd+→` stops at the end of the visual row first, then the line; `Home` / `Cmd+←` likewise.
- Input methods (Japanese, Chinese, Korean, press-and-hold accents): the text being composed shows underlined at the cursor until you commit it.
- **Settings** (`Cmd+,`): one screen with a section list. Agents (install, sign in, which agents to use, Add agent for any other CLI; unticking one removes it from agents.toml), Appearance (theme for this project or all, accent color and brightness, transparency and blur, top bar, traffic lights in full screen), Editor (Vim mode, keys), Layout (the agent stage presets as diagrams), and Files (open config.toml, keymap.toml, agents.toml). Changes apply and save as you make them; Esc closes. It also opens by itself on first launch.
- 17 built-in themes plus Auto (follows macOS), a theme per project, and window transparency with blur. Every theme passes a contrast test.
- Every command is in the macOS menu bar and the command palette (`Cmd+Shift+P`). No toolbars; the one settings screen is Qur > Settings…, and its settings are also menu commands (View > Appearance, Edit > Vim Mode).

## Everyday keys

| Keys | |
|---|---|
| `Cmd+Shift+T` / `Cmd+T` | New Agent Terminal / new shell |
| `Cmd+Shift+N` | New structured agent task |
| `Ctrl+T`, `Ctrl+1..9`, `Ctrl+Opt+↑/↓` | Switch sessions (`Ctrl+T` picks one by name) |
| `Cmd+I`, `Esc` / `Cmd+.` | Focus the prompt, stop the agent |
| `Cmd+P`, `Cmd+Shift+P` | Go to file, command palette |
| `Cmd+Shift+O`, `Ctrl+Shift+Cmd+→/←` | Go to symbol in file; expand / shrink selection |
| `Cmd+K Cmd+L`, `Cmd+1..5` | Structured agents: cycle layouts; Live / To-do / Chat / Changes / Browser (`Cmd+5` works for Agent Terminals too) |
| `Cmd+Shift+C` | Browser: inspect element |
| `Ctrl+Shift+G` | Source control |
| `Cmd+K Cmd+T` | Theme (this project or all projects) |

Full list: [docs/keymap.md](keymap.md). Menu bar: [docs/menus.md](menus.md).

## Configuration

Everything lives in `~/Library/Application Support/Qur/`. Qur never writes into your repo; only agents do, in their worktrees.

| File | What |
|---|---|
| `config.toml` | Theme, window opacity/blur, editor font, soft wrap, peek layout, `[keys] vim`, `[alerts]`, and window gaps: `[ui] gaps = true`, `gap = 8`, `rounded = true`, `radius = 8`, `tile_border = false`. Qur > Settings… changes most of it; Qur > Settings Files > Open config.toml opens it, and saved changes apply right away. |
| `agents.toml` | Which agents Qur can start (ACP or terminal mode). Written on first launch. |
| `keymap.toml` | Your own key bindings (see below). Qur > Settings Files > Open keymap.toml (or Settings > Editor); saved changes apply right away. |
| `projects/<id>.toml` | Per-project theme, opacity, `[worktree] setup = "pnpm install"` (runs in each new worktree before the agent starts), `[checks] command = "cargo check"` (runs after each agent turn that changed files), and `[run] command` / `url` with `$PORT` (Run Project). |
| `projects/<id>.tasks.toml` | Tasks, so they survive restarts and resume their sessions. |
| `worktrees/<id>/<task>/` | The task worktrees. |

In your repo, an optional **`.worktreeinclude`** file lists one path per line (e.g. `.env`, `node_modules`, `apps/*/.env`; a leading `/` means the repo root, as in `.gitignore`). Qur copies those into each new worktree with APFS copy-on-write: instant, no extra disk. Files the worktree already has are left alone.

### Keymap

`keymap.toml` rebinds commands by name, under `[bindings]`:

```toml
[bindings]
"Go to Symbol in File" = "cmd-r"                        # replaces Cmd+Shift+O
"editor::DeleteLine" = ["cmd-shift-k", "ctrl-shift-k"]  # several keys
"View: Sidebar" = ""                                     # unbound
```

- **Command names:** the action (`qur::GoToSymbol`, `editor::DeleteLine`; `QUR_DEBUG_ACTIONS` uses the same names), the command palette entry (`Go: Go to Symbol in File…`), or just its title when only one command has it. Case and a trailing `…` don't matter.
- **Keys:** modifiers `cmd`, `ctrl`, `alt` (Option), `shift`, joined with `-` before the key: `cmd-shift-o`, `` ctrl-` ``, `alt-up`, `f5`. A chord is two keystrokes with a space: `cmd-k cmd-s`. A list binds several keys; `""` or `[]` unbinds.
- A binding replaces all of that command's default keys and keeps where the command works (the editor, a terminal, anywhere).
- Entries Qur can't use (an unknown or ambiguous name, a malformed key, a value that isn't a string or list) are skipped and written to the log (Help > Show Logs), and shown in the top bar when you save the file in Qur; the rest still apply. Qur never fails to start over keymap.toml.
- The menu bar and the palette show your keys.

## Lightness

Measured on a MacBook Air M5 with `scripts/budget.sh` ([docs/budget.md](budget.md)):

| | Target | Qur |
|---|---|---|
| App | < 40 MB | 29 MB (`Qur.app`), including 13 tree-sitter grammars (~11 MB) |
| Idle memory, project + small file (physical footprint) | < 80 MB | 73 MB |
| Memory with 3 Claude agents active (Qur's process) | < 150 MB | 129 MB |
| Start to first frame | < 250 ms | 113 ms |
| Open a 10,000-line file, fully highlighted | < 100 ms | 84 ms |
| Agent write → glow on screen (Qur unfocused) | < 100 ms | 81 ms (max 84.5) |
| Keystroke handled and drawn (CPU side, p99) | ≤ 8.3 ms | 2.0 ms |
| Project search, 5,000 files | < 300 ms | 51 ms |
| Network connections from Qur itself | none | none (`lsof`) |

## How Qur compares

**Zed.** Qur shares Zed's GPUI and native feel, but it isn't a general-purpose editor with an agent panel. In Qur the agents are the main view; code appears in the heatmap, Live, the code modal and the review diff.

**Orca** ([stablyai/orca](https://github.com/stablyai/orca)) and **Superset** ([superset-sh/superset](https://github.com/superset-sh/superset)) are the closest peers: desktop apps that run several coding agents side by side in worktrees. We studied Orca closely ([docs/research/orca.md](research/orca.md)) and adopted several ideas: worktree setup commands, `.worktreeinclude`, review comments sent back to the agent, and Design-Mode-style inspect.

| | Qur | Orca | Superset |
|---|---|---|---|
| Tech | Rust + GPUI, native; ~29 MB app | Electron + React; ~254 MB app | Electron + React; ~431 MB download (arm64 .dmg) |
| License | GPL-3.0-or-later | MIT | Elastic License 2.0 (source-available) |
| Account | None | None | Sign-in (GitHub or Google) |
| Agents in isolated worktrees | Always, outside the repo | Yes | Yes, outside the repo |
| Agent link | Terminal or ACP (structured) | Terminal by default; structured chat experimental | Terminal by default; optional ACP chat |
| Live view of the agent's current file, with Take over | Yes | — | — |
| File-tree heatmap of agent activity | Yes | — | — |
| Permissions | No bypass flags; asked in a permission bar (structured Claude sessions start in the adapter's `auto` mode, which asks less) | Permission-bypass flags by default | Bypass flags by default for Claude Code and Codex |
| Shipping changes | Local Merge into your branch (index- and stash-safe) | Push and pull request | Commit, push and pull request |
| Browser + element inspect | System WebKit, freed when idle | Bundled Chromium | Bundled Chromium (Electron webview) |
| Telemetry / background daemon | None / none (`lsof` shows no sockets in Qur's process) | Opt-out telemetry / keeps terminals alive after quit | PostHog and Sentry / agents keep running after ⌘Q |

Superset also has cloud workspaces, a mobile app and Slack/Linear integrations; Qur stays local by design. Based on our reading of each project's repository and docs in October 2026; corrections welcome.

## Development

Read [HANDOFF.md](../HANDOFF.md) and [docs/decisions.md](decisions.md) first.

```sh
cargo test --release --workspace   # includes the theme contrast test and ACP tests against a fake agent
scripts/budget.sh 5                # weight budget; needs an unlocked screen (AC power recommended)
```

CI (`.github/workflows/ci.yml`, one macOS job) runs clippy with `-D warnings`, the tests, a release build, the 40 MB binary budget, and the `qur doctor` checks that don't need an agent.

### Scripted runs

`QUR_DEBUG_ACTIONS` drives a window after its first frame: comma-separated steps, 200 ms apart. Always use a scratch `QUR_HOME`.

| Step | Does |
|---|---|
| `qur::Name` | dispatches that action (as in the palette) |
| `wait:<ms>` | pauses |
| `type:<text>` | types the text as key presses: `\,` comma, `\n` Enter, `\t` Tab, `\\` backslash |
| `key:<keys>` | presses keystrokes, space-separated, keymap syntax: `key:cmd-shift-p`, `key:escape`, `key:cmd-\,` (Cmd+,) |
| `click:<at>` `dclick:` `rclick:` `cmdclick:` | left click, double click, right click, Cmd-click |
| `drag:<at>,<at>` | press, move there a frame at a time, release (tabs, dividers, sliders) |
| `scroll:<at>,<dy>` | turns the scroll wheel over a point by `dy` points (negative scrolls down) |
| `state` | prints `STATE …`: overlay (and its text), selected agent, code modal (file, line, Vim mode, text), terminal find and selection, Git panel, both dock layouts |
| `marks` | prints `BOUNDS <name> x,y wxh` for every marked element on screen |

`<at>` is `x,y` in window points, or a marked element: `@tab:Terminal:Live` (its middle) or `@pane:Terminal:root~0.9~0.5` (fractions of its size). Marks: `agent:<button id>` (e.g. `agent:tt-root-1`), `menu:<item>`, `tab:` / `close:` / `add:<Kind>:<Panel>`, `pane:` / `split:<Kind>:<path>` (`root`, then `a`/`b` per split), `term:<session>`, `term-cell:<session>` (one cell, so `@term-cell:1~4.5~0.5` is column 4 of the top row), `git-row:` / `git-act:<staged|unstaged>:<path>`, and in Settings `settings:nav:<Section>`, `settings:<row>:<choice>` (`settings:vim:on`, `settings:top:zen`, `settings:lights:mono`, `settings:theme-scope:project`), `settings:theme:<Auto|n>`, `settings:slider:<Opacity|Hue|Brightness>`, `settings:blur`, `settings:file:<name>`, `settings:close`, `settings:done`. Marks exist only in scripted runs. Each input step draws the window first, so it works while the window is covered; check `STATE`, `FLASH` and files on disk rather than screenshots.

`scripts/handcheck.sh` runs the hand checks this way (Vim keys, Find in Terminal, Rename, the agent menu, dock tabs and dividers, Cmd-click, the Git panel's stage, diff and typed commit, About, Accent Color, Run Doctor, window gaps with a dock divider and the sidebar gap, the sidebar's section order, auto-hide and side, Settings from `Cmd+,` saving Vim, a theme, opacity and blur, and the first-run welcome) and prints ✓/✕ per assertion. It needs a logged-in session but no agent: a plain shell stands in for Claude Code.

House rules:
- Lightness is the product: measure with `scripts/budget.sh`, don't estimate.
- No UI that isn't in the spec or decisions.
- Agents only ever use their own CLI logins; Qur never sets `ANTHROPIC_API_KEY`.

### Project layout

```
crates/
  qur_app       the app (binary `qur`, also `qur doctor`): window, sidebar, stage, Live, Browser, Changes, Git panel, Search, alerts, menus
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
  qur_search    project search and replace (ignore + regex)
docs/           spec (QUR_HANDOFF.md), owner decisions, keymap, menus, budget, signing, research
scripts/        bundle.sh (Qur.app), make_icns.swift, budget.sh, shot.sh, handcheck.sh
spikes/         M0 spikes and test fixtures
```

## License and credits

[GPL-3.0-or-later](../LICENSE). © PEUR Technologies.

Themes are ported from their upstream projects under their licenses ([docs/THEME_CREDITS.md](THEME_CREDITS.md)). Fonts, grammars and other components are listed in [docs/THIRD_PARTY.md](THIRD_PARTY.md). The UI font is Inter (SIL OFL).

**Trademark:** "Qur" has not been through trademark clearance. Don't register or rely on the name commercially until it has.
