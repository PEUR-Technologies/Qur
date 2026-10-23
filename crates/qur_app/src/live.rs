//! Live view (handoff section 8, decision 25): the file the agent is working
//! in, following it across files. Read-only while the agent edits it, with
//! "<agent> is editing · Take over". Follow is on by default but never fights
//! the user: no jump within 5 s of a scroll or click in the view.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{Context, Entity, EventEmitter, Subscription, Window, div, prelude::*, px};
use qur_heat::HeatState;
use qur_theme::Rgba;

use crate::editor_view::EditorView;
use crate::look::Look;
use crate::monitor::{MonitorEvent, TaskMonitor};

/// "Do not fight the user" window.
const USER_GRACE: Duration = Duration::from_secs(5);

pub enum LiveEvent {
    /// Take over: the owner stops the agent's turn.
    TakeOver,
    /// Open the current file in the code modal (full editing).
    Open(PathBuf),
}

pub struct LivePane {
    pub monitor: Entity<TaskMonitor>,
    agent: String,
    pub accent: Rgba,
    editor: Option<Entity<EditorView>>,
    current: Option<PathBuf>,
    pub follow: bool,
    taken_over: bool,
    editing: bool,
    wrap: bool,
    tab: usize,
    _sub: Subscription,
}

impl EventEmitter<LiveEvent> for LivePane {}

impl LivePane {
    pub fn new(monitor: Entity<TaskMonitor>, agent: String, wrap: bool, tab: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe_in(&monitor, window, |this, _, ev: &MonitorEvent, _, cx| match ev {
            MonitorEvent::Changed => this.sync(cx),
            MonitorEvent::Files(paths) => this.on_files(paths, cx),
        });
        let accent = monitor.read(cx).accent;
        let mut p = LivePane { monitor, agent, accent, editor: None, current: None, follow: true, taken_over: false, editing: false, wrap, tab, _sub: sub };
        p.sync(cx);
        p
    }

    fn user_busy(&self, cx: &gpui::App) -> bool {
        self.editor.as_ref().is_some_and(|e| e.read(cx).user_active_within(USER_GRACE))
    }

    /// Follow the agent's focus; refresh the editing state.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let (focus, accent) = {
            let m = self.monitor.read(cx);
            (m.heat.focus(), m.accent)
        };
        self.accent = accent;
        if let Some((path, line)) = focus {
            let switch = self.current.as_deref() != Some(path.as_path());
            if switch && (self.current.is_none() || (self.follow && !self.user_busy(cx))) {
                self.show(path.clone(), cx);
            }
            if self.current.as_deref() == Some(path.as_path())
                && let (Some(l), Some(e)) = (line, &self.editor)
                && self.follow
                && !self.user_busy(cx)
            {
                let l = (l as usize).saturating_sub(1);
                e.update(cx, |e, cx| e.reveal_lines(l..l + 1, cx));
            }
        }
        let editing = self.current.as_ref().is_some_and(|p| {
            let m = self.monitor.read(cx);
            m.rel(p).is_some_and(|rel| m.heat.cell(&rel, std::time::Instant::now()).state == HeatState::Editing)
        });
        if editing != self.editing {
            self.editing = editing;
            if editing {
                self.taken_over = false;
            }
        }
        let ro = self.editing && !self.taken_over;
        if let Some(e) = &self.editor {
            e.update(cx, |e, _| e.read_only = ro);
        }
        cx.notify();
    }

    fn show(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match EditorView::load(&path) {
            Ok(loaded) => {
                let (wrap, tab) = (self.wrap, self.tab);
                self.editor = Some(cx.new(|cx| EditorView::new(loaded, wrap, tab, cx)));
                self.current = Some(path);
                self.taken_over = false;
            }
            Err(_) => {
                // Deleted or not text (binary): keep the previous file.
            }
        }
        cx.notify();
    }

    fn on_files(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let (Some(cur), Some(e)) = (self.current.clone(), self.editor.clone()) else { return };
        if !paths.contains(&cur) {
            return;
        }
        let accent = self.accent;
        let follow = self.follow && !self.user_busy(cx);
        e.update(cx, |e, cx| {
            if let Some(range) = e.disk_change(cx) {
                e.agent_mark = Some((range.clone(), accent));
                if follow {
                    e.reveal_lines(range, cx);
                }
            }
        });
    }

    fn take_over(&mut self, cx: &mut Context<Self>) {
        self.taken_over = true;
        self.follow = false;
        if let Some(e) = &self.editor {
            e.update(cx, |e, _| e.read_only = false);
        }
        cx.emit(LiveEvent::TakeOver);
        cx.notify();
    }
}

impl Render for LivePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let wt = self.monitor.read(cx).worktree.clone();
        let rel = self.current.as_ref().map(|p| p.strip_prefix(&wt).unwrap_or(p).to_string_lossy().into_owned());
        let small_button = |label: &'static str, id: &'static str| {
            div()
                .id(id)
                .flex_none()
                .px(look.z(8.))
                .py(look.z(1.))
                .rounded(look.z(4.))
                .border_1()
                .border_color(look.c(ui.border_strong))
                .cursor_pointer()
                .text_size(look.text(-2.))
                .child(label)
        };
        let header = div()
            .flex_none()
            .h(look.z(30.))
            .px(look.z(12.))
            .flex()
            .items_center()
            .gap(look.z(8.))
            .border_b_1()
            .border_color(look.c(ui.border))
            .text_size(look.text(-1.))
            .child(div().flex_none().text_size(look.text(-2.)).font_weight(gpui::FontWeight::MEDIUM).text_color(look.c(ui.text_muted)).child("LIVE"))
            .child(div().min_w(px(0.)).truncate().text_color(look.c(ui.text)).child(rel.clone().unwrap_or_default()))
            .when(self.editing && !self.taken_over, |d| {
                d.child(div().flex_none().text_color(look.c(self.accent)).child(format!("{} is editing", self.agent)))
                    .child(small_button("Take over", "take-over").on_click(cx.listener(|this, _, _, cx| this.take_over(cx))))
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("follow")
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(look.z(4.))
                    .cursor_pointer()
                    .text_size(look.text(-2.))
                    .text_color(look.c(if self.follow { ui.text } else { ui.text_muted }))
                    .child(
                        div()
                            .size(look.z(10.))
                            .rounded(look.z(2.5))
                            .border_1()
                            .border_color(look.c(ui.text_secondary))
                            .when(self.follow, |d| d.bg(look.c(self.accent))),
                    )
                    .child("Follow")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.follow = !this.follow;
                        if this.follow {
                            this.sync(cx);
                        }
                        cx.notify();
                    })),
            )
            .when_some(self.current.clone(), |d, p| {
                d.child(small_button("Open", "live-open").on_click(cx.listener(move |_, _, _, cx| cx.emit(LiveEvent::Open(p.clone())))))
            });
        let body = match &self.editor {
            Some(e) => div().flex_1().min_h(px(0.)).child(e.clone()).into_any_element(),
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(look.c(ui.text_muted))
                .child(format!("Waiting for {} to open a file…", self.agent))
                .into_any_element(),
        };
        div().size_full().flex().flex_col().bg(look.bg(ui.editor_bg)).child(header).child(body)
    }
}
