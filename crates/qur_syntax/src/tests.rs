use super::*;
use std::time::Instant;

/// Highlight names covering any byte of `needle` in `src`.
fn names_at(state: &SyntaxState, src: &str, needle: &str) -> Vec<&'static str> {
    let at = src.find(needle).unwrap_or_else(|| panic!("{needle:?} not in sample"));
    let r = at..at + needle.len();
    state
        .highlights(&src, 0..src.len())
        .into_iter()
        .filter(|s| s.range.start < r.end && s.range.end > r.start)
        .map(|s| HIGHLIGHT_NAMES[s.highlight])
        .collect()
}

fn parsed(lang: Language, src: &str) -> SyntaxState {
    let mut s = SyntaxState::new(lang);
    s.parse(&src);
    s
}

/// Asserts `needle` is highlighted with a name starting with `prefix`.
#[track_caller]
fn expect(state: &SyntaxState, src: &str, needle: &str, prefix: &str) {
    let got = names_at(state, src, needle);
    assert!(
        got.iter().any(|n| n.starts_with(prefix)),
        "{:?}: {needle:?} expected {prefix}*, got {got:?}",
        state.language()
    );
}

/// (language, sample, [(needle, expected highlight prefix)])
type Case<'a> = (Language, &'a str, &'a [(&'a str, &'a str)]);

#[test]
fn smoke_every_language() {
    let cases: &[Case] = &[
        (
            Language::TypeScript,
            "// note\ninterface Foo { a: string }\nconst x: number = 42;\nlet s = \"hi\";\n",
            &[("// note", "comment"), ("interface", "keyword"), ("\"hi\"", "string"), ("42", "number"), ("number", "type")],
        ),
        (
            Language::Tsx,
            "// c\nconst App = () => <div className=\"a\">hi</div>;\n",
            &[("// c", "comment"), ("const", "keyword"), ("div", "tag"), ("\"a\"", "string")],
        ),
        (
            Language::JavaScript,
            "/* c */\nfunction greet(a) { return `t${a}`; }\nconst el = <b>x</b>;\n",
            &[("/* c */", "comment"), ("function", "keyword"), ("return", "keyword"), ("greet", "function"), ("`t", "string")],
        ),
        (
            Language::Rust,
            "// c\nfn main() { let s = \"x\"; println!(\"{}\", s); }\n",
            &[("// c", "comment"), ("fn", "keyword"), ("\"x\"", "string"), ("main", "function")],
        ),
        (
            Language::Python,
            "# c\ndef greet(x):\n    return 'hi'\n",
            &[("# c", "comment"), ("def", "keyword"), ("'hi'", "string"), ("greet", "function")],
        ),
        (Language::Json, "{\"a\": 1, \"b\": true}\n", &[("\"a\"", "string"), ("1", "number"), ("true", "constant")]),
        (
            Language::Css,
            "/* c */\n.box { color: red; width: 10px; }\n",
            &[("/* c */", "comment"), ("color", "property"), ("10", "number")],
        ),
        (
            Language::Html,
            "<!-- c -->\n<div class=\"a\">x</div>\n<script>const y = 1;</script>\n<style>p { color: red; }</style>\n",
            &[("<!-- c -->", "comment"), ("div", "tag"), ("class", "attribute"), ("const", "keyword"), ("color", "property")],
        ),
        (
            Language::Markdown,
            "# Title\n\nSome *em* and `code` [link](http://x.y)\n\n```rust\nfn main() {}\n```\n",
            &[("Title", "title"), ("*em*", "emphasis"), ("`code`", "text.literal"), ("http://x.y", "link_uri"), ("fn", "keyword")],
        ),
        (
            Language::Toml,
            "# c\n[pkg]\nname = \"qur\"\nn = 3\n",
            &[("# c", "comment"), ("\"qur\"", "string"), ("3", "number"), ("name", "property")],
        ),
        (
            Language::Yaml,
            "# c\nname: qur\nn: 3\nok: true\n",
            &[("# c", "comment"), ("name", "property"), ("3", "number"), ("true", "boolean")],
        ),
        (
            Language::Kotlin,
            "// c\nfun main() {\n    val s = \"hi\"\n    println(s)\n}\n",
            &[("// c", "comment"), ("fun", "keyword"), ("\"hi\"", "string")],
        ),
        (
            Language::Dart,
            "// c\nvoid main() {\n  final s = 'hi';\n  print(s);\n}\n",
            &[("// c", "comment"), ("final", "keyword"), ("'hi'", "string")],
        ),
    ];
    for (lang, src, expects) in cases {
        let s = parsed(*lang, src);
        for (needle, prefix) in *expects {
            expect(&s, src, needle, prefix);
        }
    }
}

#[test]
fn spans_sorted_non_overlapping_and_clipped() {
    let src = std::fs::read_to_string(fixture()).unwrap();
    let s = parsed(Language::TypeScript, &src);
    let r = 1000..5000;
    let spans = s.highlights(&src, r.clone());
    assert!(!spans.is_empty());
    for w in spans.windows(2) {
        assert!(w[0].range.end <= w[1].range.start, "{:?} overlaps {:?}", w[0], w[1]);
    }
    assert!(spans.iter().all(|s| s.range.start >= r.start && s.range.end <= r.end && !s.range.is_empty()));
}

#[test]
fn for_path_and_metadata() {
    let p = |s: &str| Language::for_path(Path::new(s));
    assert_eq!(p("a/b.ts"), Some(Language::TypeScript));
    assert_eq!(p("x.mts"), Some(Language::TypeScript));
    assert_eq!(p("x.tsx"), Some(Language::Tsx));
    assert_eq!(p("x.jsx"), Some(Language::JavaScript));
    assert_eq!(p("Cargo.lock"), Some(Language::Toml));
    assert_eq!(p("tsconfig.json"), Some(Language::Json));
    assert_eq!(p("x.YML"), Some(Language::Yaml));
    assert_eq!(p("build.gradle.kts"), Some(Language::Kotlin));
    assert_eq!(p("x.dart"), Some(Language::Dart));
    assert_eq!(p("README"), None);
    assert_eq!(Language::Python.line_comment(), Some("#"));
    assert_eq!(Language::Html.block_comment(), Some(("<!--", "-->")));
    assert_eq!(Language::Rust.indent_unit(), "    ");
    for l in Language::ALL {
        assert!(!l.name().is_empty());
    }
}

#[test]
fn matching_brackets() {
    let src = "function f(a) { if (a) { return [1, 2]; } }\n";
    let s = parsed(Language::TypeScript, src);
    let open = src.find('{').unwrap();
    let close = src.rfind('}').unwrap();
    assert_eq!(s.matching_bracket(&src, open), Some(close..close + 1));
    assert_eq!(s.matching_bracket(&src, close), Some(open..open + 1));
    let lb = src.find('[').unwrap();
    assert_eq!(s.matching_bracket(&src, lb), Some(src.find(']').unwrap()..src.find(']').unwrap() + 1));
    assert_eq!(s.matching_bracket(&src, 0), None);
    // Fallback scan without a tree.
    let plain = SyntaxState::new(Language::TypeScript);
    assert_eq!(plain.matching_bracket(&src, open), Some(close..close + 1));
    assert_eq!(plain.matching_bracket(&src, close), Some(open..open + 1));
}

#[test]
fn incremental_edit_reports_changed_ranges() {
    let mut src = String::from("const a = 1;\nconst b = 2;\n");
    let mut s = parsed(Language::TypeScript, &src);
    // Turn `1` into a string: `"x"`.
    let at = src.find('1').unwrap();
    src.replace_range(at..at + 1, "\"x\"");
    s.edit(&TextEdit {
        start_byte: at,
        old_end_byte: at + 1,
        new_end_byte: at + 3,
        start_point: (0, at),
        old_end_point: (0, at + 1),
        new_end_point: (0, at + 3),
    });
    let changed = s.reparse(&src);
    assert!(changed.iter().any(|r| r.start <= at && r.end >= at + 3), "{changed:?}");
    expect(&s, &src, "\"x\"", "string");
    expect(&s, &src, "2", "number");
}

#[cfg(feature = "rope")]
#[test]
fn rope_source_matches_str() {
    let src = std::fs::read_to_string(fixture()).unwrap();
    let rope = ropey::Rope::from_str(&src);
    let mut a = SyntaxState::new(Language::TypeScript);
    a.parse(&src);
    let mut b = SyntaxState::new(Language::TypeScript);
    b.parse(&rope);
    let r = 200_000..201_000;
    assert_eq!(a.highlights(&src, r.clone()), b.highlights(&rope, r));
}

fn fixture() -> std::path::PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/fixtures/large.ts");
    assert!(p.exists(), "run `python3 spikes/fixtures/gen.py` first");
    p
}

/// Budgets (release): full parse < 50 ms, 100-line viewport < 2 ms,
/// incremental reparse after a 1-char insert < 5 ms.
/// `cargo test -p qur_syntax --release -- --nocapture bench`
#[cfg(feature = "rope")]
#[test]
fn bench_large_ts() {
    let src = std::fs::read_to_string(fixture()).unwrap();
    let rope = ropey::Rope::from_str(&src);
    let lines = rope.len_lines();

    let t = Instant::now();
    let _ = grammars::config(grammars::Grammar::Lang(Language::TypeScript));
    let compile = t.elapsed();

    let mut s = SyntaxState::new(Language::TypeScript);
    let t = Instant::now();
    s.parse(&rope);
    let parse = t.elapsed();

    let mid = rope.line_to_byte(lines / 2);
    let vp = mid..rope.line_to_byte(lines / 2 + 100);
    let _ = s.highlights(&rope, vp.clone()); // warm
    let t = Instant::now();
    let spans = s.highlights(&rope, vp.clone());
    let viewport = t.elapsed();

    let mut rope2 = rope.clone();
    let char_at = rope2.byte_to_char(mid + 3);
    rope2.insert_char(char_at, 'x');
    let line = rope2.byte_to_line(mid + 3);
    let col = mid + 3 - rope2.line_to_byte(line);
    s.edit(&TextEdit {
        start_byte: mid + 3,
        old_end_byte: mid + 3,
        new_end_byte: mid + 4,
        start_point: (line, col),
        old_end_point: (line, col),
        new_end_point: (line, col + 1),
    });
    let t = Instant::now();
    let changed = s.reparse(&rope2);
    let reparse = t.elapsed();

    let t = Instant::now();
    let all = s.highlights(&rope2, 0..rope2.len_bytes());
    let whole = t.elapsed();

    eprintln!(
        "bench large.ts ({lines} lines): query compile {:.1} ms, full parse {:.1} ms, \
         100-line viewport {:.3} ms ({} spans), 1-char reparse {:.2} ms (changed {:?}), \
         whole-file highlights {:.1} ms ({} spans)",
        compile.as_secs_f64() * 1e3,
        parse.as_secs_f64() * 1e3,
        viewport.as_secs_f64() * 1e3,
        spans.len(),
        reparse.as_secs_f64() * 1e3,
        changed,
        whole.as_secs_f64() * 1e3,
        all.len()
    );
    assert!(changed.iter().all(|r| r.len() < 10_000), "changed ranges too wide: {changed:?}");
    // Budgets hold on a dev Mac; shared CI runners are slower and noisy (58–70 ms
    // full parses seen), so CI only prints the numbers. scripts/budget.sh is the real check.
    if !cfg!(debug_assertions) && std::env::var_os("CI").is_none() {
        assert!(parse.as_millis() < 50, "full parse {parse:?}");
        assert!(viewport.as_micros() < 2000, "viewport {viewport:?}");
        assert!(reparse.as_millis() < 5, "reparse {reparse:?}");
    }
}

/// Query compile cost per grammar (paid once, on first open of that language).
/// `cargo test -p qur_syntax --release -- --nocapture compile_times`
#[test]
fn compile_times() {
    let mut out = Vec::new();
    for g in Language::ALL.map(grammars::Grammar::Lang).into_iter().chain([grammars::Grammar::MarkdownInline]) {
        let t = Instant::now();
        let c = grammars::build(g);
        out.push(format!("{g:?} {:.1} ms ({} patterns)", t.elapsed().as_secs_f64() * 1e3, c.highlights.pattern_count()));
    }
    eprintln!("query compile: {}", out.join(", "));
}

/// (name, kind, depth) of every symbol.
fn outline(lang: Language, src: &str) -> Vec<(String, &'static str, usize)> {
    parsed(lang, src).symbols(&src).into_iter().map(|s| (s.name, s.kind, s.depth)).collect()
}

#[track_caller]
fn has(got: &[(String, &'static str, usize)], name: &str, kind: &str, depth: usize) {
    assert!(
        got.iter().any(|(n, k, d)| n == name && *k == kind && *d == depth),
        "{name:?} ({kind}, depth {depth}) not in {got:?}"
    );
}

#[test]
fn symbols_every_language() {
    let rust = outline(Language::Rust, "mod m { fn f() { fn inner() {} } }\nstruct S;\nimpl<T> Tr for S { fn g(&self) {} }\nimpl S {}\ntrait Tr { fn h(); }\nmacro_rules! mm { () => {} }\nconst C: u8 = 1;\ntype Ty = u8;\nenum En { A }\n");
    has(&rust, "m", "mod", 0);
    has(&rust, "f", "fn", 1);
    has(&rust, "inner", "fn", 2);
    has(&rust, "S", "struct", 0);
    has(&rust, "impl Tr for S", "impl", 0);
    has(&rust, "g", "method", 1);
    has(&rust, "impl S", "impl", 0);
    has(&rust, "h", "method", 1);
    has(&rust, "mm", "macro", 0);
    has(&rust, "C", "const", 0);
    has(&rust, "Ty", "type", 0);
    has(&rust, "En", "enum", 0);

    let py = outline(Language::Python, "class A:\n    def m(self):\n        def local(): pass\n@dec\ndef f(): pass\nX = 1\n");
    assert_eq!(py, vec![("A".into(), "class", 0), ("m".into(), "method", 1), ("local".into(), "def", 2), ("f".into(), "def", 0)]);

    let ts = outline(Language::TypeScript, "interface I { m(): void; p: number }\ntype T = string;\nenum E { A }\nnamespace N { export const k = 1 }\nexport class C { field = 1; constructor() {} m() {} }\nconst arrow = () => 1;\nconst n = 2;\nfunction f() {}\nabstract class A { abstract am(): void }\n");
    for (n, k, d) in [("I", "interface", 0), ("m", "method", 1), ("T", "type", 0), ("E", "enum", 0), ("N", "namespace", 0), ("C", "class", 0), ("constructor", "method", 1), ("arrow", "function", 0), ("f", "function", 0), ("A", "class", 0), ("am", "method", 1)] {
        has(&ts, n, k, d);
    }
    assert!(!ts.iter().any(|s| s.0 == "n"), "plain constants aren't symbols: {ts:?}");
    let tsx = outline(Language::Tsx, "export function App() { return <div/> }\nconst Btn = () => <b/>;\n");
    has(&tsx, "App", "function", 0);
    has(&tsx, "Btn", "function", 0);
    let js = outline(Language::JavaScript, "class C { m() {} }\nfunction f() {}\nconst g = function() {};\nlet h = async () => {};\n");
    for (n, k, d) in [("C", "class", 0), ("m", "method", 1), ("f", "function", 0), ("g", "function", 0), ("h", "function", 0)] {
        has(&js, n, k, d);
    }

    let kt = outline(Language::Kotlin, "package a\nclass Foo(val x: Int) {\n  fun bar(y: Int): Int = y\n  val z = 1\n}\nfun top() { val local = 1 }\ntypealias S = String\n");
    for (n, k, d) in [("Foo", "class", 0), ("bar", "method", 1), ("z", "val", 1), ("top", "fun", 0), ("S", "typealias", 0)] {
        has(&kt, n, k, d);
    }
    assert!(!kt.iter().any(|s| s.0 == "local"), "{kt:?}");

    let dart = outline(Language::Dart, "class Foo extends Bar {\n  Foo(this.x);\n  void bar() {}\n  int get y => 2;\n}\nvoid main() {\n  print(1);\n}\nenum Color { red }\nmixin M {}\nextension E on int {}\n");
    for (n, k, d) in [("Foo", "class", 0), ("Foo", "constructor", 1), ("bar", "method", 1), ("y", "method", 1), ("main", "function", 0), ("Color", "enum", 0), ("M", "mixin", 0), ("E", "extension", 0)] {
        has(&dart, n, k, d);
    }

    let css = outline(Language::Css, "a.b, c { color: red; }\n@media (x) { d { e: f } }\n@keyframes spin { from {} }\n");
    has(&css, "a.b, c", "rule", 0);
    has(&css, "@media (x)", "@media", 0);
    has(&css, "d", "rule", 1);
    has(&css, "spin", "@keyframes", 0);

    let html = outline(Language::Html, "<html><body><div class=\"x\" id=\"main\"><h1>Title</h1><p id=inner>t</p></div></body></html>\n");
    assert_eq!(html, vec![("div#main".into(), "element", 0), ("p#inner".into(), "element", 1)]);

    let md = outline(Language::Markdown, "# Title\n\ntext\n\n## Sub *x*\n\nSetext\n------\n");
    has(&md, "# Title", "heading", 0);
    has(&md, "## Sub *x*", "heading", 1);
    has(&md, "## Setext", "heading", 1);

    let toml = outline(Language::Toml, "a = 1\n[server]\nport = 8\n[[bin]]\nname = \"x\"\n");
    assert_eq!(toml, vec![("a".into(), "key", 0), ("[server]".into(), "table", 0), ("port".into(), "key", 1), ("[[bin]]".into(), "array", 0), ("name".into(), "key", 1)]);

    let yaml = outline(Language::Yaml, "a: 1\nb:\n  c: 2\n");
    assert_eq!(yaml, vec![("a".into(), "key", 0), ("b".into(), "key", 0), ("c".into(), "key", 1)]);

    let json = outline(Language::Json, "{\"a\": {\"b\": 1}, \"c\": [1]}\n");
    assert_eq!(json, vec![("a".into(), "key", 0), ("b".into(), "key", 1), ("c".into(), "key", 0)]);
}

#[test]
fn symbol_ranges_point_at_names() {
    let src = "fn alpha() {}\nstruct Beta;\n";
    let syms = parsed(Language::Rust, src).symbols(&src);
    assert_eq!(&src[syms[0].name_range.clone()], "alpha");
    assert_eq!(&src[syms[0].range.clone()], "fn alpha() {}");
    assert_eq!(&src[syms[1].name_range.clone()], "Beta");
    // No tree yet: nothing, no panic.
    assert!(SyntaxState::new(Language::Rust).symbols(&src).is_empty());
}

/// Outline of the 10k-line fixture: well under a frame's worth of budget.
#[test]
fn symbols_large_ts_fast() {
    let src = std::fs::read_to_string(fixture()).unwrap();
    let s = parsed(Language::TypeScript, &src);
    let t = Instant::now();
    let syms = s.symbols(&src);
    let ms = t.elapsed().as_secs_f64() * 1e3;
    eprintln!("symbols: {} in {ms:.1} ms", syms.len());
    assert!(!syms.is_empty());
    assert!(ms < 50.0, "outline took {ms:.1} ms");
}

#[test]
fn expand_range_climbs_nodes() {
    let src = "fn f() { let x = foo(bar, 1); }\n";
    let s = parsed(Language::Rust, src);
    let at = |n: &str| src.find(n).unwrap();
    let grow = |r: std::ops::Range<usize>| s.expand_range(&src, r).map(|r| &src[r]);
    // Caret inside, at the start of, and just after a word: the word.
    assert_eq!(grow(at("bar") + 1..at("bar") + 1), Some("bar"));
    assert_eq!(grow(at("bar")..at("bar")), Some("bar"));
    assert_eq!(grow(at("bar") + 3..at("bar") + 3), Some("bar"));
    // Then the arguments, the call, the let, the block, the function.
    let r = at("bar")..at("bar") + 3;
    assert_eq!(grow(r), Some("(bar, 1)"));
    let r = at("(bar")..at("(bar") + 8;
    assert_eq!(grow(r), Some("foo(bar, 1)"));
    let r = at("foo")..at("foo") + 11;
    assert_eq!(grow(r), Some("let x = foo(bar, 1);"));
    let r = at("let")..at("1);") + 3;
    assert_eq!(grow(r), Some("{ let x = foo(bar, 1); }"));
    let r = at("{")..src.len() - 1;
    assert_eq!(grow(r), Some("fn f() { let x = foo(bar, 1); }"));
    // A partial selection grows to the node that covers it.
    assert_eq!(grow(at("oo(")..at("ar,")), Some("foo(bar, 1)"));
    // The whole file: nothing bigger.
    assert_eq!(grow(0..src.len()), None);
    assert_eq!(SyntaxState::new(Language::Rust).expand_range(&src, 0..1), None);
}
