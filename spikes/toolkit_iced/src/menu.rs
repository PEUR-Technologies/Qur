//! Native macOS menu bar (docs/menus.md) via `muda`. Item ids are command ids;
//! clicks and accelerators arrive as `Message::Command` through a channel.

use std::collections::HashMap;

use iced::futures::channel::mpsc::UnboundedSender;
use muda::accelerator::KeyAccelerator;
use muda::{CheckMenuItemBuilder, IsMenuItem, Menu, MenuEvent, MenuItemBuilder, PredefinedMenuItem, Submenu};

use crate::commands::{self, Tier};

enum Entry {
    Cmd(&'static str),
    Check(&'static str),
    Sep,
    Sub(&'static str, Vec<Entry>),
    /// AppKit-handled item (Hide, Services, …).
    Native(fn() -> PredefinedMenuItem),
}

use Entry::{Check, Cmd, Native, Sep, Sub};

fn layout() -> Vec<(&'static str, Vec<Entry>)> {
    vec![
        ("Qur", vec![
            Cmd("about"), Cmd("check_setup"), Sep,
            Sub("Settings", vec![Cmd("open_config"), Cmd("open_keymap"), Cmd("open_agents")]),
            Cmd("theme"), Cmd("transparency"), Check("vim_mode"), Sep,
            Native(|| PredefinedMenuItem::services(None)), Sep,
            Native(|| PredefinedMenuItem::hide(Some("Hide Qur"))),
            Native(|| PredefinedMenuItem::hide_others(None)),
            Native(|| PredefinedMenuItem::show_all(None)), Sep,
            Cmd("quit"),
        ]),
        ("File", vec![
            Cmd("new_task"), Cmd("new_terminal"), Cmd("new_claude_terminal"), Cmd("new_window"), Sep,
            Cmd("open_folder"), Sub("Open Recent", vec![]), Cmd("quick_open"), Sep,
            Cmd("save"), Cmd("save_all"), Cmd("revert"), Sep,
            Cmd("close_terminal"), Cmd("close_window"),
        ]),
        ("Edit", vec![
            Cmd("undo"), Cmd("redo"), Sep,
            Cmd("cut"), Cmd("copy"), Cmd("paste"), Cmd("select_all"), Sep,
            Cmd("find"), Cmd("replace"), Cmd("find_next"), Cmd("find_prev"), Cmd("find_use_selection"), Cmd("replace_all"),
            Cmd("find_in_project"), Cmd("replace_in_project"), Sep,
            Cmd("toggle_comment"), Cmd("toggle_block_comment"), Cmd("indent"), Cmd("outdent"), Cmd("trim_whitespace"), Sep,
            Native(|| PredefinedMenuItem::start_dictation(None)),
            Native(|| PredefinedMenuItem::emoji_and_symbols(None)),
        ]),
        ("Selection", vec![
            Cmd("select_line"), Cmd("expand_selection"), Cmd("shrink_selection"), Sep,
            Cmd("copy_line_up"), Cmd("copy_line_down"), Cmd("move_line_up"), Cmd("move_line_down"), Cmd("delete_line"),
            Cmd("insert_line_below"), Cmd("insert_line_above"), Sep,
            Cmd("add_next_match"), Cmd("select_all_matches"), Cmd("add_cursor_above"), Cmd("add_cursor_below"),
        ]),
        ("View", vec![
            Cmd("palette"), Sep,
            Check("toggle_sidebar"), Cmd("reveal_files"), Sep,
            Check("toggle_panel"), Cmd("toggle_terminal"), Cmd("git_panel"), Sep,
            Sub("Peek Layout", vec![Check("peek_layout_a"), Check("peek_layout_b"), Check("peek_layout_c"), Cmd("peek_cycle")]),
            Sub("Peek", vec![Cmd("peek_live"), Cmd("peek_todo"), Cmd("peek_chat"), Cmd("peek_changes")]),
            Check("follow_agent"), Sep,
            Check("soft_wrap"), Cmd("zoom_in"), Cmd("zoom_out"), Cmd("zoom_reset"), Sep,
            Cmd("fullscreen"),
        ]),
        ("Go", vec![
            Cmd("quick_open"), Cmd("goto_line"), Cmd("goto_symbol"), Sep,
            Cmd("nav_back"), Cmd("nav_forward"), Cmd("recent_file"), Cmd("matching_bracket"),
        ]),
        ("Agent", vec![
            Cmd("new_task"), Cmd("focus_prompt"), Cmd("stop_agent"), Cmd("take_over"), Sep,
            Cmd("next_agent"), Cmd("prev_agent"), Cmd("agent_n"), Sep,
            Cmd("peek_changes"), Cmd("merge"), Cmd("keep_working"), Cmd("discard_task"), Sep,
            Cmd("open_task_terminal"), Cmd("reveal_worktree"),
        ]),
        ("Terminal", vec![
            Cmd("new_terminal"), Cmd("new_claude_terminal"), Cmd("next_terminal"), Cmd("prev_terminal"),
            Cmd("clear_terminal"), Cmd("close_terminal"),
        ]),
        ("Window", vec![Cmd("minimize"), Cmd("zoom_window"), Sep, Native(|| PredefinedMenuItem::bring_all_to_front(None))]),
        ("Help", vec![Cmd("shortcuts"), Cmd("doctor"), Cmd("show_logs"), Cmd("report_problem"), Sep, Cmd("about")]),
    ]
}

/// Accelerator for a command: its first single-step chord. Unimplemented
/// items only get one if no spike command uses the same keys.
fn accel(id: &str) -> Option<&'static str> {
    let c = commands::get(id);
    let chord = c.keys.iter().find(|k| !k.contains(' ') && !k.starts_with("fn+"))?;
    if c.tier != Tier::S {
        let taken = commands::COMMANDS.iter().any(|o| o.tier == Tier::S && o.keys.contains(chord));
        if taken {
            return None;
        }
    }
    chord.parse::<KeyAccelerator>().is_ok().then_some(*chord)
}

pub struct NativeMenu {
    _menu: Menu,
    checks: HashMap<&'static str, Vec<muda::CheckMenuItem>>,
}

impl NativeMenu {
    /// Must run on the main thread after the app finished launching (winit
    /// installs its default menu at launch and would replace ours).
    pub fn install(tx: UnboundedSender<String>) -> Self {
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.unbounded_send(e.id.0);
        }));
        let menu = Menu::new();
        let mut checks: HashMap<&'static str, Vec<muda::CheckMenuItem>> = HashMap::new();
        let mut window_menu = None;
        let mut help_menu = None;
        for (title, entries) in layout() {
            let sub = Submenu::new(title, true);
            build(&sub, &entries, &mut checks);
            let _ = menu.append(&sub);
            match title {
                "Window" => window_menu = Some(sub),
                "Help" => help_menu = Some(sub),
                _ => {}
            }
        }
        menu.init_for_nsapp();
        if let Some(w) = window_menu {
            w.set_as_windows_menu_for_nsapp();
        }
        if let Some(h) = help_menu {
            h.set_as_help_menu_for_nsapp();
        }
        NativeMenu { _menu: menu, checks }
    }

    pub fn set_checked(&self, id: &str, on: bool) {
        for c in self.checks.get(id).into_iter().flatten() {
            c.set_checked(on);
        }
    }
}

fn build(
    sub: &Submenu,
    entries: &[Entry],
    checks: &mut HashMap<&'static str, Vec<muda::CheckMenuItem>>,
) {
    for e in entries {
        let item: Box<dyn IsMenuItem> = match e {
            Sep => Box::new(PredefinedMenuItem::separator()),
            Native(f) => Box::new(f()),
            Sub(title, kids) => {
                let s = Submenu::new(*title, !kids.is_empty());
                build(&s, kids, checks);
                Box::new(s)
            }
            Cmd(id) | Check(id) => {
                let c = commands::get(id);
                let enabled = c.tier == Tier::S;
                let a = accel(id);
                if matches!(e, Check(_)) {
                    let mut b = CheckMenuItemBuilder::new().id(c.id.into()).text(c.title).enabled(enabled);
                    if let Some(a) = a {
                        b = b.key_accelerator(Some(a)).expect("validated accelerator");
                    }
                    let item = b.build();
                    checks.entry(c.id).or_default().push(item.clone());
                    Box::new(item)
                } else {
                    let mut b = MenuItemBuilder::new().id(c.id.into()).text(c.title).enabled(enabled);
                    if let Some(a) = a {
                        b = b.key_accelerator(Some(a)).expect("validated accelerator");
                    }
                    Box::new(b.build())
                }
            }
        };
        let _ = sub.append(item.as_ref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s_tier_menu_items_get_accelerators() {
        fn walk(entries: &[Entry], out: &mut Vec<&'static str>) {
            for e in entries {
                match e {
                    Cmd(id) | Check(id) => out.push(id),
                    Sub(_, kids) => walk(kids, out),
                    _ => {}
                }
            }
        }
        let mut ids = Vec::new();
        for (_, entries) in layout() {
            walk(&entries, &mut ids);
        }
        for id in ids {
            let c = commands::get(id);
            if c.tier == Tier::S && !c.keys.is_empty() {
                assert!(accel(id).is_some(), "{id} {:?}", c.keys);
            }
        }
    }
}
