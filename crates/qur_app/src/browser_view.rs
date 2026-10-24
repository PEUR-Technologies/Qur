//! Browser view (decision 26): a system WKWebView over the stage, one per
//! agent task, for the task's dev server. Inspect highlights and selects
//! elements (or page text); picks become prompt attachments (context text
//! plus a screenshot) for that task's agent. The web view is created when
//! first shown and freed after a few minutes hidden, so WebKit's processes
//! only run while you use it.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt;
use gpui::{
    Bounds, Context, Entity, EventEmitter, FocusHandle, Pixels, SharedString, Subscription, Task, Window, canvas, div, prelude::*, px,
};
use qur_browser::{Browser, BrowserEvent, EditCommand, Picked};
use qur_theme::Rgba;

use crate::commands::{browser as ba, edit};
use crate::input::{InputEvent, TextInput};
use crate::look::Look;

/// Hidden this long → the web view (and its WebKit processes) is freed.
const IDLE_RELEASE: Duration = Duration::from_secs(300);

/// Something picked in the page, ready for the agent.
#[derive(Clone)]
pub struct Attachment {
    pub label: SharedString,
    /// Text context for the agent (selector, source, HTML, styles, URL).
    pub context: String,
    pub png: Option<Vec<u8>>,
}

pub enum BrowserPaneEvent {
    /// Attach to the agent's prompt.
    Attach(Attachment),
    /// The URL changed (persist it on the task).
    Navigated(String),
}

pub struct BrowserPane {
    /// Focused while you're in the page or its toolbar: the Browser keys
    /// (Cmd+R, Cmd+L, Cmd+[…) and Cmd+C/V/A act on the page.
    pub focus: FocusHandle,
    /// The page was clicked (it has the keyboard): focus this pane at the
    /// next render, which has the window.
    want_focus: bool,
    web: Option<Browser>,
    url: Entity<TextInput>,
    title: String,
    loading: bool,
    error: Option<String>,
    inspecting: bool,
    pub accent: Rgba,
    /// The page's URL to restore (task's last URL).
    pub home: Option<String>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The workspace decides each frame whether this pane is on screen.
    pub shown: bool,
    release: Option<Task<()>>,
    /// Dev/test: QUR_DEBUG_PICK=<css selector> inspects and picks it once loaded.
    debug_pick: Option<String>,
    _events: Option<Task<()>>,
    _sub: Subscription,
}

impl EventEmitter<BrowserPaneEvent> for BrowserPane {}

impl BrowserPane {
    pub fn new(home: Option<String>, accent: Rgba, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let url = cx.new(|cx| TextInput::new("localhost:3000", cx));
        if let Some(h) = &home {
            url.update(cx, |u, cx| u.set_text(h, cx));
        }
        let sub = cx.subscribe_in(&url, window, |this, input, ev: &InputEvent, window, cx| {
            if let InputEvent::Confirm { .. } = ev {
                let u = input.read(cx).text.trim().to_string();
                if !u.is_empty() {
                    this.ensure(window, cx);
                    if let Some(w) = &this.web {
                        w.load(&u);
                        w.focus();
                    }
                    window.focus(&this.focus, cx);
                }
            }
        });
        BrowserPane {
            focus: cx.focus_handle(),
            want_focus: false,
            web: None,
            url,
            title: String::new(),
            loading: false,
            error: None,
            inspecting: false,
            accent,
            home,
            bounds: Rc::new(Cell::new(None)),
            shown: false,
            release: None,
            debug_pick: std::env::var("QUR_DEBUG_PICK").ok(),
            _events: None,
            _sub: sub,
        }
    }

    /// Create the web view on first use (and after an idle release).
    fn ensure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.web.is_some() {
            return;
        }
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = window.window_handle() else { return };
        let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
        // SAFETY: GPUI's window outlives this pane; the view pointer is valid on the main thread.
        let (web, mut events) = unsafe { Browser::new(h.ns_view.as_ptr()) };
        if let Some(home) = self.home.clone() {
            web.load(&home);
        }
        self.web = Some(web);
        self._events = Some(cx.spawn(async move |this, cx| {
            while let Some(ev) = events.next().await {
                if this.update(cx, |this, cx| this.on_event(ev, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn on_event(&mut self, ev: BrowserEvent, cx: &mut Context<Self>) {
        match ev {
            BrowserEvent::Navigated { url, title } => {
                if std::env::var_os("QUR_DEBUG_ACTIONS").is_some() {
                    println!("BROWSER navigated {url}");
                }
                self.title = title;
                self.error = None;
                self.url.update(cx, |u, cx| {
                    if u.text != url {
                        u.set_text(&url, cx)
                    }
                });
                self.home = Some(url.clone());
                cx.emit(BrowserPaneEvent::Navigated(url));
                if let Some(sel) = self.debug_pick.take() {
                    self.inspecting = true;
                    if let Some(w) = &self.web {
                        w.set_inspect(true, &css_hex(self.accent));
                        let js = format!(
                            "setTimeout(()=>{{const el=document.querySelector({sel:?});if(!el)return;const r=el.getBoundingClientRect();const o={{bubbles:true,cancelable:true,clientX:r.x+r.width/2,clientY:r.y+r.height/2}};el.dispatchEvent(new MouseEvent('mousemove',o));el.dispatchEvent(new MouseEvent('click',o));}},600)"
                        );
                        w.evaluate_js(&js);
                    }
                }
            }
            BrowserEvent::Loading(on) => self.loading = on,
            BrowserEvent::Error(e) => {
                self.loading = false;
                self.error = Some(e);
            }
            BrowserEvent::InspectEnded => self.inspecting = false,
            BrowserEvent::Focused => {
                self.want_focus = true;
                cx.notify();
            }
            BrowserEvent::Text { text, url } => {
                let label: SharedString = format!("“{}”", truncate(&text, 40)).into();
                let context = format!("Text the user selected on {url}:\n\n> {}\n", text.trim().replace('\n', "\n> "));
                cx.emit(BrowserPaneEvent::Attach(Attachment { label, context, png: None }));
            }
            BrowserEvent::Picked(p) => self.picked(p, cx),
        }
        cx.notify();
    }

    fn picked(&mut self, p: Picked, cx: &mut Context<Self>) {
        let label: SharedString = element_label(&p).into();
        let context = element_context(&p);
        let Some(web) = &self.web else { return };
        let this = cx.weak_entity();
        let mut async_cx = cx.to_async();
        web.snapshot(Some(p.rect), move |png| {
            let _ = this.update(&mut async_cx, |_, cx| {
                cx.emit(BrowserPaneEvent::Attach(Attachment { label, context, png }));
            });
        });
    }

    /// Go to `url` now if the page is loaded, else when it's next shown
    /// (Run Project).
    pub fn navigate(&mut self, url: &str, cx: &mut Context<Self>) {
        self.home = Some(url.to_string());
        self.url.update(cx, |u, cx| u.set_text(url, cx));
        if let Some(w) = &self.web {
            w.load(url);
        }
        cx.notify();
    }

    /// Cmd+C, Cmd+V… in the page.
    fn page_edit(&self, cmd: EditCommand) {
        if let Some(w) = &self.web {
            w.edit(cmd);
        }
    }

    /// Take the keyboard back from the page (see `Browser::release_focus`).
    pub fn release_focus(&self) {
        if let Some(w) = &self.web {
            w.release_focus();
        }
    }

    /// Reload the page if it's loaded (the agent changed files).
    pub fn reload(&mut self) {
        if let Some(w) = &self.web {
            w.reload();
        }
    }

    pub fn toggle_inspect(&mut self, cx: &mut Context<Self>) {
        self.inspecting = !self.inspecting;
        if let Some(w) = &self.web {
            w.set_inspect(self.inspecting, &css_hex(self.accent));
        }
        cx.notify();
    }

    /// Called by the workspace every frame (before paint) with whether this
    /// pane is actually on screen and not under a Qur overlay.
    pub fn set_shown(&mut self, shown: bool, window: &mut Window, cx: &mut Context<Self>) {
        if shown == self.shown {
            return;
        }
        self.shown = shown;
        if shown {
            self.release = None;
            self.ensure(window, cx);
        } else {
            if let Some(w) = &self.web {
                w.set_visible(false);
            }
            // Free WebKit's processes after a while out of sight.
            let bg = cx.background_executor().clone();
            self.release = Some(cx.spawn(async move |this, cx| {
                bg.timer(IDLE_RELEASE).await;
                let _ = this.update(cx, |this, cx| {
                    if !this.shown {
                        this.web = None;
                        this._events = None;
                        this.inspecting = false;
                        cx.notify();
                    }
                });
            }));
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    let t: String = s.trim().chars().take(n).collect();
    if s.trim().chars().count() > n { format!("{t}…") } else { t }
}

fn css_hex(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

fn element_label(p: &Picked) -> String {
    let mut s = p.tag.to_lowercase();
    if let Some(id) = &p.id {
        s.push('#');
        s.push_str(id);
    } else if let Some(c) = p.classes.first() {
        s.push('.');
        s.push_str(c);
    }
    if !p.text.trim().is_empty() {
        s.push_str(&format!(" “{}”", truncate(&p.text, 24)));
    }
    s
}

/// What the agent gets for a picked element (plus the screenshot).
pub fn element_context(p: &Picked) -> String {
    let mut s = format!("The user selected this element in the browser (page {}).\n", p.url);
    s.push_str(&format!("- selector: `{}`\n", p.selector));
    if let Some(src) = &p.source {
        s.push_str(&format!("- source: `{src}`\n"));
    }
    s.push_str(&format!("- size: {:.0}×{:.0} px at ({:.0}, {:.0})\n", p.rect[2], p.rect[3], p.rect[0], p.rect[1]));
    if !p.text.trim().is_empty() {
        s.push_str(&format!("- text: {}\n", truncate(&p.text, 200)));
    }
    if !p.styles.is_empty() {
        s.push_str("- computed styles: ");
        s.push_str(&p.styles.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("; "));
        s.push('\n');
    }
    s.push_str(&format!("\n```html\n{}\n```\n", p.outer_html));
    s
}

impl Render for BrowserPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let button = |label: &'static str, id: &'static str| {
            div()
                .id(id)
                .flex_none()
                .px(look.z(6.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_size(look.text(-1.))
                .text_color(look.c(ui.text_secondary))
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .child(label)
        };
        let bar = div()
            .flex_none()
            .h(look.z(32.))
            .px(look.z(8.))
            .flex()
            .items_center()
            .gap(look.z(4.))
            .border_b_1()
            .border_color(look.c(ui.border))
            .child(button("‹", "br-back").on_click(cx.listener(|t, _, _, _| {
                if let Some(w) = &t.web {
                    w.go_back()
                }
            })))
            .child(button("›", "br-fwd").on_click(cx.listener(|t, _, _, _| {
                if let Some(w) = &t.web {
                    w.go_forward()
                }
            })))
            .child(button(if self.loading { "✕" } else { "↻" }, "br-reload").on_click(cx.listener(|t, _, _, _| {
                if let Some(w) = &t.web {
                    if t.loading { w.stop() } else { w.reload() }
                }
            })))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .px(look.z(8.))
                    .rounded(look.z(4.))
                    .border_1()
                    .border_color(look.c(ui.border))
                    .bg(look.c(ui.editor_bg))
                    .child(self.url.clone()),
            )
            .child(
                div()
                    .id("br-inspect")
                    .flex_none()
                    .px(look.z(8.))
                    .py(look.z(1.))
                    .rounded(look.z(4.))
                    .cursor_pointer()
                    .text_size(look.text(-1.))
                    .border_1()
                    .border_color(look.c(if self.inspecting { self.accent } else { ui.border_strong }))
                    .text_color(look.c(if self.inspecting { self.accent } else { ui.text_secondary }))
                    .child(if self.inspecting { "Inspecting · esc" } else { "Inspect" })
                    .on_click(cx.listener(|t, _, _, cx| t.toggle_inspect(cx))),
            );
        let bounds = self.bounds.clone();
        let web_shown = self.shown && self.web.is_some();
        let body = div()
            .flex_1()
            .min_h(px(0.))
            .relative()
            .child(
                // The native web view sits exactly over this box.
                canvas(
                    move |b, _, _| bounds.set(Some(b)),
                    {
                        let me = cx.entity();
                        move |b, _, _, cx| {
                            me.update(cx, |t, _| {
                                if let Some(w) = &t.web
                                    && t.shown
                                {
                                    let (x, y, wd, ht) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
                                    w.set_frame(x as f64, y as f64, wd as f64, ht as f64);
                                    w.set_visible(true);
                                }
                            });
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .when(!web_shown || self.error.is_some(), |d| {
                d.child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(look.z(6.))
                        .text_color(look.c(ui.text_muted))
                        .child(match (&self.error, &self.home) {
                            (Some(e), _) => e.clone(),
                            (None, None) => "Type your dev server's address above (e.g. localhost:3000), then Inspect to point at things.".into(),
                            (None, Some(_)) => "Loading…".into(),
                        }),
                )
            });
        if std::mem::take(&mut self.want_focus) {
            window.focus(&self.focus, cx);
        }
        div()
            .key_context("Browser")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(look.surface(ui.editor_bg))
            // The address bar handles these itself; here they go to the page.
            .on_action(cx.listener(|t, _: &edit::Copy, _, _| t.page_edit(EditCommand::Copy)))
            .on_action(cx.listener(|t, _: &edit::Cut, _, _| t.page_edit(EditCommand::Cut)))
            .on_action(cx.listener(|t, _: &edit::Paste, _, _| t.page_edit(EditCommand::Paste)))
            .on_action(cx.listener(|t, _: &edit::SelectAll, _, _| t.page_edit(EditCommand::SelectAll)))
            .on_action(cx.listener(|t, _: &edit::Undo, _, _| t.page_edit(EditCommand::Undo)))
            .on_action(cx.listener(|t, _: &edit::Redo, _, _| t.page_edit(EditCommand::Redo)))
            .on_action(cx.listener(|t, _: &ba::Reload, _, _| t.reload()))
            .on_action(cx.listener(|t, _: &ba::Back, _, _| t.web.iter().for_each(|w| w.go_back())))
            .on_action(cx.listener(|t, _: &ba::Forward, _, _| t.web.iter().for_each(|w| w.go_forward())))
            .on_action(cx.listener(|t, _: &ba::ZoomIn, _, _| t.web.iter().for_each(|w| w.zoom(1))))
            .on_action(cx.listener(|t, _: &ba::ZoomOut, _, _| t.web.iter().for_each(|w| w.zoom(-1))))
            .on_action(cx.listener(|t, _: &ba::ZoomReset, _, _| t.web.iter().for_each(|w| w.zoom(0))))
            .on_action(cx.listener(|t, _: &ba::FocusAddress, window, cx| {
                // Out of the page, into the address bar with its text selected.
                t.release_focus();
                let f = t.url.read(cx).focus.clone();
                window.focus(&f, cx);
                t.url.update(cx, |u, cx| u.select_text(cx));
            }))
            .child(bar)
            .child(body)
    }
}

impl Drop for BrowserPane {
    fn drop(&mut self) {
        // Browser's own Drop removes the native view.
        self.web = None;
    }
}

/// Small base64 encoder (screenshots in prompts; no extra crate).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_reference() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }
}
