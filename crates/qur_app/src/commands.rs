//! The one command table: every action, its title, keys, context and tier.
//! Key bindings, the command palette and the native menu bar are all built
//! from `COMMANDS` (docs/keymap.md, docs/menus.md).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

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
            About, CheckSetup, OpenKeymap, OpenAgents, VimMode, NewAgentTask, FindInProject,
            ReplaceInProject, GitPanel, SearchPanel, PeekLayoutA, PeekLayoutB, PeekLayoutC,
            CyclePeekLayout, PeekLive, PeekTodo, PeekChat, PeekChanges, FollowAgent, GoToSymbol,
            FocusPrompt, FocusInput, StopAgent, TakeOver, NextAgent, PrevAgent, ReviewChanges, Merge,
            KeepWorking, DiscardTask, OpenWorktreeInTerminal, RevealWorktree, KeyboardShortcuts,
            RunDoctor, ShowLogs, ReportProblem, ExpandSelection, ShrinkSelection,
            RunChecks, RunProject, RenameAgent, AccentColor, ChooseLayout, GitStageAll, GitUnstageAll, GitCommit, NextConflict, PrevConflict, AcceptOurs, AcceptTheirs, AcceptBoth, FinishMerge, AbortMerge, Caffeinate, ShowKeepAwake, SwitchSession, ZenMode,
            Gaps, ToggleGaps, AutoHideSidebar, SidebarOnRight,
            ShowLastTurn, RewindLastTurn, ReviewWork, ReviewPlan, SendReviewFindings,
            PlanMode, CommentOnPlan, ApprovePlan, SendPlanComments,
            PushPullRequest, OpenPullRequest,
            NextChange, PrevChange, ShowChange, RevertChange, AcceptChange, ChangesLastTurn,
            OpenFile, OpenAsText, ReopenWithEncoding, ConvertToEncoding, ConvertLineEndings, ShowWhitespace,
            RevealInFinder, CopyPath, CopyRelativePath,
            CleanUpWorktrees,
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

/// Answers for the permission bar of the selected agent, and its chat.
pub mod agent {
    gpui::actions!(agent, [AllowOnce, AllowAlways, Deny, AllowAllPending, CycleMode, FindInChat, CopyChatMarkdown]);
}

pub mod terminal {
    gpui::actions!(terminal, [Clear, SendLineStart, SendLineEnd, SendKillLine, SendBackTab, Find, SplitRight, SplitDown, Tile, FocusLeft, FocusRight, FocusUp, FocusDown, MovePaneLeft, MovePaneRight, MovePaneUp, MovePaneDown]);
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
    #[allow(dead_code)]
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

    /// The keys in effect: keymap.toml's, else the default (`|`-separated).
    pub fn keys(&self) -> String {
        let i = COMMANDS.iter().position(|c| std::ptr::eq(c, self));
        let o = OVERRIDES.read().unwrap_or_else(|e| e.into_inner());
        match i.and_then(|i| o.get(&i)) {
            Some(keys) => keys.join("|"),
            None => self.keys.to_string(),
        }
    }

    pub fn first_keys(&self) -> String {
        self.keys().split('|').next().unwrap_or("").to_string()
    }

    /// `qur::GoToSymbol`, as keymap.toml and QUR_DEBUG_ACTIONS name it.
    pub fn action_name(&self) -> &'static str {
        (self.action)().name()
    }
}

/// keymap.toml's rebindings: command index -> keys (empty = unbound).
static OVERRIDES: std::sync::RwLock<BTreeMap<usize, Vec<String>>> = std::sync::RwLock::new(BTreeMap::new());
/// Bumped on every keymap load, so the menu bar picks up the new shortcuts.
static KEYMAP_GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn keymap_generation() -> u64 {
    KEYMAP_GENERATION.load(Ordering::Relaxed)
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
/// A chat agent's view, but not the editor, terminal or page inside its stage.
const CHAT: Option<&str> = Some("AgentView && !Editor && !Terminal && !Browser");
use Tier::Ready as R;

#[rustfmt::skip]
pub static COMMANDS: &[Cmd] = &[
    // App and window
    cmd!(qur::Quit, "Qur", "Quit Qur", "cmd-q", W, R),
    cmd!(qur::Hide, "Qur", "Hide Qur", "cmd-h", W, R),
    cmd!(qur::HideOthers, "Qur", "Hide Others", "cmd-alt-h", W, R),
    cmd!(qur::ShowAll, "Qur", "Show All", "", W, R),
    cmd!(qur::About, "Qur", "About Qur", "", W, R),
    cmd!(qur::CheckSetup, "Qur", "Settings…", "cmd-,", W, R),
    cmd!(qur::OpenConfig, "Qur", "Open config.toml", "", W, R),
    cmd!(qur::OpenKeymap, "Qur", "Open keymap.toml", "", W, R),
    cmd!(qur::OpenAgents, "Qur", "Agents (agents.toml)", "", W, R),
    cmd!(qur::ThemePicker, "View", "Theme…", "cmd-k cmd-t", NT, R),
    cmd!(qur::AccentColor, "View", "Accent Color…", "", W, R),
    cmd!(qur::UseDefaultTheme, "View", "Use Default Theme for This Project", "", W, R),
    cmd!(qur::Transparency, "View", "Transparency…", "", W, R),
    cmd!(qur::Gaps, "View", "Gaps…", "", W, R),
    cmd!(qur::VimMode, "Edit", "Vim Mode", "", W, R),
    cmd!(qur::NewWindow, "File", "New Window", "cmd-n", W, R),
    cmd!(qur::NewAgentTask, "Agent", "New Agent Task…", "cmd-shift-n", W, R),
    cmd!(qur::OpenFile, "File", "Open File…", "cmd-o", W, R),
    cmd!(qur::OpenFolder, "File", "Open Folder…", "cmd-k cmd-o", NT, R),
    cmd!(qur::ProjectSwitcher, "File", "Switch Project…", "cmd-alt-o", W, R),
    cmd!(qur::CloseWindow, "File", "Close Window", "cmd-shift-w", W, R),
    cmd!(qur::Minimize, "Window", "Minimize", "cmd-m", W, R),
    cmd!(qur::Zoom, "Window", "Zoom", "", W, R),
    cmd!(qur::ToggleFullScreen, "View", "Enter Full Screen", "ctrl-cmd-f", W, R),
    cmd!(qur::CommandPalette, "View", "Command Palette…", "cmd-shift-p|f1", W, R),
    cmd!(qur::ToggleSidebar, "View", "Sidebar", "cmd-b", W, R),
    cmd!(qur::AutoHideSidebar, "View", "Auto-hide Sidebar", "", W, R),
    cmd!(qur::SidebarOnRight, "View", "Sidebar on Right", "", W, R),
    cmd!(qur::ZoomIn, "View", "Zoom In", "cmd-=|cmd-+", W, R),
    cmd!(qur::ZoomOut, "View", "Zoom Out", "cmd--", W, R),
    cmd!(qur::ZoomReset, "View", "Reset Zoom", "cmd-0", W, R),
    // Files and navigation
    cmd!(qur::QuickOpen, "Go", "Go to File…", "cmd-p", W, R),
    cmd!(qur::RecentFile, "Go", "Next Recent File", "ctrl-tab", NT, R),
    cmd!(editor::Save, "File", "Save", "cmd-s", E, R),
    cmd!(qur::SaveAll, "File", "Save All", "cmd-alt-s", W, R),
    cmd!(qur::RevertFile, "File", "Revert File", "", W, R),
    // Notepad++'s Encoding menu.
    cmd!(qur::OpenAsText, "File", "Open as Text", "", W, R),
    cmd!(qur::ReopenWithEncoding, "File", "Reopen with Encoding…", "", W, R),
    cmd!(qur::ConvertToEncoding, "File", "Convert to Encoding…", "", W, R),
    cmd!(qur::ConvertLineEndings, "File", "Convert Line Endings (LF ⇄ CRLF)", "", W, R),
    // The file in the code modal (the Files tree's right-click menu has them too).
    // VS Code's Opt+Cmd+R and Opt+Cmd+C are Qur's find toggles, so only Copy
    // Relative Path takes VS Code's key.
    cmd!(qur::RevealInFinder, "File", "Reveal in Finder", "", W, R),
    cmd!(qur::CopyPath, "File", "Copy Path", "", W, R),
    cmd!(qur::CopyRelativePath, "File", "Copy Relative Path", "cmd-alt-shift-c", W, R),
    cmd!(qur::RevealInFiles, "View", "Files Dropdown", "cmd-shift-e", W, R),
    cmd!(qur::GoToLine, "Go", "Go to Line…", "ctrl-g", NT, R),
    cmd!(qur::NavBack, "Go", "Back", "ctrl--", NT, R),
    cmd!(qur::NavForward, "Go", "Forward", "ctrl-shift--", NT, R),
    cmd!(qur::GoToSymbol, "Go", "Go to Symbol in File…", "cmd-shift-o", W, R),
    cmd!(editor::JumpToBracket, "Go", "Jump to Matching Bracket", "cmd-shift-\\", E, R),
    // Inline diff review in the code modal (VS Code's Alt+F5 / Alt+F3)
    cmd!(qur::NextChange, "Go", "Next Change", "alt-f5", E, R),
    cmd!(qur::PrevChange, "Go", "Previous Change", "shift-alt-f5", E, R),
    cmd!(qur::ShowChange, "Changes", "Show Change", "alt-f3", E, R),
    cmd!(qur::RevertChange, "Changes", "Revert Change", "", W, R),
    cmd!(qur::AcceptChange, "Changes", "Accept Change", "", W, R),
    cmd!(qur::ChangesLastTurn, "Changes", "Last Turn Only", "", W, R),
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
    // VS Code's keys (Smart Select).
    cmd!(qur::ExpandSelection, "Selection", "Expand Selection", "ctrl-shift-cmd-right", E, R),
    cmd!(qur::ShrinkSelection, "Selection", "Shrink Selection", "ctrl-shift-cmd-left", E, R),
    cmd!(editor::AddNextMatch, "Selection", "Add Next Match", "cmd-d", E, R),
    cmd!(editor::SelectAllMatches, "Selection", "Select All Matches", "cmd-shift-l", E, R),
    cmd!(editor::AddCursorAbove, "Selection", "Add Cursor Above", "cmd-alt-up", E, R),
    cmd!(editor::AddCursorBelow, "Selection", "Add Cursor Below", "cmd-alt-down", E, R),
    cmd!(editor::TrimWhitespace, "Edit", "Trim Trailing Whitespace", "cmd-k cmd-x", E, R),
    cmd!(editor::Escape, "Editor", "Collapse Selection / Close Find", "escape", E, R),
    cmd!(editor::ToggleSoftWrap, "View", "Soft Wrap", "alt-z", W, R),
    cmd!(qur::ShowWhitespace, "View", "Show Whitespace", "", W, R),
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
    // In an agent it closes (or discards) the agent, asking first.
    cmd!(qur::CloseTerminal, "Terminal", "Close Terminal or Agent", "cmd-w", Some("Terminal || AgentView"), R),
    cmd!(qur::ToggleTerminal, "View", "Go to Terminal", "ctrl-`", W, R),
    cmd!(qur::Caffeinate, "View", "Keep Mac Awake (caffeinate)", "", W, R),
    cmd!(qur::ShowKeepAwake, "View", "Show Keep Awake in Sidebar", "", W, R),
    cmd!(qur::ZenMode, "View", "Zen Mode", "ctrl-cmd-z", W, R),
    cmd!(qur::ToggleGaps, "View", "Toggle Gaps", "", W, R),
    cmd!(terminal::Clear, "Terminal", "Clear", "cmd-k", T, R),
    cmd!(terminal::Find, "Terminal", "Find in Terminal", "cmd-f", T, R),
    cmd!(terminal::Tile, "Terminal", "New Tiled Terminal (Grid)", "cmd-enter", W, R),
    cmd!(terminal::SplitRight, "Terminal", "Split (Dwindle)", "cmd-d", T, R),
    cmd!(terminal::SplitDown, "Terminal", "Split Down", "cmd-shift-d", T, R),
    cmd!(terminal::FocusLeft, "Terminal", "Focus Pane Left", "cmd-alt-left", T, R),
    cmd!(terminal::FocusRight, "Terminal", "Focus Pane Right", "cmd-alt-right", T, R),
    cmd!(terminal::FocusUp, "Terminal", "Focus Pane Above", "cmd-alt-up", T, R),
    cmd!(terminal::FocusDown, "Terminal", "Focus Pane Below", "cmd-alt-down", T, R),
    cmd!(terminal::MovePaneLeft, "Terminal", "Move Pane Left", "cmd-alt-shift-left", T, R),
    cmd!(terminal::MovePaneRight, "Terminal", "Move Pane Right", "cmd-alt-shift-right", T, R),
    cmd!(terminal::MovePaneUp, "Terminal", "Move Pane Up", "cmd-alt-shift-up", T, R),
    cmd!(terminal::MovePaneDown, "Terminal", "Move Pane Down", "cmd-alt-shift-down", T, R),
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
    // Everywhere, terminals too (it takes the shell's own Ctrl+T).
    cmd!(qur::SwitchSession, "Agent", "Switch Agent or Terminal…", "ctrl-t", W, R),
    // Zed's keys. Not in the editor or a terminal (they'd answer an agent you
    // can't see) or a picker; the workspace answers the selected agent first.
    cmd!(agent::AllowOnce, "Agent", "Permission: Allow Once", "cmd-y", PERM, R),
    cmd!(agent::AllowAlways, "Agent", "Permission: Allow for This Task", "cmd-alt-y", PERM, R),
    cmd!(agent::Deny, "Agent", "Permission: Deny", "cmd-alt-z", PERM, R),
    // Each waiting request "allow once", never "always".
    cmd!(agent::AllowAllPending, "Agent", "Permission: Allow All Pending", "", W, R),
    cmd!(agent::FindInChat, "Agent", "Find in Chat", "cmd-f", CHAT, R),
    cmd!(agent::CopyChatMarkdown, "Agent", "Copy Chat as Markdown", "", W, R),
    cmd!(agent::CycleMode, "Agent", "Next Mode (Default / Accept Edits / Plan…)", "shift-tab", Some("AgentView"), R),
    // Plan mode (in the New Agent Task dialog: start the task in it).
    cmd!(qur::PlanMode, "Agent", "Plan Mode", "cmd-alt-p", W, R),
    cmd!(qur::CommentOnPlan, "Agent", "Comment on Plan…", "", W, R),
    cmd!(qur::ApprovePlan, "Agent", "Approve Plan", "", W, R),
    cmd!(qur::SendPlanComments, "Agent", "Send Plan Comments", "", W, R),
    cmd!(qur::ReviewChanges, "Agent", "Review Changes", "", W, R),
    cmd!(qur::ShowLastTurn, "Agent", "Show Last Turn's Changes", "", W, R),
    cmd!(qur::RewindLastTurn, "Agent", "Rewind Last Turn…", "", W, R),
    cmd!(qur::ReviewWork, "Agent", "Review Agent's Work…", "", W, R),
    cmd!(qur::ReviewPlan, "Agent", "Review Agent's Plan…", "", W, R),
    cmd!(qur::SendReviewFindings, "Agent", "Send Review Findings to Agent", "", W, R),
    cmd!(qur::Merge, "Agent", "Merge into My Branch", "", W, R),
    // Run by the user's own git and gh, with their logins (pull_request.rs).
    cmd!(qur::PushPullRequest, "Agent", "Push and Open Pull Request…", "", W, R),
    cmd!(qur::OpenPullRequest, "Agent", "Open Pull Request", "", W, R),
    cmd!(qur::KeepWorking, "Agent", "Keep Working", "", W, R),
    cmd!(qur::RenameAgent, "Agent", "Rename Agent…", "", W, R),
    cmd!(qur::RunChecks, "Agent", "Run Checks Now", "", W, R),
    cmd!(qur::RunProject, "Agent", "Run Project in Worktree", "", W, R),
    cmd!(qur::DiscardTask, "Agent", "Discard Task…", "", W, R),
    cmd!(qur::CleanUpWorktrees, "Agent", "Clean Up Worktrees…", "", W, R),
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

/// Commands that need the selected agent to have a turn (disabled otherwise).
pub fn needs_turn(c: &Cmd) -> bool {
    let a = (c.action)();
    a.partial_eq(&qur::ShowLastTurn) || a.partial_eq(&qur::RewindLastTurn)
}

/// Commands that need a permission request waiting (disabled otherwise).
pub fn needs_pending(c: &Cmd) -> bool {
    (c.action)().partial_eq(&agent::AllowAllPending)
}

/// Commands that only mean something in a project window: the empty window
/// (no project) leaves them unhandled, so their menu items grey out and their
/// keys do nothing. New Agent Task and New Agent Terminal aren't here: in the
/// empty window they ask for the folder to work in.
const PROJECT_ONLY: &[&str] = &[
    "qur::QuickOpen", "qur::RevealInFiles", "qur::UseDefaultTheme", "qur::FindInProject", "qur::ReplaceInProject",
    "qur::SearchPanel", "qur::FocusPrompt", "qur::StopAgent", "qur::TakeOver", "qur::RenameAgent",
    "agent::AllowOnce", "agent::AllowAlways", "agent::Deny", "agent::AllowAllPending", "agent::FindInChat",
    "agent::CopyChatMarkdown", "qur::ReviewChanges", "qur::ShowLastTurn", "qur::RewindLastTurn", "qur::ReviewWork",
    "qur::ReviewPlan", "qur::SendReviewFindings", "qur::Merge", "qur::KeepWorking", "qur::RunChecks", "qur::RunProject",
    "qur::DiscardTask", "qur::OpenWorktreeInTerminal", "qur::RevealWorktree", "qur::CyclePeekLayout",
    "qur::ChooseLayout", "qur::PeekLayoutA", "qur::PeekLayoutB", "qur::PeekLayoutC", "qur::PeekLive", "qur::PeekTodo",
    "qur::PeekChat", "qur::PeekChanges", "qur::PeekBrowser", "qur::ToggleInspect", "qur::FollowAgent",
    "qur::CleanUpWorktrees", "qur::PlanMode", "qur::CommentOnPlan", "qur::ApprovePlan", "qur::SendPlanComments",
    "qur::PushPullRequest", "qur::OpenPullRequest", "qur::NextChange", "qur::PrevChange", "qur::ShowChange",
    "qur::RevertChange", "qur::AcceptChange", "qur::ChangesLastTurn",
];

/// Commands that need the project to be a git repository (a plain folder
/// opens as a project without them).
const REPO_ONLY: &[&str] = &[
    "qur::GitPanel", "qur::GitStageAll", "qur::GitUnstageAll", "qur::GitCommit", "qur::NextConflict", "qur::PrevConflict",
    "qur::AcceptOurs", "qur::AcceptTheirs", "qur::AcceptBoth", "qur::FinishMerge", "qur::AbortMerge",
];

/// Whether `c` can run in a window with (`project`) or without a project,
/// and in a git repository (`repo`) or a plain folder.
pub fn available_in(c: &Cmd, project: bool, repo: bool) -> bool {
    let name = c.action_name();
    let repo_only = REPO_ONLY.contains(&name);
    (project || (!PROJECT_ONLY.contains(&name) && !repo_only)) && (repo || !repo_only)
}

/// What the selected agent offers for review (disabled, not hidden, without it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReviewState {
    /// A structured (ACP) agent is selected: Review Agent's Work….
    pub work: bool,
    /// …and it has a plan: Review Agent's Plan….
    pub plan: bool,
    /// Its reviewer has findings: Send Review Findings to Agent.
    pub findings: bool,
}

/// Is a review command available in `s`? Other commands always are.
pub fn review_available(c: &Cmd, s: ReviewState) -> bool {
    let a = (c.action)();
    if a.partial_eq(&qur::ReviewWork) {
        s.work
    } else if a.partial_eq(&qur::ReviewPlan) {
        s.plan
    } else if a.partial_eq(&qur::SendReviewFindings) {
        s.findings
    } else {
        true
    }
}

/// The selected agent's plan mode (Agent menu).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlanState {
    /// A structured agent is selected (or the New Agent Task dialog is open): Plan Mode.
    pub agent: bool,
    /// It's in plan mode (the checkmark).
    pub on: bool,
    /// It has a plan to comment on: Comment on Plan…, Approve Plan.
    pub plan: bool,
    /// …and comments on it: Send Plan Comments.
    pub comments: bool,
}

/// Is a plan command available in `s`? Other commands always are.
pub fn plan_available(c: &Cmd, s: PlanState) -> bool {
    let a = (c.action)();
    if a.partial_eq(&qur::PlanMode) {
        s.agent
    } else if a.partial_eq(&qur::CommentOnPlan) || a.partial_eq(&qur::ApprovePlan) {
        s.plan
    } else if a.partial_eq(&qur::SendPlanComments) {
        s.comments
    } else {
        true
    }
}

/// Commands shown in the palette (not the low-level cursor/input ones).
pub fn palette_commands() -> impl Iterator<Item = &'static Cmd> {
    COMMANDS.iter().filter(|c| !matches!(c.category, "Editor" | "Input"))
}

fn binding(c: &Cmd, keys: &str, cx: &App) -> Result<KeyBinding, String> {
    let predicate = c.context.map(|ctx| gpui::KeyBindingContextPredicate::parse(ctx).unwrap().into());
    KeyBinding::load(keys, (c.action)(), predicate, false, None, cx).map_err(|e| e.to_string())
}

/// The command keymap.toml means by `name`: an action name
/// (`qur::GoToSymbol`), a palette entry (`Go: Go to Symbol in File…`) or a
/// title alone when only one command has it. Case and a trailing `…` don't matter.
pub fn resolve(name: &str) -> Result<usize, String> {
    let norm = |s: &str| s.trim().trim_end_matches('…').trim_end_matches("...").trim().to_lowercase();
    let want = norm(name);
    if let Some(i) = COMMANDS.iter().position(|c| c.action_name().eq_ignore_ascii_case(name.trim())) {
        return Ok(i);
    }
    if let Some(i) = COMMANDS.iter().position(|c| norm(&format!("{}: {}", c.category, c.title)) == want) {
        return Ok(i);
    }
    let hits: Vec<usize> = (0..COMMANDS.len()).filter(|&i| norm(COMMANDS[i].title) == want).collect();
    match hits.as_slice() {
        [i] => Ok(*i),
        [] => Err(format!("no command named `{name}`")),
        many => Err(format!(
            "`{name}` is ambiguous; use one of {}",
            many.iter().map(|&i| format!("`{}`", COMMANDS[i].action_name())).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Load keymap.toml and rebind every key. Returns one message per entry that
/// was skipped (unknown command, bad keystroke, wrong shape); the rest apply.
pub fn reload_keymap(cx: &mut App) -> Vec<String> {
    let (entries, mut errors) = qur_config::load_keymap();
    let mut map = BTreeMap::new();
    for e in entries {
        let at = if e.line > 0 { format!("keymap.toml line {}", e.line) } else { "keymap.toml".to_string() };
        let i = match resolve(&e.command) {
            Ok(i) => i,
            Err(msg) => {
                errors.push(format!("{at}: {msg}"));
                continue;
            }
        };
        let mut keys = Vec::new();
        for k in e.keys {
            match binding(&COMMANDS[i], &k, cx) {
                Ok(_) => keys.push(k),
                Err(msg) => errors.push(format!("{at}: `{k}` for `{}`: {msg}", e.command)),
            }
        }
        map.insert(i, keys);
    }
    *OVERRIDES.write().unwrap_or_else(|e| e.into_inner()) = map;
    KEYMAP_GENERATION.fetch_add(1, Ordering::Relaxed);
    cx.clear_key_bindings();
    bind_keys(cx);
    errors
}

pub fn bind_keys(cx: &mut App) {
    let mut bindings = Vec::new();
    for c in COMMANDS {
        for keys in c.keys().split('|').filter(|k| !k.is_empty()) {
            // Defaults always load; user keys were checked by `reload_keymap`.
            if let Ok(b) = binding(c, keys, cx) {
                bindings.push(b);
            }
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
    /// `[ui] sidebar = "auto"` and `sidebar_side = "right"`.
    pub sidebar_auto: bool,
    pub sidebar_right: bool,
    pub soft_wrap: bool,
    /// `[editor] show_whitespace`.
    pub show_whitespace: bool,
    pub recent: Vec<PathBuf>,
    /// A merge stopped on conflicts: the Merge Conflicts items are enabled.
    pub merging: bool,
    /// Change markers show a task's last turn only (Changes > Last Turn Only).
    pub diff_last_turn: bool,
    /// The selected agent has a turn to show or rewind (ACP agents only).
    pub turns: bool,
    /// An agent has a permission request waiting (Allow All Pending).
    pub pending: bool,
    /// Review Agent's Work… / Plan… / Send Review Findings for the selected agent.
    pub review: ReviewState,
    /// Plan Mode and the plan's commands for the selected agent.
    pub plan: PlanState,
    pub vim: bool,
    pub keep_awake: bool,
    pub zen: bool,
    /// Window gaps are on (`[ui] gaps`).
    pub gaps: bool,
    /// `keymap_generation()`: new shortcuts rebuild the menus.
    pub keymap: u64,
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
            M::submenu(Menu::new("Settings Files").items([item(qur::OpenConfig), item(qur::OpenKeymap), item(qur::OpenAgents)])),
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
            item(qur::OpenFile),
            item(qur::OpenFolder),
            M::submenu(Menu::new("Open Recent").items(recent)),
            item(qur::ProjectSwitcher),
            titled("Quick Open…", qur::QuickOpen),
            sep(),
            item(editor::Save),
            item(qur::SaveAll),
            item(qur::RevertFile),
            sep(),
            item(qur::OpenAsText),
            M::submenu(Menu::new("Encoding").items([
                item(qur::ReopenWithEncoding),
                item(qur::ConvertToEncoding),
                sep(),
                item(qur::ConvertLineEndings),
            ])),
            sep(),
            item(qur::RevealInFinder),
            item(qur::CopyPath),
            item(qur::CopyRelativePath),
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
            sep(),
            M::submenu(Menu::new("Changes").items([
                item(qur::NextChange),
                item(qur::PrevChange),
                item(qur::ShowChange),
                item(qur::RevertChange),
                item(qur::AcceptChange),
                sep(),
                item(qur::ChangesLastTurn).checked(s.diff_last_turn),
            ])),
            sep(),
            item(qur::VimMode).checked(s.vim),
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
            item(qur::AutoHideSidebar).checked(s.sidebar_auto),
            item(qur::SidebarOnRight).checked(s.sidebar_right),
            item(qur::RevealInFiles),
            item(qur::ToggleTerminal),
            item(qur::Caffeinate),
            item(qur::ShowKeepAwake).checked(s.keep_awake),
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
            item(qur::ShowWhitespace).checked(s.show_whitespace),
            item(qur::ZoomIn),
            item(qur::ZoomOut),
            item(qur::ZoomReset),
            sep(),
            M::submenu(Menu::new("Appearance").items([
                item(qur::ThemePicker),
                item(qur::UseDefaultTheme),
                sep(),
                item(qur::AccentColor),
                item(qur::Transparency),
                item(qur::Gaps),
            ])),
            item(qur::ZenMode).checked(s.zen),
            item(qur::ToggleGaps).checked(s.gaps),
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
            sep(),
            item(qur::NextChange),
            item(qur::PrevChange),
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
            item(agent::AllowAllPending).disabled(!s.pending),
            sep(),
            item(agent::FindInChat),
            item(agent::CopyChatMarkdown),
            sep(),
            item(qur::PlanMode).checked(s.plan.on).disabled(!s.plan.agent),
            item(qur::CommentOnPlan).disabled(!s.plan.plan),
            item(qur::ApprovePlan).disabled(!s.plan.plan),
            item(qur::SendPlanComments).disabled(!s.plan.comments),
            sep(),
            item(qur::NextAgent),
            item(qur::PrevAgent),
            item(qur::SwitchSession),
            sep(),
            item(qur::ReviewChanges),
            item(qur::ShowLastTurn).disabled(!s.turns),
            item(qur::RewindLastTurn).disabled(!s.turns),
            item(qur::ReviewWork).disabled(!s.review.work),
            item(qur::ReviewPlan).disabled(!s.review.plan),
            item(qur::SendReviewFindings).disabled(!s.review.findings),
            item(qur::Merge),
            item(qur::PushPullRequest),
            item(qur::OpenPullRequest),
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
            item(qur::CleanUpWorktrees),
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
            item(terminal::MovePaneLeft),
            item(terminal::MovePaneRight),
            item(terminal::MovePaneUp),
            item(terminal::MovePaneDown),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn name(r: Result<usize, String>) -> Result<&'static str, String> {
        r.map(|i| COMMANDS[i].action_name())
    }

    #[test]
    fn keymap_names_resolve() {
        assert_eq!(name(resolve("qur::GoToSymbol")), Ok("qur::GoToSymbol"));
        assert_eq!(name(resolve("Go: Go to Symbol in File…")), Ok("qur::GoToSymbol"));
        assert_eq!(name(resolve("go to symbol in file")), Ok("qur::GoToSymbol"));
        assert_eq!(name(resolve("Expand Selection")), Ok("qur::ExpandSelection"));
        assert_eq!(name(resolve("Selection: Delete Line")), Ok("editor::DeleteLine"));
        let e = resolve("Line Start").unwrap_err();
        assert!(e.contains("editor::LineStart") && e.contains("terminal::SendLineStart"), "{e}");
        assert!(resolve("No Such Command").unwrap_err().contains("no command"));
    }

    /// Cmd+, opens Settings (macOS standard), not config.toml; the files stay
    /// reachable from Qur > Settings Files and the palette.
    #[test]
    fn settings_on_cmd_comma() {
        assert_eq!(COMMANDS[resolve("qur::CheckSetup").unwrap()].keys(), "cmd-,");
        assert_eq!(COMMANDS[resolve("Qur: Settings…").unwrap()].action_name(), "qur::CheckSetup");
        let bound: Vec<&str> = COMMANDS.iter().filter(|c| c.keys().split('|').any(|k| k == "cmd-,")).map(|c| c.action_name()).collect();
        assert_eq!(bound, ["qur::CheckSetup"]);
        assert_eq!(COMMANDS[resolve("qur::OpenConfig").unwrap()].keys(), "");
        assert_eq!(COMMANDS[resolve("View: Theme…").unwrap()].action_name(), "qur::ThemePicker");
        assert_eq!(COMMANDS[resolve("Edit: Vim Mode").unwrap()].action_name(), "qur::VimMode");
    }

    /// The empty window's and plain folders' lists name real commands, and
    /// opening a project or a folder, terminals and settings work everywhere.
    #[test]
    fn project_and_repo_only_commands() {
        for name in PROJECT_ONLY.iter().chain(REPO_ONLY) {
            assert!(resolve(name).is_ok(), "{name} isn't a command");
        }
        let c = |name: &str| &COMMANDS[resolve(name).unwrap()];
        for name in ["qur::OpenFolder", "qur::NewWindow", "qur::ProjectSwitcher", "qur::NewTerminal", "qur::NewAgentTerminal", "qur::NewAgentTask", "qur::CheckSetup", "qur::ThemePicker", "editor::Save"] {
            assert!(available_in(c(name), false, false), "{name} should work without a project");
        }
        assert!(!available_in(c("qur::QuickOpen"), false, false));
        assert!(available_in(c("qur::QuickOpen"), true, false));
        assert!(!available_in(c("qur::GitPanel"), true, false));
        assert!(!available_in(c("qur::GitPanel"), false, true));
        assert!(available_in(c("qur::GitPanel"), true, true));
    }

    #[test]
    fn nothing_left_greyed_out() {
        for c in COMMANDS {
            assert!(c.ready(), "{} is still 'coming in a later version'", c.title);
        }
        for a in ["qur::GoToSymbol", "qur::ExpandSelection", "qur::ShrinkSelection", "qur::OpenKeymap"] {
            let c = &COMMANDS[resolve(a).unwrap()];
            assert!(c.ready());
        }
        assert_eq!(COMMANDS[resolve("qur::ExpandSelection").unwrap()].keys(), "ctrl-shift-cmd-right");
        assert_eq!(COMMANDS[resolve("qur::ShrinkSelection").unwrap()].keys(), "ctrl-shift-cmd-left");
    }

    /// Cmd+O opens a file (any path); Open Folder moves to Cmd+K Cmd+O, and
    /// no other command has either key.
    #[test]
    fn open_file_and_folder_keys() {
        assert_eq!(COMMANDS[resolve("qur::OpenFile").unwrap()].keys(), "cmd-o");
        assert_eq!(COMMANDS[resolve("qur::OpenFolder").unwrap()].keys(), "cmd-k cmd-o");
        for keys in ["cmd-o", "cmd-k cmd-o"] {
            let bound: Vec<&str> = COMMANDS.iter().filter(|c| c.keys().split('|').any(|k| k == keys)).map(|c| c.action_name()).collect();
            assert_eq!(bound.len(), 1, "{keys}: {bound:?}");
        }
        for a in ["qur::OpenAsText", "qur::ReopenWithEncoding", "qur::ConvertToEncoding", "qur::ConvertLineEndings", "qur::ShowWhitespace"] {
            assert!(COMMANDS[resolve(a).unwrap()].ready());
        }
    }

    /// VS Code's keys for the file path commands, where Qur has them free:
    /// Opt+Cmd+R and Opt+Cmd+C stay the find toggles.
    #[test]
    fn file_path_keys() {
        let bound = |k: &str| -> Vec<&str> { COMMANDS.iter().filter(|c| c.keys().split('|').any(|x| x == k)).map(|c| c.action_name()).collect() };
        assert_eq!(bound("cmd-alt-shift-c"), ["qur::CopyRelativePath"]);
        assert_eq!(bound("cmd-alt-r"), ["editor::ToggleRegex"]);
        assert_eq!(bound("cmd-alt-c"), ["editor::ToggleCase"]);
        assert_eq!(COMMANDS[resolve("qur::RevealInFinder").unwrap()].keys(), "");
        assert_eq!(COMMANDS[resolve("qur::CopyPath").unwrap()].keys(), "");
    }
}
