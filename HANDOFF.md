# Qur: working handoff

Read this first when you pick the project up, whether you're a person or a Claude session. It covers where things stand, the rules, how to work on the code, and what's next. Keep it current: update **Status** and **Next** at the end of every session.

Last updated: 2026-10-24. All milestones M0–M7 are done; `main` is at `558ce36`. Installed at `/Applications/Qur.app`.

## What Qur is

A lightweight, native agentic code editor for macOS, written in Rust on GPUI (Zed's UI framework). It's by PEUR Technologies (owner: Yobi) and licensed GPL-3.0-or-later. The repo is https://github.com/PEUR-Technologies/Qur (private), checked out at `~/qur`.

Agents are first-class. The sidebar lists agents as buttons. Each agent works in its own git worktree, except the first Agent Terminal, which works in the project folder. The file tree glows as a live heatmap while agents read and edit. You watch an agent in the stage, review its changes, and Merge. The owner's goal is **"the lightest ADE in the market"**: every feature is weighed against size, memory and startup.

## Rules (read before changing anything)

1. **`docs/decisions.md` wins** over everything else (36 owner decisions so far). Then `docs/QUR_HANDOFF.md` (the original spec; its section 0 rules still apply), `docs/keymap.md` and `docs/menus.md`.
2. **Lightness is the product.** Measure, don't estimate (`scripts/budget.sh`). Budgets: binary under 40 MB, idle physical footprint under 80 MB, 3 agents under 150 MB, first frame under 250 ms, 10k-line file highlighted in under 100 ms, glow under 100 ms, keystroke p99 at most 8.3 ms, search over 5,000 files under 300 ms.
3. **Ask the owner before:** adding a crate that isn't already compiled in, any network call made by Qur itself, telemetry, cloud features, or UI that isn't in the spec or decisions. So far every feature since M4 reused crates already in the build.
4. **Agents use their own CLI logins.** Never set or ask for `ANTHROPIC_API_KEY` or `OPENAI_API_KEY`. **No Codex for now** (decision 6).
5. **Tests that run real agents use a one-line prompt** (decision 20), such as "Reply with just the word ok."
6. **Always use a scratch `QUR_HOME` in tests**, never the owner's real settings. Leave `spikes/fixtures/big-repo` clean (no `qur/*` branches or worktrees); work on a clone of it.
7. **Git:** never add Co-Authored-By or Claude attribution lines to commits or PRs (owner preference). Branch per milestone, then merge into `main` and push once it's verified.
8. **Brand:** never recolor or add effects to the logo (`assets/brand/README.md`). The logo isn't in the top bar (decision 33). It appears only in About and the first-run picker, with "by PEUR Technologies".

## Status: what's built

All milestones are on `main`. There are 160 tests (`cargo test --release --workspace`), all passing.

| Milestone | What | Decisions |
|---|---|---|
| M0 | Spikes: ACP, worktree merge, GPUI chosen over iced | 5, 14 |
| M1 | Editor shell: themes (17 + Auto, contrast-tested), project switcher, code modal, tree-sitter (13 languages), VS Code keymap, palette | 1–4, 7–9, 13, 16–18 |
| M2 | Agents in worktrees: ACP chat, permissions, Agent Terminals, `.worktreeinclude`, setup command, unread markers | 12, 19–24 |
| M3 | Live view and file-tree heatmap (FSEvents watcher) | 24–25 |
| Browser | WKWebView Browser with Inspect, per agent | 26 |
| M4 | Changes view, Merge (index- and stash-safe), conflicts, Git panel, Discard | 28 |
| Research | Turn-end checks, Run Project in Worktree (ideas from OpenADE and PandaCode) | 29 |
| M5 | Terminal agents: any CLI via `agents.toml`, task text and setup, "busy then quiet" turns, agent-named tasks, Rename, Find in Terminal, Cmd-click links | 30 |
| M6 | Project search and replace, alerts (notification, Dock badge, chime), first-run picker, Vim mode | 31 |
| M7 | `Qur.app` with icon, `qur doctor`, Help menu, About, logs, release README, acceptance checks | 34 |
| After M7 | Rearrangeable dock stage, layout diagrams, accent colour picker | 32, 35 |
| 2026-10-24 fixes | `claude` found when opened from Finder; first Agent Terminal in the project folder; right-click agent menu; transparency fixed; terminal answers queries | 36 |

Latest numbers (`docs/budget.md`, M7 table, measured on battery): app 25 MB, first frame 113 ms, idle 73 MB, 3 agents 129 MB, glow 81 ms, keystroke p99 2.0 ms, search 51 ms. `lsof` shows no network sockets in Qur's own process.

### Not yet checked by hand

Scripted tests can't type into Qur, click inside it, or drag, so these were only unit-tested or checked through logs. Ask the owner to try them, or do it with computer use:
- **Dock:** dragging a tab to split or join panes, dragging a divider, × and +, and the right-click agent menu.
- **Editor:** typing Vim keys in the code modal, and typing in Find in Terminal.
- **Other interactions:** Cmd-click links, the Rename box, the first-run picker's Check button.
- **Git panel:** per-file stage and unstage, the file diff, a typed commit.
- **Alerts:** the chime being audible, and no alert while you're looking at that agent.
- **Panels and the bundled app:** About, Run Doctor's sheet, the Accent Color popover on screen. Notifications in the installed app (they need the bundle; dev builds skip them).

## Architecture map

```
crates/
  qur_app       the app (binary `qur`; `qur doctor` checks the setup)
    main.rs         startup, scripted-action driver, notification clicks, logging (~/Library/Logs/Qur)
    workspace.rs    one window = one project: sidebar, stage, overlays, tasks, merge, checks, alerts
    agent_view.rs   a structured (ACP) agent: chat, permission bar, plan; its stage is the dock
    dock.rs         the rearrangeable stage: pane tree, tabs, drag and drop, dividers, presets, diagrams
    terminal.rs     terminals (alacritty_terminal): PTY, find, Cmd-click, title capture, query replies
    editor_view.rs  the code modal's editor (also Live), and Vim routing
    changes_view.rs, git_panel.rs, search_view.rs, browser_view.rs, live.rs, tree_view.rs
    setup_view.rs   the first-run picker; doctor.rs; alerts.rs; commands.rs (every command, key and menu)
  qur_acp       hand-rolled ACP v1 client (JSON-RPC over stdio, threads, no async runtime)
  qur_agents    agents.toml roster, the task store, the login PATH (interactive shell)
  qur_worktree  worktrees, .worktreeinclude, setup and check runners (refuses to remove the project folder)
  qur_git       diffs, merge (merge-tree → commit-tree → read-tree → update-ref), conflicts, status/stage/commit
  qur_search    project search and replace (ignore + regex, both already linked)
  qur_editor    UI-free editor model (rope, multi-cursor, undo, find, soft wrap) and vim.rs
  qur_watch     FSEvents watcher;  qur_heat: the heatmap model;  qur_tree: lazy gitignore-aware tree
  qur_syntax    tree-sitter, 13 grammars;  qur_theme: themes and contrast tests;  qur_config: settings files
  qur_browser   system WKWebView embedding and the element inspector
scripts/        bundle.sh (Qur.app), make_icns.swift, budget.sh, shot.sh, wins.swift
docs/           decisions.md, QUR_HANDOFF.md (spec), budget.md, keymap.md, menus.md, signing.md, research/
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
- **Claude Code's light/dark look is its own setting** (`/theme`; the owner's is `dark`). A dark setting on a light Qur theme draws black bars around the mascot. Qur now answers terminal color queries anyway.
- **Measuring:** memory is budgeted as physical footprint (`footprint <pid>`). On battery the 60 Hz panel paces frames at about 16.7 ms.
- **Shell safety:** the Bash tool refuses `rm` on variable paths that could expand to `/`. Use literal paths or `"${D:?}"`.

## How to run things

```sh
source ~/.cargo/env
cargo build --release -p qur_app && target/release/qur ~/some/project   # no argument = last project
cargo test --release --workspace            # 160 tests
target/release/qur doctor                   # setup checks, exit 1 on any problem
scripts/bundle.sh [--zip]                   # target/release/Qur.app (ad-hoc signed); then cp -R to /Applications
scripts/budget.sh 3                         # the budget, incl. glow and 3 real agents; unlocked screen
python3 spikes/fixtures/gen.py              # big-repo (5,000 files) and large.ts fixtures, once
```

Scripted runs (all with a scratch `QUR_HOME`):
- `QUR_DEBUG_ACTIONS="qur::NewAgentTerminal,wait:5000,qur::Quit"` runs actions in order. The log gets `ACTION`, and top-bar messages as `FLASH`.
- `QUR_DEBUG_PROMPT="0,1"` answers native dialogs in order with these button indexes (logged as `PROMPT`).
- `QUR_DEBUG_TASKS="prompt|prompt"` starts agent tasks with roster agent 0 (use tiny prompts).
- `QUR_DEBUG_SEARCH=<query>`, `QUR_DEBUG_SETUP=1` (show the first-run picker), `QUR_DEBUG_BROWSER_URL` (with `{slug}`), `QUR_DEBUG_PICK=<css>`, `QUR_DEBUG_SEND`.
- `QUR_BENCH=first_frame|type|scroll|theme|glow` runs benchmarks; any `QUR_BENCH` value floats the window. `QUR_ACP_LOG=<dir>` logs ACP traffic; `QUR_WEB_DEVTOOLS=1` turns on the WebKit inspector.
- To test like the app opened from Finder, use `env -i HOME=$HOME USER=$USER SHELL=/bin/zsh PATH=/usr/bin:/bin:/usr/sbin:/sbin …`.

## Next

1. **Hand checks** (the list above), ideally with the owner watching.
2. **Distribution:** publish a GitHub release with `Qur.zip`. That's outward-facing, so ask first. Developer ID signing and notarization need the owner's Apple Developer account (`docs/signing.md`); until then a downloaded copy has to be allowed in System Settings > Privacy & Security.
3. **From the research shortlist** (`docs/research/openade-pandacode.md`, section 4):
   - **Undo an agent's turn:** per-turn trees under `refs/qur/turns/<task>/<n>`, with "View · Rewind" on each turn, for ACP tasks first. Size M, no crates. This is the best remaining idea.
   - **Needs the owner's OK** (costs Claude usage or depends on the adapter): a second-opinion review session, and Plan mode with commentable plans.
   - **Chat polish:** find in chat, grouped tool calls, Allow All Pending, Copy Chat as Markdown.
4. **Greyed-out commands** ("coming in a later version"): Go to Symbol, Expand/Shrink Selection (tree-sitter nodes), and `keymap.toml` overrides.
5. **Known gaps:**
   - **qur_git:** `root` must be the top of the checkout; `diff --cc` entries are skipped; a change to only the final newline shows no marker; submodules and fsmonitor are untested; a gpgsign passphrase prompt could block commits.
   - **Browser:** iframes and closed shadow roots can't be inspected; React 19 has no `_debugSource` (use `data-qur-source`); keyboard text selection isn't captured; page JavaScript can see `window.__qur`.
   - **Editor:** IME marked text isn't drawn; End and Cmd+Right go to the line end, not the visual-row end.
   - **Tree and watcher:** the watcher honours only the root `.gitignore`; deleted files don't appear as tree rows.
   - **Agents:** project-folder agents share the project's tree and heatmap with the main checkout. Their Changes view diffs against HEAD at the time they started.
   - **Transparency:** `THEME_COLORS`, used to answer terminal color queries, is process-wide, so two windows with different themes answer with the last one drawn.

## Open questions for the owner

- Trademark clearance for "Qur" hasn't been done (the README says so).
- Publish a GitHub release now, or wait for Developer ID signing?
- Which of the research items that need approval to do next (second-opinion review, Plan mode)?
