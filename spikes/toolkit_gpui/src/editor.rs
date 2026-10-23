//! Spike editor: lines + per-line highlight spans, one selection, undo/redo,
//! soft wrap in visual rows, find/replace, and the S-tier keymap.

use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ScrollEvent, ShapedLine, SharedString,
    Style, Subscription, TextRun, UTF16Selection, Window, div, fill, font, point, prelude::*, px,
    relative, size,
};
use spike_common::{self as sc, Highlighter, Samples, qur_dark};

use crate::commands::{edit, editor as ea};
use crate::ui::{CODE_FONT, InputEvent, TextInput, Ui, hsla};

type LineSpans = Vec<(Range<usize>, u32)>;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Pos {
    pub line: usize,
    /// Byte column.
    pub col: usize,
}

fn pos(line: usize, col: usize) -> Pos {
    Pos { line, col }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Sel {
    anchor: Pos,
    head: Pos,
}

impl Sel {
    fn caret(p: Pos) -> Sel {
        Sel { anchor: p, head: p }
    }
    fn range(&self) -> (Pos, Pos) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }
    fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone)]
struct EditRec {
    start: Pos,
    old: String,
    new: String,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Kind {
    Typing,
    Other,
}

struct Txn {
    id: u64,
    edits: Vec<EditRec>,
    sel_before: Sel,
    sel_after: Sel,
    kind: Kind,
    at: Instant,
}

/// Emitted so the window can update the title (dirty marker) and menus.
pub enum EditorEvent {
    DirtyChanged,
    WrapChanged,
}

struct Find {
    query: Entity<TextInput>,
    replace: Option<Entity<TextInput>>,
    count: usize,
    _subs: Vec<Subscription>,
}

/// Spike measurement state (see spike_common::Config).
#[derive(Default)]
pub struct Bench {
    pub first_frame_done: bool,
    pub scroll: Samples,
    pub scroll_work: Samples,
    pub typing: Samples,
    pub keys_sent: usize,
    pub pending_key: Option<Instant>,
    pub key_outstanding: bool,
    pub frame_start: Option<Instant>,
    pub done: bool,
}

pub struct Editor {
    pub focus: FocusHandle,
    pub path: std::path::PathBuf,
    lines: Vec<String>,
    spans: Vec<LineSpans>,
    sel: Sel,
    /// Preferred column (chars into the visual row) for up/down.
    goal: Option<usize>,

    pub wrap: bool,
    wrap_cols: usize,
    /// Visual rows per line, and the first visual row of each line.
    rows: Vec<u32>,
    row_start: Vec<usize>,

    pub scroll_y: Pixels,
    viewport_h: Pixels,
    cell_w: Pixels,
    text_x: Pixels,
    last_bounds: Option<Bounds<Pixels>>,
    selecting: bool,

    undo: Vec<Txn>,
    redo: Vec<Txn>,
    line_clip: Option<String>,
    /// Bumped on every text change (re-highlight staleness check).
    version: u64,
    next_txn_id: u64,
    /// Undo position (top transaction id) at the last save.
    saved_txn: u64,
    /// Typing after a save starts a new transaction.
    block_merge: bool,

    find: Option<Find>,

    highlighter: Arc<Mutex<Highlighter>>,
    hl_in_flight: bool,

    pub cfg_autoscroll: bool,
    pub cfg_autotype: bool,
    pub cfg_exit_after_first_frame: bool,
    pub bench: Bench,
}

impl EventEmitter<EditorEvent> for Editor {}

fn split_spans(src: &str, spans: &[sc::Span]) -> Vec<LineSpans> {
    let mut out = Vec::new();
    let mut si = 0;
    let mut line_start = 0;
    for line in src.split('\n') {
        let line_end = line_start + line.len();
        let mut ls = Vec::new();
        while si < spans.len() && spans[si].range.start < line_end {
            let s = &spans[si];
            let (a, b) = (s.range.start.max(line_start), s.range.end.min(line_end));
            if b > a {
                ls.push((a - line_start..b - line_start, qur_dark::syntax(s.kind)));
            }
            if s.range.end <= line_end + 1 {
                si += 1;
            } else {
                break;
            }
        }
        out.push(ls);
        line_start = line_end + 1;
    }
    out
}

#[derive(PartialEq, Clone, Copy)]
enum CharClass {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' || c == '$' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

fn char_count(s: &str) -> usize {
    if s.is_ascii() { s.len() } else { s.chars().count() }
}

/// Byte offset of the `n`th char (clamped to the end).
fn byte_at_char(s: &str, n: usize) -> usize {
    if s.is_ascii() {
        n.min(s.len())
    } else {
        s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len())
    }
}

impl Editor {
    pub fn new(cfg: &sc::Config, cx: &mut Context<Self>) -> Self {
        let t = Instant::now();
        let src = std::fs::read_to_string(&cfg.file).expect("read spike file");
        let mut hl = Highlighter::new();
        let spans = hl.highlight(&src);
        let mut spans = split_spans(&src, &spans);
        let mut lines: Vec<String> = src.split('\n').map(str::to_string).collect();
        if src.ends_with('\n') && lines.len() > 1 {
            lines.pop();
            spans.pop();
        }
        sc::metric("open_10k_ms", t.elapsed().as_millis());
        sc::metric("lines", lines.len());
        let mut e = Editor {
            focus: cx.focus_handle(),
            path: cfg.file.clone(),
            rows: vec![1; lines.len()],
            row_start: Vec::new(),
            lines,
            spans,
            sel: Sel::default(),
            goal: None,
            wrap: true,
            wrap_cols: 0,
            scroll_y: px(0.),
            viewport_h: px(800.),
            cell_w: px(8.),
            text_x: px(0.),
            last_bounds: None,
            selecting: false,
            undo: Vec::new(),
            redo: Vec::new(),
            line_clip: None,
            version: 0,
            next_txn_id: 1,
            saved_txn: 0,
            block_merge: false,
            find: None,
            highlighter: Arc::new(Mutex::new(hl)),
            hl_in_flight: false,
            cfg_autoscroll: cfg.autoscroll,
            cfg_autotype: cfg.autotype,
            cfg_exit_after_first_frame: cfg.exit_after_first_frame,
            bench: Bench::default(),
        };
        e.rebuild_rows();
        e
    }

    pub fn file_name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Spike scripting: print the selection and the lines around it.
    pub fn debug_dump(&self) {
        let l = self.sel.head.line;
        eprintln!("DUMP sel={:?}..{:?} lines={} dirty={} wrap={} rows={}", self.sel.anchor, self.sel.head, self.lines.len(), self.is_dirty(), self.wrap, self.total_rows());
        for i in l.saturating_sub(1)..(l + 3).min(self.lines.len()) {
            eprintln!("DUMP {:>5}| {}", i + 1, self.lines[i]);
        }
    }

    pub fn total_lines(&self) -> usize {
        self.lines.len()
    }

    pub fn is_dirty(&self) -> bool {
        self.undo.last().map(|t| t.id).unwrap_or(0) != self.saved_txn
    }

    fn indent_unit(&self) -> &'static str {
        match self.path.extension().and_then(|e| e.to_str()) {
            Some("ts" | "tsx" | "js" | "jsx" | "json" | "yaml" | "yml" | "css" | "html" | "dart") => "  ",
            _ => "    ",
        }
    }

    fn comment_prefix(&self) -> &'static str {
        match self.path.extension().and_then(|e| e.to_str()) {
            Some("py" | "toml" | "yaml" | "yml" | "sh") => "#",
            _ => "//",
        }
    }

    pub fn lh(cx: &App) -> Pixels {
        Ui::get(cx).z(19.)
    }

    // -- visual rows --------------------------------------------------------

    fn line_rows(&self, l: usize) -> u32 {
        if !self.wrap || self.wrap_cols == 0 {
            return 1;
        }
        let n = char_count(&self.lines[l]);
        n.div_ceil(self.wrap_cols).max(1) as u32
    }

    fn rebuild_rows(&mut self) {
        self.rows = (0..self.lines.len()).map(|l| self.line_rows(l)).collect();
        self.rebuild_row_start();
    }

    fn rebuild_row_start(&mut self) {
        self.row_start.clear();
        self.row_start.reserve(self.rows.len());
        let mut acc = 0;
        for &r in &self.rows {
            self.row_start.push(acc);
            acc += r as usize;
        }
    }

    pub fn total_rows(&self) -> usize {
        self.row_start.last().map(|s| s + *self.rows.last().unwrap() as usize).unwrap_or(0)
    }

    /// Visual row -> (line, row within line).
    fn row_to_line(&self, row: usize) -> (usize, usize) {
        let l = self.row_start.partition_point(|&s| s <= row).saturating_sub(1);
        (l, row - self.row_start[l])
    }

    /// Byte range of row `k` within line `l`.
    fn row_bytes(&self, l: usize, k: usize) -> Range<usize> {
        let s = &self.lines[l];
        if !self.wrap || self.wrap_cols == 0 {
            return 0..s.len();
        }
        let a = byte_at_char(s, k * self.wrap_cols);
        let b = byte_at_char(s, (k + 1) * self.wrap_cols);
        a..b
    }

    /// (visual row, chars into that row) of a position.
    fn pos_to_row(&self, p: Pos) -> (usize, usize) {
        let s = &self.lines[p.line];
        let c = char_count(&s[..p.col]);
        if !self.wrap || self.wrap_cols == 0 {
            return (self.row_start[p.line], c);
        }
        let rows = self.rows[p.line] as usize;
        let mut k = c / self.wrap_cols;
        if k >= rows {
            k = rows - 1;
        }
        (self.row_start[p.line] + k, c - k * self.wrap_cols)
    }

    fn row_col_to_pos(&self, row: usize, col: usize) -> Pos {
        let (l, k) = self.row_to_line(row.min(self.total_rows().saturating_sub(1)));
        let r = self.row_bytes(l, k);
        let seg = &self.lines[l][r.clone()];
        let last_row = k + 1 == self.rows[l] as usize;
        let max = char_count(seg) - if !last_row && !seg.is_empty() { 1 } else { 0 };
        pos(l, r.start + byte_at_char(seg, col.min(max)))
    }

    pub fn max_scroll(&self, cx: &App) -> Pixels {
        (Self::lh(cx) * self.total_rows() as f32 - self.viewport_h).max(px(0.))
    }

    fn reveal(&mut self, cx: &App) {
        let lh = Self::lh(cx);
        let (row, _) = self.pos_to_row(self.sel.head);
        let y = lh * row as f32;
        if y < self.scroll_y {
            self.scroll_y = y;
        } else if y + lh > self.scroll_y + self.viewport_h {
            self.scroll_y = y + lh - self.viewport_h;
        }
    }

    // -- text model ---------------------------------------------------------

    fn text_between(&self, a: Pos, b: Pos) -> String {
        if a.line == b.line {
            return self.lines[a.line][a.col..b.col].to_string();
        }
        let mut s = self.lines[a.line][a.col..].to_string();
        for l in a.line + 1..b.line {
            s.push('\n');
            s.push_str(&self.lines[l]);
        }
        s.push('\n');
        s.push_str(&self.lines[b.line][..b.col]);
        s
    }

    fn end_pos(&self) -> Pos {
        let l = self.lines.len() - 1;
        pos(l, self.lines[l].len())
    }

    /// Replace `a..b` with `text`. Returns the end of the inserted text.
    /// Keeps spans roughly right (shifted/clipped) until the background
    /// re-highlight lands.
    fn splice(&mut self, a: Pos, b: Pos, text: &str) -> Pos {
        let prefix = self.lines[a.line][..a.col].to_string();
        let suffix = self.lines[b.line][b.col..].to_string();
        let pieces: Vec<&str> = text.split('\n').collect();
        let k = pieces.len();
        let last_len = if k == 1 { a.col + text.len() } else { pieces[k - 1].len() };

        let mut new_lines = Vec::with_capacity(k);
        for (i, p) in pieces.iter().enumerate() {
            let mut s = String::new();
            if i == 0 {
                s.push_str(&prefix);
            }
            s.push_str(p);
            if i == k - 1 {
                s.push_str(&suffix);
            }
            new_lines.push(s);
        }

        // Spans: keep the head of the first line, shift the tail of the last.
        let mut first: LineSpans = self.spans[a.line]
            .iter()
            .filter(|(r, _)| r.start < a.col)
            .map(|(r, c)| (r.start..r.end.min(a.col), *c))
            .collect();
        let tail: LineSpans = self.spans[b.line]
            .iter()
            .filter(|(r, _)| r.end > b.col)
            .map(|(r, c)| (r.start.max(b.col) - b.col + last_len..r.end - b.col + last_len, *c))
            .collect();
        let mut new_spans: Vec<LineSpans> = vec![Vec::new(); k];
        if k == 1 {
            first.extend(tail);
            new_spans[0] = first;
        } else {
            new_spans[0] = first;
            new_spans[k - 1] = tail;
        }

        self.lines.splice(a.line..=b.line, new_lines);
        self.spans.splice(a.line..=b.line, new_spans);
        let new_rows: Vec<u32> = (a.line..a.line + k).map(|l| self.line_rows(l)).collect();
        self.rows.splice(a.line..=b.line, new_rows);
        self.rebuild_row_start();
        pos(a.line + k - 1, last_len)
    }

    /// One undoable edit. Typing merges into the previous transaction.
    fn edit(&mut self, a: Pos, b: Pos, text: &str, sel_after: Option<Sel>, kind: Kind, cx: &mut Context<Self>) {
        let was_dirty = self.is_dirty();
        let old = self.text_between(a, b);
        let before = self.sel;
        let end = self.splice(a, b, text);
        self.sel = sel_after.unwrap_or(Sel::caret(end));
        let rec = EditRec { start: a, old, new: text.to_string() };
        let merge = kind == Kind::Typing
            && !std::mem::take(&mut self.block_merge)
            && self.undo.last().is_some_and(|t| {
                t.kind == Kind::Typing
                    && t.at.elapsed() < Duration::from_secs(1)
                    && t.edits.last().is_some_and(|e| {
                        let mut ep = e.start;
                        let segs: Vec<&str> = e.new.split('\n').collect();
                        ep.line += segs.len() - 1;
                        ep.col = if segs.len() == 1 { ep.col + e.new.len() } else { segs[segs.len() - 1].len() };
                        ep == a
                    })
            })
            && !text.contains('\n');
        if merge {
            let t = self.undo.last_mut().unwrap();
            t.edits.push(rec);
            t.sel_after = self.sel;
            t.at = Instant::now();
        } else {
            let id = self.next_txn_id;
            self.next_txn_id += 1;
            self.undo.push(Txn { id, edits: vec![rec], sel_before: before, sel_after: self.sel, kind, at: Instant::now() });
        }
        self.redo.clear();
        self.after_change(was_dirty, cx);
    }

    fn after_change(&mut self, was_dirty: bool, cx: &mut Context<Self>) {
        self.version += 1;
        self.goal = None;
        self.reveal(cx);
        self.spawn_rehighlight(cx);
        if was_dirty != self.is_dirty() {
            cx.emit(EditorEvent::DirtyChanged);
        }
        if self.find.is_some() {
            self.recount(cx);
        }
        cx.notify();
    }

    fn apply_inverse(&mut self, t: &Txn) {
        for e in t.edits.iter().rev() {
            let segs: Vec<&str> = e.new.split('\n').collect();
            let end = pos(
                e.start.line + segs.len() - 1,
                if segs.len() == 1 { e.start.col + e.new.len() } else { segs[segs.len() - 1].len() },
            );
            self.splice(e.start, end, &e.old);
        }
    }

    fn undo(&mut self, _: &edit::Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.undo.pop() {
            self.apply_inverse(&t);
            self.sel = t.sel_before;
            self.redo.push(t);
            self.version += 1;
            self.after_undo(cx);
        }
    }

    fn redo(&mut self, _: &edit::Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.redo.pop() {
            for e in &t.edits {
                let segs: Vec<&str> = e.old.split('\n').collect();
                let end = pos(
                    e.start.line + segs.len() - 1,
                    if segs.len() == 1 { e.start.col + e.old.len() } else { segs[segs.len() - 1].len() },
                );
                self.splice(e.start, end, &e.new);
            }
            self.sel = t.sel_after;
            self.undo.push(t);
            self.version += 1;
            self.after_undo(cx);
        }
    }

    fn after_undo(&mut self, cx: &mut Context<Self>) {
        self.reveal(cx);
        self.spawn_rehighlight(cx);
        cx.emit(EditorEvent::DirtyChanged);
        cx.notify();
    }

    fn spawn_rehighlight(&mut self, cx: &mut Context<Self>) {
        if self.hl_in_flight {
            return;
        }
        self.hl_in_flight = true;
        let version = self.version;
        let src = self.lines.join("\n");
        let hl = self.highlighter.clone();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = bg
                .spawn(async move {
                    let spans = hl.lock().unwrap().highlight(&src);
                    split_spans(&src, &spans)
                })
                .await;
            this.update(cx, |this, cx| {
                this.hl_in_flight = false;
                if this.version == version && result.len() == this.lines.len() {
                    this.spans = result;
                    cx.notify();
                } else {
                    this.spawn_rehighlight(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // -- typing -------------------------------------------------------------

    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let (a, b) = self.sel.range();
        let kind = if a == b && text.chars().count() == 1 { Kind::Typing } else { Kind::Other };
        self.edit(a, b, text, None, kind, cx);
    }

    fn newline(&mut self, _: &ea::Newline, _: &mut Window, cx: &mut Context<Self>) {
        let l = self.sel.range().0.line;
        let indent: String = self.lines[l].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        self.insert_text(&format!("\n{indent}"), cx);
    }

    fn delete_dir(&mut self, target: Pos, cx: &mut Context<Self>) {
        if !self.sel.is_empty() {
            let (a, b) = self.sel.range();
            return self.edit(a, b, "", None, Kind::Other, cx);
        }
        let h = self.sel.head;
        let (a, b) = if target < h { (target, h) } else { (h, target) };
        if a != b {
            self.edit(a, b, "", Some(Sel::caret(a)), Kind::Other, cx);
        }
    }

    fn prev_char(&self, p: Pos) -> Pos {
        if p.col > 0 {
            let c = self.lines[p.line][..p.col].char_indices().last().map(|(i, _)| i).unwrap_or(0);
            pos(p.line, c)
        } else if p.line > 0 {
            pos(p.line - 1, self.lines[p.line - 1].len())
        } else {
            p
        }
    }

    fn next_char(&self, p: Pos) -> Pos {
        if let Some(ch) = self.lines[p.line][p.col..].chars().next() {
            pos(p.line, p.col + ch.len_utf8())
        } else if p.line + 1 < self.lines.len() {
            pos(p.line + 1, 0)
        } else {
            p
        }
    }

    fn word_left(&self, p: Pos) -> Pos {
        if p.col == 0 {
            return self.prev_char(p);
        }
        let s = &self.lines[p.line][..p.col];
        let mut it = s.char_indices().rev().peekable();
        while let Some(&(_, c)) = it.peek() {
            if class(c) != CharClass::Space {
                break;
            }
            it.next();
        }
        let Some(&(mut at, c0)) = it.peek() else { return pos(p.line, 0) };
        let k = class(c0);
        for (i, c) in it {
            if class(c) != k {
                break;
            }
            at = i;
        }
        pos(p.line, at)
    }

    fn word_right(&self, p: Pos) -> Pos {
        let s = &self.lines[p.line];
        if p.col >= s.len() {
            return self.next_char(p);
        }
        let mut it = s[p.col..].char_indices().peekable();
        while let Some(&(_, c)) = it.peek() {
            if class(c) != CharClass::Space {
                break;
            }
            it.next();
        }
        let Some(&(_, c0)) = it.peek() else { return pos(p.line, s.len()) };
        let k = class(c0);
        for (i, c) in it {
            if class(c) != k {
                return pos(p.line, p.col + i);
            }
        }
        pos(p.line, s.len())
    }

    fn smart_home(&self, p: Pos) -> Pos {
        let s = &self.lines[p.line];
        let first = s.len() - s.trim_start().len();
        // Within a wrapped continuation row, Home goes to the row start first.
        let (row, _) = self.pos_to_row(p);
        let (_, k) = self.row_to_line(row);
        if k > 0 {
            let rs = self.row_bytes(p.line, k).start;
            if p.col != rs {
                return pos(p.line, rs);
            }
        }
        pos(p.line, if p.col == first { 0 } else { first })
    }

    fn line_end(&self, p: Pos) -> Pos {
        let (row, _) = self.pos_to_row(p);
        let (_, k) = self.row_to_line(row);
        let r = self.row_bytes(p.line, k);
        if k + 1 < self.rows[p.line] as usize {
            let seg = &self.lines[p.line][r.clone()];
            let end = r.start + byte_at_char(seg, char_count(seg) - 1);
            if p.col != end {
                return pos(p.line, end);
            }
        }
        pos(p.line, self.lines[p.line].len())
    }

    fn vertical(&mut self, delta: isize) -> Pos {
        let (row, col) = self.pos_to_row(self.sel.head);
        let goal = *self.goal.get_or_insert(col);
        let target = row as isize + delta;
        if target < 0 {
            return pos(0, 0);
        }
        if target as usize >= self.total_rows() {
            return self.end_pos();
        }
        self.row_col_to_pos(target as usize, goal)
    }

    fn page_rows(&self, cx: &App) -> isize {
        ((self.viewport_h / Self::lh(cx)).floor() as isize - 1).max(1)
    }

    fn move_to(&mut self, p: Pos, extend: bool, keep_goal: bool, cx: &mut Context<Self>) {
        if extend {
            self.sel.head = p;
        } else {
            self.sel = Sel::caret(p);
        }
        if !keep_goal {
            self.goal = None;
        }
        self.reveal(cx);
        cx.notify();
    }

    fn left(&mut self, _: &ea::Left, _: &mut Window, cx: &mut Context<Self>) {
        let p = if self.sel.is_empty() { self.prev_char(self.sel.head) } else { self.sel.range().0 };
        self.move_to(p, false, false, cx)
    }
    fn right(&mut self, _: &ea::Right, _: &mut Window, cx: &mut Context<Self>) {
        let p = if self.sel.is_empty() { self.next_char(self.sel.head) } else { self.sel.range().1 };
        self.move_to(p, false, false, cx)
    }
    fn up(&mut self, _: &ea::Up, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.vertical(-1);
        self.move_to(p, false, true, cx)
    }
    fn down(&mut self, _: &ea::Down, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.vertical(1);
        self.move_to(p, false, true, cx)
    }
    fn select_left(&mut self, _: &ea::SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.prev_char(self.sel.head);
        self.move_to(p, true, false, cx)
    }
    fn select_right(&mut self, _: &ea::SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.next_char(self.sel.head);
        self.move_to(p, true, false, cx)
    }
    fn select_up(&mut self, _: &ea::SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.vertical(-1);
        self.move_to(p, true, true, cx)
    }
    fn select_down(&mut self, _: &ea::SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.vertical(1);
        self.move_to(p, true, true, cx)
    }
    fn word_left_a(&mut self, _: &ea::WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.word_left(self.sel.head);
        self.move_to(p, false, false, cx)
    }
    fn word_right_a(&mut self, _: &ea::WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.word_right(self.sel.head);
        self.move_to(p, false, false, cx)
    }
    fn select_word_left(&mut self, _: &ea::SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.word_left(self.sel.head);
        self.move_to(p, true, false, cx)
    }
    fn select_word_right(&mut self, _: &ea::SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.word_right(self.sel.head);
        self.move_to(p, true, false, cx)
    }
    fn line_start(&mut self, _: &ea::LineStart, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.smart_home(self.sel.head);
        self.move_to(p, false, false, cx)
    }
    fn line_end_a(&mut self, _: &ea::LineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.line_end(self.sel.head);
        self.move_to(p, false, false, cx)
    }
    fn select_line_start(&mut self, _: &ea::SelectLineStart, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.smart_home(self.sel.head);
        self.move_to(p, true, false, cx)
    }
    fn select_line_end(&mut self, _: &ea::SelectLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.line_end(self.sel.head);
        self.move_to(p, true, false, cx)
    }
    fn doc_start(&mut self, _: &ea::DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(pos(0, 0), false, false, cx)
    }
    fn doc_end(&mut self, _: &ea::DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.end_pos();
        self.move_to(p, false, false, cx)
    }
    fn select_doc_start(&mut self, _: &ea::SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(pos(0, 0), true, false, cx)
    }
    fn select_doc_end(&mut self, _: &ea::SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.end_pos();
        self.move_to(p, true, false, cx)
    }
    fn page(&mut self, dir: isize, extend: bool, cx: &mut Context<Self>) {
        let n = self.page_rows(cx) * dir;
        let p = self.vertical(n);
        self.scroll_y = (self.scroll_y + Self::lh(cx) * n as f32).clamp(px(0.), self.max_scroll(cx));
        self.move_to(p, extend, true, cx)
    }
    fn page_up(&mut self, _: &ea::PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.page(-1, false, cx)
    }
    fn page_down(&mut self, _: &ea::PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.page(1, false, cx)
    }
    fn select_page_up(&mut self, _: &ea::SelectPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.page(-1, true, cx)
    }
    fn select_page_down(&mut self, _: &ea::SelectPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.page(1, true, cx)
    }
    fn select_all(&mut self, _: &edit::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.sel = Sel { anchor: pos(0, 0), head: self.end_pos() };
        cx.notify();
    }

    fn backspace(&mut self, _: &ea::Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let t = self.prev_char(self.sel.head);
        self.delete_dir(t, cx)
    }
    fn delete(&mut self, _: &ea::Delete, _: &mut Window, cx: &mut Context<Self>) {
        let t = self.next_char(self.sel.head);
        self.delete_dir(t, cx)
    }
    fn delete_word_left(&mut self, _: &ea::DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let t = self.word_left(self.sel.head);
        self.delete_dir(t, cx)
    }
    fn delete_word_right(&mut self, _: &ea::DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let t = self.word_right(self.sel.head);
        self.delete_dir(t, cx)
    }
    fn delete_to_line_start(&mut self, _: &ea::DeleteToLineStart, _: &mut Window, cx: &mut Context<Self>) {
        let t = pos(self.sel.head.line, 0);
        self.delete_dir(t, cx)
    }

    // -- line operations ----------------------------------------------------

    /// Lines covered by the selection (a trailing selection end at column 0
    /// doesn't count, as in VS Code).
    fn sel_lines(&self) -> (usize, usize) {
        let (a, b) = self.sel.range();
        let last = if b.col == 0 && b.line > a.line { b.line - 1 } else { b.line };
        (a.line, last)
    }

    fn shift_sel(&self, dl: isize) -> Sel {
        let f = |p: Pos| pos((p.line as isize + dl) as usize, p.col);
        Sel { anchor: f(self.sel.anchor), head: f(self.sel.head) }
    }

    fn delete_line(&mut self, _: &ea::DeleteLine, _: &mut Window, cx: &mut Context<Self>) {
        let (l0, l1) = self.sel_lines();
        let (a, b) = if l1 + 1 < self.lines.len() {
            (pos(l0, 0), pos(l1 + 1, 0))
        } else if l0 > 0 {
            (pos(l0 - 1, self.lines[l0 - 1].len()), pos(l1, self.lines[l1].len()))
        } else {
            (pos(0, 0), pos(l1, self.lines[l1].len()))
        };
        self.edit(a, b, "", None, Kind::Other, cx);
    }

    fn move_line(&mut self, down: bool, cx: &mut Context<Self>) {
        let (l0, l1) = self.sel_lines();
        if (!down && l0 == 0) || (down && l1 + 1 >= self.lines.len()) {
            return;
        }
        let block = self.lines[l0..=l1].join("\n");
        if down {
            let other = self.lines[l1 + 1].clone();
            let sel = self.shift_sel(1);
            self.edit(pos(l0, 0), pos(l1 + 1, other.len()), &format!("{other}\n{block}"), Some(sel), Kind::Other, cx);
        } else {
            let other = self.lines[l0 - 1].clone();
            let sel = self.shift_sel(-1);
            self.edit(pos(l0 - 1, 0), pos(l1, self.lines[l1].len()), &format!("{block}\n{other}"), Some(sel), Kind::Other, cx);
        }
    }
    fn move_line_up(&mut self, _: &ea::MoveLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_line(false, cx)
    }
    fn move_line_down(&mut self, _: &ea::MoveLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_line(true, cx)
    }

    fn copy_line(&mut self, down: bool, cx: &mut Context<Self>) {
        let (l0, l1) = self.sel_lines();
        let block = self.lines[l0..=l1].join("\n");
        if down {
            let sel = self.shift_sel((l1 - l0 + 1) as isize);
            let end = pos(l1, self.lines[l1].len());
            self.edit(end, end, &format!("\n{block}"), Some(sel), Kind::Other, cx);
        } else {
            let sel = self.sel;
            self.edit(pos(l0, 0), pos(l0, 0), &format!("{block}\n"), Some(sel), Kind::Other, cx);
        }
    }
    fn copy_line_up(&mut self, _: &ea::CopyLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_line(false, cx)
    }
    fn copy_line_down(&mut self, _: &ea::CopyLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_line(true, cx)
    }

    fn leading_ws(&self, l: usize) -> String {
        self.lines[l].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
    }

    fn insert_line_below(&mut self, _: &ea::InsertLineBelow, _: &mut Window, cx: &mut Context<Self>) {
        let l = self.sel.head.line;
        let ind = self.leading_ws(l);
        let end = pos(l, self.lines[l].len());
        self.edit(end, end, &format!("\n{ind}"), None, Kind::Other, cx);
    }
    fn insert_line_above(&mut self, _: &ea::InsertLineAbove, _: &mut Window, cx: &mut Context<Self>) {
        let l = self.sel.head.line;
        let ind = self.leading_ws(l);
        let n = ind.len();
        self.edit(pos(l, 0), pos(l, 0), &format!("{ind}\n"), Some(Sel::caret(pos(l, n))), Kind::Other, cx);
    }

    /// Rewrite whole lines l0..=l1 with `f`, keeping the selection on them.
    fn map_lines(&mut self, f: impl Fn(&str) -> String, cx: &mut Context<Self>) {
        let (l0, l1) = self.sel_lines();
        let new: Vec<String> = self.lines[l0..=l1].iter().map(|s| f(s)).collect();
        let delta_a = new[0].len() as isize - self.lines[l0].len() as isize;
        let delta_b = new[l1 - l0].len() as isize - self.lines[l1].len() as isize;
        let adj = |p: Pos| {
            let d = if p.line == l0 { delta_a } else if p.line == l1 { delta_b } else { 0 };
            pos(p.line, (p.col as isize + d).clamp(0, if p.line == l0 { new[0].len() } else { new[p.line - l0].len() } as isize) as usize)
        };
        let sel = if (l0..=l1).contains(&self.sel.anchor.line) && (l0..=l1).contains(&self.sel.head.line) {
            Sel { anchor: adj(self.sel.anchor), head: adj(self.sel.head) }
        } else {
            self.sel
        };
        let end = pos(l1, self.lines[l1].len());
        self.edit(pos(l0, 0), end, &new.join("\n"), Some(sel), Kind::Other, cx);
    }

    fn indent_lines(&mut self, cx: &mut Context<Self>) {
        let unit = self.indent_unit();
        self.map_lines(|s| if s.is_empty() { String::new() } else { format!("{unit}{s}") }, cx)
    }

    fn outdent_lines(&mut self, cx: &mut Context<Self>) {
        let n = self.indent_unit().len();
        self.map_lines(
            |s| {
                let k = s.chars().take(n).take_while(|c| *c == ' ').count();
                let k = if k == 0 && s.starts_with('\t') { 1 } else { k };
                s[k..].to_string()
            },
            cx,
        )
    }

    fn indent(&mut self, _: &ea::Indent, _: &mut Window, cx: &mut Context<Self>) {
        self.indent_lines(cx)
    }
    fn outdent(&mut self, _: &ea::Outdent, _: &mut Window, cx: &mut Context<Self>) {
        self.outdent_lines(cx)
    }
    fn tab(&mut self, _: &ea::Tab, _: &mut Window, cx: &mut Context<Self>) {
        let (a, b) = self.sel.range();
        if a.line != b.line {
            return self.indent_lines(cx);
        }
        let unit = self.indent_unit().len();
        let col = char_count(&self.lines[a.line][..a.col]);
        let n = unit - col % unit;
        self.insert_text(&" ".repeat(n), cx)
    }
    fn shift_tab(&mut self, _: &ea::ShiftTab, _: &mut Window, cx: &mut Context<Self>) {
        self.outdent_lines(cx)
    }

    fn toggle_comment(&mut self, _: &ea::ToggleComment, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.comment_prefix();
        let (l0, l1) = self.sel_lines();
        let body: Vec<&String> = self.lines[l0..=l1].iter().filter(|s| !s.trim().is_empty()).collect();
        if body.is_empty() {
            return;
        }
        let all = body.iter().all(|s| s.trim_start().starts_with(p));
        let min_ind = body.iter().map(|s| s.len() - s.trim_start().len()).min().unwrap_or(0);
        self.map_lines(
            |s| {
                if s.trim().is_empty() {
                    return s.to_string();
                }
                if all {
                    let i = s.len() - s.trim_start().len();
                    let rest = &s[i + p.len()..];
                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                    format!("{}{}", &s[..i], rest)
                } else {
                    format!("{}{} {}", &s[..min_ind], p, &s[min_ind..])
                }
            },
            cx,
        )
    }

    fn select_line(&mut self, _: &ea::SelectLine, _: &mut Window, cx: &mut Context<Self>) {
        let (a, b) = self.sel.range();
        let next = |l: usize| if l + 1 < self.lines.len() { pos(l + 1, 0) } else { self.end_pos() };
        // Already whole lines: extend by one more line (repeat to grow).
        let whole = !self.sel.is_empty() && a.col == 0 && b.col == 0;
        let anchor = if whole { a } else { pos(a.line, 0) };
        self.sel = Sel { anchor, head: next(b.line) };
        self.reveal(cx);
        cx.notify();
    }

    fn escape(&mut self, _: &ea::Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_some() {
            window.focus(&self.focus, cx);
        } else {
            self.sel = Sel::caret(self.sel.head);
        }
        cx.notify();
    }

    pub fn toggle_soft_wrap(&mut self, cx: &mut Context<Self>) {
        // Keep the caret's line at the same screen offset.
        let lh = Self::lh(cx);
        let (row, _) = self.pos_to_row(self.sel.head);
        let off = lh * row as f32 - self.scroll_y;
        self.wrap = !self.wrap;
        self.rebuild_rows();
        let (row, _) = self.pos_to_row(self.sel.head);
        self.scroll_y = (lh * row as f32 - off).clamp(px(0.), self.max_scroll(cx));
        cx.emit(EditorEvent::WrapChanged);
        cx.notify();
    }

    fn save(&mut self, _: &ea::Save, _: &mut Window, cx: &mut Context<Self>) {
        if std::env::var("QUR_SPIKE_ALLOW_SAVE").as_deref() == Ok("1") {
            let mut s = self.lines.join("\n");
            s.push('\n');
            if let Err(e) = std::fs::write(&self.path, s) {
                eprintln!("save failed: {e}");
                return;
            }
        }
        self.saved_txn = self.undo.last().map(|t| t.id).unwrap_or(0);
        self.block_merge = true;
        cx.emit(EditorEvent::DirtyChanged);
        cx.notify();
    }

    // -- clipboard ----------------------------------------------------------

    fn copy_text(&mut self) -> (String, bool) {
        if self.sel.is_empty() {
            let l = self.sel.head.line;
            (format!("{}\n", self.lines[l]), true)
        } else {
            let (a, b) = self.sel.range();
            (self.text_between(a, b), false)
        }
    }

    fn copy(&mut self, _: &edit::Copy, _: &mut Window, cx: &mut Context<Self>) {
        let (t, whole) = self.copy_text();
        self.line_clip = whole.then(|| t.clone());
        cx.write_to_clipboard(ClipboardItem::new_string(t));
    }

    fn cut(&mut self, _: &edit::Cut, _: &mut Window, cx: &mut Context<Self>) {
        let (t, whole) = self.copy_text();
        self.line_clip = whole.then(|| t.clone());
        cx.write_to_clipboard(ClipboardItem::new_string(t));
        if whole {
            let l = self.sel.head.line;
            let (a, b) = if l + 1 < self.lines.len() {
                (pos(l, 0), pos(l + 1, 0))
            } else if l > 0 {
                (pos(l - 1, self.lines[l - 1].len()), pos(l, self.lines[l].len()))
            } else {
                (pos(0, 0), pos(0, self.lines[0].len()))
            };
            self.edit(a, b, "", None, Kind::Other, cx);
        } else {
            let (a, b) = self.sel.range();
            self.edit(a, b, "", None, Kind::Other, cx);
        }
    }

    fn paste(&mut self, _: &edit::Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        if self.sel.is_empty() && self.line_clip.as_deref() == Some(t.as_str()) {
            let l = self.sel.head.line;
            let caret = pos(self.sel.head.line + 1, self.sel.head.col);
            self.edit(pos(l, 0), pos(l, 0), &t, Some(Sel::caret(caret)), Kind::Other, cx);
        } else {
            self.insert_text(&t, cx);
        }
    }

    // -- find / replace -----------------------------------------------------

    pub fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let seed = if !self.sel.is_empty() && self.sel.anchor.line == self.sel.head.line {
            let (a, b) = self.sel.range();
            Some(self.text_between(a, b))
        } else {
            None
        };
        if self.find.is_none() {
            let query = cx.new(|cx| TextInput::new("Find", cx));
            let sub = cx.subscribe_in(&query, window, |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Changed => this.recount(cx),
                InputEvent::Confirm { alt, .. } => this.find_step(!*alt, cx),
                InputEvent::Cancel => {
                    this.find = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }
                _ => {}
            });
            self.find = Some(Find { query, replace: None, count: 0, _subs: vec![sub] });
        }
        if replace && self.find.as_ref().unwrap().replace.is_none() {
            let r = cx.new(|cx| TextInput::new("Replace", cx));
            let sub = cx.subscribe_in(&r, window, |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Confirm { all: true, .. } => this.replace_all(cx),
                InputEvent::Confirm { .. } => this.replace_one(cx),
                InputEvent::Cancel => {
                    this.find = None;
                    window.focus(&this.focus, cx);
                    cx.notify();
                }
                _ => {}
            });
            let f = self.find.as_mut().unwrap();
            f.replace = Some(r);
            f._subs.push(sub);
        }
        let q = self.find.as_ref().unwrap().query.clone();
        if let Some(seed) = seed {
            q.update(cx, |i, cx| i.set_text(&seed, cx));
        }
        window.focus(&q.read(cx).focus.clone(), cx);
        self.recount(cx);
        cx.notify();
    }

    fn query(&self, cx: &App) -> String {
        self.find.as_ref().map(|f| f.query.read(cx).text.clone()).unwrap_or_default()
    }

    fn recount(&mut self, cx: &mut Context<Self>) {
        let q = self.query(cx);
        let n = if q.is_empty() { 0 } else { self.lines.iter().map(|l| l.matches(q.as_str()).count()).sum() };
        if let Some(f) = self.find.as_mut() {
            f.count = n;
        }
        cx.notify();
    }

    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let q = self.query(cx);
        if q.is_empty() {
            return;
        }
        let (a, b) = self.sel.range();
        let n = self.lines.len();
        let hit = if forward {
            (0..=n).find_map(|i| {
                let l = (b.line + i) % n;
                let from = if i == 0 { b.col } else { 0 };
                let s = &self.lines[l];
                s.get(from..).and_then(|t| t.find(&q)).map(|c| pos(l, from + c))
            })
        } else {
            (0..=n).find_map(|i| {
                let l = (a.line + n - i % n) % n;
                let to = if i == 0 { a.col } else { self.lines[l].len() };
                self.lines[l][..to].rfind(&q).map(|c| pos(l, c))
            })
        };
        if let Some(p) = hit {
            self.sel = Sel { anchor: p, head: pos(p.line, p.col + q.len()) };
            self.reveal(cx);
            cx.notify();
        }
    }

    fn replace_text(&self, cx: &App) -> String {
        self.find
            .as_ref()
            .and_then(|f| f.replace.as_ref())
            .map(|r| r.read(cx).text.clone())
            .unwrap_or_default()
    }

    fn replace_one(&mut self, cx: &mut Context<Self>) {
        let q = self.query(cx);
        let r = self.replace_text(cx);
        let (a, b) = self.sel.range();
        if !q.is_empty() && a.line == b.line && self.text_between(a, b) == q {
            self.edit(a, b, &r, None, Kind::Other, cx);
        }
        self.find_step(true, cx);
    }

    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let q = self.query(cx);
        if q.is_empty() {
            return;
        }
        let r = self.replace_text(cx);
        let all = self.lines.join("\n");
        let new = all.replace(&q, &r);
        if new != all {
            let end = self.end_pos();
            self.edit(pos(0, 0), end, &new, Some(Sel::caret(pos(0, 0))), Kind::Other, cx);
        }
    }

    pub fn find_next(&mut self, cx: &mut Context<Self>) {
        self.find_step(true, cx)
    }
    pub fn find_prev(&mut self, cx: &mut Context<Self>) {
        self.find_step(false, cx)
    }

    pub fn use_selection_for_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (a, b) = self.sel.range();
        let word = if self.sel.is_empty() {
            let s = self.word_left(self.next_char(a));
            let e = self.word_right(s);
            (s.line == e.line).then(|| self.text_between(s, e))
        } else if a.line == b.line {
            Some(self.text_between(a, b))
        } else {
            None
        };
        if let Some(w) = word {
            let had = self.find.is_some();
            self.open_find(false, window, cx);
            let q = self.find.as_ref().unwrap().query.clone();
            q.update(cx, |i, cx| i.set_text(&w, cx));
            if !had {
                window.focus(&self.focus, cx);
            }
            self.recount(cx);
        }
    }

    pub fn goto_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let l = line.saturating_sub(1).min(self.lines.len() - 1);
        self.sel = Sel::caret(pos(l, 0));
        // Center it.
        let lh = Self::lh(cx);
        let (row, _) = self.pos_to_row(self.sel.head);
        self.scroll_y = (lh * row as f32 - self.viewport_h / 2.).clamp(px(0.), self.max_scroll(cx));
        cx.notify();
    }

    // -- mouse --------------------------------------------------------------

    fn hit(&self, p: gpui::Point<Pixels>, cx: &App) -> Pos {
        let Some(bounds) = self.last_bounds else { return self.sel.head };
        let lh = Self::lh(cx);
        let y = p.y - bounds.top() + self.scroll_y;
        let row = (y / lh).floor().max(0.) as usize;
        if row >= self.total_rows() {
            return self.end_pos();
        }
        let x = (p.x - bounds.left() - self.text_x).max(px(0.));
        let col = (x / self.cell_w).round() as usize;
        self.row_col_to_pos(row, col)
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let p = self.hit(ev.position, cx);
        match ev.click_count {
            2 => {
                let s = self.word_left(self.next_char(p));
                let e = self.word_right(s);
                self.sel = Sel { anchor: s, head: e };
            }
            3 => {
                let head = if p.line + 1 < self.lines.len() { pos(p.line + 1, 0) } else { self.end_pos() };
                self.sel = Sel { anchor: pos(p.line, 0), head };
            }
            _ if ev.modifiers.shift => self.sel.head = p,
            _ => self.sel = Sel::caret(p),
        }
        self.goal = None;
        self.selecting = ev.click_count == 1;
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && ev.pressed_button == Some(MouseButton::Left) {
            self.sel.head = self.hit(ev.position, cx);
            cx.notify();
        } else {
            self.selecting = false;
        }
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dy = ev.delta.pixel_delta(Self::lh(cx)).y;
        self.scroll_y = (self.scroll_y - dy).clamp(px(0.), self.max_scroll(cx));
        cx.notify();
    }

    // -- spike measurement hooks -------------------------------------------

    /// Runs after the frame with this editor's paint was handed to the GPU.
    fn after_present(&mut self, window_handle: gpui::AnyWindowHandle, cx: &mut Context<Self>) {
        if self.bench.done {
            return;
        }
        if !self.bench.first_frame_done {
            self.bench.first_frame_done = true;
            sc::metric("first_frame_ms", sc::since_start().as_millis());
            if self.cfg_exit_after_first_frame {
                self.bench.done = true;
                return cx.quit();
            }
            if self.cfg_autotype {
                self.sel = Sel::caret(pos(sc::AUTOTYPE_LINE.min(self.lines.len() - 1), 0));
                self.reveal(cx);
                cx.notify();
            }
        }
        if self.cfg_autoscroll {
            self.bench.scroll.tick();
            if let Some(t0) = self.bench.frame_start.take() {
                self.bench.scroll_work.push(t0.elapsed());
            }
            if self.scroll_y >= self.max_scroll(cx) {
                self.bench.scroll.report("scroll");
                self.bench.scroll_work.report("scroll_work");
                sc::metric("scroll_visual_rows", self.total_rows());
                self.bench.done = true;
                return cx.quit();
            }
            let entity = cx.entity();
            window_handle
                .update(cx, |_, window, _| {
                    window.on_next_frame(move |_, cx| {
                        entity.update(cx, |e, cx| {
                            e.bench.frame_start = Some(Instant::now());
                            e.scroll_y = (e.scroll_y + Self::lh(cx) * sc::AUTOSCROLL_LINES_PER_FRAME as f32)
                                .min(e.max_scroll(cx));
                            cx.notify();
                        });
                    });
                })
                .ok();
        }
        if self.cfg_autotype {
            if let Some(t0) = self.bench.pending_key.take() {
                self.bench.typing.push(t0.elapsed());
                self.bench.key_outstanding = false;
            }
            if self.bench.key_outstanding {
                return;
            }
            if self.bench.keys_sent >= sc::AUTOTYPE_KEYS {
                self.bench.typing.report("type");
                self.bench.done = true;
                return cx.quit();
            }
            const CHARS: [&str; 26] = [
                "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p",
                "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
            ];
            let entity = cx.entity();
            let ch = CHARS[self.bench.keys_sent % CHARS.len()];
            self.bench.keys_sent += 1;
            self.bench.key_outstanding = true;
            window_handle
                .update(cx, |_, window, _| {
                    window.on_next_frame(move |window, cx| {
                        entity.update(cx, |e, _| e.bench.pending_key = Some(Instant::now()));
                        window.dispatch_keystroke(gpui::Keystroke::parse(ch).unwrap(), cx);
                    });
                })
                .ok();
        }
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Minimal platform text input: committed text goes in at the selection.
/// IME composition is inserted directly (no marked-text display).
impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let h = self.sel.head;
        let u = self.lines[h.line][..h.col].encode_utf16().count();
        Some(UTF16Selection { range: u..u, reversed: false })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if !text.is_empty() {
            self.insert_text(text, cx);
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(r, text, window, cx);
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let lh = Self::lh(cx);
        let (row, col) = self.pos_to_row(self.sel.head);
        let y = bounds.top() + lh * row as f32 - self.scroll_y;
        Some(Bounds::new(point(bounds.left() + self.text_x + self.cell_w * col as f32, y), size(px(2.), lh)))
    }
    fn character_index_for_point(&mut self, _: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

// ---------------------------------------------------------------------------
// Element: paints only the visible visual rows
// ---------------------------------------------------------------------------

struct EditorElement {
    editor: Entity<Editor>,
}

struct Row {
    y: Pixels,
    line: usize,
    bytes: Range<usize>,
    gutter: Option<ShapedLine>,
    text: ShapedLine,
}

struct Prepaint {
    rows: Vec<Row>,
    gutter_w: Pixels,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let ui = Ui::get(cx);
        let fs = ui.z(13.);
        let code_font = font(CODE_FONT);
        let ts = window.text_system().clone();
        let cell_w = ts
            .advance(ts.resolve_font(&code_font), fs, 'm')
            .map(|s| s.width)
            .unwrap_or(px(8.));
        let lh = Editor::lh(cx);
        // Gutter: 8px | right-aligned numbers | 8px, then an 8px gap before text.
        let digits = self.editor.read(cx).lines.len().to_string().len().max(3);
        let gutter_w = ui.z(8.) * 2. + cell_w * digits as f32;
        let text_x = gutter_w + ui.z(8.);
        let cols = ((bounds.size.width - text_x - ui.z(8.)) / cell_w).floor().max(8.) as usize;

        self.editor.update(cx, |e, _| {
            e.viewport_h = bounds.size.height;
            e.cell_w = cell_w;
            e.text_x = text_x;
            if e.wrap_cols != cols {
                // Keep the top line anchored across re-wraps (resize, zoom).
                let top_row = (e.scroll_y / lh).floor() as usize;
                let top_line = if e.row_start.is_empty() { 0 } else { e.row_to_line(top_row.min(e.total_rows().saturating_sub(1))).0 };
                e.wrap_cols = cols;
                if e.wrap {
                    e.rebuild_rows();
                    e.scroll_y = lh * e.row_start[top_line] as f32;
                }
            }
            e.scroll_y = e.scroll_y.min((lh * e.total_rows() as f32 - bounds.size.height).max(px(0.)));
        });

        let e = self.editor.read(cx);
        let first = (e.scroll_y / lh).floor() as usize;
        let offset = e.scroll_y - lh * first as f32;
        let count = (bounds.size.height / lh).ceil() as usize + 1;
        let last = (first + count).min(e.total_rows());
        let run = |len: usize, color: u32| TextRun {
            len,
            font: code_font.clone(),
            color: hsla(color),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let mut rows = Vec::with_capacity(last.saturating_sub(first));
        for (i, row) in (first..last).enumerate() {
            let (l, k) = e.row_to_line(row);
            let r = e.row_bytes(l, k);
            let text = &e.lines[l][r.clone()];
            let mut runs = Vec::new();
            let mut at = r.start;
            for (sr, color) in &e.spans[l] {
                let (s, t) = (sr.start.max(r.start).max(at), sr.end.min(r.end));
                if s >= t || !e.lines[l].is_char_boundary(s) || !e.lines[l].is_char_boundary(t) {
                    continue;
                }
                if s > at {
                    runs.push(run(s - at, qur_dark::TEXT));
                }
                runs.push(run(t - s, *color));
                at = t;
            }
            if at < r.end {
                runs.push(run(r.end - at, qur_dark::TEXT));
            }
            let shaped = ts.shape_line(SharedString::from(text.to_string()), fs, &runs, None);
            let gutter = (k == 0).then(|| {
                let num = SharedString::from(format!("{:>w$}", l + 1, w = digits));
                ts.shape_line(num.clone(), fs, &[run(num.len(), qur_dark::TEXT_MUTED)], None)
            });
            rows.push(Row { y: bounds.top() + lh * i as f32 - offset, line: l, bytes: r, gutter, text: shaped });
        }
        Prepaint { rows, gutter_w }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        pre: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let ui = Ui::get(cx);
        let lh = Editor::lh(cx);
        let bg_fill = fill(bounds, ui.bg(qur_dark::BG));
        // Pass 1 (editor borrowed): quads under the text, and the caret.
        let (focus, tx, under, caret) = {
            let e = self.editor.read(cx);
            let focus = e.focus.clone();
            let focused = focus.is_focused(window);
            let cell_w = e.cell_w;
            let tx = bounds.left() + e.text_x;
            let (sa, sb) = e.sel.range();
            let head = e.sel.head;
            let query = e.query(cx);
            let lines = &e.lines;
            let x_of = |line: usize, row_start: usize, col: usize| -> Pixels {
                tx + cell_w * char_count(&lines[line][row_start..col]) as f32
            };
            let mut under = Vec::new();
            let mut caret = None;
            let mut match_cache: Option<(usize, Vec<usize>)> = None;
            for row in &pre.rows {
                let l = row.line;
                let r = &row.bytes;
                let row_end_incl = r.end == lines[l].len();
                if e.sel.is_empty() && head.line == l {
                    under.push(fill(
                        Bounds::new(point(bounds.left(), row.y), size(bounds.size.width, lh)),
                        ui.bg(qur_dark::CURRENT_LINE),
                    ));
                }
                // Find matches (visible lines only).
                if !query.is_empty() {
                    if match_cache.as_ref().map(|(ml, _)| *ml) != Some(l) {
                        match_cache = Some((l, lines[l].match_indices(query.as_str()).map(|(i, _)| i).collect()));
                    }
                    for &m in &match_cache.as_ref().unwrap().1 {
                        let (s, t) = (m.max(r.start), (m + query.len()).min(r.end));
                        if s < t {
                            under.push(fill(
                                Bounds::from_corners(point(x_of(l, r.start, s), row.y), point(x_of(l, r.start, t), row.y + lh)),
                                hsla(0x3A3A2A),
                            ));
                        }
                    }
                }
                // Selection.
                if !e.sel.is_empty() && l >= sa.line && l <= sb.line {
                    let s = if l == sa.line { sa.col.max(r.start) } else { r.start };
                    let t = if l == sb.line { sb.col.min(r.end) } else { r.end };
                    if s <= t && !(l == sb.line && sb.col < r.start) && !(l == sa.line && sa.col > r.end) {
                        let mut x1 = x_of(l, r.start, t);
                        if l < sb.line && row_end_incl {
                            x1 += cell_w; // newline
                        }
                        under.push(fill(
                            Bounds::from_corners(point(x_of(l, r.start, s), row.y), point(x1, row.y + lh)),
                            hsla(qur_dark::SELECTION),
                        ));
                    }
                }
                // Caret: in this row if head.col is inside it (the end of a wrapped
                // row belongs to the next row, except at the end of the line).
                if focused && head.line == l && head.col >= r.start && (head.col < r.end || row_end_incl) {
                    caret = Some(fill(
                        Bounds::new(point(x_of(l, r.start, head.col), row.y), size(px(2.), lh)),
                        hsla(qur_dark::TEXT),
                    ));
                }
            }
            (focus, tx, under, caret)
        };
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        // Pass 2: paint.
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(bg_fill);
            for q in under {
                window.paint_quad(q);
            }
            for row in &pre.rows {
                if let Some(g) = &row.gutter {
                    g.paint(point(bounds.left() + ui.z(8.), row.y), lh, gpui::TextAlign::Left, None, window, cx).ok();
                }
                row.text.paint(point(tx, row.y), lh, gpui::TextAlign::Left, None, window, cx).ok();
            }
            if let Some(c) = caret {
                window.paint_quad(c);
            }
            let _ = pre.gutter_w;
        });
        let editor = self.editor.clone();
        let handle = window.window_handle();
        editor.update(cx, |e, _| e.last_bounds = Some(bounds));
        cx.defer(move |cx| editor.update(cx, |e, cx| e.after_present(handle, cx)));
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let find_bar = self.find.as_ref().map(|f| {
            let field = |input: &Entity<TextInput>| {
                div()
                    .flex_1()
                    .px(ui.z(6.))
                    .py(ui.z(2.))
                    .border_1()
                    .border_color(hsla(qur_dark::BORDER))
                    .rounded(ui.z(4.))
                    .child(input.clone())
            };
            let q = self.query(cx);
            let count = if q.is_empty() { String::new() } else { format!("{} matches", f.count) };
            div()
                .flex()
                .flex_col()
                .gap(ui.z(4.))
                .px(ui.z(8.))
                .py(ui.z(6.))
                .bg(ui.bg(qur_dark::SIDEBAR))
                .border_b_1()
                .border_color(hsla(qur_dark::BORDER))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(ui.z(8.))
                        .child(field(&f.query))
                        .child(div().text_size(ui.z(11.)).text_color(hsla(qur_dark::TEXT_MUTED)).child(count)),
                )
                .children(f.replace.as_ref().map(|r| {
                    div()
                        .flex()
                        .items_center()
                        .gap(ui.z(8.))
                        .child(field(r))
                        .child(div().text_size(ui.z(11.)).text_color(hsla(qur_dark::TEXT_MUTED)).child("↩ one · ⌘↩ all"))
                }))
        });
        div()
            .key_context("Editor")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left_a))
            .on_action(cx.listener(Self::word_right_a))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end_a))
            .on_action(cx.listener(Self::select_line_start))
            .on_action(cx.listener(Self::select_line_end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::select_page_up))
            .on_action(cx.listener(Self::select_page_down))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::delete_line))
            .on_action(cx.listener(Self::move_line_up))
            .on_action(cx.listener(Self::move_line_down))
            .on_action(cx.listener(Self::copy_line_up))
            .on_action(cx.listener(Self::copy_line_down))
            .on_action(cx.listener(Self::insert_line_below))
            .on_action(cx.listener(Self::insert_line_above))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::shift_tab))
            .on_action(cx.listener(Self::toggle_comment))
            .on_action(cx.listener(Self::select_line))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::newline))
            .children(find_bar)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_scroll(cx.listener(Self::on_scroll))
                    .child(EditorElement { editor: cx.entity() }),
            )
    }
}
