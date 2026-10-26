//! Notepad++-style file handling: Open File… (any path, `Cmd+O`), Open as
//! Text, Reopen with / Convert to Encoding…, Convert Line Endings, Show
//! Whitespace, the code modal's "UTF-8 · LF" label, and what the file viewer
//! shows for files that aren't images: the hex view for binaries, paged text
//! for files over `[editor] large_file_mb`.

use std::cell::RefCell;
use std::time::SystemTime;

use qur_editor::encoding::{self, Encoding};

use super::*;
use crate::hex_view::{HexEvent, HexView};
use crate::large_view::{LargeEvent, LargeView};

/// The file viewer's body for a file that isn't an image.
pub(super) enum FileBody {
    Hex(Entity<HexView>),
    Text(Entity<LargeView>),
}

impl FileBody {
    pub(super) fn element(&self) -> AnyElement {
        match self {
            FileBody::Hex(v) => v.clone().into_any_element(),
            FileBody::Text(v) => v.clone().into_any_element(),
        }
    }

    fn focus(&self, cx: &App) -> FocusHandle {
        match self {
            FileBody::Hex(v) => v.read(cx).focus.clone(),
            FileBody::Text(v) => v.read(cx).focus.clone(),
        }
    }

    pub(super) fn debug_state(&self, cx: &App) -> String {
        match self {
            FileBody::Hex(v) => v.read(cx).debug_state(),
            FileBody::Text(v) => v.read(cx).debug_state(cx),
        }
    }
}

/// Encoding picker: reopen the file decoded this way, or convert the buffer.
#[derive(Clone, Copy)]
pub(super) enum EncodingAction {
    Reopen,
    Convert,
}

thread_local! {
    /// Files opened from outside the project (no watcher covers them): when
    /// each was last seen on disk, checked when the window comes back.
    static OUTSIDE: RefCell<HashMap<PathBuf, SystemTime>> = RefCell::new(HashMap::new());
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Workspace {
    fn large_bytes(&self) -> u64 {
        self.config.editor.large_file_mb.saturating_mul(1 << 20)
    }

    /// The file viewer, not the code modal: images, binaries (a NUL in the
    /// first 8 KB that isn't UTF-16) and files over the large-file size.
    /// Text in any other encoding opens as text.
    pub(super) fn wants_viewer(&self, path: &Path) -> bool {
        if crate::files::viewable(path).is_some() {
            return true;
        }
        match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m.len() > self.large_bytes() || encoding::file_is_binary(path).unwrap_or(false),
            _ => false,
        }
    }

    /// The viewer's body for a non-image file of `len` bytes: paged text for
    /// a large text file, the hex view otherwise.
    pub(super) fn file_body(&mut self, path: &Path, len: u64, window: &mut Window, cx: &mut Context<Self>) -> Option<FileBody> {
        let binary = encoding::file_is_binary(path).unwrap_or(true);
        Some(if len > self.large_bytes() && !binary { self.text_body(path, window, cx) } else { self.hex_body(path, window, cx) })
    }

    fn hex_body(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> FileBody {
        let v = cx.new(|cx| HexView::new(path, cx));
        let sub = cx.subscribe_in(&v, window, |this, _, ev: &HexEvent, _, cx| match ev {
            HexEvent::Message(m) => this.flash(m.clone(), cx),
        });
        self._subs.push(sub);
        FileBody::Hex(v)
    }

    fn text_body(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> FileBody {
        let v = cx.new(|cx| LargeView::new(path, cx));
        let sub = cx.subscribe_in(&v, window, |this, _, ev: &LargeEvent, _, cx| match ev {
            LargeEvent::Message(m) => this.flash(m.clone(), cx),
        });
        self._subs.push(sub);
        FileBody::Text(v)
    }

    /// "2.1 GB · binary" for the viewer's header.
    pub(super) fn body_detail(body: Option<&FileBody>, len: u64) -> String {
        let kind = match body {
            Some(FileBody::Hex(_)) => "binary",
            Some(FileBody::Text(_)) => "large file",
            None => "not a text file",
        };
        format!("{} · {kind}", crate::files::human_size(len))
    }

    /// The keyboard goes to the viewer's hex or text view (scroll keys, ⌘C, ⌃G, ⌘F).
    pub(super) fn focus_viewer_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.viewer.as_ref().and_then(|v| v.body.as_ref()).map(|b| b.focus(cx)) {
            window.focus(&f, cx);
        }
    }

    /// Esc in the viewer: the body's Go to / Find bar first. True if it took it.
    pub(super) fn viewer_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.viewer.as_ref().and_then(|v| v.body.as_ref()) {
            Some(FileBody::Hex(v)) => v.clone().update(cx, |v, cx| v.escape(window, cx)),
            Some(FileBody::Text(v)) => v.clone().update(cx, |v, cx| v.escape(window, cx)),
            None => false,
        }
    }

    /// The viewer's extra buttons: Open as Text for any file, and Hex / Text
    /// to switch a large file between the two.
    pub(super) fn viewer_buttons(&self, look: &Look, cx: &mut Context<Self>) -> Option<AnyElement> {
        let v = self.viewer.as_ref()?;
        let path = v.path.clone()?;
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
        let large = std::fs::metadata(&path).is_ok_and(|m| m.len() > self.large_bytes());
        let el = match &v.body {
            Some(FileBody::Text(_)) => button("viewer-hex", "Hex").on_click(cx.listener(|this, _, window, cx| this.switch_body(true, window, cx))).into_any_element(),
            Some(FileBody::Hex(_)) if large => {
                button("viewer-text", "Open as Text").on_click(cx.listener(|this, _, window, cx| this.switch_body(false, window, cx))).into_any_element()
            }
            _ => button("viewer-text", "Open as Text").on_click(cx.listener(move |this, _, window, cx| this.open_as_text_at(&path, window, cx))).into_any_element(),
        };
        Some(el)
    }

    /// A large file in the viewer: hex (`hex`) or paged text.
    fn switch_body(&mut self, hex: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.viewer.as_ref().and_then(|v| v.path.clone()) else { return };
        let body = if hex { self.hex_body(&path, window, cx) } else { self.text_body(&path, window, cx) };
        if let Some(v) = self.viewer.as_mut() {
            v.body = Some(body);
        }
        self.focus_viewer_body(window, cx);
        cx.notify();
    }

    /// Open `path` as text whatever it holds: in the code modal (binary bytes
    /// through Windows-1252, read-only if saving couldn't give them back), or
    /// paged in the viewer when it's over the large-file size.
    pub(super) fn open_as_text_at(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if len > self.large_bytes() {
            if self.viewer.as_ref().and_then(|v| v.path.as_deref()) != Some(path) {
                self.view_file(path, window, cx);
            }
            if !matches!(self.viewer.as_ref().and_then(|v| v.body.as_ref()), Some(FileBody::Text(_))) {
                self.switch_body(false, window, cx);
            }
            return;
        }
        self.close_viewer(window, cx);
        self.open_text_at(path, None, window, cx);
    }

    // -- actions ------------------------------------------------------------------

    /// File > Open File… (`Cmd+O`): any file, in or outside the project.
    /// Dev/test: QUR_DEBUG_OPEN_FILE=<path> skips the panel.
    pub(super) fn open_file(&mut self, _: &qur::OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = std::env::var_os("QUR_DEBUG_OPEN_FILE") {
            return self.open_any(&PathBuf::from(p), window, cx);
        }
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Open".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| this.open_any(&p, window, cx)).ok();
            }
        })
        .detach();
    }

    fn open_any(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let outside = !path.starts_with(&self.root) && self.where_is(path, cx).1.is_none();
        if outside && let Some(t) = modified(path) {
            OUTSIDE.with(|o| o.borrow_mut().insert(path.to_path_buf(), t));
        }
        self.open_path(path, window, cx);
    }

    /// Files from outside the project changed while Qur was in the background:
    /// clean copies reload, dirty ones get the "changed on disk" bar.
    pub(super) fn check_outside_files(&mut self, cx: &mut Context<Self>) {
        let changed: Vec<PathBuf> = OUTSIDE.with(|o| {
            let mut o = o.borrow_mut();
            o.iter_mut()
                .filter_map(|(p, seen)| {
                    let now = modified(p)?;
                    (now != *seen).then(|| {
                        *seen = now;
                        p.clone()
                    })
                })
                .collect()
        });
        for p in changed {
            if let Some(b) = self.buffers.find(&p, cx) {
                b.update(cx, |b, cx| {
                    b.disk_change(cx);
                });
            }
        }
    }

    /// File > Open as Text: the viewer's file (else the code modal's).
    pub(super) fn open_as_text(&mut self, _: &qur::OpenAsText, window: &mut Window, cx: &mut Context<Self>) {
        match self.viewer.as_ref().and_then(|v| v.path.clone()) {
            Some(p) => self.open_as_text_at(&p, window, cx),
            None if self.code.is_some() => self.flash("This file is already open as text", cx),
            None => self.flash("Open a file first (the file viewer's file opens as text)", cx),
        }
    }

    pub(super) fn reopen_with_encoding(&mut self, _: &qur::ReopenWithEncoding, window: &mut Window, cx: &mut Context<Self>) {
        self.encoding_picker(EncodingAction::Reopen, window, cx);
    }

    pub(super) fn convert_to_encoding(&mut self, _: &qur::ConvertToEncoding, window: &mut Window, cx: &mut Context<Self>) {
        self.encoding_picker(EncodingAction::Convert, window, cx);
    }

    fn encoding_picker(&mut self, action: EncodingAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(code) = self.code.clone() else {
            return self.flash("Open a file in the code modal first", cx);
        };
        let current = code.read(cx).model.encoding();
        let items = encoding::CHOICES
            .iter()
            .map(|&(e, b)| {
                let item = PickItem::new(encoding::label(e, b));
                if (e, b) == current { item.right("current") } else { item }
            })
            .collect();
        let title = match action {
            EncodingAction::Reopen => "Reopen with encoding…",
            EncodingAction::Convert => "Convert to encoding…",
        };
        self.open_picker(title, items, PickKind::Encoding(action), false, window, cx);
    }

    /// An encoding was picked (`encoding::CHOICES[i]`).
    pub(super) fn encoding_chosen(&mut self, i: usize, action: EncodingAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&(enc, bom)) = encoding::CHOICES.get(i) else { return };
        let Some(code) = self.code.clone() else { return };
        let label = encoding::label(enc, bom);
        match action {
            EncodingAction::Reopen => {
                if !code.read(cx).is_dirty() {
                    return Self::reopen(&code, enc, bom, cx);
                }
                let answer = ask(window, PromptLevel::Warning, &format!("Reopen as {label}?"), Some("Your unsaved changes to this file are lost."), &["Reopen", "Cancel"], cx);
                cx.spawn(async move |_, cx| {
                    if answer.await.ok() == Some(0) {
                        cx.update(|cx| Self::reopen(&code, enc, bom, cx));
                    }
                })
                .detach();
            }
            EncodingAction::Convert => {
                if code.read(cx).read_only {
                    return self.flash("This file is read-only (it didn't decode cleanly); Reopen with Encoding… first", cx);
                }
                let result = code.update(cx, |b, cx| {
                    let r = b.model.set_encoding(enc, bom);
                    if r == Ok(true) {
                        cx.emit(EditorEvent::Changed);
                        cx.notify();
                    }
                    r
                });
                match result {
                    Ok(true) => self.flash(format!("Converted to {label}; save to keep it"), cx),
                    Ok(false) => self.flash(format!("Already {label}"), cx),
                    Err(c) => self.flash(format!("{label} can't store {c:?} (U+{:04X})", c as u32), cx),
                }
            }
        }
    }

    fn reopen(code: &Entity<EditorView>, enc: Encoding, bom: bool, cx: &mut App) {
        code.update(cx, |b, cx| {
            b.model.force_encoding(Some((enc, bom)));
            b.reload_from_disk(cx);
            b.read_only = b.model.is_lossy();
        });
    }

    /// File > Encoding > Convert Line Endings: LF ⇄ CRLF on save (undoable).
    pub(super) fn convert_line_endings(&mut self, _: &qur::ConvertLineEndings, _: &mut Window, cx: &mut Context<Self>) {
        let Some(code) = self.code.clone() else {
            return self.flash("Open a file in the code modal first", cx);
        };
        if code.read(cx).read_only {
            return self.flash("This file is read-only", cx);
        }
        let crlf = code.update(cx, |b, cx| {
            let crlf = !b.model.is_crlf();
            b.model.set_line_endings(crlf);
            cx.emit(EditorEvent::Changed);
            cx.notify();
            crlf
        });
        self.flash(format!("Line endings: {} (save to keep, ⌘Z undoes)", if crlf { "CRLF" } else { "LF" }), cx);
    }

    /// View > Show Whitespace, saved as `[editor] show_whitespace`.
    pub(super) fn show_whitespace(&mut self, _: &qur::ShowWhitespace, window: &mut Window, cx: &mut Context<Self>) {
        let on = !self.config.editor.show_whitespace;
        self.write_setting(false, "editor.show_whitespace", Some(Value::Bool(on)), cx);
        self.reload_settings(window, cx);
        self.broadcast_reload(cx);
        self.sync_menus(cx);
    }

    /// The code modal header's "UTF-8 · LF", plus "read-only".
    pub(super) fn code_status(c: &EditorView) -> String {
        let (enc, bom) = c.model.encoding();
        let eol = if c.model.is_crlf() { "CRLF" } else { "LF" };
        let ro = if c.read_only { " · read-only" } else { "" };
        format!("{} · {eol}{ro}", encoding::label(enc, bom))
    }

    /// This module's actions, for the workspace's root element.
    pub(super) fn file_actions(d: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        d.on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::open_as_text))
            .on_action(cx.listener(Self::reopen_with_encoding))
            .on_action(cx.listener(Self::convert_to_encoding))
            .on_action(cx.listener(Self::convert_line_endings))
            .on_action(cx.listener(Self::show_whitespace))
    }
}
