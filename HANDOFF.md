# Qur: working handoff

Read this first when you pick the project up, whether you're a person or a Claude session. It covers where things stand, the rules, how to work on the code, and what's next. Keep it current: update **Status** and **Next** at the end of every session.

Last updated: 2026-10-27, release v0.1.8. All milestones M0–M7 are done, plus the 2026-10-25 improvements (decisions 49–58), the 2026-10-26 window and pane work (decisions 59–61) v0.1.6 (decisions 62–70), v0.1.7 and v0.1.8 (decision 71). The newest build is installed at `/Applications/Qur.app`.

## What Qur is

A lightweight, native agentic code editor for macOS, written in Rust on GPUI (Zed's UI framework). It's by PEUR Technologies (owner: Yobi) and licensed GPL-3.0-or-later. The repo is https://github.com/PEUR-Technologies/Qur (private), checked out at `~/qur`.

Agents are first-class. The sidebar lists agents as buttons. Each agent works in its own git worktree, except the first Agent Terminal, which works in the project folder. The file tree glows as a live heatmap while agents read and edit. You watch an agent in the stage, review its changes, and Merge. The owner's goal is **"the lightest ADE in the market"**: every feature is weighed against size, memory and startup.

## Rules (read before changing anything)

1. **`docs/decisions.md` wins** over everything else (38 owner decisions so far). Then `docs/QUR_HANDOFF.md` (the original spec; its section 0 rules still apply), `docs/keymap.md` and `docs/menus.md`.
2. **Lightness is the product.** Measure, don't estimate (`scripts/budget.sh`). Budgets: binary under 40 MB, idle physical footprint under 80 MB, 3 agents under 150 MB, first frame under 250 ms, 10k-line file highlighted in under 100 ms, glow under 100 ms, keystroke p99 at most 8.3 ms, search over 5,000 files under 300 ms.
3. **Ask the owner before:** adding a crate that isn't already compiled in, any network call made by Qur itself, telemetry, cloud features, or UI that isn't in the spec or decisions. So far every feature since M4 reused crates already in the build.
4. **Agents use their own CLI logins.** Never set or ask for `ANTHROPIC_API_KEY` or `OPENAI_API_KEY`. Codex is supported again (decision 44): Claude and Codex, over ACP or in a terminal.
5. **Tests that run real agents use a one-line prompt** (decision 20), such as "Reply with just the word ok."
6. **Always use a scratch `QUR_HOME` in tests**, never the owner's real settings. Leave `spikes/fixtures/big-repo` clean (no `qur/*` branches or worktrees); work on a clone of it.
7. **Git:** never add Co-Authored-By or Claude attribution lines to commits or PRs (owner preference). Branch per milestone, then merge into `main` and push once it's verified.
8. **README:** the owner keeps it short and visual (PR #1). Put details in `docs/guide.md`, not the README.
9. **Brand:** never recolor or add effects to the logo (`assets/brand/README.md`). The logo isn't in the top bar (decision 33). It appears only in About and the first-run picker, with "by PEUR Technologies".

## Status: what's built

All milestones are on `main`. There are 359 tests (`cargo test --release --workspace`), all passing; clippy is clean with `-D warnings`; `scripts/handcheck.sh` passes 125 scripted hand checks.

| Milestone | What | Decisions |
|---|---|---|
| M0 | Spikes: ACP, worktree merge, GPUI chosen over iced | 5, 14 |
| M1 | Editor shell: themes (17 + Auto, contrast-tested), project switcher, code modal, tree-sitter (13 languages), VS Code keymap, palette | 1–4, 7–9, 13, 16–18 |
| M2 | Agents in worktrees: ACP chat, permissions, Agent Terminals, `.worktreeinclude`, setup command, unread markers | 12, 19–24 |
| M3 | Live view and file-tree heatmap (FSEvents watcher) | 24–25 |
| Browser | WKWebView Browser with Inspect, per agent (chat agents and Agent Terminals) | 26 |
| M4 | Changes view, Merge (index- and stash-safe), conflicts, Git panel, Discard | 28 |
| Research | Turn-end checks, Run Project in Worktree (ideas from OpenADE and PandaCode) | 29 |
| M5 | Terminal agents: any CLI via `agents.toml`, task text and setup, "busy then quiet" turns, agent-named tasks, Rename, Find in Terminal, Cmd-click links | 30 |
| M6 | Project search and replace, alerts (notification, Dock badge, chime), first-run picker, Vim mode | 31 |
| M7 | `Qur.app` with icon, `qur doctor`, Help menu, About, logs, release README, acceptance checks | 34 |
| After M7 | Rearrangeable dock stage, layout diagrams, accent colour picker | 32, 35 |
| 2026-10-24 fixes | `claude` found when opened from Finder; first Agent Terminal in the project folder; right-click agent menu; transparency fixed; terminal answers queries | 36 |
| Terminal drawing and paste | Block and box characters drawn as shapes (fixes the Claude Code mascot); paste images (Ctrl+V to Claude) and copied files; drop files onto a terminal | 37 |
| Transparency range | Slider fixed at 60–100% on every theme (per-theme floors had locked some themes at 100%) | 38 |
| README (owner, PR #1) | README shortened to visuals (`docs/images/window.svg`, `flow.svg`, `numbers.svg`); the full guide moved to `docs/guide.md` | — |
| 2026-10-25 improvements (v0.1.4) | Turn checkpoints and Rewind; Go to Symbol, Expand/Shrink Selection, `keymap.toml`, IME marked text; nested ignores in the watcher, deleted-file rows, own-save heat attribution; Appearance submenu; terminal names; chat text size; CI and scripted input (`handcheck.sh`); chat polish (Find in Chat, grouped tool calls, permission queue, Allow All Pending, Copy Chat as Markdown); cross-vendor second-opinion review; window gaps; Settings screen (`Cmd+,`); `workspace/` and `agent_view/` split into modules | 49–58 |
| 2026-10-26 window and panes (v0.1.5) | No window shadow with gaps; zen and gaps together, zen's bar easing the content down; the bar zooms (never below 32 pt) with Qur's own traffic lights; Chrome-style sidebar (always / auto-hide / hidden, left or right, sections reordered by dragging); dock rearranging like macOS tiling (drop preview, nearest-edge targets, whole-pane drags, one + menu, × always clickable) with panes gliding into place, split terminals too; the Browser steps aside and zen holds still during drags | 59–61 |
| 2026-10-26 v0.1.6 | Files and images in the chat and a file viewer; images decoded to their drawn size; idle ACP agents release their processes; turns and Rewind for Agent Terminals; plan mode with commentable plans; tokens and cost in the header; Push and Open Pull Request; inline diff review in the code modal; Notepad++-style file handling (encodings, hex, large files); closed files freed; chats on disk and after a restart; commit messages from turns; local crash reports; Clean Up Worktrees; a window without a project; Reveal in Finder / Copy Path; memory readout and `memcheck.sh`; per-window terminal colors, Copy Image from a browser, Vim `G`, project-folder agent diffs and heat; zen's bar no longer jiggles terminals | 62–69 |
| 2026-10-26 v0.1.7 | Settings > Agents: Add agent (name, command, Structured or Terminal), agents added by hand listed too, unticking removes; Gemini CLI, opencode, GitHub Copilot and Cursor built in (none tried in Qur yet); agent dropdown in New Agent Task; with gaps the green light fills the screen (Option-click for full screen); the auto-hide sidebar slides in and out like zen's bar, the stage easing with it; Cmd+W closes a dwindled row's newest pane first | — |
| 2026-10-27 v0.1.8 | Opening Qur without a folder is a terminal; cd into a git repo (or pick a folder) and that window becomes the project, terminals kept; Shift+Enter inserts a newline in terminals; Agent Terminal changes show in the Changes panel and Files tree | 71 |

Latest numbers (`docs/budget.md`, M7 table, measured on battery): app 25 MB, first frame 113 ms, idle 73 MB, 3 agents 129 MB, glow 81 ms, keystroke p99 2.0 ms, search 51 ms. `lsof` shows no network sockets in Qur's own process.

### Not yet checked by hand

`scripts/handcheck.sh` now types, clicks and drags inside Qur (decision 54) and covers what used to be on this list: Vim keys, Find in Terminal, Rename, the agent menu, dock tabs and dividers, Cmd-click, the Git panel, About, Accent Color, Run Doctor, window gaps, Settings and the first-run welcome. It checks state and files, never pictures, so these still need a person looking at the screen:
- **Looks:** the Settings screen (spacing, theme previews, sliders on light and dark), window gaps (rounded corners over terminals, Live and the Browser; traffic lights with gaps at different zooms and in zen and full screen), the turn line ("N files · +A −D · View · Rewind"), Find in Chat highlights, grouped tool calls, the review picker and `↳ review` row, deleted-file rows in the tree.
- **Hand checks and the real mouse:** `handcheck.sh`'s drags mix with the real pointer's moves over the window, so leave the mouse still while it runs (the `dock` and `sidebar` drag checks fail at random otherwise).
- **Window gaps:** that a click in a gap never falls through to the window behind (the root paints alpha 1/255; raise it to 2/255 in `Look::root_bg` if it does).
- **Typing that scripts can't do:** IME composition (Japanese, Chinese, Korean; press-and-hold accents), `Ctrl+Shift+Cmd+→/←`, a key rebound in `keymap.toml`.
- **v0.1.6 looks and hands-on:** pasting Finder files, a screenshot and a browser's Copy Image into an ACP prompt; dropping files on it; chips, thumbnails and the file viewer; the plan view, header token readout and Plan first chip; the Agent Terminal turn strip; the Changes view's Pull Request button; the diff gutter and peek; the encoding/line-ending header, hex view and large-file view; the crash notice; Clean Up Worktrees; the empty window; zen's bar over Claude Code (the input box should stay still). A real `gh pr create` on GitHub, and Codex for plan mode and usage, were never run.
- **Terminal paste:** an image (Cmd+V sends Ctrl+V to Claude), copied files, and files dragged onto a terminal (scripted runs would overwrite the clipboard).
- **Alerts:** the chime being audible, and notifications in the installed app.
- **Codex as reviewer:** Codex isn't installed on the dev Mac, so only Claude reviewing Claude ran; check that `codex-acp` offers a read-only mode.

## Architecture map

```
crates/
  qur_app       the app (binary `qur`; `qur doctor` checks the setup)
    main.rs         startup, scripted-action driver, notification clicks, logging (~/Library/Logs/Qur)
    workspace/      one window = one project: mod.rs (struct, Render, action wiring), sidebar, top_bar,
                    sessions (stage, terminals, peeks), tasks, merge, turns, checks, code, pickers,
                    overlays, settings, actions, debug
    agent_view/     a structured (ACP) agent: mod.rs (types, Render), session (ACP, sign-in, sending,
                    permissions), chat, find, stage (header, notes, queue), turns, second_opinion
    dock.rs         the rearrangeable stage: pane tree, tabs, drag and drop, dividers, presets, diagrams
    terminal.rs     terminals (alacritty_terminal): PTY, find, Cmd-click, title capture, query replies
    editor_view.rs  the code modal's editor (also Live), and Vim routing
    changes_view.rs, git_panel.rs, search_view.rs, browser_view.rs, live.rs, tree_view.rs
    setup_view.rs   the Settings screen (Cmd+, and the first-run welcome); review.rs (second-opinion prompts);
                    script.rs (the QUR_DEBUG_ACTIONS driver); look.rs (theme, zoom, gaps tiles);
                    doctor.rs; alerts.rs; commands.rs (every command, key and menu)
  qur_acp       hand-rolled ACP v1 client (JSON-RPC over stdio, threads, no async runtime)
  qur_agents    agents.toml roster, the task store, the login PATH (interactive shell)
  qur_worktree  worktrees, .worktreeinclude, setup and check runners (refuses to remove the project folder)
  qur_git       diffs, merge (merge-tree → commit-tree → read-tree → update-ref), conflicts, status/stage/commit
  qur_search    project search and replace (ignore + regex, both already linked)
  qur_editor    UI-free editor model (rope, multi-cursor, undo, find, soft wrap) and vim.rs
  qur_watch     FSEvents watcher;  qur_heat: the heatmap model;  qur_tree: lazy gitignore-aware tree
  qur_syntax    tree-sitter, 13 grammars;  qur_theme: themes and contrast tests;  qur_config: settings files
  qur_browser   system WKWebView embedding and the element inspector
scripts/        bundle.sh (Qur.app), handcheck.sh (scripted hand checks), make_icns.swift, budget.sh, shot.sh, wins.swift
.github/        workflows/ci.yml: one macOS job (clippy -D warnings, tests, 40 MB budget, doctor); generates large.ts
docs/           decisions.md, QUR_HANDOFF.md (spec), guide.md (install, features, config), budget.md, keymap.md,
                menus.md, signing.md, research/, images/ (README visuals and screenshots)
```

Where things are stored on the owner's Mac:
- Settings: `~/Library/Application Support/Qur/`: `config.toml`, `agents.toml`, `state.toml`, `projects/<id>.toml` and `.tasks.toml`, and `worktrees/`.
- Logs: `~/Library/Logs/Qur/qur.log`.
- Qur never writes into the user's repo; only agents do.

## Hard-won findings

- **The agents' PATH must come from an interactive login shell** (`$SHELL -l -i -c`, between markers). Many setups add `~/.local/bin` (where `claude` lives) or nvm in `~/.zshrc`. A plain login shell skips that file, so Qur opened from Finder couldn't find `claude`. Terminals get this PATH too.
- **Only one translucent background layer can be on any pixel.** Regions (the top bar, sidebar, whatever fills the stage) use `Look::bg`; anything inside them uses `Look::surface`, which paints nothing while the window is translucent. Stacked layers made 60% look like 85%.
- **macOS stops drawing a fully covered window.** In scripted runs, a view focused since the last frame isn't in GPUI's dispatch tree yet. The `QUR_DEBUG_ACTIONS` driver then sends actions to the workspace instead. `QUR_BENCH=x` floats the window (PopUp). Screenshots of a covered window are stale, so check the top-bar message or log lines (`FLASH`, `BROWSER`, `SEARCH`, `ALERT`) rather than trusting a picture.
- **Screenshots fail when the owner's screen is locked** ("could not create image from window"). Check with `ioreg -n Root -d1 -a | grep -A1 CGSSessionScreenIsLocked`.
- **ACP:** both adapters speak ACP v1. Claude's `tool_call` has an empty `locations`; the path comes 0.5–0.9 s later in a `tool_call_update`. Spawn agents in their own process group (`process_group(0)`) and signal the group, so none are left orphaned.
- **Merge** uses `merge-tree --write-tree`, then `commit-tree`, `read-tree -m -u` and `update-ref`, so the user's index and stash are never touched. It needs git 2.38 or newer.
- **GPUI** is pinned to Zed rev `9e84b066`. It needs `gpui_platform`'s `font-kit` feature, or no text renders at all. GPUI already provides notifications (`App::show_system_notification`, a no-op without a bundle) and compiles `objc2-app-kit` and `objc2-user-notifications`. Zed source for reference: `~/src-ref/zed`.
- **Block and box characters are drawn as shapes** (decision 37). The font's glyphs don't fill Qur's 1.3× line height, which left gaps in Claude Code's mascot and boxes. Qur also answers terminal color queries.
- **Measuring:** memory is budgeted as physical footprint (`footprint <pid>`). On battery the 60 Hz panel paces frames at about 16.7 ms.
- **Shell safety:** the Bash tool refuses `rm` on variable paths that could expand to `/`. Use literal paths or `"${D:?}"`.

## How to run things

```sh
source ~/.cargo/env
cargo build --release -p qur_app && target/release/qur ~/some/project   # no argument = last project
cargo test --release --workspace            # 359 tests
scripts/handcheck.sh [check ...]            # 125 scripted hand checks (-l lists them); unlocked session
target/release/qur doctor                   # setup checks, exit 1 on any problem
scripts/bundle.sh [--zip]                   # target/release/Qur.app (ad-hoc signed); then cp -R to /Applications
scripts/budget.sh 3                         # the budget, incl. glow and 3 real agents; unlocked screen
python3 spikes/fixtures/gen.py              # big-repo (5,000 files) and large.ts fixtures, once
```

Scripted runs (all with a scratch `QUR_HOME`):
- `QUR_DEBUG_ACTIONS="qur::NewAgentTerminal,wait:5000,qur::Quit"` runs actions in order. The log gets `ACTION`, and top-bar messages as `FLASH`.
- Input steps in `QUR_DEBUG_ACTIONS` (`script.rs`): `type:<text>`, `key:<keys>` (`\,` for a comma, so `key:cmd-\,` is Cmd+,), `click:` / `dclick:` / `rclick:` / `cmdclick:` at `x,y` or a named mark (`@tab:Terminal:Live`, `@settings:gaps:16`, `@pane:Terminal:root~0.9~0.5`), `drag:<from>,<to>`, `scroll:<at>,<dy>`, and `state` / `marks` lines to assert on. Marks are recorded only in scripted runs. Marks scrolled out of view can't be clicked; scroll first.
- `QUR_DEBUG_PROMPT="0,1"` answers native dialogs in order with these button indexes (logged as `PROMPT`).
- `QUR_DEBUG_TASKS="prompt|prompt"` starts agent tasks with roster agent 0 (use tiny prompts).
- `QUR_DEBUG_SEARCH=<query>`, `QUR_DEBUG_SETUP=1` (the first-run welcome), `QUR_DEBUG_REVIEW="<agent>[:<model>]|<instruction>"` (start a review), `QUR_DEBUG_FIND=<query>` (Find in Chat), `QUR_DEBUG_BROWSER_URL` (with `{slug}`), `QUR_DEBUG_PICK=<css>`, `QUR_DEBUG_SEND`.
- `QUR_BENCH=first_frame|type|scroll|theme|glow` runs benchmarks; any `QUR_BENCH` value floats the window. `QUR_ACP_LOG=<dir>` logs ACP traffic; `QUR_WEB_DEVTOOLS=1` turns on the WebKit inspector.
- To test like the app opened from Finder, use `env -i HOME=$HOME USER=$USER SHELL=/bin/zsh PATH=/usr/bin:/bin:/usr/sbin:/sbin …`.

## Next

1. **Look at the screen** (the list above), ideally with the owner.
2. **Distribution:** Developer ID signing and notarization need the owner's Apple Developer account (`docs/signing.md`); until then a downloaded copy has to be allowed in System Settings > Privacy & Security.
3. **Re-measure 3 agents** with `scripts/budget.sh 3` (its prompts now differ; the old 129 MB was probably one agent).
4. **On hold (owner's notes):** a window background image or GIF with blur and readable text.
5. **Known gaps:**
   - **qur_git:** `root` must be the top of the checkout; `diff --cc` entries are skipped; a change to only the final newline shows no marker; submodules and fsmonitor are untested; a gpgsign passphrase prompt could block commits.
   - **Browser:** iframes and closed shadow roots can't be inspected; React 19 has no `_debugSource` (use `data-qur-source`); keyboard text selection isn't captured; page JavaScript can see `window.__qur`.
   - **Agents:** a project-folder Agent Terminal's heat still uses the busy rule, so your writes from another editor while it's busy can glow. A restored chat's images are blank frames that open their file (a pasted image in the temp folder may be gone). Ghost rows for deleted files appear only in a task's tree. Claude Code's permission prompts go quiet, so one terminal turn can split into several Qur turns.
   - **Vim:** a count before the operator (`3dG`) still goes to the last line.
   - **Review:** models are listed only for the task's own agent; one reviewer per task.
   - **Builds:** never share one `CARGO_TARGET_DIR` between worktrees: cargo hands one worktree's crates to another and builds stale or wrong binaries. Give each worktree its own (an APFS clone with `cp -c -R target …` is instant), and after a suspicious build, `touch` the crate sources.

## Open questions for the owner

- Trademark clearance for "Qur" hasn't been done (the README says so).
