//! One window = one project. The workspace owns the top bar, sidebar (agents +
//! Files dropdown), the editor area, the bottom terminal panel and the
//! overlays (pickers, go to line, transparency). It also applies the
//! project's look (theme and transparency, decisions 8 and 13).

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, Entity, FocusHandle, MouseButton, Pixels, PromptLevel,
    SharedString, Subscription, Task, Window, WindowAppearance, canvas, div, prelude::*, px, svg,
};
use qur_config::{Config, ProjectSettings, Value};
use qur_editor::Point;
use qur_theme::{Registry, ThemeChoice};

use crate::assets::{self, icon};
use crate::commands::{self, MenuState, edit, editor as ea, qur};
use crate::editor_view::{Buffers, EditorEvent, EditorView};
use crate::input::{InputEvent, TextInput};
use crate::look::Look;
use crate::picker::{PickItem, Picker, PickerEvent};
use crate::terminal::{Kind, PanelEvent, TerminalPanel};
use crate::tree_view::{TreeEvent, TreeView};

/// Height of our own top bar (the system titlebar is transparent underneath).
pub const TITLEBAR_H: f32 = 32.0;
/// Left inset that clears the traffic lights placed at (9, 9).
const TRAFFIC_LIGHTS_W: f32 = 78.0;
const SIDEBAR_W: f32 = 220.0;
const PANEL_H: f32 = 220.0;

struct AgentTask {
    n: usize,
    title: String,
    worktree: PathBuf,
    term_id: Option<usize>,
}

enum PickKind {
    Commands(Vec<&'static commands::Cmd>),
    Files(Vec<PathBuf>),
    Themes { names: Vec<String>, project_scope: bool },
    Projects(Vec<PathBuf>),
}

enum Overlay {
    Picker(Entity<Picker>, PickKind),
    GoTo(Entity<TextInput>),
    Transparency { project_scope: bool },
}

pub struct Workspace {
    focus: FocusHandle,
    pub root: PathBuf,
    project_id: String,
    config: Config,
    project: ProjectSettings,
    system_dark: bool,
    /// Theme being previewed in the picker (not saved yet).
    preview_theme: Option<String>,
    buffers: Buffers,
    active: Option<Entity<EditorView>>,
    tree: Entity<TreeView>,
    files_open: bool,
    panel: Entity<TerminalPanel>,
    panel_visible: bool,
    sidebar: bool,
    overlay: Option<Overlay>,
    prev_focus: Option<FocusHandle>,
    tasks: Vec<AgentTask>,
    /// Back/forward history: (file, cursor).
    nav: Vec<(PathBuf, Point)>,
    nav_ix: usize,
    message: Option<(SharedString, Instant)>,
    message_timer: Option<Task<()>>,
    file_list: Option<Vec<PathBuf>>,
    slider: Rc<Cell<Bounds<Pixels>>>,
    sliding: bool,
    title_drag: bool,
    first_frame: bool,
    _subs: Vec<Subscription>,
}

impl Workspace {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::mark("ws_new");
        let (config, err) = Config::load();
        let project_id = qur_config::project_id(&root);
        crate::mark("project_id");
        let project = ProjectSettings::load(&project_id);
        let tree = cx.new(|_| TreeView::new(&root));
        let panel = cx.new(TerminalPanel::new);
        let mut subs = vec![
            cx.subscribe_in(&tree, window, |this, _, ev: &TreeEvent, window, cx| match ev {
                TreeEvent::Open(p) => this.open_path(p, window, cx),
            }),
            cx.subscribe_in(&panel, window, |this, _, ev: &PanelEvent, window, cx| {
                if let PanelEvent::Closed = ev
                    && this.panel.read(cx).sessions.is_empty()
                {
                    this.panel_visible = false;
                    this.focus_editor(window, cx);
                    this.sync_menus(cx);
                }
                cx.notify();
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                this.system_dark = is_dark(window.appearance());
                this.apply_look(window, cx);
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.sync_menus(cx);
                    // Cheap way to pick up files created outside Qur until the M3 watcher.
                    this.tree.update(cx, |t, cx| t.refresh(cx));
                    this.file_list = None;
                }
            }),
        ];
        let _ = &mut subs;
        crate::mark("tree_loaded");
        let mut state = qur_config::State::load();
        state.touch_project(&root);
        state.save().ok();
        crate::mark("state_saved");
        let mut ws = Workspace {
            focus: cx.focus_handle(),
            root,
            project_id,
            config,
            project,
            system_dark: is_dark(window.appearance()),
            preview_theme: None,
            buffers: Buffers { list: Vec::new() },
            active: None,
            tree,
            files_open: false,
            panel,
            panel_visible: false,
            sidebar: true,
            overlay: None,
            prev_focus: None,
            tasks: Vec::new(),
            nav: Vec::new(),
            nav_ix: 0,
            message: None,
            message_timer: None,
            file_list: None,
            slider: Rc::new(Cell::new(Bounds::default())),
            sliding: false,
            title_drag: false,
            first_frame: false,
            _subs: subs,
        };
        ws.apply_look(window, cx);
        crate::mark("look_applied");
        // Something must hold focus, or shortcuts have nowhere to go before a file opens.
        window.focus(&ws.focus, cx);
        if let Some(e) = err {
            ws.flash(e, cx);
        }
        ws
    }

    pub fn project_name(&self) -> String {
        self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }

    // -- look (theme, transparency, zoom) -------------------------------------

    fn effective(&self) -> qur_config::Effective {
        qur_config::effective(&self.config, &self.project)
    }

    fn apply_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let eff = self.effective();
        let name = self.preview_theme.clone().unwrap_or(eff.theme);
        let choice = if name == qur_config::THEME_AUTO { ThemeChoice::Auto } else { ThemeChoice::Named(name) };
        let theme = Registry::global().resolve(&choice, self.system_dark);
        let min = theme.min_opacity().max(qur_theme::MIN_OPACITY);
        let code_font = if self.config.editor.font_family.is_empty() { default_code_font(cx) } else { self.config.editor.font_family.clone() };
        let look = Look {
            opacity: eff.opacity.clamp(min, 1.0),
            blur: eff.blur,
            zoom: self.config.ui.zoom,
            ui_font: assets::UI_FONT.into(),
            code_font: code_font.into(),
            ui_size: self.config.ui.font_size,
            code_size: self.config.editor.font_size,
            line_height: self.config.editor.line_height,
            theme,
        };
        window.set_background_appearance(look.background_appearance());
        Look::set(window, look, cx);
        window.refresh();
        cx.notify();
    }

    /// Re-read config.toml and this project's settings (after another window
    /// or a hand edit changed them).
    pub fn reload_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (config, err) = Config::load();
        self.config = config;
        self.project = ProjectSettings::load(&self.project_id);
        if let Some(e) = err {
            self.flash(e, cx);
        }
        let wrap = self.config.editor.soft_wrap;
        for b in &self.buffers.list {
            b.update(cx, |b, cx| b.set_soft_wrap(wrap, cx));
        }
        self.apply_look(window, cx);
        self.sync_menus(cx);
    }

    fn write_setting(&mut self, project_scope: bool, key: &str, value: Option<Value>, cx: &mut Context<Self>) {
        let path = if project_scope { qur_config::project_settings_path(&self.project_id) } else { qur_config::config_path() };
        if let Err(e) = qur_config::write_key(&path, key, value) {
            self.flash(format!("Couldn't save {}: {e}", path.display()), cx);
        }
    }

    /// Tell every window to re-read settings (a global value changed).
    fn broadcast_reload(&self, cx: &mut Context<Self>) {
        let me = cx.entity();
        for handle in cx.windows() {
            if let Some(ws) = handle.downcast::<Workspace>() {
                let _ = ws.update(cx, |ws, window, cx| {
                    if cx.entity() != me {
                        ws.reload_settings(window, cx);
                    }
                });
            }
        }
    }

    fn set_zoom(&mut self, z: f32, window: &mut Window, cx: &mut Context<Self>) {
        let z = (z * 10.).round() / 10.;
        self.config.ui.zoom = z.clamp(qur_config::MIN_ZOOM, qur_config::MAX_ZOOM);
        self.write_setting(false, "ui.zoom", Some(self.config.ui.zoom.into()), cx);
        self.apply_look(window, cx);
        self.broadcast_reload(cx);
    }

    // -- messages ---------------------------------------------------------------

    fn flash(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.message = Some((msg.into(), Instant::now()));
        let bg = cx.background_executor().clone();
        self.message_timer = Some(cx.spawn(async move |this, cx| {
            bg.timer(Duration::from_secs(6)).await;
            this.update(cx, |this, cx| {
                this.message = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    // -- menus ------------------------------------------------------------------

    pub fn menu_state(&self, cx: &App) -> MenuState {
        MenuState {
            sidebar: self.sidebar,
            bottom_panel: self.panel_visible,
            soft_wrap: self.active.as_ref().map(|e| e.read(cx).soft_wrap()).unwrap_or(self.config.editor.soft_wrap),
            recent: qur_config::State::load().recent_projects,
        }
    }

    pub fn sync_menus(&self, cx: &mut Context<Self>) {
        crate::set_menus(self.menu_state(cx), cx);
    }

    // -- focus ------------------------------------------------------------------

    fn focus_editor(&self, window: &mut Window, cx: &mut App) {
        match &self.active {
            Some(e) => {
                let f = e.read(cx).focus.clone();
                window.focus(&f, cx)
            }
            None => window.focus(&self.focus, cx),
        }
    }

    fn focus_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel_visible = true;
        let f = self.panel.read(cx).focus.clone();
        window.focus(&f, cx);
        self.sync_menus(cx);
        cx.notify();
    }

    // -- files ------------------------------------------------------------------

    pub fn open_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.open_path_at(path, None, window, cx)
    }

    fn open_path_at(&mut self, path: &Path, at: Option<Point>, window: &mut Window, cx: &mut Context<Self>) {
        self.record_nav(cx);
        let buf = match self.buffers.find(path, cx) {
            Some(b) => b,
            None => {
                let wrap = self.config.editor.soft_wrap;
                let tab = self.config.editor.tab_width;
                let view = match EditorView::load(path) {
                    Ok(loaded) => cx.new(|cx| EditorView::new(loaded, wrap, tab, cx)),
                    Err(e) => {
                        self.flash(format!("Couldn't open {}: {e}", path.display()), cx);
                        return;
                    }
                };
                let sub = cx.subscribe_in(&view, window, Self::on_editor_event);
                self._subs.push(sub);
                view
            }
        };
        self.buffers.touch(&buf);
        self.active = Some(buf.clone());
        if let Some(p) = at {
            buf.update(cx, |b, cx| b.set_cursor(p, cx));
        }
        self.tree.update(cx, |t, cx| {
            t.current = Some(path.to_path_buf());
            cx.notify();
        });
        self.push_nav(cx);
        self.focus_editor(window, cx);
        self.sync_menus(cx);
        cx.notify();
    }

    fn on_editor_event(&mut self, view: &Entity<EditorView>, ev: &EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            EditorEvent::Changed => {
                // Saving config.toml applies it everywhere.
                let v = view.read(cx);
                if !v.is_dirty() && v.path() == Some(qur_config::config_path().as_path()) {
                    self.reload_settings(window, cx);
                    self.broadcast_reload(cx);
                }
                cx.notify();
            }
            EditorEvent::Error(e) => self.flash(e.clone(), cx),
            EditorEvent::WrapToggled => self.sync_menus(cx),
        }
    }

    fn record_nav(&mut self, cx: &App) {
        if let Some(a) = &self.active {
            let a = a.read(cx);
            if let (Some(p), Some(entry)) = (a.path(), self.nav.get_mut(self.nav_ix))
                && entry.0 == p
            {
                entry.1 = a.cursor();
            }
        }
    }

    fn push_nav(&mut self, cx: &App) {
        let Some(a) = &self.active else { return };
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
        self.nav_ix = ix;
        let saved = std::mem::take(&mut self.nav);
        self.open_path_at(&p, Some(at), window, cx);
        self.nav = saved;
        self.nav_ix = ix;
    }

    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().into_owned()
    }

    // -- actions: app & window ---------------------------------------------------

    fn quit(&mut self, _: &qur::Quit, window: &mut Window, cx: &mut Context<Self>) {
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
        let answer = window.prompt(PromptLevel::Warning, &msg, None, &["Save All", "Don't Save", "Cancel"], cx);
        cx.spawn(async move |_, cx| {
            match answer.await {
                Ok(0) => {
                    cx.update(|cx| {
                        for b in &dirty {
                            b.update(cx, |b, cx| b.save(cx));
                        }
                        cx.quit()
                    });
                }
                Ok(1) => cx.update(|cx| cx.quit()),
                _ => {}
            }
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
        let answer = window.prompt(PromptLevel::Warning, &msg, None, &["Save All", "Don't Save", "Cancel"], cx);
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
                        return; // a save failed; keep the window
                    }
                } else {
                    // Discard: drop the buffers so the next close passes.
                    this.buffers.list.retain(|b| !b.read(cx).is_dirty());
                    this.active = this.buffers.list.first().cloned();
                }
                this.shutdown(cx);
                window.remove_window();
            })
            .ok();
        })
        .detach();
        false
    }

    fn hide(&mut self, _: &qur::Hide, _: &mut Window, cx: &mut Context<Self>) {
        cx.hide()
    }
    fn hide_others(&mut self, _: &qur::HideOthers, _: &mut Window, cx: &mut Context<Self>) {
        cx.hide_other_apps()
    }
    fn show_all(&mut self, _: &qur::ShowAll, _: &mut Window, cx: &mut Context<Self>) {
        cx.unhide_other_apps()
    }
    fn close_window(&mut self, _: &qur::CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_close(window, cx) {
            window.remove_window()
        }
    }
    fn minimize(&mut self, _: &qur::Minimize, window: &mut Window, _: &mut Context<Self>) {
        window.minimize_window()
    }
    fn zoom_window(&mut self, _: &qur::Zoom, window: &mut Window, _: &mut Context<Self>) {
        window.zoom_window()
    }
    fn fullscreen(&mut self, _: &qur::ToggleFullScreen, window: &mut Window, _: &mut Context<Self>) {
        window.toggle_fullscreen()
    }
    fn toggle_sidebar(&mut self, _: &qur::ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar = !self.sidebar;
        self.sync_menus(cx);
        cx.notify();
    }
    fn reveal_files(&mut self, _: &qur::RevealInFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar = true;
        self.files_open = true;
        if let Some(p) = self.active.as_ref().and_then(|a| a.read(cx).path().map(Path::to_path_buf)) {
            self.tree.update(cx, |t, cx| t.reveal(&p, cx));
        }
        self.sync_menus(cx);
        cx.notify();
    }
    fn toggle_panel(&mut self, _: &qur::ToggleBottomPanel, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panel_visible && self.panel.read(cx).sessions.is_empty() {
            return self.new_terminal(&qur::NewTerminal, window, cx);
        }
        self.panel_visible = !self.panel_visible;
        if !self.panel_visible && self.panel.read(cx).focus.contains_focused(window, cx) {
            self.focus_editor(window, cx);
        }
        self.sync_menus(cx);
        cx.notify();
    }
    fn toggle_terminal(&mut self, _: &qur::ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.panel.read(cx).focus.contains_focused(window, cx);
        if self.panel_visible && focused {
            self.panel_visible = false;
            self.focus_editor(window, cx);
        } else {
            if self.panel.read(cx).sessions.is_empty() {
                return self.new_terminal(&qur::NewTerminal, window, cx);
            }
            self.focus_panel(window, cx);
        }
        self.sync_menus(cx);
        cx.notify();
    }
    fn zoom_in(&mut self, _: &qur::ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        let z = self.config.ui.zoom + 0.1;
        self.set_zoom(z, window, cx)
    }
    fn zoom_out(&mut self, _: &qur::ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        let z = self.config.ui.zoom - 0.1;
        self.set_zoom(z, window, cx)
    }
    fn zoom_reset(&mut self, _: &qur::ZoomReset, window: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(1.0, window, cx)
    }

    fn open_config(&mut self, _: &qur::OpenConfig, window: &mut Window, cx: &mut Context<Self>) {
        let path = qur_config::config_path();
        if !path.exists() {
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            let _ = std::fs::write(&path, CONFIG_TEMPLATE);
        }
        self.open_path(&path, window, cx);
    }

    fn new_window(&mut self, _: &qur::NewWindow, _: &mut Window, cx: &mut Context<Self>) {
        crate::prompt_open_folder(cx);
    }
    fn open_folder(&mut self, _: &qur::OpenFolder, _: &mut Window, cx: &mut Context<Self>) {
        crate::prompt_open_folder(cx);
    }
    fn open_project(&mut self, a: &qur::OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        crate::open_project(a.path.clone(), None, cx);
    }

    // -- actions: files -----------------------------------------------------------

    fn save_all(&mut self, _: &qur::SaveAll, _: &mut Window, cx: &mut Context<Self>) {
        for b in self.buffers.dirty(cx) {
            b.update(cx, |b, cx| b.save(cx));
        }
    }
    fn revert(&mut self, _: &qur::RevertFile, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = &self.active {
            a.update(cx, |a, cx| a.reload_from_disk(cx));
        }
    }
    fn recent_file(&mut self, _: &qur::RecentFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.buffers.list.get(1).and_then(|b| b.read(cx).path().map(Path::to_path_buf)) {
            self.open_path(&p, window, cx);
        }
    }
    fn nav_back(&mut self, _: &qur::NavBack, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav_ix > 0 {
            let ix = self.nav_ix - 1;
            self.nav_to(ix, window, cx);
        }
    }
    fn nav_forward(&mut self, _: &qur::NavForward, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav_ix + 1 < self.nav.len() {
            let ix = self.nav_ix + 1;
            self.nav_to(ix, window, cx);
        }
    }

    // -- actions forwarded to the editor from anywhere -------------------------------

    fn with_editor(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut EditorView, &mut Context<EditorView>)) {
        if let Some(a) = self.active.clone() {
            a.update(cx, f);
        }
    }
    fn find(&mut self, _: &ea::Find, window: &mut Window, cx: &mut Context<Self>) {
        self.with_editor(cx, |e, cx| e.open_find(false, window, cx))
    }
    fn replace(&mut self, _: &ea::Replace, window: &mut Window, cx: &mut Context<Self>) {
        self.with_editor(cx, |e, cx| e.open_find(true, window, cx))
    }
    fn find_next(&mut self, _: &ea::FindNext, window: &mut Window, cx: &mut Context<Self>) {
        self.with_editor(cx, |e, cx| e.find_next(window, cx))
    }
    fn find_prev(&mut self, _: &ea::FindPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.with_editor(cx, |e, cx| e.find_prev(window, cx))
    }
    fn use_selection(&mut self, _: &ea::UseSelectionForFind, window: &mut Window, cx: &mut Context<Self>) {
        self.with_editor(cx, |e, cx| e.use_selection_for_find(window, cx))
    }
    fn soft_wrap(&mut self, _: &ea::ToggleSoftWrap, _: &mut Window, cx: &mut Context<Self>) {
        self.with_editor(cx, |e, cx| e.toggle_soft_wrap(cx));
        self.sync_menus(cx);
    }
    // Edit-menu items with nothing focused that handles them: no-ops, so the
    // menu items stay enabled (GPUI greys items without a handler).
    fn noop_undo(&mut self, _: &edit::Undo, _: &mut Window, _: &mut Context<Self>) {}
    fn noop_redo(&mut self, _: &edit::Redo, _: &mut Window, _: &mut Context<Self>) {}

    // -- overlays ----------------------------------------------------------------------

    fn open_picker(&mut self, placeholder: &str, items: Vec<PickItem>, kind: PickKind, paths: bool, window: &mut Window, cx: &mut Context<Self>) -> Entity<Picker> {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let p = cx.new(|cx| Picker::new(placeholder, items, paths, window, cx));
        let sub = cx.subscribe_in(&p, window, Self::on_picker_event);
        self._subs.push(sub);
        let f = p.read(cx).input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::Picker(p.clone(), kind));
        cx.notify();
        p
    }

    fn on_picker_event(&mut self, picker: &Entity<Picker>, ev: &PickerEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Picker(p, kind)) = &mut self.overlay else { return };
        if p != picker {
            return;
        }
        match (ev, kind) {
            (PickerEvent::Confirm(i), PickKind::Commands(cmds)) => {
                let action = (cmds[*i].action)();
                self.close_overlays(window, cx);
                // After focus is restored, so contextual commands reach the editor/terminal.
                window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
            }
            (PickerEvent::Confirm(i), PickKind::Files(files)) => {
                let path = self.root.join(&files[*i]);
                self.close_overlays(window, cx);
                self.open_path(&path, window, cx);
            }
            (PickerEvent::Confirm(i), PickKind::Projects(list)) => {
                let target = list.get(*i).cloned();
                self.close_overlays(window, cx);
                match target {
                    Some(p) => crate::open_project(p, None, cx),
                    None => crate::prompt_open_folder(cx),
                }
            }
            (PickerEvent::Highlight(i), PickKind::Themes { names, .. }) => {
                self.preview_theme = Some(names[*i].clone());
                self.apply_look(window, cx);
            }
            (PickerEvent::Confirm(i), PickKind::Themes { names, project_scope, .. }) => {
                let name = names[*i].clone();
                let project_scope = *project_scope;
                self.preview_theme = None;
                if project_scope {
                    self.write_setting(true, "theme", Some(name.as_str().into()), cx);
                } else {
                    self.write_setting(false, "theme", Some(name.as_str().into()), cx);
                    // "All projects" also drops this project's override so the change shows.
                    self.write_setting(true, "theme", None, cx);
                }
                self.overlay = None;
                self.reload_settings(window, cx);
                if !project_scope {
                    self.broadcast_reload(cx);
                }
                self.restore_focus(window, cx);
            }
            (PickerEvent::Tab, PickKind::Themes { project_scope, .. }) => {
                *project_scope = !*project_scope;
                let header = theme_scope_header(*project_scope, &self.project_name());
                picker.update(cx, |p, cx| {
                    p.header = Some(header);
                    cx.notify();
                });
            }
            (PickerEvent::Dismiss, PickKind::Themes { .. }) => {
                self.preview_theme = None;
                self.apply_look(window, cx);
                self.close_overlays(window, cx);
            }
            (PickerEvent::Dismiss, _) => self.close_overlays(window, cx),
            _ => {}
        }
    }

    fn palette(&mut self, _: &qur::CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(&self.overlay, Some(Overlay::Picker(_, PickKind::Commands(_)))) {
            return self.close_overlays(window, cx);
        }
        let cmds: Vec<_> = commands::palette_commands().collect();
        let items = cmds
            .iter()
            .map(|c| {
                let right = match c.tier {
                    commands::Tier::Ready => commands::display_keys(c.first_keys()),
                    commands::Tier::Later(m) => format!("coming in {m}"),
                };
                PickItem::new(format!("{}: {}", c.category, c.title)).right(right).enabled(c.ready())
            })
            .collect();
        self.open_picker("Type a command…", items, PickKind::Commands(cmds), false, window, cx);
    }

    fn quick_open(&mut self, _: &qur::QuickOpen, window: &mut Window, cx: &mut Context<Self>) {
        let files = match &self.file_list {
            Some(f) => f.clone(),
            None => {
                let f = qur_tree::list_files(&self.root, 200_000);
                self.file_list = Some(f.clone());
                f
            }
        };
        // Open files first (most recent), then everything else.
        let recent: Vec<PathBuf> = self
            .buffers
            .paths(cx)
            .into_iter()
            .filter_map(|p| p.strip_prefix(&self.root).ok().map(Path::to_path_buf))
            .collect();
        let mut ordered = recent.clone();
        ordered.extend(files.into_iter().filter(|f| !recent.contains(f)));
        let items = ordered
            .iter()
            .map(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                PickItem::new(name).detail(dir)
            })
            .collect();
        self.open_picker("Go to file…", items, PickKind::Files(ordered), true, window, cx);
    }

    fn theme_picker(&mut self, _: &qur::ThemePicker, window: &mut Window, cx: &mut Context<Self>) {
        let eff = self.effective();
        let mut names = vec![qur_config::THEME_AUTO.to_string()];
        let mut items = vec![PickItem::new("Auto").detail("follows macOS: Qur Light / Qur Dark")];
        for t in Registry::global().all() {
            names.push(t.name.clone());
            items.push(PickItem::new(t.name.clone()).detail(t.group));
        }
        let current = names.iter().position(|n| *n == eff.theme).unwrap_or(0);
        // Default scope: this project once one is open (decision 13).
        let project_scope = true;
        let header = theme_scope_header(project_scope, &self.project_name());
        let p = self.open_picker("Select a theme…", items, PickKind::Themes { names, project_scope }, false, window, cx);
        p.update(cx, |p, cx| {
            p.header = Some(header);
            p.select_item(current, cx);
        });
    }

    fn use_default_theme(&mut self, _: &qur::UseDefaultTheme, window: &mut Window, cx: &mut Context<Self>) {
        self.write_setting(true, "theme", None, cx);
        self.reload_settings(window, cx);
        self.flash(format!("{} now uses the default theme", self.project_name()), cx);
    }

    fn project_switcher(&mut self, _: &qur::ProjectSwitcher, window: &mut Window, cx: &mut Context<Self>) {
        let home = std::env::var("HOME").unwrap_or_default();
        let mut list = qur_config::State::load().recent_projects;
        list.retain(|p| p != &self.root);
        let mut items: Vec<PickItem> = list
            .iter()
            .map(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                PickItem::new(name).detail(p.to_string_lossy().replacen(&home, "~", 1))
            })
            .collect();
        items.push(PickItem::new("Open Folder…").right(commands::display_keys("cmd-o")));
        self.open_picker("Switch project…", items, PickKind::Projects(list), false, window, cx);
    }

    fn goto_line(&mut self, _: &qur::GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.active.clone() else { return };
        self.close_overlays(window, cx);
        self.prev_focus = Some(active.read(cx).focus.clone());
        let n = active.read(cx).model.line_count();
        let input = cx.new(|cx| TextInput::new(format!("Go to line (1–{n})"), cx));
        let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Confirm { .. } => {
                let t = input.read(cx).text.trim().to_string();
                let mut it = t.split(':');
                let line = it.next().and_then(|l| l.parse::<usize>().ok());
                let col = it.next().and_then(|c| c.parse::<usize>().ok());
                this.close_overlays(window, cx);
                if let (Some(l), Some(a)) = (line, this.active.clone()) {
                    a.update(cx, |a, cx| {
                        a.goto_line(l, cx);
                        if let Some(c) = col {
                            a.set_cursor(Point { line: l.saturating_sub(1), col: c.saturating_sub(1) }, cx);
                        }
                    });
                }
            }
            InputEvent::Cancel => this.close_overlays(window, cx),
            _ => {}
        });
        self._subs.push(sub);
        let f = input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::GoTo(input));
        cx.notify();
    }

    fn transparency(&mut self, _: &qur::Transparency, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let project_scope = self.effective().window_is_project;
        self.overlay = Some(Overlay::Transparency { project_scope });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.prev_focus.take() {
            Some(f) => window.focus(&f, cx),
            None => self.focus_editor(window, cx),
        }
        cx.notify();
    }

    fn close_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Picker(_, PickKind::Themes { .. })) = &self.overlay
            && self.preview_theme.take().is_some()
        {
            self.apply_look(window, cx);
        }
        if self.overlay.take().is_some() {
            self.restore_focus(window, cx);
        }
        cx.notify();
    }

    fn set_opacity(&mut self, opacity: f32, blur: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Transparency { project_scope }) = self.overlay else { return };
        let min = Look::of(window, cx).theme.min_opacity().max(qur_theme::MIN_OPACITY);
        let opacity = ((opacity.clamp(min, 1.0)) * 100.).round() / 100.;
        if project_scope {
            self.project.window.opacity = Some(opacity);
            self.project.window.blur = Some(blur);
        } else {
            self.config.window.opacity = opacity;
            self.config.window.blur = blur;
        }
        self.apply_look(window, cx);
    }

    /// Persist the transparency popover's values when it closes.
    fn save_transparency(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Transparency { project_scope }) = self.overlay else { return };
        let eff = self.effective();
        if project_scope {
            self.write_setting(true, "window.opacity", Some(eff.opacity.into()), cx);
            self.write_setting(true, "window.blur", Some(eff.blur.into()), cx);
        } else {
            self.write_setting(false, "window.opacity", Some(eff.opacity.into()), cx);
            self.write_setting(false, "window.blur", Some(eff.blur.into()), cx);
            self.write_setting(true, "window.opacity", None, cx);
            self.write_setting(true, "window.blur", None, cx);
            self.broadcast_reload(cx);
        }
    }

    fn slide_to(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let b = self.slider.get();
        if b.size.width <= px(0.) {
            return;
        }
        let min = Look::of(window, cx).theme.min_opacity().max(qur_theme::MIN_OPACITY);
        let t = ((x - b.left()) / b.size.width).clamp(0., 1.);
        let blur = self.effective().blur;
        self.set_opacity(min + (1. - min) * t, blur, window, cx);
    }

    // -- terminals & Claude tasks -----------------------------------------------------

    fn new_terminal(&mut self, _: &qur::NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.root.clone();
        self.panel.update(cx, |p, cx| {
            let label = p.label_for(Kind::Shell);
            p.open(&dir, Kind::Shell, label, cx);
        });
        self.focus_panel(window, cx);
    }

    fn new_agent_terminal(&mut self, _: &qur::NewAgentTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let repo_id = self.project_id.clone();
        self.panel_visible = true;
        self.sync_menus(cx);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = bg.spawn(async move { create_task_worktree(&root, &repo_id) }).await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((n, worktree)) => {
                    let term_id = this.panel.update(cx, |p, cx| p.open(&worktree, Kind::Claude(n), format!("claude {n}"), cx));
                    this.tasks.push(AgentTask { n, title: format!("claude terminal {n}"), worktree, term_id });
                    this.focus_panel(window, cx);
                }
                Err(msg) => this.flash(msg, cx),
            })
            .ok();
        })
        .detach();
    }

    fn next_terminal(&mut self, _: &qur::NextTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.panel.update(cx, |p, cx| p.cycle(1, cx));
        self.focus_panel(window, cx);
    }
    fn prev_terminal(&mut self, _: &qur::PrevTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.panel.update(cx, |p, cx| p.cycle(-1, cx));
        self.focus_panel(window, cx);
    }

    fn open_task(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.tasks[ix].term_id;
        let alive = id.is_some_and(|id| self.panel.update(cx, |p, cx| p.activate_id(id, cx)));
        if !alive {
            // The tab was closed; the task persists, so resume Claude in its worktree.
            let (wt, n) = (self.tasks[ix].worktree.clone(), self.tasks[ix].n);
            let new_id = self.panel.update(cx, |p, cx| p.open(&wt, Kind::Claude(n), format!("claude {n}"), cx));
            self.tasks[ix].term_id = new_id;
        }
        self.focus_panel(window, cx);
    }

    /// Window closing or app quitting: stop every PTY session (no orphans).
    pub fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.panel.update(cx, |p, _| p.close_all_sync());
    }

    /// QUR_BENCH=theme: switch through every theme, one per frame, and time
    /// each switch from apply to the end of the next frame.
    fn bench_theme_step(&mut self, i: usize, mut samples: Vec<f64>, window: &mut Window, cx: &mut Context<Self>) {
        let themes = Registry::global().all();
        if i >= themes.len() * 2 {
            samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let pct = |p: f64| samples[((samples.len() - 1) as f64 * p).round() as usize];
            println!("METRIC theme_switch_n={}", samples.len());
            println!("METRIC theme_switch_p50_ms={:.2}", pct(0.5));
            println!("METRIC theme_switch_max_ms={:.2}", samples[samples.len() - 1]);
            return cx.quit();
        }
        let t0 = Instant::now();
        self.preview_theme = Some(themes[i % themes.len()].name.clone());
        self.apply_look(window, cx);
        let me = cx.entity();
        window.on_next_frame(move |window, cx| {
            samples.push(t0.elapsed().as_secs_f64() * 1000.0);
            me.update(cx, |ws, cx| ws.bench_theme_step(i + 1, samples, window, cx));
        });
    }

    // -- render pieces -------------------------------------------------------------------

    fn title_bar(&self, look: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = &look.theme.ui;
        let file = self.active.as_ref().map(|a| {
            let a = a.read(cx);
            let name = a.path().map(|p| self.rel(p)).unwrap_or_else(|| a.title());
            (name, a.is_dirty())
        });
        let message = self.message.as_ref().map(|(m, _)| m.clone());
        div()
            .id("titlebar")
            .window_control_area(gpui::WindowControlArea::Drag)
            .h(look.z(TITLEBAR_H))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(look.z(8.))
            .pl(px(TRAFFIC_LIGHTS_W))
            .pr(look.z(12.))
            .bg(look.bg(ui.top_bar_bg))
            .border_b_1()
            .border_color(look.c(ui.border))
            .text_size(look.text(0.))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, _| this.title_drag = true))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.title_drag = false))
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.title_drag = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.title_drag {
                    this.title_drag = false;
                    window.start_window_move();
                }
            }))
            .on_click(|ev, window, _| {
                if ev.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(
                svg()
                    .path(assets::symbol(look.logo_white(), 16. * look.zoom))
                    .size(look.z(16.))
                    .flex_none()
                    // Pure black or white only: the logo is never tinted.
                    .text_color(if look.logo_white() { gpui::white() } else { gpui::black() }),
            )
            .child(
                div()
                    .id("project")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(look.z(2.))
                    .px(look.z(4.))
                    .rounded(look.z(4.))
                    .cursor_pointer()
                    .hover(|d| d.bg(look.c(ui.hover_bg)))
                    .text_color(look.c(ui.text))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(self.project_name())
                    .child(svg().path(icon::CHEVRON_DOWN).size(look.z(12.)).text_color(look.c(ui.text_muted)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| this.project_switcher(&qur::ProjectSwitcher, window, cx))),
            )
            .children(file.map(|(name, dirty)| {
                div()
                    .flex()
                    .min_w(px(0.))
                    .items_center()
                    .gap(look.z(6.))
                    .text_color(look.c(ui.text_secondary))
                    .child(div().truncate().child(name))
                    .when(dirty, |d| d.child(div().size(look.z(6.)).flex_none().rounded(look.z(1.5)).bg(look.c(ui.text_secondary))))
            }))
            .child(div().flex_1())
            .children(message.map(|m| {
                div().flex_none().max_w(look.z(420.)).truncate().text_size(look.text(-1.)).text_color(look.c(ui.text_secondary)).child(m)
            }))
    }

    fn sidebar(&self, look: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = &look.theme.ui;
        let header = |t: &'static str| {
            div()
                .px(look.z(8.))
                .pt(look.z(10.))
                .pb(look.z(4.))
                .text_size(look.text(-2.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(look.c(ui.text_muted))
                .child(t)
        };
        let panel = self.panel.read(cx);
        let rows: Vec<_> = self
            .tasks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let s = t.term_id.and_then(|id| panel.session_by_id(id));
                let busy = s.is_some_and(|s| s.is_busy());
                let title = s.and_then(|s| s.first_line.clone()).unwrap_or_else(|| t.title.clone());
                let accent = look.theme.accents[i % look.theme.accents.len()].base;
                div()
                    .id(("task", i))
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .px(look.z(8.))
                    .py(look.z(5.))
                    .cursor_pointer()
                    .hover(|d| d.bg(look.c(ui.hover_bg)))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_task(i, window, cx)))
                    .child(div().flex_none().size(look.z(9.)).rounded(look.z(2.5)).bg(look.c(accent)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .child(div().truncate().text_size(look.text(0.)).text_color(look.c(ui.text)).child(format!("claude {}", t.n)))
                            .child(div().truncate().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(title)),
                    )
                    .child(div().flex_none().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(if busy { "~" } else { "·" }))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .w(look.z(SIDEBAR_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(look.bg(ui.sidebar_bg))
            .border_r_1()
            .border_color(look.c(ui.border))
            .child(header("AGENTS"))
            .children(rows)
            .when(empty, |d| {
                d.child(div().px(look.z(8.)).py(look.z(4.)).text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child("No agents yet"))
            })
            .child(
                div()
                    .id("files-header")
                    .flex()
                    .items_center()
                    .gap(look.z(2.))
                    .px(look.z(8.))
                    .pt(look.z(12.))
                    .pb(look.z(4.))
                    .cursor_pointer()
                    .text_size(look.text(-2.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(look.c(ui.text_muted))
                    .child(svg().path(if self.files_open { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT }).size(look.z(12.)).text_color(look.c(ui.text_muted)))
                    .child("FILES")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.files_open = !this.files_open;
                        cx.notify();
                    })),
            )
            .when(self.files_open, |d| d.child(self.tree.clone()))
    }

    fn empty_editor(&self, look: &Look) -> impl IntoElement {
        let ui = &look.theme.ui;
        let hint = |label: &'static str, keys: &str| {
            div()
                .flex()
                .justify_between()
                .gap(look.z(24.))
                .child(div().text_color(look.c(ui.text_secondary)).child(label))
                .child(div().text_color(look.c(ui.text_muted)).child(commands::display_keys(keys)))
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(look.bg(ui.editor_bg))
            .text_size(look.text(0.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.z(8.))
                    .child(hint("Go to File", "cmd-p"))
                    .child(hint("Command Palette", "cmd-shift-p"))
                    .child(hint("New Terminal", "cmd-t"))
                    .child(hint("New Agent Terminal", "cmd-shift-t")),
            )
    }

    fn overlay_el(&self, look: &Look, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
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
        match self.overlay.as_ref()? {
            Overlay::Picker(p, _) => Some(centered(p.clone().into_any_element())),
            Overlay::GoTo(g) => Some(centered(
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
        }
    }

    fn transparency_popover(&self, project_scope: bool, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let min = look.theme.min_opacity().max(qur_theme::MIN_OPACITY);
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
                    this.set_opacity(eff.opacity, eff.blur, window, cx);
                }))
        };
        div()
            .absolute()
            .top(look.z(TITLEBAR_H + 4.))
            .left(px(TRAFFIC_LIGHTS_W))
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
            .child(
                div()
                    .flex()
                    .gap(look.z(4.))
                    .child(scope("This project", project_scope, "scope-project", true))
                    .child(scope("All projects", !project_scope, "scope-all", false)),
            )
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
                    .on_click(cx.listener(|this, _, window, cx| {
                        let eff = this.effective();
                        this.set_opacity(eff.opacity, !eff.blur, window, cx);
                    })),
            )
            .into_any_element()
    }
}

fn theme_scope_header(project_scope: bool, project: &str) -> SharedString {
    if project_scope {
        format!("Theme for {project} only  ·  ⇥ for all projects").into()
    } else {
        "Theme for all projects  ·  ⇥ for this project only".into()
    }
}

fn is_dark(a: WindowAppearance) -> bool {
    matches!(a, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

/// SF Mono when installed (Xcode, Terminal), else Menlo; nothing extra ships.
/// A direct lookup (listing every font costs ~40 ms at startup).
fn default_code_font(cx: &App) -> String {
    static FOUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let sf = *FOUND.get_or_init(|| {
        let ts = cx.text_system();
        let id = ts.resolve_font(&gpui::font("SF Mono"));
        ts.get_font_for_id(id).is_some_and(|f| f.family.as_ref() == "SF Mono")
    });
    if sf { "SF Mono".into() } else { "Menlo".into() }
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Creates branch `qur/claude/term-<n>` in a worktree outside the repo
/// (decision 12). Blocking; run off the UI thread.
fn create_task_worktree(root: &Path, repo_id: &str) -> Result<(usize, PathBuf), String> {
    if git(root, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Ok("true") {
        return Err("Claude tasks need a git repo".into());
    }
    let used: Vec<usize> = git(root, &["branch", "--list", "qur/claude/term-*", "--format=%(refname:short)"])?
        .lines()
        .filter_map(|b| b.rsplit('-').next()?.parse().ok())
        .collect();
    let dir = qur_config::qur_home().join("worktrees").join(repo_id);
    let mut n = used.iter().max().copied().unwrap_or(0) + 1;
    while dir.join(format!("term-{n}")).exists() {
        n += 1;
    }
    let path = dir.join(format!("term-{n}"));
    let branch = format!("qur/claude/term-{n}");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    git(root, &["worktree", "add", "-q", "-b", &branch, path.to_str().unwrap(), "HEAD"])?;
    Ok((n, path))
}

const CONFIG_TEMPLATE: &str = r#"# Qur settings. Saved changes apply right away.
# Per-project overrides live in projects/<id>.toml (set them from Qur > Theme… / Transparency…).

# theme = "Auto"            # Auto follows macOS: Qur Light / Qur Dark

[window]
# opacity = 1.0             # 0.5 to 1.0
# blur = true

[editor]
# font_family = ""          # empty = SF Mono if installed, else Menlo
# font_size = 13
# line_height = 1.5
# soft_wrap = true
# tab_width = 4

[ui]
# font_size = 13
# zoom = 1.0

[panels]
# terminal = true
# git = true
# search = true

[keys]
# vim = false
"#;

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        if !self.first_frame {
            self.first_frame = true;
            let handle: AnyWindowHandle = window.window_handle();
            let me = cx.entity();
            window.on_next_frame(move |_, cx| {
                crate::first_frame(handle, cx);
                // The menu bar is built after the first frame (~20 ms off startup).
                me.update(cx, |ws, cx| ws.sync_menus(cx));
            });
            if std::env::var("QUR_BENCH").as_deref() == Ok("theme") {
                let me = cx.entity();
                window.on_next_frame(move |window, cx| me.update(cx, |ws, cx| ws.bench_theme_step(0, Vec::new(), window, cx)));
            }
        }
        let editor_area = match &self.active {
            Some(a) => div().flex_1().min_w(px(0.)).h_full().child(a.clone()).into_any_element(),
            None => div().flex_1().min_w(px(0.)).h_full().child(self.empty_editor(&look)).into_any_element(),
        };
        let body = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .when(self.sidebar, |d| d.child(self.sidebar(&look, cx)))
            .child(editor_area);
        let panel = self.panel_visible.then(|| {
            div()
                .h(look.z(PANEL_H))
                .flex_none()
                .w_full()
                .bg(look.bg(ui.panel_bg))
                .border_t_1()
                .border_color(look.c(ui.border))
                .child(self.panel.clone())
        });
        let any_overlay = self.overlay.is_some();
        div()
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .font_family(look.ui_font.clone())
            .text_color(look.c(ui.text))
            .bg(look.bg(ui.window_bg))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::hide))
            .on_action(cx.listener(Self::hide_others))
            .on_action(cx.listener(Self::show_all))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::minimize))
            .on_action(cx.listener(Self::zoom_window))
            .on_action(cx.listener(Self::fullscreen))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::reveal_files))
            .on_action(cx.listener(Self::toggle_panel))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::open_config))
            .on_action(cx.listener(Self::new_window))
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::save_all))
            .on_action(cx.listener(Self::revert))
            .on_action(cx.listener(Self::recent_file))
            .on_action(cx.listener(Self::nav_back))
            .on_action(cx.listener(Self::nav_forward))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::replace))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::use_selection))
            .on_action(cx.listener(Self::soft_wrap))
            .on_action(cx.listener(Self::noop_undo))
            .on_action(cx.listener(Self::noop_redo))
            .on_action(cx.listener(Self::palette))
            .on_action(cx.listener(Self::quick_open))
            .on_action(cx.listener(Self::theme_picker))
            .on_action(cx.listener(Self::use_default_theme))
            .on_action(cx.listener(Self::project_switcher))
            .on_action(cx.listener(Self::goto_line))
            .on_action(cx.listener(Self::transparency))
            .on_action(cx.listener(Self::new_terminal))
            .on_action(cx.listener(Self::new_agent_terminal))
            .on_action(cx.listener(Self::next_terminal))
            .on_action(cx.listener(Self::prev_terminal))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                // Esc closes the transparency popover (it has no text field).
                if ev.keystroke.key == "escape" && matches!(this.overlay, Some(Overlay::Transparency { .. })) {
                    this.save_transparency(cx);
                    this.close_overlays(window, cx);
                }
            }))
            .child(self.title_bar(&look, cx))
            .child(body)
            .children(panel)
            .when(any_overlay, |d| {
                d.child(div().absolute().size_full().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.save_transparency(cx);
                        this.close_overlays(window, cx);
                    }),
                ))
            })
            .children(self.overlay_el(&look, cx))
    }
}
