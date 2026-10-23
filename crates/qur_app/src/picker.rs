//! One picker for the command palette, Quick Open, the theme picker and the
//! project switcher. It only filters and selects; the workspace decides what
//! an item means (events below).

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, uniform_list,
};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

use crate::input::{InputEvent, TextInput};
use crate::look::Look;

#[derive(Clone)]
pub struct PickItem {
    pub label: SharedString,
    /// Shown dimmed after the label (a folder path, a group).
    pub detail: Option<SharedString>,
    /// Right-aligned (a shortcut, "M2").
    pub right: Option<SharedString>,
    pub enabled: bool,
}

impl PickItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        PickItem { label: label.into(), detail: None, right: None, enabled: true }
    }
    pub fn detail(mut self, d: impl Into<SharedString>) -> Self {
        self.detail = Some(d.into());
        self
    }
    pub fn right(mut self, r: impl Into<SharedString>) -> Self {
        self.right = Some(r.into());
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
}

pub enum PickerEvent {
    /// Index into the original `items`.
    Confirm(usize),
    /// The highlighted item changed (theme live preview).
    Highlight(usize),
    Tab,
    Dismiss,
}

pub struct Picker {
    pub input: Entity<TextInput>,
    items: Vec<PickItem>,
    /// Text matched for each item (label + detail, or a path).
    haystack: Vec<String>,
    matches: Vec<usize>,
    selected: usize,
    /// Optional line shown above the list (theme scope toggle).
    pub header: Option<SharedString>,
    match_paths: bool,
    scroll: UniformListScrollHandle,
    matcher: Matcher,
    _sub: Subscription,
}

impl EventEmitter<PickerEvent> for Picker {}

const MAX_ROWS: usize = 12;

impl Picker {
    pub fn new(placeholder: &str, items: Vec<PickItem>, match_paths: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new(placeholder.to_string(), cx));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| match ev {
            InputEvent::Changed => this.filter(cx),
            InputEvent::Confirm { .. } => {
                if let Some(&i) = this.matches.get(this.selected)
                    && this.items[i].enabled
                {
                    cx.emit(PickerEvent::Confirm(i));
                }
            }
            InputEvent::Cancel => cx.emit(PickerEvent::Dismiss),
            InputEvent::Up => this.step(-1, cx),
            InputEvent::Down => this.step(1, cx),
            InputEvent::Tab => cx.emit(PickerEvent::Tab),
        });
        let haystack = items
            .iter()
            .map(|it| match &it.detail {
                Some(d) if match_paths => format!("{d}/{}", it.label),
                Some(d) => format!("{} {d}", it.label),
                None => it.label.to_string(),
            })
            .collect();
        let mut p = Picker {
            input,
            matches: (0..items.len()).collect(),
            items,
            haystack,
            selected: 0,
            header: None,
            match_paths,
            scroll: UniformListScrollHandle::new(),
            matcher: Matcher::new(if match_paths { Config::DEFAULT.match_paths() } else { Config::DEFAULT }),
            _sub: sub,
        };
        p.filter(cx);
        p
    }

    /// Start with item `i` highlighted (current theme).
    pub fn select_item(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(pos) = self.matches.iter().position(|&m| m == i) {
            self.selected = pos;
            self.scroll.scroll_to_item(pos, ScrollStrategy::Center);
            cx.notify();
        }
    }

    fn filter(&mut self, cx: &mut Context<Self>) {
        let q = self.input.read(cx).text.trim().to_string();
        let before = self.matches.get(self.selected).copied();
        if q.is_empty() {
            self.matches = (0..self.items.len()).collect();
        } else {
            let pat = Pattern::parse(&q, CaseMatching::Smart, Normalization::Smart);
            let mut buf = Vec::new();
            let mut scored: Vec<(usize, u32)> = self
                .haystack
                .iter()
                .enumerate()
                .filter_map(|(i, h)| {
                    let hs = nucleo_matcher::Utf32Str::new(h, &mut buf);
                    pat.score(hs, &mut self.matcher).map(|s| (i, s + if self.items[i].enabled { 1 } else { 0 }))
                })
                .collect();
            // Stable: equal scores keep the caller's order (recent first).
            scored.sort_by_key(|&(_, s)| std::cmp::Reverse(s));
            self.matches = scored.into_iter().map(|(i, _)| i).collect();
        }
        let _ = self.match_paths;
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        if let Some(&i) = self.matches.first()
            && Some(i) != before
        {
            cx.emit(PickerEvent::Highlight(i));
        }
        cx.notify();
    }

    fn step(&mut self, d: isize, cx: &mut Context<Self>) {
        let n = self.matches.len() as isize;
        if n == 0 {
            return;
        }
        self.selected = (self.selected as isize + d).rem_euclid(n) as usize;
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Center);
        cx.emit(PickerEvent::Highlight(self.matches[self.selected]));
        cx.notify();
    }
}

impl Focusable for Picker {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.input.read(cx).focus.clone()
    }
}

impl Render for Picker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let row_h = look.z(26.);
        let rows = self.matches.len().min(MAX_ROWS);
        div()
            .key_context("Picker")
            .w(look.z(560.))
            .flex()
            .flex_col()
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(6.))
            .overflow_hidden()
            .text_size(look.text(0.))
            .child(
                div()
                    .px(look.z(10.))
                    .py(look.z(6.))
                    .border_b_1()
                    .border_color(look.c(ui.border))
                    .child(self.input.clone()),
            )
            .children(self.header.clone().map(|h| {
                div()
                    .px(look.z(10.))
                    .py(look.z(4.))
                    .text_size(look.text(-2.))
                    .text_color(look.c(ui.text_muted))
                    .border_b_1()
                    .border_color(look.c(ui.border))
                    .child(h)
            }))
            .when(self.matches.is_empty(), |d| {
                d.child(div().px(look.z(10.)).py(look.z(6.)).text_color(look.c(ui.text_muted)).child("No matches"))
            })
            .child(
                uniform_list(
                    "picker",
                    self.matches.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                        let look = Look::of(window, cx);
                        let ui = &look.theme.ui;
                        range
                            .map(|pos| {
                                let it = &this.items[this.matches[pos]];
                                let sel = pos == this.selected;
                                div()
                                    .id(pos)
                                    .w_full()
                                    .h(look.z(26.))
                                    .px(look.z(10.))
                                    .flex()
                                    .items_center()
                                    .gap(look.z(8.))
                                    .when(sel, |d| d.bg(look.c(ui.active_bg)))
                                    .child(
                                        div()
                                            .flex_none()
                                            .max_w(look.z(360.))
                                            .truncate()
                                            .text_color(look.c(if it.enabled { ui.text } else { ui.text_muted }))
                                            .child(it.label.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(gpui::px(0.))
                                            .truncate()
                                            .text_size(look.text(-2.))
                                            .text_color(look.c(ui.text_muted))
                                            .children(it.detail.clone()),
                                    )
                                    .children(it.right.clone().map(|r| {
                                        div().flex_none().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(r)
                                    }))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.selected = pos;
                                        let i = this.matches[pos];
                                        if this.items[i].enabled {
                                            cx.emit(PickerEvent::Confirm(i));
                                        }
                                        cx.notify();
                                    }))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .h(row_h * rows as f32),
            )
    }
}
