//! Binary-size probe: tree-sitter + the TypeScript grammar only.
fn main() {
    let src = std::env::args().nth(1).unwrap_or_default();
    let mut p = tree_sitter::Parser::new();
    p.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()).unwrap();
    let q = tree_sitter::Query::new(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), tree_sitter_typescript::HIGHLIGHTS_QUERY).unwrap();
    let t = p.parse(&src, None).unwrap();
    println!("{} {}", t.root_node().kind(), q.pattern_count());
}
