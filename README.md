# Qur

**See what your agents see.** A native macOS editor for running AI coding agents: each one gets its own worktree, and the file tree lights up as it works.

Rust + GPUI · no Electron · runs locally · uses your Claude Code login, not an API key · pre-release 0.1, macOS only

![Live heatmap: files glow as the agent writes them; Live follows the file](docs/images/heatmap-live.png)

## How it works

![Start an agent, it works in its own worktree, watch files glow, review and merge](docs/images/flow.svg)

## The window

![Sidebar with agents, terminals, git and the file heatmap; the stage shows the agent's session and the file it's editing](docs/images/window.svg)

## Features

- **Browser + Inspect.** Your dev server, right next to the agent. Click an element to point at it.
- **Worktree per agent.** Run as many as you like. Your checkout never changes.
- **Port per agent.** Run Project starts each agent's app on its own port.
- **Heatmap.** See which files are being read and edited.
- **Live.** Follow the agent's cursor. Take over anytime.
- **Review → Merge.** Comment on lines, send them back, merge.
- **Checks.** `cargo check` (or anything) after every turn.
- **Alerts.** Notification and Dock badge when it's your turn.
- **Any CLI agent.** Claude Code, Aider, …
- **Editor.** 13 languages, multi-cursor, Vim, 17 themes.

## Light

![25 MB app vs 254 MB and 431 MB; 113 ms launch, 73 MB idle, 81 ms write to glow, no network connections](docs/images/numbers.svg)

## Install

Needs macOS 13+, git 2.38+, [Claude Code](https://claude.com/claude-code), Rust and Xcode.

```sh
git clone https://github.com/PEUR-Technologies/Qur.git && cd Qur
scripts/bundle.sh && cp -R target/release/Qur.app /Applications/
```

Open a folder, press `⌘⇧T`, describe the task.

| `⌘⇧T` Agent Terminal | `⌘⇧N` Structured agent | `⌘P` Go to file | `⌘⇧P` Commands | `⌘⇧F` Search |
|---|---|---|---|---|

## More

[Guide](docs/guide.md) (every feature, config, comparisons) · [Keys](docs/keymap.md) · [Menus](docs/menus.md) · [Status](HANDOFF.md) · [GPL-3.0-or-later](LICENSE) © PEUR Technologies
