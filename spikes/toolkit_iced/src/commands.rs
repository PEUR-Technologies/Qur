//! The one command table (docs/keymap.md + docs/menus.md). Keys, the native
//! menu and the command palette all read from here.

use iced::keyboard::{key::Named, Key, Modifiers};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctx {
    /// Works wherever focus is.
    Global,
    Editor,
    Terminal,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    S,
    M1,
    M2,
    M3,
    M4,
    M6,
    M7,
}

pub struct Command {
    pub id: &'static str,
    pub title: &'static str,
    /// muda/keymap syntax: `cmd+shift+p`; a space separates chord steps.
    pub keys: &'static [&'static str],
    pub ctx: Ctx,
    pub tier: Tier,
    /// Listed in the command palette (plain cursor keys are not).
    pub palette: bool,
}

const fn c(id: &'static str, title: &'static str, keys: &'static [&'static str], ctx: Ctx, tier: Tier) -> Command {
    Command { id, title, keys, ctx, tier, palette: true }
}

/// Cursor motion: bound, routed, but kept out of the palette.
const fn m(id: &'static str, title: &'static str, keys: &'static [&'static str]) -> Command {
    Command { id, title, keys, ctx: Ctx::Editor, tier: Tier::S, palette: false }
}

use Ctx::{Editor as E, Global as G, Terminal as T};
use Tier::*;

pub const COMMANDS: &[Command] = &[
    // App and window
    c("quit", "Quit Qur", &["cmd+q"], G, S),
    c("hide", "Hide Qur", &["cmd+h"], G, S),
    c("hide_others", "Hide Others", &["cmd+alt+h"], G, S),
    c("show_all", "Show All", &[], G, S),
    c("about", "About Qur", &[], G, M7),
    c("check_setup", "Check Setup…", &[], G, M6),
    c("open_config", "Open config.toml", &["cmd+,"], G, M1),
    c("open_keymap", "Open keymap.toml", &[], G, M1),
    c("open_agents", "Agents (agents.toml)", &[], G, M2),
    c("theme", "Theme…", &["cmd+k cmd+t"], G, M1),
    c("transparency", "Window: Transparency…", &[], G, S),
    c("vim_mode", "Vim Mode", &[], G, M6),
    c("new_window", "New Window", &["cmd+n"], G, M1),
    c("open_folder", "Open Folder…", &["cmd+o"], G, M1),
    c("open_recent", "Open Recent", &[], G, M1),
    c("close_window", "Close Window", &["cmd+shift+w"], G, S),
    c("minimize", "Minimize", &["cmd+m"], G, S),
    c("zoom_window", "Zoom", &[], G, S),
    c("fullscreen", "Toggle Full Screen", &["ctrl+cmd+f"], G, S),
    c("palette", "Command Palette", &["cmd+shift+p", "f1"], G, S),
    c("toggle_sidebar", "Toggle Sidebar", &["cmd+b"], G, S),
    c("toggle_panel", "Toggle Bottom Panel", &["cmd+j"], G, S),
    c("zoom_in", "Zoom In", &["cmd+="], G, S),
    c("zoom_out", "Zoom Out", &["cmd+-"], G, S),
    c("zoom_reset", "Reset Zoom", &["cmd+0"], G, S),
    // Files and navigation
    c("quick_open", "Quick Open…", &["cmd+p"], G, M1),
    c("recent_file", "Next Recent File", &["ctrl+tab"], G, M1),
    c("recent_file_prev", "Previous Recent File", &["ctrl+shift+tab"], G, M1),
    c("save", "Save", &["cmd+s"], E, S),
    c("save_all", "Save All", &["cmd+alt+s"], G, M1),
    c("revert", "Revert File", &[], E, M1),
    c("reveal_files", "Reveal in Files", &["cmd+shift+e"], G, S),
    c("goto_line", "Go to Line…", &["ctrl+g"], E, S),
    c("nav_back", "Back", &["ctrl+-"], G, M1),
    c("nav_forward", "Forward", &["ctrl+shift+-"], G, M1),
    c("goto_symbol", "Go to Symbol in File…", &["cmd+shift+o"], E, M1),
    c("matching_bracket", "Jump to Matching Bracket", &["shift+cmd+\\"], E, M1),
    // Editing
    c("undo", "Undo", &["cmd+z"], E, S),
    c("redo", "Redo", &["cmd+shift+z"], E, S),
    c("cut", "Cut", &["cmd+x"], E, S),
    c("copy", "Copy", &["cmd+c"], E, S),
    c("paste", "Paste", &["cmd+v"], E, S),
    c("select_all", "Select All", &["cmd+a"], E, S),
    c("delete_word_left", "Delete Word Left", &["alt+backspace"], E, S),
    c("delete_word_right", "Delete Word Right", &["alt+delete"], E, S),
    c("delete_to_line_start", "Delete to Line Start", &["cmd+backspace"], E, S),
    c("delete_line", "Delete Line", &["cmd+shift+k"], E, S),
    c("move_line_up", "Move Line Up", &["alt+up"], E, S),
    c("move_line_down", "Move Line Down", &["alt+down"], E, S),
    c("copy_line_up", "Copy Line Up", &["shift+alt+up"], E, S),
    c("copy_line_down", "Copy Line Down", &["shift+alt+down"], E, S),
    c("insert_line_below", "Insert Line Below", &["cmd+enter"], E, S),
    c("insert_line_above", "Insert Line Above", &["cmd+shift+enter"], E, S),
    c("indent", "Indent", &["cmd+]"], E, S),
    c("outdent", "Outdent", &["cmd+["], E, S),
    c("tab", "Indent / Insert Tab", &["tab"], E, S),
    c("shift_tab", "Outdent", &["shift+tab"], E, S),
    c("toggle_comment", "Toggle Line Comment", &["cmd+/"], E, S),
    c("toggle_block_comment", "Toggle Block Comment", &["shift+alt+a"], E, M1),
    c("select_line", "Select Line", &["cmd+l"], E, S),
    c("expand_selection", "Expand Selection", &[], E, M1),
    c("shrink_selection", "Shrink Selection", &[], E, M1),
    c("add_next_match", "Add Selection to Next Match", &["cmd+d"], E, M1),
    c("select_all_matches", "Select All Matches", &["cmd+shift+l"], E, M1),
    c("add_cursor_above", "Add Cursor Above", &["cmd+alt+up"], E, M1),
    c("add_cursor_below", "Add Cursor Below", &["cmd+alt+down"], E, M1),
    c("escape", "Collapse Selection / Close Find", &["escape"], G, S),
    c("trim_whitespace", "Trim Trailing Whitespace", &["cmd+k cmd+x"], E, M1),
    c("soft_wrap", "Toggle Soft Wrap", &["alt+z"], E, S),
    // Cursor motion (Shift extends)
    m("left", "Cursor Left", &["left"]),
    m("right", "Cursor Right", &["right"]),
    m("up", "Cursor Up", &["up"]),
    m("down", "Cursor Down", &["down"]),
    m("select_left", "Select Left", &["shift+left"]),
    m("select_right", "Select Right", &["shift+right"]),
    m("select_up", "Select Up", &["shift+up"]),
    m("select_down", "Select Down", &["shift+down"]),
    c("word_left", "Cursor Word Left", &["alt+left"], E, S),
    c("word_right", "Cursor Word Right", &["alt+right"], E, S),
    m("select_word_left", "Select Word Left", &["shift+alt+left"]),
    m("select_word_right", "Select Word Right", &["shift+alt+right"]),
    c("line_start", "Cursor Line Start", &["cmd+left", "home"], E, S),
    c("line_end", "Cursor Line End", &["cmd+right", "end"], E, S),
    m("select_line_start", "Select to Line Start", &["shift+cmd+left", "shift+home"]),
    m("select_line_end", "Select to Line End", &["shift+cmd+right", "shift+end"]),
    c("doc_start", "Cursor File Start", &["cmd+up"], E, S),
    c("doc_end", "Cursor File End", &["cmd+down"], E, S),
    m("select_doc_start", "Select to File Start", &["shift+cmd+up"]),
    m("select_doc_end", "Select to File End", &["shift+cmd+down"]),
    c("page_up", "Page Up", &["pageup", "fn+up"], E, S),
    c("page_down", "Page Down", &["pagedown", "fn+down"], E, S),
    m("select_page_up", "Select Page Up", &["shift+pageup"]),
    m("select_page_down", "Select Page Down", &["shift+pagedown"]),
    m("newline", "Newline", &["enter"]),
    m("backspace", "Delete Left", &["backspace"]),
    m("delete", "Delete Right", &["delete"]),
    // Find
    c("find", "Find", &["cmd+f"], G, S),
    c("replace", "Replace", &["cmd+alt+f"], G, S),
    c("find_next", "Find Next", &["cmd+g"], G, S),
    c("find_prev", "Find Previous", &["cmd+shift+g"], G, S),
    c("find_use_selection", "Use Selection for Find", &["cmd+e"], E, S),
    c("replace_all", "Replace All", &[], G, S),
    c("toggle_case", "Toggle Match Case", &["alt+cmd+c"], G, M1),
    c("toggle_whole_word", "Toggle Whole Word", &["alt+cmd+w"], G, M1),
    c("toggle_regex", "Toggle Regex", &["alt+cmd+r"], G, M1),
    c("find_in_project", "Find in Project", &["cmd+shift+f"], G, M6),
    c("replace_in_project", "Replace in Project", &["cmd+shift+h"], G, M6),
    // Terminal
    c("new_terminal", "New Terminal", &["cmd+t"], G, S),
    c("new_claude_terminal", "New Claude Terminal", &["cmd+shift+t"], G, S),
    c("close_terminal", "Close Terminal", &["cmd+w"], T, S),
    c("toggle_terminal", "Toggle Terminal", &["ctrl+`"], G, S),
    c("clear_terminal", "Clear Terminal", &["cmd+k"], T, S),
    c("prev_terminal", "Previous Terminal", &["cmd+shift+["], G, S),
    c("next_terminal", "Next Terminal", &["cmd+shift+]"], G, S),
    // Panels
    c("git_panel", "Git Panel", &["ctrl+shift+g"], G, M4),
    // Agents and peek
    c("new_task", "New Agent Task…", &["cmd+shift+n"], G, M2),
    c("focus_prompt", "Focus Prompt", &["cmd+i"], G, M2),
    c("stop_agent", "Stop Agent", &["cmd+."], G, M2),
    c("take_over", "Take Over", &[], G, M3),
    c("next_agent", "Next Agent", &[], G, M2),
    c("prev_agent", "Previous Agent", &[], G, M2),
    c("agent_n", "Agent 1–9", &["ctrl+1"], G, M2),
    c("peek_layout_a", "Peek Layout A: Watch + side notes", &[], G, M3),
    c("peek_layout_b", "Peek Layout B: One view at a time", &[], G, M3),
    c("peek_layout_c", "Peek Layout C: Watch + drawer", &[], G, M3),
    c("peek_cycle", "Cycle Peek Layout", &["cmd+k cmd+l"], G, M3),
    c("peek_live", "Peek: Live", &["cmd+1"], G, M3),
    c("peek_todo", "Peek: To-do", &["cmd+2"], G, M3),
    c("peek_chat", "Peek: Chat", &["cmd+3"], G, M3),
    c("peek_changes", "Peek: Changes", &["cmd+4"], G, M3),
    c("follow_agent", "Follow Agent", &[], G, M3),
    c("merge", "Merge…", &["cmd+shift+enter"], G, M4),
    c("keep_working", "Keep Working", &[], G, M4),
    c("discard_task", "Discard Task…", &[], G, M4),
    c("open_task_terminal", "Open Task Worktree in Terminal", &[], G, M2),
    c("reveal_worktree", "Reveal Worktree in Finder", &[], G, M2),
    // Help
    c("shortcuts", "Qur Keyboard Shortcuts", &[], G, M1),
    c("doctor", "Run Doctor", &[], G, M7),
    c("show_logs", "Show Logs in Finder", &[], G, M1),
    c("report_problem", "Report a Problem…", &[], G, M1),
];

pub fn get(id: &str) -> &'static Command {
    COMMANDS.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("unknown command {id}"))
}

// ---------------------------------------------------------------------------
// Key matching
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct KeyPress {
    pub key: Key,
    /// Layout-independent base character (Shift not applied).
    pub latin: Option<char>,
    pub mods: Modifiers,
    pub text: Option<String>,
}

#[derive(PartialEq, Eq, Debug)]
enum KeySpec {
    Char(char),
    Named(Named),
}

struct Chord {
    cmd: bool,
    shift: bool,
    alt: bool,
    ctrl: bool,
    key: KeySpec,
}

fn parse(chord: &str) -> Option<Chord> {
    if chord.contains(' ') {
        return None; // multi-step chords are M1
    }
    let (mods, key) = match chord.rfind('+') {
        Some(i) if i + 1 < chord.len() => (&chord[..i], &chord[i + 1..]),
        _ => ("", chord),
    };
    let mut ch = Chord { cmd: false, shift: false, alt: false, ctrl: false, key: KeySpec::Char(' ') };
    for m in mods.split('+').filter(|m| !m.is_empty()) {
        match m {
            "cmd" => ch.cmd = true,
            "shift" => ch.shift = true,
            "alt" => ch.alt = true,
            "ctrl" => ch.ctrl = true,
            "fn" => {}
            _ => return None,
        }
    }
    ch.key = match key {
        "left" => KeySpec::Named(Named::ArrowLeft),
        "right" => KeySpec::Named(Named::ArrowRight),
        "up" => KeySpec::Named(Named::ArrowUp),
        "down" => KeySpec::Named(Named::ArrowDown),
        "enter" => KeySpec::Named(Named::Enter),
        "backspace" => KeySpec::Named(Named::Backspace),
        "delete" => KeySpec::Named(Named::Delete),
        "tab" => KeySpec::Named(Named::Tab),
        "escape" => KeySpec::Named(Named::Escape),
        "home" => KeySpec::Named(Named::Home),
        "end" => KeySpec::Named(Named::End),
        "pageup" => KeySpec::Named(Named::PageUp),
        "pagedown" => KeySpec::Named(Named::PageDown),
        "f1" => KeySpec::Named(Named::F1),
        k if k.chars().count() == 1 => KeySpec::Char(k.chars().next()?),
        _ => return None,
    };
    // `fn+up` is PageUp on Mac keyboards.
    if mods.contains("fn") {
        ch.key = match ch.key {
            KeySpec::Named(Named::ArrowUp) => KeySpec::Named(Named::PageUp),
            KeySpec::Named(Named::ArrowDown) => KeySpec::Named(Named::PageDown),
            k => k,
        };
    }
    Some(ch)
}

fn matches(chord: &Chord, k: &KeyPress) -> bool {
    let m = k.mods;
    if chord.cmd != m.logo() || chord.shift != m.shift() || chord.alt != m.alt() || chord.ctrl != m.control() {
        return false;
    }
    match (&chord.key, &k.key) {
        (KeySpec::Named(n), Key::Named(kn)) => n == kn,
        (KeySpec::Char(c), _) => k.latin.is_some_and(|l| l.to_ascii_lowercase() == *c),
        _ => false,
    }
}

/// First command in `ctxs` (in order) bound to this key press.
pub fn lookup(k: &KeyPress, ctxs: &[Ctx]) -> Option<&'static Command> {
    for ctx in ctxs {
        for cmd in COMMANDS.iter().filter(|c| c.ctx == *ctx && c.tier == Tier::S) {
            if cmd.keys.iter().filter_map(|s| parse(s)).any(|ch| matches(&ch, k)) {
                return Some(cmd);
            }
        }
    }
    None
}

/// Display form for menus/palette: `⇧⌘P`.
pub fn pretty(keys: &str) -> String {
    keys.split(' ')
        .map(|step| {
            let mut out = String::new();
            let (mods, key) = match step.rfind('+') {
                Some(i) if i + 1 < step.len() => (&step[..i], &step[i + 1..]),
                _ => ("", step),
            };
            for (m, s) in [("ctrl", "⌃"), ("alt", "⌥"), ("shift", "⇧"), ("cmd", "⌘"), ("fn", "fn ")] {
                if mods.split('+').any(|x| x == m) {
                    out.push_str(s);
                }
            }
            out.push_str(&match key {
                "left" => "←".into(),
                "right" => "→".into(),
                "up" => "↑".into(),
                "down" => "↓".into(),
                "enter" => "↩".into(),
                "backspace" => "⌫".into(),
                "delete" => "⌦".into(),
                "escape" => "esc".into(),
                "tab" => "⇥".into(),
                k => k.to_uppercase(),
            });
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Subsequence fuzzy score (higher is better), `None` if no match.
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut ti = 0;
    let mut prev = None;
    for qc in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = (ti..t.len()).find(|&i| t[i] == qc)?;
        score += if prev == Some(found.wrapping_sub(1)) { 5 } else { 1 };
        if found == 0 || !t[found - 1].is_alphanumeric() {
            score += 3;
        }
        prev = Some(found);
        ti = found + 1;
    }
    Some(score - t.len() as i32 / 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kp(key: Key, latin: Option<char>, mods: Modifiers) -> KeyPress {
        KeyPress { key, latin, mods, text: None }
    }

    #[test]
    fn every_s_tier_chord_parses() {
        for c in COMMANDS.iter().filter(|c| c.tier == Tier::S) {
            for k in c.keys {
                assert!(parse(k).is_some(), "{} {}", c.id, k);
            }
        }
    }

    #[test]
    fn routes_by_context() {
        let cmd_k = kp(Key::Character("k".into()), Some('k'), Modifiers::LOGO);
        assert_eq!(lookup(&cmd_k, &[Ctx::Terminal, Ctx::Global]).unwrap().id, "clear_terminal");
        assert!(lookup(&cmd_k, &[Ctx::Editor, Ctx::Global]).is_none());
        let up = kp(Key::Named(Named::ArrowUp), None, Modifiers::ALT | Modifiers::SHIFT);
        assert_eq!(lookup(&up, &[Ctx::Editor]).unwrap().id, "copy_line_down".replace("down", "up"));
        let pal = kp(Key::Character("P".into()), Some('p'), Modifiers::LOGO | Modifiers::SHIFT);
        assert_eq!(lookup(&pal, &[Ctx::Editor, Ctx::Global]).unwrap().id, "palette");
    }

    #[test]
    fn fuzzy_ranks_prefix_words() {
        assert!(fuzzy("tsb", "Toggle Sidebar").is_some());
        assert!(fuzzy("zzz", "Toggle Sidebar").is_none());
    }
}
