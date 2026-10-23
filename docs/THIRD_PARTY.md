# Third-party components

Qur is GPL-3.0-or-later. Bundled third-party code and data, with licenses.
Theme attributions are in `docs/THEME_CREDITS.md`.

## Syntax grammars and queries (`crates/qur_syntax`)

All grammars are compiled into the binary through their crates.io packages (C parser tables via `tree-sitter-language`). The highlight queries ship inside the same crates unless noted.

| Language | Crate (version) | Upstream | License | Highlight / injection queries |
|---|---|---|---|---|
| TypeScript, TSX | `tree-sitter-typescript` 0.23.2 | github.com/tree-sitter/tree-sitter-typescript | MIT | crate `HIGHLIGHTS_QUERY`, layered on the JavaScript **0.23.1** queries vendored in `crates/qur_syntax/queries/javascript-0.23.1/` (`highlights.scm`, `highlights-jsx.scm`, from github.com/tree-sitter/tree-sitter-javascript tag v0.23.1, MIT). The JS 0.25 queries name nodes the TS 0.23 grammar lacks. |
| JavaScript (incl. JSX) | `tree-sitter-javascript` 0.25.0 | github.com/tree-sitter/tree-sitter-javascript | MIT | crate `HIGHLIGHT_QUERY` + `JSX_HIGHLIGHT_QUERY` |
| Rust | `tree-sitter-rust` 0.24.2 | github.com/tree-sitter/tree-sitter-rust | MIT | crate `HIGHLIGHTS_QUERY` |
| Python | `tree-sitter-python` 0.25.0 | github.com/tree-sitter/tree-sitter-python | MIT | crate `HIGHLIGHTS_QUERY` |
| JSON | `tree-sitter-json` 0.24.8 | github.com/tree-sitter/tree-sitter-json | MIT | crate `HIGHLIGHTS_QUERY` |
| CSS | `tree-sitter-css` 0.25.0 | github.com/tree-sitter/tree-sitter-css | MIT | crate `HIGHLIGHTS_QUERY` |
| HTML | `tree-sitter-html` 0.23.2 | github.com/tree-sitter/tree-sitter-html | MIT | crate `HIGHLIGHTS_QUERY` + `INJECTIONS_QUERY` (`<script>` → JS, `<style>` → CSS) |
| Markdown (block + inline) | `tree-sitter-md` 0.5.3 | github.com/tree-sitter-grammars/tree-sitter-markdown | MIT | crate block/inline highlight + injection queries (inline, fenced code, HTML, YAML/TOML front matter); queries derived from nvim-treesitter (Apache-2.0) as noted in the files |
| TOML | `tree-sitter-toml-ng` 0.7.0 | github.com/tree-sitter-grammars/tree-sitter-toml | MIT | crate `HIGHLIGHTS_QUERY` + one Qur pattern `(pair (bare_key) @property)` so keys read as properties |
| YAML | `tree-sitter-yaml` 0.7.2 | github.com/tree-sitter-grammars/tree-sitter-yaml | MIT | crate `HIGHLIGHTS_QUERY` |
| Kotlin | `tree-sitter-kotlin-sg` 0.4.1 | github.com/ast-grep/tree-sitter-kotlin (fork of fwcd/tree-sitter-kotlin) | MIT | crate `HIGHLIGHTS_QUERY`. `tree-sitter-kotlin-ng` 1.1.0 was rejected: it ships no queries, and its repository has none. |
| Dart | `tree-sitter-dart` 0.2.0 | github.com/nielsenko/tree-sitter-dart | MIT | crate `HIGHLIGHTS_QUERY` (its injections query is disabled upstream) |

Runtime: `tree-sitter` 0.27.1 (MIT), `tree-sitter-language` 0.1.9 (MIT), `streaming-iterator` (MIT/Apache-2.0).

## Fonts

| Font | License | Source | Notes |
|---|---|---|---|
| Inter 4.1 (Regular, Medium, SemiBold) | SIL OFL 1.1 (`assets/fonts/Inter-LICENSE.txt`) | https://github.com/rsms/inter/releases/tag/v4.1 | Subset with fontTools to Latin, Latin-1/Ext-A, punctuation, arrows, macOS key symbols, box drawing (≈82 KB per weight) |

## UI toolkit and terminal

| Component | License | Source |
|---|---|---|
| GPUI (`gpui`, `gpui_platform`) | Apache-2.0 | Zed, pinned rev `9e84b066` |
| alacritty_terminal 0.26 | Apache-2.0 | https://github.com/alacritty/alacritty |
