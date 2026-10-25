//! Tiled panes that glide (decision 60): a split tree laid out as rectangles,
//! and the motion from one arrangement to the next. The dock and the split
//! terminals draw with flex while still; while panes move they're drawn at
//! these rectangles instead, eased from where each pane was to where it goes.

use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::time::{Duration, Instant};

/// How long a pane takes to glide to its new place.
pub const GLIDE: Duration = Duration::from_millis(220);

/// Draw `cx`'s view again in a frame's time, while something glides. (A
/// frame asked for from an element's prepaint didn't draw the view again,
/// which left panes stuck where their glide began.)
pub fn next_frame<V: 'static>(cx: &mut gpui::Context<V>) {
    let bg = cx.background_executor().clone();
    cx.spawn(async move |this, cx| {
        bg.timer(Duration::from_millis(16)).await;
        this.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn lerp(self, to: Rect, t: f32) -> Rect {
        let m = |a: f32, b: f32| a + (b - a) * t;
        Rect { x: m(self.x, to.x), y: m(self.y, to.y), w: m(self.w, to.w), h: m(self.h, to.h) }
    }
}

/// A split tree reduced to what layout needs: leaves by key.
pub enum Shape<K> {
    Leaf(K),
    /// `vertical` = stacked top/bottom; `ratio` = the first child's share.
    Split { vertical: bool, ratio: f32, a: Box<Shape<K>>, b: Box<Shape<K>> },
}

impl<K: Debug> Shape<K> {
    /// The arrangement without its ratios: what changes when panes move,
    /// but not when a divider is dragged or the window resized.
    pub fn signature(&self) -> String {
        match self {
            Shape::Leaf(k) => format!("{k:?}"),
            Shape::Split { vertical, a, b, .. } => format!("{}({}|{})", if *vertical { 'v' } else { 'h' }, a.signature(), b.signature()),
        }
    }
}

pub struct Leaf<K> {
    pub key: K,
    /// From the root: false = first child, true = second.
    pub path: Vec<bool>,
    pub rect: Rect,
    /// Where it grows from when it's new: its rectangle squeezed flat
    /// against the outer edge (a new right or bottom pane slides in from there).
    pub enter: Rect,
}

/// Lay `s` out in `area` as the flex layout does: the first child takes
/// `ratio` of the whole, then `gap`, then the second child the rest.
pub fn lay<K: Clone>(s: &Shape<K>, area: Rect, gap: f32) -> Vec<Leaf<K>> {
    fn go<K: Clone>(s: &Shape<K>, r: Rect, gap: f32, enter: Rect, path: &mut Vec<bool>, out: &mut Vec<Leaf<K>>) {
        match s {
            Shape::Leaf(k) => out.push(Leaf { key: k.clone(), path: path.clone(), rect: r, enter }),
            Shape::Split { vertical, ratio, a, b } => {
                let (ra, rb, ea, eb) = if *vertical {
                    let ah = r.h * ratio;
                    let ra = Rect { h: ah, ..r };
                    let rb = Rect { y: r.y + ah + gap, h: (r.h - ah - gap).max(0.), ..r };
                    (ra, rb, Rect { h: 0., ..ra }, Rect { y: rb.y + rb.h, h: 0., ..rb })
                } else {
                    let aw = r.w * ratio;
                    let ra = Rect { w: aw, ..r };
                    let rb = Rect { x: r.x + aw + gap, w: (r.w - aw - gap).max(0.), ..r };
                    (ra, rb, Rect { w: 0., ..ra }, Rect { x: rb.x + rb.w, w: 0., ..rb })
                };
                path.push(false);
                go(a, ra, gap, ea, path, out);
                path.pop();
                path.push(true);
                go(b, rb, gap, eb, path, out);
                path.pop();
            }
        }
    }
    let mut out = Vec::new();
    go(s, area, gap, area, &mut Vec::new(), &mut out);
    out
}

fn ease(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// Where each pane is drawn as arrangements change.
pub struct Motion<K> {
    sig: String,
    from: HashMap<K, Rect>,
    to: HashMap<K, Rect>,
    start: Option<Instant>,
}

impl<K> Default for Motion<K> {
    fn default() -> Self {
        Motion { sig: String::new(), from: HashMap::new(), to: HashMap::new(), start: None }
    }
}

impl<K: Clone + Eq + Hash> Motion<K> {
    /// The keys of the arrangement last shown.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.to.keys()
    }

    /// Where each leaf is drawn now, and whether any is still moving. A new
    /// arrangement (`sig` changed) glides from where the panes are drawn, new
    /// ones growing from their edge; the same arrangement resized (a divider
    /// dragged, the window resized) follows at once, and so does one sharing
    /// no pane with the last (another group of terminals shown).
    pub fn step(&mut self, sig: &str, leaves: &[Leaf<K>]) -> (HashMap<K, Rect>, bool) {
        if sig != self.sig {
            // Scripted runs don't glide: a covered window draws only when a
            // step makes it, so a glide would start late and hide the
            // dividers from the next step.
            let first = !leaves.iter().any(|l| self.to.contains_key(&l.key)) || crate::script::on();
            let now = self.current();
            self.from = leaves.iter().map(|l| (l.key.clone(), if first { l.rect } else { now.get(&l.key).copied().unwrap_or(l.enter) })).collect();
            self.start = (!first).then(Instant::now);
            self.sig = sig.to_string();
        }
        self.to = leaves.iter().map(|l| (l.key.clone(), l.rect)).collect();
        if self.progress() >= 1. {
            self.start = None;
            self.from = self.to.clone();
        }
        (self.current(), self.start.is_some())
    }

    fn progress(&self) -> f32 {
        self.start.map_or(1., |s| (s.elapsed().as_secs_f32() / GLIDE.as_secs_f32()).min(1.))
    }

    fn current(&self) -> HashMap<K, Rect> {
        let e = ease(self.progress());
        self.to.iter().map(|(k, r)| (k.clone(), self.from.get(k).map_or(*r, |f| f.lerp(*r, e)))).collect()
    }
}

/// One rectangle that glides to each new place it's given (the drop preview).
#[derive(Default)]
pub struct Glide {
    from: Rect,
    to: Option<Rect>,
    start: Option<Instant>,
}

impl Glide {
    /// Head for `to` (from where it is now; it appears in place the first time).
    /// Returns where to draw it and whether it's still moving.
    pub fn set(&mut self, to: Rect) -> (Rect, bool) {
        match self.to {
            None => {
                self.from = to;
                self.start = None;
            }
            Some(old) if old != to => {
                self.from = self.now();
                self.start = Some(Instant::now());
            }
            _ => {}
        }
        self.to = Some(to);
        let r = self.now();
        let moving = self.start.is_some_and(|s| s.elapsed() < GLIDE);
        (r, moving)
    }

    pub fn clear(&mut self) {
        self.to = None;
        self.start = None;
    }

    fn now(&self) -> Rect {
        let Some(to) = self.to else { return self.from };
        let t = self.start.map_or(1., |s| (s.elapsed().as_secs_f32() / GLIDE.as_secs_f32()).min(1.));
        self.from.lerp(to, ease(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(vertical: bool, ratio: f32, a: Shape<u8>, b: Shape<u8>) -> Shape<u8> {
        Shape::Split { vertical, ratio, a: Box::new(a), b: Box::new(b) }
    }

    #[test]
    fn lays_out_like_flex_and_new_panes_grow_from_the_edge() {
        let s = split(false, 0.25, Shape::Leaf(1), split(true, 0.5, Shape::Leaf(2), Shape::Leaf(3)));
        let l = lay(&s, Rect { x: 0., y: 0., w: 400., h: 200. }, 10.);
        let r: Vec<_> = l.iter().map(|l| (l.key, l.path.clone(), l.rect)).collect();
        assert_eq!(r[0], (1, vec![false], Rect { x: 0., y: 0., w: 100., h: 200. }));
        assert_eq!(r[1], (2, vec![true, false], Rect { x: 110., y: 0., w: 290., h: 100. }));
        assert_eq!(r[2], (3, vec![true, true], Rect { x: 110., y: 110., w: 290., h: 90. }));
        assert_eq!(l[2].enter, Rect { x: 110., y: 200., w: 290., h: 0. }, "the bottom pane grows up from the bottom edge");
        assert_eq!(s.signature(), "h(1|v(2|3))");
    }

    #[test]
    fn motion_glides_on_a_new_arrangement_but_not_on_a_resize() {
        let area = Rect { x: 0., y: 0., w: 100., h: 100. };
        let mut m = Motion::default();
        let one = Shape::Leaf(1u8);
        let (r, moving) = m.step(&one.signature(), &lay(&one, area, 0.));
        assert!(!moving, "the first arrangement just appears");
        assert_eq!(r[&1], area);
        let two = split(false, 0.5, Shape::Leaf(1), Shape::Leaf(2));
        let (r, moving) = m.step(&two.signature(), &lay(&two, area, 0.));
        assert!(moving);
        assert!(r[&1].w > 90., "pane 1 starts where it was: {:?}", r[&1]);
        assert!(r[&2].w < 10., "pane 2 grows from the right edge: {:?}", r[&2]);
        let mut m = Motion::default();
        let wide = split(false, 0.5, Shape::Leaf(1), Shape::Leaf(2));
        m.step(&wide.signature(), &lay(&wide, area, 0.));
        let dragged = split(false, 0.7, Shape::Leaf(1), Shape::Leaf(2));
        let (r, moving) = m.step(&dragged.signature(), &lay(&dragged, area, 0.));
        assert!(!moving, "a divider drag follows at once");
        assert_eq!(r[&1].w, 70.);
    }
}
