//! Overlays and popovers: the agent menu, accent colour, layouts, About,
//! transparency and gaps.

use super::*;

impl Workspace {
    pub(super) fn open_agent_menu(&mut self, which: Selected, at: gpui::Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let id = match &which {
            Selected::Agent(id) | Selected::TermTask(id) => id.clone(),
            _ => return,
        };
        self.close_overlays(window, cx);
        self.select(Some(which), window, cx);
        self.prev_focus = window.focused(cx);
        self.overlay = Some(Overlay::AgentMenu { id, at });
        cx.stop_propagation();
        cx.notify();
    }

    fn agent_menu(&self, id: &str, at: gpui::Point<Pixels>, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let in_root = self.in_root(id, cx);
        let acp = self.agent(id).is_some();
        type Make = fn() -> Box<dyn gpui::Action>;
        let mut items: Vec<(&'static str, Make, bool, bool)> = vec![
            ("Rename…", || Box::new(qur::RenameAgent), true, false),
            ("Review Changes", || Box::new(qur::ReviewChanges), true, false),
            ("Merge into My Branch", || Box::new(qur::Merge), !in_root, false),
            ("Run Checks Now", || Box::new(qur::RunChecks), true, false),
            ("Run Project", || Box::new(qur::RunProject), true, false),
            ("Reveal in Finder", || Box::new(qur::RevealWorktree), true, false),
            ("Open Folder in Terminal", || Box::new(qur::OpenWorktreeInTerminal), true, false),
        ];
        if acp {
            let turns = self.agent(id).is_some_and(|a| a.read(cx).record.last_turn().is_some());
            items.push(("Show Last Turn's Changes", || Box::new(qur::ShowLastTurn), turns, false));
            items.push(("Rewind Last Turn…", || Box::new(qur::RewindLastTurn), turns, false));
            let pending = self.agent(id).is_some_and(|a| a.read(cx).has_permission());
            items.push(("Allow All Pending", || Box::new(aa::AllowAllPending), pending, false));
            items.push(("Copy Chat as Markdown", || Box::new(aa::CopyChatMarkdown), true, false));
            let review = self.agent(id).map(|a| {
                let a = a.read(cx);
                (a.plan_text().is_some(), a.has_findings(cx))
            });
            let (plan, findings) = review.unwrap_or_default();
            items.push(("Review Agent's Work…", || Box::new(qur::ReviewWork), true, false));
            items.push(("Review Agent's Plan…", || Box::new(qur::ReviewPlan), plan, false));
            items.push(("Send Review Findings to Agent", || Box::new(qur::SendReviewFindings), findings, false));
            items.push(("Stop", || Box::new(qur::StopAgent), true, false));
        }
        items.push((if in_root { "Close Agent…" } else { "Delete Agent…" }, || Box::new(qur::DiscardTask), true, true));
        let n = items.len();
        div()
            .absolute()
            .left(at.x)
            .top(at.y)
            .child(
                div()
                    .id("agent-menu")
                    .occlude()
                    .min_w(look.z(210.))
                    .py(look.z(4.))
                    .flex()
                    .flex_col()
                    .bg(look.c(ui.overlay_bg))
                    .border_1()
                    .border_color(look.c(ui.border_strong))
                    .rounded(look.z(6.))
                    .shadow_md()
                    .children(items.into_iter().enumerate().flat_map(|(i, (label, make, enabled, danger))| {
                        let sep = (danger && i + 1 == n).then(|| div().my(look.z(4.)).h(px(1.)).bg(look.c(ui.border)).into_any_element());
                        let row = div()
                            .id(("agent-menu-item", i))
                            .relative()
                            .children(crate::script::mark(|| format!("menu:{label}")))
                            .mx(look.z(4.))
                            .px(look.z(10.))
                            .py(look.z(4.))
                            .rounded(look.z(4.))
                            .text_size(look.text(-1.))
                            .text_color(look.c(if !enabled { ui.text_muted } else if danger { ui.error } else { ui.text }))
                            .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(look.c(ui.hover_bg))))
                            .child(label)
                            .when(enabled, |d| {
                                d.on_click(cx.listener(move |this, _, window, cx| {
                                    this.overlay = None;
                                    this.restore_focus(window, cx);
                                    window.dispatch_action(make(), cx);
                                    cx.notify();
                                }))
                            })
                            .into_any_element();
                        sep.into_iter().chain(std::iter::once(row))
                    })),
            )
            .into_any_element()
    }

    /// View > Appearance > Accent Color…: pick which of the theme's accents leads (hover to
    /// preview, click to keep).
    pub(super) fn accent_color(&mut self, _: &qur::AccentColor, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        window.focus(&self.focus, cx);
        self.overlay = Some(Overlay::Accent);
        cx.notify();
    }

    pub(super) fn set_accent(&mut self, k: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_accent = None;
        self.write_setting(false, "ui.accent", Some(Value::Int(k as i64)), cx);
        self.reload_settings(window, cx);
        self.broadcast_reload(cx);
    }

    /// Drag on an Accent Color slider (0 hue, 1 brightness): preview it.
    pub(super) fn tune_to(&mut self, which: usize, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let b = self.tune_sliders[which].get();
        if b.size.width <= px(0.) {
            return;
        }
        let t = ((x - b.left()) / b.size.width).clamp(0., 1.);
        let (mut hue, mut brightness) = self.preview_tune.unwrap_or((self.config.ui.accent_hue, self.config.ui.accent_brightness));
        if which == 0 {
            hue = (-180. + 360. * t).round();
        } else {
            brightness = ((-1. + 2. * t) * 100.).round() / 100.;
        }
        self.preview_tune = Some((hue, brightness));
        self.apply_look(window, cx);
        cx.notify();
    }

    /// Save the sliders' hue and brightness (on release, or Reset).
    pub(super) fn save_tune(&mut self, tune: (f32, f32), window: &mut Window, cx: &mut Context<Self>) {
        self.preview_tune = None;
        self.write_setting(false, "ui.accent_hue", Some(Value::from(tune.0)), cx);
        self.write_setting(false, "ui.accent_brightness", Some(Value::from(tune.1)), cx);
        self.reload_settings(window, cx);
        self.broadcast_reload(cx);
    }

    fn accent_box(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let (hue, brightness) = self.preview_tune.unwrap_or((self.config.ui.accent_hue, self.config.ui.accent_brightness));
        let base_color = self.accent_base;
        // A track painted with the colors it gives, and a thumb at `t` (0..=1).
        let slider = |which: usize, t: f32, color_at: &dyn Fn(f32) -> Rgba| {
            const STEPS: usize = 24;
            let bounds = self.tune_sliders[which].clone();
            let track_w = look.z(220.);
            div()
                .id(("accent-slider", which))
                .relative()
                .w(track_w)
                .h(look.z(16.))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                    this.tuning = Some(which);
                    this.tune_to(which, ev.position.x, window, cx);
                }))
                .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().size_full())
                .child(
                    div()
                        .absolute()
                        .top(look.z(4.))
                        .left_0()
                        .w(track_w)
                        .h(look.z(8.))
                        .rounded(look.z(4.))
                        .overflow_hidden()
                        .flex()
                        .children((0..STEPS).map(|i| div().flex_1().h_full().bg(look.c(color_at(i as f32 / (STEPS - 1) as f32))))),
                )
                .child(
                    div()
                        .absolute()
                        .top(look.z(1.))
                        .left(track_w * t - look.z(7.))
                        .size(look.z(14.))
                        .rounded_full()
                        .border_2()
                        .border_color(look.c(ui.text))
                        .bg(look.c(look.theme.accents[0].base)),
                )
        };
        let label = |name: &'static str, value: String| {
            div().w(look.z(220.)).flex().justify_between().text_size(look.text(-2.)).text_color(look.c(ui.text_secondary)).child(name).child(div().text_color(look.c(ui.text_muted)).child(value))
        };
        let tuned = hue != 0. || brightness != 0.;
        // The theme's own order: undo the rotation the look carries.
        let shown = self.preview_accent.unwrap_or(self.config.ui.accent) % 6;
        let saved = self.config.ui.accent % 6;
        let mut base = look.theme.accents;
        base.rotate_right(shown);
        div()
            .id("accent")
            .occlude()
            .p(look.z(16.))
            .flex()
            .flex_col()
            .gap(look.z(10.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(10.))
            .child(div().text_size(look.text(-1.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(look.c(ui.text)).child("Accent color"))
            .child(div().flex().gap(look.z(10.)).children(base.iter().enumerate().map(|(i, a)| {
                let on = i == saved;
                div()
                    .id(("accent-swatch", i))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(look.z(4.))
                    .cursor_pointer()
                    .child(div().size(look.z(30.)).rounded_full().bg(look.c(a.base)).border_2().border_color(look.c(if on { ui.text } else { ui.overlay_bg })))
                    .child(div().text_size(look.text(-3.)).text_color(look.c(if on { ui.text } else { ui.text_muted })).child(hue_name(a.base)))
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        let want = if *hovered { Some(i) } else { None };
                        if this.preview_accent != want {
                            this.preview_accent = want;
                            this.apply_look(window, cx);
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_accent(i, window, cx);
                        this.close_overlays(window, cx);
                    }))
            })))
            .child(label("Color", format!("{hue:+.0}°")))
            .child(slider(0, (hue + 180.) / 360., &|t| base_color.tuned(-180. + 360. * t, brightness)))
            .child(label("Brightness", format!("{:+.0}%", brightness * 100.)))
            .child(slider(1, (brightness + 1.) / 2., &|t| base_color.tuned(hue, -1. + 2. * t)))
            .when(tuned, |d| {
                d.child(
                    div()
                        .id("accent-reset")
                        .text_size(look.text(-2.))
                        .text_color(look.c(ui.text_secondary))
                        .cursor_pointer()
                        .hover(|d| d.text_color(look.c(ui.text)))
                        .child("Reset color and brightness")
                        .on_click(cx.listener(|this, _, window, cx| this.save_tune((0., 0.), window, cx))),
                )
            })
            .child(div().text_size(look.text(-3.)).text_color(look.c(ui.text_muted)).child("Agents take the theme's colors in order, starting here; the sliders tune them all. Focus outlines use your color. Saved as [ui] accent, accent_hue, accent_brightness."))
            .into_any_element()
    }

    /// View > Choose Stage Layout…: the presets (and yours) as diagrams.
    pub(super) fn choose_layout(&mut self, _: &qur::ChooseLayout, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        window.focus(&self.focus, cx);
        self.overlay = Some(Overlay::Layouts);
        cx.notify();
    }

    fn layouts_box(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let preset = self.layout.as_str();
        let custom = Docks::get(cx, dock::Kind::Acp) != dock::Dock::preset(dock::Kind::Acp, preset) || Docks::get(cx, dock::Kind::Terminal) != dock::Dock::preset(dock::Kind::Terminal, preset);
        let selected = if custom { "yours" } else { preset };
        div()
            .id("layouts")
            .occlude()
            .p(look.z(16.))
            .flex()
            .flex_col()
            .gap(look.z(10.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(10.))
            .child(div().text_size(look.text(-1.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(look.c(ui.text)).child("Stage layout"))
            .child(dock::layout_cards(selected, look, cx, |this: &mut Workspace, key, cx| {
                if matches!(key, "A" | "B" | "C") {
                    this.set_layout(Layout::parse(key), cx);
                }
                this.overlay = None;
                cx.notify();
            }))
            .child(div().text_size(look.text(-3.)).text_color(look.c(ui.text_muted)).child("Then drag tabs and dividers to make it yours; it's saved as you go."))
            .into_any_element()
    }

    pub(super) fn about(&mut self, _: &qur::About, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        window.focus(&self.focus, cx);
        self.overlay = Some(Overlay::About);
        cx.notify();
    }

    fn about_box(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let credit = look.theme.credit.as_ref().map(|c| format!("Theme “{}” by {} ({})", look.theme.name, c.author, c.license));
        let line = |s: String| div().text_size(look.text(-1.)).text_color(look.c(ui.text_secondary)).child(s);
        div()
            .id("about")
            .occlude()
            .w(look.z(380.))
            .p(look.z(22.))
            .flex()
            .flex_col()
            .items_center()
            .gap(look.z(6.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(10.))
            .child(
                svg()
                    .path(if look.logo_white() { "brand/svg/qur-horizontal-white.svg" } else { "brand/svg/qur-horizontal-black.svg" })
                    .w(look.z(132.))
                    .h(look.z(44.))
                    // Pure black or white only: the logo is never tinted.
                    .text_color(if look.logo_white() { gpui::white() } else { gpui::black() }),
            )
            .child(div().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child("by PEUR Technologies"))
            .child(div().h(look.z(8.)))
            .child(line(format!("Version {}", env!("CARGO_PKG_VERSION"))))
            .child(line("GPL-3.0-or-later · Inter (SIL OFL)".into()))
            .children(credit.map(line))
            .child(
                div()
                    .id("about-ok")
                    .mt(look.z(12.))
                    .px(look.z(16.))
                    .py(look.z(4.))
                    .rounded(look.z(5.))
                    .cursor_pointer()
                    .bg(look.c(ui.button_bg))
                    .text_color(look.c(ui.button_text))
                    .text_size(look.text(-1.))
                    .child("OK")
                    .on_click(cx.listener(|this, _, window, cx| this.close_overlays(window, cx))),
            )
            .into_any_element()
    }

    pub(super) fn transparency(&mut self, _: &qur::Transparency, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let project_scope = self.effective().window_is_project;
        self.overlay = Some(Overlay::Transparency { project_scope });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Qur > Appearance > Gaps…: the popover; changes show as you make
    /// them and are saved when it closes.
    pub(super) fn gaps(&mut self, _: &qur::Gaps, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        self.overlay = Some(Overlay::Gaps);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// View > Toggle Gaps, saved as `[ui] gaps`.
    pub(super) fn toggle_gaps(&mut self, _: &qur::ToggleGaps, window: &mut Window, cx: &mut Context<Self>) {
        let on = !self.config.ui.gaps;
        self.config.ui.gaps = on;
        self.write_setting(false, "ui.gaps", Some(Value::Bool(on)), cx);
        self.apply_look(window, cx);
        self.broadcast_reload(cx);
        self.sync_menus(cx);
        self.flash(if on { "Gaps on (View > Appearance > Gaps… to adjust)" } else { "Gaps off" }, cx);
    }

    /// Drag on a Gaps slider (0 gap, 1 corner radius). Moving the gap
    /// slider turns gaps on, and the radius slider rounded corners.
    fn gap_slide_to(&mut self, which: usize, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let b = self.gap_sliders[which].get();
        if b.size.width <= px(0.) {
            return;
        }
        let t = ((x - b.left()) / b.size.width).clamp(0., 1.);
        self.set_gaps(
            |u| {
                if which == 0 {
                    u.gaps = true;
                    u.gap = qur_config::MIN_GAP + (qur_config::MAX_GAP - qur_config::MIN_GAP) * t;
                } else {
                    u.rounded = true;
                    u.radius = qur_config::MAX_RADIUS * t;
                }
            },
            window,
            cx,
        );
    }

    pub(super) fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.prev_focus.take() {
            Some(f) => window.focus(&f, cx),
            None => self.focus_stage(window, cx),
        }
        cx.notify();
    }

    pub(super) fn close_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tuning = None;
        if self.preview_accent.take().is_some() | self.preview_tune.take().is_some() {
            self.apply_look(window, cx);
        }
        if matches!(&self.overlay, Some(Overlay::Picker(_, PickKind::Themes { .. })) | Some(Overlay::Setup(_)))
            && self.preview_theme.take().is_some()
        {
            self.apply_look(window, cx);
        }
        self.mention_over_dialog = None;
        if self.overlay.take().is_some() {
            self.restore_focus(window, cx);
        }
        cx.notify();
    }

    fn slide_to(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let b = self.slider.get();
        if b.size.width <= px(0.) {
            return;
        }
        let min = qur_theme::MIN_OPACITY;
        let t = ((x - b.left()) / b.size.width).clamp(0., 1.);
        let blur = self.effective().blur;
        let Some(Overlay::Transparency { project_scope }) = self.overlay else { return };
        self.set_opacity(project_scope, min + (1. - min) * t, blur, window, cx);
    }

    pub(super) fn overlay_el(&self, look: &Look, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let ui = &look.theme.ui;
        let centered = |child: gpui::AnyElement| {
            div()
                .absolute()
                .top(look.z(TITLEBAR_H + 40.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(div().occlude().child(child))
                .into_any_element()
        };
        // Pickers opened from a prompt (agent commands, @ mentions, the mode
        // and model chips) sit just above that prompt, not at the top.
        let above_prompt = |target: &Entity<PromptInput>, child: gpui::AnyElement| -> Option<gpui::AnyElement> {
            let b = target.read(cx).bounds?;
            Some(
                gpui::anchored()
                    .position(gpui::point(b.left() - look.z(12.), b.top() - look.z(14.)))
                    .anchor(gpui::Anchor::BottomLeft)
                    .snap_to_window_with_margin(look.z(8.))
                    .child(div().occlude().child(child))
                    .into_any_element(),
            )
        };
        match self.overlay.as_ref()? {
            Overlay::Picker(p, PickKind::Slash { target, .. } | PickKind::Mention { target, .. }) => {
                let el = || p.clone().into_any_element();
                Some(above_prompt(target, el()).unwrap_or_else(|| centered(el())))
            }
            Overlay::Picker(p, PickKind::Setting { agent, .. }) => {
                let target = agent.read(cx).prompt.clone();
                let el = || p.clone().into_any_element();
                Some(above_prompt(&target, el()).unwrap_or_else(|| centered(el())))
            }
            Overlay::Picker(p, _) => Some(centered(p.clone().into_any_element())),
            Overlay::Setup(s) => Some(centered(s.clone().into_any_element())),
            Overlay::About => Some(centered(self.about_box(look, cx))),
            Overlay::Accent => Some(centered(self.accent_box(look, cx))),
            Overlay::AgentMenu { id, at } => Some(self.agent_menu(id, *at, look, cx)),
            Overlay::Layouts => Some(centered(self.layouts_box(look, cx))),
            Overlay::GoTo(g) | Overlay::Rename(g) => Some(centered(
                div()
                    .w(look.z(360.))
                    .px(look.z(10.))
                    .py(look.z(6.))
                    .bg(look.c(ui.overlay_bg))
                    .border_1()
                    .border_color(look.c(ui.border_strong))
                    .rounded(look.z(6.))
                    .child(g.clone())
                    .into_any_element(),
            )),
            Overlay::Transparency { project_scope } => Some(self.transparency_popover(*project_scope, look, cx)),
            Overlay::Gaps => Some(self.gaps_popover(look, cx)),
            Overlay::NewTask { input, agent, plan } => {
                let agents = self.roster.agents.clone();
                let chosen = *agent;
                // Plan first (Cmd+Opt+P here too): structured agents only.
                let plan_chip = agents.get(chosen).filter(|a| a.mode == Mode::Acp).map(|_| {
                    let on = *plan;
                    div()
                        .id("plan-first")
                        .px(look.z(8.))
                        .py(look.z(2.))
                        .rounded(look.z(4.))
                        .cursor_pointer()
                        .text_size(look.text(-1.))
                        .border_1()
                        .border_color(look.c(if on { ui.border_strong } else { ui.border }))
                        .text_color(look.c(if on { ui.text } else { ui.text_muted }))
                        .when(on, |d| d.bg(look.c(ui.active_bg)))
                        .child(if on { "✓ Plan first" } else { "Plan first" })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_plan_first(cx)))
                });
                let chips: Vec<_> = agents
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        div()
                            .id(("agent-chip", i))
                            .px(look.z(8.))
                            .py(look.z(2.))
                            .rounded(look.z(4.))
                            .cursor_pointer()
                            .text_size(look.text(-1.))
                            .text_color(look.c(if i == chosen { ui.text } else { ui.text_muted }))
                            .when(i == chosen, |d| d.bg(look.c(ui.active_bg)))
                            .child(if a.mode == Mode::Terminal && !a.label.to_lowercase().contains("terminal") { format!("{} (terminal)", a.label) } else { a.label.clone() })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(Overlay::NewTask { agent, .. }) = &mut this.overlay {
                                    *agent = i;
                                    cx.notify();
                                }
                            }))
                    })
                    .collect();
                let mention = self.mention_over_dialog.clone();
                let agent_id = agents.get(chosen).map(|a| a.id.clone()).unwrap_or_else(|| "agent".into());
                Some(
                    div()
                        .absolute()
                        .top(look.z(TITLEBAR_H + 40.))
                        .left_0()
                        .right_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(look.z(8.))
                        .child(
                            div()
                                .occlude()
                                .w(look.z(600.))
                                .p(look.z(12.))
                                .flex()
                                .flex_col()
                                .gap(look.z(10.))
                                .bg(look.c(ui.overlay_bg))
                                .border_1()
                                .border_color(look.c(ui.border_strong))
                                .rounded(look.z(8.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(look.z(6.))
                                        .child(div().font_weight(gpui::FontWeight::MEDIUM).child("New agent task"))
                                        .child(div().flex_1())
                                        .children(plan_chip)
                                        .children(chips),
                                )
                                .child(
                                    div()
                                        .px(look.z(8.))
                                        .py(look.z(6.))
                                        .rounded(look.z(4.))
                                        .border_1()
                                        .border_color(look.c(ui.border))
                                        .bg(look.c(ui.editor_bg))
                                        .child(input.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(look.text(-2.))
                                        .text_color(look.c(ui.text_muted))
                                        .child(format!("↩ start · runs in its own worktree on qur/{agent_id}/<task> · esc cancel")),
                                ),
                        )
                        .children(mention.map(|m| div().occlude().child(m)))
                        .into_any_element(),
                )
            }
            Overlay::Review { input, subject, choices, chosen, .. } => {
                let roster = &self.roster.agents;
                let chosen = *chosen;
                let chips: Vec<_> = choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let def = &roster[c.agent];
                        let signed_out = self.review_logins.as_ref().and_then(|l| l.get(&review::vendor(def))) == Some(&false);
                        let label = if signed_out { format!("{} (not signed in)", c.label(roster)) } else { c.label(roster) };
                        div()
                            .id(("review-chip", i))
                            .px(look.z(8.))
                            .py(look.z(2.))
                            .rounded(look.z(4.))
                            .cursor_pointer()
                            .text_size(look.text(-1.))
                            .text_color(look.c(if i == chosen { ui.text } else { ui.text_muted }))
                            .when(i == chosen, |d| d.bg(look.c(ui.active_bg)))
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(Overlay::Review { chosen, touched, .. }) = &mut this.overlay {
                                    *chosen = i;
                                    *touched = true;
                                    cx.notify();
                                }
                            }))
                    })
                    .collect();
                let title = match subject {
                    Subject::Work => "Review the agent's work",
                    Subject::Plan => "Review the agent's plan",
                };
                Some(centered(
                    div()
                        .w(look.z(600.))
                        .p(look.z(12.))
                        .flex()
                        .flex_col()
                        .gap(look.z(10.))
                        .bg(look.c(ui.overlay_bg))
                        .border_1()
                        .border_color(look.c(ui.border_strong))
                        .rounded(look.z(8.))
                        .child(div().font_weight(gpui::FontWeight::MEDIUM).child(title))
                        .child(div().flex().flex_wrap().items_center().gap(look.z(6.)).child(div().text_size(look.text(-1.)).text_color(look.c(ui.text_muted)).child("Reviewer")).children(chips))
                        .child(
                            div()
                                .px(look.z(8.))
                                .py(look.z(6.))
                                .rounded(look.z(4.))
                                .border_1()
                                .border_color(look.c(ui.border))
                                .bg(look.c(ui.editor_bg))
                                .child(input.clone()),
                        )
                        .child(
                            div()
                                .text_size(look.text(-2.))
                                .text_color(look.c(ui.text_muted))
                                .child("↩ start · ↑↓ reviewer · esc cancel. It reads the task's diff and chat in the same worktree, on the reviewer's own account (one more session's usage). Read-only: Qur denies every edit or command it asks for and starts it in plan or read-only mode; that's Qur's rule, not an OS sandbox."),
                        )
                        .into_any_element(),
                ))
            }
        }
    }

    fn transparency_popover(&self, project_scope: bool, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let min = qur_theme::MIN_OPACITY;
        let pct = (look.opacity * 100.).round() as i32;
        let t = if min < 1. { (look.opacity - min) / (1. - min) } else { 1. };
        let slider = self.slider.clone();
        let track_w = look.z(200.);
        let check = |on: bool| {
            div()
                .size(look.z(12.))
                .flex_none()
                .rounded(look.z(3.))
                .border_1()
                .border_color(look.c(ui.text_secondary))
                .flex()
                .items_center()
                .justify_center()
                .when(on, |d| d.child(svg().path(icon::CHECK).size(look.z(10.)).text_color(look.c(ui.text))))
        };
        let scope = |label: &'static str, on: bool, id: &'static str, project: bool| {
            div()
                .id(id)
                .px(look.z(8.))
                .py(look.z(2.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_color(look.c(if on { ui.text } else { ui.text_muted }))
                .when(on, |d| d.bg(look.c(ui.active_bg)))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.overlay = Some(Overlay::Transparency { project_scope: project });
                    let eff = this.effective();
                    this.set_opacity(project, eff.opacity, eff.blur, window, cx);
                }))
        };
        div()
            .absolute()
            .top(self.bar_h(look) + look.z(4.))
            .left(self.title_lead(look))
            .occlude()
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.sliding = false))
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                if this.sliding && ev.pressed_button == Some(MouseButton::Left) {
                    this.slide_to(ev.position.x, window, cx);
                } else {
                    this.sliding = false;
                }
            }))
            .w(look.z(248.))
            .p(look.z(12.))
            .flex()
            .flex_col()
            .gap(look.z(10.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(6.))
            .text_size(look.text(-1.))
            .text_color(look.c(ui.text))
            .child(div().flex().gap(look.z(4.)).child(scope("This project", project_scope, "scope-project", true)).child(scope("All projects", !project_scope, "scope-all", false)))
            .child(div().flex().justify_between().child("Window opacity").child(div().text_color(look.c(ui.text_muted)).child(format!("{pct}%"))))
            .child(
                div()
                    .id("slider")
                    .relative()
                    .w(track_w)
                    .h(look.z(16.))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &gpui::MouseDownEvent, window, cx| {
                        this.sliding = true;
                        this.slide_to(ev.position.x, window, cx);
                    }))
                    .child(canvas(move |b, _, _| slider.set(b), |_, _, _, _| {}).absolute().size_full())
                    .child(div().absolute().top(look.z(7.)).left_0().w(track_w).h(look.z(2.)).bg(look.c(ui.border_strong)))
                    .child(div().absolute().top(look.z(7.)).left_0().w(track_w * t).h(look.z(2.)).bg(look.c(ui.text)))
                    .child(div().absolute().top(look.z(2.)).left(track_w * t - look.z(6.)).size(look.z(12.)).rounded(look.z(3.)).bg(look.c(ui.text))),
            )
            .child(
                div()
                    .id("blur")
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .cursor_pointer()
                    .child(check(look.blur))
                    .child("Blur background")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let eff = this.effective();
                        this.set_opacity(project_scope, eff.opacity, !eff.blur, window, cx);
                    })),
            )
            .into_any_element()
    }
    /// Qur > Appearance > Gaps…, modelled on Transparency….
    fn gaps_popover(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let u = &self.config.ui;
        let check = |on: bool| {
            div()
                .size(look.z(12.))
                .flex_none()
                .rounded(look.z(3.))
                .border_1()
                .border_color(look.c(ui.text_secondary))
                .flex()
                .items_center()
                .justify_center()
                .when(on, |d| d.child(svg().path(icon::CHECK).size(look.z(10.)).text_color(look.c(ui.text))))
        };
        let toggle = |id: &'static str, label: &'static str, on: bool, f: fn(&mut qur_config::UiConfig)| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(look.z(8.))
                .cursor_pointer()
                .relative()
                .child(check(on))
                .child(label)
                .children(crate::script::mark(|| format!("gaps:{id}")))
                .on_click(cx.listener(move |this, _, window, cx| this.set_gaps(f, window, cx)))
        };
        let track_w = look.z(200.);
        let slider = |which: usize, label: &'static str, value: f32, t: f32, dim: bool| {
            let bounds = self.gap_sliders.clone();
            div()
                .flex()
                .flex_col()
                .gap(look.z(4.))
                .when(dim, |d| d.opacity(0.5))
                .child(div().flex().justify_between().child(label).child(div().text_color(look.c(ui.text_muted)).child(format!("{} px", value.round() as i32))))
                .child(
                    div()
                        .id(("gap-slider", which))
                        .relative()
                        .w(track_w)
                        .h(look.z(16.))
                        .cursor_pointer()
                        .children(crate::script::mark(move || format!("gaps:slider{which}")))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                            this.gap_sliding = Some(which);
                            this.gap_slide_to(which, ev.position.x, window, cx);
                        }))
                        .child(canvas(move |b, _, _| bounds[which].set(b), |_, _, _, _| {}).absolute().size_full())
                        .child(div().absolute().top(look.z(7.)).left_0().w(track_w).h(look.z(2.)).bg(look.c(ui.border_strong)))
                        .child(div().absolute().top(look.z(7.)).left_0().w(track_w * t).h(look.z(2.)).bg(look.c(ui.text)))
                        .child(div().absolute().top(look.z(2.)).left(track_w * t - look.z(6.)).size(look.z(12.)).rounded(look.z(3.)).bg(look.c(ui.text))),
                )
        };
        let (min, max) = (qur_config::MIN_GAP, qur_config::MAX_GAP);
        div()
            .absolute()
            .top(self.bar_h(look) + look.z(4.) + look.gap())
            .left(self.title_lead(look) + look.gap())
            .occlude()
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.gap_sliding = None))
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                match this.gap_sliding {
                    Some(which) if ev.pressed_button == Some(MouseButton::Left) => this.gap_slide_to(which, ev.position.x, window, cx),
                    _ => this.gap_sliding = None,
                }
            }))
            .w(look.z(248.))
            .p(look.z(12.))
            .flex()
            .flex_col()
            .gap(look.z(10.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(6.))
            .text_size(look.text(-1.))
            .text_color(look.c(ui.text))
            .child(toggle("gaps", "Window gaps", u.gaps, |u| u.gaps = !u.gaps))
            .child(slider(0, "Gap", u.gap, (u.gap - min) / (max - min), !u.gaps))
            .child(toggle("rounded", "Rounded corners", u.rounded, |u| u.rounded = !u.rounded))
            .child(slider(1, "Corner radius", u.radius, u.radius / qur_config::MAX_RADIUS, !u.rounded))
            .child(toggle("border", "Tile border", u.tile_border, |u| u.tile_border = !u.tile_border))
            .child(
                div()
                    .text_size(look.text(-2.))
                    .text_color(look.c(ui.text_muted))
                    .child("The gaps show your desktop. Below 100% opacity with blur on, they're blurred too."),
            )
            .into_any_element()
    }
}

/// A plain name for an accent swatch, from its hue.
fn hue_name(c: Rgba) -> &'static str {
    let [r, g, b, _] = c.to_f32();
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max - min < 0.08 {
        return "gray";
    }
    let d = max - min;
    let h = if max == r { 60. * (((g - b) / d) % 6.) } else if max == g { 60. * ((b - r) / d + 2.) } else { 60. * ((r - g) / d + 4.) };
    let h = if h < 0. { h + 360. } else { h };
    match h as u32 {
        0..=14 | 345..=360 => "red",
        15..=39 => "orange",
        40..=69 => "yellow",
        70..=164 => "green",
        165..=199 => "teal",
        200..=254 => "blue",
        255..=289 => "purple",
        _ => "pink",
    }
}
