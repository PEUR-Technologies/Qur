//! Command palette: fuzzy filter over every command in `commands::COMMANDS`.

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, uniform_list,
};
use spike_common::qur_dark;

use crate::commands::{self, Cmd};
use crate::ui::{InputEvent, TextInput, Ui, hsla};

pub enum PaletteEvent {
    Run(&'static Cmd),
    Dismiss,
}

pub struct Palette {
    pub input: Entity<TextInput>,
    all: Vec<&'static Cmd>,
    matches: Vec<&'static Cmd>,
    selected: usize,
    scroll: UniformListScrollHandle,
    _sub: Subscription,
}

impl EventEmitter<PaletteEvent> for Palette {}

/// Subsequence match; higher is better. Consecutive and word-start hits score more.
fn score(hay: &str, needle: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = hay.to_lowercase().chars().collect();
    let mut s = 0;
    let mut hi = 0;
    let mut prev_hit = false;
    for nc in needle.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let mut found = false;
        while hi < hay.len() {
            let hc = hay[hi];
            hi += 1;
            if hc == nc {
                let word_start = hi == 1 || !hay[hi - 2].is_alphanumeric();
                s += 1 + if prev_hit { 3 } else { 0 } + if word_start { 4 } else { 0 };
                prev_hit = true;
                found = true;
                break;
            }
            prev_hit = false;
        }
        if !found {
            return None;
        }
    }
    Some(s - hay.len() as i32 / 8)
}

impl Palette {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Type a command…", cx));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| match ev {
            InputEvent::Changed => this.filter(cx),
            InputEvent::Confirm { .. } => {
                if let Some(c) = this.matches.get(this.selected) {
                    if c.implemented() {
                        cx.emit(PaletteEvent::Run(c));
                    }
                }
            }
            InputEvent::Cancel => cx.emit(PaletteEvent::Dismiss),
            InputEvent::Up => this.step(-1, cx),
            InputEvent::Down => this.step(1, cx),
        });
        let all: Vec<_> = commands::palette_commands().collect();
        let mut p = Palette {
            input,
            matches: all.clone(),
            all,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            _sub: sub,
        };
        p.filter(cx);
        p
    }

    fn filter(&mut self, cx: &mut Context<Self>) {
        let q = self.input.read(cx).text.clone();
        let mut scored: Vec<(i32, &'static Cmd)> = self
            .all
            .iter()
            .filter_map(|c| score(&format!("{}: {}", c.category, c.title), &q).map(|s| (s + if c.implemented() { 2 } else { 0 }, *c)))
            .collect();
        if !q.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0));
        }
        self.matches = scored.into_iter().map(|(_, c)| c).collect();
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn step(&mut self, d: isize, cx: &mut Context<Self>) {
        let n = self.matches.len() as isize;
        if n == 0 {
            return;
        }
        self.selected = ((self.selected as isize + d).rem_euclid(n)) as usize;
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Center);
        cx.notify();
    }
}

impl Focusable for Palette {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.input.read(cx).focus.clone()
    }
}

impl Render for Palette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let rows = self.matches.len().min(12);
        div()
            .key_context("Palette")
            .w(ui.z(560.))
            .flex()
            .flex_col()
            .bg(hsla(qur_dark::SIDEBAR))
            .border_1()
            .border_color(hsla(qur_dark::BORDER))
            .rounded(ui.z(6.))
            .overflow_hidden()
            .child(
                div()
                    .px(ui.z(10.))
                    .py(ui.z(8.))
                    .border_b_1()
                    .border_color(hsla(qur_dark::BORDER))
                    .child(self.input.clone()),
            )
            .child(
                uniform_list(
                    "palette",
                    self.matches.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        let ui = Ui::get(cx);
                        range
                            .map(|i| {
                                let c = this.matches[i];
                                let sel = i == this.selected;
                                let color = if c.implemented() { qur_dark::TEXT } else { qur_dark::TEXT_MUTED };
                                let right = if c.implemented() {
                                    commands::display_keys(c.first_keys())
                                } else {
                                    let tier = match c.tier {
                                        commands::Tier::Later(t) => t,
                                        _ => "",
                                    };
                                    format!("not in spike ({tier})")
                                };
                                div()
                                    .id(i)
                                    .h(ui.z(26.))
                                    .px(ui.z(10.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(ui.z(12.))
                                    .text_size(ui.z(13.))
                                    .when(sel, |d| d.bg(hsla(qur_dark::SELECTION)))
                                    .child(div().truncate().text_color(hsla(color)).child(format!("{}: {}", c.category, c.title)))
                                    .child(div().flex_none().text_size(ui.z(11.)).text_color(hsla(qur_dark::TEXT_MUTED)).child(right))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.selected = i;
                                        let c = this.matches[i];
                                        if c.implemented() {
                                            cx.emit(PaletteEvent::Run(c));
                                        }
                                        cx.notify();
                                    }))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .h(ui.z(26.) * rows as f32),
            )
    }
}
