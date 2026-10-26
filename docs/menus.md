# Qur menu bar (v1)

Owner decision (2026-10-23): **commands live in the native macOS menu bar, like Zed, not as buttons in the window.** The window shows only the top bar (symbol + project), sidebar, editor/peek, prompt bar and enabled panels. Every menu item is also a command-palette entry and shows its shortcut (`docs/keymap.md`). Items that don't apply right now are disabled, never hidden, so the menu stays predictable.

Ellipsis (…) means the item opens a picker, popover or dialog.

## Qur
- About Qur
- Settings… `Cmd+,` (the settings screen; it also opens by itself on first launch)
- Settings Files ▸
  - Open config.toml
  - Open keymap.toml (created with examples when missing)
  - Agents (agents.toml)
- ---
- Services ▸
- ---
- Hide Qur `Cmd+H` · Hide Others `Cmd+Opt+H` · Show All
- ---
- Quit Qur `Cmd+Q`

## File
- New Agent Task… `Cmd+Shift+N`
- New Terminal `Cmd+T`
- New Agent Terminal `Cmd+Shift+T`
- New Window `Cmd+N`
- ---
- Open File… `Cmd+O` (any file, also outside the project)
- Open Folder… `Cmd+K Cmd+O`
- Open Recent ▸ (projects)
- Quick Open… `Cmd+P`
- ---
- Save `Cmd+S` · Save All `Cmd+Opt+S`
- Revert File
- ---
- Open as Text (the file viewer's file, in the code modal or, when large, as paged text)
- Encoding ▸ (like Notepad++'s Encoding menu)
  - Reopen with Encoding… (UTF-8, UTF-8 BOM, UTF-16 LE, UTF-16 BE, Windows-1252)
  - Convert to Encoding… (saved in it from now on; undoable)
  - ---
  - Convert Line Endings (LF ⇄ CRLF; undoable)
- ---
- Reveal in Finder · Copy Path · Copy Relative Path `Shift+Opt+Cmd+C` (the file in the code modal; also in the Files tree's right-click menu)
- ---
- Close Terminal or Agent `Cmd+W` (in a terminal or an agent; an agent asks first)
- Close Window `Cmd+Shift+W`

## Edit
- Undo `Cmd+Z` · Redo `Cmd+Shift+Z`
- ---
- Cut · Copy · Paste · Select All (standard OS selectors)
- ---
- Find `Cmd+F` · Replace `Cmd+Opt+F` · Find Next `Cmd+G` · Find Previous `Cmd+Shift+G` · Use Selection for Find `Cmd+E`
- Find in Project `Cmd+Shift+F` · Replace in Project `Cmd+Shift+H`
- ---
- Toggle Line Comment `Cmd+/` · Toggle Block Comment `Shift+Opt+A`
- Indent `Cmd+]` · Outdent `Cmd+[`
- Trim Trailing Whitespace
- ---
- Changes ▸ Next Change `Opt+F5` · Previous Change `Shift+Opt+F5` · Show Change `Opt+F3` · Revert Change · Accept Change · --- · Last Turn Only (checkmark) (the code modal's change markers: an agent's changes vs its task's base, your own files vs HEAD)
- ---
- Vim Mode (checkmark)
- ---
- (macOS adds Start Dictation / Emoji & Symbols)

## Selection
- Select Line `Cmd+L` · Expand Selection `Ctrl+Shift+Cmd+→` · Shrink Selection `Ctrl+Shift+Cmd+←` (tree-sitter nodes)
- ---
- Copy Line Up/Down · Move Line Up/Down · Delete Line `Cmd+Shift+K`
- Insert Line Below/Above
- ---
- Add Next Match `Cmd+D` · Select All Matches · Add Cursor Above/Below

## View
- Command Palette… `Cmd+Shift+P`
- ---
- Sidebar `Cmd+B` (checkmark) · Auto-hide Sidebar (checkmark) · Sidebar on Right (checkmark) · Files Dropdown `Cmd+Shift+E`
- Go to Terminal ``Ctrl+` `` · Git `Ctrl+Shift+G` · Search `Cmd+Shift+F` (a view in the stage, decision 31)
- Git ▸ Stage All Changes · Unstage All Changes · Commit Staged…
- ---
- Peek Layout ▸ presets A Watch + side notes · B One view at a time · C Watch + drawer · Cycle `Cmd+K Cmd+L` (the stage itself is rearranged by dragging tabs and dividers, decision 32)
- Peek ▸ Live `Cmd+1` · To-do `Cmd+2` · Chat `Cmd+3` · Changes `Cmd+4` · Browser `Cmd+5`
- Browser: Inspect Element `Cmd+Shift+C`
- Follow Agent (checkmark)
- ---
- Soft Wrap `Opt+Z` (checkmark) · Show Whitespace (checkmark; `[editor] show_whitespace`)
- Zoom In `Cmd+=` · Zoom Out `Cmd+-` · Reset Zoom `Cmd+0`
- ---
- Appearance ▸
  - Theme… `Cmd+K Cmd+T` (scope: This Project / All Projects)
  - Use Default Theme for This Project
  - ---
  - Accent Color… (hue and brightness sliders, live)
  - Transparency… (opacity slider + blur, live)
  - Gaps… (window gaps on/off, gap and corner-radius sliders, rounded corners, tile border; live, saved on close)
- Zen Mode `Ctrl+Cmd+Z` (checkmark)
- Toggle Gaps (checkmark; Hyprland-style window gaps)
- Enter Full Screen `Ctrl+Cmd+F`

## Go
- Go to File… `Cmd+P` · Go to Line… `Ctrl+G` · Go to Symbol in File… `Cmd+Shift+O`
- Back `Ctrl+-` · Forward `Ctrl+Shift+-`
- Next / Previous Recent File `Ctrl+Tab`
- Jump to Matching Bracket
- ---
- Next Change `Opt+F5` · Previous Change `Shift+Opt+F5` (also in Edit > Changes)

## Agent
- New Agent Task… `Cmd+Shift+N` · New Agent Terminal `Cmd+Shift+T`
- Focus Prompt `Cmd+I`
- Stop `Cmd+.`
- Take Over (Live, when an agent is editing)
- Rename Agent… (also double-click the agent's button)
- Permission: Allow Once `Cmd+Y` · Allow for This Task `Cmd+Opt+Y` · Deny `Cmd+Opt+Z`
- Permission: Allow All Pending (every queued request of the agent, each "allow once", never "always"; disabled when no request is waiting)
- ---
- Find in Chat `Cmd+F` (in a structured agent's view) · Copy Chat as Markdown (also in the sidebar's right-click menu, with Allow All Pending)
- ---
- Plan Mode `Cmd+Opt+P` (checkmark; the selected agent's plan or read-only mode, or back to the mode it was in; in the New Agent Task dialog it toggles **Plan first**) · Comment on Plan… (shows the plan the agent proposed, or in plan mode its latest reply; click a line to comment) · Approve Plan · Send Plan Comments (disabled when there's no plan, or no comments). Structured agents only.
- ---
- Next Agent · Previous Agent · Switch Agent or Terminal… `Ctrl+T` · Agent 1–9 `Ctrl+1..9`
- ---
- Review Changes `Cmd+4` · Merge into My Branch · Keep Working · Discard Task…
- Push and Open Pull Request… (asks first; commits the task's uncommitted work as "qur: <title>" like Merge, runs your own `git push -u <remote> <task branch>` (the remote of your current branch, else `origin`), then `gh pr create` into your branch; without `gh` or its login, GitHub's compare page opens in the browser, and other hosts just get the push. The PR's address is kept on the task and shown in the Changes view; a second run pushes new work to the same PR. Remote branches are never deleted, also on Discard) · Open Pull Request (once the task has one). Also in the agent's right-click menu, and as a button next to Merge in the Changes view.
- Show Last Turn's Changes · Rewind Last Turn… (structured agents with a turn that changed files; disabled otherwise, and for Agent Terminals)
- Review Agent's Work… · Review Agent's Plan… (a read-only second opinion from another agent or model; Plan only when the agent has a plan) · Send Review Findings to Agent (once its reviewer has findings). Disabled otherwise, and for Agent Terminals. Also in the agent's right-click menu.
- Run Checks Now (the project's `[checks] command`, decision 29)
- Run Project in Worktree (the project's `[run] command`, decision 29)
- Merge Conflicts ▸ Next Conflict · Previous Conflict · Accept Ours · Accept Theirs · Accept Both · Finish Merge · Abort Merge (enabled only during a merge)
- ---
- Open Task Worktree in Terminal
- Reveal Worktree in Finder

## Terminal
- New Terminal `Cmd+T` · New Agent Terminal `Cmd+Shift+T`
- Next / Previous Terminal `Cmd+Shift+]` / `Cmd+Shift+[`
- New Tiled Terminal (Grid) `Cmd+Enter` · Split (Dwindle) `Cmd+D` · Split Down `Cmd+Shift+D`
- Focus Pane Left / Right / Above / Below `Cmd+Opt+Arrows`
- Clear `Cmd+K` · Find in Terminal `Cmd+F`
- Close Terminal or Agent `Cmd+W`

## Window
- Minimize `Cmd+M` · Zoom · (macOS window list, tiling items)

## Help
- (macOS search field)
- Qur Keyboard Shortcuts (opens the keymap reference)
- Run Doctor (`qur doctor` checks; result in a sheet)
- Show Logs in Finder
- Report a Problem… (opens the GitHub issues page in the browser; no in-app network call)
- About Qur

## Right-click menus

- **An agent's button** in the sidebar: its Agent menu commands (Rename…, Review Changes, Merge, Run Checks Now, Run Project, Reveal in Finder, Open Folder in Terminal, turn, review and permission commands, Stop, Close/Delete Agent…).
- **A Files tree row** (the project or a task's worktree): Open · Open in Default App (files only) · Reveal in Finder · Copy Path · Copy Relative Path (relative to the tree's root). A deleted file's row shows them all greyed out. Esc or a click outside closes it.

## What this removes from the window

- No toolbar, no gear icon. The one settings screen is **Qur > Settings…** (`Cmd+,`), the macOS place for it: Agents, Appearance (theme, accent, transparency, top bar, traffic lights), Editor (Vim mode, keys), Layout, and links to the settings files. Each of its settings is also a menu command or popover (View > Appearance, Edit > Vim Mode, View > Zen Mode, View > Peek Layout), and each change there saves the same way.
- Peek header keeps only the Follow toggle and Live/To-do/Chat/Changes segments (layout B). Layout switching moves to the **View** menu and shortcut.
- Merge/Discard (and Pull Request…) keep their buttons only in the Changes view (handoff section 10), since they are the action of that view; they are also in the **Agent** menu.
- The quick popovers stay: Theme… (picker), Accent Color…, Transparency… and Gaps… open in place without the settings screen.
