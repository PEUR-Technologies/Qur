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
        self.flash(if on { "Zen mode on (move the mouse to the top edge for the bar)" } else { "Zen mode off" }, cx);
    }

    /// Zen mode: slide the bar down at the top edge, and away once the mouse
    /// leaves it (not while dragging the window by it).
    pub(super) fn zen_track(&mut self, y: Pixels, window: &Window, cx: &mut Context<Self>) {
        if !self.config.ui.zen {
            return;
        }
        let look = Look::of(window, cx);
        let (zoom, gap) = (look.zoom, look.gap());
        let show = if self.zen_shown {
            self.title_drag || y <= px((TITLEBAR_H + ZEN_SLACK) * zoom) + gap
        } else {
            y <= px(ZEN_EDGE * zoom) + gap
        };
        if show != self.zen_shown {
            self.zen_shown = show;
            cx.notify();
        }
    }

    // -- render pieces -------------------------------------------------------------------

    /// Where title bar content starts: past the traffic lights, or near the edge in full screen.
    pub(super) fn title_lead(&self, look: &Look) -> Pixels {
        if self.fullscreen { look.z(8.) } else { px(TRAFFIC_LIGHTS_W) }
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
            .h(look.z(TITLEBAR_H))
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
            .when(self.fullscreen, |d| d.child(self.traffic_lights(look, cx)))
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
            .children(message.map(|m| div().flex_none().max_w(look.z(420.)).truncate().text_size(look.text(-1.)).text_color(look.c(ui.text_secondary)).child(m)))
    }

    /// Full screen: close, minimize and zoom in the bar itself, in line with the
    /// project name, instead of the system's strip that slides down above it.
    /// Minimize is off in full screen, as in AppKit; zoom leaves full screen.
    /// `[ui] traffic_lights` may draw them in the theme's grey instead of red,
    /// yellow and green: "auto" does in dark themes, "mono" always.
    fn traffic_lights(&self, look: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let mono = match self.config.ui.traffic_lights.as_str() {
            "mono" => true,
            "color" => false,
            _ => look.theme.appearance == Appearance::Dark,
        };
        let grey = look.c(look.theme.ui.text_muted);
        let light = |id: &'static str, color: u32| {
            div()
                .id(id)
                .size(look.z(12.))
                .rounded_full()
                .map(|d| if mono { d.bg(grey) } else { d.bg(gpui::rgb(color)) })
                .border_1()
                .border_color(gpui::rgba(0x0000001f))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(look.z(8.))
            .pr(look.z(6.))
            .child(
                light("light-close", 0xff5f57)
                    .hover(|d| d.opacity(0.8))
                    .on_click(cx.listener(|this, _, window, cx| this.close_window(&qur::CloseWindow, window, cx))),
            )
            .child(light("light-minimize", 0xfebc2e).opacity(0.35))
            .child(
                light("light-zoom", 0x28c840)
                    .hover(|d| d.opacity(0.8))
                    .on_click(|_, window, _| window.toggle_fullscreen()),
            )
    }

    /// The title bar, or in zen mode the bar sliding in and out from above,
    /// over the window's content rather than pushing it down (which reflowed
    /// everything, terminals included, on every frame of the slide).
    pub(super) fn top(&self, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let bar = self.title_bar(look, cx);
        if !self.config.ui.zen {
            return bar.into_any_element();
        }
        let full = f32::from(look.z(TITLEBAR_H));
        let shown = self.zen_shown;
        // With window gaps it slides in to a gap below the window's edge.
        let gap = f32::from(look.gap());
        div()
            .absolute()
            .left(px(gap))
            .right(px(gap))
            .child(bar)
            .with_animation(
                if shown { "zen-in" } else { "zen-out" },
                Animation::new(ZEN_SLIDE).with_easing(gpui::ease_out_quint()),
                move |d, t| d.top(px(gap - (full + gap) * if shown { 1. - t } else { t })),
            )
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
