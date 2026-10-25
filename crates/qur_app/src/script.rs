//! The `QUR_DEBUG_ACTIONS` driver: scripted runs for tests and screenshots.
//! Only runs when that variable is set (`first_frame`); otherwise the only
//! cost is `mark`'s check of a cached flag.
//!
//! Steps, comma-separated, run in order with 200 ms between them:
//! - `qur::Name` dispatches an action; `wait:<ms>` pauses.
//! - `type:<text>` types text as key presses (`\,` is a comma, `\n` Enter, `\t` Tab, `\\` a backslash).
//! - `key:<keystrokes>` presses keys, space-separated, as in keymaps: `key:cmd-shift-p`, `key:escape d d`, `key:cmd-\,` (Cmd+,).
//! - `click:<at>`, `dclick:` (double), `rclick:` (right), `cmdclick:` (with Cmd).
//! - `drag:<at>,<at>` presses, moves there in small steps (a frame apart), and releases.
//! - `move:<at>` moves the mouse there (no button), as hovering does.
//! - `scroll:<at>,<dy>` turns the wheel over a point by `dy` points (negative scrolls down).
//! - `state` prints a `STATE …` line: overlay, code modal, terminal find, git panel, docks.
//! - `marks` prints a `BOUNDS name x,y wxh` line for every marked element on screen.
//!
//! `<at>` is a point in the window, in points from the top-left of its
//! content: `x,y`, or a marked element (`mark`) by name: `@tab:Terminal:Live`
//! is its middle, and `@name~fx~fy` the point at those fractions of its size
//! (`@pane:Terminal:root~0.9~0.5` is near its right edge; `@term-cell:1~4.5~1.5`
//! is the middle of column 4, row 1, as that mark is one terminal cell).
//!
//! Each input step draws the window first, so focus changes and layout since
//! the last frame count even when macOS isn't drawing the (covered) window.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use gpui::{
    AnyWindowHandle, App, Bounds, IntoElement, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, OperatingSystem, Pixels, PlatformInput, Point, Styled, Window, canvas, point, px,
};

use crate::workspace::Workspace;

/// Scripted run? (Cached: `mark` asks on every render.)
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("QUR_DEBUG_ACTIONS").is_some())
}

thread_local! {
    static MARKS: RefCell<BTreeMap<String, Bounds<Pixels>>> = const { RefCell::new(BTreeMap::new()) };
}

/// In scripted runs, an invisible child that records its parent's bounds
/// under `name` each frame, so steps can aim at it (`@name`). Nothing otherwise.
pub fn mark(name: impl FnOnce() -> String) -> Option<impl IntoElement> {
    on().then(|| {
        let name = name();
        canvas(move |b, _, _| record(|| name, b), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
    })
}

/// Record bounds measured elsewhere (e.g. in a custom element's prepaint).
pub fn record(name: impl FnOnce() -> String, b: Bounds<Pixels>) {
    if on() {
        MARKS.with(|m| m.borrow_mut().insert(name(), b));
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum At {
    Xy(f32, f32),
    /// A mark, and where in it (fractions of its size).
    Mark(String, f32, f32),
}

#[derive(Debug, PartialEq)]
pub enum Step {
    Wait(u64),
    Action(String),
    Type(String),
    Keys(String),
    Click { at: At, button: MouseButton, count: usize, cmd: bool },
    Drag { from: At, to: At },
    Move(At),
    Scroll { at: At, dy: f32 },
    State,
    Marks,
}

/// Split on commas (not `\,`), then let `click:`/`drag:` take their points.
pub fn parse(list: &str) -> Result<Vec<Step>, String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut chars = list.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                cur.push('\\');
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            ',' => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    let mut parts = parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    let mut steps = Vec::new();
    while let Some(p) = parts.next() {
        let (head, rest) = p.split_once(':').unwrap_or((p.as_str(), ""));
        let click = |at, button, count, cmd| Step::Click { at, button, count, cmd };
        steps.push(match head {
            "wait" => Step::Wait(rest.parse().map_err(|_| format!("{p}: not a number of ms"))?),
            "type" => Step::Type(unescape(rest)),
            "key" | "keys" => Step::Keys(unescape(rest)),
            "click" => click(at(&p, rest, &mut parts)?, MouseButton::Left, 1, false),
            "dclick" => click(at(&p, rest, &mut parts)?, MouseButton::Left, 2, false),
            "rclick" => click(at(&p, rest, &mut parts)?, MouseButton::Right, 1, false),
            "cmdclick" => click(at(&p, rest, &mut parts)?, MouseButton::Left, 1, true),
            "drag" => {
                let from = at(&p, rest, &mut parts)?;
                let next = parts.next().ok_or_else(|| format!("{p}: needs a second point"))?;
                Step::Drag { from, to: at(&p, &next, &mut parts)? }
            }
            "move" => Step::Move(at(&p, rest, &mut parts)?),
            "scroll" => {
                let at = at(&p, rest, &mut parts)?;
                let dy = parts.next().ok_or_else(|| format!("{p}: needs an amount"))?;
                Step::Scroll { at, dy: dy.parse().map_err(|_| format!("{p}: {dy}: not a number"))? }
            }
            "state" => Step::State,
            "marks" => Step::Marks,
            _ => Step::Action(p.clone()),
        });
    }
    Ok(steps)
}

/// One point: `@mark[~fx~fy]`, or `x,y` (the y is the next part).
fn at(p: &str, first: &str, parts: &mut impl Iterator<Item = String>) -> Result<At, String> {
    if let Some(m) = first.strip_prefix('@') {
        let mut it = m.split('~');
        let name = it.next().unwrap_or_default().to_string();
        let f = |v: Option<&str>| v.map(|v| v.parse::<f32>().map_err(|_| format!("{p}: {v}: not a fraction"))).unwrap_or(Ok(0.5));
        return Ok(At::Mark(name, f(it.next())?, f(it.next())?));
    }
    let x = first.parse().map_err(|_| format!("{p}: {first}: not a number"))?;
    let y = parts.next().ok_or_else(|| format!("{p}: needs x,y"))?;
    Ok(At::Xy(x, y.parse().map_err(|_| format!("{p}: {y}: not a number"))?))
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

/// One typed character as macOS reports it: the key, Shift for capitals.
fn char_keystroke(c: char) -> Keystroke {
    let (key, shift) = match c {
        ' ' => ("space".to_string(), false),
        '\n' => ("enter".to_string(), false),
        '\t' => ("tab".to_string(), false),
        c if c.is_ascii_uppercase() => (c.to_ascii_lowercase().to_string(), true),
        c => (c.to_string(), false),
    };
    Keystroke { modifiers: Modifiers { shift, ..Default::default() }, key, key_char: Some(c.to_string()) }
}

/// Draw now, so the dispatch tree, hitboxes, input handler and marks match
/// the latest state (a covered window gets no frames from macOS).
fn fresh(window: &mut Window, cx: &mut App) {
    MARKS.with(|m| m.borrow_mut().clear());
    window.draw(cx).clear(cx);
}

/// Where a point is, after a fresh draw. None (and a `MISS` line) for a mark
/// that isn't on screen.
fn resolve(handle: AnyWindowHandle, cx: &mut gpui::AsyncApp, at: &At) -> Option<Point<Pixels>> {
    let _ = handle.update(cx, |_, window, cx| fresh(window, cx));
    match at {
        At::Xy(x, y) => Some(point(px(*x), px(*y))),
        At::Mark(name, fx, fy) => {
            let b = MARKS.with(|m| m.borrow().get(name).copied());
            if b.is_none() {
                let near: Vec<String> = MARKS.with(|m| m.borrow().keys().filter(|k| k.split(':').next() == name.split(':').next()).cloned().collect());
                println!("MISS {name} (marks like it: {})", near.join(" "));
            }
            b.map(|b| point(b.left() + b.size.width * *fx, b.top() + b.size.height * *fy))
        }
    }
}

fn mouse(handle: AnyWindowHandle, cx: &mut gpui::AsyncApp, ev: PlatformInput) {
    let _ = handle.update(cx, |_, window, cx| {
        fresh(window, cx);
        window.dispatch_event(ev, cx);
    });
}

pub fn run(handle: AnyWindowHandle, list: &str, cx: &mut App) {
    let steps = match parse(list) {
        Ok(s) => s,
        Err(e) => return eprintln!("QUR_DEBUG_ACTIONS: {e}"),
    };
    let bg = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        let pause = |ms| bg.timer(Duration::from_millis(ms));
        for step in steps {
            if let Step::Wait(ms) = step {
                pause(ms).await;
                continue;
            }
            pause(200).await;
            let t = crate::START.get().map(|s| s.elapsed().as_millis()).unwrap_or(0);
            match step {
                Step::Action(name) => {
                    println!("ACTION {name} t={t}ms");
                    let _ = handle.update(cx, |root, window, cx| match cx.build_action(&name, None) {
                        // macOS stops drawing a fully covered window, so a view
                        // focused since the last frame (e.g. the code modal) isn't in
                        // the dispatch tree yet and the action would go nowhere.
                        // Scripted runs usually sit behind other windows: fall back
                        // to the workspace's own node.
                        Ok(a) => match root.downcast::<Workspace>().map(|ws| ws.read(cx).root_focus()) {
                            Ok(ws) if !ws.contains_focused(window, cx) => ws.dispatch_action(a.as_ref(), window, cx),
                            _ => window.dispatch_action(a, cx),
                        },
                        Err(e) => eprintln!("QUR_DEBUG_ACTIONS: {name}: {e}"),
                    });
                }
                Step::Type(text) => {
                    println!("INPUT type {text:?} t={t}ms");
                    for c in text.chars() {
                        let _ = handle.update(cx, |_, window, cx| {
                            fresh(window, cx);
                            window.dispatch_keystroke(char_keystroke(c), cx);
                        });
                        pause(15).await;
                    }
                }
                Step::Keys(keys) => {
                    println!("INPUT key {keys} t={t}ms");
                    for k in keys.split_whitespace() {
                        match Keystroke::parse_with_os(k, OperatingSystem::Mac) {
                            Ok(ks) => {
                                let _ = handle.update(cx, |_, window, cx| {
                                    fresh(window, cx);
                                    window.dispatch_keystroke(ks, cx);
                                });
                            }
                            Err(e) => eprintln!("QUR_DEBUG_ACTIONS: key {k}: {e}"),
                        }
                        pause(30).await;
                    }
                }
                Step::Click { at, button, count, cmd } => {
                    let Some(pos) = resolve(handle, cx, &at) else { continue };
                    println!("INPUT click {at:?} at {:.0},{:.0} {button:?} x{count}{} t={t}ms", f32::from(pos.x), f32::from(pos.y), if cmd { " cmd" } else { "" });
                    let modifiers = Modifiers { platform: cmd, ..Default::default() };
                    mouse(handle, cx, PlatformInput::MouseMove(MouseMoveEvent { position: pos, pressed_button: None, modifiers }));
                    pause(30).await;
                    for n in 1..=count {
                        mouse(handle, cx, PlatformInput::MouseDown(MouseDownEvent { button, position: pos, modifiers, click_count: n, first_mouse: false }));
                        mouse(handle, cx, PlatformInput::MouseUp(MouseUpEvent { button, position: pos, modifiers, click_count: n }));
                        pause(30).await;
                    }
                }
                Step::Drag { from, to } => {
                    let (Some(a), Some(b)) = (resolve(handle, cx, &from), resolve(handle, cx, &to)) else { continue };
                    println!("INPUT drag {from:?} -> {to:?} at {:.0},{:.0} -> {:.0},{:.0} t={t}ms", f32::from(a.x), f32::from(a.y), f32::from(b.x), f32::from(b.y));
                    let at = |f: f32| point(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f);
                    let (m, left) = (Modifiers::default(), MouseButton::Left);
                    mouse(handle, cx, PlatformInput::MouseMove(MouseMoveEvent { position: a, pressed_button: None, modifiers: m }));
                    pause(30).await;
                    mouse(handle, cx, PlatformInput::MouseDown(MouseDownEvent { button: left, position: a, modifiers: m, click_count: 1, first_mouse: false }));
                    const STEPS: usize = 12;
                    for i in 1..=STEPS {
                        pause(20).await;
                        let p = at(i as f32 / STEPS as f32);
                        mouse(handle, cx, PlatformInput::MouseMove(MouseMoveEvent { position: p, pressed_button: Some(left), modifiers: m }));
                    }
                    pause(30).await;
                    mouse(handle, cx, PlatformInput::MouseUp(MouseUpEvent { button: left, position: b, modifiers: m, click_count: 1 }));
                }
                Step::Move(at) => {
                    let Some(pos) = resolve(handle, cx, &at) else { continue };
                    println!("INPUT move {at:?} at {:.0},{:.0} t={t}ms", f32::from(pos.x), f32::from(pos.y));
                    mouse(handle, cx, PlatformInput::MouseMove(MouseMoveEvent { position: pos, pressed_button: None, modifiers: Modifiers::default() }));
                }
                Step::Scroll { at, dy } => {
                    let Some(pos) = resolve(handle, cx, &at) else { continue };
                    println!("INPUT scroll {at:?} at {:.0},{:.0} by {dy} t={t}ms", f32::from(pos.x), f32::from(pos.y));
                    let m = Modifiers::default();
                    mouse(handle, cx, PlatformInput::MouseMove(MouseMoveEvent { position: pos, pressed_button: None, modifiers: m }));
                    pause(30).await;
                    let delta = gpui::ScrollDelta::Pixels(point(px(0.), px(dy)));
                    mouse(handle, cx, PlatformInput::Scroll(gpui::ScrollEvent { position: pos, delta, modifiers: m, touch_phase: gpui::TouchPhase::Moved, source: Default::default() }));
                }
                Step::State => {
                    let _ = handle.update(cx, |root, window, cx| {
                        fresh(window, cx);
                        if let Ok(ws) = root.downcast::<Workspace>() {
                            println!("STATE t={t}ms {}", ws.read(cx).debug_state(cx));
                        }
                    });
                }
                Step::Marks => {
                    let _ = handle.update(cx, |_, window, cx| fresh(window, cx));
                    MARKS.with(|m| {
                        for (name, b) in m.borrow().iter() {
                            let f = |p: Pixels| f32::from(p);
                            println!("BOUNDS {name} {:.0},{:.0} {:.0}x{:.0}", f(b.left()), f(b.top()), f(b.size.width), f(b.size.height));
                        }
                    });
                }
                Step::Wait(_) => {}
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steps() {
        let s = parse("qur::Find, wait:50, type:a\\, b\\n, key:cmd-shift-p escape, click:10,20, drag:1,2,@tab:X~0.9~0.1, rclick:@agent:tt-1, scroll:@list,-120, state, marks").unwrap();
        assert_eq!(
            s,
            vec![
                Step::Action("qur::Find".into()),
                Step::Wait(50),
                Step::Type("a, b\n".into()),
                Step::Keys("cmd-shift-p escape".into()),
                Step::Click { at: At::Xy(10., 20.), button: MouseButton::Left, count: 1, cmd: false },
                Step::Drag { from: At::Xy(1., 2.), to: At::Mark("tab:X".into(), 0.9, 0.1) },
                Step::Click { at: At::Mark("agent:tt-1".into(), 0.5, 0.5), button: MouseButton::Right, count: 1, cmd: false },
                Step::Scroll { at: At::Mark("list".into(), 0.5, 0.5), dy: -120. },
                Step::State,
                Step::Marks,
            ]
        );
        assert!(parse("click:1").is_err());
        assert!(parse("drag:1,2").is_err());
        assert!(parse("scroll:@list").is_err());
        assert!(parse("click:@a~x").is_err());
    }

    #[test]
    fn capitals_are_shifted() {
        let k = char_keystroke('G');
        assert_eq!((k.key.as_str(), k.modifiers.shift, k.key_char.as_deref()), ("g", true, Some("G")));
        assert_eq!(char_keystroke(' ').key, "space");
    }
}
