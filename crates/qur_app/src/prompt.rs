//! The multi-line prompt field (prompt bar, New Agent Task). Reuses the
//! TextInput key context: Enter sends, Shift+Enter is a newline, Esc cancels.
//! Text wraps to the field width; the field grows up to `MAX_LINES`.
//! `@` asks the workspace for a file mention (Quick Open picker); dropped
//! files from the tree are inserted as mentions too.

use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, LayoutId, Pixels, SharedString, Style, TextRun, UTF16Selection, Window,
    WrappedLine, div, fill, font, point, prelude::*, px, relative, size,
};
use smallvec::SmallVec;

use crate::browser_view::Attachment;
use crate::commands::{edit, input};
use crate::look::Look;

const MAX_LINES: usize = 10;

pub enum PromptEvent {
    Submit,
    Cancel,
    /// `@` typed: the workspace opens a file picker and calls `insert_mention`.
    MentionRequested,
    /// `/` typed into an empty field: the workspace offers the agent's commands.
    SlashRequested,
    /// Up in an empty field: the agent view pulls the last queued message back.
    UpInEmpty,
    Changed,
}

/// The field's text, caret and mentions at one point (an undo step).
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    text: String,
    caret: usize,
    mentions: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    /// One character typed at the caret.
    Type,
    /// One character or word deleted at the caret.
    Delete,
    /// Paste, newline, mention, cut, replace-all: always its own step.
    Other,
}

/// Typing (or deleting) within this window of the last keystroke, with the
/// caret where that left it, is one undo step.
const GROUP_WINDOW: Duration = Duration::from_secs(1);
const MAX_UNDO: usize = 200;

/// Undo/redo for the prompt: whole-text snapshots (prompts are short), with
/// consecutive typing coalesced like the editor's `history.rs`.
#[derive(Default)]
struct EditHistory {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The last edit's kind, time and caret after it (None: next edit starts a step).
    last: Option<(EditKind, Instant, usize)>,
}

impl EditHistory {
    /// Before an edit: keep `before` as an undo step unless the edit
    /// continues the previous one. `caret_after` is where the edit leaves the caret.
    fn record(&mut self, before: Snapshot, kind: EditKind, caret_after: usize, now: Instant) {
        self.redo.clear();
        let joins = kind != EditKind::Other
            && matches!(self.last, Some((k, t, c)) if k == kind && now.saturating_duration_since(t) < GROUP_WINDOW && c == before.caret);
        if !joins {
            self.undo.push(before);
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.last = (kind != EditKind::Other).then_some((kind, now, caret_after));
    }

    /// A caret move ends the typing group.
    fn break_group(&mut self) {
        self.last = None;
    }

    fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let s = self.undo.pop()?;
        self.redo.push(current);
        self.last = None;
        Some(s)
    }

    fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let s = self.redo.pop()?;
        self.undo.push(current);
        self.last = None;
        Some(s)
    }
}

/// A file dragged from the Files tree.
#[derive(Clone)]
pub struct DraggedFile {
    pub abs: PathBuf,
    pub label: SharedString,
}

impl Render for DraggedFile {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = &look.theme.ui;
        div()
            .px(look.z(8.))
            .py(look.z(3.))
            .rounded(look.z(4.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .text_size(look.text(-1.))
            .text_color(look.c(ui.text))
            .font_family(look.ui_font.clone())
            .child(self.label.clone())
    }
}

pub struct PromptInput {
    pub focus: FocusHandle,
    pub text: String,
    caret: usize,
    select_all: bool,
    placeholder: SharedString,
    /// Absolute paths mentioned with `@` (sent as resource links).
    pub mentions: Vec<PathBuf>,
    /// Paths are shown relative to this folder.
    pub base: PathBuf,
    /// Browser picks (elements, page text) sent with the next message.
    pub attachments: Vec<Attachment>,
    last_width: Pixels,
    /// Where the text area was last drawn (window coordinates): pickers
    /// opened from this prompt (/, @, settings) sit just above it.
    pub bounds: Option<Bounds<Pixels>>,
    /// How far the text is scrolled up inside the field (past MAX_LINES).
    scroll_y: Pixels,
    max_scroll: Pixels,
    /// Caret and text length at the last paint: when they change, the
    /// field scrolls to show the caret; wheel scrolling leaves them alone.
    painted: (usize, usize),
    history: EditHistory,
}

impl EventEmitter<PromptEvent> for PromptInput {}

impl PromptInput {
    pub fn new(placeholder: impl Into<SharedString>, base: PathBuf, cx: &mut Context<Self>) -> Self {
        PromptInput {
            focus: cx.focus_handle(),
            text: String::new(),
            caret: 0,
            select_all: false,
            placeholder: placeholder.into(),
            mentions: Vec::new(),
            base,
            attachments: Vec::new(),
            last_width: px(400.),
            bounds: None,
            scroll_y: px(0.),
            max_scroll: px(0.),
            painted: (0, 0),
            history: EditHistory::default(),
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { text: self.text.clone(), caret: self.caret, mentions: self.mentions.clone() }
    }

    /// Note the state before an edit, for undo.
    fn record(&mut self, kind: EditKind, caret_after: usize) {
        let before = self.snapshot();
        self.history.record(before, kind, caret_after, Instant::now());
    }

    fn restore(&mut self, s: Snapshot, cx: &mut Context<Self>) {
        self.text = s.text;
        self.caret = s.caret.min(self.text.len());
        self.mentions = s.mentions;
        self.select_all = false;
        cx.emit(PromptEvent::Changed);
        cx.notify();
    }

    /// Take the text and mentions (after Submit), clearing the field.
    pub fn take(&mut self, cx: &mut Context<Self>) -> (String, Vec<PathBuf>) {
        let text = std::mem::take(&mut self.text);
        let mut mentions = std::mem::take(&mut self.mentions);
        // Keep only mentions still present in the text.
        mentions.retain(|m| text.contains(&format!("@{}", self.rel(m))));
        mentions.dedup();
        self.caret = 0;
        self.select_all = false;
        // A sent message starts a fresh history.
        self.history = EditHistory::default();
        cx.emit(PromptEvent::Changed);
        cx.notify();
        (text, mentions)
    }

    fn rel(&self, p: &std::path::Path) -> String {
        p.strip_prefix(&self.base).unwrap_or(p).to_string_lossy().into_owned()
    }

    pub fn attach(&mut self, a: Attachment, cx: &mut Context<Self>) {
        self.attachments.push(a);
        cx.emit(PromptEvent::Changed);
        cx.notify();
    }

    pub fn take_attachments(&mut self) -> Vec<Attachment> {
        std::mem::take(&mut self.attachments)
    }

    /// Replace the whole text (slash command chosen), caret at the end.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.text != text {
            self.record(EditKind::Other, text.len());
        }
        self.text = text.to_string();
        self.caret = self.text.len();
        self.select_all = false;
        cx.emit(PromptEvent::Changed);
        cx.notify();
    }

    /// Insert `@path ` at the caret (replacing a just-typed `@`).
    pub fn insert_mention(&mut self, abs: PathBuf, cx: &mut Context<Self>) {
        let rel = self.rel(&abs);
        if self.caret > 0 && self.text[..self.caret].ends_with('@') {
            self.caret -= 1;
            self.text.remove(self.caret);
        }
        let needs_space = self.caret > 0 && !self.text[..self.caret].ends_with(char::is_whitespace);
        let ins = format!("{}@{rel} ", if needs_space { " " } else { "" });
        self.record(EditKind::Other, self.caret + ins.len());
        self.text.insert_str(self.caret, &ins);
        self.caret += ins.len();
        self.mentions.push(abs);
        cx.emit(PromptEvent::Changed);
        cx.notify();
    }

    fn replace(&mut self, new: &str, cx: &mut Context<Self>) {
        let typed = !self.select_all && new.chars().count() == 1 && new != "\n";
        let after = if self.select_all { new.len() } else { self.caret + new.len() };
        self.record(if typed { EditKind::Type } else { EditKind::Other }, after);
        if self.select_all {
            self.text.clear();
            self.mentions.clear();
            self.caret = 0;
            self.select_all = false;
        }
        self.text.insert_str(self.caret, new);
        self.caret += new.len();
        cx.emit(PromptEvent::Changed);
        if new == "@" {
            cx.emit(PromptEvent::MentionRequested);
        }
        if new == "/" && self.text == "/" {
            cx.emit(PromptEvent::SlashRequested);
        }
        cx.notify();
    }

    fn delete_range(&mut self, r: Range<usize>, cx: &mut Context<Self>) {
        if self.select_all {
            return self.replace("", cx);
        }
        if r.is_empty() {
            return;
        }
        self.record(EditKind::Delete, r.start);
        self.text.replace_range(r.clone(), "");
        self.caret = r.start;
        cx.emit(PromptEvent::Changed);
        cx.notify();
    }

    fn prev(&self) -> usize {
        self.text[..self.caret].char_indices().last().map(|(i, _)| i).unwrap_or(0)
    }
    fn next(&self) -> usize {
        self.text[self.caret..].chars().next().map(|c| self.caret + c.len_utf8()).unwrap_or(self.caret)
    }
    fn line_start(&self) -> usize {
        self.text[..self.caret].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }
    fn line_end(&self) -> usize {
        self.text[self.caret..].find('\n').map(|i| self.caret + i).unwrap_or(self.text.len())
    }
    fn word_left_of(&self, from: usize) -> usize {
        let s = &self.text[..from];
        let t = s.trim_end_matches(|c: char| !(c.is_alphanumeric() || c == '_'));
        t.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_').len()
    }
    fn word_right_of(&self, from: usize) -> usize {
        let s = &self.text[from..];
        let skip = s.len() - s.trim_start_matches(|c: char| !(c.is_alphanumeric() || c == '_')).len();
        let rest = &s[skip..];
        from + skip + (rest.len() - rest.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_').len())
    }
    fn mv(&mut self, to: usize, cx: &mut Context<Self>) {
        self.history.break_group();
        self.select_all = false;
        self.caret = to;
        cx.notify();
    }
    /// Move to the same column on the previous/next hard line.
    fn vertical(&mut self, down: bool, cx: &mut Context<Self>) {
        let start = self.line_start();
        let col = self.text[start..self.caret].chars().count();
        let target_start = if down {
            let end = self.line_end();
            if end >= self.text.len() {
                return;
            }
            end + 1
        } else {
            if start == 0 {
                return;
            }
            self.text[..start - 1].rfind('\n').map(|i| i + 1).unwrap_or(0)
        };
        let line_end = self.text[target_start..].find('\n').map(|i| target_start + i).unwrap_or(self.text.len());
        let to = self.text[target_start..line_end].char_indices().nth(col).map(|(i, _)| target_start + i).unwrap_or(line_end);
        self.mv(to, cx)
    }

    fn a_backspace(&mut self, _: &input::Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.prev();
        self.delete_range(p..self.caret, cx)
    }
    fn a_delete(&mut self, _: &input::Delete, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.next();
        self.delete_range(self.caret..n, cx)
    }
    fn a_del_word(&mut self, _: &input::DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.word_left_of(self.caret);
        self.delete_range(p..self.caret, cx)
    }
    fn a_del_start(&mut self, _: &input::DeleteToStart, _: &mut Window, cx: &mut Context<Self>) {
        let s = self.line_start();
        self.delete_range(s..self.caret, cx)
    }
    fn a_left(&mut self, _: &input::Left, _: &mut Window, cx: &mut Context<Self>) {
        let p = if self.select_all { 0 } else { self.prev() };
        self.mv(p, cx)
    }
    fn a_right(&mut self, _: &input::Right, _: &mut Window, cx: &mut Context<Self>) {
        let n = if self.select_all { self.text.len() } else { self.next() };
        self.mv(n, cx)
    }
    fn a_word_left(&mut self, _: &input::WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.word_left_of(self.caret);
        self.mv(p, cx)
    }
    fn a_word_right(&mut self, _: &input::WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.word_right_of(self.caret);
        self.mv(n, cx)
    }
    fn a_home(&mut self, _: &input::Home, _: &mut Window, cx: &mut Context<Self>) {
        let s = self.line_start();
        self.mv(s, cx)
    }
    fn a_end(&mut self, _: &input::End, _: &mut Window, cx: &mut Context<Self>) {
        let e = self.line_end();
        self.mv(e, cx)
    }
    fn a_up(&mut self, _: &input::Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.text.is_empty() {
            return cx.emit(PromptEvent::UpInEmpty);
        }
        self.vertical(false, cx)
    }
    fn a_down(&mut self, _: &input::Down, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(true, cx)
    }
    fn a_submit(&mut self, _: &input::Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if !self.text.trim().is_empty() || !self.attachments.is_empty() {
            cx.emit(PromptEvent::Submit)
        }
    }
    fn a_newline(&mut self, _: &input::ConfirmAlt, _: &mut Window, cx: &mut Context<Self>) {
        self.replace("\n", cx)
    }
    fn a_submit_cmd(&mut self, _: &input::ConfirmAll, w: &mut Window, cx: &mut Context<Self>) {
        self.a_submit(&input::Confirm, w, cx)
    }
    fn a_cancel(&mut self, _: &input::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PromptEvent::Cancel)
    }
    fn a_tab(&mut self, _: &input::Tab, _: &mut Window, _: &mut Context<Self>) {}
    fn a_select_all(&mut self, _: &edit::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all = !self.text.is_empty();
        cx.notify();
    }
    fn a_copy(&mut self, _: &edit::Copy, _: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.text.clone()));
    }
    fn a_cut(&mut self, _: &edit::Cut, _: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.text.clone()));
        self.select_all = true;
        self.replace("", cx);
    }
    fn a_paste(&mut self, _: &edit::Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(clip) = cx.read_from_clipboard() else { return };
        // A screenshot or copied image: attach it, like a browser pick.
        if let Some(gpui::ClipboardEntry::Image(img)) = clip.entries().first()
            && img.format == gpui::ImageFormat::Png
            && !img.bytes().is_empty()
        {
            return self.attach(Attachment { label: "Pasted image".into(), context: String::new(), png: Some(img.bytes().to_vec()) }, cx);
        }
        if let Some(t) = clip.text() {
            let t = t.replace("\r\n", "\n").replace('\r', "\n");
            self.replace(&t, cx);
        }
    }
    fn a_undo(&mut self, _: &edit::Undo, _: &mut Window, cx: &mut Context<Self>) {
        let now = self.snapshot();
        if let Some(s) = self.history.undo(now) {
            self.restore(s, cx);
        }
    }
    fn a_redo(&mut self, _: &edit::Redo, _: &mut Window, cx: &mut Context<Self>) {
        let now = self.snapshot();
        if let Some(s) = self.history.redo(now) {
            self.restore(s, cx);
        }
    }

    fn shape(&self, width: Pixels, window: &mut Window, look: &Look) -> (SmallVec<[WrappedLine; 1]>, bool) {
        let ui = &look.theme.ui;
        let (text, color, placeholder) = if self.text.is_empty() {
            (self.placeholder.clone(), ui.text_muted, true)
        } else {
            (SharedString::from(self.text.clone()), ui.text, false)
        };
        let run = TextRun {
            len: text.len(),
            font: font(look.ui_font.clone()),
            color: look.c(color),
            background_color: (self.select_all && !placeholder).then(|| look.c(ui.selection)),
            underline: None,
            strikethrough: None,
        };
        let lines = window.text_system().shape_text(text, look.text(0.), &[run], Some(width), None).unwrap_or_default();
        (lines, placeholder)
    }
}

impl Focusable for PromptInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for PromptInput {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let u = self.text[..self.caret].encode_utf16().count();
        Some(UTF16Selection { range: u..u, reversed: false })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if !text.is_empty() {
            self.replace(text, cx);
        }
    }
    fn replace_and_mark_text_in_range(&mut self, r: Option<Range<usize>>, text: &str, _: Option<Range<usize>>, w: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(r, text, w, cx)
    }
    fn bounds_for_range(&mut self, _: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }
    fn character_index_for_point(&mut self, _: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

struct PromptElement {
    input: Entity<PromptInput>,
}

impl IntoElement for PromptElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

pub struct Shaped {
    lines: SmallVec<[WrappedLine; 1]>,
    placeholder: bool,
    focused: bool,
}

impl Element for PromptElement {
    type RequestLayoutState = ();
    type PrepaintState = Shaped;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let look = Look::of(window, cx);
        let lh = look.text(0.) * 1.45;
        let inp = self.input.read(cx);
        let (lines, _) = inp.shape(inp.last_width, window, &look);
        let rows: usize = lines.iter().map(|l| l.wrap_boundaries().len() + 1).sum::<usize>().clamp(1, MAX_LINES);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (lh * rows as f32).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Shaped {
        let look = Look::of(window, cx);
        let width = bounds.size.width;
        if self.input.read(cx).bounds != Some(bounds) {
            self.input.update(cx, |i, _| i.bounds = Some(bounds));
        }
        if self.input.read(cx).last_width != width {
            // Re-measure next frame at the real width.
            self.input.update(cx, |i, cx| {
                i.last_width = width;
                cx.notify();
            });
        }
        let inp = self.input.read(cx);
        let (lines, placeholder) = inp.shape(width, window, &look);
        Shaped { lines, placeholder, focused: inp.focus.is_focused(window) }
    }
    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), s: &mut Shaped, window: &mut Window, cx: &mut App) {
        let look = Look::of(window, cx);
        let lh = look.text(0.) * 1.45;
        let inp = self.input.read(cx);
        let focus = inp.focus.clone();
        let caret = inp.caret;
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        // Where the caret is, relative to the top of the text.
        let mut y = px(0.);
        let mut start = 0usize;
        let mut caret_at = None;
        for line in &s.lines {
            let len = line.len();
            if !s.placeholder && caret >= start && caret <= start + len && caret_at.is_none() {
                caret_at = line.position_for_index(caret - start, lh).map(|p| point(p.x, y + p.y));
            }
            y += lh * (line.wrap_boundaries().len() + 1) as f32;
            start += len + 1; // the '\n'
        }
        // Past MAX_LINES the text scrolls inside the field (wheel, or to keep
        // the caret in view after an edit) and is clipped to it rather than
        // drawn over what's below.
        let caret_at = caret_at.unwrap_or_default();
        let max_scroll = (y - bounds.size.height).max(px(0.));
        let mut scroll = inp.scroll_y;
        let now = (caret, inp.text.len());
        if now != inp.painted {
            scroll = scroll.min(caret_at.y).max(caret_at.y + lh - bounds.size.height);
        }
        let scroll = scroll.clamp(px(0.), max_scroll);
        self.input.update(cx, |i, _| {
            i.scroll_y = scroll;
            i.max_scroll = max_scroll;
            i.painted = now;
        });
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let mut y = bounds.top() - scroll;
            for line in &s.lines {
                let h = lh * (line.wrap_boundaries().len() + 1) as f32;
                if y + h > bounds.top() && y < bounds.bottom() {
                    line.paint(point(bounds.left(), y), lh, gpui::TextAlign::Left, None, window, cx).ok();
                }
                y += h;
            }
            if s.focused {
                let at = point(bounds.left() + caret_at.x, bounds.top() + caret_at.y - scroll);
                window.paint_quad(fill(Bounds::new(point(at.x, at.y + px(3.)), size(px(1.5), lh - px(6.))), look.c(look.theme.editor.cursor)));
            }
        });
    }
}

impl Render for PromptInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let chips = (!self.attachments.is_empty()).then(|| {
            div().flex().flex_wrap().gap(look.z(4.)).pb(look.z(6.)).children(self.attachments.iter().enumerate().map(|(i, a)| {
                div()
                    .id(("attachment", i))
                    .flex()
                    .items_center()
                    .gap(look.z(4.))
                    .px(look.z(6.))
                    .py(look.z(1.))
                    .rounded(look.z(4.))
                    .bg(look.c(ui.active_bg))
                    .text_size(look.text(-2.))
                    .text_color(look.c(ui.text_secondary))
                    .child(if a.png.is_some() { "▣" } else { "❝" })
                    .child(div().max_w(look.z(220.)).truncate().child(a.label.clone()))
                    .child(
                        div()
                            .id(("attachment-x", i))
                            .cursor_pointer()
                            .text_color(look.c(ui.text_muted))
                            .child("×")
                            .on_click(cx.listener(move |t, _, _, cx| {
                                if i < t.attachments.len() {
                                    t.attachments.remove(i);
                                }
                                cx.notify();
                            })),
                    )
            }))
        });
        div()
            .key_context("TextInput")
            .track_focus(&self.focus)
            .w_full()
            .cursor_text()
            // Clicking the field focuses it (else Cmd+V and typing went to
            // whatever had focus before: the chat, the browser).
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.focus, cx);
                cx.notify();
            }))
            // Long text scrolls inside the field; short text lets the wheel
            // through to the chat.
            .on_scroll(cx.listener(|this, ev: &gpui::ScrollEvent, window, cx| {
                if this.max_scroll <= px(0.) {
                    return;
                }
                let d = ev.delta.pixel_delta(Look::of(window, cx).text(0.) * 1.45).y;
                this.scroll_y = (this.scroll_y - d).clamp(px(0.), this.max_scroll);
                cx.stop_propagation();
                cx.notify();
            }))
            .children(chips)
            .on_action(cx.listener(Self::a_backspace))
            .on_action(cx.listener(Self::a_delete))
            .on_action(cx.listener(Self::a_del_word))
            .on_action(cx.listener(Self::a_del_start))
            .on_action(cx.listener(Self::a_left))
            .on_action(cx.listener(Self::a_right))
            .on_action(cx.listener(Self::a_word_left))
            .on_action(cx.listener(Self::a_word_right))
            .on_action(cx.listener(Self::a_home))
            .on_action(cx.listener(Self::a_end))
            .on_action(cx.listener(Self::a_up))
            .on_action(cx.listener(Self::a_down))
            .on_action(cx.listener(Self::a_submit))
            .on_action(cx.listener(Self::a_newline))
            .on_action(cx.listener(Self::a_submit_cmd))
            .on_action(cx.listener(Self::a_cancel))
            .on_action(cx.listener(Self::a_tab))
            .on_action(cx.listener(Self::a_select_all))
            .on_action(cx.listener(Self::a_copy))
            .on_action(cx.listener(Self::a_cut))
            .on_action(cx.listener(Self::a_paste))
            .on_action(cx.listener(Self::a_undo))
            .on_action(cx.listener(Self::a_redo))
            .on_drop(cx.listener(|this, f: &DraggedFile, window, cx| {
                window.focus(&this.focus, cx);
                this.insert_mention(f.abs.clone(), cx);
            }))
            .child(PromptElement { input: cx.entity() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(text: &str) -> Snapshot {
        Snapshot { text: text.into(), caret: text.len(), mentions: Vec::new() }
    }

    /// Apply typing `c` to `cur` the way the field does.
    fn type_char(h: &mut EditHistory, cur: &mut Snapshot, c: char, at: Instant) {
        h.record(cur.clone(), EditKind::Type, cur.caret + c.len_utf8(), at);
        cur.text.push(c);
        cur.caret += c.len_utf8();
    }

    #[test]
    fn typing_coalesces_into_one_step() {
        let mut h = EditHistory::default();
        let t0 = Instant::now();
        let mut cur = snap("");
        for (i, c) in "hello".chars().enumerate() {
            type_char(&mut h, &mut cur, c, t0 + Duration::from_millis(100 * i as u64));
        }
        assert_eq!(h.undo.len(), 1);
        let back = h.undo(cur.clone()).unwrap();
        assert_eq!(back.text, "");
        // Redo brings the whole word back.
        assert_eq!(h.redo(back).unwrap().text, "hello");
    }

    #[test]
    fn pause_caret_move_or_kind_change_starts_a_step() {
        let mut h = EditHistory::default();
        let t0 = Instant::now();
        let mut cur = snap("");
        type_char(&mut h, &mut cur, 'a', t0);
        // Past the window: a new step.
        type_char(&mut h, &mut cur, 'b', t0 + GROUP_WINDOW * 2);
        assert_eq!(h.undo.len(), 2);
        // A caret move ends the group.
        h.break_group();
        type_char(&mut h, &mut cur, 'c', t0 + GROUP_WINDOW * 2);
        assert_eq!(h.undo.len(), 3);
        // Deleting after typing is its own step; a paste always is.
        h.record(cur.clone(), EditKind::Delete, cur.caret - 1, t0 + GROUP_WINDOW * 2);
        cur.text.pop();
        cur.caret -= 1;
        assert_eq!(h.undo.len(), 4);
        h.record(cur.clone(), EditKind::Other, cur.caret + 3, t0 + GROUP_WINDOW * 2);
        h.record(cur.clone(), EditKind::Other, cur.caret + 3, t0 + GROUP_WINDOW * 2);
        assert_eq!(h.undo.len(), 6);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut h = EditHistory::default();
        let t0 = Instant::now();
        let mut cur = snap("");
        type_char(&mut h, &mut cur, 'a', t0);
        let back = h.undo(cur.clone()).unwrap();
        assert_eq!(h.redo.len(), 1);
        let mut cur = back;
        type_char(&mut h, &mut cur, 'z', t0);
        assert!(h.redo.is_empty());
        assert!(h.redo(cur).is_none());
    }
}
