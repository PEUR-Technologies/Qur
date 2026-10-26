# Qur: site notes

Material for qursite. Written 2026-10-26. Every number here was measured on a MacBook Air M5 with `scripts/budget.sh` unless it says otherwise; the sources are `docs/budget.md` and `docs/guide.md` in the repo. "Shipped" means it's in the v0.1.5 release. "Next release" means it's built and tested on a branch but not released yet. Don't put anything from "In progress" on the site until it ships.

---

## Positioning

**One line:** Qur is the lightest way to run AI coding agents: a native macOS app where every agent works in its own worktree and you watch the files light up as it works.

**Taglines to choose from**
- See what your agents see.
- Run five agents. Use less memory than one browser tab.
- Agents first. Electron never.
- 25 MB. No account. No telemetry. Your agents, your machine.
- Watch the work, not the terminal.

**Who it's for:** developers who already use Claude Code or Codex and want to run several agents at once without their checkout, their laptop's memory or their attention falling apart.

**The three things to lead with**
1. **Light.** A 25 MB native app (Rust + GPUI, the UI framework Zed uses), about 70 MB of memory with a project open, on screen in about a tenth of a second.
2. **Safe parallel agents.** Every agent gets its own git worktree outside your repo. Your checkout never changes until you press Merge, and Merge never touches your staged changes or stash.
3. **You can see the work.** The file tree glows as agents read and edit, Live follows the file the agent is typing in, and you can take over at any moment.

---

## Key numbers

Use these as they are, with the footnote "Measured on a MacBook Air M5."

| What | Qur | Notes for the site |
|---|---|---|
| App size | **25 MB** (`Qur.app`) | Includes 13 language grammars (~11 MB of it) |
| Memory with a project open, no agents | **70 MB** (project only), **73 MB** with a small file open | Physical footprint, what Activity Monitor calls "Memory". Re-measured 2026-10-26: 66–69 MB, project open, no agents |
| Memory with 3 Claude agents working | **129 MB** (Qur's own process) | The agents' CLIs are separate processes and use their own memory |
| Start to first frame | **113 ms** | Cold start |
| Agent writes a file → the tree glows | **81 ms** | Measured with Qur in the background |
| Open a 10,000-line file, fully highlighted | **84 ms** | |
| Keystroke handled and drawn | **2.0 ms** (p99) | Well inside one frame at 120 Hz (8.3 ms) |
| Search 5,000 files | **51 ms** | |
| Network connections from Qur itself | **none** | Checked with `lsof` while agents ran. Agents and the Browser are separate processes |
| Agent processes left after you quit | **0** | |

**Comparison** (from each project's own releases and docs, October 2026):

| | Qur | Orca | Superset |
|---|---|---|---|
| Built with | Rust + GPUI, native | Electron + React | Electron + React |
| Download / app size | **25 MB** | ~254 MB app | ~431 MB download |
| Account needed | No | No | Yes (GitHub or Google) |
| Telemetry | None | Opt-out | PostHog and Sentry |
| License | GPL-3.0-or-later | MIT | Elastic License 2.0 |

Only compare sizes and these facts. We haven't measured Orca's or Superset's memory use, so don't claim a memory comparison until we have.

**Our own budgets** (good for a "built to stay light" section): every feature is weighed against these limits and measured before it ships. App under 40 MB, idle memory under 80 MB, 3 agents under 150 MB, first frame under 250 ms, typing under 8.3 ms per keystroke, search over 5,000 files under 300 ms. Since the fourth milestone, every feature has reused libraries already in the app instead of adding new ones.

---

## Features (shipped in v0.1.5)

### Agents
- **A worktree per agent.** Each task gets its own git worktree outside your repo, on a `qur/<agent>/<task>` branch. Run as many as you like; your checkout stays untouched.
- **Claude Code and Codex, your own login.** Uses the CLI logins you already have, never an API key. Run them as real terminals (Agent Terminals) or as structured sessions over the Agent Client Protocol.
- **Any CLI agent.** Aider or anything else that runs in a terminal: add it to `agents.toml`.
- **Agents name their own work.** The sidebar shows what each agent says it's doing; rename any time.
- **Permissions you control.** Structured agents ask in a permission bar: allow once, allow for this task, or deny. Several requests at once queue up, with "Allow all pending" for the impatient. Qur never turns on bypass flags by default.
- **A real chat for structured agents.** Streaming tool calls folded into tidy groups, the agent's to-do plan, Markdown with highlighted code, Find in Chat, Copy Chat as Markdown, queued messages, `@` file mentions and `/` agent commands.
- **Turn-end checks.** Run `cargo check`, your tests or anything else after each turn that changed files; failures go straight back into the prompt.
- **Alerts.** A notification, a Dock badge and a soft chime when an agent finishes, fails or needs you.

### Watching
- **Live file-tree heatmap.** Files glow while an agent edits them, fade while it reads them, and show `+n −m` once changed. Heat builds up with use, and folders roll up their contents.
- **Live view.** The file the agent is working in, as it types, read-only until you press **Take over**.
- **Browser with Inspect.** Your dev server next to the agent, using the system's WebKit (freed when idle). Click an element and the agent gets its selector, HTML, key CSS, source location and a screenshot.
- **Run Project in Worktree.** Each agent's dev server on its own port, so parallel agents never collide.

### Review and ship
- **Review and comment.** Read the task's diff, comment on lines, send the comments back as one message.
- **Turns and Rewind.** Every turn that changed files ends with "3 files · +12 −4 · View · Rewind". View shows just that turn; Rewind puts the files back.
- **Second opinion.** Another agent or model reviews the work read-only and rates each finding's criticality: Codex reviewing Claude, or the other way round.
- **Local Merge.** Merges into your branch without touching your staged changes or stash, warns about conflicts first, and resolves them in the app.
- **Git panel.** Stage, unstage and commit without leaving Qur.

### Editor and window
- **Code in a modal, not a permanent editor.** Open a file, edit with multi-cursor, find, Vim mode, Go to Symbol, then Esc back to the agents.
- **13 languages** with tree-sitter highlighting, **17 themes** plus Auto, accent colours, transparency and blur.
- **Window gaps.** Every pane as its own floating tile, like a tiling window manager.
- **A layout you arrange.** Drag panes and tabs like macOS window tiling, with a live drop preview, and the panes glide into place.
- **Chrome-style sidebar.** Always shown, auto-hidden, or hidden, on the left or the right.
- **Zen mode.** The bar slides away until you need it.
- **Project search and replace** across the whole tree in about 50 ms.
- **Keys you know.** VS Code's shortcuts, or rebind any of them in `keymap.toml`.

### Trust
- **Runs locally.** No account, no cloud, no telemetry, no background daemon. Qur itself makes no network connections.
- **Open source.** GPL-3.0-or-later.
- **Qur never writes into your repo.** Only your agents do, in their own worktrees, until you merge.

---

## Next release (built, not released)

- **Files and images in the chat.** Paste files copied in Finder or drop them on the prompt; paste screenshots and images. Images go to the agent as images, any other file as a link. Thumbnails and file chips stay under each message.
- **File viewer.** Click any image or file to see it in a modal, from the chat or the file tree, with Open in Default App and Reveal in Finder.
- **Smoother zen bar.** The window no longer re-lays itself out on every frame of the slide.

**Before marketing images:** right now, showing a large image keeps it in memory at full size until Qur quits (measured: +70 MB for one 1600×3065 image). The fix is being built. Don't advertise image features until it's in.

---

## v0.1.7 (built, not released)

- **More agents, built in.** Gemini CLI, opencode, GitHub Copilot and Cursor join Claude Code and Codex as structured agents (Agent Client Protocol). Settings lists each one as soon as it's installed; tick it to use it.
- **Add any agent from Settings.** Settings > Agents > Add agent: a name, the command, and whether it runs as a structured agent or in a terminal. No config file to edit. Agents added by hand in `agents.toml` show up in Settings too.
- **Agent dropdown in New Agent Task.** Pick the agent from one dropdown instead of a row of buttons, so the dialog stays tidy however many agents you have.

**Before putting these on the site:**
- None of the four new agents has been run in Qur yet. Only Claude Code and Codex have been tried. Try each one before naming it on the site.
- Copilot's ACP support is a public preview, and a GitHub issue (#845) reports it doesn't ask for permission before running tools. Don't say "permissions you control" about Copilot until that's checked.
- Gemini needs `@google/gemini-cli` 0.50.0 or newer. Cursor needs a sign-in first (`agent login`).
- Gemini, Copilot and Cursor sign in with their own accounts (Google, GitHub, Cursor), so "no API keys" still holds for them. opencode usually runs on an API key you bring, so leave it out of that claim.
- When these ship, update the FAQ answer to "Does it work with other agents?" and the requirements line.

---

## In progress (not for the site yet)

Being built now, each measured before and after:
- Lower memory: images decoded at the size they're drawn; old chat history kept on disk; idle agents release their processes and resume on click; closed files freed from memory; chats saved across restarts.
- Memory checks that fail the build if memory doesn't come back, and a memory readout in About and `qur doctor`.
- Plan mode with comments on the plan; a token and cost readout if the agents report it.
- Turns and Rewind for Agent Terminals.
- Inline diff review in the code modal: jump between an agent's changes and revert them one hunk at a time.
- Commit messages written from the agent's turns.
- Opening Qur without a project folder.
- Fixes: per-window terminal colours, pasting images copied from a browser, Vim `G`, and more accurate Changes and heat for the agent that works in the project folder.

Once these land, the strongest new claims will be **memory that comes back** (backed by checks that run on every build) and **chats that survive a restart**.

---

## Copy blocks

**Hero**
> **See what your agents see.**
> Qur runs your AI coding agents side by side, each in its own worktree, and lights up your file tree as they work. Native, 25 MB, no account.
> [Download for macOS]

**Light section**
> **Lighter than the tab you're reading this in.**
> Qur is a native app in Rust, not a web page in a box. It's 25 MB, opens in about a tenth of a second, and uses about 70 MB of memory with your project open. Three agents working at once: about 130 MB for Qur itself.

**Safety section**
> **Your checkout is off limits.**
> Every agent gets its own git worktree. Nothing touches your branch until you review the changes and press Merge, and Merge leaves your staged work and stash alone.

**Watching section**
> **Watch the work, not the terminal.**
> Files glow as agents read and write them. Live follows the agent's cursor. Take over whenever you want.

**Privacy section**
> **No account. No telemetry. No cloud.**
> Qur uses the Claude Code or Codex login you already have. Qur itself makes no network connections; we checked.

**FAQ**
- *Do I need an API key?* No. Qur uses your Claude Code or Codex CLI login.
- *Does it work with other agents?* Any CLI agent runs as an Agent Terminal. Claude Code and Codex also get a structured chat over the Agent Client Protocol.
- *Will it change my repo?* Not until you merge. Agents work in worktrees outside your repo.
- *Windows or Linux?* macOS only for now (macOS 13 or newer).
- *Is it free?* Yes, and open source under GPL-3.0-or-later.
- *Why "lightest"?* Every feature has a size and memory budget, measured before it ships. The numbers above are measurements, not estimates.

**Requirements:** macOS 13+, git 2.38+, Claude Code or Codex installed and signed in.

**Status:** pre-release (0.1.x). The download isn't notarized yet, so macOS asks you to allow it in System Settings → Privacy & Security the first time.
