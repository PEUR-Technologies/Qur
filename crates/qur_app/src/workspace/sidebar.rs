//! The sidebar: AGENTS, TERMINALS, SOURCE CONTROL and FILES, in the order
//! `[ui] sidebar_sections` gives, on either side, always shown or auto-hidden.

use super::*;

impl Workspace {
    fn section_header(&self, label: SharedString, look: &Look) -> gpui::Div {
        let ui = &look.theme.ui;
        div()
            .flex()
            .items_center()
            .px(look.z(10.))
            .pt(look.z(10.))
            .pb(look.z(4.))
            .text_size(look.text(-2.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(look.c(ui.text_muted))
            .child(div().flex_1().truncate().child(label))
    }

    #[allow(clippy::too_many_arguments)]
    fn session_row(
        &self,
        id: SharedString,
        accent: Option<Rgba>,
        title: String,
        detail: String,
        glyph: &'static str,
        selected: bool,
        look: &Look,
    ) -> gpui::Stateful<gpui::Div> {
        let ui = &look.theme.ui;
        div()
            .id(id)
            .mx(look.z(4.))
            .px(look.z(6.))
            .py(look.z(4.))
            .rounded(look.z(4.))
            .flex()
            .items_center()
            .gap(look.z(8.))
            .cursor_pointer()
            .when(selected, |d| d.bg(look.c(ui.active_bg)))
            .hover(|d| d.bg(look.c(ui.hover_bg)))
            .child(match accent {
                Some(a) => div().flex_none().size(look.z(9.)).rounded(look.z(2.5)).bg(look.c(a)),
                None => div().flex_none().size(look.z(9.)).rounded(look.z(2.5)).border_1().border_color(look.c(ui.text_muted)),
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .child(div().truncate().text_size(look.text(-1.)).text_color(look.c(ui.text)).child(title))
                    .when(!detail.is_empty(), |d| d.child(div().truncate().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(detail))),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(look.text(-1.))
                    .text_color(look.c(if glyph == "!" { accent.unwrap_or(ui.warning) } else { ui.text_muted }))
                    .child(glyph),
            )
    }

    /// An agent in the AGENTS section: a button that fills with the agent's
    /// accent while it is active (working, or its terminal is busy). Clicking it
    /// opens the agent's session.
    #[allow(clippy::too_many_arguments)]
    fn agent_button(
        &self,
        id: SharedString,
        accent: Rgba,
        title: String,
        badge: String,
        active: bool,
        selected: bool,
        look: &Look,
    ) -> gpui::Stateful<gpui::Div> {
        let ui = &look.theme.ui;
        let tint = |a: f32| {
            let mut h = look.c(accent);
            h.a = a;
            h
        };
        let name = id.to_string();
        div()
            .id(id)
            .relative()
            .children(crate::script::mark(|| format!("agent:{name}")))
            .mx(look.z(6.))
            .mb(look.z(4.))
            .px(look.z(8.))
            .py(look.z(5.))
            .rounded(look.z(5.))
            .border_1()
            .flex()
            .items_center()
            .gap(look.z(8.))
            .cursor_pointer()
            .map(|d| match (active, selected) {
                (true, _) => d.bg(tint(if selected { 0.32 } else { 0.22 })).border_color(look.c(accent)),
                (false, true) => d.bg(look.c(ui.active_bg)).border_color(tint(0.6)),
                (false, false) => d.border_color(look.c(ui.border)).hover(|d| d.bg(look.c(ui.hover_bg))),
            })
            .child(
                div()
                    .flex_none()
                    .size(look.z(8.))
                    .rounded_full()
                    .map(|d| if active { d.bg(look.c(accent)) } else { d.border_1().border_color(look.c(accent)) }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(look.text(-1.))
                    .text_color(look.c(if active || selected { ui.text } else { ui.text_secondary }))
                    .child(title),
            )
            .when(!badge.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(look.text(-2.))
                        .text_color(look.c(if badge.ends_with('!') { accent } else { ui.text_muted }))
                        .child(badge),
                )
            })
    }

    pub(super) fn sidebar(&self, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let ui = look.theme.ui.clone();
        let mut agents: Vec<gpui::AnyElement> = Vec::new();
        for a in &self.agents {
            let t = a.read(cx);
            let id = t.record.id.clone();
            let sel = self.selected == Some(Selected::Agent(id.clone()));
            let ch = t.changes.read(cx);
            let (add, del, conflicts) = (ch.totals.0, ch.totals.1, !ch.conflicts.is_empty());
            let active = matches!(t.status, Status::Working | Status::Starting | Status::NeedsYou);
            let mut badge = if add + del > 0 { format!("+{add} −{del}{}", if conflicts { " ⚠" } else { "" }) } else { String::new() };
            if !matches!(t.status, Status::Working | Status::Starting) {
                let g = t.status.glyph();
                if g != "·" {
                    badge = if badge.is_empty() { g.to_string() } else { format!("{badge} {g}") };
                }
            }
            badge = with_check(badge, self.checks.get(&id));
            let unread = self.unread.contains(&id);
            let row = self
                .agent_button(format!("agent-{id}").into(), t.accent, t.record.title.clone(), badge, active, sel, look)
                .on_mouse_down(MouseButton::Right, {
                    let id = id.clone();
                    cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| this.open_agent_menu(Selected::Agent(id.clone()), ev.position, window, cx))
                })
                .when(unread, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                    this.select(Some(Selected::Agent(id.clone())), window, cx);
                    // The agent's own row shows its chat, not a subagent's log.
                    if let Some(a) = this.agent(&id).cloned() {
                        a.update(cx, |a, cx| {
                            a.show_subagent(None, cx);
                            a.show_review(false, cx);
                        });
                    }
                    if ev.click_count() == 2 {
                        this.rename_agent(&qur::RenameAgent, window, cx);
                    }
                }));
            agents.push(row.into_any_element());
            // Its subagents, indented under it; a click shows the agent.
            let subs: Vec<_> = a.read(cx).subagents.iter().map(|s| (s.id.clone(), s.title.clone(), s.state.clone(), s.last.clone())).collect();
            for (sid, title, state, last) in subs {
                let (glyph, detail) = match state.as_str() {
                    "running" => ("~", last.unwrap_or_default()),
                    "requires_action" => ("!", "needs you".to_string()),
                    "idle" => ("✓", "done".to_string()),
                    other => ("", other.to_string()),
                };
                let id = a.read(cx).record.id.clone();
                let open = sel && a.read(cx).open_subagent.as_deref() == Some(sid.as_str());
                let running = state == "running";
                let agent = a.clone();
                let row = self
                    .session_row(format!("sub-{sid}").into(), None, format!("↳ {title}"), detail, glyph, open, look)
                    .pl(look.z(22.))
                    .on_click({
                        let (agent, sid) = (agent.clone(), sid.clone());
                        cx.listener(move |this, _, window, cx| {
                            // Show its work in the agent's chat panel.
                            this.select(Some(Selected::Agent(id.clone())), window, cx);
                            agent.update(cx, |a, cx| a.show_subagent(Some(sid.clone()), cx));
                            Docks::update(cx, dock::Kind::Acp, |d| d.activate(dock::Panel::Session));
                        })
                    })
                    .when(running, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("sub-stop-{sid}")))
                                .flex_none()
                                .px(look.z(4.))
                                .rounded(look.z(3.))
                                .cursor_pointer()
                                .hover(|d| d.bg(look.c(ui.hover_bg)))
                                .text_color(look.c(ui.text_muted))
                                .child("✕")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    agent.update(cx, |a, cx| a.stop_subagent(&sid, cx));
                                })),
                        )
                    });
                agents.push(row.into_any_element());
            }
            // Its reviewer (Review Agent's Work…), indented like a subagent;
            // a click shows its chat in the agent's chat panel.
            if let Some(r) = a.read(cx).reviewer.clone() {
                let (title, status, findings) = {
                    let r = r.read(cx);
                    (r.record.title.clone(), r.status.clone(), r.findings().is_some())
                };
                let (open, unread) = {
                    let a = a.read(cx);
                    (sel && a.open_review, a.review_unread)
                };
                let detail = match &status {
                    Status::Done if findings => "findings ready".to_string(),
                    Status::Working => r.read(cx).last_line().unwrap_or_else(|| "reviewing…".into()),
                    s => s.label(),
                };
                let running = matches!(status, Status::Working | Status::NeedsYou | Status::Starting);
                let glyph = if running { "~" } else { status.glyph() };
                let id = a.read(cx).record.id.clone();
                let agent = a.clone();
                let row = self
                    .session_row(format!("review-{id}").into(), None, format!("↳ {title}"), detail, glyph, open, look)
                    .pl(look.z(22.))
                    .when(unread, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                    .on_click({
                        let agent = agent.clone();
                        cx.listener(move |this, _, window, cx| {
                            this.select(Some(Selected::Agent(id.clone())), window, cx);
                            agent.update(cx, |a, cx| a.show_review(true, cx));
                            Docks::update(cx, dock::Kind::Acp, |d| d.activate(dock::Panel::Session));
                        })
                    })
                    .when(running, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("review-stop-{}", a.read(cx).record.id)))
                                .flex_none()
                                .px(look.z(4.))
                                .rounded(look.z(3.))
                                .cursor_pointer()
                                .hover(|d| d.bg(look.c(ui.hover_bg)))
                                .text_color(look.c(ui.text_muted))
                                .child("✕")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    r.update(cx, |r, cx| r.stop(cx));
                                })),
                        )
                    });
                agents.push(row.into_any_element());
            }
        }
        let panel = self.terms.read(cx);
        for t in &self.term_tasks {
            let id = t.record.id.clone();
            let sel = self.selected == Some(Selected::TermTask(id.clone()));
            let s = t.term_id.and_then(|tid| panel.session_by_id(tid));
            let busy = s.is_some_and(|s| s.is_busy());
            let title = t.record.title.clone();
            let badge = with_check(if s.is_some() { String::new() } else { "stopped".to_string() }, self.checks.get(&id));
            let row = self
                .agent_button(format!("tt-{id}").into(), t.accent, title, badge, busy, sel, look)
                .on_mouse_down(MouseButton::Right, {
                    let id = id.clone();
                    cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| this.open_agent_menu(Selected::TermTask(id.clone()), ev.position, window, cx))
                })
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                    this.select(Some(Selected::TermTask(id.clone())), window, cx);
                    if ev.click_count() == 2 {
                        this.rename_agent(&qur::RenameAgent, window, cx);
                    }
                }));
            agents.push(row.into_any_element());
        }
        let shells: Vec<gpui::AnyElement> = self
            .shells
            .iter()
            .filter_map(|&tid| {
                let s = panel.session_by_id(tid)?;
                let sel = self.selected == Some(Selected::Shell(tid));
                Some(
                    self.session_row(format!("sh-{tid}").into(), None, s.display_label(), String::new(), if s.is_busy() { "~" } else { "" }, sel, look)
                        .group("shell-row")
                        .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                            this.select(Some(Selected::Shell(tid)), window, cx);
                            if ev.click_count() == 2 {
                                this.rename_agent(&qur::RenameAgent, window, cx);
                            }
                        }))
                        // Close the terminal (and its splits), shown on hover.
                        .child(
                            div()
                                .id(SharedString::from(format!("sh-close-{tid}")))
                                .flex_none()
                                .px(look.z(4.))
                                .rounded(look.z(3.))
                                .invisible()
                                .group_hover("shell-row", |d| d.visible())
                                .hover(|d| d.bg(look.c(ui.hover_bg)))
                                .text_color(look.c(ui.text_muted))
                                .child("✕")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.terms.update(cx, |p, cx| p.close_row(tid, cx));
                                })),
                        )
                        .into_any_element(),
                )
            })
            .collect();
        let root = self.current_root(cx);
        let tree = self.trees.get(&root).cloned();
        let files_label: SharedString = if root == self.root {
            "FILES".into()
        } else {
            format!("FILES · {}", root.file_name().unwrap_or_default().to_string_lossy()).into()
        };
        let empty_agents = agents.is_empty();
        // Scrolls on its own once there are many agents and subagents, so
        // Source Control and the file tree stay on screen below it.
        let agents = div().id("sidebar-agents").pt(look.z(2.)).flex().flex_col().flex_shrink_0().max_h(gpui::relative(0.6)).overflow_y_scroll().children(agents);
        let tiled = look.tiles.is_some();
        let right = self.sidebar_right();
        let side = div();
        let side = if tiled {
            look.tile(side, ui.sidebar_bg)
        } else {
            side.bg(look.bg(ui.sidebar_bg)).when(right, |d| d.border_l_1()).when(!right, |d| d.border_r_1()).border_color(look.c(ui.border))
        };
        let mut side = side
            .relative()
            .w(look.z(self.sidebar_w))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(self.config.ui.keep_awake, |d| d.child({
                // Keep Awake toggle, above the agents (View > Show Keep Awake in Sidebar).
                let on = self.caffeinate.is_some();
                div()
                    .id("caffeinate")
                    .mx(look.z(6.))
                    .mt(look.z(8.))
                    .px(look.z(8.))
                    .py(look.z(4.))
                    .rounded(look.z(4.))
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .cursor_pointer()
                    .hover(|d| d.bg(look.c(ui.hover_bg)))
                    .text_size(look.text(-1.))
                    .text_color(look.c(if on { ui.text } else { ui.text_muted }))
                    .child(
                        // A small switch: filled with the accent when on.
                        div()
                            .flex_none()
                            .w(look.z(22.))
                            .h(look.z(12.))
                            .rounded(look.z(6.))
                            .p(look.z(2.))
                            .flex()
                            .when(on, |d| d.justify_end())
                            .bg(look.c(if on { look.theme.accents[0].base } else { ui.border_strong }))
                            .child(div().size(look.z(8.)).rounded(look.z(4.)).bg(look.c(ui.editor_bg))),
                    )
                    .child(if on { "Keep awake · on" } else { "Keep awake" })
                    .on_click(cx.listener(|this, _, window, cx| this.caffeinate(&qur::Caffeinate, window, cx)))
            }));
        // The sections in `[ui] sidebar_sections` order; each header drags
        // onto another to reorder them.
        let mut agents_part = Vec::new();
        // Like TERMINALS: the section only appears once there's something in it
        // (Cmd+Shift+N or File > New Agent Task starts one).
        if !empty_agents {
            let header = self.section_header("AGENTS".into(), look).child(
                div()
                    .id("new-task")
                    .flex_none()
                    .p(look.z(2.))
                    .rounded(look.z(3.))
                    .cursor_pointer()
                    .hover(|d| d.bg(look.c(ui.hover_bg)))
                    .child(svg().path(icon::PLUS).size(look.z(12.)).text_color(look.c(ui.text_secondary)))
                    .on_click(cx.listener(|this, _, window, cx| this.new_agent_task(&qur::NewAgentTask, window, cx))),
            );
            agents_part.push(self.section_drag("agents", "AGENTS", header, look, cx).into_any_element());
            agents_part.push(agents.into_any_element());
        }
        let mut terminals_part = Vec::new();
        if !shells.is_empty() {
            terminals_part.push(self.section_drag("terminals", "TERMINALS", self.section_header("TERMINALS".into(), look), look, cx).into_any_element());
            terminals_part.extend(shells);
        }
        // A folder without git has no Source Control; the empty window shows
        // its terminals only.
        let git_part = if !self.repo { Vec::new() } else { vec![
            self.section_drag("git", "SOURCE CONTROL", self.section_header("SOURCE CONTROL".into(), look), look, cx).into_any_element(),
            {
                let (branch, n) = self
                    .git
                    .as_ref()
                    .map(|g| {
                        let st = &g.read(cx).status;
                        (st.branch.clone().unwrap_or_else(|| "detached".into()), st.staged.len() + st.unstaged.len() + st.untracked.len())
                    })
                    .unwrap_or_else(|| ("Git".into(), 0));
                let detail = match (&self.merge, n) {
                    (Some(m), _) => format!("merging · {} conflicted", m.files.len()),
                    (None, 0) => "clean".to_string(),
                    (None, n) => format!("{n} changed"),
                };
                self.session_row("git".into(), None, branch, detail, if self.merge.is_some() { "!" } else { "" }, self.selected == Some(Selected::Git), look)
                    .on_click(cx.listener(|this, _, window, cx| this.show_git(&qur::GitPanel, window, cx)))
                    .into_any_element()
            },
        ] };
        let files_header = div()
            .id("files-toggle")
            .cursor_pointer()
            .child(
                self.section_header(files_label.clone(), look)
                    .pt(look.z(14.))
                    .gap(look.z(4.))
                    .child(
                        svg()
                            .path(if self.files_open { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT })
                            .size(look.z(12.))
                            .flex_none()
                            .text_color(look.c(ui.text_muted)),
                    ),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.files_open = !this.files_open;
                cx.notify();
            }));
        let mut files_part = if self.home { Vec::new() } else { vec![self.section_drag("files", files_label, files_header, look, cx).into_any_element()] };
        if self.files_open && !self.home {
            files_part.extend(tree.map(|t| div().flex_1().min_h(px(0.)).flex().flex_col().child(t).into_any_element()));
        }
        let mut parts = [("agents", agents_part), ("terminals", terminals_part), ("git", git_part), ("files", files_part)];
        for key in &self.config.ui.sidebar_sections {
            if let Some((_, part)) = parts.iter_mut().find(|(k, _)| k == key) {
                side = side.children(std::mem::take(part));
            }
        }
        // Drag handle on the stage's edge (with window gaps, the gap is the handle).
        let side = side.when(!tiled, |d| {
            d.child(
                div()
                    .id("sidebar-resize")
                    .absolute()
                    .top_0()
                    .when(right, |d| d.left(px(-3.)))
                    .when(!right, |d| d.right(px(-3.)))
                    .w(px(6.))
                    .h_full()
                    .cursor_col_resize()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.resizing = true;
                        cx.stop_propagation();
                    })),
            )
        });
        if self.config.ui.sidebar != "auto" {
            return side.into_any_element();
        }
        // Auto-hide: the sidebar's space grows as it slides in (and shrinks
        // as it slides out), so the stage eases over with its edge, as the
        // content does under zen's bar.
        let p = self.sidebar_p();
        if self.sidebar_since.elapsed() < ZEN_SLIDE {
            crate::tiling::next_frame(cx);
        }
        let w = f32::from(look.z(self.sidebar_w));
        div()
            .flex_none()
            .w(px(w * p))
            .h_full()
            .relative()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w(px(w))
                    .flex()
                    .when(right, |d| d.left_0())
                    .when(!right, |d| d.right_0())
                    .child(side),
            )
            .into_any_element()
    }

    /// A section's header, dragged onto another's to move the section there.
    fn section_drag(&self, key: &'static str, label: impl Into<SharedString>, header: impl IntoElement, look: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = look.c(look.theme.ui.focus_ring);
        let drag = DragSection { key, label: label.into() };
        div()
            .id(SharedString::from(format!("section-{key}")))
            .border_t_2()
            .border_color(gpui::transparent_black())
            .child(header)
            .children(crate::script::mark(|| format!("section:{key}")))
            .on_drag(drag, |d: &DragSection, _, _, cx| cx.new(|_| d.clone()))
            .drag_over::<DragSection>(move |s, _, _, _| s.border_color(accent))
            .on_drop(cx.listener(move |this, d: &DragSection, _, cx| this.move_section(d.key, key, cx)))
    }

    /// Move a sidebar section to where `to` is, saved as `[ui] sidebar_sections`.
    fn move_section(&mut self, key: &str, to: &str, cx: &mut Context<Self>) {
        let list = &mut self.config.ui.sidebar_sections;
        let (Some(from), Some(at)) = (list.iter().position(|k| k == key), list.iter().position(|k| k == to)) else { return };
        if from == at {
            return;
        }
        let k = list.remove(from);
        list.insert(at, k);
        let list = list.clone();
        self.write_setting(false, "ui.sidebar_sections", Some(Value::List(list)), cx);
        self.broadcast_reload(cx);
        cx.notify();
    }

    /// Whether the sidebar takes its column now: on (`Cmd+B`), and with
    /// auto-hide only while slid in.
    /// The empty window (no project) has none.
    pub(super) fn sidebar_visible(&self) -> bool {
        self.sidebar && (self.config.ui.sidebar != "auto" || self.sidebar_p() > 0.) && !self.home
    }

    /// Auto-hide: how far in the sidebar is, 0 (away) to 1 (in), eased over
    /// `ZEN_SLIDE` like zen's bar (symmetric, so turning around doesn't jump).
    /// Always 1 when it doesn't auto-hide.
    pub(super) fn sidebar_p(&self) -> f32 {
        if self.config.ui.sidebar != "auto" {
            return 1.;
        }
        let e = gpui::ease_in_out((self.sidebar_since.elapsed().as_secs_f32() / ZEN_SLIDE.as_secs_f32()).min(1.));
        if self.sidebar_shown { e } else { 1. - e }
    }

    pub(super) fn sidebar_right(&self) -> bool {
        self.config.ui.sidebar_side == "right"
    }

    /// Auto-hide: slide the sidebar in at its edge of the window, and away
    /// once the mouse leaves it (not while resizing it or with a menu open).
    pub(super) fn sidebar_track(&mut self, x: Pixels, window: &Window, cx: &mut Context<Self>) {
        // Held as it is while something is dragged (a tab, a pane, a section).
        if self.config.ui.sidebar != "auto" || !self.sidebar || cx.has_active_drag() {
            return;
        }
        let look = Look::of(window, cx);
        let x = if self.sidebar_right() { window.viewport_size().width - x } else { x };
        let show = if self.sidebar_shown {
            self.resizing || self.overlay.is_some() || x <= look.z(self.sidebar_w) + look.gap() * 2. + px(ZEN_SLACK)
        } else {
            x <= px(ZEN_EDGE) + look.gap()
        };
        if show != self.sidebar_shown {
            // Turning around mid-slide carries on from where the sidebar is.
            let t = (self.sidebar_since.elapsed().as_secs_f32() / ZEN_SLIDE.as_secs_f32()).min(1.);
            self.sidebar_since = Instant::now().checked_sub(ZEN_SLIDE.mul_f32(1. - t)).unwrap_or_else(Instant::now);
            self.sidebar_shown = show;
            cx.notify();
        }
    }

    /// View > Sidebar > Always Show / Auto-hide / Hidden, and Settings.
    pub(super) fn set_sidebar_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        self.config.ui.sidebar = mode.to_string();
        self.write_setting(false, "ui.sidebar", Some(mode.into()), cx);
        self.sidebar = mode != "hidden";
        self.sidebar_shown = false;
        self.sidebar_since = Instant::now().checked_sub(ZEN_SLIDE).unwrap_or_else(Instant::now);
        self.broadcast_reload(cx);
        self.sync_menus(cx);
        self.sync_settings(cx);
        cx.notify();
    }

    pub(super) fn auto_hide_sidebar(&mut self, _: &qur::AutoHideSidebar, _: &mut Window, cx: &mut Context<Self>) {
        let mode = if self.config.ui.sidebar == "auto" { "show" } else { "auto" };
        self.set_sidebar_mode(mode, cx);
        self.flash(if mode == "auto" { "Sidebar auto-hides (move the mouse to the window's edge)" } else { "Sidebar always shown" }, cx);
    }

    pub(super) fn set_sidebar_side(&mut self, right: bool, cx: &mut Context<Self>) {
        let side = if right { "right" } else { "left" };
        self.config.ui.sidebar_side = side.to_string();
        self.write_setting(false, "ui.sidebar_side", Some(side.into()), cx);
        self.broadcast_reload(cx);
        self.sync_menus(cx);
        self.sync_settings(cx);
        cx.notify();
    }

    pub(super) fn sidebar_on_right(&mut self, _: &qur::SidebarOnRight, _: &mut Window, cx: &mut Context<Self>) {
        self.set_sidebar_side(!self.sidebar_right(), cx);
    }

    /// With window gaps: the gap between the sidebar and the stage, which
    /// resizes the sidebar.
    pub(super) fn sidebar_gap(&self, look: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar-resize")
            .flex_none()
            .w(look.gap() * self.sidebar_p())
            .h_full()
            .cursor_col_resize()
            .relative()
            .children(crate::script::mark(|| "sidebar-gap".to_string()))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.resizing = true;
                cx.stop_propagation();
            }))
    }
}

/// An agent button's badge plus its latest check (decision 29).
fn with_check(badge: String, check: Option<&Check>) -> String {
    let mark = match check {
        Some(Check::Running) => "checking…",
        Some(Check::Failed) => "✕ checks",
        Some(Check::Passed) | None => return badge,
    };
    if badge.is_empty() { mark.to_string() } else { format!("{badge} · {mark}") }
}

/// What's being dragged: a sidebar section, by its header.
#[derive(Clone)]
pub(super) struct DragSection {
    key: &'static str,
    label: SharedString,
}

impl Render for DragSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = &look.theme.ui;
        div()
            .px(look.z(10.))
            .py(look.z(3.))
            .rounded(look.z(4.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.focus_ring))
            .text_size(look.text(-2.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(look.c(ui.text_muted))
            .font_family(look.ui_font.clone())
            .child(self.label.clone())
    }
}
