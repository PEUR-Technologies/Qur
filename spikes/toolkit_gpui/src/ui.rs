//! Shared UI state (zoom, transparency) and a small single-line text field.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, Global, GlobalElementId, Hsla,
    LayoutId, Pixels, ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window, div, fill,
    font, point, prelude::*, px, relative, rgb, size,
};
use spike_common::qur_dark;

use crate::commands::{edit, input};

pub const CODE_FONT: &str = "Menlo";

/// Window-wide look settings. The real app stores these in config.toml
/// (`[window] opacity = 0.85, blur = true`, plus `ui_zoom`).
#[derive(Clone, Copy)]
pub struct Ui {
    pub zoom: f32,
    /// 0.5..=1.0; only large background fills use it.
    pub opacity: f32,
    pub blur: bool,
}

impl Global for Ui {}

impl Ui {
    pub fn get(cx: &App) -> Ui {
        *cx.global::<Ui>()
    }

    /// A UI length scaled by zoom.
    pub fn z(&self, v: f32) -> Pixels {
        px(v * self.zoom)
    }

    /// A large background fill: translucent when opacity < 1.
    pub fn bg(&self, c: u32) -> Hsla {
        let mut h: Hsla = rgb(c).into();
        h.a = self.opacity;
        h
    }

    pub fn background_appearance(&self) -> gpui::WindowBackgroundAppearance {
        if self.opacity >= 1.0 {
            gpui::WindowBackgroundAppearance::Opaque
        } else if self.blur {
            gpui::WindowBackgroundAppearance::Blurred
        } else {
            gpui::WindowBackgroundAppearance::Transparent
        }
    }
}

pub fn hsla(c: u32) -> Hsla {
    rgb(c).into()
}

// ---------------------------------------------------------------------------
// TextInput
// ---------------------------------------------------------------------------

pub enum InputEvent {
    Changed,
    /// Enter (`alt` = Shift+Enter).
    Confirm { alt: bool, all: bool },
    Cancel,
    Up,
    Down,
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

impl TextInput {
    pub fn new(placeholder: &str, cx: &mut Context<Self>) -> Self {
        TextInput {
            focus: cx.focus_handle(),
            text: String::new(),
            caret: 0,
            select_all: false,
            placeholder: SharedString::from(placeholder.to_string()),
        }
    }

    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.text = text.to_string();
        self.caret = self.text.len();
        self.select_all = true;
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

    fn prev(&self) -> usize {
        self.text[..self.caret].char_indices().last().map(|(i, _)| i).unwrap_or(0)
    }

    fn next(&self) -> usize {
        self.text[self.caret..].chars().next().map(|c| self.caret + c.len_utf8()).unwrap_or(self.caret)
    }

    fn backspace(&mut self, _: &input::Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.select_all {
            return self.replace("", cx);
        }
        let p = self.prev();
        self.text.replace_range(p..self.caret, "");
        self.caret = p;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn delete(&mut self, _: &input::Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.select_all {
            return self.replace("", cx);
        }
        let n = self.next();
        self.text.replace_range(self.caret..n, "");
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn mv(&mut self, to: usize, cx: &mut Context<Self>) {
        self.select_all = false;
        self.caret = to;
        cx.notify();
    }

    fn left(&mut self, _: &input::Left, _: &mut Window, cx: &mut Context<Self>) {
        let p = if self.select_all { 0 } else { self.prev() };
        self.mv(p, cx)
    }
    fn right(&mut self, _: &input::Right, _: &mut Window, cx: &mut Context<Self>) {
        let n = if self.select_all { self.text.len() } else { self.next() };
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
    fn select_all(&mut self, _: &edit::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all = true;
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
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let u = self.text[..self.caret].encode_utf16().count();
        Some(UTF16Selection { range: u..u, reversed: false })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }
    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
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
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let ui = Ui::get(cx);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = ui.z(20.).into();
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
        let ui = Ui::get(cx);
        let inp = self.input.read(cx);
        let (text, color) = if inp.text.is_empty() {
            (inp.placeholder.clone(), qur_dark::TEXT_MUTED)
        } else {
            (SharedString::from(inp.text.clone()), qur_dark::TEXT)
        };
        let run = TextRun {
            len: text.len(),
            font: font(".SystemUIFont"),
            color: hsla(color),
            background_color: if inp.select_all && !inp.text.is_empty() {
                Some(hsla(qur_dark::SELECTION))
            } else {
                None
            },
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(text, ui.z(13.), &[run], None);
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
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        pre.0
            .paint(bounds.origin, bounds.size.height, gpui::TextAlign::Left, None, window, cx)
            .ok();
        if pre.2 {
            window.paint_quad(fill(
                Bounds::new(point(bounds.left() + pre.1, bounds.top() + px(2.)), size(px(1.5), bounds.size.height - px(4.))),
                hsla(qur_dark::TEXT),
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
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::confirm_alt))
            .on_action(cx.listener(Self::confirm_all))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .child(InputElement { input: cx.entity() })
    }
}
