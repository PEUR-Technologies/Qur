//! The file viewer: a modal like the code modal for what isn't text. Images
//! (from the tree, a chat link, a pasted screenshot) show fitted to it; any
//! other file gets its name and size, with Open in Default App and Reveal in
//! Finder. Esc or a click outside closes it.

use std::sync::Arc;

use super::*;
use crate::files::{self, Preview};

pub(super) struct Viewer {
    title: String,
    /// The task whose worktree the file is in.
    owner: Option<String>,
    path: Option<PathBuf>,
    /// None: a file Qur can't show.
    image: Option<Arc<gpui::Image>>,
    /// "1280 × 800 · 245.1 KB".
    detail: String,
    prev_focus: Option<FocusHandle>,
}

impl Viewer {
    /// Scripted runs' `state` line: "title (image|file) detail".
    pub(super) fn debug_state(&self) -> String {
        format!("{} ({}) {}", self.title, if self.image.is_some() { "image" } else { "file" }, self.detail)
    }
}

impl Workspace {
    /// Show what a chip or a sent attachment points at.
    pub(super) fn preview(&mut self, p: Preview, window: &mut Window, cx: &mut Context<Self>) {
        match p {
            Preview::File(path) => self.open_path(&path, window, cx),
            Preview::Image { label, image } => {
                let size = files::image_size(image.bytes());
                let detail = detail(size, image.bytes().len() as u64);
                self.show_viewer(Viewer { title: label.to_string(), owner: None, path: None, image: Some(image), detail, prev_focus: None }, window, cx);
            }
        }
    }

    /// An image or a file that isn't text, from its path.
    pub(super) fn view_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let len = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m.len(),
            Ok(_) => return self.flash(format!("{} isn't a file", path.display()), cx),
            Err(e) => return self.flash(format!("Couldn't open {}: {e}", path.display()), cx),
        };
        let image = files::viewable(path).and_then(|format| Some(Arc::new(gpui::Image::from_bytes(format, std::fs::read(path).ok()?))));
        let size = image.as_ref().and_then(|i| files::image_size(i.bytes()));
        let (title, owner) = self.where_is(path, cx);
        let detail = if image.is_some() { detail(size, len) } else { format!("{} · not a text file", files::human_size(len)) };
        self.show_viewer(Viewer { title, owner, path: Some(path.to_path_buf()), image, detail, prev_focus: None }, window, cx);
    }

    fn show_viewer(&mut self, mut v: Viewer, window: &mut Window, cx: &mut Context<Self>) {
        // Opened from another viewer: back to where that one came from.
        v.prev_focus = self.viewer.take().and_then(|o| o.prev_focus).or_else(|| window.focused(cx));
        self.viewer = Some(v);
        // The keyboard leaves the prompt or terminal behind (Esc closes).
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn close_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(v) = self.viewer.take() {
            match v.prev_focus {
                Some(f) => window.focus(&f, cx),
                None => self.focus_stage(window, cx),
            }
            cx.notify();
        }
    }

    pub(super) fn viewer_modal(&self, look: &Look, cx: &mut Context<Self>) -> Option<AnyElement> {
        let v = self.viewer.as_ref()?;
        let ui = &look.theme.ui;
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .flex_none()
                .px(look.z(8.))
                .py(look.z(2.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_color(look.c(ui.text_secondary))
                .hover(|d| d.bg(look.c(ui.hover_bg)).text_color(look.c(ui.text)))
                .child(label)
        };
        let buttons = v.path.clone().map(|p| {
            let q = p.clone();
            div()
                .flex()
                .flex_none()
                .gap(look.z(2.))
                .child(button("viewer-open", "Open in Default App").on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&p))))
                .child(button("viewer-reveal", "Reveal in Finder").on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&q))))
        });
        let body = match &v.image {
            // Never larger than its own size; scaled down to fit the modal.
            Some(image) => div().size_full().p(look.z(16.)).child(img(image.clone()).size_full().object_fit(gpui::ObjectFit::ScaleDown)).into_any_element(),
            None => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(look.z(6.))
                .text_color(look.c(ui.text_muted))
                .child(div().text_size(look.text(8.)).child("▤"))
                .child(div().text_color(look.c(ui.text_secondary)).child("Qur can't show this file."))
                .child(div().text_size(look.text(-1.)).child("Open it in its own app, or reveal it in Finder."))
                .into_any_element(),
        };
        Some(
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.35))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_viewer(window, cx)))
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
                                .child(div().min_w(px(0.)).truncate().text_color(look.c(ui.text)).child(v.title.clone()))
                                .children(v.owner.clone().map(|w| div().flex_none().text_color(look.c(ui.text_muted)).child(format!("in {w}"))))
                                .child(div().flex_none().text_color(look.c(ui.text_muted)).child(v.detail.clone()))
                                .child(div().flex_1())
                                .children(buttons)
                                .child(div().flex_none().text_color(look.c(ui.text_muted)).child("esc to close")),
                        )
                        .child(div().flex_1().min_h(px(0.)).child(body)),
                )
                .into_any_element(),
        )
    }
}

/// "1280 × 800 · 245.1 KB", or just the size when the pixels aren't known.
fn detail(size: Option<(u32, u32)>, len: u64) -> String {
    match size {
        Some((w, h)) => format!("{w} × {h} · {}", files::human_size(len)),
        None => files::human_size(len),
    }
}

