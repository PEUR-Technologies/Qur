//! The stage's pieces around the chat: subagents, the notes column, the
//! queue, the header, and which view and layout show.

use super::*;

impl AgentTask {
    /// Show a subagent's work in place of the chat (`None`: back to the chat).
    pub fn show_subagent(&mut self, id: Option<SessionId>, cx: &mut Context<Self>) {
        if id.is_some() {
            self.open_review = false;
        }
        self.open_subagent = id;
        cx.notify();
    }

    /// Stop a running subagent (the agent grants `cancel` on its session).
    pub fn stop_subagent(&mut self, id: &SessionId, cx: &mut Context<Self>) {
        if let Some(c) = &self.client {
            c.cancel(id);
        }
        if let Some(s) = self.subagents.iter_mut().find(|s| s.id == *id) {
            s.state = "stopped".into();
        }
        cx.emit(AgentEvent::Subagents);
        cx.notify();
    }

    fn subagent_view(&self, sub: &Subagent, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let running = sub.state == "running";
        let id = sub.id.clone();
        let button = |label: &'static str, key: &'static str| {
            div().id(key).flex_none().px(look.z(8.)).py(look.z(2.)).rounded(look.z(4.)).cursor_pointer().bg(look.c(ui.button_bg)).text_color(look.c(ui.button_text)).child(label)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px(look.z(16.))
                    .py(look.z(8.))
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .border_b_1()
                    .border_color(look.c(ui.border))
                    .child(button("← Chat", "sub-back").on_click(cx.listener(|t, _, _, cx| t.show_subagent(None, cx))))
                    .child(div().flex_1().min_w(px(0.)).truncate().font_weight(gpui::FontWeight::MEDIUM).child(format!("↳ {}", sub.title)))
                    .child(div().flex_none().text_color(look.c(ui.text_muted)).child(sub.state.replace('_', " ")))
                    .when(running, |d| d.child(button("Stop", "sub-stop").on_click(cx.listener(move |t, _, _, cx| t.stop_subagent(&id, cx))))),
            )
            .child(
                div()
                    .id("sub-log")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .px(look.z(16.))
                    .py(look.z(8.))
                    .flex()
                    .flex_col()
                    .gap(look.z(6.))
                    .when(sub.log.is_empty(), |d| d.child(div().text_color(look.c(ui.text_muted)).child("Nothing from this subagent yet.")))
                    .children(sub.log.iter().enumerate().map(|(n, (tool, text))| {
                        if *tool {
                            div().text_color(look.c(ui.text_secondary)).child(format!("· {}", text.trim()))
                        } else {
                            self.markdown(text.trim(), format!("md-{}-{}-{n}", cx.entity_id(), sub.id), look, cx)
                        }
                    })),
            )
    }

    /// To-do + Chat + permission bar + prompt (the "side notes").
    pub(super) fn notes(&self, show_plan: bool, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        if self.open_review
            && let Some(r) = &self.reviewer
        {
            return self.review_view(r, look, cx);
        }
        if let Some(sub) = self.open_subagent.as_ref().and_then(|id| self.subagents.iter().find(|s| s.id == *id)) {
            return self.subagent_view(sub, look, cx);
        }
        let ui = look.theme.ui.clone();
        let me = cx.entity();
        let look2 = look.clone();
        // Sized here too, so nothing in a row falls back to GPUI's default size.
        let chat = list(self.list.clone(), move |ix, _, cx| me.update(cx, |t, cx| t.render_item(ix, &look2, cx))).size_full().text_size(look.text(0.));
        // Scrolled up: a "↓" to jump back to the newest message.
        let jump = (!self.list.is_following_tail() && self.list.is_scrolled_to_end() == Some(false)).then(|| {
            div()
                .id("chat-bottom")
                .absolute()
                .right(look.z(16.))
                .bottom(look.z(8.))
                .px(look.z(9.))
                .py(look.z(3.))
                .rounded(look.z(12.))
                .cursor_pointer()
                .border_1()
                .border_color(look.c(ui.border_strong))
                .bg(look.c(ui.overlay_bg))
                .text_size(look.text(0.))
                .text_color(look.c(ui.text_secondary))
                .hover(|d| d.text_color(look.c(ui.text)))
                .child("↓")
                .on_click(cx.listener(|t, _, _, cx| {
                    t.follow_bottom();
                    cx.notify();
                }))
        });
        let chat = div().flex_1().min_h(px(0.)).relative().child(chat).children(jump);
        let working = matches!(self.status, Status::Working | Status::NeedsYou);
        let empty = self.chat.is_empty();
        let prompt_bar = div()
            .flex_none()
            .mx(look.z(12.))
            .mb(look.z(12.))
            .px(look.z(12.))
            .py(look.z(8.))
            .rounded(look.z(6.))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .bg(look.c(ui.editor_bg))
            .child(self.prompt.clone());
        let busy = working || !self.pending.is_empty();
        // "24k / 1M · $0.31": context used of the window (warning colour
        // past 80 %, as Zed does), and the session's cost.
        let usage = self.usage.filter(|u| u.1 > 0).map(|(used, size, cost)| {
            let k = |n: u64| if n >= 1_000_000 { format!("{}M", (n as f64 / 1e6 * 10.).round() / 10.) } else { format!("{}k", n.div_ceil(1000)) };
            let text = format!("{} / {}{}", k(used), k(size), cost.map(|c| format!(" · ${c:.2}")).unwrap_or_default());
            let hot = used as f64 >= size as f64 * 0.8;
            div().flex_none().mr(look.z(6.)).when(hot, |d| d.text_color(look.c(ui.warning))).child(text)
        });
        // Just above the field, on the right: usage, then queue and Esc
        // hints while working. Settings are in the `/` menu, not here.
        let hints = (busy || usage.is_some()).then(|| {
            div()
                .flex_none()
                .mx(look.z(12.))
                .mb(look.z(3.))
                .flex()
                .justify_end()
                .items_center()
                .gap(look.z(2.))
                .text_size(look.text(0.))
                .text_color(look.c(ui.text_muted))
                .children(usage)
                .when(!self.pending.is_empty(), |d| d.child(div().flex_none().mr(look.z(6.)).text_color(look.c(ui.text_secondary)).child(format!("{} queued", self.pending.len()))))
                .when(working, |d| d.child(div().min_w(px(0.)).mr(look.z(2.)).truncate().child("esc to stop")))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(show_plan, |d| d.children(self.plan_strip(look)))
            .children(self.find_bar(look, cx))
            .when(empty, |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px(look.z(16.))
                        .text_color(look.c(ui.text_muted))
                        .child(match self.status {
                            _ if self.setting_up => "Setting up the worktree…".to_string(),
                            Status::Starting => "Starting the agent…".to_string(),
                            Status::Stopped => format!("{} is stopped. Send a message to resume.", self.def.label),
                            _ => "No messages yet.".to_string(),
                        }),
                )
            })
            .when(!empty, |d| d.child(chat))
            .children(self.sign_in_bar(look, cx))
            .children(self.permission_bar(look, cx))
            .children(self.files_strip(look, cx))
            .when(!self.pending.is_empty(), |d| d.child(self.queue_list(look, cx)))
            .children(hints)
            .child(prompt_bar)
    }

    /// Queued messages, oldest (next to send) first, each removable.
    fn queue_list(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let row_button = |label: &'static str, id: (&'static str, usize)| {
            div()
                .id(id)
                .flex_none()
                .px(look.z(6.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_size(look.text(-1.))
                .text_color(look.c(ui.text_muted))
                .hover(|d| d.bg(look.c(ui.hover_bg)).text_color(look.c(ui.text)))
                .child(label)
        };
        div()
            .flex_none()
            .mx(look.z(12.))
            .mb(look.z(6.))
            .flex()
            .flex_col()
            .gap(look.z(4.))
            .child(div().text_size(look.text(0.)).font_weight(gpui::FontWeight::MEDIUM).text_color(look.c(ui.text_muted)).child(format!("QUEUED  {}", self.pending.len())))
            .children(self.pending.iter().enumerate().map(|(i, o)| {
                let labels = if o.labels.is_empty() { String::new() } else { format!("  [{}]", o.labels.join(", ")) };
                div()
                    .flex()
                    .items_start()
                    .gap(look.z(8.))
                    .px(look.z(10.))
                    .py(look.z(4.))
                    .rounded(look.z(4.))
                    .bg(look.c(ui.active_bg))
                    .text_color(look.c(ui.text_secondary))
                    .child(div().flex_1().min_w(px(0.)).child(format!("{}{labels}", o.text.trim())))
                    .child(row_button("Send now", ("queue-send", i)).on_click(cx.listener(move |t, _, _, cx| t.send_queued_now(i, cx))))
                    .child(row_button("Edit", ("queue-edit", i)).on_click(cx.listener(move |t, _, window, cx| {
                        t.edit_queued(i, cx);
                        let f = t.prompt.read(cx).focus.clone();
                        window.focus(&f, cx);
                    })))
                    .child(
                        div()
                            .id(("queue-x", i))
                            .flex_none()
                            .cursor_pointer()
                            .text_color(look.c(ui.text_muted))
                            .hover(|d| d.text_color(look.c(ui.text)))
                            .child("✕")
                            .on_click(cx.listener(move |t, _, _, cx| t.unqueue(i, cx))),
                    )
            }))
    }

    pub(super) fn header(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = look.theme.ui.clone();
        let working = matches!(self.status, Status::Working | Status::NeedsYou);
        let elapsed = self.turn_started.filter(|_| working).map(|t| format!("  {}s", t.elapsed().as_secs())).unwrap_or_default();
        // With window gaps the header is a tile of its own above the dock's.
        let tiled = look.tiles.is_some();
        let d = div();
        let d = if tiled { look.tile(d, ui.editor_bg) } else { d.border_b_1().border_color(look.c(ui.border)) };
        d.flex_none()
            .h(look.z(36.))
            .px(look.z(16.))
            .flex()
            .items_center()
            .gap(look.z(10.))
            .child(div().flex_none().size(look.z(10.)).rounded(look.z(2.5)).bg(look.c(self.accent)))
            .child(div().min_w(px(0.)).truncate().font_weight(gpui::FontWeight::MEDIUM).child(self.record.title.clone()))
            .child(
                div()
                    .flex_none()
                    .text_size(look.text(-1.))
                    .text_color(look.c(if matches!(self.status, Status::Error(_)) { ui.error } else { ui.text_muted }))
                    .child(format!("{}{elapsed}", self.status.label())),
            )
            .child(div().flex_1())
            .child(div().flex_none().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(self.record.branch.clone()))
            .when(working, |d| {
                d.child(
                    div()
                        .id("stop")
                        .flex_none()
                        .px(look.z(8.))
                        .py(look.z(2.))
                        .rounded(look.z(4.))
                        .border_1()
                        .border_color(look.c(ui.border_strong))
                        .cursor_pointer()
                        .text_size(look.text(-1.))
                        .child("Stop")
                        .on_click(cx.listener(|t, _, _, cx| t.stop(cx))),
                )
            })
    }

    /// Cmd+1..5: bring a panel to the front of its pane.
    pub fn set_view(&mut self, v: View, cx: &mut Context<Self>) {
        if v == View::Changes {
            self.changes.update(cx, |c, cx| c.refresh(cx));
        }
        Docks::update(cx, Kind::Acp, |d| d.activate(v.panel()));
        cx.notify();
    }

    pub fn set_layout(&mut self, l: Layout, cx: &mut Context<Self>) {
        self.layout = l;
        cx.notify();
    }

    /// Does the stage show this task's browser right now?
    pub fn shows_browser(&self, cx: &App) -> bool {
        Docks::get(cx, Kind::Acp).is_visible(Panel::Browser)
    }

    pub fn show_browser(&mut self, on: bool, cx: &mut Context<Self>) {
        Docks::update(cx, Kind::Acp, |d| d.activate(if on { Panel::Browser } else { Panel::Live }));
        cx.notify();
    }

    /// Review Changes (Cmd+4 / Agent menu): show the diff, freshly computed.
    pub fn show_changes(&mut self, cx: &mut Context<Self>) {
        Docks::update(cx, Kind::Acp, |d| d.activate(Panel::Changes));
        self.changes.update(cx, |c, cx| c.show_all(cx));
        cx.notify();
    }
}
