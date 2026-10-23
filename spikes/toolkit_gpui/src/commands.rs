//! The one command table: every action, its title, keys, context and tier.
//! Key bindings, the command palette and the native menu bar are all built
//! from `COMMANDS` (docs/keymap.md, docs/menus.md).

use gpui::{Action, App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType};

/// App/window/workspace commands. Handled by `Root`, so they work whatever is focused.
pub mod qur {
    gpui::actions!(
        qur,
        [
            Quit, Hide, HideOthers, ShowAll, CloseWindow, Minimize, Zoom, ToggleFullScreen,
            CommandPalette, ToggleSidebar, ToggleBottomPanel, ToggleTerminal, ZoomIn, ZoomOut,
            ZoomReset, Transparency, GoToLine, RevealInFiles, NewTerminal, NewClaudeTask,
            NextTerminal, PrevTerminal, CloseTerminal,
            // Listed in menus/palette; not implemented in the spike (greyed).
            About, CheckSetup, OpenConfig, OpenKeymap, OpenAgents, ThemePicker, VimMode,
            NewWindow, NewAgentTask, OpenFolder, OpenRecent, QuickOpen, SaveAll, RevertFile,
            FindInProject, ReplaceInProject, GitPanel, SearchPanel, PeekLayoutA, PeekLayoutB,
            PeekLayoutC, CyclePeekLayout, PeekLive, PeekTodo, PeekChat, PeekChanges, FollowAgent,
            GoToSymbol, NavBack, NavForward, RecentFile, JumpToBracket, FocusPrompt, StopAgent,
            TakeOver, NextAgent, PrevAgent, ReviewChanges, Merge, KeepWorking, DiscardTask,
            OpenWorktreeInTerminal, RevealWorktree, KeyboardShortcuts, RunDoctor, ShowLogs,
            ReportProblem, ExpandSelection, ShrinkSelection, AddNextMatch, SelectAllMatches,
            AddCursorAbove, AddCursorBelow, ToggleBlockComment, TrimWhitespace,
        ]
    );
}

/// Shared clipboard/undo actions: the Edit menu's OS items. Whatever is focused
/// (editor, terminal, text field) handles them.
pub mod edit {
    gpui::actions!(edit, [Undo, Redo, Cut, Copy, Paste, SelectAll]);
}

pub mod editor {
    gpui::actions!(
        editor,
        [
            Left, Right, Up, Down, SelectLeft, SelectRight, SelectUp, SelectDown, WordLeft,
            WordRight, SelectWordLeft, SelectWordRight, LineStart, LineEnd, SelectLineStart,
            SelectLineEnd, DocStart, DocEnd, SelectDocStart, SelectDocEnd, PageUp, PageDown,
            SelectPageUp, SelectPageDown, Backspace, Delete, DeleteWordLeft, DeleteWordRight,
            DeleteToLineStart, DeleteLine, MoveLineUp, MoveLineDown, CopyLineUp, CopyLineDown,
            InsertLineBelow, InsertLineAbove, Indent, Outdent, Tab, ShiftTab, ToggleComment,
            SelectLine, Escape, ToggleSoftWrap, Save, Newline, Find, Replace, FindNext, FindPrev,
            UseSelectionForFind,
        ]
    );
}

pub mod terminal {
    gpui::actions!(terminal, [Clear, SendLineStart, SendLineEnd, SendKillLine]);
}

/// Single-line text fields (find, replace, go to line, palette).
pub mod input {
    gpui::actions!(
        input,
        [Backspace, Delete, Left, Right, Home, End, Confirm, ConfirmAlt, ConfirmAll, Cancel, Up, Down]
    );
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    /// Implemented in the spike.
    S,
    Later(&'static str),
}

pub struct Cmd {
    pub action: fn() -> Box<dyn Action>,
    pub category: &'static str,
    pub title: &'static str,
    /// GPUI keystroke syntax; several bindings separated by `|`.
    pub keys: &'static str,
    pub context: Option<&'static str>,
    pub tier: Tier,
}

impl Cmd {
    pub fn implemented(&self) -> bool {
        self.tier == Tier::S
    }

    pub fn first_keys(&self) -> &'static str {
        self.keys.split('|').next().unwrap_or("")
    }
}

macro_rules! cmd {
    ($a:path, $cat:literal, $title:literal, $keys:literal, $ctx:expr, $tier:expr) => {
        Cmd {
            action: || Box::new($a),
            category: $cat,
            title: $title,
            keys: $keys,
            context: $ctx,
            tier: $tier,
        }
    };
}

const E: Option<&str> = Some("Editor");
const T: Option<&str> = Some("Terminal");
const I: Option<&str> = Some("TextInput");
const W: Option<&str> = None;
/// Global, except where the terminal needs the key itself (Ctrl+G, chords that
/// would shadow Cmd+K Clear).
const NT: Option<&str> = Some("!Terminal");
use Tier::{Later as L, S};

#[rustfmt::skip]
pub static COMMANDS: &[Cmd] = &[
    // App and window
    cmd!(qur::Quit, "Qur", "Quit Qur", "cmd-q", W, S),
    cmd!(qur::Hide, "Qur", "Hide Qur", "cmd-h", W, S),
    cmd!(qur::HideOthers, "Qur", "Hide Others", "cmd-alt-h", W, S),
    cmd!(qur::ShowAll, "Qur", "Show All", "", W, S),
    cmd!(qur::About, "Qur", "About Qur", "", W, L("M7")),
    cmd!(qur::CheckSetup, "Qur", "Check Setup…", "", W, L("M6")),
    cmd!(qur::OpenConfig, "Qur", "Open config.toml", "cmd-,", W, L("M1")),
    cmd!(qur::OpenKeymap, "Qur", "Open keymap.toml", "", W, L("M1")),
    cmd!(qur::OpenAgents, "Qur", "Agents (agents.toml)", "", W, L("M2")),
    cmd!(qur::ThemePicker, "Qur", "Theme…", "cmd-k cmd-t", NT, L("M1")),
    cmd!(qur::Transparency, "Window", "Transparency…", "", W, S),
    cmd!(qur::VimMode, "Qur", "Vim Mode", "", W, L("M6")),
    cmd!(qur::NewWindow, "File", "New Window", "cmd-n", W, L("M1")),
    cmd!(qur::NewAgentTask, "Agent", "New Task…", "cmd-shift-n", W, L("M2")),
    cmd!(qur::OpenFolder, "File", "Open Folder…", "cmd-o", W, L("M1")),
    cmd!(qur::OpenRecent, "File", "Open Recent", "", W, L("M1")),
    cmd!(qur::CloseWindow, "File", "Close Window", "cmd-shift-w", W, S),
    cmd!(qur::Minimize, "Window", "Minimize", "cmd-m", W, S),
    cmd!(qur::Zoom, "Window", "Zoom", "", W, S),
    cmd!(qur::ToggleFullScreen, "View", "Enter Full Screen", "ctrl-cmd-f", W, S),
    cmd!(qur::CommandPalette, "View", "Command Palette…", "cmd-shift-p|f1", W, S),
    cmd!(qur::ToggleSidebar, "View", "Sidebar", "cmd-b", W, S),
    cmd!(qur::ToggleBottomPanel, "View", "Bottom Panel", "cmd-j", W, S),
    cmd!(qur::ZoomIn, "View", "Zoom In", "cmd-=|cmd-+", W, S),
    cmd!(qur::ZoomOut, "View", "Zoom Out", "cmd--", W, S),
    cmd!(qur::ZoomReset, "View", "Reset Zoom", "cmd-0", W, S),
    // Files and navigation
    cmd!(qur::QuickOpen, "Go", "Go to File…", "cmd-p", W, L("M1")),
    cmd!(qur::RecentFile, "Go", "Next Recent File", "ctrl-tab", NT, L("M1")),
    cmd!(editor::Save, "File", "Save", "cmd-s", E, S),
    cmd!(qur::SaveAll, "File", "Save All", "cmd-alt-s", W, L("M1")),
    cmd!(qur::RevertFile, "File", "Revert File", "", W, L("M1")),
    cmd!(qur::RevealInFiles, "View", "Files Dropdown", "cmd-shift-e", W, S),
    cmd!(qur::GoToLine, "Go", "Go to Line…", "ctrl-g", NT, S),
    cmd!(qur::NavBack, "Go", "Back", "ctrl--", NT, L("M1")),
    cmd!(qur::NavForward, "Go", "Forward", "ctrl-shift--", NT, L("M1")),
    cmd!(qur::GoToSymbol, "Go", "Go to Symbol in File…", "cmd-shift-o", W, L("M1")),
    cmd!(qur::JumpToBracket, "Go", "Jump to Matching Bracket", "cmd-shift-\\", W, L("M1")),
    // Editing
    cmd!(edit::Undo, "Edit", "Undo", "cmd-z", W, S),
    cmd!(edit::Redo, "Edit", "Redo", "cmd-shift-z", W, S),
    cmd!(edit::Cut, "Edit", "Cut", "cmd-x", W, S),
    cmd!(edit::Copy, "Edit", "Copy", "cmd-c", W, S),
    cmd!(edit::Paste, "Edit", "Paste", "cmd-v", W, S),
    cmd!(edit::SelectAll, "Edit", "Select All", "cmd-a", W, S),
    cmd!(editor::Left, "Editor", "Move Left", "left", E, S),
    cmd!(editor::Right, "Editor", "Move Right", "right", E, S),
    cmd!(editor::Up, "Editor", "Move Up", "up", E, S),
    cmd!(editor::Down, "Editor", "Move Down", "down", E, S),
    cmd!(editor::SelectLeft, "Editor", "Select Left", "shift-left", E, S),
    cmd!(editor::SelectRight, "Editor", "Select Right", "shift-right", E, S),
    cmd!(editor::SelectUp, "Editor", "Select Up", "shift-up", E, S),
    cmd!(editor::SelectDown, "Editor", "Select Down", "shift-down", E, S),
    cmd!(editor::WordLeft, "Editor", "Word Left", "alt-left", E, S),
    cmd!(editor::WordRight, "Editor", "Word Right", "alt-right", E, S),
    cmd!(editor::SelectWordLeft, "Editor", "Select Word Left", "alt-shift-left", E, S),
    cmd!(editor::SelectWordRight, "Editor", "Select Word Right", "alt-shift-right", E, S),
    cmd!(editor::LineStart, "Editor", "Line Start", "cmd-left|home", E, S),
    cmd!(editor::LineEnd, "Editor", "Line End", "cmd-right|end", E, S),
    cmd!(editor::SelectLineStart, "Editor", "Select to Line Start", "cmd-shift-left|shift-home", E, S),
    cmd!(editor::SelectLineEnd, "Editor", "Select to Line End", "cmd-shift-right|shift-end", E, S),
    cmd!(editor::DocStart, "Editor", "File Start", "cmd-up", E, S),
    cmd!(editor::DocEnd, "Editor", "File End", "cmd-down", E, S),
    cmd!(editor::SelectDocStart, "Editor", "Select to File Start", "cmd-shift-up", E, S),
    cmd!(editor::SelectDocEnd, "Editor", "Select to File End", "cmd-shift-down", E, S),
    cmd!(editor::PageUp, "Editor", "Page Up", "pageup", E, S),
    cmd!(editor::PageDown, "Editor", "Page Down", "pagedown", E, S),
    cmd!(editor::SelectPageUp, "Editor", "Select Page Up", "shift-pageup", E, S),
    cmd!(editor::SelectPageDown, "Editor", "Select Page Down", "shift-pagedown", E, S),
    cmd!(editor::Backspace, "Editor", "Delete Left", "backspace|shift-backspace", E, S),
    cmd!(editor::Delete, "Editor", "Delete Right", "delete", E, S),
    cmd!(editor::DeleteWordLeft, "Editor", "Delete Word Left", "alt-backspace", E, S),
    cmd!(editor::DeleteWordRight, "Editor", "Delete Word Right", "alt-delete", E, S),
    cmd!(editor::DeleteToLineStart, "Editor", "Delete to Line Start", "cmd-backspace", E, S),
    cmd!(editor::DeleteLine, "Selection", "Delete Line", "cmd-shift-k", E, S),
    cmd!(editor::MoveLineUp, "Selection", "Move Line Up", "alt-up", E, S),
    cmd!(editor::MoveLineDown, "Selection", "Move Line Down", "alt-down", E, S),
    cmd!(editor::CopyLineUp, "Selection", "Copy Line Up", "alt-shift-up", E, S),
    cmd!(editor::CopyLineDown, "Selection", "Copy Line Down", "alt-shift-down", E, S),
    cmd!(editor::InsertLineBelow, "Selection", "Insert Line Below", "cmd-enter", E, S),
    cmd!(editor::InsertLineAbove, "Selection", "Insert Line Above", "cmd-shift-enter", E, S),
    cmd!(editor::Indent, "Edit", "Indent", "cmd-]", E, S),
    cmd!(editor::Outdent, "Edit", "Outdent", "cmd-[", E, S),
    cmd!(editor::Tab, "Editor", "Tab", "tab", E, S),
    cmd!(editor::ShiftTab, "Editor", "Outdent (Tab)", "shift-tab", E, S),
    cmd!(editor::ToggleComment, "Edit", "Toggle Line Comment", "cmd-/", E, S),
    cmd!(qur::ToggleBlockComment, "Edit", "Toggle Block Comment", "shift-alt-a", W, L("M1")),
    cmd!(editor::SelectLine, "Selection", "Select Line", "cmd-l", E, S),
    cmd!(qur::ExpandSelection, "Selection", "Expand Selection", "", W, L("M1")),
    cmd!(qur::ShrinkSelection, "Selection", "Shrink Selection", "", W, L("M1")),
    cmd!(qur::AddNextMatch, "Selection", "Add Next Match", "cmd-d", W, L("M1")),
    cmd!(qur::SelectAllMatches, "Selection", "Select All Matches", "cmd-shift-l", W, L("M1")),
    cmd!(qur::AddCursorAbove, "Selection", "Add Cursor Above", "cmd-alt-up", W, L("M1")),
    cmd!(qur::AddCursorBelow, "Selection", "Add Cursor Below", "cmd-alt-down", W, L("M1")),
    cmd!(qur::TrimWhitespace, "Edit", "Trim Trailing Whitespace", "cmd-k cmd-x", NT, L("M1")),
    cmd!(editor::Escape, "Editor", "Collapse Selection / Close Find", "escape", E, S),
    cmd!(editor::ToggleSoftWrap, "View", "Soft Wrap", "alt-z", W, S),
    cmd!(editor::Newline, "Editor", "Newline", "enter|shift-enter", E, S),
    // Find
    cmd!(editor::Find, "Edit", "Find", "cmd-f", W, S),
    cmd!(editor::Replace, "Edit", "Replace", "cmd-alt-f", W, S),
    cmd!(editor::FindNext, "Edit", "Find Next", "cmd-g", W, S),
    cmd!(editor::FindPrev, "Edit", "Find Previous", "cmd-shift-g", W, S),
    cmd!(editor::UseSelectionForFind, "Edit", "Use Selection for Find", "cmd-e", W, S),
    cmd!(qur::FindInProject, "Edit", "Find in Project", "cmd-shift-f", W, L("M6")),
    cmd!(qur::ReplaceInProject, "Edit", "Replace in Project", "cmd-shift-h", W, L("M6")),
    // Terminal
    cmd!(qur::NewTerminal, "Terminal", "New Terminal", "cmd-t", W, S),
    cmd!(qur::NewClaudeTask, "Terminal", "New Claude Terminal", "cmd-shift-t", W, S),
    cmd!(qur::CloseTerminal, "Terminal", "Close Terminal", "cmd-w", T, S),
    cmd!(qur::ToggleTerminal, "View", "Terminal", "ctrl-`", W, S),
    cmd!(terminal::Clear, "Terminal", "Clear", "cmd-k", T, S),
    cmd!(qur::PrevTerminal, "Terminal", "Previous Terminal", "cmd-shift-[|cmd-{", W, S),
    cmd!(qur::NextTerminal, "Terminal", "Next Terminal", "cmd-shift-]|cmd-}", W, S),
    cmd!(terminal::SendLineStart, "Terminal", "Line Start", "cmd-left", T, S),
    cmd!(terminal::SendLineEnd, "Terminal", "Line End", "cmd-right", T, S),
    cmd!(terminal::SendKillLine, "Terminal", "Kill Line", "cmd-backspace", T, S),
    // Panels
    cmd!(qur::GitPanel, "View", "Git", "ctrl-shift-g", W, L("M4")),
    cmd!(qur::SearchPanel, "View", "Search", "", W, L("M6")),
    // Agents and peek
    cmd!(qur::FocusPrompt, "Agent", "Focus Prompt", "cmd-i", W, L("M2")),
    cmd!(qur::StopAgent, "Agent", "Stop", "cmd-.", W, L("M2")),
    cmd!(qur::TakeOver, "Agent", "Take Over", "", W, L("M3")),
    cmd!(qur::NextAgent, "Agent", "Next Agent", "", W, L("M2")),
    cmd!(qur::PrevAgent, "Agent", "Previous Agent", "", W, L("M2")),
    cmd!(qur::ReviewChanges, "Agent", "Review Changes", "", W, L("M4")),
    cmd!(qur::Merge, "Agent", "Merge…", "", W, L("M4")),
    cmd!(qur::KeepWorking, "Agent", "Keep Working", "", W, L("M4")),
    cmd!(qur::DiscardTask, "Agent", "Discard Task…", "", W, L("M4")),
    cmd!(qur::OpenWorktreeInTerminal, "Agent", "Open Task Worktree in Terminal", "", W, L("M2")),
    cmd!(qur::RevealWorktree, "Agent", "Reveal Worktree in Finder", "", W, L("M2")),
    cmd!(qur::CyclePeekLayout, "View", "Cycle Peek Layout", "cmd-k cmd-l", NT, L("M3")),
    cmd!(qur::PeekLayoutA, "View", "A Watch + Side Notes", "", W, L("M3")),
    cmd!(qur::PeekLayoutB, "View", "B One View at a Time", "", W, L("M3")),
    cmd!(qur::PeekLayoutC, "View", "C Watch + Drawer", "", W, L("M3")),
    cmd!(qur::PeekLive, "View", "Live", "cmd-1", W, L("M3")),
    cmd!(qur::PeekTodo, "View", "To-do", "cmd-2", W, L("M3")),
    cmd!(qur::PeekChat, "View", "Chat", "cmd-3", W, L("M3")),
    cmd!(qur::PeekChanges, "View", "Changes", "cmd-4", W, L("M3")),
    cmd!(qur::FollowAgent, "View", "Follow Agent", "", W, L("M3")),
    // Help
    cmd!(qur::KeyboardShortcuts, "Help", "Qur Keyboard Shortcuts", "", W, L("M7")),
    cmd!(qur::RunDoctor, "Help", "Run Doctor", "", W, L("M7")),
    cmd!(qur::ShowLogs, "Help", "Show Logs in Finder", "", W, L("M7")),
    cmd!(qur::ReportProblem, "Help", "Report a Problem…", "", W, L("M7")),
    // Palette is always reachable from the menu, so CommandPalette is enough.
    cmd!(input::Backspace, "Input", "Delete Left", "backspace", I, S),
    cmd!(input::Delete, "Input", "Delete Right", "delete", I, S),
    cmd!(input::Left, "Input", "Left", "left", I, S),
    cmd!(input::Right, "Input", "Right", "right", I, S),
    cmd!(input::Home, "Input", "Start", "cmd-left|home", I, S),
    cmd!(input::End, "Input", "End", "cmd-right|end", I, S),
    cmd!(input::Confirm, "Input", "Confirm", "enter", I, S),
    cmd!(input::ConfirmAlt, "Input", "Confirm (Previous)", "shift-enter", I, S),
    cmd!(input::ConfirmAll, "Input", "Confirm All", "cmd-enter", I, S),
    cmd!(input::Cancel, "Input", "Cancel", "escape", I, S),
    cmd!(input::Up, "Input", "Up", "up", I, S),
    cmd!(input::Down, "Input", "Down", "down", I, S),
];

pub fn find(action: &dyn Action) -> Option<&'static Cmd> {
    COMMANDS.iter().find(|c| (c.action)().partial_eq(action))
}

/// Commands shown in the palette (not the low-level cursor/input ones).
pub fn palette_commands() -> impl Iterator<Item = &'static Cmd> {
    COMMANDS
        .iter()
        .filter(|c| !matches!(c.category, "Editor" | "Input") || c.title == "Soft Wrap")
}

pub fn bind_keys(cx: &mut App) {
    let mut bindings = Vec::new();
    for c in COMMANDS {
        for keys in c.keys.split('|').filter(|k| !k.is_empty()) {
            let predicate = c
                .context
                .map(|ctx| gpui::KeyBindingContextPredicate::parse(ctx).unwrap().into());
            bindings.push(KeyBinding::load(keys, (c.action)(), predicate, false, None, cx).unwrap());
        }
    }
    cx.bind_keys(bindings);
}

/// `cmd-shift-p` -> `⇧⌘P` (macOS menu order: ⌃⌥⇧⌘).
pub fn display_keys(keys: &str) -> String {
    keys.split(' ')
        .map(|stroke| {
            let parts: Vec<&str> = stroke.split('-').collect();
            // Handle `cmd--` (minus key).
            let (mods, key) = if stroke.ends_with("--") {
                (&parts[..parts.len() - 2], "-")
            } else {
                (&parts[..parts.len() - 1], parts[parts.len() - 1])
            };
            let mut out = String::new();
            for (m, sym) in [("ctrl", "⌃"), ("alt", "⌥"), ("shift", "⇧"), ("cmd", "⌘")] {
                if mods.contains(&m) {
                    out.push_str(sym);
                }
            }
            out.push_str(&match key {
                "enter" => "↩".into(),
                "backspace" => "⌫".into(),
                "delete" => "⌦".into(),
                "escape" => "⎋".into(),
                "left" => "←".into(),
                "right" => "→".into(),
                "up" => "↑".into(),
                "down" => "↓".into(),
                "tab" => "⇥".into(),
                "pageup" => "PgUp".into(),
                "pagedown" => "PgDn".into(),
                "home" => "Home".into(),
                "end" => "End".into(),
                k => k.to_uppercase(),
            });
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// State reflected by checkmarks in the menu bar. Menus are rebuilt when it changes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MenuState {
    pub sidebar: bool,
    pub bottom_panel: bool,
    pub soft_wrap: bool,
}

fn item(action: impl Action) -> MenuItem {
    let cmd = find(&action).expect("menu action is in COMMANDS");
    MenuItem::action(cmd.title, action).disabled(!cmd.implemented())
}

fn titled(title: &'static str, action: impl Action) -> MenuItem {
    let implemented = find(&action).map(|c| c.implemented()).unwrap_or(false);
    MenuItem::action(title, action).disabled(!implemented)
}

pub fn menus(s: MenuState) -> Vec<Menu> {
    use MenuItem as M;
    let sep = M::separator;
    vec![
        Menu::new("Qur").items([
            titled("About Qur", qur::About),
            item(qur::CheckSetup),
            sep(),
            M::submenu(Menu::new("Settings").items([
                item(qur::OpenConfig),
                item(qur::OpenKeymap),
                item(qur::OpenAgents),
            ])),
            item(qur::ThemePicker),
            item(qur::Transparency),
            item(qur::VimMode).checked(false),
            sep(),
            M::os_submenu("Services", SystemMenuType::Services),
            sep(),
            item(qur::Hide),
            item(qur::HideOthers),
            item(qur::ShowAll),
            sep(),
            item(qur::Quit),
        ]),
        Menu::new("File").items([
            titled("New Agent Task…", qur::NewAgentTask),
            item(qur::NewTerminal),
            item(qur::NewClaudeTask),
            item(qur::NewWindow),
            sep(),
            item(qur::OpenFolder),
            M::submenu(Menu::new("Open Recent").items([item(qur::OpenRecent)]).disabled(true)),
            titled("Quick Open…", qur::QuickOpen),
            sep(),
            item(editor::Save),
            item(qur::SaveAll),
            item(qur::RevertFile),
            sep(),
            item(qur::CloseTerminal),
            item(qur::CloseWindow),
        ]),
        Menu::new("Edit").items([
            M::os_action("Undo", edit::Undo, OsAction::Undo),
            M::os_action("Redo", edit::Redo, OsAction::Redo),
            sep(),
            M::os_action("Cut", edit::Cut, OsAction::Cut),
            M::os_action("Copy", edit::Copy, OsAction::Copy),
            M::os_action("Paste", edit::Paste, OsAction::Paste),
            M::os_action("Select All", edit::SelectAll, OsAction::SelectAll),
            sep(),
            item(editor::Find),
            item(editor::Replace),
            item(editor::FindNext),
            item(editor::FindPrev),
            item(editor::UseSelectionForFind),
            item(qur::FindInProject),
            item(qur::ReplaceInProject),
            sep(),
            item(editor::ToggleComment),
            item(qur::ToggleBlockComment),
            item(editor::Indent),
            item(editor::Outdent),
            item(qur::TrimWhitespace),
        ]),
        Menu::new("Selection").items([
            item(editor::SelectLine),
            item(qur::ExpandSelection),
            item(qur::ShrinkSelection),
            sep(),
            item(editor::CopyLineUp),
            item(editor::CopyLineDown),
            item(editor::MoveLineUp),
            item(editor::MoveLineDown),
            item(editor::DeleteLine),
            item(editor::InsertLineBelow),
            item(editor::InsertLineAbove),
            sep(),
            item(qur::AddNextMatch),
            item(qur::SelectAllMatches),
            item(qur::AddCursorAbove),
            item(qur::AddCursorBelow),
        ]),
        Menu::new("View").items([
            item(qur::CommandPalette),
            sep(),
            item(qur::ToggleSidebar).checked(s.sidebar),
            item(qur::RevealInFiles),
            item(qur::ToggleBottomPanel).checked(s.bottom_panel),
            item(qur::ToggleTerminal),
            item(qur::GitPanel),
            item(qur::SearchPanel),
            sep(),
            M::submenu(Menu::new("Peek Layout").items([
                item(qur::PeekLayoutA),
                item(qur::PeekLayoutB),
                item(qur::PeekLayoutC),
                item(qur::CyclePeekLayout),
            ])),
            M::submenu(Menu::new("Peek").items([
                item(qur::PeekLive),
                item(qur::PeekTodo),
                item(qur::PeekChat),
                item(qur::PeekChanges),
            ])),
            item(qur::FollowAgent).checked(false),
            sep(),
            item(editor::ToggleSoftWrap).checked(s.soft_wrap),
            item(qur::ZoomIn),
            item(qur::ZoomOut),
            item(qur::ZoomReset),
            item(qur::Transparency),
            sep(),
            item(qur::ToggleFullScreen),
        ]),
        Menu::new("Go").items([
            item(qur::QuickOpen),
            item(qur::GoToLine),
            item(qur::GoToSymbol),
            sep(),
            item(qur::NavBack),
            item(qur::NavForward),
            item(qur::RecentFile),
            item(qur::JumpToBracket),
        ]),
        Menu::new("Agent").items([
            item(qur::NewAgentTask),
            item(qur::FocusPrompt),
            item(qur::StopAgent),
            item(qur::TakeOver),
            sep(),
            item(qur::NextAgent),
            item(qur::PrevAgent),
            sep(),
            item(qur::ReviewChanges),
            item(qur::Merge),
            item(qur::KeepWorking),
            item(qur::DiscardTask),
            sep(),
            item(qur::OpenWorktreeInTerminal),
            item(qur::RevealWorktree),
        ]),
        Menu::new("Terminal").items([
            item(qur::NewTerminal),
            item(qur::NewClaudeTask),
            item(qur::NextTerminal),
            item(qur::PrevTerminal),
            item(terminal::Clear),
            item(qur::CloseTerminal),
        ]),
        Menu::new("Window").items([item(qur::Minimize), item(qur::Zoom)]),
        Menu::new("Help").items([
            item(qur::KeyboardShortcuts),
            item(qur::RunDoctor),
            item(qur::ShowLogs),
            item(qur::ReportProblem),
            sep(),
            titled("About Qur", qur::About),
        ]),
    ]
}
