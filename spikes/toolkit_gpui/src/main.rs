//! M0 toolkit spike: GPUI candidate.
//!
//! Window: Zed-style unified top bar, sidebar (agents + virtualized file tree),
//! soft-wrapping editor, bottom terminal panel with tabs, command palette,
//! native menu bar, transparency popover. Shared work (highlighting, tree
//! model, metrics) comes from `spike_common`.

mod commands;
mod editor;
mod palette;
#[cfg(feature = "terminal")]
mod terminal;
#[cfg(not(feature = "terminal"))]
#[path = "terminal_stub.rs"]
mod terminal;
mod ui;

use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use gpui::{
    App, Bounds, Context, Entity, FocusHandle, MouseButton, Pixels, SharedString, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowOptions, canvas, div, point, prelude::*, px,
    size, uniform_list,
};
use spike_common::{self as sc, Config, FileTree, qur_dark};

use commands::{MenuState, editor as ea, qur};
use editor::{Editor, EditorEvent};
use palette::{Palette, PaletteEvent};
use terminal::{Kind, PanelEvent, TerminalPanel};
use ui::{InputEvent, TextInput, Ui, hsla};

/// Height of our own top bar (the system titlebar is transparent underneath).
const TITLEBAR_H: f32 = 32.0;
/// Left inset that clears the traffic lights placed at (9, 9).
const TRAFFIC_LIGHTS_W: f32 = 78.0;
const SIDEBAR_W: f32 = 220.0;
const PANEL_H: f32 = 200.0;

// ---------------------------------------------------------------------------
// File tree
// ---------------------------------------------------------------------------

struct TreeView {
    tree: FileTree,
    visible: Vec<usize>,
}

impl TreeView {
    fn new(cfg: &Config) -> Self {
        let mut tree = FileTree::load(&cfg.tree_root);
        if cfg.expand_all {
            tree.set_all_expanded(true);
        }
        let visible = tree.visible();
        sc::metric("tree_files", tree.file_count());
        sc::metric("tree_visible_rows", visible.len());
        TreeView { tree, visible }
    }
}

impl Render for TreeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        uniform_list(
            "files",
            self.visible.len(),
            cx.processor(|this, range: Range<usize>, _, cx| {
                let ui = Ui::get(cx);
                range
                    .map(|row| {
                        let id = this.visible[row];
                        let n = &this.tree.nodes[id];
                        let glyph = match (n.is_dir, n.expanded) {
                            (true, true) => "▾ ",
                            (true, false) => "▸ ",
                            _ => "  ",
                        };
                        div()
                            .id(row)
                            .h(ui.z(22.))
                            .pl(ui.z(8. + 12. * n.depth as f32))
                            .pr(ui.z(8.))
                            .flex()
                            .items_center()
                            .text_size(ui.z(12.))
                            .text_color(hsla(if n.is_dir { qur_dark::TEXT } else { qur_dark::TEXT_MUTED }))
                            .hover(|s| s.bg(hsla(qur_dark::CURRENT_LINE)))
                            .child(div().min_w(px(0.)).truncate().child(format!("{glyph}{}", n.name)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.tree.toggle(id);
                                this.visible = this.tree.visible();
                                cx.notify();
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
    }
}

// ---------------------------------------------------------------------------
// Agent tasks (Cmd+Shift+T)
// ---------------------------------------------------------------------------

struct AgentTask {
    n: usize,
    title: String,
    accent: u32,
    worktree: PathBuf,
    branch: String,
    term_id: Option<usize>,
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Creates branch `qur/claude/term-<n>` in a worktree outside the repo.
/// Blocking; run off the UI thread.
fn create_task_worktree(root: &Path) -> Result<(usize, PathBuf, String), String> {
    if git(root, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Ok("true") {
        return Err("Claude tasks need a git repo".into());
    }
    let first = git(root, &["rev-list", "--max-parents=0", "HEAD"])?;
    let repo_id: String = first.lines().next().unwrap_or("").chars().take(12).collect();
    let used: Vec<usize> = git(root, &["branch", "--list", "qur/claude/term-*", "--format=%(refname:short)"])?
        .lines()
        .filter_map(|b| b.rsplit('-').next()?.parse().ok())
        .collect();
    let dir = terminal::qur_home().join("worktrees").join(&repo_id);
    let mut n = used.iter().max().copied().unwrap_or(0) + 1;
    while dir.join(format!("term-{n}")).exists() {
        n += 1;
    }
    let path = dir.join(format!("term-{n}"));
    let branch = format!("qur/claude/term-{n}");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    git(root, &["worktree", "add", "-q", "-b", &branch, path.to_str().unwrap(), "HEAD"])?;
    Ok((n, path, branch))
}

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

struct Root {
    focus: FocusHandle,
    root_dir: PathBuf,
    editor: Entity<Editor>,
    tree: Entity<TreeView>,
    panel: Entity<TerminalPanel>,
    sidebar: bool,
    files_open: bool,
    panel_visible: bool,
    palette: Option<Entity<Palette>>,
    goto: Option<Entity<TextInput>>,
    /// Focus to restore when an overlay closes.
    prev_focus: Option<FocusHandle>,
    transparency: bool,
    popover_focus: FocusHandle,
    slider: Rc<Cell<Bounds<Pixels>>>,
    sliding: bool,
    tasks: Vec<AgentTask>,
    menu_state: Option<MenuState>,
    title_drag: bool,
    _subs: Vec<Subscription>,
}

impl Root {
    fn new(cfg: Config, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|_| TreeView::new(&cfg));
        let editor = cx.new(|cx| Editor::new(&cfg, cx));
        let panel = cx.new(TerminalPanel::new);
        let subs = vec![
            cx.subscribe(&editor, |this, _, ev: &EditorEvent, cx| match ev {
                EditorEvent::DirtyChanged | EditorEvent::WrapChanged => {
                    this.sync_menus(cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&panel, window, |this, _, ev: &PanelEvent, window, cx| {
                if let PanelEvent::Closed = ev
                    && this.panel.read(cx).sessions.is_empty()
                {
                    this.focus_editor(window, cx);
                }
                cx.notify();
            }),
        ];
        let open_terminal = std::env::var("QUR_SPIKE_NO_TERMINAL").as_deref() != Ok("1");
        let root_dir = cfg.tree_root.clone();
        if open_terminal {
            panel.update(cx, |p, cx| {
                let label = p.label_for(Kind::Shell);
                p.open(&root_dir, Kind::Shell, label, cx);
            });
        }
        Root {
            focus: cx.focus_handle(),
            root_dir,
            editor,
            tree,
            panel,
            sidebar: true,
            files_open: true,
            panel_visible: open_terminal,
            palette: None,
            goto: None,
            prev_focus: None,
            transparency: false,
            popover_focus: cx.focus_handle(),
            slider: Rc::new(Cell::new(Bounds::default())),
            sliding: false,
            tasks: Vec::new(),
            menu_state: None,
            title_drag: false,
            _subs: subs,
        }
    }

    fn sync_menus(&mut self, cx: &mut Context<Self>) {
        let s = MenuState {
            sidebar: self.sidebar,
            bottom_panel: self.panel_visible,
            soft_wrap: self.editor.read(cx).wrap,
        };
        if self.menu_state != Some(s) {
            cx.set_menus(commands::menus(s));
            self.menu_state = Some(s);
        }
    }

    fn focus_editor(&self, window: &mut Window, cx: &mut App) {
        let f = self.editor.read(cx).focus.clone();
        window.focus(&f, cx);
    }

    fn focus_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel_visible = true;
        let f = self.panel.read(cx).focus.clone();
        window.focus(&f, cx);
        self.sync_menus(cx);
        cx.notify();
    }

    // -- app & window ------------------------------------------------------

    fn quit(&mut self, _: &qur::Quit, _: &mut Window, cx: &mut Context<Self>) {
        cx.quit()
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
    fn close_window(&mut self, _: &qur::CloseWindow, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window()
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
        self.sync_menus(cx);
        cx.notify();
    }
    fn toggle_panel(&mut self, _: &qur::ToggleBottomPanel, window: &mut Window, cx: &mut Context<Self>) {
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
                self.new_terminal(&qur::NewTerminal, window, cx);
            }
            self.focus_panel(window, cx);
        }
        self.sync_menus(cx);
        cx.notify();
    }
    fn set_zoom(&mut self, z: f32, window: &mut Window, cx: &mut Context<Self>) {
        cx.update_global::<Ui, _>(|ui, _| ui.zoom = z.clamp(0.6, 2.0));
        window.refresh();
    }
    fn zoom_in(&mut self, _: &qur::ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        let z = Ui::get(cx).zoom + 0.1;
        self.set_zoom(z, window, cx)
    }
    fn zoom_out(&mut self, _: &qur::ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        let z = Ui::get(cx).zoom - 0.1;
        self.set_zoom(z, window, cx)
    }
    fn zoom_reset(&mut self, _: &qur::ZoomReset, window: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(1.0, window, cx)
    }

    // -- editor commands that work from anywhere ----------------------------

    fn find(&mut self, _: &ea::Find, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.open_find(false, window, cx))
    }
    fn replace(&mut self, _: &ea::Replace, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.open_find(true, window, cx))
    }
    fn find_next(&mut self, _: &ea::FindNext, _: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.find_next(cx))
    }
    fn find_prev(&mut self, _: &ea::FindPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.find_prev(cx))
    }
    fn use_selection(&mut self, _: &ea::UseSelectionForFind, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.use_selection_for_find(window, cx))
    }
    fn soft_wrap(&mut self, _: &ea::ToggleSoftWrap, _: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.toggle_soft_wrap(cx))
    }

    // -- overlays ----------------------------------------------------------

    fn open_palette(&mut self, _: &qur::CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            return self.close_overlays(window, cx);
        }
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let p = cx.new(|cx| Palette::new(window, cx));
        let sub = cx.subscribe_in(&p, window, |this, _, ev: &PaletteEvent, window, cx| {
            let run = match ev {
                PaletteEvent::Run(c) => Some((c.action)()),
                PaletteEvent::Dismiss => None,
            };
            this.close_overlays(window, cx);
            if let Some(action) = run {
                // After focus is restored, so contextual commands reach the editor/terminal.
                window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
            }
        });
        self._subs.push(sub);
        let f = p.read(cx).input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.palette = Some(p);
        cx.notify();
    }

    fn open_goto(&mut self, _: &qur::GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = Some(self.editor.read(cx).focus.clone());
        let n = self.editor.read(cx).total_lines();
        let input = cx.new(|cx| TextInput::new(&format!("Go to line (1–{n})"), cx));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Confirm { .. } => {
                let t = input.read(cx).text.trim().to_string();
                if let Ok(l) = t.split(':').next().unwrap_or("").parse::<usize>() {
                    this.editor.update(cx, |e, cx| e.goto_line(l, cx));
                }
                this.close_overlays(window, cx);
            }
            InputEvent::Cancel => this.close_overlays(window, cx),
            _ => {}
        });
        self._subs.push(sub);
        let f = input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.goto = Some(input);
        cx.notify();
    }

    fn open_transparency(&mut self, _: &qur::Transparency, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        self.transparency = true;
        window.focus(&self.popover_focus, cx);
        cx.notify();
    }

    fn close_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had = self.palette.take().is_some() | self.goto.take().is_some() | std::mem::take(&mut self.transparency);
        if had {
            match self.prev_focus.take() {
                Some(f) => window.focus(&f, cx),
                None => self.focus_editor(window, cx),
            }
        }
        cx.notify();
    }

    fn set_opacity(&mut self, opacity: f32, blur: bool, window: &mut Window, cx: &mut Context<Self>) {
        cx.update_global::<Ui, _>(|ui, _| {
            ui.opacity = opacity.clamp(0.5, 1.0);
            ui.blur = blur;
        });
        window.set_background_appearance(Ui::get(cx).background_appearance());
        window.refresh();
    }

    fn slide_to(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let b = self.slider.get();
        if b.size.width <= px(0.) {
            return;
        }
        let t = ((x - b.left()) / b.size.width).clamp(0., 1.);
        let v = ((0.5 + 0.5 * t) * 100.).round() / 100.;
        let blur = Ui::get(cx).blur;
        self.set_opacity(v, blur, window, cx);
    }

    // -- terminals & tasks -------------------------------------------------

    fn new_terminal(&mut self, _: &qur::NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.root_dir.clone();
        self.panel.update(cx, |p, cx| {
            let label = p.label_for(Kind::Shell);
            p.open(&dir, Kind::Shell, label, cx);
        });
        self.focus_panel(window, cx);
    }

    fn new_claude_task(&mut self, _: &qur::NewClaudeTask, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root_dir.clone();
        self.panel_visible = true;
        self.sync_menus(cx);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = bg.spawn(async move { create_task_worktree(&root) }).await;
            this.update_in(cx, |this, window, cx| match result {
                Ok((n, worktree, branch)) => {
                    let accent = qur_dark::ACCENTS[(3 + this.tasks.len()) % qur_dark::ACCENTS.len()];
                    let term_id = this.panel.update(cx, |p, cx| p.open(&worktree, Kind::Claude(n), format!("claude {n}"), cx));
                    this.tasks.push(AgentTask { n, title: format!("claude terminal {n}"), accent, worktree, branch, term_id });
                    this.focus_panel(window, cx);
                }
                Err(msg) => {
                    this.panel.update(cx, |p, cx| {
                        p.message = Some(msg);
                        cx.notify();
                    });
                }
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
            // Tab was closed; the task persists, so resume it in its worktree.
            let (wt, n) = (self.tasks[ix].worktree.clone(), self.tasks[ix].n);
            let new_id = self.panel.update(cx, |p, cx| p.open(&wt, Kind::Claude(n), format!("claude {n}"), cx));
            self.tasks[ix].term_id = new_id;
        }
        self.focus_panel(window, cx);
    }

    /// App quit: stop every PTY session (no orphans) and, in the spike only,
    /// optionally remove worktrees created this session.
    fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.panel.update(cx, |p, _| p.close_all_sync());
        if std::env::var("QUR_SPIKE_CLEAN_WORKTREES").as_deref() == Ok("1") {
            for t in &self.tasks {
                let _ = git(&self.root_dir, &["worktree", "remove", "--force", t.worktree.to_str().unwrap()]);
                let _ = git(&self.root_dir, &["branch", "-D", &t.branch]);
            }
        }
    }

    // -- render pieces ------------------------------------------------------

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let e = self.editor.read(cx);
        let file = format!("{}{}", e.file_name(), if e.is_dirty() { " ●" } else { "" });
        div()
            .id("titlebar")
            .window_control_area(gpui::WindowControlArea::Drag)
            .h(ui.z(TITLEBAR_H))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(ui.z(8.))
            .pl(px(TRAFFIC_LIGHTS_W))
            .bg(ui.bg(qur_dark::BG))
            .border_b_1()
            .border_color(hsla(qur_dark::BORDER))
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
            // Symbol slot: rounded-square ring placeholder until the SVG lands in M1.
            .child(div().size(ui.z(14.)).rounded(ui.z(3.5)).border_2().border_color(hsla(qur_dark::TEXT)))
            .child(div().text_size(ui.z(13.)).text_color(hsla(qur_dark::TEXT)).child("kliper"))
            .child(div().text_size(ui.z(12.)).text_color(hsla(qur_dark::TEXT_MUTED)).child(file))
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let row = |accent: u32, name: SharedString, task: SharedString, status: SharedString| {
            div()
                .flex()
                .items_center()
                .gap(ui.z(8.))
                .px(ui.z(8.))
                .py(ui.z(5.))
                .child(div().flex_none().size(ui.z(9.)).rounded(ui.z(2.5)).bg(hsla(accent)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.))
                        .child(div().truncate().text_size(ui.z(13.)).text_color(hsla(qur_dark::TEXT)).child(name))
                        .child(div().truncate().text_size(ui.z(11.)).text_color(hsla(qur_dark::TEXT_MUTED)).child(task)),
                )
                .child(div().flex_none().text_size(ui.z(11.)).text_color(hsla(qur_dark::TEXT_MUTED)).child(status))
        };
        let samples: Vec<_> = sc::sample_agents()
            .into_iter()
            .map(|a| row(a.accent, a.name.into(), a.task.into(), a.status.into()).into_any_element())
            .collect();
        let panel = self.panel.read(cx);
        let tasks: Vec<_> = self
            .tasks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let s = t.term_id.and_then(|id| panel.session_by_id(id));
                let busy = s.is_some_and(|s| s.is_busy());
                let title = s.and_then(|s| s.first_line.clone()).unwrap_or_else(|| t.title.clone());
                (i, t.accent, t.n, title, busy)
            })
            .collect();
        let tasks: Vec<_> = tasks
            .into_iter()
            .map(|(i, accent, n, title, busy)| {
                div()
                    .id(("task", i))
                    .cursor_pointer()
                    .hover(|d| d.bg(hsla(qur_dark::CURRENT_LINE)))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_task(i, window, cx)))
                    .child(row(accent, format!("claude terminal {n}").into(), title.into(), if busy { "~" } else { "·" }.into()))
                    .into_any_element()
            })
            .collect();
        let header = |t: &'static str| {
            div().px(ui.z(8.)).pt(ui.z(10.)).pb(ui.z(4.)).text_size(ui.z(11.)).text_color(hsla(qur_dark::TEXT_MUTED)).child(t)
        };
        div()
            .w(ui.z(SIDEBAR_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(ui.bg(qur_dark::SIDEBAR))
            .border_r_1()
            .border_color(hsla(qur_dark::BORDER))
            .child(header("AGENTS"))
            .children(samples)
            .children(tasks)
            .child(
                div()
                    .id("files-header")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.files_open = !this.files_open;
                        cx.notify();
                    }))
                    .child(header(if self.files_open { "▾ Files" } else { "▸ Files" })),
            )
            .when(self.files_open, |d| d.child(self.tree.clone()))
    }

    fn overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let ui = Ui::get(cx);
        let centered = |child: gpui::AnyElement| {
            div()
                .absolute()
                .top(ui.z(TITLEBAR_H + 40.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(div().occlude().child(child))
                .into_any_element()
        };
        if let Some(p) = &self.palette {
            return Some(centered(p.clone().into_any_element()));
        }
        if let Some(g) = &self.goto {
            return Some(centered(
                div()
                    .w(ui.z(360.))
                    .px(ui.z(10.))
                    .py(ui.z(8.))
                    .bg(hsla(qur_dark::SIDEBAR))
                    .border_1()
                    .border_color(hsla(qur_dark::BORDER))
                    .rounded(ui.z(6.))
                    .child(g.clone())
                    .into_any_element(),
            ));
        }
        if !self.transparency {
            return None;
        }
        let pct = (ui.opacity * 100.).round() as i32;
        let t = (ui.opacity - 0.5) / 0.5;
        let slider = self.slider.clone();
        let track_w = ui.z(200.);
        Some(
            div()
                .absolute()
                .top(ui.z(TITLEBAR_H + 4.))
                .left(px(TRAFFIC_LIGHTS_W))
                .occlude()
                .track_focus(&self.popover_focus)
                .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                    if ev.keystroke.key == "escape" {
                        this.close_overlays(window, cx);
                    }
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.sliding = false))
                .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                    if this.sliding && ev.pressed_button == Some(MouseButton::Left) {
                        this.slide_to(ev.position.x, window, cx);
                    } else {
                        this.sliding = false;
                    }
                }))
                .w(ui.z(240.))
                .p(ui.z(12.))
                .flex()
                .flex_col()
                .gap(ui.z(10.))
                .bg(hsla(qur_dark::SIDEBAR))
                .border_1()
                .border_color(hsla(qur_dark::BORDER))
                .rounded(ui.z(6.))
                .text_size(ui.z(12.))
                .text_color(hsla(qur_dark::TEXT))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child("Window opacity")
                        .child(div().text_color(hsla(qur_dark::TEXT_MUTED)).child(format!("{pct}%"))),
                )
                .child(
                    div()
                        .id("slider")
                        .relative()
                        .w(track_w)
                        .h(ui.z(16.))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &gpui::MouseDownEvent, window, cx| {
                            this.sliding = true;
                            this.slide_to(ev.position.x, window, cx);
                        }))
                        .child(canvas(move |b, _, _| slider.set(b), |_, _, _, _| {}).absolute().size_full())
                        .child(div().absolute().top(ui.z(7.)).left_0().w(track_w).h(ui.z(2.)).bg(hsla(qur_dark::BORDER)))
                        .child(div().absolute().top(ui.z(7.)).left_0().w(track_w * t).h(ui.z(2.)).bg(hsla(qur_dark::TEXT)))
                        .child(
                            div()
                                .absolute()
                                .top(ui.z(2.))
                                .left(track_w * t - ui.z(6.))
                                .size(ui.z(12.))
                                .rounded(ui.z(3.))
                                .bg(hsla(qur_dark::TEXT)),
                        ),
                )
                .child(
                    div()
                        .id("blur")
                        .flex()
                        .items_center()
                        .gap(ui.z(8.))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| {
                            let ui = Ui::get(cx);
                            this.set_opacity(ui.opacity, !ui.blur, window, cx);
                        }))
                        .child(
                            div()
                                .size(ui.z(12.))
                                .rounded(ui.z(3.))
                                .border_1()
                                .border_color(hsla(qur_dark::TEXT))
                                .when(ui.blur, |d| d.bg(hsla(qur_dark::TEXT))),
                        )
                        .child("Blur background"),
                )
                .into_any_element(),
        )
    }
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_menus(cx);
        let ui = Ui::get(cx);
        let any_overlay = self.palette.is_some() || self.goto.is_some() || self.transparency;
        let body = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .when(self.sidebar, |d| d.child(self.sidebar(cx)))
            .child(div().flex_1().min_w(px(0.)).h_full().child(self.editor.clone()));
        let panel = self.panel_visible.then(|| {
            div()
                .h(ui.z(PANEL_H))
                .flex_none()
                .w_full()
                .bg(ui.bg(qur_dark::BG))
                .border_t_1()
                .border_color(hsla(qur_dark::BORDER))
                .child(self.panel.clone())
        });
        div()
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .text_color(hsla(qur_dark::TEXT))
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
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::replace))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::use_selection))
            .on_action(cx.listener(Self::soft_wrap))
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::open_goto))
            .on_action(cx.listener(Self::open_transparency))
            .on_action(cx.listener(Self::new_terminal))
            .on_action(cx.listener(Self::new_claude_task))
            .on_action(cx.listener(Self::next_terminal))
            .on_action(cx.listener(Self::prev_terminal))
            .child(self.title_bar(cx))
            .child(body)
            .children(panel)
            .when(any_overlay, |d| {
                d.child(
                    div()
                        .absolute()
                        .size_full()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_overlays(window, cx))),
                )
            })
            .children(self.overlay(cx))
    }
}

fn main() {
    sc::mark_start();
    let cfg = Config::from_env();
    let opacity: f32 = std::env::var("QUR_SPIKE_OPACITY").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let blur = std::env::var("QUR_SPIKE_BLUR").as_deref() != Ok("0");
    gpui_platform::application().run(move |cx: &mut App| {
        cx.set_global(Ui { zoom: 1.0, opacity: opacity.clamp(0.5, 1.0), blur });
        commands::bind_keys(cx);
        let measuring = cfg.autoscroll || cfg.autotype;
        let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
        let ui = Ui::get(cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Zed-style: transparent system titlebar, content drawn under it,
                    // and we own dragging (Root::title_bar).
                    titlebar: Some(TitlebarOptions {
                        title: None,
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(9.), px(9.))),
                    }),
                    app_owns_titlebar_drag: true,
                    window_background: ui.background_appearance(),
                    // macOS stops the display link for occluded windows, and GPUI
                    // caps (in practice: stalls) unfocused windows. Measurement
                    // runs float the window and lift the cap.
                    kind: if measuring { gpui::WindowKind::PopUp } else { gpui::WindowKind::Normal },
                    inactive_frame_interval: if measuring { None } else { Some(std::time::Duration::from_micros(33_333)) },
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Root::new(cfg, window, cx)),
            )
            .unwrap();
        window
            .update(cx, |root, window, cx| {
                root.focus_editor(window, cx);
                cx.activate(true);
            })
            .unwrap();
        // Spike scripting (works while the screen is locked, when no frames run):
        // QUR_SPIKE_ACTIONS="qur::NewTerminal,qur::NewClaudeTask" dispatches commands
        // 700 ms apart; QUR_SPIKE_QUIT_AFTER_MS quits after that long.
        let actions = std::env::var("QUR_SPIKE_ACTIONS").unwrap_or_default();
        let quit_after: Option<u64> = std::env::var("QUR_SPIKE_QUIT_AFTER_MS").ok().and_then(|v| v.parse().ok());
        let bg = cx.background_executor().clone();
        cx.spawn(async move |cx| {
            for name in actions.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                if let Some(ms) = name.strip_prefix("wait:").and_then(|v| v.parse().ok()) {
                    bg.timer(std::time::Duration::from_millis(ms)).await;
                    continue;
                }
                bg.timer(std::time::Duration::from_millis(700)).await;
                // Same code path as the tab's ×, for runs where no frames render.
                if name == "dump" {
                    window.update(cx, |root, _, cx| root.editor.read(cx).debug_dump()).ok();
                    continue;
                }
                if name == "closetab" {
                    window
                        .update(cx, |root, _, cx| root.panel.update(cx, |p, cx| p.close(p.active, cx)))
                        .ok();
                    continue;
                }
                let Some(c) = commands::COMMANDS.iter().find(|c| (c.action)().name() == name) else {
                    eprintln!("unknown action {name}");
                    continue;
                };
                window.update(cx, |_, window, cx| window.dispatch_action((c.action)(), cx)).ok();
            }
            if let Some(ms) = quit_after {
                bg.timer(std::time::Duration::from_millis(ms)).await;
                cx.update(|cx| cx.quit());
            }
        })
        .detach();
        cx.on_app_quit(move |cx| {
            window.update(cx, |root, _, cx| root.shutdown(cx)).ok();
            async {}
        })
        .detach();
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
}
