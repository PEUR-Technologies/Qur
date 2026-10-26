//! The Files tree's right-click menu (Open, Open in Default App, Reveal in
//! Finder, Copy Path, Copy Relative Path), and File > Reveal in Finder / Copy
//! Path / Copy Relative Path for the file in the code modal.

use super::*;

#[derive(Clone, Copy)]
enum FileCmd {
    Open,
    OpenDefault,
    Reveal,
    CopyPath,
    CopyRelative,
}

impl Workspace {
    #[allow(clippy::too_many_arguments)] // the row's facts, as the tree sent them
    pub(super) fn open_file_menu(&mut self, abs: PathBuf, rel: PathBuf, is_dir: bool, ghost: bool, at: gpui::Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        self.overlay = Some(Overlay::FileMenu { abs, rel, is_dir, ghost, at });
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn file_menu(&self, abs: &Path, rel: &Path, is_dir: bool, ghost: bool, at: gpui::Point<Pixels>, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        // None is a separator. A deleted file's row is all greyed out: there is
        // no file to open, reveal or take the path of.
        let mut items: Vec<Option<(&'static str, FileCmd, bool)>> = Vec::new();
        if !is_dir {
            items.push(Some(("Open", FileCmd::Open, !ghost)));
            items.push(Some(("Open in Default App", FileCmd::OpenDefault, !ghost)));
            items.push(None);
        }
        items.push(Some(("Reveal in Finder", FileCmd::Reveal, !ghost)));
        items.push(None);
        items.push(Some(("Copy Path", FileCmd::CopyPath, !ghost)));
        items.push(Some(("Copy Relative Path", FileCmd::CopyRelative, !ghost)));
        let (abs, rel) = (abs.to_path_buf(), rel.to_path_buf());
        div()
            .absolute()
            .left(at.x)
            .top(at.y)
            .child(
                div()
                    .id("file-menu")
                    .occlude()
                    .min_w(look.z(190.))
                    .py(look.z(4.))
                    .flex()
                    .flex_col()
                    .bg(look.c(ui.overlay_bg))
                    .border_1()
                    .border_color(look.c(ui.border_strong))
                    .rounded(look.z(6.))
                    .shadow_md()
                    .children(items.into_iter().enumerate().map(|(i, item)| {
                        let Some((label, which, enabled)) = item else {
                            return div().my(look.z(4.)).h(px(1.)).bg(look.c(ui.border)).into_any_element();
                        };
                        let (abs, rel) = (abs.clone(), rel.clone());
                        div()
                            .id(("file-menu-item", i))
                            .relative()
                            .children(crate::script::mark(|| format!("menu:{label}")))
                            .mx(look.z(4.))
                            .px(look.z(10.))
                            .py(look.z(4.))
                            .rounded(look.z(4.))
                            .text_size(look.text(-1.))
                            .text_color(look.c(if enabled { ui.text } else { ui.text_muted }))
                            .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(look.c(ui.hover_bg))))
                            .child(label)
                            .when(enabled, |d| {
                                d.on_click(cx.listener(move |this, _, window, cx| {
                                    this.overlay = None;
                                    this.restore_focus(window, cx);
                                    this.file_cmd(which, &abs, &rel, window, cx);
                                    cx.notify();
                                }))
                            })
                            .into_any_element()
                    })),
            )
            .into_any_element()
    }

    fn file_cmd(&mut self, which: FileCmd, abs: &Path, rel: &Path, window: &mut Window, cx: &mut Context<Self>) {
        match which {
            FileCmd::Open => self.open_path(abs, window, cx),
            FileCmd::OpenDefault => cx.open_with_system(abs),
            FileCmd::Reveal => self.reveal_file(abs, cx),
            FileCmd::CopyPath => self.copy_and_flash(&abs.to_string_lossy(), "Copied path", cx),
            FileCmd::CopyRelative => self.copy_and_flash(&rel.to_string_lossy(), "Copied relative path", cx),
        }
    }

    fn reveal_file(&mut self, abs: &Path, cx: &mut Context<Self>) {
        if abs.exists() {
            cx.reveal_path(abs);
        } else {
            self.flash(format!("{} no longer exists", abs.display()), cx);
        }
    }

    fn copy_and_flash(&mut self, text: &str, msg: &'static str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        self.flash(msg, cx);
    }

    /// The file in the code modal, for File > Reveal in Finder and the Copy Paths.
    fn code_path(&mut self, cx: &mut Context<Self>) -> Option<PathBuf> {
        let p = self.code.as_ref().and_then(|c| c.read(cx).path().map(Path::to_path_buf));
        if p.is_none() {
            self.flash("No file is open", cx);
        }
        p
    }

    pub(super) fn reveal_in_finder(&mut self, _: &qur::RevealInFinder, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.code_path(cx) {
            self.reveal_file(&p, cx);
        }
    }

    pub(super) fn copy_file_path(&mut self, _: &qur::CopyPath, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.code_path(cx) {
            self.copy_and_flash(&p.to_string_lossy(), "Copied path", cx);
        }
    }

    /// Relative to the checkout the file is in: the project, or a task's worktree.
    pub(super) fn copy_relative_path(&mut self, _: &qur::CopyRelativePath, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.code_path(cx) {
            let rel = self.where_is(&p, cx).0;
            self.copy_and_flash(&rel, "Copied relative path", cx);
        }
    }
}
