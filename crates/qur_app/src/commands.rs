//! The one command table: every action, its title, keys, context and tier.
//! Key bindings, the command palette and the native menu bar are all built
//! from `COMMANDS` (docs/keymap.md, docs/menus.md).

use std::path::PathBuf;

use gpui::{Action, App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType};

/// App/window/workspace commands, handled by the workspace whatever is focused.
pub mod qur {
    gpui::actions!(
        qur,
        [
            Quit, Hide, HideOthers, ShowAll, CloseWindow, Minimize, Zoom, ToggleFullScreen,
            CommandPalette, ToggleSidebar, ToggleTerminal, ZoomIn, ZoomOut,
            ZoomReset, Transparency, GoToLine, RevealInFiles, NewTerminal, NewAgentTerminal,
            NextTerminal, PrevTerminal, CloseTerminal, OpenConfig, ThemePicker, NewWindow,
            OpenFolder, QuickOpen, SaveAll, RevertFile, NavBack, NavForward, RecentFile,
            UseDefaultTheme, ProjectSwitcher, PeekBrowser, ToggleInspect,
            // Listed in menus/palette, built in later milestones (shown disabled).
            About, CheckSetup, OpenKeymap, OpenAgents, VimMode, NewAgentTask, FindInProject,
            ReplaceInProject, GitPanel, SearchPanel, PeekLayoutA, PeekLayoutB, PeekLayoutC,
            CyclePeekLayout, PeekLive, PeekTodo, PeekChat, PeekChanges, FollowAgent, GoToSymbol,
            FocusPrompt, FocusInput, StopAgent, TakeOver, NextAgent, PrevAgent, ReviewChanges, Merge,
            KeepWorking, DiscardTask, OpenWorktreeInTerminal, RevealWorktree, KeyboardShortcuts,
            RunDoctor, ShowLogs, ReportProblem, ExpandSelection, ShrinkSelection,
            RunChecks, RunProject, RenameAgent, AccentColor, ChooseLayout, GitStageAll, GitUnstageAll, GitCommit, NextConflict, PrevConflict, AcceptOurs, AcceptTheirs, AcceptBoth, FinishMerge, AbortMerge, Caffeinate,
        ]
    );

    /// Select session N in the sidebar (Ctrl+1..9).
    #[derive(Clone, PartialEq, gpui::Action)]
    #[action(namespace = qur, no_json)]
    pub struct SelectSession {
        pub index: usize,
    }

    /// File > Open Recent > <project>.
    #[derive(Clone, PartialEq, gpui::Action)]
    #[action(namespace = qur, no_json)]
    pub struct OpenProject {
        pub path: std::path::PathBuf,
    }
}

/// The Edit menu's OS items; whatever is focused (editor, terminal, text field) handles them.
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
            ToggleBlockComment, TrimWhitespace, SelectLine, AddNextMatch, SelectAllMatches,
            AddCursorAbove, AddCursorBelow, JumpToBracket, Escape, ToggleSoftWrap, Save, Newline,
            Find, Replace, FindNext, FindPrev, UseSelectionForFind, ToggleCase, ToggleWholeWord,
            ToggleRegex,
        ]
    );
}

/// The first-run picker's keys (Enter saves, Esc cancels).
pub mod setup {
    gpui::actions!(setup, [Done, Cancel]);
}

/// Answers for the permission bar of the selected agent.
pub mod agent {
    gpui::actions!(agent, [AllowOnce, AllowAlways, Deny, CycleMode]);
}

pub mod terminal {
    gpui::actions!(terminal, [Clear, SendLineStart, SendLineEnd, SendKillLine, SendBackTab, Find, SplitRight, SplitDown, Tile, FocusLeft, FocusRight, FocusUp, FocusDown]);
}

/// The Browser view's keys (while it, or the page in it, has focus).
pub mod browser {
    gpui::actions!(browser, [Reload, FocusAddress, Back, Forward, ZoomIn, ZoomOut, ZoomReset]);
}

/// Single-line text fields (find, replace, go to line, pickers).
pub mod input {
    gpui::actions!(
        input,
        [
            Backspace, Delete, Left, Right, Home, End, WordLeft, WordRight, DeleteWordLeft,
            DeleteToStart, Confirm, ConfirmAlt, ConfirmAll, Cancel, Up, Down, Tab
        ]
    );
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Ready,
    /// Built in a later milestone; shown disabled.
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
    pub fn ready(&self) -> bool {
        self.tier == Tier::Ready
    }

    pub fn first_keys(&self) -> &'static str {
        self.keys.split('|').next().unwrap_or("")
    }
}

macro_rules! cmd {
    ($a:path, $cat:literal, $title:literal, $keys:literal, $ctx:expr, $tier:expr) => {
        Cmd { action: || Box::new($a), category: $cat, title: $title, keys: $keys, context: $ctx, tier: $tier }
    };
}

const E: Option<&str> = Some("Editor");
const T: Option<&str> = Some("Terminal");
const I: Option<&str> = Some("TextInput");
const W: Option<&str> = None;
/// The Browser view, but not its address bar (Cmd+Left/Right edit there).
const B: Option<&str> = Some("Browser && !TextInput");
/// Global, except where the terminal needs the key itself (Ctrl+G, Ctrl+Tab,
/// chords that would shadow Cmd+K Clear).
const NT: Option<&str> = Some("!Terminal");
/// Permission answers: anywhere but where you're editing or typing a command.
const PERM: Option<&str> = Some("!Editor && !Terminal && !Picker");
use Tier::{Later as L, Ready as R};

#[rustfmt::skip]
pub static COMMANDS: &[Cmd] = &[
    // App and window
    cmd!(qur::Quit, "Qur", "Quit Qur", "cmd-q", W, R),
    cmd!(qur::Hide, "Qur", "Hide Qur", "cmd-h", W, R),
    cmd!(qur::HideOthers, "Qur", "Hide Others", "cmd-alt-h", W, R),
    cmd!(qur::ShowAll, "Qur", "Show All", "", W, R),
    cmd!(qur::About, "Qur", "About Qur", "", W, R),
    cmd!(qur::CheckSetup, "Qur", "Setup…", "", W, R),
    cmd!(qur::OpenConfig, "Qur", "Open config.toml", "cmd-,", W, R),
    cmd!(qur::OpenKeymap, "Qur", "Open keymap.toml", "", W, L("a later version")),
    cmd!(qur::OpenAgents, "Qur", "Agents (agents.toml)", "", W, R),
    cmd!(qur::ThemePicker, "Qur", "Theme…", "cmd-k cmd-t", NT, R),
    cmd!(qur::AccentColor, "Qur", "Accent Color…", "", W, R),
    cmd!(qur::UseDefaultTheme, "Qur", "Use Default Theme for This Project", "", W, R),
    cmd!(qur::Transparency, "Qur", "Transparency…", "", W, R),
    cmd!(qur::VimMode, "Qur", "Vim Mode", "", W, R),
    cmd!(qur::NewWindow, "File", "New Window", "cmd-n", W, R),
    cmd!(qur::NewAgentTask, "Agent", "New Agent Task…", "cmd-shift-n", W, R),
    cmd!(qur::OpenFolder, "File", "Open Folder…", "cmd-o", W, R),
    cmd!(qur::ProjectSwitcher, "File", "Switch Project…", "cmd-alt-o", W, R),
    cmd!(qur::CloseWindow, "File", "Close Window", "cmd-shift-w", W, R),
    cmd!(qur::Minimize, "Window", "Minimize", "cmd-m", W, R),
    cmd!(qur::Zoom, "Window", "Zoom", "", W, R),
    cmd!(qur::ToggleFullScreen, "View", "Enter Full Screen", "ctrl-cmd-f", W, R),
    cmd!(qur::CommandPalette, "View", "Command Palette…", "cmd-shift-p|f1", W, R),
    cmd!(qur::ToggleSidebar, "View", "Sidebar", "cmd-b", W, R),
    cmd!(qur::ZoomIn, "View", "Zoom In", "cmd-=|cmd-+", W, R),
    cmd!(qur::ZoomOut, "View", "Zoom Out", "cmd--", W, R),
    cmd!(qur::ZoomReset, "View", "Reset Zoom", "cmd-0", W, R),
    // Files and navigation
    cmd!(qur::QuickOpen, "Go", "Go to File…", "cmd-p", W, R),
    cmd!(qur::RecentFile, "Go", "Next Recent File", "ctrl-tab", NT, R),
    cmd!(editor::Save, "File", "Save", "cmd-s", E, R),
    cmd!(qur::SaveAll, "File", "Save All", "cmd-alt-s", W, R),
    cmd!(qur::RevertFile, "File", "Revert File", "", W, R),
    cmd!(qur::RevealInFiles, "View", "Files Dropdown", "cmd-shift-e", W, R),
    cmd!(qur::GoToLine, "Go", "Go to Line…", "ctrl-g", NT, R),
    cmd!(qur::NavBack, "Go", "Back", "ctrl--", NT, R),
    cmd!(qur::NavForward, "Go", "Forward", "ctrl-shift--", NT, R),
    cmd!(qur::GoToSymbol, "Go", "Go to Symbol in File…", "cmd-shift-o", W, L("a later version")),
    cmd!(editor::JumpToBracket, "Go", "Jump to Matching Bracket", "cmd-shift-\\", E, R),
    // Editing
    cmd!(edit::Undo, "Edit", "Undo", "cmd-z", W, R),
    cmd!(edit::Redo, "Edit", "Redo", "cmd-shift-z", W, R),
    cmd!(edit::Cut, "Edit", "Cut", "cmd-x", W, R),
    cmd!(edit::Copy, "Edit", "Copy", "cmd-c", W, R),
    cmd!(edit::Paste, "Edit", "Paste", "cmd-v", W, R),
    cmd!(edit::SelectAll, "Edit", "Select All", "cmd-a", W, R),
    cmd!(editor::Left, "Editor", "Move Left", "left", E, R),
    cmd!(editor::Right, "Editor", "Move Right", "right", E, R),
    cmd!(editor::Up, "Editor", "Move Up", "up", E, R),
    cmd!(editor::Down, "Editor", "Move Down", "down", E, R),
    cmd!(editor::SelectLeft, "Editor", "Select Left", "shift-left", E, R),
    cmd!(editor::SelectRight, "Editor", "Select Right", "shift-right", E, R),
    cmd!(editor::SelectUp, "Editor", "Select Up", "shift-up", E, R),
    cmd!(editor::SelectDown, "Editor", "Select Down", "shift-down", E, R),
    cmd!(editor::WordLeft, "Editor", "Word Left", "alt-left", E, R),
    cmd!(editor::WordRight, "Editor", "Word Right", "alt-right", E, R),
    cmd!(editor::SelectWordLeft, "Editor", "Select Word Left", "alt-shift-left", E, R),
    cmd!(editor::SelectWordRight, "Editor", "Select Word Right", "alt-shift-right", E, R),
    cmd!(editor::LineStart, "Editor", "Line Start", "cmd-left|home", E, R),
    cmd!(editor::LineEnd, "Editor", "Line End", "cmd-right|end", E, R),
    cmd!(editor::SelectLineStart, "Editor", "Select to Line Start", "cmd-shift-left|shift-home", E, R),
    cmd!(editor::SelectLineEnd, "Editor", "Select to Line End", "cmd-shift-right|shift-end", E, R),
    cmd!(editor::DocStart, "Editor", "File Start", "cmd-up", E, R),
    cmd!(editor::DocEnd, "Editor", "File End", "cmd-down", E, R),
    cmd!(editor::SelectDocStart, "Editor", "Select to File Start", "cmd-shift-up", E, R),
    cmd!(editor::SelectDocEnd, "Editor", "Select to File End", "cmd-shift-down", E, R),
    cmd!(editor::PageUp, "Editor", "Page Up", "pageup", E, R),
    cmd!(editor::PageDown, "Editor", "Page Down", "pagedown", E, R),
    cmd!(editor::SelectPageUp, "Editor", "Select Page Up", "shift-pageup", E, R),
    cmd!(editor::SelectPageDown, "Editor", "Select Page Down", "shift-pagedown", E, R),
    cmd!(editor::Backspace, "Editor", "Delete Left", "backspace|shift-backspace", E, R),
    cmd!(editor::Delete, "Editor", "Delete Right", "delete", E, R),
    cmd!(editor::DeleteWordLeft, "Editor", "Delete Word Left", "alt-backspace", E, R),
    cmd!(editor::DeleteWordRight, "Editor", "Delete Word Right", "alt-delete", E, R),
    cmd!(editor::DeleteToLineStart, "Editor", "Delete to Line Start", "cmd-backspace", E, R),
    cmd!(editor::DeleteLine, "Selection", "Delete Line", "cmd-shift-k", E, R),
    cmd!(editor::MoveLineUp, "Selection", "Move Line Up", "alt-up", E, R),
    cmd!(editor::MoveLineDown, "Selection", "Move Line Down", "alt-down", E, R),
    cmd!(editor::CopyLineUp, "Selection", "Copy Line Up", "alt-shift-up", E, R),
    cmd!(editor::CopyLineDown, "Selection", "Copy Line Down", "alt-shift-down", E, R),
    cmd!(editor::InsertLineBelow, "Selection", "Insert Line Below", "cmd-enter", E, R),
    cmd!(editor::InsertLineAbove, "Selection", "Insert Line Above", "cmd-shift-enter", E, R),
    cmd!(editor::Indent, "Edit", "Indent", "cmd-]", E, R),
    cmd!(editor::Outdent, "Edit", "Outdent", "cmd-[", E, R),
    cmd!(editor::Tab, "Editor", "Tab", "tab", E, R),
    cmd!(editor::ShiftTab, "Editor", "Outdent (Tab)", "shift-tab", E, R),
    cmd!(editor::ToggleComment, "Edit", "Toggle Line Comment", "cmd-/", E, R),
    cmd!(editor::ToggleBlockComment, "Edit", "Toggle Block Comment", "shift-alt-a", E, R),
    cmd!(editor::SelectLine, "Selection", "Select Line", "cmd-l", E, R),
    cmd!(qur::ExpandSelection, "Selection", "Expand Selection", "", W, L("a later version")),
    cmd!(qur::ShrinkSelection, "Selection", "Shrink Selection", "", W, L("a later version")),
    cmd!(editor::AddNextMatch, "Selection", "Add Next Match", "cmd-d", E, R),
    cmd!(editor::SelectAllMatches, "Selection", "Select All Matches", "cmd-shift-l", E, R),
    cmd!(editor::AddCursorAbove, "Selection", "Add Cursor Above", "cmd-alt-up", E, R),
    cmd!(editor::AddCursorBelow, "Selection", "Add Cursor Below", "cmd-alt-down", E, R),
    cmd!(editor::TrimWhitespace, "Edit", "Trim Trailing Whitespace", "cmd-k cmd-x", E, R),
    cmd!(editor::Escape, "Editor", "Collapse Selection / Close Find", "escape", E, R),
    cmd!(editor::ToggleSoftWrap, "View", "Soft Wrap", "alt-z", W, R),
    cmd!(editor::Newline, "Editor", "Newline", "enter|shift-enter", E, R),
    // Find
    cmd!(editor::Find, "Edit", "Find", "cmd-f", W, R),
    cmd!(editor::Replace, "Edit", "Replace", "cmd-alt-f", W, R),
    cmd!(editor::FindNext, "Edit", "Find Next", "cmd-g", W, R),
    cmd!(editor::FindPrev, "Edit", "Find Previous", "cmd-shift-g", W, R),
    cmd!(editor::UseSelectionForFind, "Edit", "Use Selection for Find", "cmd-e", W, R),
    cmd!(editor::ToggleCase, "Edit", "Find: Match Case", "cmd-alt-c", W, R),
    cmd!(editor::ToggleWholeWord, "Edit", "Find: Whole Word", "cmd-alt-w", W, R),
    cmd!(editor::ToggleRegex, "Edit", "Find: Regular Expression", "cmd-alt-r", W, R),
    cmd!(qur::FindInProject, "Edit", "Find in Project", "cmd-shift-f", W, R),
    cmd!(qur::ReplaceInProject, "Edit", "Replace in Project", "cmd-shift-h", W, R),
    // Terminal
    cmd!(qur::NewTerminal, "Terminal", "New Terminal", "cmd-t", W, R),
    cmd!(qur::NewAgentTerminal, "Terminal", "New Agent Terminal", "cmd-shift-t", W, R),
    cmd!(qur::CloseTerminal, "Terminal", "Close Terminal", "cmd-w", T, R),
    cmd!(qur::ToggleTerminal, "View", "Go to Terminal", "ctrl-`", W, R),
    cmd!(qur::Caffeinate, "View", "Keep Mac Awake (caffeinate)", "", W, R),
    cmd!(terminal::Clear, "Terminal", "Clear", "cmd-k", T, R),
    cmd!(terminal::Find, "Terminal", "Find in Terminal", "cmd-f", T, R),
    cmd!(terminal::Tile, "Terminal", "New Tiled Terminal", "cmd-enter", W, R),
    cmd!(terminal::SplitRight, "Terminal", "Split Right", "cmd-d", T, R),
    cmd!(terminal::SplitDown, "Terminal", "Split Down", "cmd-shift-d", T, R),
    cmd!(terminal::FocusLeft, "Terminal", "Focus Pane Left", "cmd-alt-left", T, R),
    cmd!(terminal::FocusRight, "Terminal", "Focus Pane Right", "cmd-alt-right", T, R),
    cmd!(terminal::FocusUp, "Terminal", "Focus Pane Above", "cmd-alt-up", T, R),
    cmd!(terminal::FocusDown, "Terminal", "Focus Pane Below", "cmd-alt-down", T, R),
    cmd!(qur::PrevTerminal, "Terminal", "Previous Terminal", "cmd-shift-[|cmd-{", W, R),
    cmd!(qur::NextTerminal, "Terminal", "Next Terminal", "cmd-shift-]|cmd-}", W, R),
    cmd!(terminal::SendLineStart, "Terminal", "Line Start", "cmd-left", T, R),
    cmd!(terminal::SendLineEnd, "Terminal", "Line End", "cmd-right", T, R),
    cmd!(terminal::SendKillLine, "Terminal", "Kill Line", "cmd-backspace", T, R),
    cmd!(terminal::SendBackTab, "Terminal", "Back Tab (Claude Code: cycle mode)", "shift-tab", T, R),
    // Panels
    cmd!(qur::GitPanel, "View", "Source Control", "ctrl-shift-g", NT, R),
    cmd!(qur::GitStageAll, "Git", "Stage All Changes", "", W, R),
    cmd!(qur::GitUnstageAll, "Git", "Unstage All Changes", "", W, R),
    cmd!(qur::GitCommit, "Git", "Commit Staged…", "", W, R),
    cmd!(qur::SearchPanel, "View", "Search", "", W, R),
    // Agents and peek
    cmd!(qur::FocusPrompt, "Agent", "Focus Prompt", "cmd-i", W, R),
    // "/" like a browser's search box, only when you're not typing somewhere.
    cmd!(qur::FocusInput, "View", "Focus Input Field", "/", Some("!Editor && !Terminal && !TextInput && !Picker"), R),
    cmd!(qur::StopAgent, "Agent", "Stop", "cmd-.", W, R),
    cmd!(qur::TakeOver, "Agent", "Take Over", "", W, R),
    cmd!(qur::NextAgent, "Agent", "Next Session", "ctrl-alt-down", W, R),
    cmd!(qur::PrevAgent, "Agent", "Previous Session", "ctrl-alt-up", W, R),
    // Zed's keys. Not in the editor or a terminal (they'd answer an agent you
    // can't see) or a picker; the workspace answers the selected agent first.
    cmd!(agent::AllowOnce, "Agent", "Permission: Allow Once", "cmd-y", PERM, R),
    cmd!(agent::AllowAlways, "Agent", "Permission: Allow for This Task", "cmd-alt-y", PERM, R),
    cmd!(agent::Deny, "Agent", "Permission: Deny", "cmd-alt-z", PERM, R),
    cmd!(agent::CycleMode, "Agent", "Next Mode (Default / Accept Edits / Plan…)", "shift-tab", Some("AgentView"), R),
    cmd!(qur::ReviewChanges, "Agent", "Review Changes", "", W, R),
    cmd!(qur::Merge, "Agent", "Merge into My Branch", "", W, R),
    cmd!(qur::KeepWorking, "Agent", "Keep Working", "", W, R),
    cmd!(qur::RenameAgent, "Agent", "Rename Agent…", "", W, R),
    cmd!(qur::RunChecks, "Agent", "Run Checks Now", "", W, R),
    cmd!(qur::RunProject, "Agent", "Run Project in Worktree", "", W, R),
    cmd!(qur::DiscardTask, "Agent", "Discard Task…", "", W, R),
    cmd!(qur::NextConflict, "Merge", "Next Conflict", "", W, R),
    cmd!(qur::PrevConflict, "Merge", "Previous Conflict", "", W, R),
    cmd!(qur::AcceptOurs, "Merge", "Accept Ours", "", W, R),
    cmd!(qur::AcceptTheirs, "Merge", "Accept Theirs", "", W, R),
    cmd!(qur::AcceptBoth, "Merge", "Accept Both", "", W, R),
    cmd!(qur::FinishMerge, "Merge", "Finish Merge", "", W, R),
    cmd!(qur::AbortMerge, "Merge", "Abort Merge", "", W, R),
    cmd!(qur::OpenWorktreeInTerminal, "Agent", "Open Task Worktree in Terminal", "", W, R),
    cmd!(qur::RevealWorktree, "Agent", "Reveal Worktree in Finder", "", W, R),
    cmd!(qur::CyclePeekLayout, "View", "Cycle Peek Layout", "cmd-k cmd-l", NT, R),
    cmd!(qur::ChooseLayout, "View", "Choose Stage Layout…", "", W, R),
    cmd!(qur::PeekLayoutA, "View", "A Watch + Side Notes", "", W, R),
    cmd!(qur::PeekLayoutB, "View", "B One View at a Time", "", W, R),
    cmd!(qur::PeekLayoutC, "View", "C Watch + Drawer", "", W, R),
    cmd!(qur::PeekLive, "View", "Live", "cmd-1", W, R),
    cmd!(qur::PeekTodo, "View", "To-do", "cmd-2", W, R),
    cmd!(qur::PeekChat, "View", "Chat", "cmd-3", W, R),
    cmd!(qur::PeekChanges, "View", "Changes", "cmd-4", W, R),
    cmd!(qur::PeekBrowser, "View", "Browser", "cmd-5", W, R),
    cmd!(qur::ToggleInspect, "View", "Browser: Inspect Element", "cmd-shift-c", NT, R),
    cmd!(browser::Reload, "Browser", "Reload Page", "cmd-r", B, R),
    cmd!(browser::FocusAddress, "Browser", "Address Bar", "cmd-l", B, R),
    cmd!(browser::Back, "Browser", "Back", "cmd-[|cmd-left", B, R),
    cmd!(browser::Forward, "Browser", "Forward", "cmd-]|cmd-right", B, R),
    cmd!(browser::ZoomIn, "Browser", "Zoom Page In", "cmd-=|cmd-+", B, R),
    cmd!(browser::ZoomOut, "Browser", "Zoom Page Out", "cmd--", B, R),
    cmd!(browser::ZoomReset, "Browser", "Actual Size", "cmd-0", B, R),
    cmd!(qur::FollowAgent, "View", "Follow Agent", "", W, R),
    // Help
    cmd!(qur::KeyboardShortcuts, "Help", "Qur Keyboard Shortcuts", "", W, R),
    cmd!(qur::RunDoctor, "Help", "Run Doctor", "", W, R),
    cmd!(qur::ShowLogs, "Help", "Show Logs in Finder", "", W, R),
    cmd!(qur::ReportProblem, "Help", "Report a Problem…", "", W, R),
    // Text fields
    cmd!(input::Backspace, "Input", "Delete Left", "backspace", I, R),
    cmd!(input::Delete, "Input", "Delete Right", "delete", I, R),
    cmd!(input::Left, "Input", "Left", "left", I, R),
    cmd!(input::Right, "Input", "Right", "right", I, R),
    cmd!(input::WordLeft, "Input", "Word Left", "alt-left", I, R),
    cmd!(input::WordRight, "Input", "Word Right", "alt-right", I, R),
    cmd!(input::DeleteWordLeft, "Input", "Delete Word Left", "alt-backspace", I, R),
    cmd!(input::DeleteToStart, "Input", "Delete to Start", "cmd-backspace", I, R),
    cmd!(input::Home, "Input", "Start", "cmd-left|home", I, R),
    cmd!(input::End, "Input", "End", "cmd-right|end", I, R),
    cmd!(input::Confirm, "Input", "Confirm", "enter", I, R),
    cmd!(input::ConfirmAlt, "Input", "Confirm (Previous)", "shift-enter", I, R),
    cmd!(input::ConfirmAll, "Input", "Confirm All", "cmd-enter", I, R),
    cmd!(input::Cancel, "Input", "Cancel", "escape", I, R),
    cmd!(setup::Done, "Input", "Setup: Done", "enter", Some("Setup"), R),
    cmd!(setup::Cancel, "Input", "Setup: Cancel", "escape", Some("Setup"), R),
    cmd!(input::Up, "Input", "Up", "up", I, R),
    cmd!(input::Down, "Input", "Down", "down", I, R),
    cmd!(input::Tab, "Input", "Tab", "tab", I, R),
];

pub fn find(action: &dyn Action) -> Option<&'static Cmd> {
    COMMANDS.iter().find(|c| (c.action)().partial_eq(action))
}

/// Commands shown in the palette (not the low-level cursor/input ones).
pub fn palette_commands() -> impl Iterator<Item = &'static Cmd> {
    COMMANDS.iter().filter(|c| !matches!(c.category, "Editor" | "Input"))
}

pub fn bind_keys(cx: &mut App) {
    let mut bindings = Vec::new();
    for c in COMMANDS {
        for keys in c.keys.split('|').filter(|k| !k.is_empty()) {
            let predicate = c.context.map(|ctx| gpui::KeyBindingContextPredicate::parse(ctx).unwrap().into());
            bindings.push(KeyBinding::load(keys, (c.action)(), predicate, false, None, cx).unwrap());
        }
    }
    const SESSION_KEYS: [&str; 9] = ["ctrl-1", "ctrl-2", "ctrl-3", "ctrl-4", "ctrl-5", "ctrl-6", "ctrl-7", "ctrl-8", "ctrl-9"];
    for (index, keys) in SESSION_KEYS.iter().enumerate() {
        bindings.push(KeyBinding::new(keys, qur::SelectSession { index }, Some("!Terminal")));
    }
    cx.bind_keys(bindings);
}

/// `cmd-shift-p` -> `⇧⌘P` (macOS order: ⌃⌥⇧⌘).
pub fn display_keys(keys: &str) -> String {
    keys.split(' ')
        .map(|stroke| {
            let parts: Vec<&str> = stroke.split('-').collect();
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

/// State shown in the menu bar (checkmarks, recent projects). Menus are
/// rebuilt when it changes or another window becomes active.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct MenuState {
    pub layout: &'static str,
    pub follow: bool,
    pub sidebar: bool,
    pub soft_wrap: bool,
    pub recent: Vec<PathBuf>,
    /// A merge stopped on conflicts: the Merge Conflicts items are enabled.
    pub merging: bool,
    pub vim: bool,
}

fn item(action: impl Action) -> MenuItem {
    let cmd = find(&action).expect("menu action is in COMMANDS");
    MenuItem::action(cmd.title, action).disabled(!cmd.ready())
}

fn titled(title: &'static str, action: impl Action) -> MenuItem {
    let ready = find(&action).map(|c| c.ready()).unwrap_or(false);
    MenuItem::action(title, action).disabled(!ready)
}

pub fn menus(s: &MenuState) -> Vec<Menu> {
    use MenuItem as M;
    let sep = M::separator;
    let home = std::env::var("HOME").unwrap_or_default();
    let recent: Vec<MenuItem> = if s.recent.is_empty() {
        vec![MenuItem::action("No Recent Projects", qur::OpenFolder).disabled(true)]
    } else {
        s.recent
            .iter()
            .map(|p| {
                let shown = p.to_string_lossy().replacen(&home, "~", 1);
                MenuItem::action(shown, qur::OpenProject { path: p.clone() })
            })
            .collect()
    };
    vec![
        Menu::new("Qur").items([
            titled("About Qur", qur::About),
            item(qur::CheckSetup),
            sep(),
            M::submenu(Menu::new("Settings").items([item(qur::OpenConfig), item(qur::OpenKeymap), item(qur::OpenAgents)])),
            item(qur::ThemePicker),
            item(qur::AccentColor),
            item(qur::UseDefaultTheme),
            item(qur::Transparency),
            item(qur::VimMode).checked(s.vim),
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
            item(qur::NewAgentTask),
            item(qur::NewTerminal),
            item(qur::NewAgentTerminal),
            item(qur::NewWindow),
            sep(),
            item(qur::OpenFolder),
            M::submenu(Menu::new("Open Recent").items(recent)),
            item(qur::ProjectSwitcher),
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
            item(editor::ToggleBlockComment),
            item(editor::Indent),
            item(editor::Outdent),
            item(editor::TrimWhitespace),
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
            item(editor::AddNextMatch),
            item(editor::SelectAllMatches),
            item(editor::AddCursorAbove),
            item(editor::AddCursorBelow),
        ]),
        Menu::new("View").items([
            item(qur::CommandPalette),
            sep(),
            item(qur::ToggleSidebar).checked(s.sidebar),
            item(qur::RevealInFiles),
            item(qur::ToggleTerminal),
            item(qur::Caffeinate),
            item(qur::GitPanel),
            M::submenu(Menu::new("Git").items([item(qur::GitStageAll), item(qur::GitUnstageAll), item(qur::GitCommit)])),
            item(qur::SearchPanel),
            sep(),
            M::submenu(Menu::new("Peek Layout").items([
                item(qur::PeekLayoutA).checked(s.layout == "A"),
                item(qur::PeekLayoutB).checked(s.layout == "B"),
                item(qur::PeekLayoutC).checked(s.layout == "C"),
                item(qur::CyclePeekLayout),
                item(qur::ChooseLayout),
            ])),
            M::submenu(Menu::new("Peek").items([
                item(qur::PeekLive),
                item(qur::PeekTodo),
                item(qur::PeekChat),
                item(qur::PeekChanges),
                item(qur::PeekBrowser),
            ])),
            item(qur::ToggleInspect),
            item(qur::FollowAgent).checked(s.follow),
            sep(),
            item(editor::ToggleSoftWrap).checked(s.soft_wrap),
            item(qur::ZoomIn),
            item(qur::ZoomOut),
            item(qur::ZoomReset),
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
            item(editor::JumpToBracket),
        ]),
        Menu::new("Agent").items([
            item(qur::NewAgentTask),
            item(qur::NewAgentTerminal),
            item(qur::FocusPrompt),
            item(qur::StopAgent),
            item(qur::TakeOver),
            item(qur::RenameAgent),
            sep(),
            item(agent::AllowOnce),
            item(agent::AllowAlways),
            item(agent::Deny),
            sep(),
            item(qur::NextAgent),
            item(qur::PrevAgent),
            sep(),
            item(qur::ReviewChanges),
            item(qur::Merge),
            item(qur::KeepWorking),
            item(qur::RunChecks),
            item(qur::RunProject),
            item(qur::DiscardTask),
            M::submenu(Menu::new("Merge Conflicts").items([
                item(qur::NextConflict).disabled(!s.merging),
                item(qur::PrevConflict).disabled(!s.merging),
                item(qur::AcceptOurs).disabled(!s.merging),
                item(qur::AcceptTheirs).disabled(!s.merging),
                item(qur::AcceptBoth).disabled(!s.merging),
                item(qur::FinishMerge).disabled(!s.merging),
                item(qur::AbortMerge).disabled(!s.merging),
            ])),
            sep(),
            item(qur::OpenWorktreeInTerminal),
            item(qur::RevealWorktree),
        ]),
        Menu::new("Terminal").items([
            item(qur::NewTerminal),
            item(qur::NewAgentTerminal),
            item(qur::NextTerminal),
            item(qur::PrevTerminal),
            sep(),
            item(terminal::Tile),
            item(terminal::SplitRight),
            item(terminal::SplitDown),
            item(terminal::FocusLeft),
            item(terminal::FocusRight),
            item(terminal::FocusUp),
            item(terminal::FocusDown),
            sep(),
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
