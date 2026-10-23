//! A small single-line text field (find, replace, go to line, pickers).

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId, Pixels, ShapedLine, SharedString,
    Style, TextRun, UTF16Selection, Window, div, fill, font, point, prelude::*, px, relative, size,
};

use crate::commands::{edit, input};
use crate::look::Look;

pub enum InputEvent {
    Changed,
    /// Enter; `alt` = Shift+Enter, `all` = Cmd+Enter.
    Confirm { alt: bool, all: bool },
    Cancel,
    Up,
    Down,
    Tab,
}

pub struct TextInput {
    pub focus: FocusHandle,
    pub text: String,
    /// Byte offset of the caret; `select_all` highlights everything.
    caret: usize,
    select_all: bool,
    placeholder: SharedString,
}

impl EventEmitter<InputEvent> for TextInput {}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        TextInput { focus: cx.focus_handle(), text: String::new(), caret: 0, select_all: false, placeholder: placeholder.into() }
    }

    /// Replace the text and select it all (typing replaces it).
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.text = text.to_string();
        self.caret = self.text.len();
        self.select_all = !text.is_empty();
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn replace(&mut self, new: &str, cx: &mut Context<Self>) {
        if self.select_all {
            self.text.clear();
            self.caret = 0;
            self.select_all = false;
        }
        self.text.insert_str(self.caret, new);
        self.caret += new.len();
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn delete_range(&mut self, r: Range<usize>, cx: &mut Context<Self>) {
        if self.select_all {
            return self.replace("", cx);
        }
        if r.is_empty() {
            return;
        }
        self.text.replace_range(r.clone(), "");
        self.caret = r.start;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn prev(&self) -> usize {
        self.text[..self.caret].char_indices().last().map(|(i, _)| i).unwrap_or(0)
    }

    fn next(&self) -> usize {
        self.text[self.caret..].chars().next().map(|c| self.caret + c.len_utf8()).unwrap_or(self.caret)
    }

    fn word_left_of(&self, from: usize) -> usize {
        let s = &self.text[..from];
        let trimmed = s.trim_end_matches(|c: char| !is_word(c));
        trimmed.trim_end_matches(is_word).len()
    }

    fn word_right_of(&self, from: usize) -> usize {
        let s = &self.text[from..];
        let skip = s.len() - s.trim_start_matches(|c: char| !is_word(c)).len();
        let rest = &s[skip..];
        from + skip + (rest.len() - rest.trim_start_matches(is_word).len())
    }

    fn mv(&mut self, to: usize, cx: &mut Context<Self>) {
        self.select_all = false;
        self.caret = to;
        cx.notify();
    }

    fn backspace(&mut self, _: &input::Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.prev();
        self.delete_range(p..self.caret, cx)
    }
    fn delete(&mut self, _: &input::Delete, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.next();
        self.delete_range(self.caret..n, cx)
    }
    fn delete_word_left(&mut self, _: &input::DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.word_left_of(self.caret);
        self.delete_range(p..self.caret, cx)
    }
    fn delete_to_start(&mut self, _: &input::DeleteToStart, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_range(0..self.caret, cx)
    }
    fn left(&mut self, _: &input::Left, _: &mut Window, cx: &mut Context<Self>) {
        let p = if self.select_all { 0 } else { self.prev() };
        self.mv(p, cx)
    }
    fn right(&mut self, _: &input::Right, _: &mut Window, cx: &mut Context<Self>) {
        let n = if self.select_all { self.text.len() } else { self.next() };
        self.mv(n, cx)
    }
    fn word_left(&mut self, _: &input::WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let p = self.word_left_of(self.caret);
        self.mv(p, cx)
    }
    fn word_right(&mut self, _: &input::WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.word_right_of(self.caret);
        self.mv(n, cx)
    }
    fn home(&mut self, _: &input::Home, _: &mut Window, cx: &mut Context<Self>) {
        self.mv(0, cx)
    }
    fn end(&mut self, _: &input::End, _: &mut Window, cx: &mut Context<Self>) {
        let n = self.text.len();
        self.mv(n, cx)
    }
    fn confirm(&mut self, _: &input::Confirm, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Confirm { alt: false, all: false })
    }
    fn confirm_alt(&mut self, _: &input::ConfirmAlt, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Confirm { alt: true, all: false })
    }
    fn confirm_all(&mut self, _: &input::ConfirmAll, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Confirm { alt: false, all: true })
    }
    fn cancel(&mut self, _: &input::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancel)
    }
    fn up(&mut self, _: &input::Up, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Up)
    }
    fn down(&mut self, _: &input::Down, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Down)
    }
    fn tab(&mut self, _: &input::Tab, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Tab)
    }
    fn select_all(&mut self, _: &edit::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_text(cx);
    }
    /// Select the whole text (Cmd+L in the browser's address bar).
    pub fn select_text(&mut self, cx: &mut Context<Self>) {
        self.select_all = !self.text.is_empty();
        cx.notify();
    }
    fn copy(&mut self, _: &edit::Copy, _: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.text.clone()));
    }
    fn cut(&mut self, _: &edit::Cut, _: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.text.clone()));
        self.select_all = true;
        self.replace("", cx);
    }
    fn paste(&mut self, _: &edit::Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
            let line = t.lines().next().unwrap_or("").to_string();
            self.replace(&line, cx);
        }
    }
    // Undo/Redo in a one-line field: swallow so they don't reach the editor.
    fn undo(&mut self, _: &edit::Undo, _: &mut Window, _: &mut Context<Self>) {}
    fn redo(&mut self, _: &edit::Redo, _: &mut Window, _: &mut Context<Self>) {}
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TextInput {
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
        if !text.is_empty() && !text.contains('\n') {
            self.replace(text, cx);
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
        self.replace_text_in_range(r, text, window, cx)
    }
    fn bounds_for_range(&mut self, _: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }
    fn character_index_for_point(&mut self, _: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

struct InputElement {
    input: Entity<TextInput>,
}

impl IntoElement for InputElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for InputElement {
    type RequestLayoutState = ();
    type PrepaintState = (ShapedLine, Pixels, bool);

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let look = Look::of(window, cx);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (look.text(0.) * 1.5).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> (ShapedLine, Pixels, bool) {
        let look = Look::of(window, cx);
        let ui = &look.theme.ui;
        let inp = self.input.read(cx);
        let (text, color) = if inp.text.is_empty() {
            (inp.placeholder.clone(), ui.text_muted)
        } else {
            (SharedString::from(inp.text.clone()), ui.text)
        };
        let run = TextRun {
            len: text.len(),
            font: font(look.ui_font.clone()),
            color: look.c(color),
            background_color: (inp.select_all && !inp.text.is_empty()).then(|| look.c(ui.selection)),
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(text, look.text(0.), &[run], None);
        let caret = if inp.text.is_empty() { px(0.) } else { line.x_for_index(inp.caret) };
        (line, caret, inp.focus.is_focused(window))
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        pre: &mut (ShapedLine, Pixels, bool),
        window: &mut Window,
        cx: &mut App,
    ) {
        let look = Look::of(window, cx);
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        pre.0.paint(bounds.origin, bounds.size.height, gpui::TextAlign::Left, None, window, cx).ok();
        if pre.2 {
            window.paint_quad(fill(
                Bounds::new(point(bounds.left() + pre.1, bounds.top() + px(3.)), size(px(1.5), bounds.size.height - px(6.))),
                look.c(look.theme.editor.cursor),
            ));
        }
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("TextInput")
            .track_focus(&self.focus)
            .w_full()
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_to_start))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::confirm_alt))
            .on_action(cx.listener(Self::confirm_all))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .child(InputElement { input: cx.entity() })
    }
}
