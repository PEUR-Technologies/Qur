//! Inline diff review in the code modal: gutter markers for the lines an
//! agent added, changed or removed vs its task's base (or your file vs
//! HEAD), Next / Previous Change, and a peek under a change with the removed
//! lines and Revert / Accept.
//!
//! Lazy and off the UI thread: the base text is read once when the file
//! opens (the workspace hands it over); each edit only restarts a short
//! timer, then the line diff runs on the background executor over a rope
//! clone. Painting does a binary search per visible row, nothing more.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyElement, Bounds, Context, MouseDownEvent, PaintQuad, Pixels, Task, div, fill, point, prelude::*, px, size};
use qur_editor::diff::{self as ld, HunkKind, LineHunk};
use qur_editor::{Point, Rope, Selection};

use super::{EditorEvent, EditorView, Row};
use crate::look::Look;

/// Typing pauses this long before the markers are recomputed.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// Removed lines shown in the peek before "… N more".
const PEEK_LINES: usize = 30;

/// What the markers compare the buffer with.
pub struct DiffBase {
    /// For the peek and messages: "the task's base", "turn 3's start", "HEAD".
    pub label: String,
    /// Same key and text = the same base (Accept marks survive a reload).
    pub key: String,
    text: String,
    lines: Vec<Range<usize>>,
    hashes: Vec<u64>,
}

impl DiffBase {
    /// `text` as read from git; a BOM and CRLF line ends are dropped, as the
    /// editor does when it opens a file.
    pub fn new(label: String, key: String, text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let text = if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n") } else { text.to_string() };
        let lines = ld::line_ranges(&text);
        let hashes = ld::text_hashes(&text, &lines);
        DiffBase { label, key, text, lines, hashes }
    }

    /// The base's lines `r`, with their newlines (for Revert).
    fn old_text(&self, r: &Range<usize>) -> &str {
        if r.is_empty() {
            return "";
        }
        &self.text[self.lines[r.start].start..self.lines[r.end - 1].end]
    }
}

/// One buffer's change markers.
pub struct GutterDiff {
    pub base: Arc<DiffBase>,
    /// Every hunk of the last diff, with its signature.
    all: Vec<(LineHunk, u64)>,
    /// The hunks shown (not accepted), in buffer order.
    hunks: Vec<LineHunk>,
    sigs: Vec<u64>,
    /// Hunks marked reviewed (Accept): hidden until their text changes.
    accepted: HashSet<u64>,
    /// The hunk whose peek is open (by signature, so it survives a re-diff).
    peek: Option<u64>,
    /// After Revert or Accept with the peek open: reopen it on the next
    /// hunk at or after this buffer line once the diff catches up.
    peek_from: Option<usize>,
    seq: u64,
    _task: Option<Task<()>>,
}

/// The hunks between `base` and `rope`, each with a signature (its base
/// position and both sides' line hashes) that Accept remembers.
fn compute(base: &DiffBase, rope: &Rope) -> Vec<(LineHunk, u64)> {
    let new = ld::rope_hashes(rope);
    ld::diff_lines(&base.hashes, &new)
        .into_iter()
        .map(|h| {
            let mut s = 0xcbf2_9ce4_8422_2325u64 ^ h.old.start as u64;
            for x in base.hashes[h.old.clone()].iter().chain([&u64::MAX]).chain(&new[h.new.clone()]) {
                s = (s ^ x).wrapping_mul(0x100_0000_01b3);
            }
            (h, s)
        })
        .collect()
}

impl GutterDiff {
    fn new(base: DiffBase) -> Self {
        GutterDiff {
            base: Arc::new(base),
            all: Vec::new(),
            hunks: Vec::new(),
            sigs: Vec::new(),
            accepted: HashSet::new(),
            peek: None,
            peek_from: None,
            seq: 0,
            _task: None,
        }
    }

    fn set_all(&mut self, all: Vec<(LineHunk, u64)>) {
        self.all = all;
        self.refilter();
    }

    fn refilter(&mut self) {
        let shown: Vec<&(LineHunk, u64)> = self.all.iter().filter(|(_, s)| !self.accepted.contains(s)).collect();
        self.hunks = shown.iter().map(|(h, _)| h.clone()).collect();
        self.sigs = shown.iter().map(|(_, s)| *s).collect();
        if self.peek.is_some_and(|p| !self.sigs.contains(&p)) {
            self.peek = None;
        }
        if let Some(from) = self.peek_from.take() {
            self.peek = self.hunks.iter().position(|h| h.new.start >= from).map(|i| self.sigs[i]);
        }
    }

    /// The hunk at buffer line `line`: one that covers it, or a removal
    /// right above or below it.
    fn at(&self, line: usize) -> Option<usize> {
        self.hunks.iter().position(|h| {
            if h.new.is_empty() { line == h.new.start || line + 1 == h.new.start } else { h.new.contains(&line) }
        })
    }

    fn peek_ix(&self) -> Option<usize> {
        self.peek.and_then(|p| self.sigs.iter().position(|s| *s == p))
    }
}

impl EditorView {
    /// Show change markers against `base` (None: no markers). The workspace
    /// calls it when the code modal opens a file.
    pub fn set_diff_base(&mut self, base: Option<DiffBase>, cx: &mut Context<Self>) {
        let Some(base) = base else {
            if self.diff.take().is_some() {
                cx.notify();
            }
            return;
        };
        match &mut self.diff {
            // The same base again: keep the hunks and the Accept marks.
            Some(d) if d.base.key == base.key && d.base.text == base.text => {}
            Some(d) if d.base.key == base.key => d.base = Arc::new(base),
            _ => self.diff = Some(GutterDiff::new(base)),
        }
        self.schedule_diff(false, cx);
    }

    /// Recompute the hunks off the UI thread (`debounce`: after typing
    /// pauses). A newer call or edit supersedes an older one.
    pub(super) fn schedule_diff(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let Some(d) = self.diff.as_mut() else { return };
        d.seq += 1;
        let seq = d.seq;
        let base = d.base.clone();
        let rope = self.model.rope().clone();
        let bg = cx.background_executor().clone();
        d._task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                bg.timer(DEBOUNCE).await;
            }
            let all = bg.spawn(async move { compute(&base, &rope) }).await;
            this.update(cx, |this, cx| {
                if let Some(d) = this.diff.as_mut()
                    && d.seq == seq
                {
                    d.set_all(all);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn change_message(&self, cx: &mut Context<Self>) {
        let msg = match &self.diff {
            Some(d) => format!("No changes vs {}", d.base.label),
            None => "No change markers for this file (not in a git checkout)".to_string(),
        };
        cx.emit(EditorEvent::Message(msg));
    }

    /// For scripted runs' `state`: the hunks as `Kind@line`, the peek, the base.
    pub fn diff_debug(&self) -> String {
        let Some(d) = &self.diff else { return "none".into() };
        let hunks: Vec<String> = d.hunks.iter().map(|h| format!("{:?}@{}", h.kind(), h.new.start + 1)).collect();
        format!("[{}] peek={} vs {:?}", hunks.join(" "), d.peek_ix().map_or(0, |i| i + 1), d.base.label)
    }

    /// Scripted runs (QUR_DEBUG_ACTIONS) log where review commands land.
    fn log_change(&self, what: &str) {
        if std::env::var_os("QUR_DEBUG_ACTIONS").is_some() {
            println!("CHANGE {what} line={} {}", self.cursor().line + 1, self.diff_debug());
        }
    }

    fn go_to_hunk(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(d) = self.diff.as_mut() else { return };
        let Some(h) = d.hunks.get(i).cloned() else { return };
        if d.peek.is_some() {
            d.peek = Some(d.sigs[i]);
        }
        let last = self.model.line_count().saturating_sub(1);
        let line = h.new.start.min(last);
        self.model.set_selections(vec![Selection::caret(Point { line, col: 0 })], 0);
        self.reveal_lines(line..h.new.end.max(line + 1), cx);
        cx.emit(EditorEvent::Changed);
    }

    /// Next / Previous Change (Alt+F5 / Shift+Alt+F5), wrapping around.
    pub fn step_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(d) = self.diff.as_ref().filter(|d| !d.hunks.is_empty()) else {
            return self.change_message(cx);
        };
        let line = self.cursor().line;
        let last = self.model.line_count().saturating_sub(1);
        let anchor = |h: &LineHunk| h.new.start.min(last);
        let n = d.hunks.len();
        let i = if forward {
            d.hunks.iter().position(|h| anchor(h) > line).unwrap_or(0)
        } else {
            d.hunks.iter().rposition(|h| anchor(h) < line).unwrap_or(n - 1)
        };
        self.go_to_hunk(i, cx);
        self.log_change(if forward { "next" } else { "prev" });
    }

    /// The hunk Revert / Accept act on: the open peek's, else the cursor's.
    fn target_hunk(&self) -> Option<usize> {
        let d = self.diff.as_ref()?;
        d.peek_ix().or_else(|| d.at(self.cursor().line))
    }

    /// Put the base's text back for the change at the cursor: one undoable
    /// edit in the buffer (save to keep it). The index is never touched.
    pub fn revert_change(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(i) = self.target_hunk() else {
            return self.no_change_here(cx);
        };
        let Some(d) = self.diff.as_mut() else { return };
        let h = d.hunks[i].clone();
        let text = d.base.old_text(&h.old).to_string();
        if d.peek.is_some() {
            d.peek_from = Some(h.new.start);
        }
        self.apply(false, cx, |m, _| m.replace_lines(h.new.clone(), &text));
        // Re-diff now, not after the typing pause: the line numbers moved.
        self.schedule_diff(false, cx);
        self.log_change("revert");
    }

    /// Mark the change at the cursor reviewed: its marker goes until its
    /// text changes again, and the cursor moves on to the next change.
    pub fn accept_change(&mut self, cx: &mut Context<Self>) {
        let Some(i) = self.target_hunk() else {
            return self.no_change_here(cx);
        };
        let Some(d) = self.diff.as_mut() else { return };
        let from = d.hunks[i].new.start;
        d.accepted.insert(d.sigs[i]);
        let peeking = d.peek.is_some();
        d.peek_from = peeking.then_some(from);
        d.refilter();
        if let Some(next) = d.hunks.iter().position(|h| h.new.start >= from) {
            self.go_to_hunk(next, cx);
        } else if d.hunks.is_empty() {
            cx.emit(EditorEvent::Message(format!("All changes vs {} reviewed", d.base.label)));
        }
        cx.notify();
        self.log_change("accept");
    }

    /// Show Change: open (or close) the peek under the change at the cursor,
    /// else under the next one.
    pub fn toggle_peek(&mut self, cx: &mut Context<Self>) {
        let line = self.cursor().line;
        let last = self.model.line_count().saturating_sub(1);
        let Some(d) = self.diff.as_mut().filter(|d| !d.hunks.is_empty()) else {
            return self.change_message(cx);
        };
        if d.peek.take().is_none() {
            let here = d.at(line);
            let i = here.unwrap_or_else(|| d.hunks.iter().position(|h| h.new.start.min(last) > line).unwrap_or(0));
            d.peek = Some(d.sigs[i]);
            if here.is_none() {
                self.go_to_hunk(i, cx);
            }
        }
        cx.notify();
        self.log_change("peek");
    }

    /// Close the peek (Esc, ✕). True when one was open.
    pub(super) fn close_peek(&mut self, cx: &mut Context<Self>) -> bool {
        let closed = self.diff.as_mut().is_some_and(|d| d.peek.take().is_some());
        if closed {
            cx.notify();
        }
        closed
    }

    fn no_change_here(&self, cx: &mut Context<Self>) {
        if self.diff.as_ref().is_some_and(|d| !d.hunks.is_empty()) {
            cx.emit(EditorEvent::Message("No change at the cursor (Alt+F5 goes to the next one)".into()));
        } else {
            self.change_message(cx);
        }
    }

    /// A click on a change marker (the strip between the line numbers and
    /// the text) toggles that change's peek. True when it was one.
    pub(super) fn gutter_click(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(b) = self.bounds else { return false };
        let x = ev.position.x - b.left();
        if ev.click_count != 1 || x < self.text_x - self.cell_w * 1.25 || x >= self.text_x - px(2.) {
            return false;
        }
        let p = self.hit(ev.position);
        let Some(d) = self.diff.as_mut() else { return false };
        let Some(i) = d.at(p.line) else { return false };
        d.peek = if d.peek == Some(d.sigs[i]) { None } else { Some(d.sigs[i]) };
        self.model.set_selections(vec![Selection::caret(Point { line: p.line, col: 0 })], 0);
        cx.emit(EditorEvent::Changed);
        cx.notify();
        true
    }

    /// Gutter markers for the visible rows: a bar beside added (green) and
    /// changed (amber) lines, a red tick where lines were removed.
    pub(super) fn diff_quads(&self, rows: &[Row], bounds: Bounds<Pixels>, look: &Look) -> Vec<PaintQuad> {
        let Some(d) = self.diff.as_ref().filter(|d| !d.hunks.is_empty()) else { return Vec::new() };
        let colors = &look.theme.diff;
        let mut modified = look.c(colors.modified_line);
        modified.a = 1.;
        let (added, removed) = (look.c(colors.added_text), look.c(colors.removed_text));
        let lh = self.line_h;
        let x = bounds.left() + self.text_x - look.z(7.);
        let w = look.z(3.);
        let lines = self.model.rope().len_lines();
        let tick = |y: Pixels| fill(Bounds::new(point(x - look.z(1.), y - look.z(2.)), size(look.z(8.), look.z(4.))), removed);
        // Lines removed after a last line without a newline: no line below
        // to sit on, so the tick goes under the last row.
        let eof_removal = d.hunks.last().is_some_and(|h| h.new.is_empty() && h.new.start >= lines);
        let mut out = Vec::new();
        for (ri, row) in rows.iter().enumerate() {
            let i = d.hunks.partition_point(|h| h.new.end < row.line);
            for h in d.hunks[i..].iter().take_while(|h| h.new.start <= row.line) {
                if h.new.is_empty() {
                    // A removal right above this line (its first row).
                    if row.gutter.is_some() {
                        out.push(tick(row.y));
                    }
                } else if h.new.contains(&row.line) {
                    let c = if h.kind() == HunkKind::Added { added } else { modified };
                    out.push(fill(Bounds::new(point(x, row.y), size(w, lh)), c));
                }
            }
            if eof_removal && row.line + 1 == lines && rows.get(ri + 1).is_none_or(|r| r.line != row.line) {
                out.push(tick(row.y + lh));
            }
        }
        out
    }

    /// The peek under the open change: what it replaced (the removed lines,
    /// struck through in red) and its buttons.
    pub(super) fn peek_view(&self, look: &Look, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.diff.as_ref()?;
        let i = d.peek_ix()?;
        let h = &d.hunks[i];
        let ui = &look.theme.ui;
        let colors = &look.theme.diff;
        let rope = self.model.rope();
        // Below the change's last line (above the next line for a removal).
        let row = if h.new.end < rope.len_lines() {
            self.model.wrap().point_to_row(rope, Point { line: h.new.end, col: 0 }).0
        } else {
            self.model.wrap().row_count()
        };
        let top = self.line_h * row as f32 - self.scroll_y;
        let old: Vec<String> = d.base.old_text(&h.old).lines().map(|l| expand_tabs(l, self.tab_width)).collect();
        let more = old.len().saturating_sub(PEEK_LINES);
        let (added, removed) = (h.new.len(), h.old.len());
        let summary = match h.kind() {
            HunkKind::Added => format!("{added} line{} added", plural(added)),
            HunkKind::Removed => format!("{removed} line{} removed", plural(removed)),
            HunkKind::Modified => format!("{removed} line{} → {added}", plural(removed)),
        };
        let button = |label: &'static str, id: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .px(look.z(8.))
                .py(look.z(1.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .bg(look.c(if primary { ui.button_primary_bg } else { ui.button_bg }))
                .text_color(look.c(if primary { ui.button_primary_text } else { ui.button_text }))
                .child(label)
        };
        let header = div()
            .flex()
            .items_center()
            .gap(look.z(6.))
            .px(look.z(10.))
            .py(look.z(4.))
            .text_size(look.text(-1.))
            .border_b_1()
            .border_color(look.c(ui.border))
            .child(div().flex_1().min_w(px(0.)).truncate().text_color(look.c(ui.text_secondary)).child(format!(
                "Change {} of {} · {summary} · vs {}",
                i + 1,
                d.hunks.len(),
                d.base.label
            )))
            .child(button("‹", "peek-prev", false).on_click(cx.listener(|this, _, _, cx| this.step_change(false, cx))))
            .child(button("›", "peek-next", false).on_click(cx.listener(|this, _, _, cx| this.step_change(true, cx))))
            .when(!self.read_only, |el| el.child(button("Revert", "peek-revert", false).on_click(cx.listener(|this, _, _, cx| this.revert_change(cx)))))
            .child(button("Accept", "peek-accept", true).on_click(cx.listener(|this, _, _, cx| this.accept_change(cx))))
            .child(button("✕", "peek-close", false).on_click(cx.listener(|this, _, _, cx| {
                this.close_peek(cx);
            })));
        let mut struck = look.c(colors.removed_text);
        struck.a = 0.55;
        let body = div()
            .flex()
            .flex_col()
            .py(look.z(2.))
            .font_family(look.code_font.clone())
            .text_size(look.code_px())
            .children(old.into_iter().take(PEEK_LINES).map(|l| {
                div()
                    .h(self.line_h)
                    .pl(look.z(10.))
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .bg(look.c(colors.removed_line))
                    .text_color(look.c(look.theme.editor.text))
                    .child(div().flex_none().w(look.z(14.)).text_color(struck).child("−"))
                    .child(l)
            }))
            .when(more > 0, |el| {
                el.child(div().h(self.line_h).pl(look.z(24.)).text_color(look.c(ui.text_muted)).child(format!("… {more} more line{}", plural(more))))
            });
        Some(
            div()
                .absolute()
                .top(top)
                .left(self.text_x - look.z(6.))
                .right(look.z(12.))
                .occlude()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .flex()
                .flex_col()
                .rounded(look.z(5.))
                .overflow_hidden()
                .border_1()
                .border_color(look.c(ui.border_strong))
                .bg(look.c(ui.panel_bg))
                .shadow_md()
                .child(header)
                .when(h.kind() != HunkKind::Added, |el| el.child(body))
                .into_any_element(),
        )
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn expand_tabs(s: &str, tab: usize) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let tab = tab.max(1);
    let mut out = String::with_capacity(s.len() + 8);
    let mut col = 0;
    for c in s.chars() {
        if c == '\t' {
            let n = tab - col % tab;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}
