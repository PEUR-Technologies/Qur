//! Go to Symbol and Expand/Shrink Selection, from the tree that highlighting
//! already keeps. Symbols come from node kinds per language (no extra query
//! to compile), walked once with a cursor when the picker opens.

use std::ops::Range;

use tree_sitter::Node;

use crate::{Language, SyntaxState, TextSource, bytes_from, text_of};

/// One entry of a file's outline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// What the picker shows ("parse", "impl Display for Foo", "## Usage").
    pub name: String,
    /// Short kind label ("fn", "class", "method", "heading"…).
    pub kind: &'static str,
    /// Byte range of the whole definition.
    pub range: Range<usize>,
    /// Byte range of the name (where the cursor goes).
    pub name_range: Range<usize>,
    /// Enclosing symbols (0 = top level).
    pub depth: usize,
}

/// Symbols beyond this are dropped (a 50k-key JSON file stays responsive).
const MAX_SYMBOLS: usize = 10_000;
const MAX_NAME: usize = 80;

impl SyntaxState {
    /// The file's symbols in document order (empty before the first parse).
    pub fn symbols(&self, text: &dyn TextSource) -> Vec<Symbol> {
        let Some(tree) = &self.tree else { return Vec::new() };
        let lang = self.language;
        let mut out = Vec::new();
        // Each open symbol's end byte, to count depth.
        let mut open: Vec<usize> = Vec::new();
        let mut cursor = tree.walk();
        'walk: loop {
            let node = cursor.node();
            if node.is_named()
                && let Some(kind) = kind_of(lang, node, text)
                && let Some((name, name_range)) = name_of(lang, node, text)
            {
                while open.last().is_some_and(|&end| end <= node.start_byte()) {
                    open.pop();
                }
                // Markdown sections don't nest headings: indent by level.
                let depth = match lang {
                    Language::Markdown => name.bytes().take_while(|&b| b == b'#').count().saturating_sub(1),
                    _ => open.len(),
                };
                out.push(Symbol { name, kind, range: node.byte_range(), name_range, depth });
                open.push(node.end_byte());
                if out.len() >= MAX_SYMBOLS {
                    break;
                }
            }
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    continue 'walk;
                }
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
        out
    }

    /// The smallest syntax node that strictly contains `range` (byte offsets),
    /// for Expand Selection. An empty range just after a word picks that word.
    /// `None` without a tree or when only the whole file is left.
    pub fn expand_range(&self, text: &dyn TextSource, range: Range<usize>) -> Option<Range<usize>> {
        let tree = self.tree.as_ref()?;
        let root = tree.root_node();
        let (mut start, mut end) = (range.start, range.end);
        if start == end {
            // Caret between a word and punctuation ("foo|;"): prefer the word.
            let at = bytes_from(text, start).first().copied();
            let before = start.checked_sub(1).and_then(|b| bytes_from(text, b).first().copied());
            if !at.is_some_and(is_word_byte) && before.is_some_and(is_word_byte) {
                start -= 1;
            } else if at.is_some() {
                end += 1;
            }
        }
        // Any node containing the one-byte probe also contains the caret.
        let mut node = root.named_descendant_for_byte_range(start, end)?;
        loop {
            let r = node.byte_range();
            if r.start <= range.start && r.end >= range.end && r != range && !r.is_empty() {
                return Some(r);
            }
            node = node.parent()?;
        }
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// The kind label for a node that is a symbol in `lang`, if it is one.
fn kind_of(lang: Language, n: Node, text: &dyn TextSource) -> Option<&'static str> {
    let k = n.kind();
    Some(match lang {
        Language::Rust => match k {
            "function_item" | "function_signature_item" => {
                if inside(n, &["impl_item", "trait_item"]) { "method" } else { "fn" }
            }
            "struct_item" => "struct",
            "enum_item" => "enum",
            "union_item" => "union",
            "trait_item" => "trait",
            "impl_item" => "impl",
            "mod_item" => "mod",
            "type_item" => "type",
            "const_item" => "const",
            "static_item" => "static",
            "macro_definition" => "macro",
            _ => return None,
        },
        Language::Python => match k {
            "function_definition" => if inside(n, &["class_definition"]) { "method" } else { "def" },
            "class_definition" => "class",
            _ => return None,
        },
        Language::TypeScript | Language::Tsx | Language::JavaScript => match k {
            "function_declaration" | "generator_function_declaration" | "function_signature" => "function",
            "class_declaration" | "abstract_class_declaration" | "class" => "class",
            "method_definition" | "method_signature" | "abstract_method_signature" => "method",
            "interface_declaration" => "interface",
            "type_alias_declaration" => "type",
            "enum_declaration" => "enum",
            "internal_module" | "module" => "namespace",
            "variable_declarator" => {
                let v = n.child_by_field_name("value")?;
                match v.kind() {
                    "arrow_function" | "function_expression" | "function" | "generator_function" => "function",
                    "class" => "class",
                    _ => return None,
                }
            }
            _ => return None,
        },
        Language::Kotlin => match k {
            "function_declaration" => if inside(n, &["class_body", "enum_class_body"]) { "method" } else { "fun" },
            "class_declaration" => "class",
            "object_declaration" => "object",
            "companion_object" => "companion",
            "type_alias" => "typealias",
            "property_declaration" if top_or_member(n) => "val",
            _ => return None,
        },
        Language::Dart => match k {
            "class_declaration" | "class_definition" => "class",
            "function_declaration" => "function",
            "method_declaration" => "method",
            "enum_declaration" => "enum",
            "mixin_declaration" => "mixin",
            "extension_declaration" => "extension",
            "type_alias" => "typedef",
            "constructor_signature" | "factory_constructor_signature" => "constructor",
            _ => return None,
        },
        Language::Css => match k {
            "rule_set" => "rule",
            "media_statement" => "@media",
            "keyframes_statement" => "@keyframes",
            "supports_statement" => "@supports",
            _ => return None,
        },
        Language::Html => match k {
            "element" | "script_element" | "style_element" if html_id(n, text).is_some() => "element",
            _ => return None,
        },
        Language::Markdown => match k {
            "atx_heading" | "setext_heading" => "heading",
            _ => return None,
        },
        Language::Json => match k {
            "pair" => "key",
            _ => return None,
        },
        Language::Toml => match k {
            "table" => "table",
            "table_array_element" => "array",
            "pair" => "key",
            _ => return None,
        },
        Language::Yaml => match k {
            "block_mapping_pair" | "flow_pair" => "key",
            _ => return None,
        },
    })
}

/// Whether `n` sits in one of `kinds` (a class, impl…) rather than in a
/// function body (a nested function is not a method).
fn inside(n: Node, kinds: &[&str]) -> bool {
    let mut p = n.parent();
    while let Some(x) = p {
        if kinds.contains(&x.kind()) {
            return true;
        }
        if x.kind().contains("function") {
            return false;
        }
        p = x.parent();
    }
    false
}

/// Kotlin properties: only top-level and class members, not locals.
fn top_or_member(n: Node) -> bool {
    n.parent().is_some_and(|p| matches!(p.kind(), "source_file" | "class_body" | "enum_class_body"))
}

/// The `id` attribute of an HTML element's start tag.
fn html_id<'t>(n: Node<'t>, text: &dyn TextSource) -> Option<Node<'t>> {
    let start = n.child(0).filter(|c| c.kind() == "start_tag")?;
    let mut c = start.walk();
    start.named_children(&mut c).find(|a| {
        a.kind() == "attribute"
            && a.named_child(0).is_some_and(|name| text_of(text, name.byte_range()).eq_ignore_ascii_case("id"))
    })
}

const NAME_KINDS: &[&str] = &[
    "identifier",
    "type_identifier",
    "simple_identifier",
    "property_identifier",
    "field_identifier",
    "bare_key",
    "dotted_key",
    "quoted_key",
    "keyframes_name",
];

/// The display name and its byte range.
fn name_of(lang: Language, n: Node, text: &dyn TextSource) -> Option<(String, Range<usize>)> {
    let show = |r: Range<usize>| clean(&text_of(text, r));
    match (lang, n.kind()) {
        (Language::Rust, "impl_item") => {
            let ty = n.child_by_field_name("type")?;
            let name = match n.child_by_field_name("trait") {
                Some(t) => format!("impl {} for {}", show(t.byte_range()), show(ty.byte_range())),
                None => format!("impl {}", show(ty.byte_range())),
            };
            return Some((name, ty.byte_range()));
        }
        (Language::Css, "rule_set") => {
            let s = n.named_child(0).filter(|c| c.kind() == "selectors")?;
            return Some((show(s.byte_range()), s.byte_range()));
        }
        (Language::Css, "media_statement" | "supports_statement") => {
            let body = n.named_children(&mut n.walk()).find(|c| c.kind() == "block")?;
            let r = n.start_byte()..body.start_byte();
            return Some((show(r.clone()), r));
        }
        (Language::Kotlin, "companion_object") => {
            let r = n.byte_range();
            let name = n.named_children(&mut n.walk()).find(|c| c.kind() == "type_identifier");
            return Some(match name {
                Some(id) => (format!("companion {}", show(id.byte_range())), id.byte_range()),
                None => ("companion object".into(), r.start..r.start),
            });
        }
        (Language::Kotlin, "property_declaration") => {
            let v = n.named_children(&mut n.walk()).find(|c| c.kind() == "variable_declaration")?;
            let id = v.named_child(0)?;
            return Some((show(id.byte_range()), id.byte_range()));
        }
        (Language::Dart, "function_declaration" | "method_declaration") => {
            let mut sig = n.child_by_field_name("signature").or_else(|| n.named_child(0))?;
            // method_signature wraps function_signature / getter_signature…
            if sig.child_by_field_name("name").is_none()
                && let Some(inner) = sig.named_child(0)
            {
                sig = inner;
            }
            let id = sig.child_by_field_name("name")?;
            return Some((show(id.byte_range()), id.byte_range()));
        }
        (Language::Html, _) => {
            let attr = html_id(n, text)?;
            let value = attr.named_child(1)?;
            let v = value.named_child(0).unwrap_or(value);
            let tag = n.child(0).and_then(|s| s.named_child(0)).map(|t| show(t.byte_range())).unwrap_or_default();
            return Some((format!("{tag}#{}", show(v.byte_range())), v.byte_range()));
        }
        (Language::Markdown, _) => {
            let c = n.child_by_field_name("heading_content")?;
            let level = match n.named_child(0).map(|m| m.kind()) {
                Some(m) if m.starts_with("atx_h") => m.as_bytes().get(5).map(|b| (b - b'0') as usize).unwrap_or(1),
                _ if n.named_children(&mut n.walk()).any(|c| c.kind() == "setext_h2_underline") => 2,
                _ => 1,
            };
            return Some((format!("{} {}", "#".repeat(level), show(c.byte_range())), c.byte_range()));
        }
        (Language::Json, "pair") => {
            let k = n.child_by_field_name("key")?;
            let inner = k.named_child(0).unwrap_or(k);
            return Some((show(inner.byte_range()), k.byte_range()));
        }
        (Language::Yaml, _) => {
            let k = n.child_by_field_name("key")?;
            return Some((show(k.byte_range()), k.byte_range()));
        }
        (Language::Toml, "table" | "table_array_element") => {
            let k = n.named_child(0)?;
            let wrap = if n.kind() == "table" { ("[", "]") } else { ("[[", "]]") };
            return Some((format!("{}{}{}", wrap.0, show(k.byte_range()), wrap.1), k.byte_range()));
        }
        _ => {}
    }
    let id = n.child_by_field_name("name").or_else(|| {
        let mut c = n.walk();
        n.named_children(&mut c).find(|c| NAME_KINDS.contains(&c.kind()))
    })?;
    let name = show(id.byte_range());
    if name.is_empty() {
        return None;
    }
    Some((name, id.byte_range()))
}

/// First line, trimmed, at most `MAX_NAME` chars.
fn clean(s: &str) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.trim_matches('"');
    if line.chars().count() > MAX_NAME {
        let mut t: String = line.chars().take(MAX_NAME - 1).collect();
        t.push('…');
        t
    } else {
        line.to_string()
    }
}
