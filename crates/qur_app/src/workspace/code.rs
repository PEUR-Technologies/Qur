//! The code modal (open, save, navigate, find) and project search.

use super::*;

impl Workspace {
    /// Find in Project (Cmd+Shift+F), Replace in Project (Cmd+Shift+H), View >
    /// Search: the Search view over the tree you're looking at (the selected
    /// agent's worktree, or the main checkout).
    fn show_search(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.current_root(cx);
        // Seed the query with a one-line selection in the code modal.
        let seed = self
            .code
            .as_ref()
            .and_then(|c| c.read(cx).selected_text())
            .filter(|t| !t.is_empty() && !t.contains('\n'))
            // Dev/test: QUR_DEBUG_SEARCH=<query> fills the field.
            .or_else(|| std::env::var("QUR_DEBUG_SEARCH").ok());
        let panel = match &self.search {
            Some(s) => s.clone(),
            None => {
                let s = cx.new(|cx| SearchPanel::new(root.clone(), window, cx));
                let sub = cx.subscribe_in(&s, window, |this, panel, ev: &SearchEvent, window, cx| match ev {
                    SearchEvent::Open(path, line, col) => {
                        let path = path.clone();
                        this.open_path_at(&path, Some(Point { line: *line, col: *col }), window, cx);
                    }
                    SearchEvent::Message(m) => this.flash(m.clone(), cx),
                    SearchEvent::ConfirmReplace { files, matches } => {
                        let detail = format!(
                            "Replace {matches} match{} in {files} file{} under {}? Open files reload from disk.",
                            if *matches == 1 { "" } else { "es" },
                            if *files == 1 { "" } else { "s" },
                            panel.read(cx).root.display()
                        );
                        let answer = ask(window, PromptLevel::Warning, "Replace All", Some(&detail), &["Replace", "Cancel"], cx);
                        let panel = panel.clone();
                        cx.spawn_in(window, async move |_, cx| {
                            if answer.await.ok() == Some(0) {
                                panel.update(cx, |p, cx| p.replace_all(cx));
                            }
                        })
                        .detach();
                    }
                });
                self._subs.push(sub);
                self.search = Some(s.clone());
                s
            }
        };
        panel.update(cx, |p, cx| {
            if let Some(seed) = seed {
                p.query.update(cx, |q, cx| q.set_text(&seed, cx));
                p.run(cx);
            }
            p.set_root(root, cx);
        });
        self.select(Some(Selected::Search), window, cx);
        if replace {
            panel.update(cx, |p, cx| p.focus_replace(window, cx));
        }
    }

    pub(super) fn find_in_project(&mut self, _: &qur::FindInProject, window: &mut Window, cx: &mut Context<Self>) {
        self.show_search(false, window, cx);
    }

    pub(super) fn replace_in_project(&mut self, _: &qur::ReplaceInProject, window: &mut Window, cx: &mut Context<Self>) {
        self.show_search(true, window, cx);
    }

    pub(super) fn search_panel(&mut self, _: &qur::SearchPanel, window: &mut Window, cx: &mut Context<Self>) {
        self.show_search(false, window, cx);
    }

    // -- the code modal ------------------------------------------------------------

    pub fn open_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.open_path_at(path, None, window, cx)
    }

    pub(super) fn open_path_at(&mut self, path: &Path, at: Option<Point>, window: &mut Window, cx: &mut Context<Self>) {
        self.record_nav(cx);
        let buf = match self.buffers.find(path, cx) {
            Some(b) => b,
            None => match self.load_buffer(path, window, cx) {
                Ok(b) => b,
                Err(e) => {
                    self.flash(format!("Couldn't open {}: {e}", path.display()), cx);
                    return;
                }
            },
        };
        self.buffers.touch(&buf);
        if self.code.is_none() {
            self.code_prev_focus = window.focused(cx);
        }
        self.code = Some(buf.clone());
        if let Some(p) = at {
            buf.update(cx, |b, cx| b.set_cursor(p, cx));
        }
        for t in self.trees.values() {
            t.update(cx, |t, cx| {
                t.current = Some(path.to_path_buf());
                cx.notify();
            });
        }
        self.push_nav(cx);
        let f = buf.read(cx).focus.clone();
        window.focus(&f, cx);
        self.sync_menus(cx);
        cx.notify();
    }

    fn load_buffer(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> std::io::Result<Entity<EditorView>> {
        let (wrap, tab, vim) = (self.config.editor.soft_wrap, self.config.editor.tab_width, self.config.keys.vim);
        let loaded = EditorView::load(path)?;
        let view = cx.new(|cx| {
            let mut v = EditorView::new(loaded, wrap, tab, cx);
            v.set_vim(vim, cx);
            v
        });
        let sub = cx.subscribe_in(&view, window, Self::on_editor_event);
        self._subs.push(sub);
        Ok(view)
    }

    /// Reopen the files the project had open last time (quietly: a file that's
    /// gone is skipped). The code view comes back if it was showing. Clones and
    /// worktrees share a project id, so files outside this root are skipped too.
    pub(super) fn restore_open_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = qur_config::ProjectState::load(&self.project_id);
        // Oldest first, so the list ends up most recent first.
        for f in state.files.iter().take(qur_config::MAX_OPEN_FILES).rev() {
            if !f.path.starts_with(&self.root) || !f.path.is_file() || self.buffers.find(&f.path, cx).is_some() {
                continue;
            }
            let Ok(buf) = self.load_buffer(&f.path, window, cx) else { continue };
            let at = Point { line: f.line, col: f.col };
            buf.update(cx, |b, cx| b.set_cursor(at, cx));
            self.buffers.touch(&buf);
        }
        if state.code_open
            && let Some(p) = self.buffers.list.first().and_then(|b| b.read(cx).path().map(Path::to_path_buf))
        {
            self.open_path(&p, window, cx);
        }
    }

    pub(super) fn save_open_files(&self, cx: &App) {
        let files = self
            .buffers
            .list
            .iter()
            .filter_map(|b| {
                let b = b.read(cx);
                let at = b.cursor();
                b.path().map(|p| qur_config::OpenFile { path: p.to_path_buf(), line: at.line, col: at.col })
            })
            .collect();
        let state = qur_config::ProjectState { files, code_open: self.code.is_some() };
        if let Err(e) = state.save(&self.project_id) {
            crate::log(&format!("saving open files: {e}"));
        }
    }

    fn close_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.code.take().is_some() {
            match self.code_prev_focus.take() {
                Some(f) => window.focus(&f, cx),
                None => self.focus_stage(window, cx),
            }
            self.sync_menus(cx);
            cx.notify();
        }
    }

    fn on_editor_event(&mut self, view: &Entity<EditorView>, ev: &EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            EditorEvent::Changed => {
                let v = view.read(cx);
                let path = v.path().map(Path::to_path_buf);
                let clean = !v.is_dirty();
                if clean && path.as_deref() == Some(qur_config::config_path().as_path()) {
                    self.reload_settings(window, cx);
                    self.broadcast_reload(cx);
                }
                if clean && path.as_deref() == Some(qur_config::keymap_path().as_path()) {
                    self.reload_keymap(cx);
                }
                if clean && path.as_deref() == Some(qur_agents::roster_path().as_path()) {
                    let (r, err) = Roster::load();
                    self.roster = r;
                    if let Some(e) = err {
                        self.flash(e, cx);
                    }
                }
                cx.notify();
            }
            EditorEvent::Error(e) => self.flash(e.clone(), cx),
            EditorEvent::WrapToggled => self.sync_menus(cx),
            EditorEvent::CloseRequested => self.close_code(window, cx),
        }
    }

    fn record_nav(&mut self, cx: &App) {
        if let Some(a) = &self.code {
            let a = a.read(cx);
            if let (Some(p), Some(entry)) = (a.path(), self.nav.get_mut(self.nav_ix))
                && entry.0 == p
            {
                entry.1 = a.cursor();
            }
        }
    }

    fn push_nav(&mut self, cx: &App) {
        let Some(a) = &self.code else { return };
        let a = a.read(cx);
        let Some(p) = a.path() else { return };
        if self.nav.get(self.nav_ix).is_some_and(|e| e.0 == p) {
            return;
        }
        self.nav.truncate(self.nav_ix + 1);
        self.nav.push((p.to_path_buf(), a.cursor()));
        self.nav_ix = self.nav.len() - 1;
    }

    fn nav_to(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.record_nav(cx);
        let Some((p, at)) = self.nav.get(ix).cloned() else { return };
        let saved = std::mem::take(&mut self.nav);
        self.open_path_at(&p, Some(at), window, cx);
        self.nav = saved;
        self.nav_ix = ix;
    }

    pub(super) fn save_all(&mut self, _: &qur::SaveAll, _: &mut Window, cx: &mut Context<Self>) {
        for b in self.buffers.dirty(cx) {
            b.update(cx, |b, cx| b.save(cx));
        }
    }
    pub(super) fn revert(&mut self, _: &qur::RevertFile, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = &self.code {
            a.update(cx, |a, cx| a.reload_from_disk(cx));
        }
    }
    pub(super) fn recent_file(&mut self, _: &qur::RecentFile, window: &mut Window, cx: &mut Context<Self>) {
        let skip = usize::from(self.code.is_some());
        if let Some(p) = self.buffers.list.get(skip).and_then(|b| b.read(cx).path().map(Path::to_path_buf)) {
            self.open_path(&p, window, cx);
        }
    }
    pub(super) fn nav_back(&mut self, _: &qur::NavBack, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav_ix > 0 {
            let ix = self.nav_ix - 1;
            self.nav_to(ix, window, cx);
        }
    }
    pub(super) fn nav_forward(&mut self, _: &qur::NavForward, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav_ix + 1 < self.nav.len() {
            let ix = self.nav_ix + 1;
            self.nav_to(ix, window, cx);
        }
    }

    fn with_code(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut EditorView, &mut Context<EditorView>)) {
        if let Some(a) = self.code.clone() {
            a.update(cx, f);
        }
    }
    pub(super) fn find(&mut self, _: &ea::Find, window: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.open_find(false, window, cx))
    }
    pub(super) fn replace(&mut self, _: &ea::Replace, window: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.open_find(true, window, cx))
    }
    pub(super) fn find_next(&mut self, _: &ea::FindNext, window: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.find_next(window, cx))
    }
    pub(super) fn find_prev(&mut self, _: &ea::FindPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.find_prev(window, cx))
    }
    pub(super) fn use_selection(&mut self, _: &ea::UseSelectionForFind, window: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.use_selection_for_find(window, cx))
    }
    pub(super) fn soft_wrap(&mut self, _: &ea::ToggleSoftWrap, _: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.toggle_soft_wrap(cx));
        self.sync_menus(cx);
    }
    // Edit-menu items with nothing focused that handles them stay enabled.
    pub(super) fn noop_undo(&mut self, _: &edit::Undo, _: &mut Window, _: &mut Context<Self>) {}
    pub(super) fn noop_redo(&mut self, _: &edit::Redo, _: &mut Window, _: &mut Context<Self>) {}

    pub(super) fn code_modal(&self, look: &Look, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let code = self.code.clone()?;
        let ui = &look.theme.ui;
        let vim = code.read(cx).vim_status();
        let (title, dirty, owner) = {
            let c = code.read(cx);
            let path = c.path().map(Path::to_path_buf).unwrap_or_default();
            // Which tree is this file in: the main checkout or a task's worktree?
            let owner = self
                .agents
                .iter()
                .map(|a| (a.read(cx).record.worktree.clone(), a.read(cx).record.title.clone()))
                .chain(self.term_tasks.iter().map(|t| (t.record.worktree.clone(), t.record.title.clone())))
                .find(|(wt, _)| path.starts_with(wt));
            let rel = match &owner {
                Some((wt, _)) => path.strip_prefix(wt).unwrap_or(&path).to_string_lossy().into_owned(),
                None => path.strip_prefix(&self.root).unwrap_or(&path).to_string_lossy().into_owned(),
            };
            (rel, c.is_dirty(), owner.map(|(_, t)| t))
        };
        Some(
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.35))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_code(window, cx)))
                .child(
                    div()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .w(gpui::relative(0.86))
                        .h(gpui::relative(0.88))
                        .flex()
                        .flex_col()
                        .rounded(look.z(8.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(look.c(ui.border_strong))
                        .bg(look.c(ui.editor_bg))
                        .child(
                            div()
                                .flex_none()
                                .h(look.z(32.))
                                .px(look.z(12.))
                                .flex()
                                .items_center()
                                .gap(look.z(8.))
                                .border_b_1()
                                .border_color(look.c(ui.border))
                                .text_size(look.text(-1.))
                                .child(div().min_w(px(0.)).truncate().text_color(look.c(ui.text)).child(title))
                                .when(dirty, |d| d.child(div().size(look.z(6.)).flex_none().rounded(look.z(1.5)).bg(look.c(ui.text_secondary))))
                                .children(owner.map(|w| div().flex_none().text_color(look.c(ui.text_muted)).child(format!("in {w}"))))
                                .children(vim.map(|m| div().flex_none().ml(look.z(8.)).px(look.z(5.)).rounded(look.z(3.)).bg(look.c(ui.active_bg)).text_size(look.text(-2.)).font_family(look.code_font.clone()).text_color(look.c(ui.text_secondary)).child(m)))
                                .child(div().flex_1())
                                .child(div().flex_none().text_color(look.c(ui.text_muted)).child("esc to close")),
                        )
                        .child(div().flex_1().min_h(px(0.)).child(code)),
                )
                .into_any_element(),
        )
    }
}
