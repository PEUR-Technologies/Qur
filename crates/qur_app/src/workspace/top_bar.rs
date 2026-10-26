//! The top bar, traffic lights and zen mode.

use super::*;

impl Workspace {
    /// View > Zen Mode, saved as `[ui] zen`: the top bar and traffic lights
    /// hide and slide back down while the mouse is at the top edge.
    pub(super) fn toggle_zen(&mut self, _: &qur::ZenMode, window: &mut Window, cx: &mut Context<Self>) {
        let on = !self.config.ui.zen;
        self.write_setting(false, "ui.zen", Some(Value::Bool(on)), cx);
        self.reload_settings(window, cx);
        self.broadcast_reload(cx);
        self.sync_menus(cx);
        self.zen_shown = false;
        self.zen_since = Instant::now().checked_sub(ZEN_SLIDE).unwrap_or_else(Instant::now);
        self.flash(if on { "Zen mode on (move the mouse to the top edge for the bar)" } else { "Zen mode off" }, cx);
    }

    /// Zen mode: slide the bar down at the top edge, and away once the mouse
    /// leaves it (not while dragging the window by it).
    pub(super) fn zen_track(&mut self, y: Pixels, window: &Window, cx: &mut Context<Self>) {
        // Not while a tab or pane is dragged: the bar sliding in moved the
        // panes under the pointer as it neared their top edges.
        if !self.config.ui.zen || cx.has_active_drag() {
            return;
        }
        let look = Look::of(window, cx);
        let (zoom, gap) = (look.zoom, look.gap());
        let show = if self.zen_shown {
            self.title_drag || y <= self.bar_h(&look) + px(ZEN_SLACK * zoom) + gap
        } else {
            y <= px(ZEN_EDGE * zoom) + gap
        };
        if show != self.zen_shown {
            // Turning around mid-slide carries on from where the bar is.
            let t = (self.zen_since.elapsed().as_secs_f32() / ZEN_SLIDE.as_secs_f32()).min(1.);
            self.zen_since = Instant::now().checked_sub(ZEN_SLIDE.mul_f32(1. - t)).unwrap_or_else(Instant::now);
            self.zen_shown = show;
            cx.notify();
        }
    }

    /// Zen mode: how far down the bar is, 0 (away) to 1 (down), eased over
    /// `ZEN_SLIDE`; asks for the next frame while it moves. The easing is
    /// symmetric, so turning around mid-slide doesn't jump.
    pub(super) fn zen_progress(&self, cx: &mut Context<Self>) -> f32 {
        let t = (self.zen_since.elapsed().as_secs_f32() / ZEN_SLIDE.as_secs_f32()).min(1.);
        if t < 1. {
            crate::tiling::next_frame(cx);
        }
        let e = gpui::ease_in_out(t);
        if self.zen_shown { e } else { 1. - e }
    }

    // -- render pieces -------------------------------------------------------------------

    /// Where title bar content starts: in from the edge, the traffic lights first.
    pub(super) fn title_lead(&self, look: &Look) -> Pixels {
        look.z(12.)
    }

    /// The bar's height: it zooms with the rest, but never below what the
    /// system's traffic lights need (they don't zoom; zoomed out they hung
    /// below it). Full screen's lights are Qur's own and zoom with it.
    pub(super) fn bar_h(&self, look: &Look) -> Pixels {
        if self.fullscreen { look.z(TITLEBAR_H) } else { look.z(TITLEBAR_H).max(px(TITLEBAR_H)) }
    }

    fn title_bar(&self, look: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = &look.theme.ui;
        let message = self.message.as_ref().map(|(m, _)| m.clone());
        // With window gaps the bar is a tile like the rest; else a region
        // with a hairline below.
        let bar = div();
        let bar = if look.tiles.is_some() { look.tile(bar, ui.top_bar_bg) } else { bar.bg(look.bg(ui.top_bar_bg)).border_b_1().border_color(look.c(ui.border)) };
        bar.id("titlebar")
            .window_control_area(gpui::WindowControlArea::Drag)
            .h(self.bar_h(look))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(look.z(8.))
            .pl(self.title_lead(look))
            .pr(look.z(12.))
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
            .child(self.traffic_lights(look, cx))
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
            .child(div().flex_1())
            .children(message.map(|m| {
                let url = self.message_url.clone();
                div()
                    .id("flash")
                    .flex_none()
                    .max_w(look.z(420.))
                    .truncate()
                    .text_size(look.text(-1.))
                    .text_color(look.c(ui.text_secondary))
                    .child(m)
                    .when_some(url, |d, url| {
                        d.cursor_pointer()
                            .underline()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    })
            }))
    }

    /// Close, minimize and zoom, drawn by Qur in the bar itself (decision 61):
    /// the system's stay pinned near the window's corner, so with window
    /// gaps, a zoomed bar or zen's sliding one they fell out of line with it.
    /// Plain circles (no symbols on hover), grey while the window isn't
    /// active; minimize is off in full screen (zoom leaves it).
    /// In full screen `[ui] traffic_lights` may draw them in the theme's grey:
    /// "auto" does in dark themes; "mono" always does.
    fn traffic_lights(&self, look: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let mono = match self.config.ui.traffic_lights.as_str() {
            "mono" => true,
            "color" => false,
            _ => self.fullscreen && look.theme.appearance == Appearance::Dark,
        };
        let grey = !self.window_active || mono;
        let size = look.z(12.).max(px(12.));
        let light = |id: &'static str, color: u32| {
            div()
                .id(id)
                .size(size)
                .rounded_full()
                .map(|d| if grey { d.bg(look.c(look.theme.ui.border_strong)) } else { d.bg(gpui::rgb(color)) })
                .border_1()
                .border_color(gpui::rgba(0x0000001f))
                .hover(|d| d.opacity(0.8))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        let fullscreen = self.fullscreen;
        let gaps = self.config.ui.gaps;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(look.z(8.).max(px(8.)))
            .pr(look.z(10.))
            .child(light("light-close", 0xff5f57).on_click(cx.listener(|this, _, window, cx| this.close_window(&qur::CloseWindow, window, cx))))
            .child(
                light("light-minimize", 0xfebc2e)
                    .when(fullscreen, |d| d.opacity(0.35))
                    .when(!fullscreen, |d| d.on_click(|_, window, _| window.minimize_window())),
            )
            // With gaps, green fills the screen on this desktop (the gaps stay
            // over the wallpaper); Option-click, or out of full screen, toggles it.
            .child(light("light-zoom", 0x28c840).on_click(move |ev, window, _| {
                if gaps && !fullscreen && !ev.modifiers().alt { window.zoom_window() } else { window.toggle_fullscreen() }
            }))
    }

    /// The title bar, or in zen mode the bar sliding in from above (or away)
    /// and pushing the content down as it goes, `progress` of the way down.
    pub(super) fn top(&self, look: &Look, progress: f32, cx: &mut Context<Self>) -> AnyElement {
        let bar = self.title_bar(look, cx);
        if !self.config.ui.zen {
            return bar.into_any_element();
        }
        // The space for the bar (and with gaps, the gap below it) grows with
        // the slide, so the content eases down with the bar riding its
        // bottom edge; the bar itself keeps its height throughout.
        let space = f32::from(self.bar_h(look) + look.gap());
        let h = space * progress;
        div()
            .flex_none()
            .w_full()
            .h(px(h))
            .relative()
            .overflow_hidden()
            .child(div().absolute().left_0().right_0().top(px(h - space)).child(bar))
            .into_any_element()
    }
}

/// Hide or show the close, minimize and zoom buttons (zen mode, full screen). GPUI only
/// moves them, so this asks AppKit directly.
#[cfg(target_os = "macos")]
pub(super) fn set_traffic_lights_hidden(window: &Window, hidden: bool) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    // SAFETY: on the main thread, with GPUI's live content view; `window` and
    // `standardWindowButton:` may return nil, which is checked.
    unsafe {
        let view = h.ns_view.as_ptr().cast::<AnyObject>();
        let ns_window: *mut AnyObject = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }
        // NSWindowCloseButton, NSWindowMiniaturizeButton, NSWindowZoomButton.
        for kind in 0usize..3 {
            let button: *mut AnyObject = msg_send![ns_window, standardWindowButton: kind];
            if !button.is_null() {
                let _: () = msg_send![button, setHidden: hidden];
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn set_traffic_lights_hidden(_: &Window, _: bool) {}

/// Full screen: hide the system's title bar strip, which otherwise drops down
/// with the menu bar as an empty grey band over Qur's own bar and its lights.
/// The strip is the buttons' container view (NSTitlebarContainerView).
#[cfg(target_os = "macos")]
pub(super) fn set_titlebar_hidden(window: &Window, hidden: bool) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    // SAFETY: on the main thread, with GPUI's live content view; every
    // object along the way may be nil, which is checked.
    unsafe {
        let view = h.ns_view.as_ptr().cast::<AnyObject>();
        let ns_window: *mut AnyObject = msg_send![view, window];
        if ns_window.is_null() {
            return;
        }
        let button: *mut AnyObject = msg_send![ns_window, standardWindowButton: 0usize];
        if button.is_null() {
            return;
        }
        let titlebar: *mut AnyObject = msg_send![button, superview];
        if titlebar.is_null() {
            return;
        }
        let container: *mut AnyObject = msg_send![titlebar, superview];
        if !container.is_null() {
            let _: () = msg_send![container, setHidden: hidden];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn set_titlebar_hidden(_: &Window, _: bool) {}

/// The window's shadow, off with window gaps: macOS casts it under the whole
/// window, gaps included (they paint alpha 1/255), darkening them.
#[cfg(target_os = "macos")]
pub(super) fn set_shadow(window: &Window, on: bool) {
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Bool};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return };
    // SAFETY: on the main thread, with GPUI's live content view; `window`
    // may return nil, which is checked.
    unsafe {
        let view = h.ns_view.as_ptr().cast::<AnyObject>();
        let ns_window: *mut AnyObject = msg_send![view, window];
        if !ns_window.is_null() {
            let _: () = msg_send![ns_window, setHasShadow: Bool::new(on)];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn set_shadow(_: &Window, _: bool) {}
