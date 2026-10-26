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
    /// Window gaps (`[ui] gaps`): every region floats as its own tile.
    pub tiles: Option<Tiles>,
    /// `[editor] show_whitespace`: the code modal draws spaces, tabs and line ends.
    pub show_whitespace: bool,
}

/// Hyprland-style window gaps: the window paints nothing between regions,
/// and each region (top bar, sidebar, dock pane, terminal pane) is a tile
/// with its own background. Lengths are in points before zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tiles {
    pub gap: f32,
    /// 0 = square corners.
    pub radius: f32,
    pub border: bool,
}

impl Tiles {
    pub fn from_config(ui: &qur_config::UiConfig) -> Option<Tiles> {
        ui.gaps.then_some(Tiles { gap: ui.gap, radius: if ui.rounded { ui.radius } else { 0.0 }, border: ui.tile_border })
    }

    /// How far a tile's content stays in from its edges so the rounded
    /// corners aren't painted over (GPUI clips children to rectangles): the
    /// corner arc is farthest from the edges on the diagonal, r(1 - 1/√2).
    pub fn inset(&self) -> f32 {
        (self.radius * (1.0 - std::f32::consts::FRAC_1_SQRT_2)).ceil()
    }
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
            tiles: None,
            show_whitespace: false,
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

    /// The space between tiles (zoomed); 0 without gaps.
    pub fn gap(&self) -> Pixels {
        self.tiles.map(|t| self.z(t.gap)).unwrap_or(px(0.))
    }

    /// A region's background where, with gaps on, an enclosing tile already
    /// paints it (the one-translucent-layer rule): nothing then.
    pub fn region_bg(&self, c: Rgba) -> Hsla {
        if self.tiles.is_some() { gpui::transparent_black() } else { self.bg(c) }
    }

    /// The window root's fill. With gaps on it is all but invisible: alpha
    /// 1/255 keeps the gaps clickable (AppKit lets clicks on fully clear
    /// pixels of a non-opaque window fall through to the window behind),
    /// so dividers in the gaps drag and the window stays focusable.
    pub fn root_bg(&self, c: Rgba) -> Hsla {
        if self.tiles.is_some() {
            let mut h = hsla(c);
            h.a = 1.0 / 255.0;
            h
        } else {
            self.surface(c)
        }
    }

    /// Make `el` a tile: its own translucent background, rounded corners,
    /// an optional hairline, and its content inset from the corners. Without
    /// gaps it is just the region background.
    pub fn tile<E: gpui::Styled>(&self, el: E, c: Rgba) -> E {
        let el = el.bg(self.bg(c));
        let Some(t) = self.tiles else { return el };
        let el = el.rounded(self.z(t.radius)).p(self.z(t.inset()));
        if t.border { el.border_1().border_color(self.c(self.theme.ui.border)) } else { el }
    }

    pub fn background_appearance(&self) -> WindowBackgroundAppearance {
        // With gaps on the window is never opaque: the gaps show the desktop
        // even at 100%. Blur is window-wide in macOS, so it follows the
        // Transparency popover: below 100% with blur on, the gaps are
        // blurred too; at 100% (or blur off) they're crisp.
        if self.tiles.is_some() && self.opacity >= 1.0 {
            WindowBackgroundAppearance::Transparent
        } else if self.opacity >= 1.0 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_follow_config_and_inset_clears_corners() {
        let mut ui = qur_config::UiConfig::default();
        assert_eq!(Tiles::from_config(&ui), None, "gaps are off by default");
        ui.gaps = true;
        let t = Tiles::from_config(&ui).unwrap();
        assert_eq!((t.gap, t.radius, t.border), (8.0, 8.0, false));
        // The corner arc's deepest point is r(1 - 1/√2) ≈ 2.34 in from both edges.
        assert_eq!(t.inset(), 3.0);
        ui.rounded = false;
        let t = Tiles::from_config(&ui).unwrap();
        assert_eq!((t.radius, t.inset()), (0.0, 0.0));
        for r in [1.0f32, 4.0, 12.0, 16.0] {
            let t = Tiles { gap: 8.0, radius: r, border: false };
            let d = t.inset();
            // A child's corner at (d, d) is inside the arc centred at (r, r).
            assert!(((r - d).powi(2) * 2.0).sqrt() <= r + 1e-3, "r={r} d={d}");
        }
    }
}
