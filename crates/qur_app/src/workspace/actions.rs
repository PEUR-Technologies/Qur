//! Window and app menu actions: quit, hide, zoom, open, Help.

use super::*;

impl Workspace {
    // -- actions: app & window ---------------------------------------------------

    /// Keep Awake: run `caffeinate -dims` (no display, idle or system sleep)
    /// until toggled off or Qur exits.
    pub(super) fn caffeinate(&mut self, _: &qur::Caffeinate, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut c) = self.caffeinate.take() {
            let _ = c.kill();
            let _ = c.wait();
            self.flash("Keep Awake off", cx);
        } else {
            match std::process::Command::new("/usr/bin/caffeinate").args(["-dims", "-w", &std::process::id().to_string()]).spawn() {
                Ok(c) => {
                    self.caffeinate = Some(c);
                    self.flash("Keep Awake on: the Mac won't sleep while Qur is open", cx);
                }
                Err(e) => self.flash(format!("Couldn't start caffeinate: {e}"), cx),
            }
        }
        cx.notify();
    }

    pub(super) fn quit(&mut self, _: &qur::Quit, window: &mut Window, cx: &mut Context<Self>) {
        let dirty: Vec<_> = cx
            .windows()
            .into_iter()
            .filter_map(|h| h.downcast::<Workspace>())
            .flat_map(|w| w.read(cx).map(|w| w.buffers.dirty(cx)).unwrap_or_default())
            .collect();
        if dirty.is_empty() {
            return cx.quit();
        }
        let msg = format!("Save changes to {} file{} before quitting?", dirty.len(), if dirty.len() == 1 { "" } else { "s" });
        let answer = ask(window, PromptLevel::Warning, &msg, None, &["Save All", "Don't Save", "Cancel"], cx);
        cx.spawn(async move |_, cx| match answer.await {
            Ok(0) => cx.update(|cx| {
                for b in &dirty {
                    b.update(cx, |b, cx| b.save(cx));
                }
                cx.quit()
            }),
            Ok(1) => cx.update(|cx| cx.quit()),
            _ => {}
        })
        .detach();
    }

    /// Window close (button or Cmd+Shift+W): ask about unsaved files first.
    pub fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let dirty = self.buffers.dirty(cx);
        if dirty.is_empty() {
            self.shutdown(cx);
            return true;
        }
        let msg = format!("Save changes to {} file{}?", dirty.len(), if dirty.len() == 1 { "" } else { "s" });
        let answer = ask(window, PromptLevel::Warning, &msg, None, &["Save All", "Don't Save", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            let a = answer.await.unwrap_or(2);
            if a == 2 {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                if a == 0 {
                    for b in &dirty {
                        b.update(cx, |b, cx| b.save(cx));
                    }
                    if !this.buffers.dirty(cx).is_empty() {
                        return;
                    }
                } else {
                    // Edits dropped: the files stay in the list, as on disk.
                    this.code = None;
                    this.buffers.free(|b| Some(b.read(cx).spot()));
                }
                this.shutdown(cx);
                window.remove_window();
            })
            .ok();
        })
        .detach();
        false
    }

    pub(super) fn hide(&mut self, _: &qur::Hide, _: &mut Window, cx: &mut Context<Self>) {
        cx.hide()
    }
    pub(super) fn hide_others(&mut self, _: &qur::HideOthers, _: &mut Window, cx: &mut Context<Self>) {
        cx.hide_other_apps()
    }
    pub(super) fn show_all(&mut self, _: &qur::ShowAll, _: &mut Window, cx: &mut Context<Self>) {
        cx.unhide_other_apps()
    }
    pub(super) fn close_window(&mut self, _: &qur::CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_close(window, cx) {
            window.remove_window()
        }
    }
    pub(super) fn minimize(&mut self, _: &qur::Minimize, window: &mut Window, _: &mut Context<Self>) {
        window.minimize_window()
    }
    pub(super) fn zoom_window(&mut self, _: &qur::Zoom, window: &mut Window, _: &mut Context<Self>) {
        window.zoom_window()
    }
    pub(super) fn fullscreen(&mut self, _: &qur::ToggleFullScreen, window: &mut Window, _: &mut Context<Self>) {
        window.toggle_fullscreen()
    }
    pub(super) fn toggle_sidebar(&mut self, _: &qur::ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar = !self.sidebar;
        self.sync_menus(cx);
        cx.notify();
    }
    pub(super) fn reveal_files(&mut self, _: &qur::RevealInFiles, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar = true;
        let root = self.current_root(cx);
        let tree = self.tree_for(&root, window, cx);
        if let Some(p) = self.code.as_ref().and_then(|a| a.read(cx).path().map(Path::to_path_buf)) {
            tree.update(cx, |t, cx| {
                t.reveal(&p, cx);
            });
        }
        self.sync_menus(cx);
        cx.notify();
    }
    /// File > New Window: the empty window (an idle one is brought forward).
    pub(super) fn new_window(&mut self, _: &qur::NewWindow, window: &mut Window, cx: &mut Context<Self>) {
        // This window can't be looked at from inside its own update.
        if self.idle_home(cx) {
            return window.activate_window();
        }
        crate::open_home(None, None, cx);
    }
    pub(super) fn open_folder(&mut self, _: &qur::OpenFolder, _: &mut Window, cx: &mut Context<Self>) {
        crate::prompt_open_folder(cx);
    }
    pub(super) fn open_project(&mut self, a: &qur::OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        crate::open_project(a.path.clone(), None, cx);
    }

    /// Help > Run Doctor: `qur doctor`'s checks, in a sheet.
    pub(super) fn run_doctor(&mut self, _: &qur::RunDoctor, window: &mut Window, cx: &mut Context<Self>) {
        self.flash("Running the doctor…", cx);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let checks = bg.spawn(async move { crate::doctor::run() }).await;
            this.update_in(cx, |this, window, cx| {
                let fails = checks.iter().filter(|c| c.level == crate::doctor::Level::Fail).count();
                let text = crate::doctor::render(&checks);
                crate::log(&format!("doctor:\n{text}"));
                let title = if fails == 0 { "Qur Doctor: all good".to_string() } else { format!("Qur Doctor: {fails} problem(s)") };
                drop(ask(window, if fails == 0 { PromptLevel::Info } else { PromptLevel::Warning }, &title, Some(&text), &["OK"], cx));
                this.message = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn keyboard_shortcuts(&mut self, _: &qur::KeyboardShortcuts, window: &mut Window, cx: &mut Context<Self>) {
        // The palette lists every command with its keys.
        self.palette(&qur::CommandPalette, window, cx);
    }

    pub(super) fn show_logs(&mut self, _: &qur::ShowLogs, _: &mut Window, cx: &mut Context<Self>) {
        let dir = crate::log_dir();
        std::fs::create_dir_all(&dir).ok();
        if let Err(e) = std::process::Command::new("/usr/bin/open").arg(&dir).spawn() {
            self.flash(format!("Couldn't open {}: {e}", dir.display()), cx);
        }
    }

    pub(super) fn report_problem(&mut self, _: &qur::ReportProblem, _: &mut Window, cx: &mut Context<Self>) {
        // Opens the browser; Qur itself makes no network call, and a crash
        // report is only named in the issue: it's selected in Finder for the
        // user to attach if they want to.
        let report = crate::crash::recent();
        let url = crate::crash::issue_url(report.as_deref());
        if let Some(p) = &report {
            self.open_args(&["-R".as_ref(), p.as_os_str()], cx);
            self.flash(format!("{} is selected in Finder: drag it into the issue to attach it", p.file_name().unwrap_or_default().to_string_lossy()), cx);
        }
        self.open_args(&[url.as_ref()], cx);
    }
}
