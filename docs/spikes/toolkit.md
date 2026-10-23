# M0 spike: UI toolkit (GPUI vs iced)

Date: 2026-10-23. Machine: MacBook Air M5 (Mac17,3), macOS 27.0, built-in Liquid Retina display.
Raw output: `spikes/out/measure-gpui.txt`, `measure-iced.txt`, `measure-opacity.txt` (gitignored). Script: `scripts/spike_measure.sh`, `scripts/spike_measure_all.sh`.

## What was built (same feature set in both)

The same prototype twice, sharing `spikes/spike_common` (tree-sitter highlighting, file-tree model, metrics):

- **Window:** Zed-style transparent titlebar with our own top bar.
- **Sidebar:** 220 px, with agent rows and a virtualized tree of a 5,000-file repo.
- **Editor:** editable 10,000-line TypeScript file with tree-sitter highlighting, soft wrap, selections, undo, clipboard, find/replace and go-to-line.
- **Terminals:** `alacritty_terminal` panel with tabs. `Cmd+T` opens a shell; `Cmd+Shift+T` opens a Claude task in a new worktree.
- **Commands:** every S-tier command in `docs/keymap.md`, a command palette, and the full native menu bar from `docs/menus.md`.
- **Transparency:** slider and blur.

| | GPUI | iced |
|---|---|---|
| Version | Zed git rev `9e84b066` (`gpui` + `gpui_platform`, default features off) | 0.14.0 (`wgpu`, `advanced`, `thread-pool`, `crisp`) + `muda` 0.21.2 for the native menu |
| Crates in tree | 291 | 177 |
| Spike code | 4,812 lines | 4,085 lines |
| Clean release build | about 2 min (needs the Xcode Metal toolchain) | about 40 s |

## Measurements

Both binaries were measured back to back in one run. That run was **on battery**, and macOS ran both windows at 60 Hz. Earlier runs on AC paced at 120 Hz.

| Metric | Target | GPUI | iced |
|---|---|---|---|
| Release binary (stripped, incl. terminal) | < 40 MB | **8.5 MB** | 10.2 MB |
| Terminal's share of the binary | | about 0.5 MB | about 0.1 MB (+0.25 MB `muda`) |
| **Idle memory, physical footprint** (Activity Monitor "Memory") | < 80 MB | **71 MB ✅** | 120 MB ❌ |
| Idle RSS (`ps`; includes shared system-framework pages) | | 109–121 MB | 118–126 MB |
| GPU window buffers (IOSurface) | | 19.6 MB | 58.7 MB |
| Cold start to first frame, median of 5 warm runs | < 250 ms | **148 ms ✅** | 211 ms ✅ |
| Spawn to exit after first frame | | 232 ms | 288 ms |
| Open 10k-line file (read + highlight) | < 100 ms | 49 ms ✅ | 49 ms ✅ (same shared code) |
| Tree with all 6,766 rows expanded, first frame | | 149 ms | 215 ms |
| Typing: key handled through to frame presented (CPU side), p50 / p99 / max | ≤ 8.3 ms | **0.36 / 0.68 / 0.98 ms ✅** | not measured on the same basis (see note) |
| Typing: key to next displayed frame (includes the vsync wait), p50 / p99 | | | 7.9 / 9.2 ms (opacity run, 120 Hz) ; 16.7 / 33.7 ms (60 Hz run) |
| Typing: CPU draw only, p50 / p99 | | | 0.10–0.16 / 0.5–9.4 ms |
| Scroll, frame interval p50 / p99 (60 Hz run) | | 16.7 / 18.3–23.5 ms | 16.7 / 18.2–33.5 ms |
| Scroll, CPU work per frame p50 / p99 | | 2.3–2.4 / 3.1–9.8 ms | n/a |
| Opacity 0.85 + blur, cost | | none measurable | none measurable |
| Terminal spawn to first output drawn | | (not captured) | 44–78 ms |

Notes:
- **Memory metric.** The budget says "idle RSS". On macOS, RSS counts shared, clean pages of system frameworks (about 40 MB here) that every app maps. **Proposal: budget on physical footprint**, which is what Activity Monitor and macOS memory pressure use, and keep reporting RSS next to it. Needs owner approval (handoff section 4 says never to relax a target quietly).
- **Typing.** GPUI's number stops when the frame is handed to Metal. iced has no hook after present, so its number runs to the next frame callback and includes the vsync wait. Both are well under 8.3 ms of actual work. The display can't show 120 Hz on battery, so "one frame at 120 Hz" can only be judged on CPU time here.
- **One stall of about 1 s** happened in one run of each toolkit (GPUI scroll run, iced opacity scroll run). One per toolkit points to the system, not the toolkit. Re-check on AC.
- **Screenshots** for text quality failed: the terminal app running the script lacks Screen Recording permission. The owner should open both prototypes and compare text by eye (commands below).

## Capabilities that matter for Qur

| | GPUI | iced |
|---|---|---|
| Unified titlebar, inset traffic lights | ✅ position adjustable (Zed's setup) | ✅ but traffic lights fixed in place |
| Native menu bar with shortcuts, disabled and check items | ✅ built in (`set_menus`, `OsAction`) | ✅ via `muda`; Cut/Copy/Paste can't be the standard OS items (iced views don't respond to the system selectors), so Dictation/Emoji integration is weaker |
| Hide / Hide Others from code | ✅ | ❌ (menu only) |
| Live blur toggle | ✅ `WindowBackgroundAppearance::Blurred` | ❌ only at window creation |
| Text shaping / ligatures | ✅ CoreText (native macOS text) | ✅ cosmic-text (harfbuzz); not native macOS rendering |
| IME (CJK, dead keys) | input handler exists (Zed uses it); preedit not drawn in spike | none in iced's custom widgets |
| Reuse from Zed (GPL-compatible) | terminal view, editor patterns, keymap and actions system | none |
| Code-editor track record | Zed itself | no shipping editor of this scale |
| **Unfocused / occluded windows** | ⚠️ throttled to about 30 fps by default and seen stalling for seconds. Must be configured (`inactive_frame_interval`) so agent activity keeps updating while the user is in another app. Zed lives with this today. | not observed |
| Packaging and stability | ⚠️ not on crates.io in current form; pinned to a Zed git rev; API moves with Zed | crates.io release; slower but versioned |
| Build requirements | Xcode + Metal toolchain (839 MB download, one time) | none beyond Rust |

## Recommendation: **GPUI**

It's the lighter one where it counts:
- About **40 % less memory**: 71 vs 120 MB footprint. It's the only candidate inside the 80 MB budget.
- **Faster start**: 148 vs 211 ms.
- **Smaller binary**: 8.5 vs 10.2 MB.

It also gives Qur native macOS text, menus and blur, and lets us lift proven pieces from Zed (terminal view, keymap and actions, editor patterns) under our GPL license.

Risks and how to handle them:
1. **Background-window throttling.** In M1, configure the inactive frame interval and verify a heatmap glow lands within 100 ms with Qur unfocused and partly covered. Add that as an M3 acceptance check.
2. **Git-pinned dependency.** Pin one Zed rev in the workspace and upgrade on purpose: once per milestone at most, with the budget script re-run.
3. **Build cost.** The Metal toolchain is a one-time developer install. `qur doctor` and the README will say so.

iced's one advantage is a smaller, versioned dependency tree and a faster build. That doesn't outweigh 50 MB more memory and a 60 ms slower start in a product whose pitch is lightness.

## Re-run / view by eye

```sh
scripts/spike_measure_all.sh                       # waits for an unlocked screen, then measures both
spikes/toolkit_gpui/target/release/toolkit_gpui    # open GPUI prototype
spikes/toolkit_iced/target/release/toolkit_iced    # open iced prototype
QUR_SPIKE_OPACITY=0.85 QUR_SPIKE_BLUR=1 spikes/toolkit_gpui/target/release/toolkit_gpui
```
