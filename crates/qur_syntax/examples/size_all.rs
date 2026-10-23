//! Binary-size probe: links all 13 bundled grammars through qur_syntax.
fn main() {
    let src = std::env::args().nth(1).unwrap_or_default();
    for lang in qur_syntax::Language::ALL {
        let mut s = qur_syntax::SyntaxState::new(lang);
        s.parse(&src);
        println!("{} {}", lang.name(), s.highlights(&src, 0..src.len()).len());
    }
}
