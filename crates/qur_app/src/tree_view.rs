//! The Files dropdown: a virtualized tree of the project (or, from M2, the
//! selected agent's worktree). In M3 this becomes the heatmap.

use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{Context, EventEmitter, ScrollStrategy, UniformListScrollHandle, Window, div, prelude::*, px, svg, uniform_list};
use qur_tree::FileTree;

use crate::assets::icon;
use crate::look::Look;
use crate::prompt::DraggedFile;

pub enum TreeEvent {
    Open(PathBuf),
}

pub struct TreeView {
    pub tree: FileTree,
    /// Absolute path of the file shown in the editor (highlighted row).
    pub current: Option<PathBuf>,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<TreeEvent> for TreeView {}

impl TreeView {
    pub fn new(root: &Path) -> Self {
        TreeView { tree: FileTree::new(root), current: None, scroll: UniformListScrollHandle::new() }
    }

    /// Expand to `abs` and scroll it into view.
    pub fn reveal(&mut self, abs: &Path, cx: &mut Context<Self>) {
        if let Ok(rel) = abs.strip_prefix(self.tree.root())
            && let Some(row) = self.tree.reveal(rel)
        {
            self.scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.tree.refresh(Path::new(""));
        cx.notify();
    }
}

impl Render for TreeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        uniform_list(
            "files",
            self.tree.visible().len(),
            cx.processor(|this, range: Range<usize>, window, cx| {
                let look = Look::of(window, cx);
                let ui = &look.theme.ui;
                range
                    .map(|row| {
                        let id = this.tree.visible()[row];
                        let n = this.tree.node(id);
                        let is_dir = n.is_dir();
                        let current = !is_dir && this.current.as_deref() == Some(this.tree.abs_path(id).as_path());
                        let chevron = is_dir.then(|| {
                            svg()
                                .path(if n.is_expanded() { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT })
                                .size(look.z(12.))
                                .flex_none()
                                .text_color(look.c(ui.text_muted))
                        });
                        div()
                            .id(row)
                            .w_full()
                            .h(look.z(22.))
                            .pl(look.z(8. + 12. * n.depth as f32))
                            .pr(look.z(8.))
                            .flex()
                            .items_center()
                            .gap(look.z(4.))
                            .text_size(look.text(-1.))
                            .text_color(look.c(if current { ui.text } else { ui.text_secondary }))
                            .when(current, |d| d.bg(look.c(ui.active_bg)))
                            .hover(|d| d.bg(look.c(ui.hover_bg)))
                            .cursor_pointer()
                            .children(chevron)
                            .when(!is_dir, |d| d.child(div().w(look.z(12.)).flex_none()))
                            .child(div().min_w(px(0.)).truncate().child(n.name.clone()))
                            .on_drag(
                                DraggedFile { abs: this.tree.abs_path(id), label: n.name.clone().into() },
                                |f, _, _, cx| cx.new(|_| f.clone()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.tree.node(id).is_dir() {
                                    this.tree.toggle(id);
                                } else {
                                    cx.emit(TreeEvent::Open(this.tree.abs_path(id)));
                                }
                                cx.notify();
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
    }
}
