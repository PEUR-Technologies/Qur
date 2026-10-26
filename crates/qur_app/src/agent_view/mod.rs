//! One ACP agent task: its process (via `qur_acp`), chat transcript, plan,
//! pending permission and status, plus the stage view that shows it:
//! header, To-do strip, Chat, permission bar and prompt bar.

mod chat;
mod find;
mod plan;
mod second_opinion;
mod session;
mod stage;
mod turns;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui::{
    App, ClipboardItem, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable, HighlightStyle, ListAlignment, ListState,
    SharedString, StyledText, Subscription, Task, Window, div, img, list, prelude::*, px,
};
use qur_acp::{
    AcpError, AgentCommand, AuthMethod, Client, ConfigOption, Event, Media, PermissionKind, PermissionOption, PermissionOutcome,
    PlanEntry, PlanStatus, PromptBlock, RequestId, SessionId, SessionUpdate, StopReason, ToolCall, ToolCallId, ToolCallUpdate,
    ToolContent, ToolKind, ToolStatus,
};
use qur_agents::{AgentDef, QueuedMessage, ReviewLink, TaskRecord};
use qur_theme::Rgba;

use crate::review::{self, Subject};

use crate::browser_view::{Attachment, BrowserPane, BrowserPaneEvent, base64};
use crate::commands::agent as aa;
use crate::changes_view::{ChangesEvent, ChangesView};
use crate::dock::{self, Docks, Kind, Panel};
use crate::input::{InputEvent, TextInput};
use crate::live::{LiveEvent, LivePane};
use crate::files::Preview;
use crate::look::Look;
use crate::markdown::{FindHl, find_all};
use crate::monitor::{TaskMonitor, heat_kind, heat_status};
use crate::prompt::{PromptEvent, PromptInput};

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// No process (after a restart or Stop + close).
    Stopped,
    Starting,
    /// Session ready, waiting for a prompt.
    Idle,
    Working,
    /// Waiting for a permission answer.
    NeedsYou,
    /// Last turn ended.
    Done,
    Error(String),
}

impl Status {
    /// Sidebar glyph: working, done, needs you, error.
    pub fn glyph(&self) -> &'static str {
        match self {
            Status::Working | Status::Starting => "~",
            Status::NeedsYou => "!",
            Status::Done => "✓",
            Status::Error(_) => "✕",
            Status::Idle | Status::Stopped => "·",
        }
    }

    pub fn label(&self) -> String {
        match self {
            Status::Stopped => "stopped".into(),
            Status::Starting => "starting…".into(),
            Status::Idle => "ready".into(),
            Status::Working => "working…".into(),
            Status::NeedsYou => "needs you".into(),
            Status::Done => "done".into(),
            Status::Error(e) => format!("error: {e}"),
        }
    }
}

/// A subagent (Claude's Agent/Task tool) running inside an agent's session.
pub struct Subagent {
    /// Its own (child) session id.
    pub id: SessionId,
    pub title: String,
    /// "running", "requires_action", "idle" (done), or what the agent sent.
    pub state: String,
    /// What it's doing: its latest tool call or message line.
    pub last: Option<String>,
    /// Its work so far: (is a tool call, text), oldest first.
    pub log: Vec<(bool, String)>,
}

pub enum AgentEvent {
    /// Status, title or session changed (sidebar, task store).
    Changed,
    /// The user clicked a file in a tool call: open it in the code modal.
    OpenFile(PathBuf, Option<u32>),
    /// The user clicked an attachment (in the prompt or a sent message).
    Preview(Preview),
    /// `@` typed in the prompt: open the mention picker for this worktree.
    MentionRequested,
    /// `/` typed: offer this agent's settings and slash commands.
    SlashRequested,
    /// A subagent started, changed state or did something (sidebar).
    Subagents,
    /// Changes view: merge this task's branch into the user's branch.
    Merge,
    /// Changes view: push the task's branch and open a pull request.
    PullRequest,
    /// Changes view: discard the task.
    Discard,
    /// A turn ended after changing files: run the project's checks.
    TurnChangedFiles,
    /// Sign-in: run this login command in a terminal in the task's worktree.
    RunInTerminal { label: String, command: String },
    /// The chat's Rewind on turn N: ask, then `rewind`.
    Rewind(u32),
    /// A turn started, ended or was rewound (menus).
    TurnsChanged,
    /// A permission request came or was answered (Allow All Pending's menu item).
    PermissionsChanged,
    /// The task's reviewer started, changed state or went away (save, sidebar).
    Review,
    /// A released agent is back where it was (no new status to alert for).
    Resumed,
    /// Plan mode went on or off, or a plan's comments changed (menus).
    PlanChanged,
}

/// What makes an `AgentTask` a reviewer: a second-opinion session on
/// another task, in that task's worktree, kept read-only by Qur (every
/// permission request denied, plan / read-only mode). Not an OS sandbox.
#[derive(Clone, Debug)]
pub struct ReviewRole {
    pub subject: Subject,
    /// The model to switch to once the session is open (None: its default).
    pub model: Option<String>,
    /// For the sidebar: "codex", "opus".
    pub short: String,
}

#[derive(Clone)]
enum ChatItem {
    User(String),
    Agent(String),
    Thought(String),
    Tool(ToolCallId),
    Notice(String),
    /// Command output (worktree setup), shown in the code font.
    Output(String),
    /// An image in a message, and its size in pixels.
    Image(Arc<gpui::Image>, (u32, u32)),
    /// A file or web link in a message: click opens it.
    Link { uri: String, name: String },
    /// What you attached to the message above: images and files, shown
    /// small under it; click shows one.
    Files(Vec<Sent>),
    /// A failed or cut-short turn, or the agent exiting.
    Failure(Failure),
    /// End of turn N: "3 files · +12 −4 · View · Rewind".
    Turn(u32),
}

/// One attachment shown under a sent message.
#[derive(Clone)]
enum Sent {
    /// With its size in pixels (None for SVG), and the file it came from.
    Image(Arc<gpui::Image>, Option<(u32, u32)>, Option<PathBuf>),
    File(PathBuf),
}

/// What an error callout shows: a line, the full error (stderr tail…), and
/// the message to Retry / Edit & resend when there is one.
#[derive(Clone)]
struct Failure {
    title: String,
    /// The readable reason, when the raw detail buries it (`AcpError::summary`).
    summary: Option<String>,
    detail: Option<String>,
    resend: Option<Outgoing>,
}

/// The agent refused to start a session or a turn until the user signs in
/// (JSON-RPC -32000). Messages stay queued meanwhile.
struct SignIn {
    /// The agent's words.
    message: String,
    /// The method being tried (`authenticate` running, or a restart).
    busy: Option<String>,
    /// A terminal login was started: (method name, the command it runs).
    login: Option<(String, String)>,
    error: Option<String>,
}

struct Permission {
    id: RequestId,
    tool: ToolCallUpdate,
    options: Vec<PermissionOption>,
}

/// Find in Chat (Cmd+F): the query field and every match.
struct ChatFind {
    input: Entity<TextInput>,
    query: String,
    /// Every match as (chat row, its number within the row), oldest first.
    matches: Vec<(usize, usize)>,
    /// Index into `matches`.
    current: Option<usize>,
    /// The chat changed since `matches` was computed.
    dirty: bool,
    _sub: Subscription,
}

/// Stage layout (decision 25).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    /// Watch + side notes.
    A,
    /// One view at a time.
    B,
    /// Watch + pull-up drawer.
    C,
}

impl Layout {
    pub fn parse(s: &str) -> Layout {
        match s {
            "B" => Layout::B,
            "C" => Layout::C,
            _ => Layout::A,
        }
    }
    pub fn next(self) -> Layout {
        match self {
            Layout::A => Layout::B,
            Layout::B => Layout::C,
            Layout::C => Layout::A,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Layout::A => "A",
            Layout::B => "B",
            Layout::C => "C",
        }
    }
}

/// Layout B's segments.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Live,
    Todo,
    Chat,
    Changes,
}

impl View {
    pub fn panel(self) -> Panel {
        match self {
            View::Live => Panel::Live,
            View::Todo => Panel::Todo,
            View::Chat => Panel::Session,
            View::Changes => Panel::Changes,
        }
    }
}

/// One message on its way to the agent.
#[derive(Clone)]
struct Outgoing {
    /// What the user typed (shown in the chat).
    text: String,
    mentions: Vec<PathBuf>,
    /// Browser picks: context text for the agent, and their chip labels.
    context: Vec<String>,
    labels: Vec<String>,
    /// Screenshots, pasted images and image files (with the file's path).
    images: Vec<(Arc<gpui::Image>, Option<PathBuf>)>,
    /// Other files pasted or dropped in: sent as links.
    files: Vec<PathBuf>,
}

impl Outgoing {
    fn plain(text: String, mentions: Vec<PathBuf>) -> Self {
        Outgoing { text, mentions, context: Vec::new(), labels: Vec::new(), images: Vec::new(), files: Vec::new() }
    }
    /// The chat row of what was attached, if anything was.
    fn sent(&self) -> Option<ChatItem> {
        let images = self.images.iter().map(|(img, path)| Sent::Image(img.clone(), crate::files::image_size(img.bytes()), path.clone()));
        let all: Vec<Sent> = images.chain(self.files.iter().cloned().map(Sent::File)).collect();
        (!all.is_empty()).then_some(ChatItem::Files(all))
    }
    fn shown(&self) -> String {
        if self.labels.is_empty() { self.text.clone() } else { format!("{}\n\n＋ {}", self.text, self.labels.join(", ")) }
    }
}

pub struct AgentTask {
    pub record: TaskRecord,
    pub monitor: Entity<TaskMonitor>,
    pub live: Entity<LivePane>,
    pub layout: Layout,
    pub def: AgentDef,
    pub accent: Rgba,
    pub status: Status,
    client: Option<Client>,
    session_id: Option<SessionId>,
    load_session: bool,
    /// How the agent was started (a terminal login may reuse it).
    agent_cmd: Option<AgentCommand>,
    /// Sign-in methods the agent offered at `initialize`.
    auth_methods: Vec<AuthMethod>,
    /// Shown while the agent wants the user to sign in.
    sign_in: Option<SignIn>,
    /// The last message sent (Retry / Edit & resend).
    last_sent: Option<Outgoing>,
    chat: Vec<ChatItem>,
    tools: HashMap<ToolCallId, ToolCall>,
    list: ListState,
    /// Text sizes and zoom the chat rows were measured at; the list caches
    /// row heights, so a change re-measures them.
    measured_at: (f32, f32, f32),
    plan: Vec<PlanEntry>,
    /// Permission requests waiting for an answer, oldest (the one shown) first.
    permissions: VecDeque<Permission>,
    find: Option<ChatFind>,
    pub prompt: Entity<PromptInput>,
    /// Prompt to send once the session is ready.
    queued: Option<Outgoing>,
    /// Messages sent during a turn, sent in order when it ends (decision 22).
    pending: Vec<Outgoing>,
    /// The agent accepts image blocks (promptCapabilities.image).
    images_ok: bool,
    /// Per-task Browser view (decision 26).
    pub browser: Entity<BrowserPane>,
    /// Layouts A/C: what the middle pane shows.
    /// The task's diff, Merge, review comments (M4).
    pub changes: Entity<ChangesView>,
    /// Files changed during the current turn (reload the browser when it ends).
    turn_wrote: bool,
    /// Slash commands the agent advertised.
    pub commands: Vec<qur_acp::AvailableCommand>,
    /// Session settings the agent offers (mode, model…), shown above the prompt.
    pub settings: Vec<ConfigOption>,
    /// Chat items folded the other way from their default (tool call ids,
    /// `thought-<ix>`).
    toggled: HashSet<String>,
    /// Context tokens used / window, and session cost (USD) if known.
    usage: Option<(u64, u64, Option<f64>)>,
    /// Tokens this task's turns used since Qur started it (`TurnUsage`).
    tokens: u64,
    /// Plan mode: the plan shown and your comments on it (plan.rs).
    plan_review: plan::PlanReview,
    /// The mode the task was in before Qur put it in plan mode.
    pre_plan_mode: Option<String>,
    /// Put the session in plan mode as soon as it opens (Plan first).
    pub plan_first: bool,
    /// Subagents this agent started, oldest first (shown under it in the sidebar).
    pub subagents: Vec<Subagent>,
    /// The subagent shown in place of the chat (clicked in the sidebar).
    pub open_subagent: Option<SessionId>,
    /// Setup command still running for a new worktree.
    pub setting_up: bool,
    focus: FocusHandle,
    turn_started: Option<Instant>,
    /// Tool calls of the current (or last) turn, and the files they changed:
    /// the strip above the prompt. Cleared when the next turn starts.
    turn_tools: Vec<ToolCallId>,
    turn_files: Vec<FileChange>,
    /// The project folder (Rewind refuses it).
    project_root: PathBuf,
    /// The running turn's number (its checkpoint is in `record.turns`).
    turn_n: Option<u32>,
    /// Rewind to before this turn once the stopped turn has ended.
    rewind_after_stop: Option<u32>,
    /// Told to the agent with the next message after a Rewind.
    rewind_note: Option<String>,
    /// A Rewind is restoring files: messages wait.
    rewinding: bool,
    /// Set when this task is itself a reviewer (see `ReviewRole`).
    pub review: Option<ReviewRole>,
    /// This task's reviewer (Review Agent's Work…), shown under it in the sidebar.
    pub reviewer: Option<Entity<AgentTask>>,
    /// The stage's chat panel shows the reviewer instead of the chat.
    pub open_review: bool,
    /// The reviewer finished while you weren't looking at it.
    pub review_unread: bool,
    /// When the task last did or showed anything (idle release counts from here).
    last_active: Instant,
    /// Set while the agent's process is released for being idle: how to
    /// pick up again (see `release`).
    resume: Option<Resume>,
    /// A released session is loading again: the history it replays is
    /// already in the chat, so it's skipped until `Event::Loaded`.
    replaying: bool,
    _review_subs: Vec<Subscription>,
    _events: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

/// What a released agent goes back to once its session is loaded again.
struct Resume {
    status: Status,
    /// Session settings the user had (mode, model…) as (id, value): a
    /// loaded session may start from the agent's defaults.
    settings: Vec<(String, String)>,
}

impl EventEmitter<AgentEvent> for AgentTask {}

/// The views a task shows besides its chat (a reviewer borrows its task's).
struct Parts {
    monitor: Entity<TaskMonitor>,
    live: Entity<LivePane>,
    browser: Entity<BrowserPane>,
    changes: Entity<ChangesView>,
}

impl AgentTask {
    #[allow(clippy::too_many_arguments)]
    pub fn new(root: PathBuf, record: TaskRecord, def: AgentDef, accent: Rgba, layout: Layout, wrap: bool, tab: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project_root = root.clone();
        let changes = cx.new(|cx| ChangesView::new(root, record.worktree.clone(), record.base.clone(), record.branch.clone(), cx).with_pr(record.pr_url.clone()));
        let changes_sub = cx.subscribe_in(&changes, window, |this, _, ev: &ChangesEvent, window, cx| match ev {
            ChangesEvent::Merge => cx.emit(AgentEvent::Merge),
            ChangesEvent::PullRequest => cx.emit(AgentEvent::PullRequest),
            ChangesEvent::Discard => cx.emit(AgentEvent::Discard),
            ChangesEvent::KeepWorking => {
                let f = this.prompt.read(cx).focus.clone();
                window.focus(&f, cx);
            }
            ChangesEvent::SendComments(text) => {
                let text = text.clone();
                this.prompt.update(cx, |p, cx| p.set_text(&text, cx));
                this.submit(cx);
            }
            ChangesEvent::Open(p, line) => cx.emit(AgentEvent::OpenFile(p.clone(), *line)),
            ChangesEvent::Rewind(n) => cx.emit(AgentEvent::Rewind(*n)),
        });
        let monitor = cx.new(|cx| TaskMonitor::new(record.worktree.clone(), record.base.clone(), accent, record.in_root, cx));
        let label = def.label.clone();
        let live = cx.new(|cx| LivePane::new(monitor.clone(), label, wrap, tab, window, cx));
        let live_sub = cx.subscribe_in(&live, window, |this, _, ev: &LiveEvent, _, cx| match ev {
            LiveEvent::TakeOver => this.stop(cx),
            LiveEvent::Open(p) => cx.emit(AgentEvent::OpenFile(p.clone(), None)),
        });
        let monitor_sub = cx.observe(&monitor, |_, _, cx| cx.notify());
        let files_sub = cx.subscribe(&monitor, |this, _, ev: &crate::monitor::MonitorEvent, _| {
            if let crate::monitor::MonitorEvent::Files(_) = ev
                && matches!(this.status, Status::Working | Status::NeedsYou)
            {
                this.turn_wrote = true;
            }
        });
        let browser = cx.new(|cx| BrowserPane::new(record.browser_url.clone(), accent, window, cx));
        let browser_sub = cx.subscribe_in(&browser, window, |this, _, ev: &BrowserPaneEvent, window, cx| match ev {
            BrowserPaneEvent::Attach(a) => {
                let a = a.clone();
                this.prompt.update(cx, |p, cx| p.attach(a, cx));
                // The pick was a click in the page, which took the keyboard.
                this.browser.read(cx).release_focus();
                let f = this.prompt.read(cx).focus.clone();
                window.focus(&f, cx);
                // Dev/test: QUR_DEBUG_SEND="text" sends right after a pick.
                if let Ok(text) = std::env::var("QUR_DEBUG_SEND") {
                    this.prompt.update(cx, |p, cx| p.set_text(&text, cx));
                    this.submit(cx);
                }
            }
            BrowserPaneEvent::Navigated(url) => {
                this.record.browser_url = Some(url.clone());
                cx.emit(AgentEvent::Changed);
            }
        });
        let parts = Parts { monitor, live, browser, changes };
        Self::build(record, def, accent, layout, project_root, parts, None, vec![live_sub, monitor_sub, browser_sub, files_sub, changes_sub], window, cx)
    }

    /// A reviewer for `parent` (Review Agent's Work…): its own agent process
    /// and chat, in the parent's worktree. It shares the parent's heatmap,
    /// Live, Browser and Changes views (no second file watcher), and makes
    /// no turn checkpoints (it doesn't change files).
    pub fn new_reviewer(parent: &AgentTask, record: TaskRecord, def: AgentDef, role: ReviewRole, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let parts = Parts { monitor: parent.monitor.clone(), live: parent.live.clone(), browser: parent.browser.clone(), changes: parent.changes.clone() };
        Self::build(record, def, parent.accent, parent.layout, parent.project_root.clone(), parts, Some(role), Vec::new(), window, cx)
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        record: TaskRecord,
        def: AgentDef,
        accent: Rgba,
        layout: Layout,
        project_root: PathBuf,
        parts: Parts,
        review: Option<ReviewRole>,
        mut subs: Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Parts { monitor, live, browser, changes } = parts;
        let wt = record.worktree.clone();
        let placeholder = match &review {
            Some(_) => format!("Ask {} a follow-up (read-only)…", def.label),
            None => format!("Ask {}…  (@ to mention a file)", def.label),
        };
        let prompt = cx.new(|cx| PromptInput::new(placeholder, wt, cx));
        let sub = cx.subscribe_in(&prompt, window, |this, _, ev: &PromptEvent, _, cx| match ev {
            PromptEvent::Submit => this.submit(cx),
            PromptEvent::Cancel => this.stop(cx),
            PromptEvent::MentionRequested => cx.emit(AgentEvent::MentionRequested),
            PromptEvent::SlashRequested => cx.emit(AgentEvent::SlashRequested),
            PromptEvent::UpInEmpty => this.edit_queued(this.pending.len().wrapping_sub(1), cx),
            PromptEvent::Preview(p) => cx.emit(AgentEvent::Preview(p.clone())),
            PromptEvent::Changed => {}
        });
        let list = ListState::new(0, ListAlignment::Bottom, px(400.));
        list.set_follow_mode(gpui::FollowMode::Tail);
        // Re-render on scroll: the "↓" button shows while you're scrolled up.
        let weak = cx.weak_entity();
        list.set_scroll_handler(move |_, _, cx| {
            weak.update(cx, |_, cx| cx.notify()).ok();
        });
        let record_queue = record
            .queued
            .iter()
            .map(|q| Outgoing {
                text: q.text.clone(),
                mentions: q.mentions.clone(),
                context: q.context.clone(),
                labels: q.labels.clone(),
                images: Vec::new(),
                files: q.files.clone(),
            })
            .collect();
        AgentTask {
            record,
            monitor,
            live,
            layout,
            def,
            accent,
            status: Status::Stopped,
            client: None,
            session_id: None,
            load_session: false,
            agent_cmd: None,
            auth_methods: Vec::new(),
            sign_in: None,
            last_sent: None,
            chat: Vec::new(),
            tools: HashMap::new(),
            list,
            measured_at: (0., 0., 0.),
            plan: Vec::new(),
            permissions: VecDeque::new(),
            find: None,
            prompt,
            queued: None,
            // Saved by an earlier run (see `save_queue`).
            pending: record_queue,
            images_ok: false,
            browser,
            changes,
            turn_wrote: false,
            commands: Vec::new(),
            settings: Vec::new(),
            toggled: HashSet::new(),
            usage: None,
            tokens: 0,
            plan_review: plan::PlanReview::default(),
            pre_plan_mode: None,
            plan_first: false,
            subagents: Vec::new(),
            open_subagent: None,
            setting_up: false,
            focus: cx.focus_handle(),
            turn_started: None,
            turn_tools: Vec::new(),
            turn_files: Vec::new(),
            project_root,
            turn_n: None,
            rewind_after_stop: None,
            rewind_note: None,
            rewinding: false,
            review,
            reviewer: None,
            open_review: false,
            review_unread: false,
            last_active: Instant::now(),
            resume: None,
            replaying: false,
            _review_subs: Vec::new(),
            _events: None,
            _subs: {
                subs.push(sub);
                subs
            },
        }
    }

    /// Theme change: the accent also colours the heatmap and the Live mark.
    pub fn set_accent(&mut self, accent: Rgba, cx: &mut Context<Self>) {
        self.accent = accent;
        self.monitor.update(cx, |m, _| m.accent = accent);
        self.live.update(cx, |l, _| l.accent = accent);
        self.browser.update(cx, |b, _| b.accent = accent);
    }
}

/// Lines an unfolded tool call shows of a diff, output or raw input.
const MAX_TOOL_LINES: usize = 200;
/// Lines of an exited agent's stderr an error callout keeps (of ~4 KB).
const MAX_STDERR_LINES: usize = 80;

/// `s` quoted for a POSIX shell when it needs it.
fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-=:@%+,".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The last `max` lines of `text`, with a note of how many were left out.
fn tail_lines(text: &str, max: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max {
        return text.to_string();
    }
    format!("… {} earlier lines\n{}", lines.len() - max, lines[lines.len() - max..].join("\n"))
}

/// One file a turn changed, with its changed-line counts.
#[derive(Clone, Debug, PartialEq)]
struct FileChange {
    path: PathBuf,
    add: usize,
    del: usize,
}

/// The files these tool calls changed, first-touched first: each diff's
/// changed lines, summed per file; an edit without a diff counts its
/// locations. Failed calls changed nothing.
fn turn_changes(tools: &[&ToolCall]) -> Vec<FileChange> {
    let mut out: Vec<FileChange> = Vec::new();
    let mut add_to = |path: &Path, add: usize, del: usize| match out.iter_mut().find(|f| f.path == path) {
        Some(f) => {
            f.add += add;
            f.del += del;
        }
        None => out.push(FileChange { path: path.to_path_buf(), add, del }),
    };
    for t in tools.iter().filter(|t| t.status != ToolStatus::Failed) {
        let mut diffed = false;
        for c in &t.content {
            if let ToolContent::Diff { path, old_text, new_text } = c {
                let d = line_diff(old_text.as_deref().unwrap_or(""), new_text);
                add_to(path, d.iter().filter(|(k, _)| *k == '+').count(), d.iter().filter(|(k, _)| *k == '-').count());
                diffed = true;
            }
        }
        if !diffed && matches!(t.kind, ToolKind::Edit | ToolKind::Delete | ToolKind::Move) {
            for l in &t.locations {
                add_to(&l.path, 0, 0);
            }
        }
    }
    out
}

fn change_totals(files: &[FileChange]) -> (usize, usize) {
    files.iter().fold((0, 0), |(a, d), f| (a + f.add, d + f.del))
}

/// A tool kind in a word (the chat's left column, Copy Chat as Markdown).
fn kind_word(k: ToolKind) -> &'static str {
    match k {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "run",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "mode",
        ToolKind::Other => "tool",
    }
}

/// A tool call's title, or its kind when it has none.
fn tool_title(t: &ToolCall) -> String {
    if t.title.is_empty() { kind_word(t.kind).to_string() } else { t.title.clone() }
}

/// Each file diff a tool call carries, as changed lines.
fn tool_diffs(t: &ToolCall) -> Vec<(&PathBuf, Vec<(char, &str)>)> {
    t.content
        .iter()
        .filter_map(|c| match c {
            ToolContent::Diff { path, old_text, new_text } => Some((path, line_diff(old_text.as_deref().unwrap_or(""), new_text))),
            _ => None,
        })
        .collect()
}

/// The run of consecutive tool calls chat row `ix` belongs to (None when
/// it isn't a tool call). Two or more make a group.
fn tool_run(chat: &[ChatItem], ix: usize) -> Option<Range<usize>> {
    let is_tool = |i: usize| matches!(chat.get(i), Some(ChatItem::Tool(_)));
    if !is_tool(ix) {
        return None;
    }
    let start = (0..ix).rev().take_while(|&i| is_tool(i)).last().unwrap_or(ix);
    let end = (ix + 1..chat.len()).take_while(|&i| is_tool(i)).last().map_or(ix + 1, |e| e + 1);
    Some(start..end)
}

/// A group's summary: "Read 4 files, ran 2 commands", kinds in the order
/// they first came.
fn group_summary(tools: &[&ToolCall]) -> String {
    // Kinds that read the same are counted together.
    let key = |k: ToolKind| match k {
        ToolKind::Read => 0,
        ToolKind::Edit => 1,
        ToolKind::Delete => 2,
        ToolKind::Move => 3,
        ToolKind::Search => 4,
        ToolKind::Execute => 5,
        ToolKind::Fetch => 6,
        ToolKind::Think | ToolKind::SwitchMode | ToolKind::Other => 7,
    };
    let mut counts: Vec<(u8, usize)> = Vec::new();
    for t in tools {
        let k = key(t.kind);
        match counts.iter_mut().find(|(x, _)| *x == k) {
            Some((_, n)) => *n += 1,
            None => counts.push((k, 1)),
        }
    }
    let only = counts.len() == 1;
    let n = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let parts: Vec<String> = counts
        .iter()
        .map(|&(k, c)| match k {
            0 => format!("read {}", n(c, "file", "files")),
            1 => format!("edited {}", n(c, "file", "files")),
            2 => format!("deleted {}", n(c, "file", "files")),
            3 => format!("moved {}", n(c, "file", "files")),
            4 => format!("ran {}", n(c, "search", "searches")),
            5 => format!("ran {}", n(c, "command", "commands")),
            6 => format!("fetched {}", n(c, "page", "pages")),
            _ if only => format!("used {}", n(c, "tool", "tools")),
            _ => format!("used {}", n(c, "other tool", "other tools")),
        })
        .collect();
    let text = parts.join(", ");
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

/// Copy Chat as Markdown: the chat for pasting elsewhere. Your messages
/// are quoted under "## You", the agent's Markdown is kept as it is, tool
/// calls become one line each with their status, and thoughts are left out.
fn chat_markdown(title: &str, agent: &str, chat: &[ChatItem], tools: &HashMap<ToolCallId, ToolCall>, turns: &[qur_agents::Turn]) -> String {
    // Blocks are separated by a blank line; tool lines in a row form one list.
    fn add(out: &mut String, text: &str, tool_line: bool, prev_tool: &mut bool) {
        out.push_str(if tool_line && *prev_tool { "\n" } else { "\n\n" });
        out.push_str(text);
        *prev_tool = tool_line;
    }
    let quote = |t: &str| t.trim_end().lines().map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") }).collect::<Vec<_>>().join("\n");
    let mut out = format!("# {}", title.trim());
    let mut prev_tool = false;
    // The agent's heading goes once after each of your messages.
    let mut spoke = false;
    for item in chat {
        let agent_side = matches!(item, ChatItem::Agent(_) | ChatItem::Tool(_) | ChatItem::Image(..) | ChatItem::Link { .. });
        if agent_side && !spoke {
            add(&mut out, &format!("## {agent}"), false, &mut prev_tool);
            spoke = true;
        }
        match item {
            ChatItem::User(t) => {
                add(&mut out, "## You", false, &mut prev_tool);
                add(&mut out, &quote(t), false, &mut prev_tool);
                spoke = false;
            }
            ChatItem::Agent(t) if !t.trim().is_empty() => add(&mut out, t.trim(), false, &mut prev_tool),
            ChatItem::Agent(_) | ChatItem::Thought(_) => {}
            ChatItem::Tool(id) => {
                let Some(t) = tools.get(id) else { continue };
                let glyph = match t.status {
                    ToolStatus::Completed => "✓",
                    ToolStatus::Failed => "✕",
                    ToolStatus::Pending | ToolStatus::InProgress => "…",
                };
                let mut line = format!("- {glyph} {}: {}", kind_word(t.kind), tool_title(t).split_whitespace().collect::<Vec<_>>().join(" "));
                let diffs = tool_diffs(t);
                let add_n = diffs.iter().flat_map(|(_, d)| d).filter(|(k, _)| *k == '+').count();
                let del_n = diffs.iter().flat_map(|(_, d)| d).filter(|(k, _)| *k == '-').count();
                if add_n + del_n > 0 {
                    line.push_str(&format!(" (+{add_n} −{del_n})"));
                }
                if let Some(Some(c)) = t.exit
                    && c != 0
                {
                    line.push_str(&format!(" (exit {c})"));
                }
                add(&mut out, &line, true, &mut prev_tool);
            }
            ChatItem::Notice(t) => add(&mut out, &format!("*{}*", t.trim()), false, &mut prev_tool),
            ChatItem::Output(t) => {
                let fence = if t.contains("```") { "````" } else { "```" };
                add(&mut out, &format!("{fence}\n{}\n{fence}", t.trim_end()), false, &mut prev_tool);
            }
            ChatItem::Image(_, (w, h)) => add(&mut out, &format!("*[image {w}×{h}]*"), false, &mut prev_tool),
            ChatItem::Link { uri, name } => add(&mut out, &format!("[{name}]({uri})"), false, &mut prev_tool),
            ChatItem::Files(files) => {
                let names: Vec<String> = files
                    .iter()
                    .map(|f| match f {
                        Sent::Image(_, _, Some(p)) | Sent::File(p) => format!("[{}](file://{})", crate::files::chip_name(p), p.display()),
                        Sent::Image(..) => "*[image]*".into(),
                    })
                    .collect();
                add(&mut out, &format!("> ＋ {}", names.join(", ")), false, &mut prev_tool);
            }
            ChatItem::Failure(f) => {
                let text = match &f.summary {
                    Some(s) => format!("**{}**\n{s}", f.title),
                    None => format!("**{}**", f.title),
                };
                add(&mut out, &quote(&text), false, &mut prev_tool);
            }
            ChatItem::Turn(n) => {
                let Some(t) = turns.iter().find(|x| x.n == *n) else { continue };
                let rewound = if t.rewound { ", rewound" } else { "" };
                let text = format!("*Turn {n}: {} file{} changed, +{} −{}{rewound}*", t.files, if t.files == 1 { "" } else { "s" }, t.added, t.removed);
                add(&mut out, &text, false, &mut prev_tool);
            }
        }
    }
    out.push('\n');
    out
}

fn code_block(text: &str, look: &Look) -> gpui::Div {
    code_block_find(text, look, None)
}

/// `code_block` with Find in Chat's matches marked.
fn code_block_find(text: &str, look: &Look, hl: Option<&FindHl>) -> gpui::Div {
    let ui = &look.theme.ui;
    div()
        .px(look.z(10.))
        .py(look.z(6.))
        .rounded(look.z(4.))
        .bg(look.c(ui.panel_bg))
        .border_1()
        .border_color(look.c(ui.border))
        .font_family(look.code_font.clone())
        .text_size(look.text(0.))
        .text_color(look.c(ui.text_secondary))
        .when(!text.is_empty(), |d| d.child(marked(hl, text.to_string())))
}

/// `text`, with Find in Chat's matches marked when it's open.
fn marked(hl: Option<&FindHl>, text: String) -> StyledText {
    match hl {
        Some(h) => h.text(text),
        None => StyledText::new(text),
    }
}

/// Line diff of `old` to `new`: ('-', line) removed, ('+', line) added,
/// (' ', line) kept, in order. Longest common subsequence; past ~4M cells
/// (two files of 2,000 lines) it just shows all old removed, all new added.
fn line_diff<'a>(old: &'a str, new: &'a str) -> Vec<(char, &'a str)> {
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    // Common head and tail first: an edit usually touches a few lines.
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut out: Vec<(char, &str)> = a[..head].iter().map(|l| (' ', *l)).collect();
    if am.len() * bm.len() > 4_000_000 {
        out.extend(am.iter().map(|l| ('-', *l)));
        out.extend(bm.iter().map(|l| ('+', *l)));
    } else {
        // lcs[i][j]: common lines of am[i..] and bm[j..].
        let (n, m) = (am.len(), bm.len());
        let mut lcs = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * (m + 1) + j] = if am[i] == bm[j] { lcs[(i + 1) * (m + 1) + j + 1] + 1 } else { lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && am[i] == bm[j] {
                out.push((' ', am[i]));
                (i, j) = (i + 1, j + 1);
            // Ties remove first, so a changed line reads `-old` then `+new`.
            } else if j < m && (i == n || lcs[i * (m + 1) + j + 1] > lcs[(i + 1) * (m + 1) + j]) {
                out.push(('+', bm[j]));
                j += 1;
            } else {
                out.push(('-', am[i]));
                i += 1;
            }
        }
    }
    out.extend(a[a.len() - tail..].iter().map(|l| (' ', *l)));
    out
}

impl Focusable for AgentTask {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AgentTask {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        // Find in Chat: the chat changed since its matches were counted.
        if self.find.as_ref().is_some_and(|f| f.dirty) {
            self.refind(cx.entity_id());
        }
        let sizes = (look.ui_size, look.code_size, look.zoom);
        if sizes != self.measured_at {
            self.measured_at = sizes;
            self.list.remeasure();
        }
        self.sync_plan();
        let ui = look.theme.ui.clone();
        // A reviewer is shown inside its task's chat panel (`review_view`):
        // just its chat and prompt.
        if self.review.is_some() {
            return div()
                .key_context("AgentView")
                .track_focus(&self.focus)
                // Shift+Tab here mustn't reach the task's own mode.
                .on_action(cx.listener(Self::cycle_mode))
                .size_full()
                .flex()
                .flex_col()
                .child(self.notes(true, &look, cx).flex_1().min_h(px(0.)))
                .into_any_element();
        }
        let header = self.header(&look, cx);
        // The stage is a dock you can rearrange (dock.rs).
        let todo_shown = Docks::get(cx, Kind::Acp).is_visible(Panel::Todo);
        let body = div().flex_1().min_h(px(0.)).child(dock::render(Kind::Acp, &look, cx, &mut |p, cx| match p {
            Panel::Session => self.notes(!todo_shown, &look, cx).size_full().into_any_element(),
            Panel::Live => div().size_full().flex().flex_col().child(div().flex_1().min_h(px(0.)).child(self.live.clone())).into_any_element(),
            Panel::Browser => div().size_full().child(self.browser.clone()).into_any_element(),
            Panel::Changes => div().size_full().child(self.changes.clone()).into_any_element(),
            Panel::Todo => div()
                .size_full()
                .flex()
                .flex_col()
                .children(self.plan_strip(&look))
                .when(self.plan.is_empty(), |d| d.child(div().flex_1().flex().items_center().justify_center().text_color(look.c(look.theme.ui.text_muted)).child("No plan yet.")))
                .into_any_element(),
        }));
        div()
            .key_context("AgentView")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::cycle_mode))
            .size_full()
            .flex()
            .flex_col()
            // Without gaps the stage is one region; with them the header and
            // each dock pane paint their own tile.
            .bg(look.region_bg(ui.editor_bg))
            .gap(look.gap())
            .child(header)
            .child(body)
            .into_any_element()
    }
}

/// The first 12 characters of an object id (logs).
fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(d: &[(char, &str)]) -> Vec<String> {
        d.iter().filter(|(k, _)| *k != ' ').map(|(k, l)| format!("{k}{l}")).collect()
    }

    #[test]
    fn line_diff_counts_only_changed_lines() {
        // One line edited in a 5-line file: -1 +1, not -5 +5.
        let d = line_diff("a\nb\nc\nd\ne", "a\nb\nC\nd\ne");
        assert_eq!(changed(&d), ["-c", "+C"]);
        assert_eq!(d.len(), 6, "kept lines stay, in order");
        // A new file is all added; an insertion keeps its neighbours.
        assert_eq!(changed(&line_diff("", "x\ny")), ["+x", "+y"]);
        assert_eq!(changed(&line_diff("a\nc", "a\nb\nc")), ["+b"]);
        assert_eq!(changed(&line_diff("a\nb\nc", "a\nc")), ["-b"]);
        assert!(changed(&line_diff("same\n", "same\n")).is_empty());
        // Middle edits around repeated lines.
        assert_eq!(changed(&line_diff("x\na\nx\nb\nx", "x\nb\nx\na\nx")).len(), 4);
    }

    #[test]
    fn turn_changes_sums_per_file() {
        let diff = |p: &str, old: Option<&str>, new: &str| ToolContent::Diff { path: p.into(), old_text: old.map(str::to_string), new_text: new.into() };
        let loc = |p: &str| vec![qur_acp::Location { path: p.into(), line: None }];
        let edit1 = ToolCall { kind: ToolKind::Edit, status: ToolStatus::Completed, content: vec![diff("/w/a.rs", Some("x\ny"), "x\nY\nz")], ..Default::default() };
        let edit2 = ToolCall { kind: ToolKind::Edit, status: ToolStatus::InProgress, content: vec![diff("/w/a.rs", Some("q"), "")], ..Default::default() };
        let write = ToolCall { kind: ToolKind::Edit, status: ToolStatus::Completed, content: vec![diff("/w/new.ts", None, "1\n2\n3")], ..Default::default() };
        let failed = ToolCall { kind: ToolKind::Edit, status: ToolStatus::Failed, content: vec![diff("/w/b.rs", None, "nope")], ..Default::default() };
        let moved = ToolCall { kind: ToolKind::Move, status: ToolStatus::Completed, locations: loc("/w/c.rs"), ..Default::default() };
        let read = ToolCall { kind: ToolKind::Read, status: ToolStatus::Completed, locations: loc("/w/d.rs"), ..Default::default() };
        let files = turn_changes(&[&edit1, &read, &write, &edit2, &failed, &moved]);
        let got: Vec<(&str, usize, usize)> = files.iter().map(|f| (f.path.to_str().unwrap(), f.add, f.del)).collect();
        // a.rs: y -> Y, +z (+2 -1), then q removed (-1).
        assert_eq!(got, [("/w/a.rs", 2, 2), ("/w/new.ts", 3, 0), ("/w/c.rs", 0, 0)]);
        assert_eq!(change_totals(&files), (5, 2));
        assert_eq!(change_totals(&[]), (0, 0));
    }

    #[test]
    fn shell_quote_only_when_needed() {
        assert_eq!(shell_quote("/usr/bin/node"), "/usr/bin/node");
        assert_eq!(shell_quote("--claudeai"), "--claudeai");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    fn tool(id: &str, kind: ToolKind, status: ToolStatus, title: &str) -> ToolCall {
        ToolCall { id: id.into(), kind, status, title: title.into(), ..Default::default() }
    }

    #[test]
    fn tool_runs_are_consecutive_calls() {
        let t = |id: &str| ChatItem::Tool(id.into());
        let chat = vec![ChatItem::User("hi".into()), t("a"), t("b"), t("c"), ChatItem::Agent("ok".into()), t("d"), ChatItem::Thought("hm".into()), t("e"), t("f")];
        assert_eq!(tool_run(&chat, 0), None);
        for ix in 1..=3 {
            assert_eq!(tool_run(&chat, ix), Some(1..4));
        }
        assert_eq!(tool_run(&chat, 4), None);
        assert_eq!(tool_run(&chat, 5), Some(5..6), "one call alone isn't a group");
        assert_eq!(tool_run(&chat, 7), Some(7..9));
        assert_eq!(tool_run(&chat, 8), Some(7..9));
        assert_eq!(tool_run(&chat, 99), None);
    }

    #[test]
    fn group_summary_counts_by_kind() {
        let r = |id| tool(id, ToolKind::Read, ToolStatus::Completed, "");
        let (a, b, c, d) = (r("a"), r("b"), r("c"), r("d"));
        let run1 = tool("e", ToolKind::Execute, ToolStatus::Completed, "");
        let run2 = tool("f", ToolKind::Execute, ToolStatus::Failed, "");
        let think = tool("g", ToolKind::Think, ToolStatus::Completed, "");
        let other = tool("h", ToolKind::Other, ToolStatus::Completed, "");
        let edit = tool("i", ToolKind::Edit, ToolStatus::Completed, "");
        assert_eq!(group_summary(&[&a, &run1, &b, &c, &run2, &d]), "Read 4 files, ran 2 commands");
        assert_eq!(group_summary(&[&edit, &a]), "Edited 1 file, read 1 file");
        assert_eq!(group_summary(&[&think, &other]), "Used 2 tools");
        assert_eq!(group_summary(&[&a, &think, &other]), "Read 1 file, used 2 other tools");
        assert_eq!(group_summary(&[]), "");
    }

    #[test]
    fn chat_as_markdown() {
        let mut tools = HashMap::new();
        let mut edit = tool("t1", ToolKind::Edit, ToolStatus::Completed, "Edit src/a.rs");
        edit.content = vec![ToolContent::Diff { path: "/w/src/a.rs".into(), old_text: Some("x\ny".into()), new_text: "x\nY".into() }];
        let mut cmd = tool("t2", ToolKind::Execute, ToolStatus::Failed, "cargo   test\n--release");
        cmd.exit = Some(Some(101));
        tools.insert(edit.id.clone(), edit);
        tools.insert(cmd.id.clone(), cmd);
        let turns = vec![qur_agents::Turn { n: 1, files: 1, added: 1, removed: 1, ..Default::default() }];
        let chat = vec![
            ChatItem::User("Fix it\n\nplease".into()),
            ChatItem::Thought("secret thinking".into()),
            ChatItem::Agent("Sure. **On it.**".into()),
            ChatItem::Tool("t1".into()),
            ChatItem::Tool("t2".into()),
            ChatItem::Notice("Stopped.".into()),
            ChatItem::Output("$ make\n```\nok".into()),
            ChatItem::Failure(Failure { title: "Claude couldn't finish.".into(), summary: Some("Rate limited".into()), detail: Some("raw".into()), resend: None }),
            ChatItem::Turn(1),
            ChatItem::User("Thanks".into()),
            ChatItem::Agent("You're welcome.".into()),
        ];
        let md = chat_markdown("My task", "Claude", &chat, &tools, &turns);
        let want = "# My task

## You

> Fix it
>
> please

## Claude

Sure. **On it.**

- ✓ edit: Edit src/a.rs (+1 −1)
- ✕ run: cargo test --release (exit 101)

*Stopped.*

````
$ make
```
ok
````

> **Claude couldn't finish.**
> Rate limited

*Turn 1: 1 file changed, +1 −1*

## You

> Thanks

## Claude

You're welcome.
";
        assert_eq!(md, want);
        assert!(!md.contains("secret"), "thoughts are left out");
    }

    #[test]
    fn usage_in_the_header() {
        use stage::usage_readout;
        assert_eq!(usage_readout(121_175, Some(0.11930126)).as_deref(), Some("121.2k tokens · $0.12"));
        assert_eq!(usage_readout(640, None).as_deref(), Some("640 tokens"));
        assert_eq!(usage_readout(2_350_000, Some(1.5)).as_deref(), Some("2.4M tokens · $1.50"));
        assert_eq!(usage_readout(0, Some(0.04)).as_deref(), Some("$0.04"));
        assert_eq!(usage_readout(0, None), None, "nothing reported, nothing shown");
        assert_eq!(usage_readout(0, Some(0.)), None);
    }

    #[test]
    fn tail_lines_keeps_the_end() {
        assert_eq!(tail_lines("1\n2\n3", 5), "1\n2\n3");
        assert_eq!(tail_lines("1\n2\n3\n4", 2), "… 2 earlier lines\n3\n4");
    }
}
