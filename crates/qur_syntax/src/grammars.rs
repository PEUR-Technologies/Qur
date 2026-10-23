//! Bundled grammars, their queries, and the capture-name mapping.
//!
//! Each grammar's configuration (parser language + compiled highlight and
//! injection queries) is built lazily on first use and then shared.

use std::sync::OnceLock;

use tree_sitter::Query;

use crate::{HIGHLIGHT_NAMES, Language};

/// A parser grammar. Markdown needs two (block + inline), so this is wider
/// than the public [`Language`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Grammar {
    Lang(Language),
    MarkdownInline,
}

impl Grammar {
    const COUNT: usize = Language::ALL.len() + 1;

    fn index(self) -> usize {
        match self {
            Grammar::Lang(l) => l as usize,
            Grammar::MarkdownInline => Language::ALL.len(),
        }
    }

    /// Grammar for an injection language name (`injection.language` values and
    /// Markdown fence info strings).
    pub(crate) fn from_injection_name(name: &str) -> Option<Grammar> {
        let lang = match name.trim().to_ascii_lowercase().as_str() {
            "markdown_inline" => return Some(Grammar::MarkdownInline),
            "typescript" | "ts" | "mts" | "cts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "javascript" | "js" | "jsx" | "mjs" | "cjs" | "node" => Language::JavaScript,
            "rust" | "rs" => Language::Rust,
            "python" | "py" | "python3" => Language::Python,
            "json" | "jsonc" | "json5" => Language::Json,
            "css" => Language::Css,
            "html" | "htm" | "xhtml" => Language::Html,
            "markdown" | "md" => Language::Markdown,
            "toml" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            "kotlin" | "kt" | "kts" => Language::Kotlin,
            "dart" => Language::Dart,
            _ => return None,
        };
        Some(Grammar::Lang(lang))
    }
}

/// Capture value meaning "explicitly unhighlighted" (`@none`): paints over
/// outer captures but produces no span.
pub(crate) const NONE: u16 = u16::MAX;

pub(crate) struct Config {
    pub language: tree_sitter::Language,
    pub highlights: Query,
    /// Per capture index of `highlights`: HIGHLIGHT_NAMES index, NONE, or None (ignored).
    pub capture_map: Vec<Option<u16>>,
    pub injections: Option<Injections>,
}

pub(crate) struct Injections {
    pub query: Query,
    pub content: u32,
    /// `@injection.language` capture (fence info strings).
    pub language: Option<u32>,
    /// Per pattern: `#set! injection.language "..."`.
    pub pattern_language: Vec<Option<Grammar>>,
}

pub(crate) fn config(g: Grammar) -> &'static Config {
    static CONFIGS: [OnceLock<Config>; Grammar::COUNT] = [const { OnceLock::new() }; Grammar::COUNT];
    CONFIGS[g.index()].get_or_init(|| build(g))
}

pub(crate) fn build(g: Grammar) -> Config {
    use Language::*;
    let js_hl = tree_sitter_javascript::HIGHLIGHT_QUERY;
    let jsx_hl = tree_sitter_javascript::JSX_HIGHLIGHT_QUERY;
    let ts_hl = tree_sitter_typescript::HIGHLIGHTS_QUERY;
    // Highlight queries are concatenated base-first: for the same node the
    // later pattern wins (tree-sitter-highlight convention), so TS/JSX
    // patterns override the JavaScript ones they extend.
    let (language, highlights, injections): (tree_sitter::Language, String, Option<&str>) = match g {
        // tree-sitter-typescript's queries extend the JavaScript ones. The JS
        // 0.25 query references nodes the TS 0.23 grammar lacks, so TS/TSX use
        // the JS 0.23.1 query vendored in queries/ (see docs/THIRD_PARTY.md).
        Grammar::Lang(TypeScript) => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            format!("{JS_0_23_HIGHLIGHTS}\n{ts_hl}"),
            None,
        ),
        Grammar::Lang(Tsx) => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            format!("{JS_0_23_HIGHLIGHTS}\n{JS_0_23_JSX_HIGHLIGHTS}\n{ts_hl}"),
            None,
        ),
        Grammar::Lang(JavaScript) => {
            (tree_sitter_javascript::LANGUAGE.into(), format!("{js_hl}\n{jsx_hl}"), None)
        }
        Grammar::Lang(Rust) => {
            (tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY.into(), None)
        }
        Grammar::Lang(Python) => {
            (tree_sitter_python::LANGUAGE.into(), tree_sitter_python::HIGHLIGHTS_QUERY.into(), None)
        }
        Grammar::Lang(Json) => {
            (tree_sitter_json::LANGUAGE.into(), tree_sitter_json::HIGHLIGHTS_QUERY.into(), None)
        }
        Grammar::Lang(Css) => {
            (tree_sitter_css::LANGUAGE.into(), tree_sitter_css::HIGHLIGHTS_QUERY.into(), None)
        }
        Grammar::Lang(Html) => (
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY.into(),
            Some(tree_sitter_html::INJECTIONS_QUERY),
        ),
        Grammar::Lang(Markdown) => (
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
            Some(tree_sitter_md::INJECTION_QUERY_BLOCK),
        ),
        Grammar::MarkdownInline => (
            tree_sitter_md::INLINE_LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_INLINE.into(),
            Some(tree_sitter_md::INJECTION_QUERY_INLINE),
        ),
        // Upstream captures the whole `pair` as @property, so the inner
        // `bare_key @type` wins; Qur's patch makes keys properties.
        Grammar::Lang(Toml) => (
            tree_sitter_toml_ng::LANGUAGE.into(),
            format!("{}\n(pair (bare_key) @property)\n", tree_sitter_toml_ng::HIGHLIGHTS_QUERY),
            None,
        ),
        Grammar::Lang(Yaml) => {
            (tree_sitter_yaml::LANGUAGE.into(), tree_sitter_yaml::HIGHLIGHTS_QUERY.into(), None)
        }
        Grammar::Lang(Kotlin) => {
            (tree_sitter_kotlin_sg::LANGUAGE.into(), tree_sitter_kotlin_sg::HIGHLIGHTS_QUERY.into(), None)
        }
        Grammar::Lang(Dart) => {
            (tree_sitter_dart::LANGUAGE.into(), tree_sitter_dart::HIGHLIGHTS_QUERY.into(), None)
        }
    };

    let highlights = Query::new(&language, &highlights)
        .unwrap_or_else(|e| panic!("{g:?} highlight query: {e}"));
    let capture_map = highlights.capture_names().iter().map(|n| map_capture(n)).collect();
    let injections = injections.map(|src| {
        let query = Query::new(&language, src).unwrap_or_else(|e| panic!("{g:?} injection query: {e}"));
        let content = query.capture_index_for_name("injection.content").expect("injection.content");
        let language = query.capture_index_for_name("injection.language");
        let pattern_language = (0..query.pattern_count())
            .map(|i| {
                query
                    .property_settings(i)
                    .iter()
                    .find(|p| &*p.key == "injection.language")
                    .and_then(|p| p.value.as_deref())
                    .and_then(Grammar::from_injection_name)
            })
            .collect();
        Injections { query, content, language, pattern_language }
    });
    Config { language, highlights, capture_map, injections }
}

/// JavaScript 0.23.1 queries, for the TypeScript grammar (0.23.2).
const JS_0_23_HIGHLIGHTS: &str = include_str!("../queries/javascript-0.23.1/highlights.scm");
const JS_0_23_JSX_HIGHLIGHTS: &str = include_str!("../queries/javascript-0.23.1/highlights-jsx.scm");

/// Map a query capture name to a HIGHLIGHT_NAMES index.
///
/// Aliases first (nvim-treesitter / markup names used by some bundled
/// queries), then the longest dotted prefix present in HIGHLIGHT_NAMES.
pub(crate) fn map_capture(name: &str) -> Option<u16> {
    if name.starts_with('_') {
        return None; // helper captures used only by predicates
    }
    let alias = match name {
        "none" => return Some(NONE),
        "spell" | "nospell" | "conceal" | "text" | "markup" => return None,
        n if n.starts_with("local") || n.starts_with("injection") => return None,
        "text.title" | "markup.heading" => "title",
        n if n.starts_with("markup.heading") => "title",
        "text.emphasis" | "markup.italic" => "emphasis",
        "text.strong" | "markup.strong" | "markup.bold" => "emphasis.strong",
        "text.uri" | "markup.link.url" | "markup.link.uri" => "link_uri",
        "text.reference" | "markup.link" | "markup.link.label" | "markup.link.text" => "link_text",
        "markup.raw" | "markup.raw.block" | "markup.raw.inline" => "text.literal",
        "markup.list" | "markup.list.checked" | "markup.list.unchecked" => "punctuation.special",
        "markup.quote" => "comment",
        "comment.documentation" | "comment.block.documentation" => "comment.doc",
        "string.regexp" | "regexp" => "string.regex",
        "character" | "character.special" => "string",
        "float" | "number.float" | "constant.numeric" => "number",
        "parameter" => "variable.parameter",
        "field" | "variable.member" | "variable.other.member" => "property",
        "method" | "method.call" | "function.method.call" => "function.method",
        "macro" => "function.macro",
        "module" | "module.builtin" => "namespace",
        "tag.attribute" | "decorator" => "attribute",
        "tag.delimiter" => "punctuation.bracket",
        "conditional" | "repeat" | "include" | "exception" | "storageclass" | "type.qualifier" => "keyword",
        "symbol" => "string.special",
        _ => name,
    };
    let mut candidate = alias;
    loop {
        if let Some(i) = HIGHLIGHT_NAMES.iter().position(|h| *h == candidate) {
            return Some(i as u16);
        }
        candidate = &candidate[..candidate.rfind('.')?];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(n: &str) -> Option<&'static str> {
        map_capture(n).map(|i| if i == NONE { "<none>" } else { HIGHLIGHT_NAMES[i as usize] })
    }

    #[test]
    fn capture_mapping() {
        assert_eq!(name("function.call"), Some("function"));
        assert_eq!(name("string.special.url"), Some("string.special"));
        assert_eq!(name("markup.heading.1"), Some("title"));
        assert_eq!(name("text.title"), Some("title"));
        assert_eq!(name("keyword.control.return"), Some("keyword"));
        assert_eq!(name("type.builtin"), Some("type.builtin"));
        assert_eq!(name("none"), Some("<none>"));
        assert_eq!(name("_name"), None);
        assert_eq!(name("spell"), None);
        assert_eq!(name("nonsense"), None);
    }
}
