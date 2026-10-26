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
    pub(super) path: Option<PathBuf>,
    shown: Shown,
    /// Not an image: the hex view, or a large file's paged text (file_ops.rs).
    pub(super) body: Option<super::file_ops::FileBody>,
    /// "1280 × 800 · 245.1 KB".
    detail: String,
    prev_focus: Option<FocusHandle>,
    /// Reading and decoding the image; dropped (cancelled) with the viewer.
    _load: Option<Task<()>>,
}

/// What the viewer's body shows.
enum Shown {
    /// A file Qur can't show (or an image that didn't decode).
    File,
    Loading,
    /// Decoded by Qur to fit the modal: never in GPUI's asset cache, and its
    /// texture is freed when the viewer closes.
    Pixels(Arc<gpui::RenderImage>),
    /// SVG (drawn sharp at any size) and GIF (animated): GPUI decodes them;
    /// dropped from its cache when the viewer closes.
    Gpui(Arc<gpui::Image>),
}

impl Viewer {
    /// Scripted runs' `state` line: "title (image|loading|file) detail [body]".
    pub(super) fn debug_state(&self, cx: &App) -> String {
        let kind = match self.shown {
            Shown::Pixels(_) | Shown::Gpui(_) => "image",
            Shown::Loading => "loading",
            Shown::File => "file",
        };
        let body = self.body.as_ref().map(|b| format!(" [{}]", b.debug_state(cx))).unwrap_or_default();
        format!("{} ({kind}) {}{body}", self.title, self.detail)
    }

    /// Give back the image's memory: its texture, and GPUI's decoded copy.
    fn free(self, window: &mut Window, cx: &mut App) {
        match self.shown {
            Shown::Pixels(r) => cx.drop_image(r, Some(window)),
            Shown::Gpui(image) => {
                if let Some(r) = image.clone().get_render_image(window, cx) {
                    cx.drop_image(r, Some(window));
                }
                image.remove_asset(cx);
            }
            Shown::File | Shown::Loading => {}
        }
        files::trim_memory();
    }
}

impl Workspace {
    /// Show what a chip or a sent attachment points at.
    pub(super) fn preview(&mut self, p: Preview, window: &mut Window, cx: &mut Context<Self>) {
        match p {
            Preview::File(path) => self.open_path(&path, window, cx),
            Preview::Image { label, image } => {
                let detail = detail(files::image_size(image.bytes()), image.bytes().len() as u64);
                let v = Viewer { title: label.to_string(), owner: None, path: None, shown: Shown::Loading, body: None, detail, prev_focus: None, _load: None };
                self.show_viewer(v, window, cx);
                self.load_viewer(move || Some(image), window, cx);
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
        let format = files::viewable(path);
        let (title, owner) = self.where_is(path, cx);
        let body = if format.is_none() { self.file_body(path, len, window, cx) } else { None };
        let (shown, detail) = match format {
            Some(_) => (Shown::Loading, files::human_size(len)),
            None => (Shown::File, Self::body_detail(body.as_ref(), len)),
        };
        self.show_viewer(Viewer { title, owner, path: Some(path.to_path_buf()), shown, body, detail, prev_focus: None, _load: None }, window, cx);
        self.focus_viewer_body(window, cx);
        if let Some(format) = format {
            let path = path.to_path_buf();
            self.load_viewer(move || Some(Arc::new(gpui::Image::from_bytes(format, std::fs::read(&path).ok()?))), window, cx);
        }
    }

    /// Read and decode the open viewer's image off the main thread, to fit
    /// the modal's pixels (86% × 88% of the window), then show it.
    fn load_viewer(&mut self, read: impl FnOnce() -> Option<Arc<gpui::Image>> + Send + 'static, window: &mut Window, cx: &mut Context<Self>) {
        let (vp, scale) = (window.viewport_size(), window.scale_factor());
        let max = ((f32::from(vp.width) * scale * 0.86) as u32, (f32::from(vp.height) * scale * 0.88) as u32);
        let work = cx.background_executor().spawn(async move {
            let image = read()?;
            let size = files::image_size(image.bytes());
            let len = image.bytes().len() as u64;
            let shown = match image.format {
                gpui::ImageFormat::Svg | gpui::ImageFormat::Gif => Shown::Gpui(image),
                _ => Shown::Pixels(files::thumbnail(image.bytes(), max.0, max.1)?),
            };
            Some((shown, detail(size, len)))
        });
        let task = cx.spawn(async move |this, cx| {
            let loaded = work.await;
            this.update(cx, |ws, cx| {
                let Some(v) = &mut ws.viewer else { return };
                match loaded {
                    Some((shown, detail)) => {
                        v.shown = shown;
                        v.detail = detail;
                    }
                    None => {
                        v.shown = Shown::File;
                        v.detail = format!("{} · couldn't be read", v.detail);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        if let Some(v) = &mut self.viewer {
            v._load = Some(task);
        }
    }

    fn show_viewer(&mut self, mut v: Viewer, window: &mut Window, cx: &mut Context<Self>) {
        // Opened from another viewer: back to where that one came from, and
        // that one's image is freed.
        let old = self.viewer.take();
        v.prev_focus = old.as_ref().and_then(|o| o.prev_focus.clone()).or_else(|| window.focused(cx));
        if let Some(o) = old {
            o.free(window, cx);
        }
        self.viewer = Some(v);
        // The keyboard leaves the prompt or terminal behind (Esc closes).
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn close_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut v) = self.viewer.take() {
            match v.prev_focus.take() {
                Some(f) => window.focus(&f, cx),
                None => self.focus_stage(window, cx),
            }
            v.free(window, cx);
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
        let body = match &v.shown {
            // Never larger than its own size; scaled down to fit the modal.
            Shown::Pixels(r) => div().size_full().p(look.z(16.)).child(img(r.clone()).size_full().object_fit(gpui::ObjectFit::ScaleDown)).into_any_element(),
            Shown::Gpui(image) => div().size_full().p(look.z(16.)).child(img(image.clone()).size_full().object_fit(gpui::ObjectFit::ScaleDown)).into_any_element(),
            Shown::Loading => div().size_full().flex().items_center().justify_center().text_color(look.c(ui.text_muted)).child("Loading…").into_any_element(),
            Shown::File if v.body.is_some() => v.body.as_ref().map(|b| b.element()).unwrap_or_else(|| div().into_any_element()),
            Shown::File => div()
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
                                .children(self.viewer_buttons(look, cx))
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

