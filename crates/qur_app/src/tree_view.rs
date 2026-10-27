//! The Files tree: a virtualized tree of the project or the selected task's
//! worktree. For a worktree it is the live heatmap (handoff section 9): rows
//! glow in the agent's accent as it reads and edits, and files it deleted stay
//! as struck-through rows; the main checkout's tree shows overlap dots for
//! files agents changed in their worktrees.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, Context, Entity, EventEmitter, ExternalDragPayload, FileDragPaths, Hsla, MouseButton, MouseDownEvent, Pixels, Point, ScrollStrategy,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, svg, uniform_list,
};
use qur_heat::HeatState;
use qur_theme::Rgba;
use qur_tree::FileTree;

use crate::assets::icon;
use crate::look::Look;
use crate::monitor::TaskMonitor;
use crate::prompt::DraggedFile;

/// QUR_BENCH=glow: when each probe file was written (bench thread) ...
pub static GLOW_WRITES: std::sync::Mutex<Vec<(PathBuf, Instant)>> = std::sync::Mutex::new(Vec::new());
/// ... and the write-to-glow latencies measured at paint.
pub static GLOW_SEEN: std::sync::Mutex<Vec<f64>> = std::sync::Mutex::new(Vec::new());

/// "Do not fight the user": no auto-expand within 5 s of a click or scroll.
const USER_GRACE: Duration = Duration::from_secs(5);

pub enum TreeEvent {
    Open(PathBuf),
    /// Right-click on a row: its absolute path, its path in the tree, whether
    /// it is a folder or a deleted file's row, and where the pointer was.
    Menu { abs: PathBuf, rel: PathBuf, is_dir: bool, ghost: bool, at: Point<Pixels> },
}

pub struct TreeView {
    pub tree: FileTree,
    /// Absolute path of the file shown in the code modal (highlighted row).
    pub current: Option<PathBuf>,
    /// Heat for this worktree (task trees).
    monitor: Option<Entity<TaskMonitor>>,
    /// Main-checkout tree: every task's monitor, for overlap dots.
    pub overlaps: Vec<Entity<TaskMonitor>>,
    /// Repaint when a task's heat changes (the dots come from it).
    overlap_subs: Vec<Subscription>,
    last_user: Option<Instant>,
    /// The focus path last auto-revealed (reveal once per change of focus).
    revealed: Option<PathBuf>,
    scroll: UniformListScrollHandle,
    _sub: Option<Subscription>,
}

impl EventEmitter<TreeEvent> for TreeView {}

impl TreeView {
    pub fn new(root: &Path) -> Self {
        TreeView {
            tree: FileTree::new(root),
            current: None,
            monitor: None,
            overlaps: Vec::new(),
            overlap_subs: Vec::new(),
            last_user: None,
            revealed: None,
            scroll: UniformListScrollHandle::new(),
            _sub: None,
        }
    }

    /// Main checkout: the tasks whose changed files get overlap dots.
    pub fn set_overlaps(&mut self, monitors: Vec<Entity<TaskMonitor>>, cx: &mut Context<Self>) {
        self.overlap_subs = monitors.iter().map(|m| cx.observe(m, |_, _, cx| cx.notify())).collect();
        self.overlaps = monitors;
        cx.notify();
    }

    pub fn set_monitor(&mut self, m: Entity<TaskMonitor>, cx: &mut Context<Self>) {
        self._sub = Some(cx.observe(&m, |this, _, cx| {
            this.sync_ghosts(cx);
            this.follow_focus(cx);
        }));
        self.monitor = Some(m);
        self.sync_ghosts(cx);
        cx.notify();
    }

    /// Files the agent deleted keep a row (struck through, still glowing) until
    /// they come back or the task is merged or discarded. Reloads the tree only
    /// when that set changes.
    fn sync_ghosts(&mut self, cx: &mut Context<Self>) {
        let Some(m) = &self.monitor else { return };
        let deleted = m.read(cx).heat.deleted();
        if self.tree.ghosts().len() != deleted.len() || !deleted.iter().all(|d| self.tree.ghosts().contains(d)) {
            self.tree.set_ghosts(deleted);
            cx.notify();
        }
    }

    /// When the agent starts editing a deep file, expand its folders and
    /// scroll it into view (unless the user touched the tree recently).
    fn follow_focus(&mut self, cx: &mut Context<Self>) {
        let Some(m) = &self.monitor else { return };
        let focus = m.read(cx).heat.focus().map(|(p, _)| p);
        if let Some(abs) = focus.clone()
            && focus != self.revealed
            && !self.last_user.is_some_and(|t| t.elapsed() < USER_GRACE)
        {
            // A file the agent is about to create isn't in the tree yet; the
            // watcher refreshes the tree when it appears and this retries then.
            if self.reveal(&abs, cx) {
                self.revealed = focus;
            }
        }
        cx.notify();
    }

    /// Expand to `abs` and scroll it into view; false when it isn't in the tree.
    pub fn reveal(&mut self, abs: &Path, cx: &mut Context<Self>) -> bool {
        let mut found = false;
        if let Ok(rel) = abs.strip_prefix(self.tree.root())
            && let Some(row) = self.tree.reveal(rel)
        {
            self.scroll.scroll_to_item(row, ScrollStrategy::Center);
            found = true;
        }
        cx.notify();
        found
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.tree.refresh(Path::new(""));
        cx.notify();
    }
}

fn tint(accent: Rgba, alpha: f32, look: &Look) -> Hsla {
    let mut h = look.c(accent);
    h.a = alpha;
    h
}

impl Render for TreeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Overlap dots: which agents changed each path (and its folders), main tree only.
        let mut overlap: HashMap<PathBuf, Vec<Rgba>> = HashMap::new();
        for m in &self.overlaps {
            let m = m.read(cx);
            for rel in m.heat.changed() {
                let mut p = rel.as_path();
                loop {
                    let v = overlap.entry(p.to_path_buf()).or_default();
                    if !v.contains(&m.accent) {
                        v.push(m.accent);
                    }
                    match p.parent() {
                        Some(q) if !q.as_os_str().is_empty() => p = q,
                        _ => break,
                    }
                }
            }
        }
        uniform_list(
            "files",
            self.tree.visible().len(),
            cx.processor(move |this, range: Range<usize>, window, cx| {
                let look = Look::of(window, cx);
                let ui = look.theme.ui.clone();
                let now = Instant::now();
                let heat = this.monitor.as_ref().map(|m| m.read(cx));
                let accent = heat.map(|m| m.accent).unwrap_or(ui.text_muted);
                range
                    .map(|row| {
                        let id = this.tree.visible()[row];
                        let n = this.tree.node(id);
                        let is_dir = n.is_dir();
                        let ghost = n.ghost;
                        let rel = n.rel.clone();
                        let current = !is_dir && this.current.as_deref() == Some(this.tree.abs_path(id).as_path());
                        let (cell, folder) = match heat {
                            Some(m) if is_dir => (None, m.heat.folder_state(&rel, now)),
                            Some(m) => (Some(m.heat.cell(&rel, now)), None),
                            None => (None, None),
                        };
                        let state = cell.map(|c| c.state).unwrap_or_default();
                        let level = cell.map(|c| c.level).unwrap_or(0);
                        let chevron = is_dir.then(|| {
                            svg()
                                .path(if n.is_expanded() { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT })
                                .size(look.z(12.))
                                .flex_none()
                                .text_color(look.c(ui.text_muted))
                        });
                        // Right side: +n -m, "new", folder dot, overlap dots.
                        let mut right = div().flex_none().flex().items_center().gap(look.z(4.)).text_size(look.text(-3.));
                        if let Some(st) = cell.and_then(|c| c.stat) {
                            if st.created {
                                right = right.child(div().text_color(look.c(accent)).child("new"));
                            } else if st.binary {
                                right = right.child(div().text_color(look.c(ui.text_muted)).child("bin"));
                            } else if st.deleted {
                                right = right.when(st.removed > 0, |d| d.child(div().text_color(look.c(look.theme.diff.removed_text)).child(format!("−{}", st.removed))));
                            } else {
                                right = right
                                    .when(st.added > 0, |d| d.child(div().text_color(look.c(look.theme.diff.added_text)).child(format!("+{}", st.added))))
                                    .when(st.removed > 0, |d| d.child(div().text_color(look.c(look.theme.diff.removed_text)).child(format!("−{}", st.removed))));
                            }
                        }
                        if let Some(fs) = folder {
                            let strong = matches!(fs, HeatState::Editing | HeatState::NeedsApproval | HeatState::Reading);
                            right = right.child(div().size(look.z(6.)).rounded(look.z(1.5)).bg(tint(accent, if strong { 1.0 } else { 0.55 }, &look)));
                        }
                        if let Some(dots) = overlap.get(&rel) {
                            for a in dots.iter().take(3) {
                                right = right.child(div().size(look.z(6.)).rounded(look.z(1.5)).bg(look.c(*a)));
                            }
                        }
                        let text_color = if current || matches!(state, HeatState::Editing | HeatState::NeedsApproval) {
                            ui.text
                        } else {
                            ui.text_secondary
                        };
                        // Background: heat tint by level, transient glows on top.
                        let base_bg = match state {
                            HeatState::Changed | HeatState::Created | HeatState::Deleted if level > 0 => Some(tint(accent, 0.05 + 0.05 * level as f32, &look)),
                            HeatState::Changed | HeatState::Created | HeatState::Deleted => Some(tint(accent, 0.06, &look)),
                            HeatState::Reading => Some(tint(accent, 0.22 * cell.map(|c| c.fade).unwrap_or(0.), &look)),
                            _ => None,
                        };
                        let mark = crate::script::mark(|| format!("file:{}", rel.display()));
                        let row_el = div()
                            .id(row)
                            .relative()
                            .children(mark)
                            .w_full()
                            .h(look.z(22.))
                            .pl(look.z(8. + 12. * n.depth as f32))
                            .pr(look.z(8.))
                            .flex()
                            .items_center()
                            .gap(look.z(4.))
                            .text_size(look.text(-1.))
                            .text_color(look.c(text_color))
                            .when(current, |d| d.bg(look.c(ui.active_bg)))
                            .when_some(base_bg.filter(|_| !current), |d, bg| d.bg(bg))
                            .when(state == HeatState::NeedsApproval, |d| d.border_1().border_color(look.c(accent)).rounded(look.z(3.)))
                            // Deleted (or a folder of deleted files): struck through and dimmed.
                            .when(state == HeatState::Deleted || ghost, |d| d.line_through().opacity(0.55))
                            .hover(|d| d.bg(look.c(ui.hover_bg)))
                            .cursor_pointer()
                            .children(chevron)
                            .when(!is_dir, |d| d.child(div().w(look.z(12.)).flex_none()))
                            .child(div().flex_1().min_w(px(0.)).truncate().child(n.name.clone()))
                            .child(right)
                            .when(!ghost, |d| {
                                d.on_drag(DraggedFile { abs: this.tree.abs_path(id), label: n.name.clone().into() }, |f, _, _, cx| cx.new(|_| f.clone()))
                                    // Out of the window it becomes a macOS file drag (Finder, Mail, other apps).
                                    .external_drag_payload(move |f: &DraggedFile, _, _| Some(ExternalDragPayload::Files(FileDragPaths::new([(f.abs.clone(), is_dir)]))))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.last_user = Some(Instant::now());
                                if this.tree.node(id).is_dir() {
                                    this.tree.toggle(id);
                                } else if !this.tree.node(id).ghost {
                                    // A deleted file has nothing to open (Changes shows its diff).
                                    cx.emit(TreeEvent::Open(this.tree.abs_path(id)));
                                }
                                cx.notify();
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    this.last_user = Some(Instant::now());
                                    let n = this.tree.node(id);
                                    let (rel, is_dir, ghost) = (n.rel.clone(), n.is_dir(), n.ghost);
                                    cx.emit(TreeEvent::Menu { abs: this.tree.abs_path(id), rel, is_dir, ghost, at: ev.position });
                                    cx.stop_propagation();
                                }),
                            );
                        if state == HeatState::Editing && cfg!(not(test)) {
                            let abs = this.tree.abs_path(id);
                            let mut w = GLOW_WRITES.lock().unwrap();
                            if let Some(i) = w.iter().position(|(p, _)| *p == abs) {
                                let (_, t0) = w.remove(i);
                                GLOW_SEEN.lock().unwrap().push(t0.elapsed().as_secs_f64() * 1000.0);
                            }
                        }
                        if state == HeatState::Editing {
                            // Pulsing glow in the agent's accent (30 fps cap keeps it light).
                            let glow = look.c(accent);
                            row_el
                                .with_animation(
                                    ("pulse", row),
                                    Animation::new(Duration::from_millis(1400)).repeat().with_max_fps(30.).with_easing(gpui::pulsating_between(0.10, 0.34)),
                                    move |el, a| el.bg(Hsla { a, ..glow }),
                                )
                                .into_any_element()
                        } else {
                            row_el.into_any_element()
                        }
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
    }
}
