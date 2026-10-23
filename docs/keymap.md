# Qur keymap (v1)

Base: VS Code's default macOS keymap, minus features Qur doesn't have (LSP, file tabs, extensions), plus Qur's own commands. Every binding is also a **command** in the command palette (`Cmd+Shift+P`) and, where macOS expects it, a **menu item**, so it is discoverable and shows its shortcut.

Spike tier: **S** = implemented in the M0 toolkit prototypes; **M1+** = built with the real editor.

## App and window

| Keys | Command | Menu | Tier |
|---|---|---|---|
| `Cmd+Q` | Quit Qur | Qur | S |
| `Cmd+H` / `Cmd+Opt+H` | Hide Qur / Hide Others | Qur | S |
| `Cmd+,` | Open config.toml | Qur > Settings… | M1+ |
| `Cmd+N` | New Window | File | M1+ |
| `Cmd+Shift+N` | New Agent Task (same as sidebar `+`) | File | M2 ✅ |
| `Cmd+O` | Open Folder… | File | M1+ |
| `Cmd+Opt+O` | Switch Project… (also: click the project name in the top bar) | File | M1 |
| `Cmd+Shift+W` | Close Window | File | S |
| `Cmd+M` | Minimize | Window | S |
| `Ctrl+Cmd+F` | Toggle Full Screen | View | S |
| `Cmd+Shift+P` / `F1` | Command Palette | View | S |
| `Cmd+K Cmd+T` | Theme Picker | Qur | M1 |
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
| `Cmd+Shift+O` | Go to Symbol in File (tree-sitter outline, no LSP) | M1+ |

## Editing

| Keys | Command | Tier |
|---|---|---|
| `Cmd+Z` / `Cmd+Shift+Z` | Undo / Redo | S |
| `Cmd+X` / `Cmd+C` / `Cmd+V` | Cut / Copy / Paste (the whole line when the selection is empty) | S |
| `Cmd+A` | Select All | S |
| `Shift+arrows` | Extend selection | S |
| `Opt+Left/Right` | Word left/right (`Shift` extends) | S |
| `Cmd+Left/Right` | Line start (smart home) / end (`Shift` extends) | S |
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
| `Cmd+D` | Add selection to next match (multi-cursor) | M1 |
| `Cmd+Shift+L` | Select all matches | M1 |
| `Opt+Click` / `Cmd+Opt+Up/Down` | Add cursor | M1 |
| `Esc` | Collapse to one cursor / close find | S |
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
| `Cmd+W` | Close focused terminal | S |
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
| `Cmd+I` | Focus the prompt bar | M2 ✅ |
| `Enter` / `Shift+Enter` | Send (queues during a turn) / newline | M2 ✅ |
| `Esc` (prompt) / `Cmd+.` | Stop the agent's turn (`session/cancel`) | M2 ✅ |
| `@` / `/` in the prompt | Mention a file / pick an agent slash command | M2 ✅ |
| Agent menu | Permission: Allow Once / Allow for This Task / Deny | M2 ✅ |
| Agent menu | Discard Task…, Reveal Worktree in Finder, Open Task Worktree in Terminal | M2 ✅ |
| `Cmd+K Cmd+L` | Cycle peek layout | M3 ✅ |
| `Cmd+1..4` | Live / To-do / Chat / Changes (layout B) | M3 ✅ |
| `Cmd+5` | Browser: show the selected agent's browser (any layout; Agent Terminals too) | Browser ✅ |
| `Cmd+Shift+C` | Browser: Inspect Element (highlight, click to attach to the prompt, or paste into an Agent Terminal; select text to attach it) | Browser ✅ |
| Changes view / Agent menu | Merge into My Branch, Keep Working, Discard Task…, Review Changes | M4 ✅ |
| Agent menu | Run Checks Now, Run Project in Worktree (no default keys) | ✅ |
| `Cmd+F` (in a terminal) | Find in Terminal (regex; ↩ older, ⇧↩ newer, Esc closes) | M5 ✅ |
| `Cmd`-click (in a terminal) | Open a URL, or `path:line:col` in the code modal | M5 ✅ |
| Agent menu / double-click a button | Rename Agent… | M5 ✅ |
| `Cmd+Shift+F` / `Cmd+Shift+H` | Find / Replace in Project (Search view; ⌥C case, ⌥W word, ⌥R regex) | M6 ✅ |
| Qur > Vim Mode | Vim keys in the code modal: normal / insert / visual / visual line | M6 ✅ |
| Qur > Setup… | First-run picker (Enter saves, Esc cancels) | M6 ✅ |
| Agent > Merge Conflicts | Next / Previous Conflict, Accept Ours / Theirs / Both, Finish Merge, Abort Merge (no default keys) | M4 ✅ |
| View > Git | Stage All Changes, Unstage All Changes, Commit Staged… (no default keys) | M4 ✅ |
| Changes view: click a line / ⌘-click | Comment on the line (sent to the agent as one message) / open the file there | M4 ✅ |

The code modal closes with `Esc`: first the find bar, then extra cursors, then the modal (decision 18).

## Vim mode (optional, M6)

Normal/insert/visual basics: `hjkl wbe 0$ gg G`, `i a o O A I`, `x dd yy p P u Ctrl+R`, `v V`, `d c y` with motions, `/ n N`, `:w :q`. Cmd shortcuts keep working in Vim mode.

## Rules

- One table of bindings in `qur_config` (TOML, embedded default). Users may override in `~/Library/Application Support/Qur/keymap.toml`; no keymap UI.
- Chords (`Cmd+K …`) time out after 1.5 s and show the pending chord in the status area.
- Shortcuts are routed by context (editor, terminal, prompt, sidebar, palette), like VS Code `when` clauses, but simpler: the focused element's context, then the window.
- The macOS menu bar is native (File, Edit, Selection, View, Window, Help) and lists the shortcuts. Edit menu items use the standard selectors so system features (Emoji & Symbols, Dictation, AutoFill) work.
