# Qur keymap (v1)

Base: VS Code's default macOS keymap, minus features Qur doesn't have (LSP, file tabs, extensions), plus Qur's own commands. Every binding is also a **command** in the command palette (`Cmd+Shift+P`) and, where macOS expects it, a **menu item**, so it is discoverable and shows its shortcut.

Spike tier: **S** = implemented in the M0 toolkit prototypes; **M1+** = built with the real editor.

## App and window

| Keys | Command | Menu | Tier |
|---|---|---|---|
| `Cmd+Q` | Quit Qur | Qur | S |
| `Cmd+H` / `Cmd+Opt+H` | Hide Qur / Hide Others | Qur | S |
| `Cmd+,` | Settings… (agents, appearance, editor, layout, files; changes apply and save at once, Esc closes) | Qur | ✅ |
| `Cmd+N` | New Window | File | M1+ |
| `Cmd+Shift+N` | New Agent Task (same as sidebar `+`) | File | M2 ✅ |
| `Cmd+O` | Open File…: any file, also outside the project (text in the code modal; images, binaries and large files in the file viewer) | File | ✅ |
| `Cmd+K Cmd+O` | Open Folder… (was `Cmd+O`) | File | ✅ |
| `Cmd+Opt+O` | Switch Project… (also: click the project name in the top bar) | File | M1 |
| `Cmd+Shift+W` | Close Window | File | S |
| `Cmd+M` | Minimize | Window | S |
| `Cmd+\` (from any app) | Show or hide Qur: `[keys] global` in config.toml or Settings > Editor; keys macOS uses are refused | — | ✅ |
| `Ctrl+Cmd+F` | Toggle Full Screen | View | S |
| `Ctrl+Cmd+Z` | Zen Mode (hide the top bar until the mouse nears the top) | View | ✅ |
| `Cmd+Shift+P` / `F1` | Command Palette | View | S |
| `Cmd+K Cmd+T` | Theme Picker | View > Appearance | M1 |
| `Cmd+B` | Toggle Sidebar | View | S |
| `Cmd+=` / `Cmd+-` / `Cmd+0` | Zoom In / Out / Reset (UI + code font) | View | S |

## Files and navigation

| Keys | Command | Tier |
|---|---|---|
| `Cmd+P` | Quick Open file (fuzzy) | M1 |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Recent files (MRU) | M1 |
| `Cmd+S` | Save | S |
| `Cmd+Opt+S` | Save All | M1 |
| `Cmd+Shift+E` | Reveal in Files (opens the Files dropdown) | S |
| `Ctrl+G` | Go to Line… | S |
| `Ctrl+-` / `Ctrl+Shift+-` | Navigate Back / Forward | M1 |
| `Cmd+Shift+O` | Go to Symbol in File (tree-sitter outline, no LSP; arrows preview, Esc goes back) | ✅ |
| File menu / palette | Open as Text: the file viewer's file (an image, a binary) in the code modal, decoded as Windows-1252 when it isn't text; read-only if saving couldn't give the same bytes back. A large file opens as paged text in the viewer (no default keys) | ✅ |
| File > Encoding / palette | Reopen with Encoding… / Convert to Encoding… (UTF-8, UTF-8 BOM, UTF-16 LE, UTF-16 BE, Windows-1252; converting marks the file unsaved, `Cmd+Z` undoes), Convert Line Endings (LF ⇄ CRLF, undoable). The code modal's header shows the encoding and line endings (no default keys) | ✅ |
| View menu / palette | Show Whitespace: spaces as ·, tabs as →, line ends as ¬ in the code modal, saved as `[editor] show_whitespace` (no default keys) | ✅ |
| File viewer (hex view) | Scroll, arrows, `PageUp`/`PageDown`, `Cmd+Up`/`Cmd+Down`; `Ctrl+G` Go to Offset (`0x1F00` or `7936`); drag or Shift-click to select bytes, `Cmd+A` all, `Cmd+C` copies them as hex; `Esc` closes the offset bar, then the viewer | ✅ |
| File viewer (large file) | Read-only paged text: scroll and keys as in the hex view, `Ctrl+G` Go to Line, click or Shift-click lines and `Cmd+C`, `Cmd+F` Find (streams through the file; `Enter` / `Shift+Enter` or `Cmd+G` / `Cmd+Shift+G` next / previous; counts every match) | ✅ |

## Reviewing changes in the code modal

A file in an agent's worktree (the modal says "in <task>") shows the agent's changes in the gutter: a green bar beside added lines, an amber bar beside changed ones, a red tick where lines were removed, all against the task's base commit (or, with Edit > Changes > Last Turn Only, against the snapshot its last turn started from). Your own files in the main checkout get the same markers against HEAD; untracked files get none. The markers catch up a moment after you stop typing.

| Keys | Command | Tier |
|---|---|---|
| `Opt+F5` / `Shift+Opt+F5` | Next / Previous Change (wraps around; VS Code's keys) | ✅ |
| `Opt+F3` (or click a marker) | Show Change: a peek under the change with the removed lines and ‹ › Revert Accept ✕; again to close | ✅ |
| Edit > Changes / palette / peek | Revert Change: put the base's lines back in the buffer as one undoable edit (`Cmd+Z` brings the agent's back; save to keep it; the index is never touched) | ✅ |
| Edit > Changes / palette / peek | Accept Change: mark it reviewed; its marker goes until its text changes again (for this session), and the cursor moves to the next change | ✅ |
| Edit > Changes | Last Turn Only (checkmark): a task's markers show what its last turn changed, not everything since it started | ✅ |
| `Shift+Opt+Cmd+C` | Copy Relative Path of the file in the code modal (relative to its checkout: the project or the task's worktree; VS Code's key) | ✅ |
| File menu / palette | Reveal in Finder, Copy Path (the file in the code modal). No default keys: VS Code's `Opt+Cmd+R` and `Opt+Cmd+C` are Qur's find toggles | ✅ |
| Right-click a Files row | Open, Open in Default App (files), Reveal in Finder, Copy Path, Copy Relative Path (relative to the tree's root); a deleted file's row is greyed out. Drag a row out of the window to drop the file in Finder or another app | ✅ |

## Editing

| Keys | Command | Tier |
|---|---|---|
| `Cmd+Z` / `Cmd+Shift+Z` | Undo / Redo | S |
| `Cmd+X` / `Cmd+C` / `Cmd+V` | Cut / Copy / Paste (the whole line when the selection is empty) | S |
| `Cmd+A` | Select All | S |
| `Shift+arrows` | Extend selection | S |
| `Opt+Left/Right` | Word left/right (`Shift` extends) | S |
| `Cmd+Left/Right`, `Home`/`End` | Line start (smart home) / end (`Shift` extends); with soft wrap, the visual row's start / end first, pressed again the line's | S |
| `Cmd+Up/Down` | File start / end (`Shift` extends) | S |
| `PageUp/PageDown`, `Fn+arrows` | Page up/down | S |
| `Opt+Backspace` / `Opt+Delete` | Delete word left / right | S |
| `Cmd+Backspace` | Delete to line start | S |
| `Cmd+Shift+K` | Delete line | S |
| `Opt+Up/Down` | Move line up/down | S |
| `Shift+Opt+Up/Down` | Copy line up/down | S |
| `Cmd+Enter` / `Cmd+Shift+Enter` | Insert line below / above | S |
| `Cmd+]` / `Cmd+[` | Indent / Outdent | S |
| `Tab` / `Shift+Tab` | Indent (2 or 4 spaces by file) / Outdent | S |
| `Cmd+/` | Toggle line comment (per language) | S |
| `Shift+Opt+A` | Toggle block comment | M1 |
| `Cmd+L` | Select line (repeat to extend) | S |
| `Ctrl+Shift+Cmd+Right` / `Ctrl+Shift+Cmd+Left` | Expand / Shrink Selection by tree-sitter node (every cursor; shrink retraces the expands) | ✅ |
| `Cmd+D` | Add selection to next match (multi-cursor) | M1 |
| `Cmd+Shift+L` | Select all matches | M1 |
| `Opt+Click` / `Cmd+Opt+Up/Down` | Add cursor | M1 |
| `Esc` | Close a change's peek / collapse to one cursor / close find | S |
| `Shift+Cmd+\` | Jump to matching bracket | M1 |
| `Cmd+K Cmd+X` | Trim trailing whitespace | M1 |
| `Opt+Z` | Toggle soft wrap | S |

`Cmd+Shift+L` stays Select All Matches (VS Code). Peek-layout cycling moved to `Cmd+K Cmd+L` (owner decision 10).

## Find

| Keys | Command | Tier |
|---|---|---|
| `Cmd+F` | Find | S |
| `Cmd+Opt+F` | Replace | S |
| `Cmd+G` / `Cmd+Shift+G` (and `Enter` / `Shift+Enter` in the find bar) | Next / previous match | S |
| `Cmd+E` | Use selection for find | S |
| `Opt+Cmd+C` / `Opt+Cmd+W` / `Opt+Cmd+R` | Toggle case / whole word / regex | M1 |
| `Cmd+Shift+F` | Search in project | M6 |
| `Cmd+Shift+H` | Replace in project | M6 |

## Terminal (owner decisions 4)

| Keys | Command | Tier |
|---|---|---|
| `Cmd+T` | New terminal | S |
| `Cmd+Shift+T` | New Agent Terminal: a Claude agent task in its own worktree, shown as a terminal (decisions 12, 17) | S |
| `Cmd+W` | Close focused terminal (or split pane); on the last shell, close the window; in an agent, Close/Delete Agent… (asks first) | ✅ |
| `Cmd+D` / `Cmd+Shift+D` (terminal focused) | Split right / split down: a new shell in the project folder beside the focused one (decision 39) | S |
| `Cmd+Opt+Arrows` (terminal focused) | Focus the split pane in that direction. Moving the pointer onto a pane focuses it too (`[terminal] focus_follows_mouse`); a still pointer never undoes a pane picked with the keys | S |
| `Cmd+Opt+Shift+Arrows` (terminal focused) | Move Pane: swap the focused pane with the one in that direction | ✅ |
| `Cmd+Enter` | New Tiled Terminal: a shell tiled into the terminal on screen, splitting the focused pane along its longer side (like i3/Hyprland); with no terminal shown, a new one. Insert Line Below in the editor | S |
| `` Ctrl+` `` | Toggle terminal / focus it | S |
| `Cmd+K` (terminal focused) | Clear terminal | S |
| `Cmd+C` / `Cmd+V` (terminal focused) | Copy selection / paste (Ctrl+C still sends SIGINT) | S |
| `Ctrl+Tab`-style `Cmd+Shift+[` / `Cmd+Shift+]` | Previous / next terminal | S |
| `Opt+Left/Right` | Word jump (sends `ESC b` / `ESC f`) | S |
| `Cmd+Left/Right` | Line start/end (sends `Ctrl+A` / `Ctrl+E`) | S |
| `Cmd+Backspace` | Kill line (`Ctrl+U`) | S |
| `Shift+Tab` | Back tab (`ESC [ Z`): Claude Code cycles its modes (default / accept edits / plan) | S |

## Panels (handoff section 7)

| Keys | Command | Tier |
|---|---|---|
| `Ctrl+Shift+G` | Source Control (Git panel: stage, unstage, diff, commit; merge in progress: Finish / Abort) | M4 ✅ |
| `Cmd+Shift+F` | Search panel | M6 |

## Agents and sessions (M2)

| Keys | Command | Tier |
|---|---|---|
| `Cmd+Shift+N` | New agent task (dialog: agent + task; `@` mentions a file) | M2 ✅ |
| `Cmd+Shift+T` | New Agent Terminal | M2 ✅ |
| `Ctrl+1..9` | Select session N in the sidebar | M2 ✅ |
| `Ctrl+Opt+Up/Down` | Previous / next session | M2 ✅ |
| `Ctrl+T` | Switch to an agent or terminal by name (not subagents); works in terminals too, so the shell doesn't get Ctrl+T | ✅ |
| `Cmd+I` | Focus the prompt bar | M2 ✅ |
| `/` (not typing) | Focus Input Field: the agent's prompt, the search box or the terminal, like a browser's search shortcut | ✅ |
| `Enter` / `Shift+Enter` | Send (queues during a turn) / newline | M2 ✅ |
| `Esc` (prompt) / `Cmd+.` | Stop the agent's turn (`session/cancel`) | M2 ✅ |
| `@` / `/` in the prompt | Mention a file / pick an agent slash command | M2 ✅ |
| `Cmd+Y` / `Cmd+Opt+Y` / `Cmd+Opt+Z` | Permission: Allow Once / Allow for This Task / Deny (Zed's keys), for the selected agent's request, else the first agent waiting; each works only when the agent offers that kind of answer, and its button shows the key. Not while the code modal, a terminal or a picker has focus (Agent menu too) | ✅ |
| Agent menu / palette / permission bar | Permission: Allow All Pending: when several requests are queued, answer them all "allow once" (never "always"; a request without that option stays). The bar says "1 of N requests waiting" and has an **Allow all N once** button. No default keys; disabled when nothing is waiting | ✅ |
| `Up` (empty prompt) | Pull the last queued message back into the prompt to edit it (each queued message also has Send now / Edit / ✕) | ✅ |
| `Cmd+Z` / `Shift+Cmd+Z` (prompt, text fields) | Undo / redo in the prompt and the one-line fields (find, replace, go to line, pickers, address bar); typing groups into one step until a pause or a caret move | ✅ |
| `Shift+Tab` (chat agent) | Next mode (Default / Accept Edits / Plan…), from the agent's session settings; click the Mode or Model chip above the prompt to choose | ✅ |
| `Cmd+Opt+P` | Plan Mode: the selected chat agent into its plan (read-only) mode, or back to the mode it was in; in the New Agent Task dialog, Plan first (the task's first message goes in plan mode). When the agent proposes a plan it opens in place of the chat: click a line to comment, then **Send comments** (it keeps planning) or **Approve** (it starts, back in its earlier mode). Agent menu: Comment on Plan…, Approve Plan, Send Plan Comments (no default keys) | ✅ |
| Agent menu | Discard Task…, Reveal Worktree in Finder, Open Task Worktree in Terminal | M2 ✅ |
| `Cmd+K Cmd+L` | Cycle peek layout | M3 ✅ |
| `Cmd+1..4` | Live / To-do / Chat / Changes (layout B) | M3 ✅ |
| `Cmd+5` | Browser: show the selected agent's browser (any layout; Agent Terminals too) | Browser ✅ |
| `Cmd+Shift+C` | Browser: Inspect Element (highlight, click to attach to the prompt, or paste into an Agent Terminal; select text to attach it) | Browser ✅ |
| `Cmd+R` / `Cmd+L` (browser) | Reload the page / address bar (its text selected) | Browser ✅ |
| `Cmd+[` `Cmd+]` or `Cmd+Left/Right` (browser) | Back / Forward | Browser ✅ |
| `Cmd+=` `Cmd+-` `Cmd+0` (browser) | Page zoom in / out / actual size (instead of Qur's zoom while the browser has focus) | Browser ✅ |
| `Cmd+C/X/V/A/Z`, `Shift+Cmd+Z` (in a page) | Copy, cut, paste, select all, undo, redo in the page | Browser ✅ |
| Changes view / Agent menu | Merge into My Branch, Keep Working, Discard Task…, Review Changes | M4 ✅ |
| Changes view / Agent menu | Push and Open Pull Request…, Open Pull Request (your own `git` and `gh`) | ✅ |
| Agent menu | Run Checks Now, Run Project in Worktree (no default keys) | ✅ |
| Agent menu / chat turn line | Show Last Turn's Changes, Rewind Last Turn… (structured agents; no default keys) | ✅ |
| `Cmd+F` (in a terminal) | Find in Terminal (regex; ↩ older, ⇧↩ newer, Esc closes) | M5 ✅ |
| `Cmd+F` (in a structured agent's view) | Find in Chat: your messages, the agent's replies, thoughts, tool titles and output (ignores case); ↩ older, ⇧↩ newer, Esc closes. The current match's folded thought, tool call or group opens | ✅ |
| Agent menu / palette / right-click an agent | Copy Chat as Markdown (no default keys) | ✅ |
| `Cmd`-click (in a terminal) | Open a URL, or `path:line:col` in the code modal | M5 ✅ |
| Agent menu / double-click a button | Rename Agent… | M5 ✅ |
| `Cmd+Shift+F` / `Cmd+Shift+H` | Find / Replace in Project (Search view; ⌥C case, ⌥W word, ⌥R regex) | M6 ✅ |
| Edit > Vim Mode (also Settings > Editor) | Vim keys in the code modal: normal / insert / visual / visual line | M6 ✅ |
| Qur > Settings Files | Open config.toml, keymap.toml, agents.toml (no default keys; `Cmd+,` opens Settings) | ✅ |
| Agent > Merge Conflicts | Next / Previous Conflict, Accept Ours / Theirs / Both, Finish Merge, Abort Merge (no default keys) | M4 ✅ |
| View > Git | Stage All Changes, Unstage All Changes, Commit Staged… (no default keys) | M4 ✅ |
| Changes view: click a line / ⌘-click | Comment on the line (sent to the agent as one message) / open the file there | M4 ✅ |

The code modal closes with `Esc`: first a change's peek, then the find bar, then extra cursors, then the modal (decision 18).

## Vim mode (optional, M6)

Normal/insert/visual basics: `hjkl wbe 0$ gg G`, `i a o O A I`, `x dd yy p P u Ctrl+R`, `v V`, `d c y` with motions, `/ n N`, `:w :q`. Cmd shortcuts keep working in Vim mode.

## Rules

- One table of bindings (`qur_app/src/commands.rs`). Users override in `~/Library/Application Support/Qur/keymap.toml` (`[bindings]`, command name = keys; format in `docs/guide.md`, "Keymap"); no keymap UI. Bad entries are skipped and reported.
- Chords (`Cmd+K …`) time out after 1.5 s and show the pending chord in the status area.
- Shortcuts are routed by context (editor, terminal, prompt, sidebar, palette), like VS Code `when` clauses, but simpler: the focused element's context, then the window.
- The macOS menu bar is native (File, Edit, Selection, View, Window, Help) and lists the shortcuts. Edit menu items use the standard selectors so system features (Emoji & Symbols, Dictation, AutoFill) work.
