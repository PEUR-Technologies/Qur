# Qur menu bar (v1)

Owner decision (2026-10-23): **commands live in the native macOS menu bar, like Zed, not as buttons in the window.** The window shows only the top bar (symbol + project), sidebar, editor/peek, prompt bar and enabled panels. Every menu item is also a command-palette entry and shows its shortcut (`docs/keymap.md`). Items that don't apply right now are disabled, never hidden, so the menu stays predictable.

Ellipsis (…) means the item opens a picker, popover or dialog.

## Qur
- About Qur
- Check Setup… (re-opens the first-run picker)
- ---
- Settings ▸
  - Open config.toml `Cmd+,`
  - Open keymap.toml
  - Agents (agents.toml)
- Theme… `Cmd+K Cmd+T` (scope: This Project / All Projects)
- Use Default Theme for This Project
- Transparency… (opacity slider + blur, live)
- Vim Mode (checkmark)
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
- Open Folder… `Cmd+O`
- Open Recent ▸ (projects)
- Quick Open… `Cmd+P`
- ---
- Save `Cmd+S` · Save All `Cmd+Opt+S`
- Revert File
- ---
- Close Terminal `Cmd+W` (when a terminal is focused)
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
- (macOS adds Start Dictation / Emoji & Symbols)

## Selection
- Select Line `Cmd+L` · Expand / Shrink Selection (tree-sitter, M1+)
- ---
- Copy Line Up/Down · Move Line Up/Down · Delete Line `Cmd+Shift+K`
- Insert Line Below/Above
- ---
- Add Next Match `Cmd+D` · Select All Matches · Add Cursor Above/Below

## View
- Command Palette… `Cmd+Shift+P`
- ---
- Sidebar `Cmd+B` (checkmark) · Files Dropdown `Cmd+Shift+E`
- Go to Terminal ``Ctrl+` `` · Git `Ctrl+Shift+G` · Search `Cmd+Shift+F` (a view in the stage, decision 31)
- Git ▸ Stage All Changes · Unstage All Changes · Commit Staged…
- ---
- Peek Layout ▸ presets A Watch + side notes · B One view at a time · C Watch + drawer · Cycle `Cmd+K Cmd+L` (the stage itself is rearranged by dragging tabs and dividers, decision 32)
- Peek ▸ Live `Cmd+1` · To-do `Cmd+2` · Chat `Cmd+3` · Changes `Cmd+4` · Browser `Cmd+5`
- Browser: Inspect Element `Cmd+Shift+C`
- Follow Agent (checkmark)
- ---
- Soft Wrap `Opt+Z` (checkmark)
- Zoom In `Cmd+=` · Zoom Out `Cmd+-` · Reset Zoom `Cmd+0`
- ---
- Enter Full Screen `Ctrl+Cmd+F`

## Go
- Go to File… `Cmd+P` · Go to Line… `Ctrl+G` · Go to Symbol in File… `Cmd+Shift+O`
- Back `Ctrl+-` · Forward `Ctrl+Shift+-`
- Next / Previous Recent File `Ctrl+Tab`
- Jump to Matching Bracket

## Agent
- New Agent Task… `Cmd+Shift+N` · New Agent Terminal `Cmd+Shift+T`
- Focus Prompt `Cmd+I`
- Stop `Cmd+.`
- Take Over (Live, when an agent is editing)
- Rename Agent… (also double-click the agent's button)
- Permission: Allow Once `Cmd+Y` · Allow for This Task `Cmd+Opt+Y` · Deny `Cmd+Opt+Z`
- ---
- Next Agent · Previous Agent · Agent 1–9 `Ctrl+1..9`
- ---
- Review Changes `Cmd+4` · Merge into My Branch · Keep Working · Discard Task…
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
- Close Terminal `Cmd+W`

## Window
- Minimize `Cmd+M` · Zoom · (macOS window list, tiling items)

## Help
- (macOS search field)
- Qur Keyboard Shortcuts (opens the keymap reference)
- Run Doctor (`qur doctor` checks; result in a sheet)
- Show Logs in Finder
- Report a Problem… (opens the GitHub issues page in the browser; no in-app network call)
- About Qur

## What this removes from the window

- No toolbar, no gear icon, no settings screen. Theme, transparency, Vim and config live under the **Qur** menu.
- Peek header keeps only the Follow toggle and Live/To-do/Chat/Changes segments (layout B). Layout switching moves to the **View** menu and shortcut.
- Merge/Discard keep their buttons only in the Changes view (handoff section 10), since they are the action of that view; they are also in the **Agent** menu.
- The "Transparency…" popover is the only settings surface outside the first-run and theme pickers.
