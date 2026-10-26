//! A monospace grid painted from a view that hands over only the rows on
//! screen each frame: the hex view and large files, which read those rows
//! from disk and never hold the whole file.

use gpui::{
    App, Bounds, ContentMask, Context, ElementId, Entity, GlobalElementId, Hsla, LayoutId, Pixels, SharedString, ShapedLine,
    Style, TextRun, Window, fill, font, point, prelude::*, px, relative, size,
};

use crate::look::Look;

/// One row: an optional gutter label (offset, line number) and its text in
/// styled runs (byte lengths of `text`).
pub struct GridRow {
    pub gutter: Option<SharedString>,
    pub text: String,
    pub runs: Vec<GridRun>,
    /// The whole row's background (a selected line).
    pub bg: Option<Hsla>,
}

#[derive(Clone, Copy)]
pub struct GridRun {
    pub len: usize,
    pub color: Hsla,
    pub bg: Option<Hsla>,
}

impl GridRow {
    pub fn new(gutter: Option<SharedString>) -> Self {
        GridRow { gutter, text: String::new(), runs: Vec::new(), bg: None }
    }

    /// Append `s` in one color (merging with the previous run when it matches).
    pub fn push(&mut self, s: &str, color: Hsla, bg: Option<Hsla>) {
        if s.is_empty() {
            return;
        }
        self.text.push_str(s);
        match self.runs.last_mut() {
            Some(r) if r.color == color && r.bg == bg => r.len += s.len(),
            _ => self.runs.push(GridRun { len: s.len(), color, bg }),
        }
    }
}

/// The grid's geometry from the last prepaint, for hit-testing clicks.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub bounds: Bounds<Pixels>,
    pub cell_w: Pixels,
    pub line_h: Pixels,
    /// Left edge of the text, from the bounds' left.
    pub text_x: Pixels,
    /// Rows that fit (the last one may be cut off).
    pub rows: usize,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics { bounds: Bounds::default(), cell_w: px(8.), line_h: px(20.), text_x: px(0.), rows: 30 }
    }
}

impl Metrics {
    /// (row, column) under a window position, unclamped columns.
    pub fn cell_at(&self, p: gpui::Point<Pixels>) -> (usize, usize) {
        let row = ((p.y - self.bounds.top()) / self.line_h).floor().max(0.) as usize;
        let col = ((p.x - self.bounds.left() - self.text_x) / self.cell_w).floor().max(0.) as usize;
        (row, col)
    }

    /// Whole rows that fit.
    pub fn page(&self) -> usize {
        ((self.bounds.size.height / self.line_h).floor() as usize).max(1)
    }
}

pub trait GridSource: 'static + Sized {
    /// Gutter width in cells (0 = none).
    fn gutter_cols(&self) -> usize;
    /// The rows to draw from the top, given this frame's geometry.
    fn grid_rows(&mut self, m: Metrics, look: &Look, cx: &mut Context<Self>) -> Vec<GridRow>;
    /// (top, size) of the visible part as fractions of the whole, for a thin
    /// position bar on the right.
    fn scroll_thumb(&self) -> Option<(f32, f32)>;
}

pub struct Grid<T: GridSource> {
    pub view: Entity<T>,
}

pub struct Shaped {
    rows: Vec<(Option<ShapedLine>, ShapedLine, Option<Hsla>)>,
    text_x: Pixels,
    line_h: Pixels,
}

impl<T: GridSource> IntoElement for Grid<T> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl<T: GridSource> Element for Grid<T> {
    type RequestLayoutState = ();
    type PrepaintState = Shaped;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Shaped {
        let look = Look::of(window, cx);
        let fs = look.code_px();
        let line_h = look.code_line_h();
        let code_font = font(look.code_font.clone());
        let ts = window.text_system().clone();
        let cell_w = ts.advance(ts.resolve_font(&code_font), fs, 'm').map(|s| s.width).unwrap_or(px(8.));
        let gutter = self.view.read(cx).gutter_cols();
        let text_x = if gutter > 0 { look.z(8.) + cell_w * gutter as f32 + look.z(14.) } else { look.z(10.) };
        let m = Metrics { bounds, cell_w, line_h, text_x, rows: (bounds.size.height / line_h).ceil() as usize + 1 };
        let rows = self.view.update(cx, |v, cx| v.grid_rows(m, &look, cx));
        let gutter_color = look.c(look.theme.editor.gutter_text);
        let shape = |text: String, runs: &[GridRun]| {
            let runs: Vec<TextRun> = runs
                .iter()
                .map(|r| TextRun { len: r.len, font: code_font.clone(), color: r.color, background_color: r.bg, underline: None, strikethrough: None })
                .collect();
            ts.shape_line(SharedString::from(text), fs, &runs, None)
        };
        let rows = rows
            .into_iter()
            .map(|r| {
                let g = r.gutter.map(|g| {
                    let run = GridRun { len: g.len(), color: gutter_color, bg: None };
                    shape(g.to_string(), &[run])
                });
                (g, shape(r.text, &r.runs), r.bg)
            })
            .collect();
        Shaped { rows, text_x, line_h }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), s: &mut Shaped, window: &mut Window, cx: &mut App) {
        let look = Look::of(window, cx);
        let thumb = self.view.read(cx).scroll_thumb();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, look.surface(look.theme.ui.editor_bg)));
            for (i, (g, text, bg)) in s.rows.iter().enumerate() {
                let y = bounds.top() + s.line_h * i as f32;
                if let Some(bg) = bg {
                    window.paint_quad(fill(Bounds::new(point(bounds.left(), y), size(bounds.size.width, s.line_h)), *bg));
                }
                if let Some(g) = g {
                    g.paint(point(bounds.left() + look.z(8.), y), s.line_h, gpui::TextAlign::Left, None, window, cx).ok();
                }
                text.paint(point(bounds.left() + s.text_x, y), s.line_h, gpui::TextAlign::Left, None, window, cx).ok();
            }
            if let Some((top, frac)) = thumb {
                let h = (bounds.size.height * frac.clamp(0., 1.)).max(look.z(16.));
                let y = bounds.top() + (bounds.size.height - h) * top.clamp(0., 1.);
                let mut c = look.c(look.theme.ui.text_muted);
                c.a *= 0.35;
                window.paint_quad(fill(Bounds::new(point(bounds.right() - look.z(5.), y), size(look.z(3.), h)), c));
            }
        });
    }
}
