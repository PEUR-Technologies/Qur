//! Tree-sitter syntax highlighting for Qur.
//!
//! One [`SyntaxState`] per open buffer. Parsing is incremental: the editor
//! reports each edit with [`SyntaxState::edit`], then calls
//! [`SyntaxState::reparse`] (cheap, can run off the UI thread on a cloned
//! state). Highlighting is per byte range, so the UI only queries the
//! visible rows.
//!
//! CONTRACT (M1): the public API below is used by `qur_app`; keep the
//! signatures. Implementation details are free.

mod grammars;

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Mutex;

use tree_sitter::{Node, Parser, Point, QueryCursor, StreamingIterator, Tree};

use grammars::{Grammar, NONE, config};

/// Languages bundled in v1 (handoff M1 list).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    TypeScript,
    Tsx,
    JavaScript,
    Rust,
    Python,
    Json,
    Css,
    Html,
    Markdown,
    Toml,
    Yaml,
    Kotlin,
    Dart,
}

impl Language {
    pub const ALL: [Language; 13] = [
        Language::TypeScript,
        Language::Tsx,
        Language::JavaScript,
        Language::Rust,
        Language::Python,
        Language::Json,
        Language::Css,
        Language::Html,
        Language::Markdown,
        Language::Toml,
        Language::Yaml,
        Language::Kotlin,
        Language::Dart,
    ];

    /// By file extension or well-known file name (`Cargo.lock` -> Toml, ...).
    pub fn for_path(path: &Path) -> Option<Language> {
        let name = path.file_name()?.to_str()?;
        match name {
            "Cargo.lock" | "Pipfile" | "poetry.lock" | "uv.lock" => return Some(Language::Toml),
            ".babelrc" | ".prettierrc" | ".eslintrc" | ".swcrc" => return Some(Language::Json),
            ".clang-format" | ".clangd" => return Some(Language::Yaml),
            _ => {}
        }
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "ts" | "mts" | "cts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "rs" => Language::Rust,
            "py" | "pyi" | "pyw" => Language::Python,
            "json" | "jsonc" | "json5" | "webmanifest" => Language::Json,
            "css" => Language::Css,
            "html" | "htm" | "xhtml" => Language::Html,
            "md" | "markdown" | "mdown" | "mkd" => Language::Markdown,
            "toml" => Language::Toml,
            "yml" | "yaml" => Language::Yaml,
            "kt" | "kts" => Language::Kotlin,
            "dart" => Language::Dart,
            _ => return None,
        })
    }

    /// Display name ("TypeScript").
    pub fn name(self) -> &'static str {
        match self {
            Language::TypeScript => "TypeScript",
            Language::Tsx => "TSX",
            Language::JavaScript => "JavaScript",
            Language::Rust => "Rust",
            Language::Python => "Python",
            Language::Json => "JSON",
            Language::Css => "CSS",
            Language::Html => "HTML",
            Language::Markdown => "Markdown",
            Language::Toml => "TOML",
            Language::Yaml => "YAML",
            Language::Kotlin => "Kotlin",
            Language::Dart => "Dart",
        }
    }

    /// Line-comment prefix used by Toggle Line Comment (`//`, `#`), if any.
    pub fn line_comment(self) -> Option<&'static str> {
        match self {
            Language::TypeScript
            | Language::Tsx
            | Language::JavaScript
            | Language::Rust
            | Language::Kotlin
            | Language::Dart => Some("//"),
            Language::Python | Language::Toml | Language::Yaml => Some("#"),
            Language::Json | Language::Css | Language::Html | Language::Markdown => None,
        }
    }

    /// Block comment delimiters (`/*`, `*/`; `<!--`, `-->`), if any.
    pub fn block_comment(self) -> Option<(&'static str, &'static str)> {
        match self {
            Language::TypeScript
            | Language::Tsx
            | Language::JavaScript
            | Language::Rust
            | Language::Kotlin
            | Language::Dart
            | Language::Css => Some(("/*", "*/")),
            Language::Html | Language::Markdown => Some(("<!--", "-->")),
            Language::Python | Language::Toml | Language::Yaml | Language::Json => None,
        }
    }

    /// Indent unit for new files of this language ("  " or "    ").
    pub fn indent_unit(self) -> &'static str {
        match self {
            Language::Rust | Language::Python | Language::Kotlin => "    ",
            _ => "  ",
        }
    }
}

/// Highlight capture names Qur understands. Themes map these to colors
/// (`qur_theme`), with dotted fallback ("function.method" -> "function").
/// Index into this slice == [`HighlightSpan::highlight`].
pub const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "comment.doc",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "emphasis",
    "emphasis.strong",
    "escape",
    "function",
    "function.builtin",
    "function.method",
    "function.macro",
    "keyword",
    "label",
    "link_text",
    "link_uri",
    "namespace",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.regex",
    "string.special",
    "tag",
    "text.literal",
    "title",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// A highlighted byte range of the buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    /// Index into [`HIGHLIGHT_NAMES`].
    pub highlight: usize,
}

/// Mirrors `tree_sitter::InputEdit` so callers don't depend on tree-sitter.
/// Points are (row, byte column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: (usize, usize),
    pub old_end_point: (usize, usize),
    pub new_end_point: (usize, usize),
}

/// Read access to buffer text in chunks (a rope implements this cheaply).
pub trait TextSource {
    /// Total length in bytes.
    fn len(&self) -> usize;
    /// The chunk containing `byte` and the byte offset where it starts.
    fn chunk_at(&self, byte: usize) -> (&str, usize);
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl TextSource for str {
    fn len(&self) -> usize {
        str::len(self)
    }
    fn chunk_at(&self, _byte: usize) -> (&str, usize) {
        (self, 0)
    }
}

/// `&str` itself is unsized and can't become `&dyn TextSource`; pass
/// `&string` or `&"literal"` instead.
impl TextSource for String {
    fn len(&self) -> usize {
        String::len(self)
    }
    fn chunk_at(&self, _byte: usize) -> (&str, usize) {
        (self, 0)
    }
}

impl TextSource for &str {
    fn len(&self) -> usize {
        str::len(self)
    }
    fn chunk_at(&self, _byte: usize) -> (&str, usize) {
        (self, 0)
    }
}

#[cfg(feature = "rope")]
impl TextSource for ropey::Rope {
    fn len(&self) -> usize {
        self.len_bytes()
    }
    fn chunk_at(&self, byte: usize) -> (&str, usize) {
        let (chunk, start, _, _) = self.chunk_at_byte(byte.min(self.len_bytes()));
        (chunk, start)
    }
}

/// Bytes of `text` from `byte` to the end of its chunk ("" at the end).
fn bytes_from(text: &dyn TextSource, byte: usize) -> &[u8] {
    if byte >= text.len() {
        return &[];
    }
    let (chunk, start) = text.chunk_at(byte);
    &chunk.as_bytes()[byte - start..]
}

/// Iterates the chunks covering a byte range (TextProvider for query predicates).
struct Chunks<'a> {
    text: &'a dyn TextSource,
    pos: usize,
    end: usize,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<&'a [u8]> {
        if self.pos >= self.end {
            return None;
        }
        let rest = bytes_from(self.text, self.pos);
        if rest.is_empty() {
            return None;
        }
        let n = rest.len().min(self.end - self.pos);
        self.pos += n;
        Some(&rest[..n])
    }
}

fn text_of(text: &dyn TextSource, range: Range<usize>) -> String {
    let mut out = Vec::with_capacity(range.len());
    for c in (Chunks { text, pos: range.start, end: range.end }) {
        out.extend_from_slice(c);
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn new_parser(g: Grammar) -> Parser {
    let mut p = Parser::new();
    p.set_language(&config(g).language).expect("grammar ABI compatible with tree-sitter");
    p
}

fn parse(g: Grammar, text: &dyn TextSource, old: Option<&Tree>, ranges: Option<&[tree_sitter::Range]>) -> Option<Tree> {
    let mut parser = new_parser(g);
    if let Some(r) = ranges {
        parser.set_included_ranges(r).ok()?;
    }
    parser.parse_with_options(&mut |byte, _: Point| bytes_from(text, byte), old, None)
}

/// Injected-language trees, keyed by (grammar, content start, content end).
/// Built lazily for the ranges actually highlighted; cleared on any edit.
#[derive(Default)]
struct InjectionCache {
    trees: HashMap<(Grammar, usize, usize), Option<Tree>>,
}

const INJECTION_CACHE_MAX: usize = 1024;
const INJECTION_DEPTH_MAX: usize = 3;

/// Parse state for one buffer. `Clone` is cheap (tree-sitter trees are refcounted),
/// so a clone can be reparsed on a background thread and swapped back in.
pub struct SyntaxState {
    language: Language,
    tree: Option<Tree>,
    /// Bounding range (new coordinates) of edits since the last parse.
    dirty: Option<Range<usize>>,
    injections: Mutex<InjectionCache>,
}

impl Clone for SyntaxState {
    fn clone(&self) -> Self {
        SyntaxState {
            language: self.language,
            tree: self.tree.clone(),
            dirty: self.dirty.clone(),
            // Injection trees are cheap to rebuild for the visible range.
            injections: Mutex::new(InjectionCache::default()),
        }
    }
}

impl SyntaxState {
    pub fn new(language: Language) -> Self {
        SyntaxState { language, tree: None, dirty: None, injections: Mutex::new(InjectionCache::default()) }
    }

    pub fn language(&self) -> Language {
        self.language
    }

    /// Full parse (file open). Must keep 10k lines of TypeScript well under 100 ms.
    pub fn parse(&mut self, text: &dyn TextSource) {
        self.tree = parse(Grammar::Lang(self.language), text, None, None);
        self.dirty = None;
        self.clear_injections();
    }

    /// Record an edit on the current tree (no parsing yet).
    pub fn edit(&mut self, edit: &TextEdit) {
        if let Some(tree) = &mut self.tree {
            tree.edit(&tree_sitter::InputEdit {
                start_byte: edit.start_byte,
                old_end_byte: edit.old_end_byte,
                new_end_byte: edit.new_end_byte,
                start_position: Point::new(edit.start_point.0, edit.start_point.1),
                old_end_position: Point::new(edit.old_end_point.0, edit.old_end_point.1),
                new_end_position: Point::new(edit.new_end_point.0, edit.new_end_point.1),
            });
        }
        // Shift the pending dirty range into post-edit coordinates, then grow it.
        let map = |x: usize| {
            if x >= edit.old_end_byte {
                x - edit.old_end_byte + edit.new_end_byte
            } else if x > edit.start_byte {
                edit.new_end_byte.min(x).max(edit.start_byte)
            } else {
                x
            }
        };
        let new = edit.start_byte..edit.new_end_byte;
        self.dirty = Some(match self.dirty.take() {
            Some(r) => map(r.start).min(new.start)..map(r.end).max(new.end),
            None => new,
        });
        self.clear_injections();
    }

    /// Incremental reparse after one or more `edit` calls. Returns the byte
    /// ranges whose highlighting may have changed (to invalidate caches).
    #[allow(clippy::single_range_in_vec_init)] // one range covering the text
    pub fn reparse(&mut self, text: &dyn TextSource) -> Vec<Range<usize>> {
        let Some(old) = self.tree.take() else {
            self.parse(text);
            return vec![0..text.len()];
        };
        let new = parse(Grammar::Lang(self.language), text, Some(&old), None);
        let mut ranges: Vec<Range<usize>> = match &new {
            Some(new) => old.changed_ranges(new).map(|r| r.start_byte..r.end_byte).collect(),
            None => vec![0..text.len()],
        };
        if let Some(d) = self.dirty.take() {
            // Edits inside a token change its extent without a syntactic
            // change, so the edited text itself is always reported.
            ranges.push(d);
        }
        self.tree = new;
        self.clear_injections();
        merge_ranges(ranges)
    }

    /// Highlights intersecting `range`, sorted, non-overlapping (innermost
    /// capture wins). Includes injected languages where bundled (e.g. CSS/JS
    /// inside HTML, code fences in Markdown when the fence language is bundled).
    pub fn highlights(&self, text: &dyn TextSource, range: Range<usize>) -> Vec<HighlightSpan> {
        let end = range.end.min(text.len());
        let start = range.start.min(end);
        let Some(tree) = &self.tree else { return Vec::new() };
        if start == end {
            return Vec::new();
        }
        let mut marks = Vec::new();
        let mut cache = self.injections.lock().unwrap_or_else(|e| e.into_inner());
        collect(Grammar::Lang(self.language), tree, text, start..end, 0, &mut marks, &mut cache);
        if cache.trees.len() > INJECTION_CACHE_MAX {
            cache.trees.clear();
        }
        drop(cache);
        paint(marks, start..end)
    }

    /// Byte range of the bracket matching the one at `byte`, if any.
    pub fn matching_bracket(&self, text: &dyn TextSource, byte: usize) -> Option<Range<usize>> {
        let ch = *bytes_from(text, byte).first()?;
        let (open, close, forward) = match ch {
            b'(' => (b'(', b')', true),
            b'[' => (b'[', b']', true),
            b'{' => (b'{', b'}', true),
            b')' => (b'(', b')', false),
            b']' => (b'[', b']', false),
            b'}' => (b'{', b'}', false),
            _ => return None,
        };
        if let Some(tree) = &self.tree
            && let Some(r) = tree_bracket(tree, byte, open, close, forward)
        {
            return Some(r);
        }
        scan_bracket(text, byte, open, close, forward)
    }

    fn clear_injections(&mut self) {
        self.injections.get_mut().unwrap_or_else(|e| e.into_inner()).trees.clear();
    }
}

/// A capture to paint: lower `rank` paints first, so higher ranks win.
struct Mark {
    range: Range<usize>,
    value: u16,
    rank: (usize, std::cmp::Reverse<usize>, usize),
}

/// Gather highlight captures of `tree` (and its injections) intersecting `range`.
fn collect(
    g: Grammar,
    tree: &Tree,
    text: &dyn TextSource,
    range: Range<usize>,
    depth: usize,
    marks: &mut Vec<Mark>,
    cache: &mut InjectionCache,
) {
    let cfg = config(g);
    let provider = |n: Node| Chunks { text, pos: n.start_byte(), end: n.end_byte() };
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let mut matches = cursor.matches(&cfg.highlights, tree.root_node(), provider);
    while let Some(m) = matches.next() {
        for c in m.captures() {
            let Some(value) = cfg.capture_map[c.index as usize] else { continue };
            let r = c.node.byte_range();
            if r.end <= range.start || r.start >= range.end || r.is_empty() {
                continue;
            }
            marks.push(Mark {
                rank: (depth, std::cmp::Reverse(r.len()), m.pattern_index),
                range: r,
                value,
            });
        }
    }

    let Some(inj) = &cfg.injections else { return };
    if depth >= INJECTION_DEPTH_MAX {
        return;
    }
    let mut sites: Vec<(Grammar, Vec<tree_sitter::Range>)> = Vec::new();
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let mut matches = cursor.matches(&inj.query, tree.root_node(), provider);
    while let Some(m) = matches.next() {
        let mut lang = inj.pattern_language[m.pattern_index];
        let mut content = Vec::new();
        for c in m.captures() {
            if Some(c.index) == inj.language {
                let name = text_of(text, c.node.byte_range());
                lang = Grammar::from_injection_name(name.split_whitespace().next().unwrap_or(""));
            } else if c.index == inj.content {
                content.push(c.node.range());
            }
        }
        if let Some(lang) = lang
            && !content.is_empty()
        {
            sites.push((lang, content));
        }
    }
    for (lang, ranges) in sites {
        let key = (lang, ranges[0].start_byte, ranges[ranges.len() - 1].end_byte);
        let sub = cache
            .trees
            .entry(key)
            .or_insert_with(|| parse(lang, text, None, Some(&ranges)))
            .clone();
        if let Some(sub) = sub {
            let r = range.start.max(key.1)..range.end.min(key.2);
            if !r.is_empty() {
                collect(lang, &sub, text, r, depth + 1, marks, cache);
            }
        }
    }
}

/// Resolve overlapping captures (deepest injection layer, then innermost node,
/// then the *later* pattern wins, as in tree-sitter-highlight) into sorted, non-overlapping spans clipped to `range`.
fn paint(mut marks: Vec<Mark>, range: Range<usize>) -> Vec<HighlightSpan> {
    const EMPTY: u16 = u16::MAX - 1;
    marks.sort_by_key(|m| m.rank);
    let mut cells = vec![EMPTY; range.len()];
    for m in &marks {
        let s = m.range.start.max(range.start) - range.start;
        let e = m.range.end.min(range.end) - range.start;
        cells[s..e].fill(m.value);
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let v = cells[i];
        let mut j = i + 1;
        while j < cells.len() && cells[j] == v {
            j += 1;
        }
        if v != EMPTY && v != NONE {
            out.push(HighlightSpan { range: range.start + i..range.start + j, highlight: v as usize });
        }
        i = j;
    }
    out
}

fn merge_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.retain(|r| r.start <= r.end);
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

fn tree_bracket(tree: &Tree, byte: usize, open: u8, close: u8, forward: bool) -> Option<Range<usize>> {
    let node = tree.root_node().descendant_for_byte_range(byte, byte + 1)?;
    let (o, c) = ([open], [close]);
    let (o, c) = (std::str::from_utf8(&o).ok()?, std::str::from_utf8(&c).ok()?);
    let want = if forward { o } else { c };
    if node.is_named() || node.kind() != want || node.start_byte() != byte {
        return None;
    }
    let parent = node.parent()?;
    let mut cursor = parent.walk();
    let kids: Vec<Node> = parent.children(&mut cursor).collect();
    let pos = kids.iter().position(|k| k.id() == node.id())?;
    let mut depth = 0i32;
    let iter: Box<dyn Iterator<Item = &Node>> =
        if forward { Box::new(kids[pos..].iter()) } else { Box::new(kids[..=pos].iter().rev()) };
    for k in iter {
        if k.kind() == o {
            depth += if forward { 1 } else { -1 };
        } else if k.kind() == c {
            depth += if forward { -1 } else { 1 };
        }
        if depth == 0 {
            return Some(k.byte_range());
        }
    }
    None
}

/// Plain scan fallback (no tree, or brackets inside an error node).
/// Bounded to 1 MiB in either direction.
fn scan_bracket(text: &dyn TextSource, byte: usize, open: u8, close: u8, forward: bool) -> Option<Range<usize>> {
    const LIMIT: usize = 1 << 20;
    let mut depth = 0i32;
    if forward {
        let mut pos = byte;
        let end = text.len().min(byte + LIMIT);
        while pos < end {
            let chunk = bytes_from(text, pos);
            for (i, &b) in chunk.iter().enumerate().take(end - pos) {
                if b == open {
                    depth += 1;
                } else if b == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(pos + i..pos + i + 1);
                    }
                }
            }
            pos += chunk.len();
        }
    } else {
        let lo = byte.saturating_sub(LIMIT);
        let mut pos = byte + 1; // exclusive end of the window still to scan
        while pos > lo {
            let (chunk, start) = text.chunk_at(pos - 1);
            let s = start.max(lo);
            for i in (s..pos).rev() {
                let b = chunk.as_bytes()[i - start];
                if b == close {
                    depth += 1;
                } else if b == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i..i + 1);
                    }
                }
            }
            pos = s;
        }
    }
    None
}

#[cfg(test)]
mod tests;
