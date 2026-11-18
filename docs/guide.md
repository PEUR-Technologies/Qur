# Qur guide

This guide covers what the [README](../README.md) leaves out. That means requirements, every feature, configuration, measurements, comparisons and development.

## Install

### Requirements

- macOS 13 or newer. Qur is developed on macOS 27, Apple Silicon.
- **git** 2.38 or newer. Merge uses `git merge-tree --write-tree`.
- Agents run on your own logins. Qur never sets an API key.
  - [Claude Code](https://claude.com/claude-code), logged in with your subscription (`claude` CLI). Agent Terminals need nothing else.
  - Structured (ACP) agents also need the adapter, pinned: `npm i -g @agentclientprotocol/claude-agent-acp@0.94.0`. Don't use `npx` at runtime.
  - Gemini CLI, opencode, GitHub Copilot and Cursor run over ACP. Aider runs in a terminal. Settings > Agents lists them once they're installed. To add any other CLI agent, use Settings > Agents > **Add agent** (name, command, Structured or Terminal), or edit `agents.toml` by hand.

### The app

```sh
git clone https://github.com/PEUR-Technologies/Qur.git && cd Qur
scripts/bundle.sh                      # -> target/release/Qur.app (~29 MB, ad-hoc signed for this Mac)
cp -R target/release/Qur.app /Applications/
ln -sf /Applications/Qur.app/Contents/MacOS/qur /usr/local/bin/qur   # optional: `qur <folder>` from a terminal
qur doctor                             # checks git, the agents and their adapters, then a real ACP initialize
```

Building needs **Rust** stable ([rustup](https://rustup.rs)) and **Xcode** with the Metal toolchain. If the build asks for it, run `xcodebuild -downloadComponent MetalToolchain` once. Release signing and notarization are in [docs/signing.md](signing.md). Notifications need the app bundle. Everything else also runs straight from `target/release/qur`.

On first launch the settings screen opens as a welcome. Connect an AI account there (install and sign in, decision 45). You can also pick the theme, accent, layout and keys. **Qur > Settings…** (`Cmd+,`) brings it back any time.

Then open a project with File > Open Folder, or run `qur ~/path/to/repo`. Press `Cmd+Shift+T` for an Agent Terminal, which runs Claude in its own worktree. Press `Cmd+Shift+N` for a structured agent task. Then describe the task.

### From source, without bundling

```sh
cargo build --release -p qur_app
target/release/qur ~/path/to/your/repo   # a folder or a file; no argument reopens the last project
```

## The window

One window is one project. There's no permanent editor area. The window is for agents and files.

- **Left column**
  - **AGENTS**: one button per agent. A button fills with the agent's colour while it's working. The selected agent has a coloured outline. Click a button to open its session.
  - **TERMINALS**: plain shells. This section shows only when you have some. To split a shell, use Terminal > Split (Dwindle) or press `Cmd+Shift+D` (down). The new shell starts in the project folder and shares the row. Move between panes with `Cmd+Opt+Arrows` or a click. `Cmd+Opt+Shift+Arrows` swaps panes. To resize, drag the divider or press `Cmd+Ctrl+Arrows`. `Cmd+Opt+J` flips a split between side by side and stacked. `Cmd+D` makes the focused pane fill the row. Press it again to go back. `Cmd+W` closes the focused pane. Agent Terminals split the same way.
  - **SOURCE CONTROL**: the branch and change count. Click it to open the Git panel.
  - **FILES**: the file tree. It's the only collapsible section. It shows the selected agent's worktree, or your checkout when no agent is selected.
- **Stage**: the rest of the window. It shows the selected session in panes you arrange like VS Code.
  - An Agent Terminal fills the stage as a real terminal. Use the pane's **+** to add Live, Browser or Changes beside it. `Cmd+5` adds the Browser. Inspect picks are pasted into its terminal with a screenshot file path.
  - A structured (ACP) agent shows Live, Browser and Changes next to its Chat and To-do. The Chat holds the permission bar and the prompt.
  - Drag a tab over another pane. You can also drag a whole pane by its tab strip, including the agent's own pane. A rounded preview shows where it will land, like macOS window tiling. Near an edge, it goes beside the pane on that side. On the middle or the tab strip, it joins the pane. Let go and the panes glide into their new places. Drag the dividers to resize. × tucks a tab away, and the pane's + menu brings it back. Qur saves your arrangement. Split terminals glide the same way when you split them (Split (Dwindle), `Cmd+Shift+D`), lay them out as a grid (`Cmd+Enter`) or close one.
- **Code modal**: opening a file shows it in a modal with full editing. `Esc` closes it. In an agent's worktree, the gutter marks the agent's changes. In your checkout, it marks yours against HEAD. Next/Previous Change opens a peek with the removed lines. Revert undoes one change at a time. Revert is undoable and never touches the index.
- **Files of any kind**: the modal detects the encoding (UTF-8, UTF-16, Windows-1252) and line endings. It shows them in its header and saves them back. File > Reopen with Encoding…, Convert to Encoding… and Convert Line Endings change them. Images open in the file viewer. Other binaries open in a hex view. Open as Text shows them as text anyway. Files over 50 MB (`[editor] large_file_mb`) open read-only and paged from disk, so a 1 GB log costs almost no memory. View > Show Whitespace shows whitespace. A file changed on disk reloads by itself. If you have unsaved edits, Qur asks first. File > Open File… (`⌘O`) opens any file, in or outside the project. Closed files are dropped from memory and reopen where you left them. Unsaved files are never dropped.
- **Files tree menu**: right-click a file for Open, Open in Default App, Reveal in Finder, Copy Path and Copy Relative Path. The File menu has the same items for the open file.
- **No project**: opening Qur with no folder, from Spotlight or the Dock, gives you a terminal. The empty window has a shell in your home folder and no sidebar. `cd` into a git repository, or pick a folder (Open Folder…, a recent project). That window then becomes the project's window, terminals and all. Closing the last project shows the same window with recent projects, Open Folder… and terminals. A folder without git opens with git features off.
- **File viewer**: an image (PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, SVG) opens in its own modal. The modal fits the image and never enlarges it. It shows the size in pixels and bytes. A file that isn't text shows its name and size instead of an error. Both offer **Open in Default App** and **Reveal in Finder**. `Esc` or a click outside closes the viewer.
- **Window gaps**: turn them on with View > Toggle Gaps. Adjust them with View > Appearance > Gaps…, or with the row in Setup. As in Hyprland, the top bar, the sidebar, every dock pane and every split terminal float as separate tiles. Clear gaps sit between them and around the window's edge, so Qur looks like a stack of separate windows. The popover sets the gap (2–24 pt), rounded corners and their radius (0–16 pt), and a hairline border per tile. It previews as you drag and saves when it closes. Drag a gap between panes, or between the sidebar and the stage, to resize. The gaps show your desktop even at 100% opacity. Below 100% with blur on, the gaps are blurred too, because macOS blurs the whole window. With zen mode also on, the top bar's tile slides down at the top edge and pushes the tiles below it down.
- **Sidebar**: use View > Sidebar (`Cmd+B`), View > Auto-hide Sidebar and View > Sidebar on Right. The Sidebar rows in Settings > Appearance do the same. The sidebar can be always shown, auto-hidden, or hidden until `Cmd+B`. When auto-hidden, it slides in when the mouse reaches its edge of the window and pushes the stage over. It slides away once the mouse leaves it, like Chrome's vertical tabs. It can sit on the left or the right. Drag a section header (AGENTS, TERMINALS, SOURCE CONTROL, FILES) onto another to reorder the sections. Qur saves these as `[ui] sidebar`, `sidebar_side` and `sidebar_sections`.

## Features

### Agents

- **Isolated worktrees.** Each task gets a git worktree outside your repo, on branch `qur/<agent>/<task>`. Your checkout stays untouched. You can run several agents at once.
- **Agent Terminals** (`Cmd+Shift+T`): the interactive `claude` CLI in its own worktree. It gets the sidebar button, heatmap, review and merge like any other task. Other CLI agents work too. Add one in Settings > Agents (**Add agent**, Terminal) or in `agents.toml` with `mode = "terminal"`.
- **Agents name their own work.** A button shows what the agent says it's doing. For Claude Code, that's its terminal title. To rename it yourself, use Agent > Rename Agent… or double-click.
- **Terminal comforts:** Find (`Cmd+F`) searches the scrollback and supports regex. `Cmd`-click opens URLs and `file:line` paths.
- **Structured sessions** run over the [Agent Client Protocol](https://agentclientprotocol.com):
  - You see streaming tool calls, the agent's to-do plan, and a permission bar (Allow once / Allow for this task / Deny). When the agent asks several things at once, the bar shows them one at a time ("1 of 3 requests waiting"). **Allow all 3 once** answers them all "allow once", never "always". Agent > Permission: Allow All Pending does the same.
  - Tool calls in a row fold into one line, like "Read 4 files, ran 2 commands ▸". Click it to see each call. A call that's running, failed or waiting for your permission stays in view.
  - **Find in Chat** (`Cmd+F` in the agent's view) searches your messages, the agent's replies and thoughts, and tool titles and output. It ignores case. `↩` goes to the older match and `⇧↩` to the newer one. `Esc` closes it. A folded thought, tool call or group opens to show the match.
  - **Copy Chat as Markdown** is in the Agent menu and the palette, or right-click the agent. It puts the whole chat on the clipboard. Your messages are quoted. The agent's Markdown stays as it wrote it. Each tool call gets one line with ✓ / ✕. Thoughts are left out.
  - You can stop the agent at any time. Messages you send mid-turn are queued. `@` mentions a file and `/` runs agent commands.
  - **Files and images:** paste (`Cmd+V`) files copied in Finder, or drop them on the prompt. They become chips above it. A pasted screenshot or copied image does too. Images go to the agent as images. Images over the 5 MB limit are scaled down, and TIFF and BMP are sent as PNG. Any other file or folder goes as a link the agent reads itself. Click a chip, or a thumbnail or file under a sent message, to see it in the file viewer. Text files open in the code modal. × removes a chip. In the New Agent Task dialog, attached files become mentions.
- **Plan mode** (Agent > Plan Mode `⌥⌘P`, or "Plan first" in New Agent Task): the agent plans before it edits. Its plan opens in place of the chat with numbered lines. Click a line to comment. Then click **Send Comments** to have it revise, or **Approve** to return to the mode you were in.
- **Usage:** the agent's header shows tokens and cost when the adapter reports them ("121.2k tokens · $0.12"). On a subscription, the cost is the adapter's figure.
- **Idle agents let go of their memory.** Qur stops the processes of a structured agent you haven't looked at once it's idle for 10 minutes. That frees about 220 MB each for Claude. Select the agent or send it a message and the session resumes where it was. Set `[agents] idle_release` in minutes, or 0 for never. Qur never stops agents that are working, waiting for you or holding queued messages. It never stops Agent Terminals.
- **Chats are kept.** A structured agent's chat survives a restart. Only its last few hundred rows stay in memory. **Show earlier messages** loads the rest from disk.
- **Unread markers** appear on agents that finished or need you while you were elsewhere.
- **Turn-end checks.** Set `[checks] command` per project. Qur runs it in the worktree after each agent turn that changed files. A failure marks the agent's button `✕ checks`. The failure also lands in the prompt, or the Agent Terminal's input line, for you to send back. Agent > Run Checks Now runs the check any time.

### Watching

- **Live file-tree heatmap.**
  - Files waiting for your approval get an outline. Files being edited get a pulsing glow. Files being read get a fading glow. Changed files show "new" or `+n −m`.
  - Heat deepens with how often a file was touched. Folders roll up their contents.
  - Files the agent deleted stay in the tree, struck through and dimmed with their glow. They stay until they come back or you merge or discard the task.
  - The heatmap follows the same ignore rules as the tree: `.gitignore` and `.ignore` files in any folder, `.git/info/exclude` and your global git excludes.
  - Your own saves from the code modal and Replace All never glow as an agent's edit.
  - An agent working in the project folder (the first Agent Terminal) shares the folder with you. Only files it touched glow. Writes count as its own while it's busy, meaning an Agent Terminal printing output or a structured agent running a tool. Your uncommitted work stays quiet.
  - On your main checkout, dots show which files agents changed in their worktrees.
- **Live view** (structured agents): shows the file the agent is working in, as it types. It's read-only while the agent edits. **Take over** lets you step in. It follows the agent across files. It never jumps within 5 s of your own scroll or click.
- **Browser with Inspect.** Open your dev server next to the agent. Pick elements or text and ask the agent to adjust them. The agent gets the selector, HTML, key CSS, the source location when available, and a screenshot. The Browser uses the system WebKit and frees it when idle.
- **Run Project in Worktree.** This starts your dev server in the agent's worktree on its own port. It opens the server in that agent's Browser, so parallel agents never collide. It runs on localhost only. It stops on Discard, Merge cleanup or quit.

### Review and merge

- Review the task's diff and comment on lines. Your comments go back to the agent as one message.
- **Turns** (structured agents): each turn that changed files ends with a line like "3 files · +12 −4 · View · Rewind".
  - **View** shows only that turn's changes in Changes. Comment as usual. **All changes** goes back.
  - **Rewind** asks first, then puts the worktree's files back to how they were before that turn. It stops the agent first if it's working. It warns you if files changed since the turn ended. It tells the agent with your next message. Commits stay.
  - Agent > Show Last Turn's Changes and Rewind Last Turn… do the same.
  - Qur keeps a snapshot at the start and end of each turn. Each snapshot is a git tree in the task's worktree, pinned under `refs/qur/turns/<task>/`. It's deleted on Discard or after Merge. Your own checkout, index and stash are never touched.
  - Agent Terminals get turns too. Each busy-then-quiet stretch that changes files counts as one. Their Changes tab lists them. Rewind waits until the agent is quiet. Then it types a one-line note into the agent's input line but doesn't send it.
- **Second opinion** (structured agents): Agent > **Review Agent's Work…** starts a reviewer on the task. When the agent has a plan, the item is **Review Agent's Plan…**.
  - Pick who reviews with Up/Down or a click. It can be another agent from your roster, such as Codex reviewing a Claude task or the reverse. It can also be another model of the same agent. The default is the other account when it's installed and signed in. Otherwise it's another model. An optional line tells the reviewer what to look at.
  - The reviewer is a second session in the same worktree. It gets the task's diff and a summary of its chat, both capped in size. It answers with findings, each with a **Criticality N/10**.
  - It shows as `↳ review · codex` under the agent in the sidebar. Click it to watch it in the chat panel.
  - It's **read-only by Qur's rule, not by an OS sandbox**. Qur denies every edit or command it asks to run. There's no permission bar, and the chat says what was denied. Qur starts it in its plan or read-only mode.
  - When it's done, **Send Findings to Agent** puts a reply request into the agent's prompt. For each finding, the request asks for Agree/Disagree, Why and Fix, then asks before changing anything. You edit and send it. It queues if the agent is busy.
  - A review costs one more session on the reviewer's own account.
  - Qur saves the reviewer with the task. Click it after a restart to see its findings again. It goes away on Close, Discard or Merge.
- A pre-check warns about conflicts. **Merge** never touches your staged changes or stash.
- **Push and Open Pull Request** is in the Agent menu and next to Merge. It pushes the task's branch with your git. It opens a pull request with your `gh`, or GitHub's compare page without it. The link stays on the task. Remote branches are never deleted.
- Resolve conflicts in the app with Accept Ours / Theirs / Both, then Finish or Abort. Agent > Merge Conflicts has the same. Stage and commit in the Git panel.
- Discard removes the task's worktree and branch.
- **Commit messages** are written from the agent's turns. The Git panel's box is prefilled, and Merge uses it.
- **Clean Up Worktrees…** (Agent menu) lists every worktree and `qur/*` branch with its size. Leftovers are preselected. Anything with unmerged commits or unsaved changes asks twice.

### Search and alerts

- **Project search** (`Cmd+Shift+F`) searches the tree you're looking at, either an agent's worktree or your checkout. It takes about 50 ms for 5,000 files. It has case, whole-word and regex options and Replace All.
- **Alerts:** when an agent finishes, errors or needs approval while you're elsewhere, you get a notification, a Dock badge and a soft chime. Click the notification to jump to the agent. You can turn each one off in `config.toml` (`[alerts]`).
- **Crash reports** stay on your Mac in `~/Library/Logs/Qur/crash-*.txt`. Qur keeps the newest 10. After a crash, the next launch offers View Report, Report a Problem or Dismiss. Report a Problem opens the issue page. Nothing is uploaded.
- **Memory:** About and `qur doctor` show what Qur, its agents and terminals use. They also show how much disk the worktrees take.

### Editor and looks

- The code modal has tree-sitter highlighting for 13 languages, multi-cursor, regex find/replace, soft wrap and VS Code keybindings. Vim mode is optional (Edit > Vim Mode, or Settings).
- **Go to Symbol in File** (`Cmd+Shift+O`) lists functions, methods, types, classes, headings, keys and more. It uses the same tree-sitter parse as the colors, with no language server. Arrow keys preview each symbol. Esc puts the cursor back.
- **Expand / Shrink Selection** (`Ctrl+Shift+Cmd+→` / `←`) grows every cursor's selection to the enclosing syntax node: word, call, statement, block, function and so on. It shrinks back one step at a time. In files Qur doesn't parse, it grows by word, then line, then the whole file.
- With soft wrap on, `End` / `Cmd+→` stops at the end of the visual row first, then the line. `Home` / `Cmd+←` works the same way.
- Input methods work, including Japanese, Chinese, Korean and press-and-hold accents. The text being composed shows underlined at the cursor until you commit it.
- **Settings** (`Cmd+,`) is one screen with a section list.
  - Agents: install, sign in, choose which agents to use, and Add agent for any other CLI. Unticking an agent removes it from agents.toml.
  - Appearance: theme for this project or all, accent color and brightness, transparency and blur, top bar, and traffic lights in full screen.
  - Editor: Vim mode and keys.
  - Layout: the agent stage presets as diagrams.
  - Files: open config.toml, keymap.toml and agents.toml.

  Changes apply and save as you make them. Esc closes the screen. It also opens by itself on first launch.
- Qur has 17 built-in themes plus Auto, which follows macOS. You can set a theme per project, and window transparency with blur. Every theme passes a contrast test.
- Every command is in the macOS menu bar and the command palette (`Cmd+Shift+P`). There are no toolbars. The one settings screen is Qur > Settings…. Its settings are also menu commands (View > Appearance, Edit > Vim Mode).

## Everyday keys

| Keys | |
|---|---|
| `Cmd+Shift+T` / `Cmd+T` | New Agent Terminal / new shell |
| `Cmd+Shift+N` | New structured agent task |
| `Ctrl+T`, `Ctrl+1..9`, `Ctrl+Opt+↑/↓` | Switch sessions (`Ctrl+T` picks one by name) |
| `Cmd+I`, `Cmd+.` | Focus the prompt, stop the agent |
| `Cmd+P`, `Cmd+Shift+P` | Go to file, command palette |
| `Cmd+Shift+O`, `Ctrl+Shift+Cmd+→/←` | Go to symbol in file; expand / shrink selection |
| `Cmd+K Cmd+L`, `Cmd+1..5` | Structured agents: cycle layouts; Live / To-do / Chat / Changes / Browser (`Cmd+5` works for Agent Terminals too) |
| `Cmd+Shift+C` | Browser: inspect element |
| `Ctrl+Shift+G` | Source control |
| `Cmd+K Cmd+T` | Theme (this project or all projects) |

The full list is in [docs/keymap.md](keymap.md). The menu bar is in [docs/menus.md](menus.md).

## Configuration

Everything lives in `~/Library/Application Support/Qur/`. Qur never writes into your repo. Only agents do, in their worktrees.

| File | What |
|---|---|
| `config.toml` | Theme, window opacity/blur, editor font, soft wrap, peek layout, `[keys] vim`, `[alerts]`, and window gaps: `[ui] gaps = true`, `gap = 8`, `rounded = true`, `radius = 8`, `tile_border = false`. Qur > Settings… changes most of it. Qur > Settings Files > Open config.toml opens it. Saved changes apply right away. |
| `agents.toml` | Which agents Qur can start (ACP or terminal mode). Written on first launch. |
| `keymap.toml` | Your own key bindings (see below). Open it with Qur > Settings Files > Open keymap.toml or from Settings > Editor. Saved changes apply right away. |
| `projects/<id>.toml` | Per-project theme, opacity, `[worktree] setup = "pnpm install"` (runs in each new worktree before the agent starts), `[checks] command = "cargo check"` (runs after each agent turn that changed files), and `[run] command` / `url` with `$PORT` (Run Project). |
| `projects/<id>.tasks.toml` | Tasks, so they survive restarts and resume their sessions. |
| `worktrees/<id>/<task>/` | The task worktrees. |

In your repo, an optional **`.worktreeinclude`** file lists one path per line, for example `.env`, `node_modules` or `apps/*/.env`. A leading `/` means the repo root, as in `.gitignore`. Qur copies those paths into each new worktree with APFS copy-on-write. The copy is instant and takes no extra disk. Qur leaves alone any files the worktree already has.

### Keymap

`keymap.toml` rebinds commands by name, under `[bindings]`:

```toml
[bindings]
"Go to Symbol in File" = "cmd-r"                        # replaces Cmd+Shift+O
"editor::DeleteLine" = ["cmd-shift-k", "ctrl-shift-k"]  # several keys
"View: Sidebar" = ""                                     # unbound
```

- **Command names:** use the action (`qur::GoToSymbol`, `editor::DeleteLine`) or the command palette entry (`Go: Go to Symbol in File…`). `QUR_DEBUG_ACTIONS` uses the same action names. You can also use the title alone when only one command has it. Case and a trailing `…` don't matter.
- **Keys:** the modifiers are `cmd`, `ctrl`, `alt` (Option) and `shift`. Join them with `-` before the key: `cmd-shift-o`, `` ctrl-` ``, `alt-up`, `f5`. A chord is two keystrokes with a space: `cmd-k cmd-s`. A list binds several keys. `""` or `[]` unbinds.
- A binding replaces all of that command's default keys. It keeps where the command works: the editor, a terminal or anywhere.
- Qur skips entries it can't use, such as an unknown or ambiguous name, a malformed key, or a value that isn't a string or list. It writes them to the log (Help > Show Logs). When you save the file in Qur, it also shows them in the top bar. The rest still apply. Qur never fails to start over keymap.toml.
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

**Zed.** Qur shares Zed's GPUI and native feel. Zed is a general-purpose editor with an agent panel. Qur isn't. In Qur the agents are the main view. Code appears in the heatmap, Live, the code modal and the review diff.

**Orca** ([stablyai/orca](https://github.com/stablyai/orca)) and **Superset** ([superset-sh/superset](https://github.com/superset-sh/superset)) are the closest peers. Both are desktop apps that run several coding agents side by side in worktrees. We studied Orca closely ([docs/research/orca.md](research/orca.md)). We adopted several of its ideas: worktree setup commands, `.worktreeinclude`, review comments sent back to the agent, and Design-Mode-style inspect.

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

Superset also has cloud workspaces, a mobile app and Slack/Linear integrations. Qur stays local by design. This comparison is based on our reading of each project's repository and docs in October 2026. Corrections are welcome.

## Development

Read [HANDOFF.md](../HANDOFF.md) and [docs/decisions.md](decisions.md) first.

```sh
cargo test --release --workspace   # includes the theme contrast test and ACP tests against a fake agent
scripts/budget.sh 5                # weight budget; needs an unlocked screen (AC power recommended)
```

CI (`.github/workflows/ci.yml`, one macOS job) runs clippy with `-D warnings`, the tests and a release build. It also checks the 40 MB binary budget and runs the `qur doctor` checks that don't need an agent.

### Scripted runs

`QUR_DEBUG_ACTIONS` drives a window after its first frame. It takes comma-separated steps, run 200 ms apart. Always use a scratch `QUR_HOME`.

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

`<at>` is `x,y` in window points, or a marked element. `@tab:Terminal:Live` targets its middle. `@pane:Terminal:root~0.9~0.5` uses fractions of its size.

These marks exist:

- `agent:<button id>`, for example `agent:tt-root-1`.
- `menu:<item>`.
- `tab:` / `close:` / `add:<Kind>:<Panel>`.
- `pane:` / `split:<Kind>:<path>`. The path is `root`, then `a`/`b` per split.
- `term:<session>`.
- `term-cell:<session>`, one cell. So `@term-cell:1~4.5~0.5` is column 4 of the top row.
- `git-row:` / `git-act:<staged|unstaged>:<path>`.
- In Settings: `settings:nav:<Section>`, `settings:<row>:<choice>` (`settings:vim:on`, `settings:top:zen`, `settings:lights:mono`, `settings:theme-scope:project`), `settings:theme:<Auto|n>`, `settings:slider:<Opacity|Hue|Brightness>`, `settings:blur`, `settings:file:<name>`, `settings:close`, `settings:done`.

Marks exist only in scripted runs. Each input step draws the window first, so it works while the window is covered. Check `STATE`, `FLASH` and files on disk. Don't rely on screenshots.

`scripts/handcheck.sh` runs the hand checks this way and prints ✓/✕ per assertion. It covers Vim keys, Find in Terminal, Rename, the agent menu, dock tabs and dividers, and Cmd-click. It covers the Git panel's stage, diff and typed commit. It covers About, Accent Color and Run Doctor. It covers window gaps with a dock divider and the sidebar gap, and the sidebar's section order, auto-hide and side. It covers Settings from `Cmd+,` saving Vim, a theme, opacity and blur, and the first-run welcome. It needs a logged-in session but no agent. A plain shell stands in for Claude Code.

House rules:
- Lightness is the product. Measure with `scripts/budget.sh`. Don't estimate.
- No UI that isn't in the spec or decisions.
- Agents only ever use their own CLI logins. Qur never sets `ANTHROPIC_API_KEY`.

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
