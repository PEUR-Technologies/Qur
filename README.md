# Qur

**See what your agents see.** A native macOS editor for running AI coding agents: each one gets its own worktree, and the file tree lights up as it works.

Rust + GPUI · no Electron · runs locally · uses your Claude Code login, not an API key · pre-release 0.1, macOS only

![Live heatmap: files glow as the agent writes them; Live follows the file](docs/images/heatmap-live.png)

## How it works

![Start an agent, it works in its own worktree, watch files glow, review and merge](docs/images/flow.svg)

## The window

![Sidebar with agents, terminals, git and the file heatmap; the stage shows the agent's session and the file it's editing](docs/images/window.svg)

![Browser + Inspect: pick an element, ask the agent](docs/images/browser-inspect.jpg)

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
