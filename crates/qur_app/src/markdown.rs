//! Markdown for the agent chat: `parse` turns a reply into a small block
//! list (pure, tested), `render` turns that into GPUI elements. Replies
//! stream in chunk by chunk, so half-written Markdown must parse (pulldown-cmark
//! closes whatever is open at the end) and re-rendering must stay cheap: the
//! parse is cached per chat item and code highlights per code block.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    App, ClipboardItem, FontStyle, FontWeight, HighlightStyle, InteractiveText, SharedString, StrikethroughStyle,
    StyledText, UnderlineStyle, Window, div, prelude::*, px,
};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use qur_syntax::{HIGHLIGHT_NAMES, Language, SyntaxState};

use crate::look::Look;

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Para(Inline),
    Heading(u8, Inline),
    /// `spans`: syntax highlights (byte range, index into `HIGHLIGHT_NAMES`),
    /// filled in by `cached`, not by `parse`.
    Code { lang: String, text: String, spans: Rc<Vec<(Range<usize>, usize)>> },
    /// `start`: the first number of an ordered list.
    List { start: Option<u64>, items: Vec<Vec<Block>> },
    Quote(Vec<Block>),
    Rule,
    Table { head: Vec<Inline>, rows: Vec<Vec<Inline>> },
}

/// One paragraph's text with its styled runs. Runs cover the whole text, in
/// order; adjacent text with the same marks shares a run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    pub text: String,
    pub runs: Vec<(Range<usize>, Marks)>,
    /// What each link run opens (`Marks::link` indexes this); None: nothing.
    pub links: Vec<Option<Target>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<u16>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Url(String),
    /// A worktree file, opened in the code modal.
    File(PathBuf, Option<u32>),
}

impl Inline {
    fn push(&mut self, s: &str, m: Marks) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(s);
        match self.runs.last_mut() {
            Some((r, last)) if *last == m && r.end == start => r.end = self.text.len(),
            _ => self.runs.push((start..self.text.len(), m)),
        }
    }
}

enum Frame {
    /// Root, list item or block quote.
    Blocks(Vec<Block>),
    List(Option<u64>, Vec<Vec<Block>>),
    /// Head, body rows, the row being read.
    Table(Vec<Inline>, Vec<Vec<Inline>>, Vec<Inline>),
}

struct Builder<'a> {
    root: Option<&'a Path>,
    stack: Vec<Frame>,
    inline: Option<Inline>,
    code: Option<(String, String)>,
    heading: Option<u8>,
    bold: u32,
    italic: u32,
    strike: u32,
    links: Vec<Option<u16>>,
}

impl Builder<'_> {
    fn marks(&self) -> Marks {
        Marks { bold: self.bold > 0, italic: self.italic > 0, strike: self.strike > 0, code: false, link: self.links.last().copied().flatten() }
    }

    fn text(&mut self, s: &str, m: Marks) {
        self.inline.get_or_insert_with(Inline::default).push(s, m);
    }

    fn push_block(&mut self, b: Block) {
        match self.stack.last_mut() {
            Some(Frame::Blocks(v)) => v.push(b),
            Some(Frame::List(_, items)) => match items.last_mut() {
                Some(item) => item.push(b),
                None => items.push(vec![b]),
            },
            Some(Frame::Table(..)) | None => {}
        }
    }

    /// Close loose text (tight list items have no paragraph events).
    fn flush(&mut self) {
        if let Some(i) = self.inline.take().filter(|i| !i.text.trim().is_empty()) {
            self.push_block(Block::Para(i));
        }
    }

    fn pop_blocks(&mut self) -> Vec<Block> {
        self.flush();
        if self.stack.len() > 1
            && let Some(Frame::Blocks(v)) = self.stack.pop()
        {
            return v;
        }
        Vec::new()
    }

    fn event(&mut self, e: Event) {
        match e {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => match &mut self.code {
                Some((_, code)) => code.push_str(&t),
                None => self.text(&t, self.marks()),
            },
            Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                let link = self.marks().link.or_else(|| {
                    let target = file_target(&t, self.root?)?;
                    let i = self.inline.get_or_insert_with(Inline::default);
                    i.links.push(Some(target));
                    Some((i.links.len() - 1) as u16)
                });
                self.text(&t, Marks { code: true, link, ..self.marks() });
            }
            Event::Html(t) | Event::InlineHtml(t) => self.text(t.trim_end_matches('\n'), self.marks()),
            Event::FootnoteReference(t) => self.text(&format!("[{t}]"), self.marks()),
            Event::SoftBreak => self.text(" ", self.marks()),
            Event::HardBreak => self.text("\n", self.marks()),
            Event::Rule => {
                self.flush();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => self.text(if done { "☑ " } else { "☐ " }, self.marks()),
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.flush(),
            Tag::Heading { level, .. } => {
                self.flush();
                self.heading = Some(level as u8);
            }
            Tag::BlockQuote(_) | Tag::Item => {
                self.flush();
                self.stack.push(Frame::Blocks(Vec::new()));
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.split([' ', ',', '{']).next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                self.flush();
                self.stack.push(Frame::List(start, Vec::new()));
            }
            Tag::Table(_) => {
                self.flush();
                self.stack.push(Frame::Table(Vec::new(), Vec::new(), Vec::new()));
            }
            Tag::TableCell => self.inline = Some(Inline::default()),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                let target = resolve(&dest_url, self.root);
                let i = self.inline.get_or_insert_with(Inline::default);
                i.links.push(target);
                self.links.push(Some((i.links.len() - 1) as u16));
            }
            _ => self.flush(),
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                let level = self.heading.take().unwrap_or(1);
                if let Some(i) = self.inline.take().filter(|i| !i.text.trim().is_empty()) {
                    self.push_block(Block::Heading(level, i));
                }
            }
            TagEnd::BlockQuote(_) => {
                let v = self.pop_blocks();
                self.push_block(Block::Quote(v));
            }
            TagEnd::Item => {
                let v = self.pop_blocks();
                if let Some(Frame::List(_, items)) = self.stack.last_mut() {
                    items.push(v);
                }
            }
            TagEnd::CodeBlock => self.end_code(),
            TagEnd::List(_) => {
                self.flush();
                if matches!(self.stack.last(), Some(Frame::List(..)))
                    && let Some(Frame::List(start, items)) = self.stack.pop()
                {
                    self.push_block(Block::List { start, items });
                }
            }
            TagEnd::TableCell => {
                let cell = self.inline.take().unwrap_or_default();
                if let Some(Frame::Table(_, _, row)) = self.stack.last_mut() {
                    row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(Frame::Table(head, _, row)) = self.stack.last_mut() {
                    *head = std::mem::take(row);
                }
            }
            TagEnd::TableRow => {
                if let Some(Frame::Table(_, rows, row)) = self.stack.last_mut() {
                    rows.push(std::mem::take(row));
                }
            }
            TagEnd::Table => {
                if matches!(self.stack.last(), Some(Frame::Table(..)))
                    && let Some(Frame::Table(head, rows, _)) = self.stack.pop()
                {
                    self.push_block(Block::Table { head, rows });
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => {
                self.links.pop();
            }
            _ => self.flush(),
        }
    }

    fn end_code(&mut self) {
        if let Some((lang, mut text)) = self.code.take() {
            if text.ends_with('\n') {
                text.pop();
            }
            self.push_block(Block::Code { lang, text, spans: Rc::default() });
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.end_code();
        self.flush();
        while self.stack.len() > 1 {
            match self.stack.pop() {
                Some(Frame::Blocks(v)) => self.push_block(Block::Quote(v)),
                Some(Frame::List(start, items)) => self.push_block(Block::List { start, items }),
                Some(Frame::Table(head, rows, _)) => self.push_block(Block::Table { head, rows }),
                None => {}
            }
        }
        match self.stack.pop() {
            Some(Frame::Blocks(v)) => v,
            _ => Vec::new(),
        }
    }
}

/// Markdown to blocks. `root`: the worktree, for making `path:line` links
/// and inline code naming an existing file clickable.
pub fn parse(text: &str, root: Option<&Path>) -> Vec<Block> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut b = Builder { root, stack: vec![Frame::Blocks(Vec::new())], inline: None, code: None, heading: None, bold: 0, italic: 0, strike: 0, links: Vec::new() };
    for e in Parser::new_ext(text, opts) {
        b.event(e);
    }
    b.finish()
}

fn resolve(dest: &str, root: Option<&Path>) -> Option<Target> {
    let lower = dest.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
        return Some(Target::Url(dest.to_string()));
    }
    file_target(dest, root?)
}

/// `src/a.rs`, `src/a.rs:12`, `src/a.rs:12:5`, `src/a.rs#L12` or an absolute
/// path, when it names a file inside `root`.
pub fn file_target(s: &str, root: &Path) -> Option<Target> {
    let s = s.trim().strip_prefix("file://").unwrap_or(s.trim());
    if s.is_empty() || s.len() > 400 || s.contains(char::is_whitespace) {
        return None;
    }
    let (path, line) = if let Some((p, l)) = s.split_once("#L") {
        (p, l.split(['-', ':']).next().and_then(|l| l.parse().ok()))
    } else if let Some((p, rest)) = s.split_once(':') {
        match rest.split(':').next().and_then(|l| l.parse::<u32>().ok()) {
            Some(l) => (p, Some(l)),
            None => (s, None),
        }
    } else {
        (s, None)
    };
    let path = Path::new(path.strip_prefix("./").unwrap_or(path));
    let full = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
    (full.starts_with(root) && full.is_file()).then_some(Target::File(full, line))
}

fn fence_language(lang: &str) -> Option<Language> {
    let ext = match lang.to_ascii_lowercase().as_str() {
        "rust" => "rs".to_string(),
        "typescript" => "ts".into(),
        "javascript" | "node" => "js".into(),
        "python" | "python3" => "py".into(),
        "kotlin" => "kt".into(),
        "markdown" => "md".into(),
        l => l.to_string(),
    };
    if ext.is_empty() {
        return None;
    }
    Language::for_path(Path::new(&format!("x.{ext}")))
}

fn hash_of(a: impl Hash, b: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (a, b).hash(&mut h);
    h.finish()
}

/// Past this many entries a cache is just cleared (it refills from what's on screen).
const CACHE_MAX: usize = 512;

/// (hash of text and root, blocks).
type Parsed = (u64, Rc<Vec<Block>>);
/// Byte ranges and their highlight ids.
type Highlights = Rc<Vec<(Range<usize>, usize)>>;

thread_local! {
    /// Parsed blocks per caller id.
    static PARSED: RefCell<HashMap<SharedString, Parsed>> = RefCell::default();
    /// Syntax highlights per (language, code) hash: a streaming reply only
    /// re-highlights the block being written.
    static HIGHLIGHTS: RefCell<HashMap<u64, Highlights>> = RefCell::default();
}

fn cached(id: &SharedString, text: &str, root: Option<&Path>) -> Rc<Vec<Block>> {
    let key = hash_of(text, root);
    if let Some(b) = PARSED.with_borrow(|m| m.get(id).filter(|(k, _)| *k == key).map(|(_, b)| b.clone())) {
        return b;
    }
    let mut blocks = parse(text, root);
    highlight_all(&mut blocks);
    let blocks = Rc::new(blocks);
    PARSED.with_borrow_mut(|m| {
        if m.len() >= CACHE_MAX {
            m.clear();
        }
        m.insert(id.clone(), (key, blocks.clone()));
    });
    blocks
}

fn highlight_all(blocks: &mut [Block]) {
    for b in blocks {
        match b {
            Block::Code { lang, text, spans } => *spans = highlights(lang, text),
            Block::List { items, .. } => items.iter_mut().for_each(|i| highlight_all(i)),
            Block::Quote(v) => highlight_all(v),
            _ => {}
        }
    }
}

fn highlights(lang: &str, text: &str) -> Rc<Vec<(Range<usize>, usize)>> {
    let Some(language) = fence_language(lang) else { return Rc::default() };
    let key = hash_of(lang, text);
    if let Some(s) = HIGHLIGHTS.with_borrow(|m| m.get(&key).cloned()) {
        return s;
    }
    let mut state = SyntaxState::new(language);
    state.parse(&text);
    // Sorted and non-overlapping, as StyledText wants.
    let mut out: Vec<(Range<usize>, usize)> = Vec::new();
    for s in state.highlights(&text, 0..text.len()) {
        let start = s.range.start.max(out.last().map_or(0, |(r, _)| r.end));
        let end = s.range.end.min(text.len());
        if start < end && text.is_char_boundary(start) && text.is_char_boundary(end) && s.highlight < HIGHLIGHT_NAMES.len() {
            out.push((start..end, s.highlight));
        }
    }
    let out = Rc::new(out);
    HIGHLIGHTS.with_borrow_mut(|m| {
        if m.len() >= CACHE_MAX {
            m.clear();
        }
        m.insert(key, out.clone());
    });
    out
}

// -- rendering -----------------------------------------------------------------

/// Called when a file link is clicked: open it in the code modal.
pub type OnFile = Rc<dyn Fn(PathBuf, Option<u32>, &mut Window, &mut App)>;

/// Render `text` as Markdown. `id` must be unique and stable per chat item
/// (element ids and the parse cache hang off it).
pub fn render(text: &str, id: impl Into<SharedString>, root: Option<&Path>, look: &Look, on_file: OnFile) -> gpui::Div {
    let id = id.into();
    let blocks = cached(&id, text, root);
    let r = Renderer { look, on_file, id, n: std::cell::Cell::new(0) };
    r.blocks(&blocks, 0).text_size(look.text(0.)).text_color(look.c(look.theme.ui.text))
}

/// A small "Copy" chip that puts `text` on the clipboard.
pub fn copy_button(id: impl Into<gpui::ElementId>, text: String, look: &Look) -> gpui::Stateful<gpui::Div> {
    let ui = &look.theme.ui;
    div()
        .id(id)
        .flex_none()
        .px(look.z(6.))
        .rounded(look.z(4.))
        .cursor_pointer()
        .text_color(look.c(ui.text_muted))
        .hover(|d| d.bg(look.c(ui.hover_bg)).text_color(look.c(ui.text)))
        .active(|d| d.bg(look.c(ui.active_bg)))
        .child("Copy")
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
}

struct Renderer<'a> {
    look: &'a Look,
    on_file: OnFile,
    id: SharedString,
    n: std::cell::Cell<usize>,
}

impl Renderer<'_> {
    fn next_id(&self) -> gpui::ElementId {
        let n = self.n.get();
        self.n.set(n + 1);
        gpui::ElementId::Name(format!("{}-{n}", self.id).into())
    }

    fn blocks(&self, blocks: &[Block], depth: usize) -> gpui::Div {
        div().flex().flex_col().gap(self.look.z(6.)).min_w(px(0.)).children(blocks.iter().map(|b| self.block(b, depth)))
    }

    fn block(&self, b: &Block, depth: usize) -> gpui::AnyElement {
        let look = self.look;
        let ui = &look.theme.ui;
        match b {
            Block::Para(i) => div().child(self.inline(i)).into_any_element(),
            Block::Heading(level, i) => {
                let delta = match level {
                    1 => 6.,
                    2 => 4.,
                    3 => 2.,
                    _ => 1.,
                };
                div().pt(look.z(4.)).text_size(look.text(delta)).font_weight(FontWeight::SEMIBOLD).child(self.inline(i)).into_any_element()
            }
            Block::Code { lang, text, spans } => {
                let styles: Vec<HighlightStyle> = HIGHLIGHT_NAMES
                    .iter()
                    .map(|n| {
                        let s = look.theme.syntax_style(n);
                        HighlightStyle {
                            color: s.color.map(|c| look.c(c)),
                            font_weight: s.bold.then_some(FontWeight::BOLD),
                            font_style: s.italic.then_some(FontStyle::Italic),
                            ..Default::default()
                        }
                    })
                    .collect();
                let hl: Vec<(Range<usize>, HighlightStyle)> = spans.iter().map(|(r, h)| (r.clone(), styles[*h])).collect();
                div()
                    .flex()
                    .flex_col()
                    .rounded(look.z(4.))
                    .bg(look.c(ui.panel_bg))
                    .border_1()
                    .border_color(look.c(ui.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(look.z(10.))
                            .pt(look.z(4.))
                            .text_color(look.c(ui.text_muted))
                            .child(lang.clone())
                            .child(copy_button(self.next_id(), text.clone(), look)),
                    )
                    .child(
                        div()
                            .px(look.z(10.))
                            .pb(look.z(6.))
                            .font_family(look.code_font.clone())
                            .text_color(look.c(ui.text_secondary))
                            .when(!text.is_empty(), |d| d.child(StyledText::new(text.clone()).with_highlights(hl))),
                    )
                    .into_any_element()
            }
            Block::List { start, items } => div()
                .flex()
                .flex_col()
                .gap(look.z(4.))
                .children(items.iter().enumerate().map(|(n, item)| {
                    let marker = match start {
                        Some(s) => format!("{}.", s + n as u64),
                        None => if depth.is_multiple_of(2) { "•" } else { "◦" }.to_string(),
                    };
                    div()
                        .flex()
                        .gap(look.z(6.))
                        .child(div().flex_none().min_w(look.z(14.)).text_color(look.c(ui.text_muted)).child(marker))
                        .child(self.blocks(item, depth + 1).flex_1().gap(look.z(4.)))
                }))
                .into_any_element(),
            Block::Quote(v) => self.blocks(v, depth).pl(look.z(10.)).border_l_2().border_color(look.c(ui.border_strong)).text_color(look.c(ui.text_secondary)).into_any_element(),
            Block::Rule => div().my(look.z(4.)).h(px(1.)).bg(look.c(ui.border)).into_any_element(),
            Block::Table { head, rows } => {
                let cols = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
                let row = |cells: &[Inline], header: bool, last: bool| {
                    div()
                        .flex()
                        .when(!last, |d| d.border_b_1().border_color(look.c(ui.border)))
                        .when(header, |d| d.bg(look.c(ui.panel_bg)).font_weight(FontWeight::SEMIBOLD))
                        .children((0..cols).map(|c| {
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .px(look.z(8.))
                                .py(look.z(4.))
                                .when(c + 1 < cols, |d| d.border_r_1().border_color(look.c(ui.border)))
                                .when_some(cells.get(c), |d, i| d.child(self.inline(i)))
                        }))
                };
                div()
                    .flex()
                    .flex_col()
                    .rounded(look.z(4.))
                    .border_1()
                    .border_color(look.c(ui.border))
                    .overflow_hidden()
                    .child(row(head, true, rows.is_empty()))
                    .children(rows.iter().enumerate().map(|(n, r)| row(r, false, n + 1 == rows.len())))
                    .into_any_element()
            }
        }
    }

    fn inline(&self, i: &Inline) -> gpui::AnyElement {
        let look = self.look;
        let ui = &look.theme.ui;
        let link_color = look.c(ui.focus_ring);
        let hl: Vec<(Range<usize>, HighlightStyle)> = i
            .runs
            .iter()
            .filter(|(_, m)| *m != Marks::default())
            .map(|(r, m)| {
                let link = m.link.is_some_and(|l| matches!(i.links.get(l as usize), Some(Some(_))));
                let style = HighlightStyle {
                    color: link.then_some(link_color),
                    font_weight: m.bold.then_some(FontWeight::BOLD),
                    font_style: m.italic.then_some(FontStyle::Italic),
                    background_color: m.code.then(|| look.c(ui.hover_bg)),
                    underline: (link && !m.code).then_some(UnderlineStyle { thickness: px(1.), color: None, wavy: false }),
                    strikethrough: m.strike.then_some(StrikethroughStyle { thickness: px(1.), color: None }),
                    fade_out: None,
                };
                (r.clone(), style)
            })
            .collect();
        let code: Vec<(Range<usize>, SharedString)> = i.runs.iter().filter(|(_, m)| m.code).map(|(r, _)| (r.clone(), look.code_font.clone())).collect();
        let text = StyledText::new(i.text.clone()).with_highlights(hl).with_font_family_overrides(code);
        let (ranges, targets): (Vec<Range<usize>>, Vec<Target>) = i
            .runs
            .iter()
            .filter_map(|(r, m)| Some((r.clone(), i.links.get(m.link? as usize)?.clone()?)))
            .unzip();
        if ranges.is_empty() {
            return text.into_any_element();
        }
        let on_file = self.on_file.clone();
        InteractiveText::new(self.next_id(), text)
            .on_click(ranges, move |ix, window, cx| match targets.get(ix) {
                Some(Target::Url(u)) => cx.open_url(u),
                Some(Target::File(p, line)) => on_file(p.clone(), *line, window, cx),
                None => {}
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(b: &Block) -> &Inline {
        match b {
            Block::Para(i) | Block::Heading(_, i) => i,
            other => panic!("not a paragraph: {other:?}"),
        }
    }

    #[test]
    fn inline_marks() {
        let b = parse("plain **bold** *it* ~~gone~~ `code` [x](https://a.b)", None);
        assert_eq!(b.len(), 1);
        let i = para(&b[0]);
        assert_eq!(i.text, "plain bold it gone code x");
        let mark = |s: &str| i.runs.iter().find(|(r, _)| &i.text[r.clone()] == s).map(|(_, m)| *m).unwrap_or_default();
        assert!(mark("bold").bold);
        assert!(mark("it").italic);
        assert!(mark("gone").strike);
        assert!(mark("code").code);
        let l = mark("x").link.map(|l| l as usize);
        assert_eq!(l.and_then(|l| i.links[l].clone()), Some(Target::Url("https://a.b".into())));
        // Runs cover the whole text, in order.
        assert_eq!(i.runs.iter().map(|(r, _)| r.len()).sum::<usize>(), i.text.len());
    }

    #[test]
    fn blocks() {
        let md = "# Title\n\npara\n\n- a\n- b\n  1. c\n  2. d\n\n> quote\n\n---\n\n```rust\nfn x() {}\n```\n";
        let b = parse(md, None);
        assert!(matches!(&b[0], Block::Heading(1, i) if i.text == "Title"));
        assert_eq!(para(&b[1]).text, "para");
        let Block::List { start: None, items } = &b[2] else { panic!("{:?}", b[2]) };
        assert_eq!(items.len(), 2);
        assert_eq!(para(&items[0][0]).text, "a");
        assert!(matches!(&items[1][1], Block::List { start: Some(1), items } if items.len() == 2));
        assert!(matches!(&b[3], Block::Quote(v) if para(&v[0]).text == "quote"));
        assert_eq!(b[4], Block::Rule);
        assert!(matches!(&b[5], Block::Code { lang, text, .. } if lang == "rust" && text == "fn x() {}"));
    }

    #[test]
    fn table() {
        let b = parse("| a | b |\n|---|---|\n| 1 | **2** |\n| 3 |\n", None);
        let Block::Table { head, rows } = &b[0] else { panic!("{:?}", b) };
        assert_eq!(head.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][1].text, "2");
    }

    #[test]
    fn half_written() {
        // Every prefix of a reply parses (streaming), whatever is left open.
        let md = "Intro **bo\n\n```ts\nconst a = 1;\n| x | y |\n|--|--|\n- [ ] task\n> q `co";
        for end in (0..=md.len()).filter(|e| md.is_char_boundary(*e)) {
            parse(&md[..end], None);
        }
        let b = parse("text\n\n```py\nprint(1)\n", None);
        assert!(matches!(&b[1], Block::Code { lang, text, .. } if lang == "py" && text == "print(1)"));
    }

    #[test]
    fn file_links() {
        let root = std::env::temp_dir().join(format!("qur-md-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).ok();
        std::fs::write(root.join("src/a.rs"), "x").ok();
        let b = parse("see `src/a.rs:12` and `src/nope.rs` and [a](./src/a.rs#L3)", Some(&root));
        let i = para(&b[0]);
        assert_eq!(i.links, vec![Some(Target::File(root.join("src/a.rs"), Some(12))), Some(Target::File(root.join("src/a.rs"), Some(3)))]);
        assert!(i.runs.iter().any(|(r, m)| &i.text[r.clone()] == "src/nope.rs" && m.link.is_none()));
        assert_eq!(file_target("/etc/hosts", &root), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn highlights_rust() {
        let s = highlights("rust", "fn main() { let x = 1; }");
        assert!(!s.is_empty());
        assert!(s.windows(2).all(|w| w[0].0.end <= w[1].0.start));
        assert!(highlights("nolang", "x").is_empty());
    }
}
