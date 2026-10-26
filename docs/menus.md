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

## The empty window (no project)

Qur opens a window without a project when it has none to show: launched (from Finder or the Dock) with no last project, the last project moved or deleted, `qur ~` or `qur /`, the last project window closed, or File > New Window. Its stage lists recent projects and Open Folder…, New Terminal, New Agent Terminal in… and New Agent Task in…; its terminals start in the home folder, and files open in the code modal. Commands that need a project are disabled there: Quick Open, Files Dropdown, Use Default Theme for This Project, Find/Replace in Project, Search, Source Control and its Git items, the whole Agent menu except New Agent Task… and New Agent Terminal (which ask for the folder to work in and open it as a project), Peek Layout, Peek, Inspect Element and Follow Agent. Theme and Transparency changes there apply to all projects. Opening a project closes the empty window unless it has terminals or unsaved files; closing the empty window when it's the last one quits Qur.

A folder that isn't a git repository opens as a project without git: Source Control, its Git items and Merge Conflicts are disabled. Its first Agent Terminal works in the folder itself; New Agent Task… and further Agent Terminals need a worktree, so they first offer **Initialize Git** (`git init` and a first commit of the folder's files, minus anything `.gitignore`d).

## File
- New Agent Task… `Cmd+Shift+N` (in the empty window: asks for the folder first)
- New Terminal `Cmd+T`
- New Agent Terminal `Cmd+Shift+T` (in the empty window: asks for the folder first)
- New Window `Cmd+N` (the empty window; an idle one is brought forward)
- ---
- Open Folder… `Cmd+O`
- Open Recent ▸ (projects)
- Quick Open… `Cmd+P`
- ---
- Save `Cmd+S` · Save All `Cmd+Opt+S`
- Revert File
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
- Soft Wrap `Opt+Z` (checkmark)
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
- Clean Up Worktrees… (per project: each Qur worktree, `qur/*` branch without a worktree, worktree whose folder is gone and leftover turn checkpoints, with its size on disk, the task that owns it, unmerged commits and uncommitted changes. Leftovers with no task, nothing unmerged and nothing uncommitted come selected; removing anything with unmerged commits or uncommitted changes asks again and names them. A task's own worktree is removed with Discard Task…; `qur doctor` shows the total)

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

## What this removes from the window

- No toolbar, no gear icon. The one settings screen is **Qur > Settings…** (`Cmd+,`), the macOS place for it: Agents, Appearance (theme, accent, transparency, top bar, traffic lights), Editor (Vim mode, keys), Layout, and links to the settings files. Each of its settings is also a menu command or popover (View > Appearance, Edit > Vim Mode, View > Zen Mode, View > Peek Layout), and each change there saves the same way.
- Peek header keeps only the Follow toggle and Live/To-do/Chat/Changes segments (layout B). Layout switching moves to the **View** menu and shortcut.
- Merge/Discard (and Pull Request…) keep their buttons only in the Changes view (handoff section 10), since they are the action of that view; they are also in the **Agent** menu.
- The quick popovers stay: Theme… (picker), Accent Color…, Transparency… and Gaps… open in place without the settings screen.
