//! Rearrangeable stage (owner: "restructure the terminal, viewing window and
//! stuff, like photoshop or vscode"). The agent stage is a tree of panes; each
//! pane holds tabs (Chat/Terminal, Live, Browser, Changes, To-do). Drag a tab
//! (or a pane by its tab strip) over a pane: a preview shows where it lands,
//! beside the pane on the nearest edge or, on its middle or strip, joining it;
//! the panes glide into their new places (decision 60). Drag a divider to
//! resize; close a tab to tuck it away (the pane's + menu brings it back). One arrangement for structured agents and one for Agent Terminals,
//! saved in config.toml (`[layout] acp` / `terminal`). The old peek layouts
//! A/B/C are presets.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{App, Bounds, Context, Div, Global, MouseButton, Pixels, Render, SharedString, Window, canvas, div, prelude::*, px};

use crate::look::Look;
use crate::tiling::{self, Glide, Motion, Rect, Shape};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Panel {
    /// The agent itself: chat + prompt (ACP) or its terminal.
    Session,
    Live,
    Browser,
    Changes,
    Todo,
}

impl Panel {
    fn key(self) -> &'static str {
        match self {
            Panel::Session => "session",
            Panel::Live => "live",
            Panel::Browser => "browser",
            Panel::Changes => "changes",
            Panel::Todo => "todo",
        }
    }

    fn from_key(s: &str) -> Option<Panel> {
        Some(match s {
            "session" => Panel::Session,
            "live" => Panel::Live,
            "browser" => Panel::Browser,
            "changes" => Panel::Changes,
            "todo" => Panel::Todo,
            _ => return None,
        })
    }
}

/// Which stage a dock belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Acp,
    Terminal,
}

impl Kind {
    pub fn panels(self) -> &'static [Panel] {
        match self {
            Kind::Acp => &[Panel::Session, Panel::Live, Panel::Browser, Panel::Changes, Panel::Todo],
            Kind::Terminal => &[Panel::Session, Panel::Live, Panel::Browser, Panel::Changes, Panel::Todo],
        }
    }

    pub fn label(self, p: Panel) -> &'static str {
        match (self, p) {
            (Kind::Acp, Panel::Session) => "Chat",
            (Kind::Terminal, Panel::Session) => "Terminal",
            (_, Panel::Live) => "Live",
            (_, Panel::Browser) => "Browser",
            (_, Panel::Changes) => "Changes",
            (_, Panel::Todo) => "To-do",
        }
    }

    /// A pane tile's background with window gaps on: what the stage region
    /// paints without them (the chat's editor colour, the terminal's panel).
    pub fn fill(self, look: &Look) -> qur_theme::Rgba {
        match self {
            Kind::Acp => look.theme.ui.editor_bg,
            Kind::Terminal => look.theme.ui.panel_bg,
        }
    }

    pub fn config_key(self) -> &'static str {
        match self {
            Kind::Acp => "layout.acp",
            Kind::Terminal => "layout.terminal",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Pane { tabs: Vec<Panel>, active: usize },
    /// `vertical` = stacked top/bottom; `ratio` = the first child's share.
    Split { vertical: bool, ratio: f32, a: Box<Node>, b: Box<Node> },
}

/// Where a dragged tab lands on a pane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    Center,
    Left,
    Right,
    Top,
    Bottom,
}

/// Path from the root: false = first child, true = second.
pub type Path = Vec<bool>;

#[derive(Clone, Debug, PartialEq)]
pub struct Dock {
    pub root: Node,
}

fn pane(tabs: &[Panel]) -> Node {
    Node::Pane { tabs: tabs.to_vec(), active: 0 }
}

fn split(vertical: bool, ratio: f32, a: Node, b: Node) -> Node {
    Node::Split { vertical, ratio, a: Box::new(a), b: Box::new(b) }
}

impl Dock {
    /// The old peek layouts as starting points (decision 25).
    pub fn preset(kind: Kind, layout: &str) -> Dock {
        use Panel::*;
        let root = match (kind, layout) {
            (Kind::Terminal, "B") => pane(&[Session, Live, Changes, Browser]),
            (Kind::Terminal, "C") => split(true, 0.6, pane(&[Live, Browser, Changes]), pane(&[Session])),
            // Decision 27: an Agent Terminal is a real terminal, the whole stage.
            (Kind::Terminal, _) => pane(&[Session]),
            (Kind::Acp, "B") => pane(&[Live, Todo, Session, Changes, Browser]),
            (Kind::Acp, "C") => split(true, 0.6, pane(&[Live, Browser, Changes]), pane(&[Session, Todo])),
            (Kind::Acp, _) => split(false, 0.66, pane(&[Live, Browser, Changes]), pane(&[Session, Todo])),
        };
        Dock { root }
    }

    /// Compact form for config.toml, e.g. `h0.66(live,browser|session)`.
    pub fn serialize(&self) -> String {
        fn go(n: &Node, out: &mut String) {
            match n {
                Node::Pane { tabs, active } => {
                    for (i, t) in tabs.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        if i == *active && *active != 0 {
                            out.push('*');
                        }
                        out.push_str(t.key());
                    }
                }
                Node::Split { vertical, ratio, a, b } => {
                    out.push_str(&format!("{}{:.3}(", if *vertical { 'v' } else { 'h' }, ratio));
                    go(a, out);
                    out.push('|');
                    go(b, out);
                    out.push(')');
                }
            }
        }
        let mut s = String::new();
        go(&self.root, &mut s);
        s
    }

    /// Parse `serialize`'s form; None if it's malformed. Panels this kind
    /// doesn't have are dropped.
    pub fn parse(kind: Kind, s: &str) -> Option<Dock> {
        fn node(s: &str, kind: Kind) -> Option<(Node, &str)> {
            let s = s.trim_start();
            if let Some(dir @ ('h' | 'v')) = s.chars().next()
                && let Some(open) = s.find('(')
                && let Ok(ratio) = s[1..open].parse::<f32>()
            {
                let (a, rest) = node(&s[open + 1..], kind)?;
                let rest = rest.strip_prefix('|')?;
                let (b, rest) = node(rest, kind)?;
                let rest = rest.strip_prefix(')')?;
                return Some((Node::Split { vertical: dir == 'v', ratio: ratio.clamp(0.1, 0.9), a: Box::new(a), b: Box::new(b) }, rest));
            }
            let end = s.find(['|', ')']).unwrap_or(s.len());
            let mut tabs = Vec::new();
            let mut active = 0;
            for t in s[..end].split(',').map(str::trim).filter(|t| !t.is_empty()) {
                let (star, key) = t.strip_prefix('*').map(|k| (true, k)).unwrap_or((false, t));
                let p = Panel::from_key(key)?;
                if kind.panels().contains(&p) && !tabs.contains(&p) {
                    if star {
                        active = tabs.len();
                    }
                    tabs.push(p);
                }
            }
            Some((Node::Pane { tabs, active }, &s[end..]))
        }
        let (root, rest) = node(s, kind)?;
        if !rest.trim().is_empty() {
            return None;
        }
        let mut d = Dock { root };
        d.normalize();
        // Never lose the agent itself.
        if !d.contains(Panel::Session) {
            d.add(Panel::Session);
        }
        Some(d)
    }

    pub fn contains(&self, p: Panel) -> bool {
        self.find(p).is_some()
    }

    /// The pane holding `p`.
    pub fn find(&self, p: Panel) -> Option<Path> {
        fn go(n: &Node, p: Panel, path: &mut Path) -> bool {
            match n {
                Node::Pane { tabs, .. } => tabs.contains(&p),
                Node::Split { a, b, .. } => {
                    path.push(false);
                    if go(a, p, path) {
                        return true;
                    }
                    path.pop();
                    path.push(true);
                    if go(b, p, path) {
                        return true;
                    }
                    path.pop();
                    false
                }
            }
        }
        let mut path = Vec::new();
        go(&self.root, p, &mut path).then_some(path)
    }

    /// Shown right now: the active tab of some pane.
    pub fn is_visible(&self, p: Panel) -> bool {
        self.find(p).and_then(|path| match self.node(&path) {
            Some(Node::Pane { tabs, active }) => Some(tabs.get(*active) == Some(&p)),
            _ => None,
        }) == Some(true)
    }

    pub fn node(&self, path: &[bool]) -> Option<&Node> {
        let mut n = &self.root;
        for &second in path {
            n = match n {
                Node::Split { a, b, .. } => {
                    if second {
                        b
                    } else {
                        a
                    }
                }
                Node::Pane { .. } => return None,
            };
        }
        Some(n)
    }

    fn node_mut(&mut self, path: &[bool]) -> Option<&mut Node> {
        let mut n = &mut self.root;
        for &second in path {
            n = match n {
                Node::Split { a, b, .. } => {
                    if second {
                        b
                    } else {
                        a
                    }
                }
                Node::Pane { .. } => return None,
            };
        }
        Some(n)
    }

    /// Bring `p` to the front, adding it (to the first pane that isn't the
    /// agent's own, else the first pane) if it was tucked away.
    pub fn activate(&mut self, p: Panel) {
        if !self.contains(p) {
            self.add(p);
        }
        if let Some(path) = self.find(p)
            && let Some(Node::Pane { tabs, active }) = self.node_mut(&path)
            && let Some(i) = tabs.iter().position(|t| *t == p)
        {
            *active = i;
        }
    }

    pub fn add(&mut self, p: Panel) {
        if self.contains(p) {
            return;
        }
        let target = self.first_pane(|tabs| !tabs.contains(&Panel::Session)).or_else(|| self.first_pane(|_| true)).unwrap_or_default();
        if let Some(Node::Pane { tabs, active }) = self.node_mut(&target) {
            tabs.push(p);
            *active = tabs.len() - 1;
        }
    }

    fn first_pane(&self, ok: impl Fn(&[Panel]) -> bool + Copy) -> Option<Path> {
        fn go(n: &Node, path: &mut Path, ok: &dyn Fn(&[Panel]) -> bool) -> bool {
            match n {
                Node::Pane { tabs, .. } => ok(tabs),
                Node::Split { a, b, .. } => {
                    path.push(false);
                    if go(a, path, ok) {
                        return true;
                    }
                    path.pop();
                    path.push(true);
                    if go(b, path, ok) {
                        return true;
                    }
                    path.pop();
                    false
                }
            }
        }
        let mut path = Vec::new();
        go(&self.root, &mut path, &ok).then_some(path)
    }

    /// Tuck `p` away (the agent itself always stays).
    pub fn close(&mut self, p: Panel) {
        if p == Panel::Session {
            return;
        }
        self.remove(p);
        self.normalize();
    }

    fn remove(&mut self, p: Panel) {
        if let Some(path) = self.find(p)
            && let Some(Node::Pane { tabs, active }) = self.node_mut(&path)
            && let Some(i) = tabs.iter().position(|t| *t == p)
        {
            tabs.remove(i);
            if *active >= tabs.len() || *active > i {
                *active = active.saturating_sub(1);
            }
        }
    }

    /// Move `p` to the pane at `target`: into it (Center) or beside it.
    pub fn move_panel(&mut self, p: Panel, target: &[bool], edge: Edge) {
        self.move_panels(&[p], p, target, edge);
    }

    /// Move `moved` (one tab, or a whole pane's tabs) to the pane at
    /// `target`: into it (Center) or beside it as a pane of their own,
    /// `active` in front.
    pub fn move_panels(&mut self, moved: &[Panel], active: Panel, target: &[bool], edge: Edge) {
        let Some(Node::Pane { tabs: target_tabs, .. }) = self.node(target) else { return };
        // A tab that stays at the target finds it again after the removal
        // (which can reshape the tree). None staying: a pane onto itself.
        let Some(&anchor) = target_tabs.iter().find(|t| !moved.contains(t)) else { return };
        for &p in moved {
            self.remove(p);
        }
        let Some(target) = self.find(anchor) else { return };
        let Some(node) = self.node_mut(&target) else { return };
        match edge {
            Edge::Center => {
                if let Node::Pane { tabs, active: a } = node {
                    tabs.extend_from_slice(moved);
                    *a = tabs.iter().position(|t| *t == active).unwrap_or(tabs.len() - 1);
                }
            }
            _ => {
                let old = std::mem::replace(node, pane(&[]));
                let new = Node::Pane { tabs: moved.to_vec(), active: moved.iter().position(|t| *t == active).unwrap_or(0) };
                let vertical = matches!(edge, Edge::Top | Edge::Bottom);
                *node = if matches!(edge, Edge::Left | Edge::Top) { split(vertical, 0.5, new, old) } else { split(vertical, 0.5, old, new) };
            }
        }
        self.normalize();
    }

    /// The tabs of the pane holding `p`.
    pub fn pane_of(&self, p: Panel) -> Vec<Panel> {
        match self.find(p).and_then(|path| self.node(&path)) {
            Some(Node::Pane { tabs, .. }) => tabs.clone(),
            _ => vec![p],
        }
    }

    pub fn set_ratio(&mut self, path: &[bool], r: f32) {
        if let Some(Node::Split { ratio, .. }) = self.node_mut(path) {
            *ratio = r.clamp(0.12, 0.88);
        }
    }

    /// Drop empty panes and collapse splits left with one child.
    fn normalize(&mut self) {
        fn go(n: Node) -> Option<Node> {
            match n {
                Node::Pane { tabs, active } => (!tabs.is_empty()).then(|| Node::Pane { active: active.min(tabs.len() - 1), tabs }),
                Node::Split { vertical, ratio, a, b } => match (go(*a), go(*b)) {
                    (Some(a), Some(b)) => Some(Node::Split { vertical, ratio, a: Box::new(a), b: Box::new(b) }),
                    (Some(x), None) | (None, Some(x)) => Some(x),
                    (None, None) => None,
                },
            }
        }
        let root = std::mem::replace(&mut self.root, pane(&[]));
        self.root = go(root).unwrap_or_else(|| pane(&[Panel::Session]));
    }
}

/// Both stages' docks, shared by every window (like a workbench layout).
pub struct Docks {
    pub acp: Dock,
    pub terminal: Dock,
    /// A divider being dragged: (kind, split path, the split's bounds).
    resizing: Option<(Kind, Path, Bounds<Pixels>)>,
}

impl Global for Docks {}

impl Docks {
    pub fn load(config: &qur_config::Config, cx: &mut App) {
        let preset = config.peek_layout.as_str();
        let acp = config.layout.acp.as_deref().and_then(|s| Dock::parse(Kind::Acp, s)).unwrap_or_else(|| Dock::preset(Kind::Acp, preset));
        let terminal = config.layout.terminal.as_deref().and_then(|s| Dock::parse(Kind::Terminal, s)).unwrap_or_else(|| Dock::preset(Kind::Terminal, preset));
        cx.set_global(Docks { acp, terminal, resizing: None });
    }

    pub fn get(cx: &App, kind: Kind) -> Dock {
        let d = cx.global::<Docks>();
        match kind {
            Kind::Acp => d.acp.clone(),
            Kind::Terminal => d.terminal.clone(),
        }
    }

    /// Change a dock and save it (`[layout]` in config.toml).
    pub fn update(cx: &mut App, kind: Kind, f: impl FnOnce(&mut Dock)) {
        let text = {
            let d = cx.global_mut::<Docks>();
            let dock = match kind {
                Kind::Acp => &mut d.acp,
                Kind::Terminal => &mut d.terminal,
            };
            let before = dock.clone();
            f(dock);
            (dock != &before).then(|| dock.serialize())
        };
        if let Some(text) = text {
            qur_config::write_key(&qur_config::config_path(), kind.config_key(), Some(text.as_str().into())).ok();
            cx.refresh_windows();
        }
    }

    /// Change it without saving (live divider dragging; saved on release).
    fn update_quiet(cx: &mut App, kind: Kind, f: impl FnOnce(&mut Dock)) {
        let d = cx.global_mut::<Docks>();
        f(match kind {
            Kind::Acp => &mut d.acp,
            Kind::Terminal => &mut d.terminal,
        });
        cx.refresh_windows();
    }
}

/// What's being dragged: a tab of the given dock, or (`whole`, dragged by
/// its tab strip) the pane it's the front tab of.
#[derive(Clone)]
pub struct DragTab {
    pub kind: Kind,
    pub panel: Panel,
    pub whole: bool,
    pub label: SharedString,
}

impl Render for DragTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = &look.theme.ui;
        div()
            .px(look.z(10.))
            .py(look.z(3.))
            .rounded(look.z(4.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.focus_ring))
            .text_size(look.text(-1.))
            .text_color(look.c(ui.text))
            .font_family(look.ui_font.clone())
            .child(self.label.clone())
    }
}

type Bnds = Rc<RefCell<HashMap<Path, Bounds<Pixels>>>>;

/// One drawn dock's own state, per kind and per view drawing it (each window
/// draws its own): its area, the panes' motion, the drop target under a
/// dragged tab and its preview, and the pane whose + menu is open.
#[derive(Default)]
struct Drawn {
    area: Bounds<Pixels>,
    motion: Motion<Panel>,
    hover: Option<(Path, Edge)>,
    preview: Glide,
    adding: Option<Path>,
}

thread_local! {
    static DRAWN: RefCell<HashMap<(Kind, gpui::EntityId), Drawn>> = RefCell::new(HashMap::new());
}

fn drawn<R>(key: (Kind, gpui::EntityId), f: impl FnOnce(&mut Drawn) -> R) -> R {
    DRAWN.with(|d| f(d.borrow_mut().entry(key).or_default()))
}

/// The tree for layout, each pane keyed by one of its tabs: one that keyed a
/// pane last time when it has one, so a pane that loses or gains a tab
/// glides to its new place instead of appearing anew.
fn shape(n: &Node, prev: &HashSet<Panel>) -> Shape<Panel> {
    match n {
        Node::Pane { tabs, .. } => Shape::Leaf(tabs.iter().copied().find(|t| prev.contains(t)).or(tabs.first().copied()).unwrap_or(Panel::Session)),
        Node::Split { vertical, ratio, a, b } => Shape::Split { vertical: *vertical, ratio: *ratio, a: Box::new(shape(a, prev)), b: Box::new(shape(b, prev)) },
    }
}

/// Where a tab dropped at `p` on a pane lands: on the tab strip or the
/// middle it joins the pane, else beside it on the nearest edge.
pub(crate) fn edge_at(b: Bounds<Pixels>, p: gpui::Point<Pixels>, strip: Pixels) -> Edge {
    if p.y - b.top() <= strip {
        return Edge::Center;
    }
    let fx = (p.x - b.left()) / b.size.width;
    let fy = (p.y - b.top()) / b.size.height;
    if (0.3..=0.7).contains(&fx) && (0.3..=0.7).contains(&fy) {
        return Edge::Center;
    }
    [(fx, Edge::Left), (1. - fx, Edge::Right), (fy, Edge::Top), (1. - fy, Edge::Bottom)]
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(Edge::Center, |e| e.1)
}

/// The drop preview: the part of the pane the dropped tab will take, inset.
fn drop_rect(r: Rect, edge: Edge) -> Rect {
    let part = match edge {
        Edge::Center => r,
        Edge::Left => Rect { w: r.w / 2., ..r },
        Edge::Right => Rect { x: r.x + r.w / 2., w: r.w / 2., ..r },
        Edge::Top => Rect { h: r.h / 2., ..r },
        Edge::Bottom => Rect { y: r.y + r.h / 2., h: r.h / 2., ..r },
    };
    let i = 6f32.min(part.w / 4.).min(part.h / 4.);
    Rect { x: part.x + i, y: part.y + i, w: part.w - 2. * i, h: part.h - 2. * i }
}

/// Draw `kind`'s dock, asking `content` for each visible panel. Generic over
/// the view so the panels' own listeners keep working (AgentTask, Workspace).
/// While a tab is dragged, a rounded preview shows where it will land (as
/// macOS window tiling does); after a rearrangement the panes glide to their
/// new places, drawn at absolute rectangles until they settle.
pub fn render<V: 'static>(kind: Kind, look: &Look, cx: &mut Context<V>, content: &mut dyn FnMut(Panel, &mut Context<V>) -> gpui::AnyElement) -> gpui::AnyElement {
    let dock = Docks::get(cx, kind);
    let key = (kind, cx.entity_id());
    let gap = if look.tiles.is_some() { f32::from(look.gap()) } else { 1. };
    let dragging = cx.has_active_drag();
    let (leaves, rects, moving, preview) = drawn(key, |d| {
        let prev: HashSet<Panel> = d.motion.keys().copied().collect();
        let shape = shape(&dock.root, &prev);
        let area = Rect { x: 0., y: 0., w: f32::from(d.area.size.width), h: f32::from(d.area.size.height) };
        let leaves = tiling::lay(&shape, area, gap);
        let (rects, moving) = d.motion.step(&shape.signature(), &leaves);
        if !dragging {
            d.hover = None;
        }
        let preview = d.hover.clone().and_then(|(path, edge)| leaves.iter().find(|l| l.path == path).map(|l| d.preview.set(drop_rect(l.rect, edge))));
        if preview.is_none() {
            d.preview.clear();
        }
        (leaves, rects, moving, preview)
    });
    if moving || preview.is_some_and(|p| p.1) {
        tiling::next_frame(cx);
    }
    let bounds: Bnds = Rc::new(RefCell::new(HashMap::new()));
    let body = if moving {
        div()
            .relative()
            .size_full()
            .children(leaves.iter().filter_map(|l| {
                let Some(Node::Pane { tabs, active }) = dock.node(&l.path) else { return None };
                let r = rects.get(&l.key).copied().unwrap_or(l.rect);
                Some(div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).overflow_hidden().child(pane_el(kind, tabs, *active, l.path.clone(), look, cx, content)))
            }))
            .into_any_element()
    } else {
        node(kind, &dock.root, Vec::new(), look, cx, content, &bounds)
    };
    let accent = look.c(look.theme.ui.focus_ring);
    let preview = preview.map(|(r, _)| {
        let mut fill = accent;
        fill.a = 0.16;
        div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).rounded(look.z(10.)).bg(fill).border_2().border_color(accent)
    });
    div()
        .id(("dock", kind as usize))
        .relative()
        .size_full()
        .flex()
        .child(
            canvas(
                move |b, _, _| drawn(key, |d| d.area = b),
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .on_mouse_move(move |ev: &gpui::MouseMoveEvent, _, cx| {
            let Some((k, path, b)) = cx.global::<Docks>().resizing.clone() else { return };
            if ev.pressed_button != Some(MouseButton::Left) {
                cx.global_mut::<Docks>().resizing = None;
                let text = Docks::get(cx, k).serialize();
                qur_config::write_key(&qur_config::config_path(), k.config_key(), Some(text.as_str().into())).ok();
                return;
            }
            let vertical = matches!(Docks::get(cx, k).node(&path), Some(Node::Split { vertical: true, .. }));
            let r = if vertical { (ev.position.y - b.top()) / b.size.height } else { (ev.position.x - b.left()) / b.size.width };
            Docks::update_quiet(cx, k, |d| d.set_ratio(&path, r));
        })
        .on_mouse_up(MouseButton::Left, |_, _, cx| {
            if let Some((k, ..)) = cx.global_mut::<Docks>().resizing.take() {
                let text = Docks::get(cx, k).serialize();
                qur_config::write_key(&qur_config::config_path(), k.config_key(), Some(text.as_str().into())).ok();
            }
        })
        .child(body)
        .children(preview)
        .into_any_element()
}

/// A pane's path for scripted runs' marks: `root`, else `a`/`b` per split (first/second).
fn path_name(path: &[bool]) -> String {
    if path.is_empty() { "root".into() } else { path.iter().map(|&b| if b { 'b' } else { 'a' }).collect() }
}

fn measure(path: Path, bounds: &Bnds) -> impl IntoElement {
    let bounds = bounds.clone();
    canvas(
        move |b, _, _| {
            bounds.borrow_mut().insert(path.clone(), b);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

fn node<V: 'static>(kind: Kind, n: &Node, path: Path, look: &Look, cx: &mut Context<V>, content: &mut dyn FnMut(Panel, &mut Context<V>) -> gpui::AnyElement, bounds: &Bnds) -> gpui::AnyElement {
    let ui = &look.theme.ui;
    match n {
        Node::Split { vertical, ratio, a, b } => {
            let (mut pa, mut pb) = (path.clone(), path.clone());
            pa.push(false);
            pb.push(true);
            let first = node(kind, a, pa, look, cx, content, bounds);
            let second = node(kind, b, pb, look, cx, content, bounds);
            let (bounds2, path2) = (bounds.clone(), path.clone());
            // Without gaps a 1 px line; with them the divider is the gap
            // itself (clear, the accent line showing while hovered).
            let gap = look.gap();
            let group = SharedString::from(format!("split-{kind:?}-{path:?}"));
            let handle = div()
                .id(group.clone())
                .group(group.clone())
                .flex_none()
                .when(look.tiles.is_none(), |d| {
                    d.bg(look.c(ui.border))
                        .hover(|d| d.bg(look.c(ui.focus_ring)))
                        .when(*vertical, |d| d.h(px(1.)).w_full())
                        .when(!*vertical, |d| d.w(px(1.)).h_full())
                })
                .when(look.tiles.is_some(), |d| {
                    d.when(*vertical, |d| d.h(gap).w_full())
                        .when(!*vertical, |d| d.w(gap).h_full())
                        .child(
                            div()
                                .absolute()
                                .when(*vertical, |d| d.left(gap * 2.).right(gap * 2.).top((gap - px(1.)) / 2.).h(px(1.)))
                                .when(!*vertical, |d| d.top(gap * 2.).bottom(gap * 2.).left((gap - px(1.)) / 2.).w(px(1.)))
                                .group_hover(group.clone(), |d| d.bg(look.c(ui.focus_ring))),
                        )
                })
                .when(*vertical, |d| d.cursor_row_resize())
                .when(!*vertical, |d| d.cursor_col_resize())
                // A wider grab area than the 1 px line.
                .child(div().absolute().when(*vertical, |d| d.left_0().right_0().top(px(-3.)).h(px(7.))).when(!*vertical, |d| d.top_0().bottom_0().left(px(-3.)).w(px(7.))))
                .relative()
                .children(crate::script::mark(|| format!("split:{kind:?}:{}", path_name(&path))))
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    if let Some(b) = bounds2.borrow().get(&path2).copied() {
                        cx.global_mut::<Docks>().resizing = Some((kind, path2.clone(), b));
                    }
                    cx.stop_propagation();
                });
            div()
                .relative()
                .size_full()
                .flex()
                .when(*vertical, |d| d.flex_col())
                .child(measure(path.clone(), bounds))
                .child(div().flex_none().overflow_hidden().when(*vertical, |d| d.w_full().h(gpui::relative(*ratio))).when(!*vertical, |d| d.h_full().w(gpui::relative(*ratio))).child(first))
                .child(handle)
                .child(div().flex_1().min_w(px(0.)).min_h(px(0.)).overflow_hidden().child(second))
                .into_any_element()
        }
        Node::Pane { tabs, active } => pane_el(kind, tabs, *active, path, look, cx, content),
    }
}

/// One pane: its tab strip (drag a tab to move it, or the strip to move the
/// whole pane; + brings back a tucked-away panel) and the front tab's panel.
/// A tab dragged over it shows where it would land; dropped, it moves there.
fn pane_el<V: 'static>(kind: Kind, tabs: &[Panel], active: usize, path: Path, look: &Look, cx: &mut Context<V>, content: &mut dyn FnMut(Panel, &mut Context<V>) -> gpui::AnyElement) -> gpui::AnyElement {
    let ui = &look.theme.ui;
    let key = (kind, cx.entity_id());
    let current = tabs.get(active).copied();
    let hidden: Vec<Panel> = kind.panels().iter().copied().filter(|p| !Docks::get(cx, kind).contains(*p)).collect();
    let strip_h = look.z(26.);
    let front = current.unwrap_or(Panel::Session);
    let label = if tabs.len() > 1 { format!("{} +{}", kind.label(front), tabs.len() - 1) } else { kind.label(front).to_string() };
    let whole = DragTab { kind, panel: front, whole: true, label: label.into() };
    let strip = div()
        .id(SharedString::from(format!("strip-{kind:?}-{path:?}")))
        .flex_none()
        .h(strip_h)
        .px(look.z(6.))
        .flex()
        .items_center()
        .gap(look.z(2.))
        .border_b_1()
        .border_color(look.c(ui.border))
        .children(crate::script::mark(|| format!("strip:{kind:?}:{}", path_name(&path))))
        .on_drag(whole, |d: &DragTab, _, _, cx| cx.new(|_| d.clone()))
        .children(tabs.iter().enumerate().map(|(i, &p)| {
            let on = i == active;
            let label: SharedString = kind.label(p).into();
            let drag = DragTab { kind, panel: p, whole: false, label: label.clone() };
            div()
                .id(SharedString::from(format!("tab-{kind:?}-{p:?}")))
                .group("tab")
                .flex()
                .items_center()
                .gap(look.z(4.))
                .px(look.z(8.))
                .py(look.z(1.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_size(look.text(-2.))
                .text_color(look.c(if on { ui.text } else { ui.text_muted }))
                .when(on, |d| d.bg(look.c(ui.active_bg)))
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .child(label)
                .children(crate::script::mark(|| format!("tab:{kind:?}:{p:?}")))
                .when(p != Panel::Session, |d| {
                    // Shown on the front tab and on hover; clear, not
                    // hidden, elsewhere, so it always takes the click.
                    d.child(
                        div()
                            .id(SharedString::from(format!("tab-close-{kind:?}-{p:?}")))
                            .relative()
                            .children(crate::script::mark(|| format!("close:{kind:?}:{p:?}")))
                            .when(!on, |d| d.opacity(0.))
                            .group_hover("tab", |d| d.opacity(1.))
                            .text_color(look.c(ui.text_muted))
                            .hover(|d| d.text_color(look.c(ui.text)))
                            .child("×")
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                Docks::update(cx, kind, |d| d.close(p));
                            }),
                    )
                })
                .on_click(move |_, _, cx| Docks::update(cx, kind, |d| d.activate(p)))
                .on_drag(drag, |d: &DragTab, _, _, cx| cx.new(|_| d.clone()))
        }))
        .child(div().flex_1())
        .when(!hidden.is_empty(), |d| {
            // One + for the tucked-away panels: a menu of them, added here.
            let target = path.clone();
            d.child(
                div()
                    .id(SharedString::from(format!("plus-{kind:?}-{path:?}")))
                    .px(look.z(6.))
                    .rounded(look.z(3.))
                    .cursor_pointer()
                    .text_color(look.c(ui.text_muted))
                    .hover(|d| d.bg(look.c(ui.hover_bg)).text_color(look.c(ui.text_secondary)))
                    .relative()
                    .child("+")
                    .children(crate::script::mark(|| format!("plus:{kind:?}:{}", path_name(&path))))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, _| {
                        drawn(key, |d| d.adding = Some(target.clone()));
                        window.refresh();
                    }),
            )
        });
    let menu = (drawn(key, |d| d.adding.as_ref() == Some(&path)) && !hidden.is_empty()).then(|| {
        let target = path.clone();
        div()
            .id(SharedString::from(format!("add-menu-{kind:?}-{path:?}")))
            .absolute()
            .top(strip_h)
            .right(look.z(6.))
            .mt(look.z(2.))
            .py(look.z(4.))
            .min_w(look.z(140.))
            .rounded(look.z(6.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border))
            .shadow_md()
            .flex()
            .flex_col()
            .on_mouse_down_out(move |_, window, _| {
                drawn(key, |d| d.adding = None);
                window.refresh();
            })
            .children(hidden.iter().map(|&p| {
                let target = target.clone();
                div()
                    .id(SharedString::from(format!("add-{kind:?}-{p:?}")))
                    .px(look.z(10.))
                    .py(look.z(3.))
                    .cursor_pointer()
                    .text_size(look.text(-1.))
                    .text_color(look.c(ui.text))
                    .hover(|d| d.bg(look.c(ui.hover_bg)))
                    .relative()
                    .child(kind.label(p))
                    .children(crate::script::mark(|| format!("add:{kind:?}:{p:?}")))
                    .on_click(move |_, window, cx| {
                        drawn(key, |d| d.adding = None);
                        Docks::update(cx, kind, |d| {
                            d.add(p);
                            d.move_panel(p, &target, Edge::Center);
                        });
                        window.refresh();
                    })
            }))
    });
    let body = div()
        .flex_1()
        .min_h(px(0.))
        .relative()
        .children(current.map(|p| content(p, cx)))
        .children(crate::script::mark(|| format!("pane:{kind:?}:{}", path_name(&path))));
    // With window gaps each pane is a tile (its own background); without
    // them the stage region behind the dock paints one.
    let pane = div().size_full().flex().flex_col().relative();
    let pane = if look.tiles.is_some() { look.tile(pane, kind.fill(look)) } else { pane };
    let (hover_path, drop_path, own) = (path.clone(), path, tabs.to_vec());
    pane.child(strip)
        .child(body)
        .children(menu)
        .on_drag_move::<DragTab>(move |ev, window, cx| {
            let t = ev.drag(cx);
            if t.kind != kind || !ev.bounds.contains(&ev.event.position) {
                return;
            }
            let moved = if t.whole { Docks::get(cx, kind).pane_of(t.panel) } else { vec![t.panel] };
            // A pane dropped on itself (or its only tab) changes nothing: no preview.
            let hover = (!own.iter().all(|x| moved.contains(x))).then(|| (hover_path.clone(), edge_at(ev.bounds, ev.event.position, strip_h)));
            if drawn(key, |d| std::mem::replace(&mut d.hover, hover.clone()) != hover) {
                window.refresh();
            }
        })
        .on_drop(move |t: &DragTab, window, cx| {
            let Some((at, edge)) = drawn(key, |d| d.hover.take()) else { return };
            if t.kind != kind || at != drop_path {
                return;
            }
            let moved = if t.whole { Docks::get(cx, kind).pane_of(t.panel) } else { vec![t.panel] };
            Docks::update(cx, kind, |d| d.move_panels(&moved, t.panel, &at, edge));
            window.refresh();
        })
        .into_any_element()
}

/// A miniature of an arrangement: panes, their tab strips (the active tab
/// highlighted) and a hint of each pane's content. For choosing a layout.
pub fn diagram(dock: &Dock, kind: Kind, look: &Look, w: f32, h: f32) -> Div {
    fn go(n: &Node, kind: Kind, look: &Look) -> Div {
        let ui = &look.theme.ui;
        match n {
            // No pane drawn under 30% of its split: a thin drawer still reads
            // as a pane in something this small.
            Node::Split { vertical, ratio, a, b } => div()
                .size_full()
                .flex()
                .gap(px(2.))
                .when(*vertical, |d| d.flex_col())
                .child(
                    div()
                        .flex_none()
                        .overflow_hidden()
                        .when(*vertical, |d| d.w_full().h(gpui::relative(ratio.clamp(0.3, 0.7))))
                        .when(!*vertical, |d| d.h_full().w(gpui::relative(ratio.clamp(0.3, 0.7))))
                        .child(go(a, kind, look)),
                )
                .child(div().flex_1().min_w(px(0.)).min_h(px(0.)).overflow_hidden().child(go(b, kind, look))),
            Node::Pane { tabs, active } => {
                let current = tabs.get(*active).copied().unwrap_or(Panel::Session);
                let mut accent = look.c(look.theme.accents[0].base);
                let tab = move |on: bool| {
                    let mut c = if on { accent } else { look.c(ui.text_muted) };
                    c.a = if on { 0.9 } else { 0.35 };
                    div().flex_none().h(px(3.)).w(px(9.)).rounded(px(1.)).bg(c)
                };
                accent.a = 0.55;
                let line = |w: f32, c: gpui::Hsla| div().flex_none().h(px(2.)).w(gpui::relative(w)).rounded(px(1.)).bg(c);
                let mut faint = look.c(ui.text_muted);
                faint.a = 0.45;
                let mut added = look.c(look.theme.diff.added_text);
                added.a = 0.7;
                let mut removed = look.c(look.theme.diff.removed_text);
                removed.a = 0.7;
                // A hint of what the active tab shows.
                let body = div().flex_1().min_h(px(0.)).p(px(3.)).flex().flex_col().gap(px(2.)).overflow_hidden();
                let body = match current {
                    Panel::Session if kind == Kind::Terminal => body.child(line(0.5, faint)).child(line(0.3, faint)).child(div().flex_none().h(px(4.)).w(px(4.)).bg(accent)),
                    Panel::Session => body.child(div().flex_1()).child(line(0.7, faint)).child(line(0.55, faint)).child(div().flex_none().h(px(5.)).w_full().rounded(px(1.)).border_1().border_color(faint)),
                    Panel::Live => body.child(line(0.6, faint)).child(line(0.8, accent)).child(line(0.5, faint)).child(line(0.7, faint)),
                    Panel::Browser => body.child(div().flex_1().rounded(px(1.)).bg(faint)),
                    Panel::Changes => body.child(line(0.7, added)).child(line(0.5, removed)).child(line(0.6, added)),
                    Panel::Todo => body.child(line(0.6, accent)).child(line(0.5, faint)).child(line(0.4, faint)),
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .rounded(px(2.))
                    .border_1()
                    .border_color(look.c(ui.border_strong))
                    .bg(look.c(ui.editor_bg))
                    .child(div().flex_none().px(px(2.)).py(px(2.)).flex().gap(px(2.)).children(tabs.iter().enumerate().map(|(i, _)| tab(i == *active))))
                    .child(body)
            }
        }
    }
    div().w(look.z(w)).h(look.z(h)).flex_none().child(go(&dock.root, kind, look))
}

/// The layout choices as cards (presets A/B/C, plus "Yours" when the saved
/// arrangement differs from them): each shows a structured agent's stage and
/// an Agent Terminal's. `on_pick` gets "A", "B", "C" or "yours".
pub fn layout_cards<V: 'static>(selected: &str, look: &Look, cx: &mut Context<V>, on_pick: impl Fn(&mut V, &str, &mut Context<V>) + Clone + 'static) -> Div {
    let ui = &look.theme.ui;
    let mut choices: Vec<(String, String, Dock, Dock)> = [("A", "Watch + side notes"), ("B", "One view at a time"), ("C", "Watch + drawer")]
        .iter()
        .map(|(k, label)| (k.to_string(), format!("{k} · {label}"), Dock::preset(Kind::Acp, k), Dock::preset(Kind::Terminal, k)))
        .collect();
    let (acp, term) = (Docks::get(cx, Kind::Acp), Docks::get(cx, Kind::Terminal));
    if !choices.iter().any(|(_, _, a, t)| *a == acp && *t == term) {
        choices.push(("yours".into(), "Yours (as you arranged it)".into(), acp, term));
    }
    div().flex().flex_wrap().gap(look.z(12.)).children(choices.into_iter().enumerate().map(|(i, (key, label, a, t))| {
        let on = selected == key;
        let pick = on_pick.clone();
        div()
            .id(("layout-card", i))
            .p(look.z(8.))
            .flex()
            .flex_col()
            .gap(look.z(5.))
            .rounded(look.z(6.))
            .border_1()
            .border_color(look.c(if on { ui.focus_ring } else { ui.border }))
            .when(on, |d| d.bg(look.c(ui.active_bg)))
            .hover(|d| d.bg(look.c(ui.hover_bg)))
            .cursor_pointer()
            .child(diagram(&a, Kind::Acp, look, 150., 88.))
            .child(div().flex().items_center().gap(look.z(6.)).child(diagram(&t, Kind::Terminal, look, 64., 36.)).child(div().text_size(look.text(-3.)).text_color(look.c(ui.text_muted)).child("Agent Terminal")))
            .child(div().text_size(look.text(-2.)).text_color(look.c(if on { ui.text } else { ui.text_secondary })).child(label))
            .on_click(cx.listener(move |this, _, _, cx| pick(this, &key, cx)))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Panel::*;

    #[test]
    fn presets_round_trip() {
        for kind in [Kind::Acp, Kind::Terminal] {
            for l in ["A", "B", "C"] {
                let d = Dock::preset(kind, l);
                assert_eq!(Dock::parse(kind, &d.serialize()), Some(d.clone()), "{kind:?} {l}: {}", d.serialize());
                assert!(d.contains(Session));
            }
        }
        assert_eq!(Dock::preset(Kind::Acp, "A").serialize(), "h0.660(live,browser,changes|session,todo)");
        assert_eq!(Dock::parse(Kind::Acp, "garbage("), None);
        // The agent itself is always kept; Agent Terminals have a To-do tab too.
        let t = Dock::parse(Kind::Terminal, "todo").unwrap();
        assert!(t.contains(Todo) && t.contains(Session));
        assert!(Dock::parse(Kind::Terminal, "session,browser").unwrap().contains(Browser));
    }

    #[test]
    fn move_split_close_activate() {
        let mut d = Dock::preset(Kind::Terminal, "A");
        assert!(d.is_visible(Session) && !d.contains(Live));
        // Bring Live back, then dock it to the right of the terminal.
        d.activate(Live);
        assert_eq!(d.root, Node::Pane { tabs: vec![Session, Live], active: 1 });
        d.move_panel(Live, &[], Edge::Right);
        assert_eq!(d.serialize(), "h0.500(session|live)");
        assert!(d.is_visible(Session) && d.is_visible(Live));
        // Changes into Live's pane, then back below the terminal.
        d.activate(Changes);
        assert_eq!(d.serialize(), "h0.500(session|live,*changes)");
        d.move_panel(Changes, &[false], Edge::Bottom);
        assert_eq!(d.serialize(), "h0.500(v0.500(session|changes)|live)");
        d.set_ratio(&[], 0.7);
        d.close(Live);
        assert_eq!(d.serialize(), "v0.500(session|changes)", "an emptied pane collapses");
        d.close(Session);
        assert!(d.contains(Session), "the agent can't be closed");
        // Dropping a pane's only tab on itself changes nothing.
        let before = d.clone();
        d.move_panel(Changes, &[true], Edge::Left);
        assert_eq!(d, before);
        // Moving the last tab out of a pane removes that pane.
        d.move_panel(Changes, &[false], Edge::Center);
        assert_eq!(d.root, Node::Pane { tabs: vec![Session, Changes], active: 1 });
    }

    #[test]
    fn whole_pane_moves() {
        let mut d = Dock::parse(Kind::Acp, "h0.660(live,browser,changes|session,todo)").unwrap();
        // The Live pane, dragged by its strip, below the agent's pane.
        let moved = d.pane_of(Browser);
        assert_eq!(moved, vec![Live, Browser, Changes]);
        d.move_panels(&moved, Browser, &[true], Edge::Bottom);
        assert_eq!(d.serialize(), "v0.500(session,todo|live,*browser,changes)");
        // The agent's own pane moves too: to the left of the rest.
        let moved = d.pane_of(Session);
        d.move_panels(&moved, Session, &[true], Edge::Left);
        assert_eq!(d.serialize(), "h0.500(session,todo|live,*browser,changes)");
        // A pane onto itself changes nothing; its tabs into another pane join it.
        let before = d.clone();
        d.move_panels(&d.pane_of(Todo), Todo, &[false], Edge::Right);
        assert_eq!(d, before);
        d.move_panels(&d.pane_of(Todo), Todo, &[true], Edge::Center);
        assert_eq!(d.serialize(), "live,browser,changes,session,*todo");
    }
}
