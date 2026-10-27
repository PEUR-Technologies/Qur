//! Sessions in the stage: selecting them, terminals, layouts and peeks, and
//! drawing the stage.

use super::*;

impl Workspace {
    pub(super) fn select(&mut self, s: Option<Selected>, window: &mut Window, cx: &mut Context<Self>) {
        // The agent you leave starts its idle clock now.
        if let Some(a) = self.selected_agent() {
            a.update(cx, |a, _| a.touch());
        }
        self.selected = s;
        match &self.selected {
            Some(Selected::Agent(id)) | Some(Selected::TermTask(id)) => {
                let id = id.clone();
                self.seen(&id, window, cx);
            }
            _ => {}
        }
        // The empty window has no file tree (it would list the home folder).
        if !self.home {
            let root = self.current_root(cx);
            self.tree_for(&root, window, cx);
        }
        match &self.selected {
            Some(Selected::Agent(id)) => {
                if let Some(a) = self.agent(id).cloned() {
                    // A stopped task with a saved session starts so its
                    // history loads, and one released for being idle picks
                    // up again. No prompt is sent.
                    a.update(cx, |a, cx| a.wake(cx));
                    let f = a.read(cx).prompt.read(cx).focus.clone();
                    window.focus(&f, cx);
                }
            }
            Some(Selected::TermTask(id)) => {
                let id = id.clone();
                self.show_term_task(&id, window, cx);
            }
            Some(Selected::Shell(tid)) => {
                let tid = *tid;
                self.terms.update(cx, |p, cx| p.activate_id(tid, cx));
                let f = self.terms.read(cx).focus.clone();
                window.focus(&f, cx);
            }
            Some(Selected::Git) => {
                self.git_panel(window, cx);
                self.suggest_commit(cx);
                window.focus(&self.focus, cx);
            }
            Some(Selected::Search) => {
                if let Some(s) = &self.search {
                    s.update(cx, |s, cx| s.focus_query(window, cx));
                }
            }
            None => window.focus(&self.focus, cx),
        }
        self.sync_menus(cx);
        cx.notify();
    }

    pub(super) fn new_terminal_in(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let tid = self.terms.update(cx, |p, cx| {
            let label = p.label_for(&Kind::Shell);
            p.open(&dir, Kind::Shell, label, cx)
        });
        if let Some(tid) = tid {
            self.shells.push(tid);
            self.select(Some(Selected::Shell(tid)), window, cx);
        }
    }

    /// Always the project folder; Open Worktree in Terminal is the way into an agent's worktree.
    pub(crate) fn new_terminal(&mut self, _: &qur::NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.new_terminal_in(self.root.clone(), window, cx)
    }

    /// Split the selected terminal (a shell or an Agent Terminal): a new shell
    /// in the project folder, like Cmd+T. Below it when `vertical` is
    /// `Some(true)`; `None` halves it along its longer side (dwindle).
    fn split_terminal(&mut self, vertical: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
        let live = match &self.selected {
            Some(Selected::Shell(_)) => true,
            Some(Selected::TermTask(id)) => self.term_task(id).and_then(|t| t.term_id).is_some(),
            _ => false,
        };
        if !live {
            return self.flash("Select a terminal to split", cx);
        }
        let root = self.root.clone();
        self.terms.update(cx, |p, cx| match vertical {
            Some(v) => p.split_active(&root, v, false, cx),
            None => p.dwindle_active(&root, cx),
        });
        let f = self.terms.read(cx).focus.clone();
        window.focus(&f, cx);
    }

    /// Cmd+Enter (like Mod+Enter in i3): another terminal, the ones on screen
    /// laid out as a grid of near-square panes; with no terminal shown, a new one.
    pub(super) fn tile_terminal(&mut self, _: &ta::Tile, window: &mut Window, cx: &mut Context<Self>) {
        let live = match &self.selected {
            Some(Selected::Shell(_)) => true,
            Some(Selected::TermTask(id)) => self.term_task(id).and_then(|t| t.term_id).is_some(),
            _ => false,
        };
        if !live {
            return self.new_terminal(&qur::NewTerminal, window, cx);
        }
        let root = self.root.clone();
        self.terms.update(cx, |p, cx| p.grid_active(&root, cx));
        let f = self.terms.read(cx).focus.clone();
        window.focus(&f, cx);
    }

    pub(super) fn split_right(&mut self, _: &ta::SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        self.split_terminal(None, window, cx)
    }
    pub(super) fn split_down(&mut self, _: &ta::SplitDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split_terminal(Some(true), window, cx)
    }

    /// A terminal closed (exit or Cmd+W): forget shells; terminal tasks keep
    /// their row (the task persists) and restart when selected again.
    pub(super) fn prune_closed_terms(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.terms.read(cx);
        let live: Vec<usize> = panel.sessions.iter().map(|s| s.id).collect();
        // A split shell's row moves to the pane beside it when its own closes.
        for s in &mut self.shells {
            if !live.contains(s)
                && let Some(n) = panel.successor(*s)
            {
                if self.selected == Some(Selected::Shell(*s)) {
                    self.selected = Some(Selected::Shell(n));
                }
                *s = n;
            }
        }
        self.shells.retain(|s| live.contains(s));
        self.runs.retain(|_, s| live.contains(s));
        for t in &mut self.term_tasks {
            if t.term_id.is_some_and(|id| !live.contains(&id)) {
                t.term_id = None;
            }
        }
        let sel_gone = matches!(&self.selected, Some(Selected::Shell(s)) if !self.shells.contains(s));
        if sel_gone {
            let next = self.session_order().into_iter().last();
            self.select(next, window, cx);
        }
    }

    pub(super) fn toggle_terminal(&mut self, _: &qur::ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        match self.shells.last().copied() {
            Some(s) => self.select(Some(Selected::Shell(s)), window, cx),
            None => self.new_terminal(&qur::NewTerminal, window, cx),
        }
    }

    pub(super) fn close_terminal(&mut self, _: &qur::CloseTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let tid = match &self.selected {
            Some(Selected::Shell(s)) => Some(*s),
            Some(Selected::TermTask(id)) => self.term_task(id).and_then(|t| t.term_id),
            Some(Selected::Agent(_)) => return self.discard_task(&qur::DiscardTask, window, cx),
            _ => None,
        };
        // The last shell pane anywhere: Cmd+W closes the window, like a terminal app.
        let last = matches!(self.selected, Some(Selected::Shell(_))) && self.terms.read(cx).sessions.iter().filter(|s| s.kind == Kind::Shell).count() == 1;
        if last {
            return self.close_window(&qur::CloseWindow, window, cx);
        }
        if let Some(tid) = tid {
            self.terms.update(cx, |p, cx| p.close_newest(tid, cx));
        }
    }

    fn step_session(&mut self, d: isize, window: &mut Window, cx: &mut Context<Self>) {
        let order = self.session_order();
        if order.is_empty() {
            return;
        }
        let cur = self.selected.as_ref().and_then(|s| order.iter().position(|o| o == s));
        let n = order.len() as isize;
        let next = match cur {
            Some(i) => (i as isize + d).rem_euclid(n) as usize,
            None => 0,
        };
        self.select(Some(order[next].clone()), window, cx);
    }

    /// Ctrl+T: pick an agent or terminal by name. The list starts after the
    /// one you're in (so Ctrl+T, Return goes to the next), which comes last.
    pub(super) fn switch_session(&mut self, _: &qur::SwitchSession, window: &mut Window, cx: &mut Context<Self>) {
        let mut order = self.session_order();
        if order.is_empty() {
            return self.flash("No agents or terminals yet", cx);
        }
        if let Some(i) = self.selected.as_ref().and_then(|s| order.iter().position(|o| o == s)) {
            order.rotate_left(i + 1);
        }
        let panel = self.terms.read(cx);
        let items = order
            .iter()
            .map(|s| {
                let (title, kind) = match s {
                    Selected::Agent(id) => (self.agent(id).map(|a| a.read(cx).record.title.clone()).unwrap_or_default(), "Agent"),
                    Selected::TermTask(id) => (self.term_task(id).map(|t| t.record.title.clone()).unwrap_or_default(), "Agent Terminal"),
                    Selected::Shell(tid) => (panel.session_by_id(*tid).map(|s| s.label.clone()).unwrap_or_default(), "Terminal"),
                    _ => (String::new(), ""),
                };
                let item = PickItem::new(title).detail(kind);
                if Some(s) == self.selected.as_ref() { item.right("current") } else { item }
            })
            .collect();
        self.open_picker("Switch to an agent or terminal…", items, PickKind::Sessions(order), false, window, cx);
    }

    pub(super) fn next_session(&mut self, _: &qur::NextAgent, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(1, window, cx)
    }
    pub(super) fn prev_session(&mut self, _: &qur::PrevAgent, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(-1, window, cx)
    }
    pub(super) fn next_terminal(&mut self, _: &qur::NextTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(1, window, cx)
    }
    pub(super) fn prev_terminal(&mut self, _: &qur::PrevTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(-1, window, cx)
    }
    pub(super) fn select_session(&mut self, a: &qur::SelectSession, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.session_order().get(a.index).cloned() {
            self.select(Some(s), window, cx);
        }
    }

    pub(super) fn focus_prompt(&mut self, _: &qur::FocusPrompt, window: &mut Window, cx: &mut Context<Self>) {
        let target = match &self.selected {
            Some(Selected::Agent(id)) => Some(id.clone()),
            _ => self.agent_ids().into_iter().next(),
        };
        if let Some(id) = target {
            if self.selected != Some(Selected::Agent(id.clone())) {
                self.select(Some(Selected::Agent(id.clone())), window, cx);
            }
            if let Some(a) = self.agent(&id) {
                let f = a.read(cx).prompt.read(cx).focus.clone();
                window.focus(&f, cx);
            }
        }
    }

    /// "/": put the keyboard in the input field of what's on screen (the
    /// agent's prompt, the search box, the terminal), like a browser's search.
    pub(super) fn focus_input(&mut self, _: &qur::FocusInput, window: &mut Window, cx: &mut Context<Self>) {
        let f = match &self.selected {
            Some(Selected::Agent(id)) => self.agent(id).map(|a| a.read(cx).prompt.read(cx).focus.clone()),
            Some(Selected::Search) => self.search.as_ref().map(|s| s.read(cx).query.read(cx).focus.clone()),
            Some(Selected::Shell(_) | Selected::TermTask(_)) => Some(self.terms.read(cx).focus.clone()),
            _ => None,
        };
        if let Some(f) = f {
            window.focus(&f, cx);
        }
    }

    /// View > Peek Layout A/B/C: reset both docks to that preset.
    pub(super) fn set_layout(&mut self, l: Layout, cx: &mut Context<Self>) {
        self.layout = l;
        Docks::update(cx, dock::Kind::Acp, |d| *d = dock::Dock::preset(dock::Kind::Acp, l.as_str()));
        Docks::update(cx, dock::Kind::Terminal, |d| *d = dock::Dock::preset(dock::Kind::Terminal, l.as_str()));
        for a in &self.agents {
            a.update(cx, |a, cx| a.set_layout(l, cx));
        }
        self.config.peek_layout = l.as_str().into();
        self.write_setting(false, "peek_layout", Some(l.as_str().into()), cx);
        let name = match l {
            Layout::A => "A · watch + side notes",
            Layout::B => "B · one view at a time",
            Layout::C => "C · watch + drawer",
        };
        self.flash(format!("Peek layout {name}"), cx);
        self.sync_menus(cx);
        self.sync_settings(cx);
        cx.notify();
    }
    pub(super) fn cycle_layout(&mut self, _: &qur::CyclePeekLayout, _: &mut Window, cx: &mut Context<Self>) {
        let l = self.layout.next();
        self.set_layout(l, cx)
    }
    pub(super) fn layout_a(&mut self, _: &qur::PeekLayoutA, _: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(Layout::A, cx)
    }
    pub(super) fn layout_b(&mut self, _: &qur::PeekLayoutB, _: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(Layout::B, cx)
    }
    pub(super) fn layout_c(&mut self, _: &qur::PeekLayoutC, _: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(Layout::C, cx)
    }
    /// Cmd+1..4 (layout B): switches the selected agent's view; Live/terminal for terminal tasks.
    /// Cmd+1..5: bring that panel to the front, wherever it's docked.
    fn peek(&mut self, v: View, cx: &mut Context<Self>) {
        match self.selected.clone() {
            Some(Selected::Agent(id)) => {
                if let Some(a) = self.agent(&id).cloned() {
                    a.update(cx, |a, cx| a.set_view(v, cx));
                }
            }
            Some(Selected::TermTask(id))
                if self.term_task(&id).is_some() && dock::Kind::Terminal.panels().contains(&v.panel()) =>
            {
                Docks::update(cx, dock::Kind::Terminal, |d| d.activate(v.panel()));
            }
            _ => {}
        }
        cx.notify();
    }
    pub(super) fn peek_live(&mut self, _: &qur::PeekLive, _: &mut Window, cx: &mut Context<Self>) {
        self.peek(View::Live, cx)
    }
    pub(super) fn peek_todo(&mut self, _: &qur::PeekTodo, _: &mut Window, cx: &mut Context<Self>) {
        self.peek(View::Todo, cx)
    }
    pub(super) fn peek_chat(&mut self, _: &qur::PeekChat, _: &mut Window, cx: &mut Context<Self>) {
        self.peek(View::Chat, cx)
    }
    pub(super) fn peek_changes(&mut self, _: &qur::PeekChanges, _: &mut Window, cx: &mut Context<Self>) {
        self.peek(View::Changes, cx)
    }
    /// Cmd+5: the selected agent's Browser (any layout).
    pub(super) fn peek_browser(&mut self, _: &qur::PeekBrowser, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.selected_agent() {
            a.update(cx, |a, cx| {
                let on = !a.shows_browser(cx);
                a.show_browser(on, cx)
            });
        } else if matches!(self.selected, Some(Selected::TermTask(_))) {
            let on = !Docks::get(cx, dock::Kind::Terminal).is_visible(dock::Panel::Browser);
            Docks::update(cx, dock::Kind::Terminal, |d| d.activate(if on { dock::Panel::Browser } else { dock::Panel::Session }));
            cx.notify();
        }
    }
    /// Cmd+Shift+C: open the Browser if needed and toggle Inspect.
    pub(super) fn toggle_inspect(&mut self, _: &qur::ToggleInspect, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.selected_agent() {
            a.update(cx, |a, cx| {
                if !a.shows_browser(cx) {
                    a.show_browser(true, cx);
                }
                a.browser.update(cx, |b, cx| b.toggle_inspect(cx));
            });
        } else if let Some(Selected::TermTask(id)) = &self.selected
            && let Some(browser) = self.term_task(id).map(|t| t.browser.clone())
        {
            Docks::update(cx, dock::Kind::Terminal, |d| d.activate(dock::Panel::Browser));
            browser.update(cx, |b, cx| b.toggle_inspect(cx));
            cx.notify();
        }
    }
    pub(super) fn selected_live(&self, cx: &App) -> Option<Entity<LivePane>> {
        match &self.selected {
            Some(Selected::Agent(id)) => self.agent(id).map(|a| a.read(cx).live.clone()),
            Some(Selected::TermTask(id)) => self.term_task(id).map(|t| t.live.clone()),
            _ => None,
        }
    }
    pub(super) fn follow_agent(&mut self, _: &qur::FollowAgent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(l) = self.selected_live(cx) {
            l.update(cx, |l, cx| {
                l.follow = !l.follow;
                cx.notify();
            });
        }
        self.sync_menus(cx);
    }
    pub(super) fn take_over(&mut self, _: &qur::TakeOver, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.selected_agent() {
            a.update(cx, |a, cx| a.stop(cx));
        }
    }

    pub(super) fn focus_stage(&self, window: &mut Window, cx: &mut App) {
        match &self.selected {
            Some(Selected::Agent(id)) => {
                if let Some(a) = self.agent(id) {
                    let f = a.read(cx).prompt.read(cx).focus.clone();
                    window.focus(&f, cx);
                }
            }
            Some(Selected::Shell(_)) | Some(Selected::TermTask(_)) => {
                let f = self.terms.read(cx).focus.clone();
                window.focus(&f, cx);
            }
            Some(Selected::Search) => {
                if let Some(s) = &self.search {
                    let f = s.read(cx).query.read(cx).focus.clone();
                    window.focus(&f, cx);
                }
            }
            Some(Selected::Git) | None => window.focus(&self.focus, cx),
        }
    }

    pub(super) fn stage(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        match &self.selected {
            Some(Selected::Agent(id)) => {
                if let Some(a) = self.agent(id) {
                    return a.clone().into_any_element();
                }
            }
            Some(Selected::TermTask(id)) => {
                if let Some(t) = self.term_task(id) {
                    let term = div().size_full().bg(look.surface(ui.panel_bg)).pt(look.z(6.)).child(self.terms.clone());
                    // The agent's terminal is the whole stage (decision 27). Review
                    // Changes still swaps it for the diff until Keep Working.
                    // An arrangeable dock too (dock.rs); by default the
                    // terminal fills it (decision 27).
                    let mut term = Some(term);
                    let dock = dock::render(dock::Kind::Terminal, look, cx, &mut |p, _| match p {
                        dock::Panel::Session => term.take().map(|t| t.into_any_element()).unwrap_or_else(|| div().into_any_element()),
                        dock::Panel::Live => div().size_full().child(t.live.clone()).into_any_element(),
                        dock::Panel::Changes => div().size_full().child(t.changes.clone()).into_any_element(),
                        dock::Panel::Browser => div().size_full().child(t.browser.clone()).into_any_element(),
                        dock::Panel::Todo => todo_panel(&t.todos.todos, t.accent, look).into_any_element(),
                    });
                    // The stage region's one translucent layer (with window
                    // gaps, each dock pane paints its own instead).
                    self.terms.read(cx).standalone.set(false);
                    return div().size_full().bg(look.region_bg(ui.panel_bg)).child(dock).into_any_element();
                }
            }
            Some(Selected::Shell(_)) => {
                // With window gaps the terminal's panes are the tiles.
                self.terms.read(cx).standalone.set(true);
                return div()
                    .size_full()
                    .bg(look.region_bg(ui.panel_bg))
                    .when(look.tiles.is_none(), |d| d.pt(look.z(6.)))
                    .child(self.terms.clone())
                    .into_any_element();
            }
            Some(Selected::Git) => {
                if let Some(g) = &self.git {
                    return Self::stage_tile(look, ui.editor_bg, g.clone().into_any_element());
                }
            }
            Some(Selected::Search) => {
                if let Some(s) = &self.search {
                    return Self::stage_tile(look, ui.editor_bg, s.clone().into_any_element());
                }
            }
            None if self.home => return self.home_stage(look, cx),
            None => {}
        }
        let hint = |label: &'static str, keys: &str| {
            div()
                .flex()
                .justify_between()
                .gap(look.z(32.))
                .child(div().text_color(look.c(ui.text_secondary)).child(label))
                .child(div().text_color(look.c(ui.text_muted)).child(commands::display_keys(keys)))
        };
        look.tile(div(), ui.editor_bg)
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(look.text(0.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.z(8.))
                    .child(hint("New Agent Task", "cmd-shift-n"))
                    .child(hint("New Agent Terminal", "cmd-shift-t"))
                    .child(hint("New Terminal", "cmd-t"))
                    .child(hint("Go to File", "cmd-p"))
                    .child(hint("Command Palette", "cmd-shift-p")),
            )
            .into_any_element()
    }

    /// A view that paints its own region background (`Look::region_bg`),
    /// wrapped as one tile when window gaps are on.
    fn stage_tile(look: &Look, fill: Rgba, view: gpui::AnyElement) -> gpui::AnyElement {
        if look.tiles.is_none() {
            return view;
        }
        look.tile(div(), fill).size_full().child(view).into_any_element()
    }
}

/// An Agent Terminal's To-do tab (same look as a structured agent's).
fn todo_panel(todos: &[qur_agents::todos::Todo], accent: Rgba, look: &Look) -> gpui::Div {
    use qur_agents::todos::TodoStatus;
    let ui = &look.theme.ui;
    if todos.is_empty() {
        return div().size_full().flex().items_center().justify_center().text_color(look.c(ui.text_muted)).child("No plan yet.");
    }
    let done = todos.iter().filter(|t| t.status == TodoStatus::Completed).count();
    div()
        .size_full()
        .px(look.z(16.))
        .py(look.z(8.))
        .flex()
        .flex_col()
        .gap(look.z(3.))
        .text_size(look.text(0.))
        .child(div().font_weight(gpui::FontWeight::MEDIUM).text_color(look.c(ui.text_muted)).child(format!("TO-DO  {done}/{}", todos.len())))
        .children(todos.iter().map(|t| {
            let (g, c) = match t.status {
                TodoStatus::Completed => ("●", ui.text_muted),
                TodoStatus::InProgress => ("◐", ui.text),
                TodoStatus::Pending => ("○", ui.text_secondary),
            };
            div()
                .flex()
                .gap(look.z(8.))
                .text_color(look.c(c))
                .when(t.status == TodoStatus::InProgress, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                .child(div().flex_none().text_color(look.c(accent)).child(g))
                .child(div().min_w(px(0.)).child(t.content.clone()))
        }))
}
