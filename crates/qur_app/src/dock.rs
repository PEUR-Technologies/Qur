//! Rearrangeable stage (owner: "restructure the terminal, viewing window and
//! stuff, like photoshop or vscode"). The agent stage is a tree of panes; each
//! pane holds tabs (Chat/Terminal, Live, Browser, Changes, To-do). Drag a tab
//! onto a pane's edge to split it there or onto its middle to join it; drag a
//! divider to resize; close a tab to tuck it away (the pane's + brings it
//! back). One arrangement for structured agents and one for Agent Terminals,
//! saved in config.toml (`[layout] acp` / `terminal`). The old peek layouts
//! A/B/C are presets.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, Bounds, Context, Div, Global, MouseButton, Pixels, Render, SharedString, Window, canvas, div, prelude::*, px};

use crate::look::Look;

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
            Kind::Terminal => &[Panel::Session, Panel::Live, Panel::Changes],
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
            (Kind::Terminal, "B") => pane(&[Session, Live, Changes]),
            (Kind::Terminal, "C") => split(true, 0.6, pane(&[Live, Changes]), pane(&[Session])),
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
        let Some(Node::Pane { tabs: target_tabs, .. }) = self.node(target) else { return };
        // Dropping a pane's only tab on its own edges or middle changes nothing.
        if target_tabs == &vec![p] {
            return;
        }
        // Mark the target before removing (removal can reshape the tree).
        let marker = Panel::Todo;
        let _ = marker;
        let snapshot_target: Vec<Panel> = target_tabs.clone();
        self.remove(p);
        // Find the target again by its (unchanged) remaining tabs.
        let target = self.find_pane_with(&snapshot_target, p).unwrap_or_default();
        let Some(node) = self.node_mut(&target) else { return };
        match edge {
            Edge::Center => {
                if let Node::Pane { tabs, active } = node {
                    tabs.push(p);
                    *active = tabs.len() - 1;
                }
            }
            _ => {
                let old = std::mem::replace(node, pane(&[]));
                let new = pane(&[p]);
                let vertical = matches!(edge, Edge::Top | Edge::Bottom);
                *node = if matches!(edge, Edge::Left | Edge::Top) { split(vertical, 0.5, new, old) } else { split(vertical, 0.5, old, new) };
            }
        }
        self.normalize();
    }

    fn find_pane_with(&self, tabs: &[Panel], moved: Panel) -> Option<Path> {
        let want: Vec<Panel> = tabs.iter().copied().filter(|t| *t != moved).collect();
        if want.is_empty() {
            // The target only held the moved tab: it's now empty; find the empty pane.
            return self.first_pane(|t| t.is_empty());
        }
        self.find(want[0])
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

/// What's being dragged: a tab of the given dock.
#[derive(Clone)]
pub struct DragTab {
    pub kind: Kind,
    pub panel: Panel,
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

/// Draw `kind`'s dock, asking `content` for each visible panel. Generic over
/// the view so the panels' own listeners keep working (AgentTask, Workspace).
pub fn render<V: 'static>(kind: Kind, look: &Look, cx: &mut Context<V>, content: &mut dyn FnMut(Panel, &mut Context<V>) -> gpui::AnyElement) -> gpui::AnyElement {
    let dock = Docks::get(cx, kind);
    let bounds: Bnds = Rc::new(RefCell::new(HashMap::new()));
    let root = node(kind, &dock.root, Vec::new(), look, cx, content, &bounds);
    div()
        .id(("dock", kind as usize))
        .size_full()
        .flex()
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
        .child(root)
        .into_any_element()
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
            let handle = div()
                .id(SharedString::from(format!("split-{kind:?}-{path:?}")))
                .flex_none()
                .bg(look.c(ui.border))
                .hover(|d| d.bg(look.c(ui.focus_ring)))
                .when(*vertical, |d| d.h(px(1.)).w_full().cursor_row_resize())
                .when(!*vertical, |d| d.w(px(1.)).h_full().cursor_col_resize())
                // A wider grab area than the 1 px line.
                .child(div().absolute().when(*vertical, |d| d.left_0().right_0().top(px(-3.)).h(px(7.))).when(!*vertical, |d| d.top_0().bottom_0().left(px(-3.)).w(px(7.))))
                .relative()
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
        Node::Pane { tabs, active } => {
            let current = tabs.get(*active).copied();
            let hidden: Vec<Panel> = kind.panels().iter().copied().filter(|p| !Docks::get(cx, kind).contains(*p)).collect();
            let strip = div()
                .flex_none()
                .h(look.z(26.))
                .px(look.z(6.))
                .flex()
                .items_center()
                .gap(look.z(2.))
                .border_b_1()
                .border_color(look.c(ui.border))
                .children(tabs.iter().enumerate().map(|(i, &p)| {
                    let on = i == *active;
                    let label: SharedString = kind.label(p).into();
                    let drag = DragTab { kind, panel: p, label: label.clone() };
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
                        .when(p != Panel::Session, |d| {
                            d.child(
                                div()
                                    .id(SharedString::from(format!("tab-close-{kind:?}-{p:?}")))
                                    .invisible()
                                    .group_hover("tab", |d| d.visible())
                                    .text_color(look.c(ui.text_muted))
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
                    // Tucked-away panels come back here, into this pane.
                    let target = path.clone();
                    d.children(hidden.iter().map(|&p| {
                        let target = target.clone();
                        div()
                            .id(SharedString::from(format!("add-{kind:?}-{p:?}-{target:?}")))
                            .px(look.z(5.))
                            .rounded(look.z(3.))
                            .cursor_pointer()
                            .text_size(look.text(-3.))
                            .text_color(look.c(ui.text_muted))
                            .hover(|d| d.bg(look.c(ui.hover_bg)).text_color(look.c(ui.text_secondary)))
                            .child(format!("+ {}", kind.label(p)))
                            .on_click(move |_, _, cx| {
                                Docks::update(cx, kind, |d| {
                                    d.add(p);
                                    d.move_panel(p, &target, Edge::Center);
                                });
                            })
                    }))
                });
            let body = div().flex_1().min_h(px(0.)).relative().children(current.map(|p| content(p, cx)));
            let zones = drop_zones(kind, path.clone(), look);
            div().size_full().flex().flex_col().relative().child(strip).child(body.child(zones)).into_any_element()
        }
    }
}

/// Drop targets over a pane while a tab is dragged: edges split, the middle
/// joins. Invisible (and click-through) otherwise.
fn drop_zones(kind: Kind, path: Path, look: &Look) -> Div {
    let hl = {
        let mut c = look.c(look.theme.ui.focus_ring);
        c.a = 0.22;
        c
    };
    let zone = |edge: Edge, path: Path| {
        let d = div().absolute().drag_over::<DragTab>(move |s, _, _, _| s.bg(hl)).on_drop(move |t: &DragTab, _, cx| {
            if t.kind == kind {
                let (panel, path) = (t.panel, path.clone());
                Docks::update(cx, kind, |d| d.move_panel(panel, &path, edge));
            }
        });
        match edge {
            Edge::Left => d.left_0().top_0().bottom_0().w(gpui::relative(0.25)),
            Edge::Right => d.right_0().top_0().bottom_0().w(gpui::relative(0.25)),
            Edge::Top => d.top_0().left(gpui::relative(0.25)).right(gpui::relative(0.25)).h(gpui::relative(0.3)),
            Edge::Bottom => d.bottom_0().left(gpui::relative(0.25)).right(gpui::relative(0.25)).h(gpui::relative(0.3)),
            Edge::Center => d.top(gpui::relative(0.3)).bottom(gpui::relative(0.3)).left(gpui::relative(0.25)).right(gpui::relative(0.25)),
        }
    };
    div()
        .absolute()
        .size_full()
        .top_0()
        .left_0()
        .children([Edge::Left, Edge::Right, Edge::Top, Edge::Bottom, Edge::Center].map(|e| zone(e, path.clone())))
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
        // Unknown or foreign panels drop out; the agent itself is always kept.
        let t = Dock::parse(Kind::Terminal, "browser,todo").unwrap();
        assert_eq!(t.root, pane(&[Session]));
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
}
