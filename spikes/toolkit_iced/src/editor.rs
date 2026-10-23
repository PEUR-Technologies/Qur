//! Editor model (selection, undo, soft wrap, find, line ops) and the custom
//! virtualized code view that shapes and draws only visible visual rows.

use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{self, Paragraph as _};
use iced::advanced::widget::{tree, Tree, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::alignment::Vertical;
use iced::font::Family;
use iced::keyboard;
use iced::mouse::{self, ScrollDelta};
use iced::{Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size};
use spike_common::{qur_dark, Span};

use crate::commands::KeyPress;
use crate::{rgb, rgba, Message, Shared};

pub const LINE_H: f32 = 20.0;
pub const FONT_SIZE: f32 = 13.0;
/// Padding on each side of the line-number column, and before the text.
const GUTTER_PAD: f32 = 8.0;
const MONO: Font = Font { family: Family::Name("Menlo"), ..Font::MONOSPACE };
/// Typing within this window after the previous keystroke joins its undo group.
const UNDO_GROUP_MS: u64 = 1000;

type LineSpans = Vec<(Range<usize>, u32)>;

pub fn mono_text<C>(content: C) -> text::Text<C, Font> {
    text::Text {
        content,
        bounds: Size::new(f32::INFINITY, LINE_H),
        size: Pixels(FONT_SIZE),
        line_height: text::LineHeight::Absolute(Pixels(LINE_H)),
        font: MONO,
        align_x: text::Alignment::Default,
        align_y: Vertical::Top,
        shaping: text::Shaping::Advanced,
        wrapping: text::Wrapping::None,
    }
}

/// Splits whole-file highlight spans into per-line, line-relative spans.
pub fn split_spans(src: &str, spans: &[Span]) -> Vec<LineSpans> {
    let mut starts = vec![0usize];
    starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
    let mut out: Vec<LineSpans> = vec![Vec::new(); starts.len()];
    let mut line = 0;
    for s in spans {
        let color = qur_dark::syntax(s.kind);
        let mut start = s.range.start;
        while line + 1 < starts.len() && starts[line + 1] <= start {
            line += 1;
        }
        // A span may cross lines (block comments, template strings).
        let mut l = line;
        while start < s.range.end && l < starts.len() {
            let line_end = starts.get(l + 1).map_or(src.len(), |&n| n - 1);
            let end = s.range.end.min(line_end);
            if end > start {
                out[l].push((start - starts[l]..end - starts[l], color));
            }
            l += 1;
            start = starts.get(l).copied().unwrap_or(usize::MAX);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Pos {
    pub line: usize,
    /// Byte column.
    pub col: usize,
}

impl Pos {
    pub fn new(line: usize, col: usize) -> Self {
        Pos { line, col }
    }
}

/// Position just after `text` inserted at `a`.
fn end_after(a: Pos, text: &str) -> Pos {
    match text.rfind('\n') {
        Some(i) => Pos::new(a.line + text.matches('\n').count(), text.len() - i - 1),
        None => Pos::new(a.line, a.col + text.len()),
    }
}

struct Edit {
    at: Pos,
    removed: String,
    inserted: String,
}

struct Group {
    edits: Vec<Edit>,
    before: (Pos, Pos),
    after: (Pos, Pos),
    typing: bool,
    at: Instant,
}

#[derive(Default)]
struct Wrap {
    cols: usize,
    on: bool,
    edit_gen: u64,
    /// First visual row of each line; one extra entry = total rows.
    row_start: Vec<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' || c == '$' {
        Class::Word
    } else {
        Class::Punct
    }
}

pub struct Doc {
    pub lines: Vec<String>,
    spans: Vec<LineSpans>,
    /// Unique id per (line content, spans); paragraph cache key.
    vers: Vec<u64>,
    next_ver: u64,
    pub anchor: Pos,
    pub head: Pos,
    /// Visual column kept across up/down moves.
    goal_x: Option<usize>,
    pub scroll_y: f32,
    /// Bumped on every text change.
    pub edit_gen: u64,
    pub dirty: bool,
    pub soft_wrap: bool,
    pub indent_unit: &'static str,
    pub comment: &'static str,
    wrap: RefCell<Wrap>,
    undo: Vec<Group>,
    redo: Vec<Group>,
}

impl Doc {
    pub fn new(src: &str, spans: &[Span]) -> Self {
        let lines: Vec<String> = src.split('\n').map(str::to_owned).collect();
        let spans = split_spans(src, spans);
        let n = lines.len();
        Doc {
            lines,
            spans,
            vers: (0..n as u64).collect(),
            next_ver: n as u64,
            anchor: Pos::default(),
            head: Pos::default(),
            goal_x: None,
            scroll_y: 0.0,
            edit_gen: 0,
            dirty: false,
            soft_wrap: true,
            indent_unit: "  ",
            comment: "//",
            wrap: RefCell::default(),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn text_range(&self, a: Pos, b: Pos) -> String {
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

    pub fn sel(&self) -> (Pos, Pos) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.head
    }

    pub fn selected_text(&self) -> String {
        let (a, b) = self.sel();
        self.text_range(a, b)
    }

    fn end(&self) -> Pos {
        let l = self.lines.len() - 1;
        Pos::new(l, self.lines[l].len())
    }

    fn bump(&mut self, line: usize) {
        self.vers[line] = self.next_ver;
        self.next_ver += 1;
    }

    pub fn set_spans(&mut self, lines: Vec<LineSpans>) {
        if lines.len() != self.lines.len() {
            return; // stale (line count changed); a newer pass is queued
        }
        for (i, s) in lines.into_iter().enumerate() {
            if self.spans[i] != s {
                self.spans[i] = s;
                self.bump(i);
            }
        }
    }

    // ----- soft wrap ------------------------------------------------------

    fn rows_of(line: &str, cols: usize, on: bool) -> usize {
        if !on {
            return 1;
        }
        let n = if line.is_ascii() { line.len() } else { line.chars().count() };
        n.div_ceil(cols).max(1)
    }

    fn wrap(&self, cols: usize) -> Ref<'_, Wrap> {
        let cols = cols.max(8);
        {
            let w = self.wrap.borrow();
            if w.cols == cols && w.on == self.soft_wrap && w.edit_gen == self.edit_gen && !w.row_start.is_empty() {
                return w;
            }
        }
        {
            let mut w = self.wrap.borrow_mut();
            w.cols = cols;
            w.on = self.soft_wrap;
            w.edit_gen = self.edit_gen;
            w.row_start.clear();
            let mut r = 0;
            for l in &self.lines {
                w.row_start.push(r);
                r += Self::rows_of(l, cols, self.soft_wrap);
            }
            w.row_start.push(r);
        }
        self.wrap.borrow()
    }

    pub fn total_rows(&self, cols: usize) -> usize {
        *self.wrap(cols).row_start.last().unwrap()
    }

    /// (first visual row of `line`, rows in `line`)
    fn line_rows(&self, line: usize, cols: usize) -> (usize, usize) {
        let w = self.wrap(cols);
        (w.row_start[line], w.row_start[line + 1] - w.row_start[line])
    }

    fn line_at_row(&self, row: usize, cols: usize) -> usize {
        let w = self.wrap(cols);
        w.row_start.partition_point(|&s| s <= row).saturating_sub(1).min(self.lines.len() - 1)
    }

    fn char_col(&self, p: Pos) -> usize {
        let l = &self.lines[p.line];
        if l.is_ascii() { p.col } else { l[..p.col].chars().count() }
    }

    fn byte_col(&self, line: usize, chars: usize) -> usize {
        let l = &self.lines[line];
        if l.is_ascii() { chars.min(l.len()) } else { l.char_indices().nth(chars).map_or(l.len(), |(i, _)| i) }
    }

    /// Visual (row, x) of a position.
    pub fn visual(&self, p: Pos, cols: usize) -> (usize, usize) {
        let cols = cols.max(8);
        let (first, n) = self.line_rows(p.line, cols);
        let cc = self.char_col(p);
        if !self.soft_wrap {
            return (first, cc);
        }
        let r = (cc / cols).min(n - 1);
        (first + r, cc - r * cols)
    }

    pub fn pos_at(&self, row: usize, x: usize, cols: usize) -> Pos {
        let cols = cols.max(8);
        let line = self.line_at_row(row, cols);
        let (first, n) = self.line_rows(line, cols);
        let r = row.saturating_sub(first).min(n - 1);
        let cc = if self.soft_wrap { r * cols + x.min(if r + 1 < n { cols - 1 } else { cols }) } else { x };
        Pos::new(line, self.byte_col(line, cc))
    }

    pub fn max_scroll(&self, view_h: f32, cols: usize) -> f32 {
        (self.total_rows(cols) as f32 * LINE_H - view_h).max(0.0)
    }

    pub fn reveal_caret(&mut self, view_h: f32, cols: usize) {
        let y = self.visual(self.head, cols).0 as f32 * LINE_H;
        if y < self.scroll_y {
            self.scroll_y = y;
        } else if y + LINE_H > self.scroll_y + view_h {
            self.scroll_y = (y + LINE_H - view_h).max(0.0);
        }
    }

    // ----- primitive edit -------------------------------------------------

    /// Replace `a..b` with `text`, without touching undo. Returns end of insert.
    fn replace_raw(&mut self, a: Pos, b: Pos, text: &str) -> Pos {
        let prefix = self.lines[a.line][..a.col].to_string();
        let suffix = self.lines[b.line][b.col..].to_string();
        let joined = format!("{prefix}{text}{suffix}");
        let new: Vec<String> = joined.split('\n').map(str::to_owned).collect();
        let single = a.line == b.line && !text.contains('\n');

        let new_spans: Vec<LineSpans> = if single {
            let mut s = std::mem::take(&mut self.spans[a.line]);
            shift_spans(&mut s, a.col, b.col, text.len());
            vec![s]
        } else {
            // Keep colors before the edit; the rest waits for the re-highlight.
            let mut first: LineSpans = self.spans[a.line].iter().filter(|(r, _)| r.end <= a.col).cloned().collect();
            first.retain(|(r, _)| r.end <= prefix.len());
            let mut v = vec![first];
            v.resize(new.len(), Vec::new());
            v
        };
        let n_new = new.len();
        self.lines.splice(a.line..=b.line, new);
        self.spans.splice(a.line..=b.line, new_spans);
        self.vers.splice(a.line..=b.line, std::iter::repeat_n(0, n_new));
        for l in a.line..a.line + n_new {
            self.bump(l);
        }
        self.edit_gen += 1;
        self.dirty = true;
        end_after(a, text)
    }

    /// Recorded edit; `typing` edits can merge into the previous undo group.
    fn edit(&mut self, a: Pos, b: Pos, text: &str, typing: bool) {
        let before = (self.anchor, self.head);
        let removed = self.text_range(a, b);
        let end = self.replace_raw(a, b, text);
        self.anchor = end;
        self.head = end;
        self.goal_x = None;
        self.redo.clear();
        let e = Edit { at: a, removed, inserted: text.to_string() };
        let now = Instant::now();
        if let Some(g) = self.undo.last_mut().filter(|g| {
            typing
                && g.typing
                && now - g.at < Duration::from_millis(UNDO_GROUP_MS)
                && g.edits.last().is_some_and(|l| end_after(l.at, &l.inserted) == a)
        }) {
            g.edits.push(e);
            g.after = (end, end);
            g.at = now;
        } else {
            self.undo.push(Group { edits: vec![e], before, after: (end, end), typing, at: now });
        }
    }

    /// Replace whole lines `l0..=l1` with `new_text` (one undo step) and set the selection.
    fn edit_lines(&mut self, l0: usize, l1: usize, new_text: &str, sel: (Pos, Pos)) {
        let a = Pos::new(l0, 0);
        let b = Pos::new(l1, self.lines[l1].len());
        self.edit(a, b, new_text, false);
        if let Some(g) = self.undo.last_mut() {
            g.after = sel;
        }
        self.anchor = sel.0;
        self.head = sel.1;
    }

    pub fn undo(&mut self) {
        let Some(g) = self.undo.pop() else { return };
        for e in g.edits.iter().rev() {
            self.replace_raw(e.at, end_after(e.at, &e.inserted), &e.removed);
        }
        (self.anchor, self.head) = g.before;
        self.redo.push(g);
    }

    pub fn redo(&mut self) {
        let Some(g) = self.redo.pop() else { return };
        for e in &g.edits {
            self.replace_raw(e.at, end_after(e.at, &e.removed), &e.inserted);
        }
        (self.anchor, self.head) = g.after;
        self.undo.push(g);
    }

    // ----- text input -------------------------------------------------------

    pub fn insert(&mut self, s: &str) {
        let (a, b) = self.sel();
        let typing = a == b && !s.contains('\n') && s.chars().count() == 1;
        self.edit(a, b, s, typing);
    }

    fn indent_of(&self, line: usize) -> &str {
        let l = &self.lines[line];
        &l[..l.len() - l.trim_start().len()]
    }

    pub fn newline(&mut self) {
        let indent = self.indent_of(self.head.line).to_string();
        self.insert(&format!("\n{indent}"));
    }

    // ----- motion -----------------------------------------------------------

    fn set_head(&mut self, p: Pos, extend: bool) {
        self.head = p;
        if !extend {
            self.anchor = p;
        }
    }

    fn left_of(&self, p: Pos) -> Pos {
        if p.col > 0 {
            let c = self.lines[p.line][..p.col].char_indices().last().map_or(0, |(i, _)| i);
            Pos::new(p.line, c)
        } else if p.line > 0 {
            Pos::new(p.line - 1, self.lines[p.line - 1].len())
        } else {
            p
        }
    }

    fn right_of(&self, p: Pos) -> Pos {
        match self.lines[p.line][p.col..].chars().next() {
            Some(ch) => Pos::new(p.line, p.col + ch.len_utf8()),
            None if p.line + 1 < self.lines.len() => Pos::new(p.line + 1, 0),
            None => p,
        }
    }

    fn word_left(&self, p: Pos) -> Pos {
        if p.col == 0 {
            return self.left_of(p);
        }
        let chars: Vec<(usize, char)> = self.lines[p.line][..p.col].char_indices().collect();
        let mut i = chars.len();
        while i > 0 && class(chars[i - 1].1) == Class::Space {
            i -= 1;
        }
        if i > 0 {
            let k = class(chars[i - 1].1);
            while i > 0 && class(chars[i - 1].1) == k {
                i -= 1;
            }
        }
        Pos::new(p.line, chars.get(i).map_or(0, |c| c.0))
    }

    fn word_right(&self, p: Pos) -> Pos {
        let line = &self.lines[p.line];
        if p.col >= line.len() {
            return self.right_of(p);
        }
        let rest: Vec<(usize, char)> = line[p.col..].char_indices().collect();
        let mut i = 0;
        while i < rest.len() && class(rest[i].1) == Class::Space {
            i += 1;
        }
        if i < rest.len() {
            let k = class(rest[i].1);
            while i < rest.len() && class(rest[i].1) == k {
                i += 1;
            }
        }
        Pos::new(p.line, rest.get(i).map_or(line.len(), |c| p.col + c.0))
    }

    fn vertical(&mut self, rows: isize, extend: bool, cols: usize) {
        if !extend && self.has_selection() && rows.abs() == 1 {
            let (a, b) = self.sel();
            self.head = if rows < 0 { a } else { b };
        }
        let (row, x) = self.visual(self.head, cols);
        let goal = *self.goal_x.get_or_insert(x);
        let total = self.total_rows(cols) as isize;
        let target = row as isize + rows;
        let p = if target < 0 {
            Pos::default()
        } else if target >= total {
            self.end()
        } else {
            self.pos_at(target as usize, goal, cols)
        };
        self.set_head(p, extend);
    }

    // ----- line operations ---------------------------------------------------

    fn sel_lines(&self) -> (usize, usize) {
        let (a, b) = self.sel();
        // A selection ending at column 0 doesn't include that line.
        let l1 = if b.col == 0 && b.line > a.line { b.line - 1 } else { b.line };
        (a.line, l1)
    }

    fn block(&self, l0: usize, l1: usize) -> String {
        self.lines[l0..=l1].join("\n")
    }

    fn shift_sel(&self, dl: isize, map: impl Fn(Pos) -> Pos) -> (Pos, Pos) {
        let f = |p: Pos| map(Pos::new((p.line as isize + dl) as usize, p.col));
        (f(self.anchor), f(self.head))
    }

    /// Apply `f` to each line in the selection; columns shift by each line's delta.
    fn map_lines(&mut self, f: impl Fn(&str) -> String) {
        let (l0, l1) = self.sel_lines();
        let new: Vec<String> = self.lines[l0..=l1].iter().map(|l| f(l)).collect();
        let delta = |p: Pos| -> Pos {
            if p.line < l0 || p.line > l1 {
                return p;
            }
            let d = new[p.line - l0].len() as isize - self.lines[p.line].len() as isize;
            Pos::new(p.line, (p.col as isize + d).clamp(0, new[p.line - l0].len() as isize) as usize)
        };
        let sel = (delta(self.anchor), delta(self.head));
        self.edit_lines(l0, l1, &new.join("\n"), sel);
    }

    fn indent(&mut self) {
        let unit = self.indent_unit;
        self.map_lines(|l| if l.is_empty() { String::new() } else { format!("{unit}{l}") });
    }

    fn outdent(&mut self) {
        let n = self.indent_unit.len();
        self.map_lines(|l| {
            let strip = l.len() - l.trim_start_matches(' ').len();
            l[strip.min(n)..].to_string()
        });
    }

    fn toggle_comment(&mut self) {
        let (l0, l1) = self.sel_lines();
        let c = self.comment;
        let body = |l: &str| l.trim_start().to_string();
        let lines = &self.lines[l0..=l1];
        let all = lines.iter().filter(|l| !l.trim().is_empty()).all(|l| body(l).starts_with(c));
        let min_indent = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        self.map_lines(|l| {
            if l.trim().is_empty() {
                return l.to_string();
            }
            let ind = l.len() - l.trim_start().len();
            if all {
                let rest = &l[ind + c.len()..];
                format!("{}{}", &l[..ind], rest.strip_prefix(' ').unwrap_or(rest))
            } else {
                format!("{}{c} {}", &l[..min_indent], &l[min_indent..])
            }
        });
    }

    fn move_lines(&mut self, up: bool) {
        let (l0, l1) = self.sel_lines();
        if up && l0 > 0 {
            let text = format!("{}\n{}", self.block(l0, l1), self.lines[l0 - 1]);
            let sel = self.shift_sel(-1, |p| p);
            self.edit_lines(l0 - 1, l1, &text, sel);
        } else if !up && l1 + 1 < self.lines.len() {
            let text = format!("{}\n{}", self.lines[l1 + 1], self.block(l0, l1));
            let sel = self.shift_sel(1, |p| p);
            self.edit_lines(l0, l1 + 1, &text, sel);
        }
    }

    fn copy_lines(&mut self, up: bool) {
        let (l0, l1) = self.sel_lines();
        let block = self.block(l0, l1);
        let text = format!("{block}\n{block}");
        let sel = if up { (self.anchor, self.head) } else { self.shift_sel((l1 - l0 + 1) as isize, |p| p) };
        self.edit_lines(l0, l1, &text, sel);
    }

    fn delete_lines(&mut self) {
        let (l0, l1) = self.sel_lines();
        let (a, b) = if l1 + 1 < self.lines.len() {
            (Pos::new(l0, 0), Pos::new(l1 + 1, 0))
        } else if l0 > 0 {
            (Pos::new(l0 - 1, self.lines[l0 - 1].len()), Pos::new(l1, self.lines[l1].len()))
        } else {
            (Pos::new(l0, 0), Pos::new(l1, self.lines[l1].len()))
        };
        self.edit(a, b, "", false);
        let line = self.head.line;
        let col = self.indent_of(line).len();
        self.set_head(Pos::new(line, col), false);
    }

    fn insert_line(&mut self, below: bool) {
        let l = self.head.line;
        let indent = self.indent_of(l).to_string();
        if below {
            let at = Pos::new(l, self.lines[l].len());
            self.edit(at, at, &format!("\n{indent}"), false);
        } else {
            let at = Pos::new(l, 0);
            self.edit(at, at, &format!("{indent}\n"), false);
            self.set_head(Pos::new(l, indent.len()), false);
        }
    }

    fn select_line(&mut self) {
        let (a, b) = self.sel();
        let full = a.col == 0 && b.col == 0 && b.line > a.line;
        let start = if full { a } else { Pos::new(a.line, 0) };
        let from = if full { b.line } else { b.line + 1 };
        let end = if from < self.lines.len() { Pos::new(from, 0) } else { self.end() };
        self.anchor = start;
        self.head = end;
    }

    fn delete_sel_or(&mut self, target: impl Fn(&Self) -> Pos) {
        let (a, b) = if self.has_selection() {
            self.sel()
        } else {
            let t = target(self);
            (t.min(self.head), t.max(self.head))
        };
        if a != b {
            self.edit(a, b, "", false);
        }
    }

    /// Run an editor command by id. Returns false if it is not an editor command.
    pub fn run(&mut self, id: &str, view_h: f32, cols: usize) -> bool {
        let page = ((view_h / LINE_H) as isize - 1).max(1);
        let keep_goal = matches!(id, "up" | "down" | "select_up" | "select_down" | "page_up" | "page_down" | "select_page_up" | "select_page_down");
        match id {
            "left" | "select_left" => {
                let ext = id.starts_with("select");
                let p = if !ext && self.has_selection() { self.sel().0 } else { self.left_of(self.head) };
                self.set_head(p, ext);
            }
            "right" | "select_right" => {
                let ext = id.starts_with("select");
                let p = if !ext && self.has_selection() { self.sel().1 } else { self.right_of(self.head) };
                self.set_head(p, ext);
            }
            "up" | "select_up" => self.vertical(-1, id.starts_with("select"), cols),
            "down" | "select_down" => self.vertical(1, id.starts_with("select"), cols),
            "page_up" | "select_page_up" => self.vertical(-page, id.starts_with("select"), cols),
            "page_down" | "select_page_down" => self.vertical(page, id.starts_with("select"), cols),
            "word_left" | "select_word_left" => self.set_head(self.word_left(self.head), id.starts_with("select")),
            "word_right" | "select_word_right" => self.set_head(self.word_right(self.head), id.starts_with("select")),
            "line_start" | "select_line_start" => {
                // Smart home: first non-blank, then column 0.
                let first = self.indent_of(self.head.line).len();
                let col = if self.head.col == first { 0 } else { first };
                self.set_head(Pos::new(self.head.line, col), id.starts_with("select"));
            }
            "line_end" | "select_line_end" => {
                let l = self.head.line;
                self.set_head(Pos::new(l, self.lines[l].len()), id.starts_with("select"));
            }
            "doc_start" | "select_doc_start" => self.set_head(Pos::default(), id.starts_with("select")),
            "doc_end" | "select_doc_end" => self.set_head(self.end(), id.starts_with("select")),
            "select_all" => {
                self.anchor = Pos::default();
                self.head = self.end();
            }
            "select_line" => self.select_line(),
            "escape" => self.anchor = self.head,
            "newline" => self.newline(),
            "backspace" => self.delete_sel_or(|d| d.left_of(d.head)),
            "delete" => self.delete_sel_or(|d| d.right_of(d.head)),
            "delete_word_left" => self.delete_sel_or(|d| d.word_left(d.head)),
            "delete_word_right" => self.delete_sel_or(|d| d.word_right(d.head)),
            "delete_to_line_start" => self.delete_sel_or(|d| Pos::new(d.head.line, 0)),
            "delete_line" => self.delete_lines(),
            "move_line_up" => self.move_lines(true),
            "move_line_down" => self.move_lines(false),
            "copy_line_up" => self.copy_lines(true),
            "copy_line_down" => self.copy_lines(false),
            "insert_line_below" => self.insert_line(true),
            "insert_line_above" => self.insert_line(false),
            "indent" => self.indent(),
            "outdent" | "shift_tab" => self.outdent(),
            "tab" => {
                let (a, b) = self.sel();
                if a.line != b.line {
                    self.indent();
                } else {
                    let n = self.indent_unit.len();
                    let pad = n - self.char_col(a) % n;
                    self.insert(&" ".repeat(pad));
                }
            }
            "toggle_comment" => self.toggle_comment(),
            "undo" => self.undo(),
            "redo" => self.redo(),
            "soft_wrap" => {
                let top = self.pos_at((self.scroll_y / LINE_H) as usize, 0, cols);
                self.soft_wrap = !self.soft_wrap;
                self.scroll_y = self.visual(Pos::new(top.line, 0), cols).0 as f32 * LINE_H;
                return true;
            }
            _ => return false,
        }
        if !keep_goal {
            self.goal_x = None;
        }
        self.reveal_caret(view_h, cols);
        true
    }

    // ----- clipboard helpers ---------------------------------------------------

    /// Copy text; with no selection, the whole line (VS Code behaviour).
    pub fn copy_text(&self) -> (String, bool) {
        if self.has_selection() {
            (self.selected_text(), false)
        } else {
            (format!("{}\n", self.lines[self.head.line]), true)
        }
    }

    pub fn cut(&mut self) -> (String, bool) {
        let (s, line) = self.copy_text();
        if line {
            let l = self.head.line;
            self.anchor = Pos::new(l, 0);
            self.head = if l + 1 < self.lines.len() { Pos::new(l + 1, 0) } else { self.end() };
        }
        let (a, b) = self.sel();
        self.edit(a, b, "", false);
        (s, line)
    }

    /// Paste; a whole-line clip goes above the current line.
    pub fn paste(&mut self, s: &str, whole_line: bool) {
        if whole_line && !self.has_selection() {
            let at = Pos::new(self.head.line, 0);
            let caret = self.head;
            self.edit(at, at, s, false);
            let n = s.matches('\n').count();
            self.set_head(Pos::new(caret.line + n, caret.col), false);
        } else {
            self.insert(s);
        }
    }

    // ----- mouse -----------------------------------------------------------------

    pub fn click(&mut self, row: usize, x: usize, cols: usize, extend: bool) {
        let p = self.pos_at(row, x, cols);
        self.set_head(p, extend);
        self.goal_x = None;
    }

    pub fn goto_line(&mut self, line: usize, view_h: f32, cols: usize) {
        let l = line.saturating_sub(1).min(self.lines.len() - 1);
        let col = self.indent_of(l).len();
        self.set_head(Pos::new(l, col), false);
        // Center it.
        let y = self.visual(self.head, cols).0 as f32 * LINE_H;
        self.scroll_y = (y - view_h / 2.0).clamp(0.0, self.max_scroll(view_h, cols));
    }

    // ----- find --------------------------------------------------------------------

    fn find_in_line(line: &str, q: &str, from: usize) -> Option<usize> {
        let (l, q) = (line.as_bytes(), q.as_bytes());
        if q.is_empty() || l.len() < q.len() {
            return None;
        }
        (from..=l.len() - q.len()).find(|&i| l[i..i + q.len()].eq_ignore_ascii_case(q))
    }

    fn rfind_in_line(line: &str, q: &str, before: usize) -> Option<usize> {
        let (l, q) = (line.as_bytes(), q.as_bytes());
        if q.is_empty() || l.len() < q.len() {
            return None;
        }
        (0..=(l.len() - q.len()).min(before.saturating_sub(1).min(l.len()))).rev()
            .find(|&i| i + q.len() <= l.len() && l[i..i + q.len()].eq_ignore_ascii_case(q))
    }

    /// Byte ranges of `q` in one line (ASCII case-insensitive).
    pub fn matches_in(&self, line: usize, q: &str) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(i) = Self::find_in_line(&self.lines[line], q, at) {
            out.push(i..i + q.len());
            at = i + q.len().max(1);
        }
        out
    }

    pub fn count_matches(&self, q: &str) -> usize {
        if q.is_empty() || q.contains('\n') {
            return 0;
        }
        (0..self.lines.len()).map(|l| self.matches_in(l, q).len()).sum()
    }

    /// Select the next/previous match from the caret, wrapping around.
    pub fn find(&mut self, q: &str, forward: bool, view_h: f32, cols: usize) -> bool {
        if q.is_empty() || q.contains('\n') {
            return false;
        }
        let n = self.lines.len();
        let (a, b) = self.sel();
        let hit = if forward {
            let start = b;
            (0..=n).find_map(|k| {
                let l = (start.line + k) % n;
                let from = if k == 0 { start.col } else { 0 };
                Self::find_in_line(&self.lines[l], q, from).map(|c| Pos::new(l, c))
            })
        } else {
            let start = a;
            (0..=n).find_map(|k| {
                let l = (start.line + n * 2 - k) % n;
                let before = if k == 0 { start.col } else { usize::MAX };
                Self::rfind_in_line(&self.lines[l], q, before).map(|c| Pos::new(l, c))
            })
        };
        let Some(p) = hit else { return false };
        self.anchor = p;
        self.head = Pos::new(p.line, p.col + q.len());
        let y = self.visual(self.head, cols).0 as f32 * LINE_H;
        if y < self.scroll_y || y + LINE_H > self.scroll_y + view_h {
            self.scroll_y = (y - view_h / 3.0).clamp(0.0, self.max_scroll(view_h, cols));
        }
        true
    }

    /// Replace the current match (if selected) and move to the next one.
    pub fn replace_one(&mut self, q: &str, with: &str, view_h: f32, cols: usize) {
        if self.has_selection() && self.selected_text().eq_ignore_ascii_case(q) {
            self.insert(with);
        }
        self.find(q, true, view_h, cols);
    }

    /// One undo step for all replacements.
    pub fn replace_all(&mut self, q: &str, with: &str) -> usize {
        if q.is_empty() || q.contains('\n') {
            return 0;
        }
        let before = (self.anchor, self.head);
        let mut edits = Vec::new();
        for l in (0..self.lines.len()).rev() {
            for r in self.matches_in(l, q).into_iter().rev() {
                let a = Pos::new(l, r.start);
                let removed = self.lines[l][r.clone()].to_string();
                self.replace_raw(a, Pos::new(l, r.end), with);
                edits.push(Edit { at: a, removed, inserted: with.to_string() });
            }
        }
        let n = edits.len();
        if n > 0 {
            self.redo.clear();
            self.anchor = Pos::new(self.anchor.line.min(self.lines.len() - 1), 0);
            self.head = self.anchor;
            self.undo.push(Group { edits, before, after: (self.anchor, self.head), typing: false, at: Instant::now() });
        }
        n
    }
}

/// Keeps old colors roughly in place until the background re-highlight lands.
/// Replaces bytes `a..b` with `n` new bytes.
fn shift_spans(spans: &mut LineSpans, a: usize, b: usize, n: usize) {
    let delta = n as isize - (b - a) as isize;
    let mv = |x: usize| -> usize {
        if x <= a {
            x
        } else if x < b {
            a + n
        } else {
            (x as isize + delta) as usize
        }
    };
    for (r, _) in spans.iter_mut() {
        let grow_end = n > 0 && r.end == a && a == b;
        r.start = mv(r.start);
        r.end = if grow_end { a + n } else { mv(r.end) };
    }
    spans.retain(|(r, _)| r.end > r.start);
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

pub struct CodeView<'a> {
    doc: &'a Doc,
    shared: &'a Shared,
    focused: bool,
    find: Option<&'a str>,
    opacity: f32,
}

impl<'a> CodeView<'a> {
    pub fn new(doc: &'a Doc, shared: &'a Shared, focused: bool, find: Option<&'a str>, opacity: f32) -> Self {
        CodeView { doc, shared, focused, find, opacity }
    }
}

struct State<P> {
    /// (line version, row within line, wrap cols) -> shaped row
    cache: RefCell<HashMap<(u64, u32, u32), P>>,
    char_w: Cell<f32>,
    dragging: Cell<bool>,
}

impl<P> Default for State<P> {
    fn default() -> Self {
        State { cache: RefCell::new(HashMap::new()), char_w: Cell::new(0.0), dragging: Cell::new(false) }
    }
}

impl<P: text::Paragraph<Font = Font>> State<P> {
    fn char_w(&self) -> f32 {
        if self.char_w.get() == 0.0 {
            let p = P::with_text(mono_text("0000000000"));
            self.char_w.set(p.min_bounds().width / 10.0);
        }
        self.char_w.get()
    }
}

/// Measured Menlo advance, shared with the terminal.
pub fn cell_width<P: text::Paragraph<Font = Font>>() -> f32 {
    P::with_text(mono_text("0000000000")).min_bounds().width / 10.0
}

struct Metrics {
    gutter_w: f32,
    text_x: f32,
    cols: usize,
}

impl CodeView<'_> {
    fn metrics(&self, bounds: Rectangle, char_w: f32) -> Metrics {
        let digits = self.doc.lines.len().max(999).to_string().len() as f32;
        let gutter_w = GUTTER_PAD + digits * char_w + GUTTER_PAD;
        let text_x = bounds.x + gutter_w + GUTTER_PAD;
        let cols = (((bounds.x + bounds.width - text_x - GUTTER_PAD) / char_w).floor() as usize).max(8);
        Metrics { gutter_w, text_x, cols }
    }

    fn hit(&self, p: Point, bounds: Rectangle, char_w: f32) -> (usize, usize) {
        let m = self.metrics(bounds, char_w);
        let row = ((p.y - bounds.y + self.doc.scroll_y) / LINE_H).max(0.0) as usize;
        let x = ((p.x - m.text_x) / char_w).round().max(0.0) as usize;
        (row, x)
    }
}

impl<'a, Theme, Renderer> Widget<Message, Theme, Renderer> for CodeView<'a>
where
    Renderer: text::Renderer<Font = Font>,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State<Renderer::Paragraph>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::<Renderer::Paragraph>::default())
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let state = tree.state.downcast_ref::<State<Renderer::Paragraph>>();
        let size = limits.max();
        let m = self.metrics(Rectangle::new(Point::ORIGIN, size), state.char_w());
        self.shared.editor_h.set(size.height);
        self.shared.editor_cols.set(m.cols);
        self.shared.char_w.set(state.char_w());
        layout::Node::new(size)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State<Renderer::Paragraph>>();
        let bounds = layout.bounds();
        let cw = state.char_w();
        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let dy = match *delta {
                    ScrollDelta::Lines { y, .. } => -y * LINE_H * 3.0,
                    ScrollDelta::Pixels { y, .. } => -y,
                };
                let cols = self.metrics(bounds, cw).cols;
                let y = (self.doc.scroll_y + dy).clamp(0.0, self.doc.max_scroll(bounds.height, cols));
                shell.publish(Message::EditorScroll(y));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position_over(bounds) {
                    let (row, x) = self.hit(p, bounds, cw);
                    state.dragging.set(true);
                    shell.publish(Message::EditorMouse { row, x, drag: false });
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.dragging.get() => {
                let (row, x) = self.hit(*position, bounds, cw);
                shell.publish(Message::EditorMouse { row, x, drag: true });
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => state.dragging.set(false),
            Event::Keyboard(keyboard::Event::KeyPressed { key, physical_key, modifiers, text, .. }) if self.focused => {
                shell.publish(Message::Key(KeyPress {
                    key: key.clone(),
                    latin: key.to_latin(*physical_key),
                    mods: *modifiers,
                    text: text.as_ref().map(|t| t.to_string()),
                }));
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn mouse_interaction(&self, _t: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, _v: &Rectangle, _r: &Renderer) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) { mouse::Interaction::Text } else { mouse::Interaction::None }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State<Renderer::Paragraph>>();
        let bounds = layout.bounds();
        let doc = self.doc;
        let cw = state.char_w();
        let Metrics { gutter_w, text_x, cols } = self.metrics(bounds, cw);

        renderer.fill_quad(Quad { bounds, ..Default::default() }, rgba(qur_dark::BG, self.opacity));

        let total = doc.total_rows(cols);
        let first = ((doc.scroll_y / LINE_H) as usize).min(total);
        let last = (((doc.scroll_y + bounds.height) / LINE_H).ceil() as usize + 1).min(total);
        let y_of = |row: usize| bounds.y + row as f32 * LINE_H - doc.scroll_y;
        let (sa, sb) = doc.sel();
        let (caret_row, caret_x) = doc.visual(doc.head, cols);
        let rect = |row: usize, x0: usize, x1: usize| {
            Rectangle::new(Point::new(text_x + x0 as f32 * cw, y_of(row)), Size::new((x1 - x0) as f32 * cw, LINE_H))
        };

        renderer.with_layer(bounds, |renderer| {
            if !doc.has_selection() && (first..last).contains(&caret_row) {
                let line_first = doc.visual(Pos::new(doc.head.line, 0), cols).0;
                let rows = doc.line_rows(doc.head.line, cols).1;
                renderer.fill_quad(
                    Quad {
                        bounds: Rectangle::new(Point::new(bounds.x, y_of(line_first)), Size::new(bounds.width, rows as f32 * LINE_H)),
                        ..Default::default()
                    },
                    rgba(qur_dark::CURRENT_LINE, self.opacity),
                );
            }

            let mut cache = state.cache.borrow_mut();
            let mut live = Vec::with_capacity(last - first);
            let mut row = first;
            while row < last {
                let line = doc.line_at_row(row, cols);
                let (lfirst, nrows) = doc.line_rows(line, cols);
                let text = &doc.lines[line];
                let chars: Vec<usize> = if text.is_ascii() { Vec::new() } else { text.char_indices().map(|(i, _)| i).collect() };
                let byte = |cc: usize| if text.is_ascii() { cc.min(text.len()) } else { chars.get(cc).copied().unwrap_or(text.len()) };
                let nchars = if text.is_ascii() { text.len() } else { chars.len() };
                let cc_of = |b: usize| if text.is_ascii() { b } else { chars.partition_point(|&i| i < b) };

                // Find matches (this line only).
                let found = self.find.map(|q| doc.matches_in(line, q)).unwrap_or_default();

                for r in (row - lfirst)..nrows {
                    let vrow = lfirst + r;
                    if vrow >= last {
                        break;
                    }
                    let c0 = if doc.soft_wrap { r * cols } else { 0 };
                    let c1 = if doc.soft_wrap { (c0 + cols).min(nchars) } else { nchars };
                    let (b0, b1) = (byte(c0), byte(c1));
                    let y = y_of(vrow);

                    for m in &found {
                        let (m0, m1) = (cc_of(m.start).max(c0), cc_of(m.end).min(c1));
                        if m0 < m1 {
                            renderer.fill_quad(Quad { bounds: rect(vrow, m0 - c0, m1 - c0), ..Default::default() }, rgb(0x4A4020));
                        }
                    }
                    // Selection on this row.
                    if doc.has_selection() && Pos::new(line, b0) <= sb && sa <= Pos::new(line, b1) {
                        let s0 = if sa.line == line { cc_of(sa.col).max(c0) } else { c0 };
                        let s1 = if sb.line == line { cc_of(sb.col).min(c1) } else { c1 + 1 };
                        if s1 > s0 || (sb.line > line && r + 1 == nrows) {
                            renderer.fill_quad(
                                Quad { bounds: rect(vrow, s0 - c0, s1.max(s0 + 1) - c0), ..Default::default() },
                                rgb(qur_dark::SELECTION),
                            );
                        }
                    }

                    let ver = doc.vers[line];
                    let key = (ver, r as u32, if doc.soft_wrap { cols as u32 } else { 0 });
                    live.push(key);
                    let para = cache.entry(key).or_insert_with(|| {
                        let mut spans: Vec<text::Span<'_, (), Font>> = Vec::new();
                        let mut at = b0;
                        for (sr, c) in &doc.spans[line] {
                            let (s, e) = (sr.start.max(b0), sr.end.min(b1));
                            if s >= e || s < at || e > text.len() || !text.is_char_boundary(s) || !text.is_char_boundary(e) {
                                continue;
                            }
                            if s > at {
                                spans.push(text::Span::new(&text[at..s]));
                            }
                            spans.push(text::Span::new(&text[s..e]).color(rgb(*c)));
                            at = e;
                        }
                        if at < b1 {
                            spans.push(text::Span::new(&text[at..b1]));
                        }
                        Renderer::Paragraph::with_spans(mono_text(&spans[..]))
                    });
                    renderer.fill_paragraph(para, Point::new(text_x, y), rgb(qur_dark::TEXT), bounds);

                    if r == 0 {
                        let num = (line + 1).to_string();
                        let num_x = bounds.x + gutter_w - GUTTER_PAD - num.len() as f32 * cw;
                        let color = if line == doc.head.line { qur_dark::TEXT } else { qur_dark::TEXT_MUTED };
                        renderer.fill_text(mono_text(num), Point::new(num_x, y), rgb(color), bounds);
                    }
                }
                row = lfirst + nrows;
            }
            cache.retain(|k, _| live.contains(k));

            if (first..last).contains(&caret_row) {
                renderer.fill_quad(
                    Quad {
                        bounds: Rectangle::new(
                            Point::new(text_x + caret_x as f32 * cw - 1.0, y_of(caret_row) + 2.0),
                            Size::new(2.0, LINE_H - 4.0),
                        ),
                        ..Default::default()
                    },
                    if self.focused { Color::WHITE } else { rgb(qur_dark::TEXT_MUTED) },
                );
            }
        });

        self.shared.last_draw.set(Some(Instant::now()));
    }
}

impl<'a> From<CodeView<'a>> for Element<'a, Message> {
    fn from(v: CodeView<'a>) -> Self {
        Element::new(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(s: &str) -> Doc {
        Doc::new(s, &[])
    }

    #[test]
    fn typing_groups_into_one_undo() {
        let mut d = doc("ab");
        d.head = Pos::new(0, 2);
        d.anchor = d.head;
        for c in ["x", "y", "z"] {
            d.insert(c);
        }
        d.newline();
        d.insert("q");
        assert_eq!(d.text(), "abxyz\nq");
        d.undo();
        assert_eq!(d.text(), "abxyz\n");
        d.undo();
        assert_eq!(d.text(), "abxyz");
        d.undo();
        assert_eq!(d.text(), "ab");
        d.redo();
        assert_eq!(d.text(), "abxyz");
    }

    #[test]
    fn line_ops_and_comment() {
        let mut d = doc("a\n  b\nc");
        d.head = Pos::new(1, 0);
        d.anchor = d.head;
        d.run("move_line_up", 400.0, 80);
        assert_eq!(d.text(), "  b\na\nc");
        d.run("copy_line_down", 400.0, 80);
        assert_eq!(d.text(), "  b\n  b\na\nc");
        d.run("toggle_comment", 400.0, 80);
        assert_eq!(d.lines[1], "  // b");
        d.run("toggle_comment", 400.0, 80);
        assert_eq!(d.lines[1], "  b");
        d.run("delete_line", 400.0, 80);
        assert_eq!(d.text(), "  b\na\nc");
        d.undo();
        d.undo();
        d.undo();
        d.undo();
        d.undo();
        assert_eq!(d.text(), "a\n  b\nc");
    }

    #[test]
    fn soft_wrap_rows() {
        let mut d = doc(&format!("{}\nshort", "x".repeat(25)));
        assert_eq!(d.total_rows(10), 4);
        assert_eq!(d.visual(Pos::new(0, 12), 10), (1, 2));
        assert_eq!(d.pos_at(3, 2, 10), Pos::new(1, 2));
        d.head = Pos::new(0, 3);
        d.anchor = d.head;
        d.run("down", 400.0, 10);
        assert_eq!(d.head, Pos::new(0, 13));
        d.soft_wrap = false;
        assert_eq!(d.total_rows(10), 2);
    }

    #[test]
    fn find_and_replace_all() {
        let mut d = doc("foo bar\nFoo\nbaz foo");
        assert_eq!(d.count_matches("foo"), 3);
        assert!(d.find("foo", true, 400.0, 80));
        assert_eq!(d.head, Pos::new(0, 3));
        assert!(d.find("foo", true, 400.0, 80));
        assert_eq!(d.anchor, Pos::new(1, 0));
        assert_eq!(d.replace_all("foo", "qux"), 3);
        assert_eq!(d.text(), "qux bar\nqux\nbaz qux");
        d.undo();
        assert_eq!(d.text(), "foo bar\nFoo\nbaz foo");
    }

    #[test]
    fn word_motion_and_smart_home() {
        let mut d = doc("    let fooBar = 1;");
        d.head = Pos::new(0, 19);
        d.anchor = d.head;
        d.run("word_left", 400.0, 80);
        assert_eq!(d.head.col, 18);
        d.run("word_left", 400.0, 80);
        assert_eq!(d.head.col, 17);
        d.run("line_start", 400.0, 80);
        assert_eq!(d.head.col, 4);
        d.run("line_start", 400.0, 80);
        assert_eq!(d.head.col, 0);
    }

    #[test]
    fn whole_line_copy_paste() {
        let mut d = doc("one\ntwo");
        d.head = Pos::new(1, 1);
        d.anchor = d.head;
        let (s, line) = d.copy_text();
        assert!(line);
        d.paste(&s, line);
        assert_eq!(d.text(), "one\ntwo\ntwo");
    }
}
