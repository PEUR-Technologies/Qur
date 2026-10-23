//! Shared pieces for the M0 toolkit spike, so every candidate toolkit does the
//! same work and reports metrics the same way. Only the UI differs.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Process clock
// ---------------------------------------------------------------------------

static START: OnceLock<Instant> = OnceLock::new();

/// Call first thing in `main`.
pub fn mark_start() {
    START.get_or_init(Instant::now);
}

pub fn since_start() -> Duration {
    START.get_or_init(Instant::now).elapsed()
}

/// Every measurement line starts with `METRIC ` so `scripts/spike_measure.sh` can grep it.
pub fn metric(name: &str, value: impl std::fmt::Display) {
    println!("METRIC {name}={value}");
}

// ---------------------------------------------------------------------------
// Env-driven spike modes
// ---------------------------------------------------------------------------

pub struct Config {
    pub tree_root: PathBuf,
    pub file: PathBuf,
    /// Exit right after the first frame is presented.
    pub exit_after_first_frame: bool,
    /// Scroll the editor top to bottom, record frame times, print, exit.
    pub autoscroll: bool,
    /// Insert synthetic keystrokes, record key-to-frame latency, print, exit.
    pub autotype: bool,
    /// Expand every folder in the tree (stress test).
    pub expand_all: bool,
}

impl Config {
    pub fn from_env() -> Self {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures");
        let flag = |k: &str| std::env::var(k).map(|v| v == "1").unwrap_or(false);
        Config {
            tree_root: std::env::var_os("QUR_SPIKE_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| fixtures.join("big-repo")),
            file: std::env::var_os("QUR_SPIKE_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| fixtures.join("large.ts")),
            exit_after_first_frame: flag("QUR_SPIKE_EXIT_AFTER_FIRST_FRAME"),
            autoscroll: flag("QUR_SPIKE_AUTOSCROLL"),
            autotype: flag("QUR_SPIKE_AUTOTYPE"),
            expand_all: flag("QUR_SPIKE_EXPAND_ALL"),
        }
    }
}

/// Lines scrolled per frame in autoscroll mode (10k lines -> ~500 frames).
pub const AUTOSCROLL_LINES_PER_FRAME: usize = 20;
/// Keystrokes injected in autotype mode.
pub const AUTOTYPE_KEYS: usize = 200;
/// Line where autotype inserts text.
pub const AUTOTYPE_LINE: usize = 5000;

// ---------------------------------------------------------------------------
// Frame / latency recorder
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Samples {
    ms: Vec<f64>,
    last: Option<Instant>,
}

impl Samples {
    /// Record the interval since the previous `tick` (frame pacing).
    pub fn tick(&mut self) {
        let now = Instant::now();
        if let Some(prev) = self.last.replace(now) {
            self.ms.push((now - prev).as_secs_f64() * 1000.0);
        }
    }

    pub fn push(&mut self, d: Duration) {
        self.ms.push(d.as_secs_f64() * 1000.0);
    }

    pub fn len(&self) -> usize {
        self.ms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ms.is_empty()
    }

    /// Prints p50/p95/p99/max and counts over 8.33 ms (120 Hz) and 16.7 ms (60 Hz).
    pub fn report(&self, prefix: &str) {
        if self.ms.is_empty() {
            metric(&format!("{prefix}_n"), 0);
            return;
        }
        let mut v = self.ms.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pct = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
        metric(&format!("{prefix}_n"), v.len());
        metric(&format!("{prefix}_p50_ms"), format!("{:.2}", pct(0.50)));
        metric(&format!("{prefix}_p95_ms"), format!("{:.2}", pct(0.95)));
        metric(&format!("{prefix}_p99_ms"), format!("{:.2}", pct(0.99)));
        metric(&format!("{prefix}_max_ms"), format!("{:.2}", v[v.len() - 1]));
        metric(&format!("{prefix}_over_8ms"), v.iter().filter(|&&x| x > 8.34).count());
        metric(&format!("{prefix}_over_16ms"), v.iter().filter(|&&x| x > 16.7).count());
    }
}

// ---------------------------------------------------------------------------
// Syntax highlighting (tree-sitter, TypeScript only for the spike)
// ---------------------------------------------------------------------------

/// Highlight names we ask tree-sitter for. Index into this == `Span::kind`.
pub const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute", "comment", "constant", "constant.builtin", "constructor", "embedded",
    "function", "function.builtin", "function.method", "keyword", "number", "operator",
    "property", "punctuation", "punctuation.bracket", "punctuation.delimiter",
    "punctuation.special", "string", "string.special", "tag", "type", "type.builtin",
    "variable", "variable.builtin", "variable.parameter",
];

#[derive(Clone, Debug)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: usize,
}

pub struct Highlighter {
    config: tree_sitter_highlight::HighlightConfiguration,
    hl: tree_sitter_highlight::Highlighter,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter {
    pub fn new() -> Self {
        let lang = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        // TypeScript's highlight query extends the JavaScript one.
        let query = format!(
            "{}\n{}",
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
            JS_HIGHLIGHTS
        );
        let mut config = tree_sitter_highlight::HighlightConfiguration::new(
            lang,
            "typescript",
            &query,
            "",
            tree_sitter_typescript::LOCALS_QUERY,
        )
        .expect("typescript highlight query");
        config.configure(HIGHLIGHT_NAMES);
        Highlighter { config, hl: tree_sitter_highlight::Highlighter::new() }
    }

    /// Flat, non-overlapping spans covering highlighted byte ranges of `src`.
    pub fn highlight(&mut self, src: &str) -> Vec<Span> {
        use tree_sitter_highlight::HighlightEvent::*;
        let mut out = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let events = self
            .hl
            .highlight(&self.config, src.as_bytes(), None, None, |_| None)
            .expect("highlight");
        for ev in events {
            match ev.expect("highlight event") {
                HighlightStart(h) => stack.push(h.0),
                HighlightEnd => {
                    stack.pop();
                }
                Source { start, end } => {
                    if let Some(&kind) = stack.last() {
                        out.push(Span { range: start..end, kind });
                    }
                }
            }
        }
        out
    }
}

/// tree-sitter-typescript's query assumes the JavaScript query is loaded too;
/// the JS grammar crate is only needed for this string, so it is vendored here.
const JS_HIGHLIGHTS: &str = include_str!("js_highlights.scm");

// ---------------------------------------------------------------------------
// Qur Dark (spike palette; real themes come in M1)
// ---------------------------------------------------------------------------

pub mod qur_dark {
    pub const BG: u32 = 0x000000;
    pub const SIDEBAR: u32 = 0x0A0A0A;
    pub const BORDER: u32 = 0x1F1F1F;
    pub const TEXT: u32 = 0xFFFFFF;
    pub const TEXT_MUTED: u32 = 0x8A8A8A;
    pub const CURRENT_LINE: u32 = 0x111111;
    pub const SELECTION: u32 = 0x2A2A2A;
    pub const ACCENTS: [u32; 3] = [0xFF7A45, 0x3DDC97, 0x5B9BFF];

    /// Color for a `HIGHLIGHT_NAMES` index.
    pub fn syntax(kind: usize) -> u32 {
        match super::HIGHLIGHT_NAMES[kind] {
            "comment" => 0x6A6A6A,
            "keyword" => 0xC792EA,
            "string" | "string.special" => 0xA5D6A7,
            "number" | "constant" | "constant.builtin" => 0xF78C6C,
            "function" | "function.method" | "function.builtin" => 0x82AAFF,
            "type" | "type.builtin" | "constructor" => 0xFFCB6B,
            "property" => 0xD6D6D6,
            "punctuation" | "punctuation.bracket" | "punctuation.delimiter"
            | "punctuation.special" | "operator" => 0x9A9A9A,
            _ => TEXT,
        }
    }
}

// ---------------------------------------------------------------------------
// File tree model (flattened, only visible rows are materialized)
// ---------------------------------------------------------------------------

pub struct Node {
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub children: Vec<usize>,
}

pub struct FileTree {
    pub nodes: Vec<Node>,
    pub root_children: Vec<usize>,
}

impl FileTree {
    pub fn load(root: &Path) -> Self {
        let mut t = FileTree { nodes: Vec::new(), root_children: Vec::new() };
        t.root_children = t.walk(root, 0);
        t
    }

    fn walk(&mut self, dir: &Path, depth: usize) -> Vec<usize> {
        let mut entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).filter(|e| e.file_name() != ".git").collect(),
            Err(_) => return Vec::new(),
        };
        // Folders first, then by name (VS Code / Zed order).
        entries.sort_by_key(|e| {
            (!e.file_type().map(|t| t.is_dir()).unwrap_or(false), e.file_name())
        });
        let mut ids = Vec::with_capacity(entries.len());
        for e in entries {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let id = self.nodes.len();
            self.nodes.push(Node {
                name: e.file_name().to_string_lossy().into_owned(),
                depth,
                is_dir,
                expanded: false,
                children: Vec::new(),
            });
            if is_dir {
                let kids = self.walk(&e.path(), depth + 1);
                self.nodes[id].children = kids;
            }
            ids.push(id);
        }
        ids
    }

    pub fn file_count(&self) -> usize {
        self.nodes.iter().filter(|n| !n.is_dir).count()
    }

    pub fn set_all_expanded(&mut self, on: bool) {
        for n in &mut self.nodes {
            if n.is_dir {
                n.expanded = on;
            }
        }
    }

    pub fn toggle(&mut self, id: usize) {
        let n = &mut self.nodes[id];
        if n.is_dir {
            n.expanded = !n.expanded;
        }
    }

    /// Node ids of the rows currently visible, in display order.
    pub fn visible(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack: Vec<usize> = self.root_children.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            out.push(id);
            let n = &self.nodes[id];
            if n.is_dir && n.expanded {
                stack.extend(n.children.iter().rev().copied());
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Fake agent rows for the sidebar
// ---------------------------------------------------------------------------

pub struct AgentRow {
    pub name: &'static str,
    pub task: &'static str,
    pub status: &'static str,
    pub accent: u32,
}

pub fn sample_agents() -> [AgentRow; 3] {
    [
        AgentRow { name: "claude-1", task: "fix navbar", status: "~", accent: qur_dark::ACCENTS[0] },
        AgentRow { name: "codex-1", task: "add tests", status: "ok", accent: qur_dark::ACCENTS[1] },
        AgentRow { name: "claude-2", task: "needs approval", status: "!", accent: qur_dark::ACCENTS[2] },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_large_fixture_quickly() {
        let src = std::fs::read_to_string(Config::from_env().file).unwrap();
        let t = Instant::now();
        let spans = Highlighter::new().highlight(&src);
        let ms = t.elapsed().as_millis();
        assert!(spans.len() > 10_000, "spans: {}", spans.len());
        assert!(spans.iter().any(|s| HIGHLIGHT_NAMES[s.kind] == "keyword"));
        assert!(spans.iter().any(|s| HIGHLIGHT_NAMES[s.kind] == "type"));
        eprintln!("highlight 10k lines: {ms} ms, {} spans", spans.len());
    }

    #[test]
    fn tree_loads_5000_files() {
        let mut t = FileTree::load(&Config::from_env().tree_root);
        assert_eq!(t.file_count(), 5000);
        t.set_all_expanded(true);
        assert!(t.visible().len() > 5000);
    }
}
