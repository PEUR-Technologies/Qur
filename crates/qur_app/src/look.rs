//! Per-window look: theme, zoom, transparency and fonts. Each window can show a
//! different project theme (decision 13), so views read the look of the window
//! they render in: `Look::of(window, cx)`. UI code takes every color from the
//! theme's tokens (handoff 12a), never hard-coded values.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, Global, Hsla, Pixels, SharedString, Window, WindowBackgroundAppearance, WindowId, px};
use qur_theme::{Rgba, Theme};

#[derive(Clone)]
pub struct Look {
    pub theme: Arc<Theme>,
    pub zoom: f32,
    /// 1.0 = opaque; only large background fills use it (decision 8).
    pub opacity: f32,
    pub blur: bool,
    pub ui_font: SharedString,
    pub code_font: SharedString,
    pub ui_size: f32,
    pub code_size: f32,
    pub line_height: f32,
}

#[derive(Default)]
pub struct Looks(pub HashMap<WindowId, Look>);

impl Global for Looks {}

impl Look {
    pub fn of(window: &Window, cx: &App) -> Look {
        let id = window.window_handle().window_id();
        cx.try_global::<Looks>()
            .and_then(|l| l.0.get(&id).cloned())
            .unwrap_or_else(|| Look::fallback(cx))
    }

    pub fn set(window: &Window, look: Look, cx: &mut App) {
        let id = window.window_handle().window_id();
        cx.default_global::<Looks>().0.insert(id, look);
    }

    pub fn remove(id: WindowId, cx: &mut App) {
        cx.default_global::<Looks>().0.remove(&id);
    }

    fn fallback(_: &App) -> Look {
        Look {
            theme: qur_theme::Registry::global().resolve(&qur_theme::ThemeChoice::Auto, true),
            zoom: 1.0,
            opacity: 1.0,
            blur: false,
            ui_font: crate::assets::UI_FONT.into(),
            code_font: "Menlo".into(),
            ui_size: 13.0,
            code_size: 13.0,
            line_height: 1.5,
        }
    }

    /// A UI length scaled by zoom.
    pub fn z(&self, v: f32) -> Pixels {
        px(v * self.zoom)
    }

    /// UI text size `delta` points from the base size, zoomed.
    pub fn text(&self, delta: f32) -> Pixels {
        px((self.ui_size + delta) * self.zoom)
    }

    pub fn code_px(&self) -> Pixels {
        px(self.code_size * self.zoom)
    }

    pub fn code_line_h(&self) -> Pixels {
        px((self.code_size * self.line_height * self.zoom).round())
    }

    /// Any themed color.
    pub fn c(&self, c: Rgba) -> Hsla {
        hsla(c)
    }

    /// A large background fill: honours window opacity.
    /// A region's background (top bar, sidebar, whatever fills the stage):
    /// translucent with the window (decision 8).
    pub fn bg(&self, c: Rgba) -> Hsla {
        let mut h = hsla(c);
        h.a *= self.opacity;
        h
    }

    /// A surface inside a region (Live, Changes, an editor…). While the window
    /// is translucent it paints nothing: two translucent layers on one pixel
    /// would let less of the desktop through than the slider says.
    pub fn surface(&self, c: Rgba) -> Hsla {
        if self.opacity < 1.0 { gpui::transparent_black() } else { hsla(c) }
    }

    pub fn background_appearance(&self) -> WindowBackgroundAppearance {
        if self.opacity >= 1.0 {
            WindowBackgroundAppearance::Opaque
        } else if self.blur {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Transparent
        }
    }

    /// The flat symbol for this background.
    pub fn logo_white(&self) -> bool {
        self.theme.logo_is_white()
    }
}

pub fn hsla(c: Rgba) -> Hsla {
    gpui::Rgba {
        r: c.r() as f32 / 255.0,
        g: c.g() as f32 / 255.0,
        b: c.b() as f32 / 255.0,
        a: c.a() as f32 / 255.0,
    }
    .into()
}
