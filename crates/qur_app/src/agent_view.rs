//! One ACP agent task: its process (via `qur_acp`), chat transcript, plan,
//! pending permission and status, plus the stage view that shows it:
//! header, To-do strip, Chat, permission bar and prompt bar.

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
    /// `@` typed in the prompt: open the mention picker for this worktree.
    MentionRequested,
    /// `/` typed: offer this agent's settings and slash commands.
    SlashRequested,
    /// A subagent started, changed state or did something (sidebar).
    Subagents,
    /// Changes view: merge this task's branch into the user's branch.
    Merge,
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
    /// A failed or cut-short turn, or the agent exiting.
    Failure(Failure),
    /// End of turn N: "3 files · +12 −4 · View · Rewind".
    Turn(u32),
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
    /// Browser picks: context text for the agent, plus screenshots.
    context: Vec<String>,
    labels: Vec<String>,
    images: Vec<Vec<u8>>,
}

impl Outgoing {
    fn plain(text: String, mentions: Vec<PathBuf>) -> Self {
        Outgoing { text, mentions, context: Vec::new(), labels: Vec::new(), images: Vec::new() }
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
    _review_subs: Vec<Subscription>,
    _events: Option<Task<()>>,
    _subs: Vec<Subscription>,
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
        let changes = cx.new(|cx| ChangesView::new(root, record.worktree.clone(), record.base.clone(), record.branch.clone(), cx));
        let changes_sub = cx.subscribe_in(&changes, window, |this, _, ev: &ChangesEvent, window, cx| match ev {
            ChangesEvent::Merge => cx.emit(AgentEvent::Merge),
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
            .map(|q| Outgoing { text: q.text.clone(), mentions: q.mentions.clone(), context: q.context.clone(), labels: q.labels.clone(), images: Vec::new() })
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

    // -- chat list -------------------------------------------------------------

    fn push(&mut self, item: ChatItem) {
        // The thought before this item stops streaming: fold it.
        self.remeasure_last_thought();
        let n = self.chat.len();
        self.chat.push(item);
        self.list.splice(n..n, 1);
        self.chat_changed();
        // A tool call joining a run: the group's summary (on its first row)
        // and the row before it change.
        if let Some(run) = tool_run(&self.chat, n).filter(|r| r.len() >= 2) {
            self.list.remeasure_items(run.start..n);
        }
    }

    /// Find in Chat counts matches again before it next draws.
    fn chat_changed(&mut self) {
        if let Some(f) = &mut self.find {
            f.dirty = true;
        }
    }

    /// Re-measure a tool call's row and, in a group, the whole group (its
    /// summary and which rows show depend on every call's status).
    fn remeasure_tool_rows(&mut self, ix: usize) {
        match tool_run(&self.chat, ix) {
            Some(run) => self.list.remeasure_items(run),
            None => self.list.remeasure_items(ix..ix + 1),
        }
    }

    /// Back to the newest message, following it again as it streams.
    fn follow_bottom(&self) {
        self.list.set_follow_mode(gpui::FollowMode::Tail);
        self.list.scroll_to_end();
    }

    /// Rows are measured once; a thought's open/folded state depends on
    /// whether it's still the streaming last item, so re-measure it.
    fn remeasure_last_thought(&mut self) {
        let last = self.chat.len().wrapping_sub(1);
        if matches!(self.chat.last(), Some(ChatItem::Thought(_))) {
            self.list.splice(last..last + 1, 1);
        }
    }

    /// Append streamed text to the last item of the same kind, or start one.
    fn append(&mut self, thought: bool, text: &str) {
        let last = self.chat.len().wrapping_sub(1);
        match self.chat.last_mut() {
            Some(ChatItem::Agent(s)) if !thought => s.push_str(text),
            Some(ChatItem::Thought(s)) if thought => s.push_str(text),
            _ => {
                self.push(if thought { ChatItem::Thought(text.to_string()) } else { ChatItem::Agent(text.to_string()) });
                return;
            }
        }
        self.list.splice(last..last + 1, 1);
        self.chat_changed();
    }

    /// An image or link block from a message: its own chat row.
    fn push_media(&mut self, media: Option<Media>) {
        match media {
            Some(Media::Image { mime, data }) => match gpui::ImageFormat::from_mime_type(&mime) {
                Some(f) => {
                    // The size now: GPUI decodes later, after the row is measured.
                    let size = image::ImageReader::new(std::io::Cursor::new(&data))
                        .with_guessed_format()
                        .ok()
                        .and_then(|r| r.into_dimensions().ok())
                        .filter(|(w, h)| *w > 0 && *h > 0);
                    match size {
                        Some(size) => self.push(ChatItem::Image(Arc::new(gpui::Image::from_bytes(f, data)), size)),
                        None => self.push(ChatItem::Notice("[image that couldn't be read]".into())),
                    }
                }
                None => self.push(ChatItem::Notice(format!("[{mime} image]"))),
            },
            Some(Media::Link { uri, name }) => self.push(ChatItem::Link { uri, name }),
            None => {}
        }
    }

    /// A link in a message: a file opens in the code modal, a local address
    /// in the task's Browser view, anything else in the default browser.
    fn open_link(&mut self, uri: &str, cx: &mut Context<Self>) {
        if let Some(path) = uri.strip_prefix("file://") {
            // `file:///abs/path#L12` or `…:12`: open at that line when given.
            let (path, line) = match path.rsplit_once("#L").or_else(|| path.rsplit_once(':')) {
                Some((p, l)) if l.parse::<u32>().is_ok() => (p, l.parse().ok()),
                _ => (path, None),
            };
            let path = PathBuf::from(path.replace("%20", " "));
            return cx.emit(AgentEvent::OpenFile(path, line));
        }
        let local = ["http://localhost", "http://127.0.0.1", "https://localhost"].iter().any(|p| uri.starts_with(p));
        if local {
            self.browser.update(cx, |b, cx| b.navigate(uri, cx));
            self.show_browser(true, cx);
        } else {
            cx.open_url(uri);
        }
    }

    /// Error callout: send that message again.
    fn retry(&mut self, out: Outgoing, cx: &mut Context<Self>) {
        self.follow_bottom();
        if matches!(self.status, Status::Working | Status::NeedsYou | Status::Starting) || self.setting_up || self.sign_in.is_some() || self.rewinding {
            self.pending.push(out);
            self.save_queue(cx);
            return cx.notify();
        }
        self.send_out(out, cx);
    }

    /// Error callout: the message back in the prompt to change and send.
    fn edit_resend(&mut self, out: &Outgoing, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt.update(cx, |p, cx| p.set_text(&out.text, cx));
        let f = self.prompt.read(cx).focus.clone();
        window.focus(&f, cx);
        cx.notify();
    }

    fn refresh_tool(&mut self, id: &ToolCallId) {
        if let Some(ix) = self.chat.iter().rposition(|c| matches!(c, ChatItem::Tool(t) if t == id)) {
            self.remeasure_tool_rows(ix);
            self.chat_changed();
        }
    }

    // -- lifecycle ---------------------------------------------------------------

    /// Spawn the agent, initialize, then load the saved session (shows its
    /// history) or start a new one. Sends a queued prompt when ready.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.client.is_some() {
            return;
        }
        let path = qur_agents::login_path().to_string();
        let program = qur_agents::which(&self.def.command).map(|p| p.to_string_lossy().into_owned()).unwrap_or(self.def.command.clone());
        let cmd = AgentCommand { program, args: self.def.args.clone(), env: vec![("PATH".into(), path)] };
        let (client, events) = match Client::spawn(&cmd, &self.record.worktree) {
            Ok(x) => x,
            Err(e) => {
                self.set_status(Status::Error(format!("couldn't start {}: {e}", self.def.command)), cx);
                return;
            }
        };
        self.agent_cmd = Some(cmd);
        self.client = Some(client.clone());
        self.set_status(Status::Starting, cx);
        self._events = Some(cx.spawn(async move |this, cx| {
            let mut events = events;
            while let Some(ev) = events.next().await {
                if this.update(cx, |this, cx| this.on_event(ev, cx)).is_err() {
                    break;
                }
            }
        }));
        cx.spawn(async move |this, cx| {
            let init = client.initialize().await;
            let init = match init {
                Ok(Ok(i)) => i,
                Ok(Err(e)) => return this.update(cx, |t, cx| t.fail(format!("initialize: {e}"), cx)).ok(),
                Err(_) => return this.update(cx, |t, cx| t.fail("agent exited during start".into(), cx)).ok(),
            };
            let images = init.raw["agentCapabilities"]["promptCapabilities"]["image"].as_bool().unwrap_or(false);
            this.update(cx, |t, cx| {
                t.load_session = init.load_session;
                t.images_ok = images;
                t.auth_methods = init.auth_methods;
                t.open_session(client, cx);
            })
            .ok()
        })
        .detach();
    }

    /// Load the saved session (shows its history) or start a new one, then
    /// send what's waiting. A sign-in refusal shows the sign-in callout.
    fn open_session(&mut self, client: Client, cx: &mut Context<Self>) {
        let saved = self.record.session_id.clone().filter(|_| self.load_session).map(SessionId::from);
        let cwd = self.record.worktree.clone();
        let role = self.review.clone();
        cx.spawn(async move |this, cx| {
            let session = match saved {
                Some(id) => match client.load_session(&id, &cwd).await {
                    Ok(Ok(settings)) => Some((id, settings)),
                    _ => None,
                },
                None => None,
            };
            let (session, settings) = match session {
                Some(s) => s,
                None => match client.new_session(&cwd).await {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) if e.is_auth_required() => return this.update(cx, |t, cx| t.need_sign_in(&e, cx)).ok(),
                    Ok(Err(e)) => return this.update(cx, |t, cx| t.fail(format!("session/new: {e}"), cx)).ok(),
                    Err(_) => return this.update(cx, |t, cx| t.fail("agent exited during start".into(), cx)).ok(),
                },
            };
            // A reviewer goes to its read-only mode and its model before
            // anything is sent (also after a restart: modes aren't saved).
            let settings = match &role {
                Some(role) => review_setup(&client, &session, settings, role).await,
                None => settings,
            };
            this.update(cx, |t, cx| {
                t.sign_in = None;
                t.settings = settings;
                t.record.session_id = Some(session.to_string());
                t.session_id = Some(session);
                t.set_status(Status::Idle, cx);
                if let Some(out) = t.queued.take() {
                    t.send_out(out, cx);
                } else {
                    t.drain_pending(cx);
                }
            })
            .ok()
        })
        .detach();
    }

    // -- sign-in -------------------------------------------------------------------

    /// The agent wants a sign-in (-32000): show the callout; queued messages
    /// wait (`drain_pending` only runs once a session is ready again).
    fn need_sign_in(&mut self, e: &AcpError, cx: &mut Context<Self>) {
        let again = self.sign_in.as_ref().is_some_and(|s| s.login.is_some() || s.busy.is_some());
        self.sign_in = Some(SignIn {
            message: e.message.clone(),
            busy: None,
            login: None,
            error: again.then(|| "Still not signed in. Finish signing in, then Continue.".to_string()),
        });
        self.set_status(Status::Error("sign-in required".into()), cx);
    }

    /// A sign-in button: a terminal login runs in a Qur terminal (then
    /// Continue restarts the agent, which reads the new login); any other
    /// method is `authenticate`, then the session is opened again.
    fn sign_in_with(&mut self, method_id: &str, cx: &mut Context<Self>) {
        let Some(m) = self.auth_methods.iter().find(|m| m.id == method_id).cloned() else { return };
        let Some(s) = self.sign_in.as_mut() else { return };
        s.error = None;
        if let Some(t) = &m.terminal {
            let Some(agent) = self.agent_cmd.as_ref() else { return };
            let (program, args) = t.command_line(agent);
            let env = t.env.iter().map(|(k, v)| format!("{k}={} ", shell_quote(v))).collect::<String>();
            let command = format!("{env}{}", std::iter::once(&program).chain(&args).map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "));
            s.login = Some((m.name.clone(), command.clone()));
            cx.emit(AgentEvent::RunInTerminal { label: t.label.clone().unwrap_or_else(|| m.name.clone()), command });
            cx.notify();
            return;
        }
        let Some(client) = self.client.clone() else { return self.continue_sign_in(cx) };
        s.busy = Some(m.name.clone());
        let pending = client.authenticate(&m.id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = pending.await;
            this.update(cx, |t, cx| {
                match r {
                    Ok(Ok(())) if t.session_id.is_some() => {
                        // Signed in mid-session (a turn was refused): carry on.
                        t.sign_in = None;
                        t.set_status(Status::Idle, cx);
                        t.drain_pending(cx);
                    }
                    Ok(Ok(())) => t.open_session(client, cx),
                    Ok(Err(e)) => {
                        if let Some(s) = t.sign_in.as_mut() {
                            s.busy = None;
                            s.error = Some(format!("Sign-in didn't complete: {e}"));
                        }
                    }
                    Err(_) => t.sign_in = None,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After a terminal login (or with no method offered): restart the agent
    /// so it picks up the new login, then open the session again.
    fn continue_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.sign_in.as_mut() {
            s.busy = Some("Checking".into());
            s.error = None;
        }
        // The saved session's history is replayed on load: rebuild the chat
        // from it rather than show it twice.
        if self.load_session && self.record.session_id.is_some() && self.session_id.is_some() {
            self.chat.clear();
            self.tools.clear();
            self.toggled.clear();
            self.list.reset(0);
            self.chat_changed();
        }
        if let Some(c) = self.client.take() {
            std::thread::spawn(move || c.shutdown(Duration::from_secs(2)));
        }
        self._events = None;
        self.session_id = None;
        self.permissions.clear();
        self.start(cx);
        cx.notify();
    }

    /// The sign-in callout, above the prompt: one button per method the
    /// agent offers, or the command to run when a terminal login started.
    fn sign_in_bar(&self, look: &Look, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let s = self.sign_in.as_ref()?;
        let ui = &look.theme.ui;
        let button = |id: SharedString, label: String, primary: bool| {
            div()
                .id(id)
                .px(look.z(10.))
                .py(look.z(3.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_size(look.text(0.))
                .bg(look.c(if primary { ui.button_primary_bg } else { ui.button_bg }))
                .text_color(look.c(if primary { ui.button_primary_text } else { ui.button_text }))
                .child(label)
        };
        let methods: Vec<_> = self
            .auth_methods
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let id = m.id.clone();
                let label = if m.name.trim().is_empty() { m.id.clone() } else { m.name.clone() };
                div()
                    .flex()
                    .flex_col()
                    .gap(look.z(2.))
                    .child(button(SharedString::from(format!("auth-{}", m.id)), label, i == 0 && s.login.is_none()).on_click(cx.listener(move |t, _, _, cx| t.sign_in_with(&id, cx))))
                    .children(m.description.clone().map(|d| div().text_size(look.text(0.)).text_color(look.c(ui.text_muted)).child(d)))
            })
            .collect();
        let queued = self.pending.len() + usize::from(self.queued.is_some());
        let login_cmd = s.login.as_ref().map(|(_, c)| c.clone());
        Some(
            div()
                .flex_none()
                .mx(look.z(12.))
                .mb(look.z(6.))
                .px(look.z(12.))
                .py(look.z(8.))
                .rounded(look.z(6.))
                .border_1()
                .border_color(look.c(self.accent))
                .bg(look.c(ui.permission_bar_bg))
                .flex()
                .flex_col()
                .gap(look.z(6.))
                .text_color(look.c(ui.text))
                .child(div().font_weight(gpui::FontWeight::MEDIUM).child(format!("Sign in to {}", self.def.label)))
                .child(div().text_color(look.c(ui.text_secondary)).child(match &s.login {
                    Some((name, _)) => format!("Finish \"{name}\" in the terminal that opened, then click Continue. Or run this yourself:"),
                    // The agent offered no method: say how (Claude's own CLI login).
                    None if self.auth_methods.is_empty() && self.def.command.contains("claude") => {
                        format!("{}. Run `claude /login` in a terminal, then click Continue.", s.message.trim_end_matches('.'))
                    }
                    None if self.auth_methods.is_empty() && self.def.command.contains("codex") => {
                        format!("{}. Run `codex login` in a terminal (it signs in with ChatGPT), then click Continue.", s.message.trim_end_matches('.'))
                    }
                    None if self.auth_methods.is_empty() => format!("{}. Sign in with {}'s own login, then click Continue.", s.message.trim_end_matches('.'), self.def.label),
                    None => s.message.clone(),
                }))
                .children(login_cmd.map(|c| {
                    let copy = c.clone();
                    div()
                        .flex()
                        .items_start()
                        .gap(look.z(6.))
                        .child(code_block(&c, look).flex_1().min_w(px(0.)))
                        .child(button("auth-copy".into(), "Copy".into(), false).on_click(move |_, _, cx: &mut App| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))))
                }))
                .children(s.error.clone().map(|e| div().text_color(look.c(ui.error)).child(e)))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_start()
                        .gap(look.z(6.))
                        .children(methods)
                        .when(s.login.is_some() || self.auth_methods.is_empty(), |d| {
                            d.child(button("auth-continue".into(), "Continue".into(), true).on_click(cx.listener(|t, _, _, cx| t.continue_sign_in(cx))))
                        })
                        .children(s.busy.clone().map(|b| div().py(look.z(3.)).text_color(look.c(ui.text_muted)).child(format!("{b}…")))),
                )
                .when(queued > 0, |d| d.child(div().text_size(look.text(0.)).text_color(look.c(ui.text_muted)).child(format!("{queued} message(s) will be sent once you're signed in.")))),
        )
    }

    fn fail(&mut self, msg: String, cx: &mut Context<Self>) {
        self.sign_in = None;
        self.push(ChatItem::Failure(Failure { title: "The agent couldn't start.".into(), summary: None, detail: Some(msg.clone()), resend: None }));
        self.set_status(Status::Error(msg), cx);
        if let Some(c) = self.client.take() {
            std::thread::spawn(move || c.shutdown(Duration::from_secs(2)));
        }
    }

    fn set_status(&mut self, s: Status, cx: &mut Context<Self>) {
        if s == Status::Working && self.status != Status::NeedsYou {
            self.turn_wrote = false;
        }
        // The turn changed files: refresh the page (dev servers without hot
        // reload) and the diff.
        if s == Status::Done && std::mem::take(&mut self.turn_wrote) {
            self.browser.update(cx, |b, _| b.reload());
            self.changes.update(cx, |c, cx| c.refresh(cx));
            cx.emit(AgentEvent::TurnChangedFiles);
        }
        let active = matches!(s, Status::Working | Status::NeedsYou);
        // A thought that was streaming folds when the turn stops.
        self.remeasure_last_thought();
        // A reviewer shares its task's heatmap but never owns its turn.
        if self.review.is_none() {
            self.monitor.update(cx, |m, _| m.set_turn_active(active));
        }
        if self.status != s {
            self.status = s;
            cx.emit(AgentEvent::Changed);
        }
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let (text, mentions) = self.prompt.update(cx, |p, cx| p.take(cx));
        let atts = self.prompt.update(cx, |p, _| p.take_attachments());
        if text.trim().is_empty() && atts.is_empty() {
            return;
        }
        // Sending (or queueing) brings you back to the newest message.
        self.follow_bottom();
        let mut out = Outgoing::plain(if text.trim().is_empty() { "Adjust this.".into() } else { text }, mentions);
        for a in atts {
            out.labels.push(a.label.to_string());
            if !a.context.is_empty() {
                out.context.push(a.context);
            }
            if let Some(png) = a.png {
                out.images.push(png);
            }
        }
        if matches!(self.status, Status::Working | Status::NeedsYou | Status::Starting) || self.setting_up || self.sign_in.is_some() || self.rewinding {
            // One turn at a time: queue it and send when the turn ends.
            self.pending.push(out);
            self.save_queue(cx);
            cx.notify();
            return;
        }
        self.send_out(out, cx);
    }

    /// Send the next queued message, if any (turn ended).
    fn drain_pending(&mut self, cx: &mut Context<Self>) {
        if !self.pending.is_empty() && matches!(self.status, Status::Done | Status::Idle) && !self.rewinding {
            let out = self.pending.remove(0);
            self.save_queue(cx);
            self.send_out(out, cx);
        }
    }

    /// Drop a queued message before it's sent.
    fn unqueue(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.pending.len() {
            self.pending.remove(ix);
            self.save_queue(cx);
            cx.notify();
        }
    }

    /// Queue row "Send now": this message goes next, and the current turn
    /// stops (the stop path's `drain_pending` then sends it).
    fn send_queued_now(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.pending.len() {
            return;
        }
        let out = self.pending.remove(ix);
        self.pending.insert(0, out);
        self.save_queue(cx);
        if matches!(self.status, Status::Working | Status::NeedsYou) {
            self.stop(cx);
        } else {
            self.drain_pending(cx);
        }
        self.follow_bottom();
        cx.notify();
    }

    /// Queue row "Edit" (and Up in an empty prompt for the last one): take
    /// the message out of the queue and back into the prompt. What you'd
    /// typed since stays, after it.
    fn edit_queued(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.pending.len() {
            return;
        }
        let o = self.pending.remove(ix);
        self.save_queue(cx);
        // Context and screenshots come back as chips (what's sent is the
        // same; a pick with both shows as two).
        let named = o.labels.len() == o.context.len();
        let mut atts: Vec<Attachment> = o
            .context
            .iter()
            .enumerate()
            .map(|(i, c)| Attachment { label: if named { o.labels[i].clone().into() } else { "Context".into() }, context: c.clone(), png: None })
            .collect();
        atts.extend(o.images.into_iter().map(|png| Attachment { label: "Image".into(), context: String::new(), png: Some(png) }));
        self.prompt.update(cx, |p, cx| {
            let typed = p.text.trim().to_string();
            let text = if typed.is_empty() { o.text.clone() } else { format!("{}\n{typed}", o.text) };
            p.set_text(&text, cx);
            p.mentions.extend(o.mentions);
            for a in atts {
                p.attach(a, cx);
            }
        });
        cx.notify();
    }

    /// Mirror the queue into the task record, which the workspace saves on
    /// `Changed`: queued messages survive a quit or a crash.
    fn save_queue(&mut self, cx: &mut Context<Self>) {
        self.record.queued = self
            .pending
            .iter()
            .map(|o| QueuedMessage { text: o.text.clone(), mentions: o.mentions.clone(), context: o.context.clone(), labels: o.labels.clone() })
            .collect();
        cx.emit(AgentEvent::Changed);
    }

    pub fn output(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.push(ChatItem::Output(text.into()));
        cx.notify();
    }

    /// Put text in the prompt for the user to review, unless they're already
    /// typing something.
    pub fn prefill(&mut self, text: &str, cx: &mut Context<Self>) {
        self.prompt.update(cx, |p, cx| {
            if p.text.trim().is_empty() {
                p.set_text(text, cx);
            }
        });
        cx.notify();
    }

    pub fn notice(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.push(ChatItem::Notice(text.into()));
        cx.notify();
    }

    pub fn set_error(&mut self, msg: String, cx: &mut Context<Self>) {
        self.set_status(Status::Error(msg), cx);
    }

    /// One line for the sidebar: the agent's latest words.
    pub fn last_line(&self) -> Option<String> {
        self.chat.iter().rev().find_map(|c| match c {
            ChatItem::Agent(t) => t.lines().map(str::trim).rfind(|l| !l.is_empty() && !l.starts_with("```")).map(str::to_string),
            ChatItem::Notice(t) => Some(t.clone()),
            ChatItem::Failure(f) => Some(f.title.clone()),
            _ => None,
        })
    }

    /// Send a prompt (starting the agent first when needed).
    pub fn send(&mut self, text: String, mentions: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.send_out(Outgoing::plain(text, mentions), cx)
    }

    fn send_out(&mut self, out: Outgoing, cx: &mut Context<Self>) {
        // A Rewind is putting files back: this goes out once it's done.
        if self.rewinding {
            self.pending.insert(0, out);
            self.save_queue(cx);
            return cx.notify();
        }
        let shown = out.shown();
        let (Some(client), Some(session)) = (self.client.clone(), self.session_id.clone()) else {
            self.queued = Some(out);
            self.push(ChatItem::User(shown));
            self.start(cx);
            return;
        };
        if self.queued.is_none() && !matches!(self.chat.last(), Some(ChatItem::User(t)) if *t == shown) {
            self.push(ChatItem::User(shown));
        }
        // Kept for Retry / Edit & resend, and re-queued if a sign-in is needed.
        self.last_sent = Some(out.clone());
        let sent = out.clone();
        let Outgoing { text, mentions, context, images, .. } = out;
        let mut blocks = vec![PromptBlock::Text(text)];
        for c in context {
            blocks.push(PromptBlock::Text(c));
        }
        if self.images_ok {
            for png in images {
                blocks.push(PromptBlock::Image { mime: "image/png".into(), data_base64: base64(&png) });
            }
        }
        for m in mentions {
            let name = m.strip_prefix(&self.record.worktree).unwrap_or(&m).to_string_lossy().into_owned();
            blocks.push(PromptBlock::ResourceLink { uri: format!("file://{}", m.display()), name });
        }
        // After a Rewind, the agent hears about it with this message.
        if let Some(note) = self.rewind_note.take() {
            blocks.push(PromptBlock::Text(note));
        }
        self.turn_started = Some(Instant::now());
        self.turn_tools.clear();
        self.turn_files.clear();
        self.set_status(Status::Working, cx);
        // Checkpoint the worktree before the agent can touch it, then send.
        let wt = self.record.worktree.clone();
        let bg = cx.background_executor().clone();
        // A reviewer changes no files: no checkpoints, no turn refs.
        let checkpoint = self.review.is_none();
        cx.spawn(async move |this, cx| {
            let start = if checkpoint { bg.spawn(async move { qur_git::snapshot_tree(&wt) }).await } else { Err(String::new()) };
            let Ok(rx) = this.update(cx, |t, cx| {
                t.begin_turn(start, cx);
                client.prompt(&session, blocks)
            }) else {
                return;
            };
            let r = rx.await;
            this.update(cx, |t, cx| {
                if !t.permissions.is_empty() {
                    t.permissions.clear();
                    cx.emit(AgentEvent::PermissionsChanged);
                }
                match r {
                    Ok(Ok(StopReason::EndTurn)) => t.set_status(Status::Done, cx),
                    Ok(Ok(StopReason::Cancelled)) => {
                        t.push(ChatItem::Notice("Stopped.".into()));
                        t.set_status(Status::Done, cx)
                    }
                    // Cut short (length limit, refusal, too many steps…): say so, offer Retry.
                    Ok(Ok(other)) => {
                        let title = other.describe(&t.def.label).unwrap_or_default();
                        t.push(ChatItem::Failure(Failure { title, summary: None, detail: None, resend: Some(sent) }));
                        t.set_status(Status::Done, cx)
                    }
                    // Not signed in: the message waits at the front of the queue.
                    Ok(Err(e)) if e.is_auth_required() => {
                        t.pending.insert(0, sent);
                        t.save_queue(cx);
                        t.need_sign_in(&e, cx);
                    }
                    // The agent exited mid-turn: `Event::Exited`, right behind,
                    // shows why (stderr) and offers Retry.
                    Ok(Err(e)) if e.is_agent_exited() => {}
                    Ok(Err(e)) => {
                        let title = format!("{} couldn't finish this turn.", t.def.label);
                        let summary = e.summary();
                        // The raw text only when it says more than the summary.
                        let detail = Some(e.to_string()).filter(|d| !d.starts_with(&summary));
                        t.push(ChatItem::Failure(Failure { title, summary: Some(summary.clone()), detail, resend: Some(sent) }));
                        t.set_status(Status::Error(summary), cx)
                    }
                    Err(_) => t.set_status(Status::Error("agent exited".into()), cx),
                }
                // Queued messages go out once the end checkpoint is taken.
                t.end_turn(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Stop (Esc in the prompt, Cmd+.): cancel the current turn.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let (Some(c), Some(s)) = (&self.client, &self.session_id)
            && matches!(self.status, Status::Working | Status::NeedsYou)
        {
            c.cancel(s);
            for p in std::mem::take(&mut self.permissions) {
                c.respond_permission(p.id, PermissionOutcome::Cancelled);
            }
            cx.emit(AgentEvent::PermissionsChanged);
        }
        cx.notify();
    }

    /// Quit, close or discard: stop the process (cancel, close stdin,
    /// SIGTERM, SIGKILL after 2 s) on a thread. Join the handle to wait.
    pub fn shutdown(&mut self, cx: &mut Context<Self>) -> Option<std::thread::JoinHandle<()>> {
        let handle = self.client.take().map(|c| {
            if let Some(s) = &self.session_id {
                c.cancel(s);
            }
            std::thread::spawn(move || c.shutdown(Duration::from_secs(2)))
        });
        self.session_id = None;
        self.permissions.clear();
        self._events = None;
        if !matches!(self.status, Status::Error(_)) {
            self.set_status(Status::Stopped, cx);
        }
        // The task's reviewer stops with it.
        let reviewer = self.reviewer.clone().and_then(|r| r.update(cx, |r, cx| r.shutdown(cx)));
        match (handle, reviewer) {
            (Some(a), Some(b)) => Some(std::thread::spawn(move || {
                let _ = a.join();
                let _ = b.join();
            })),
            (a, b) => a.or(b),
        }
    }

    /// Answer by kind (the Agent menu's Allow Once / Allow for This Task / Deny).
    /// Does nothing when the agent doesn't offer that kind (Cmd+Opt+Y on a
    /// request without "always" mustn't cancel it); Deny without a reject
    /// option cancels the request.
    pub fn answer_permission(&mut self, kind: PermissionKind, cx: &mut Context<Self>) {
        let Some(p) = self.permissions.front() else { return };
        let id = p.options.iter().find(|o| o.kind == kind).map(|o| o.id.clone());
        if id.is_none() && kind != PermissionKind::RejectOnce {
            return;
        }
        self.answer_with(id, cx);
    }

    /// Answer with one of the agent's own options (the permission bar's buttons).
    fn answer_option(&mut self, option_id: &str, cx: &mut Context<Self>) {
        self.answer_with(Some(option_id.to_string()), cx);
    }

    /// Answer the request shown (the oldest); the next one shows after it.
    fn answer_with(&mut self, option_id: Option<String>, cx: &mut Context<Self>) {
        let Some(p) = self.permissions.pop_front() else { return };
        self.respond(p, option_id, cx);
        if self.permissions.is_empty() {
            self.set_status(Status::Working, cx);
        }
        cx.emit(AgentEvent::PermissionsChanged);
        cx.notify();
    }

    fn respond(&mut self, p: Permission, option_id: Option<String>, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = p.tool.locations.iter().flatten().map(|l| l.path.clone()).collect();
        self.monitor.update(cx, |m, cx| m.on_permission(&paths, false, cx));
        println!("PERMISSION {} {} {}", self.record.id, p.tool.id, option_id.as_deref().unwrap_or("cancelled"));
        let outcome = match option_id {
            Some(id) => PermissionOutcome::Selected(id),
            None => PermissionOutcome::Cancelled,
        };
        if let Some(c) = &self.client {
            c.respond_permission(p.id, outcome);
        }
        self.refresh_tool(&p.tool.id);
    }

    /// Requests waiting for an answer.
    pub fn pending_permissions(&self) -> usize {
        self.permissions.len()
    }

    /// Allow All Pending: answer every waiting request with its "allow
    /// once" option (never "always"). A request without one stays. Returns
    /// how many were allowed.
    pub fn allow_all_pending(&mut self, cx: &mut Context<Self>) -> usize {
        let mut allowed = 0;
        for p in std::mem::take(&mut self.permissions) {
            match p.options.iter().find(|o| o.kind == PermissionKind::AllowOnce).map(|o| o.id.clone()) {
                Some(id) => {
                    self.respond(p, Some(id), cx);
                    allowed += 1;
                }
                None => self.permissions.push_back(p),
            }
        }
        println!("PERMISSION {} allow all pending: {allowed} allowed, {} left", self.record.id, self.permissions.len());
        if allowed > 0 && self.permissions.is_empty() {
            self.set_status(Status::Working, cx);
        }
        cx.emit(AgentEvent::PermissionsChanged);
        cx.notify();
        allowed
    }

    /// The agent's full new set of settings. A mode kept the old way
    /// (`modes`) stays unless the new set has a mode of its own.
    fn replace_settings(&mut self, list: Vec<ConfigOption>) {
        let legacy = self.settings.iter().find(|o| o.legacy_mode).cloned().filter(|_| !list.iter().any(ConfigOption::is_mode));
        self.settings = legacy.into_iter().chain(list).collect();
    }

    /// Change a session setting (mode, model…). Shown at once; the agent's
    /// answer then replaces the whole set.
    pub fn set_setting(&mut self, id: &str, value: &str, cx: &mut Context<Self>) {
        let (Some(client), Some(session)) = (self.client.clone(), self.session_id.clone()) else { return };
        let Some(opt) = self.settings.iter_mut().find(|o| o.id == id) else { return };
        let before = std::mem::replace(&mut opt.current, value.to_string());
        let pending = client.set_option(&session, opt, value);
        let id = id.to_string();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = pending.await;
            this.update(cx, |t, cx| {
                match res {
                    Ok(Ok(Some(list))) => t.replace_settings(list),
                    Ok(Ok(None)) => {}
                    // Refused: put it back and say why.
                    Ok(Err(e)) => {
                        if let Some(o) = t.settings.iter_mut().find(|o| o.id == id) {
                            o.current = before;
                        }
                        t.notice(format!("couldn't change {id}: {e}"), cx);
                    }
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shift+Tab: the next mode, as in Claude Code's own terminal UI.
    fn cycle_mode(&mut self, _: &aa::CycleMode, _: &mut Window, cx: &mut Context<Self>) {
        // A reviewer stays in its read-only mode.
        if self.review.is_some() {
            return;
        }
        let Some(o) = self.settings.iter().find(|o| o.is_mode()) else { return };
        if let Some(next) = o.next_value().map(str::to_string) {
            let id = o.id.clone();
            self.set_setting(&id, &next, cx);
        }
    }

    pub fn has_permission(&self) -> bool {
        !self.permissions.is_empty()
    }

    fn on_event(&mut self, ev: Event, cx: &mut Context<Self>) {
        match ev {
            // Our session id is known before session/load replays history, so
            // replayed subagent work is kept out of the chat too.
            Event::Update { session_id, update }
                if !session_id.is_empty() && self.session_id.as_deref().or(self.record.session_id.as_deref()).is_some_and(|s| s != session_id.as_str()) =>
            {
                // A subagent's own work: keep it out of the chat, show its latest step.
                let Some(sub) = self.subagents.iter_mut().find(|s| s.id == session_id) else { return };
                match update {
                    SessionUpdate::ToolCall(t) if !t.title.trim().is_empty() => {
                        sub.last = Some(t.title.clone());
                        sub.log.push((true, t.title));
                    }
                    SessionUpdate::AgentMessage(c) | SessionUpdate::UserMessage(c) => match sub.log.last_mut() {
                        Some((false, s)) => s.push_str(&c.text),
                        _ => sub.log.push((false, c.text)),
                    },
                    _ => return,
                }
                cx.emit(AgentEvent::Subagents);
                cx.notify();
            }
            Event::Update { update, .. } => match update {
                SessionUpdate::AgentMessage(c) | SessionUpdate::UserMessage(c) if c.media.is_some() => self.push_media(c.media),
                SessionUpdate::AgentMessage(c) => self.append(false, &c.text),
                SessionUpdate::AgentThought(c) => self.append(true, &c.text),
                // The agent named the session: the task's title follows unless
                // the user renamed it (saved, and the sidebar updates, on Changed).
                SessionUpdate::SessionTitle(title) => {
                    if !self.record.renamed && self.record.title != title {
                        self.record.title = title;
                        cx.emit(AgentEvent::Changed);
                    }
                }
                SessionUpdate::UserMessage(c) => {
                    // Replayed history (session/load).
                    match self.chat.last_mut() {
                        // A reviewer's later blocks are the diff and chat it was sent: not shown again.
                        Some(ChatItem::User(s)) if self.status == Status::Starting => {
                            if self.review.is_none() {
                                s.push_str(&c.text)
                            }
                        }
                        _ => self.push(ChatItem::User(c.text)),
                    }
                }
                SessionUpdate::ToolCall(t) => {
                    let id = t.id.clone();
                    let paths: Vec<PathBuf> = t.locations.iter().map(|l| l.path.clone()).collect();
                    let (k, st) = (heat_kind(t.kind), heat_status(t.status));
                    let lines: Vec<(PathBuf, u32)> = t.locations.iter().filter_map(|l| Some((l.path.clone(), l.line?))).collect();
                    // A reviewer's reads aren't the task agent's work: no heat.
                    if self.review.is_none() {
                        self.monitor.update(cx, |m, cx| {
                            m.on_tool(&id, Some(k), Some(st), &paths, cx);
                            for (p, l) in &lines {
                                m.tool_line(&id, p, *l);
                            }
                        });
                    }
                    self.tools.insert(id.clone(), t);
                    self.push(ChatItem::Tool(id.clone()));
                    self.track_turn_tool(id);
                }
                SessionUpdate::ToolCallUpdate(u) => {
                    let id = u.id.clone();
                    let paths: Vec<PathBuf> = u.locations.iter().flatten().map(|l| l.path.clone()).collect();
                    let (k, st) = (u.kind.map(heat_kind), u.status.map(heat_status));
                    let lines: Vec<(PathBuf, u32)> = u.locations.iter().flatten().filter_map(|l| Some((l.path.clone(), l.line?))).collect();
                    if self.review.is_none() {
                        self.monitor.update(cx, |m, cx| {
                            m.on_tool(&id, k, st, &paths, cx);
                            for (p, l) in &lines {
                                m.tool_line(&id, p, *l);
                            }
                        });
                    }
                    match self.tools.get_mut(&id) {
                        Some(t) => t.apply(&u),
                        None => {
                            let t = u.to_tool_call();
                            self.tools.insert(id.clone(), t);
                            self.push(ChatItem::Tool(id.clone()));
                        }
                    }
                    self.refresh_tool(&id);
                    self.track_turn_tool(id);
                }
                SessionUpdate::Plan(p) => self.plan = p,
                SessionUpdate::AvailableCommands(c) => self.commands = c,
                // The agent switched mode itself (e.g. out of plan mode).
                SessionUpdate::CurrentMode(m) => {
                    if let Some(o) = self.settings.iter_mut().find(|o| o.legacy_mode) {
                        o.current = m;
                    }
                }
                SessionUpdate::ConfigOptions(list) => self.replace_settings(list),
                SessionUpdate::Usage { used, size, cost } => {
                    // A later update without a cost keeps the last one known.
                    let cost = cost.or(self.usage.and_then(|u| u.2));
                    self.usage = Some((used, size, cost));
                }
                SessionUpdate::Subagent { id, title, state } => {
                    let ix = match self.subagents.iter().position(|s| s.id == id) {
                        Some(ix) => ix,
                        None => {
                            self.subagents.push(Subagent { id, title: "Subagent".into(), state: "running".into(), last: None, log: Vec::new() });
                            self.subagents.len() - 1
                        }
                    };
                    let s = &mut self.subagents[ix];
                    if let Some(t) = title {
                        s.title = t;
                    }
                    if let Some(st) = state {
                        s.state = st;
                    }
                    cx.emit(AgentEvent::Subagents);
                }
                _ => {}
            },
            // A reviewer is read-only: whatever it asks to do is denied at
            // once, and no permission bar shows. Its chat says what was denied.
            Event::Permission { id, tool_call, options, .. } if self.review.is_some() => {
                let what = tool_call
                    .title
                    .clone()
                    .or_else(|| self.tools.get(&tool_call.id).map(|t| t.title.clone()))
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| "a tool".into());
                let reply = review::deny_option(&options);
                println!("REVIEW {} denied {:?} \"{what}\" with {}", self.record.id, tool_call.kind, reply.as_deref().unwrap_or("cancel"));
                if let Some(c) = &self.client {
                    c.respond_permission(id, reply.map_or(PermissionOutcome::Cancelled, PermissionOutcome::Selected));
                }
                self.push(ChatItem::Notice(format!("Qur denied “{what}”: reviews are read-only.")));
            }
            Event::Permission { id, tool_call, options, .. } => {
                let paths: Vec<PathBuf> = tool_call.locations.iter().flatten().map(|l| l.path.clone()).collect();
                self.monitor.update(cx, |m, cx| m.on_permission(&paths, true, cx));
                let tool = tool_call.id.clone();
                println!("PERMISSION {} {tool} asked ({} waiting)", self.record.id, self.permissions.len() + 1);
                self.permissions.push_back(Permission { id, tool: tool_call, options });
                // Its tool row shows (even in a folded group) while it waits.
                self.refresh_tool(&tool);
                self.set_status(Status::NeedsYou, cx);
                cx.emit(AgentEvent::PermissionsChanged);
            }
            Event::Exited { code, stderr_tail } => {
                self.client = None;
                self.session_id = None;
                if !self.permissions.is_empty() {
                    self.permissions.clear();
                    cx.emit(AgentEvent::PermissionsChanged);
                }
                if !matches!(self.status, Status::Stopped) {
                    // The status line gets the last stderr line; the callout the
                    // whole tail (the real error is often a few lines up).
                    let last = stderr_tail.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                    let how = code.map(|c| format!("code {c}")).unwrap_or("a signal".into());
                    let msg = format!("agent exited ({how}){}", if last.is_empty() { String::new() } else { format!(": {last}") });
                    // Mid-turn, Retry sends that turn's message again (restarting the agent).
                    let resend = self.last_sent.clone().filter(|_| matches!(self.status, Status::Working | Status::NeedsYou));
                    let detail = Some(tail_lines(stderr_tail.trim_end(), MAX_STDERR_LINES)).filter(|d| !d.is_empty());
                    self.push(ChatItem::Failure(Failure { title: format!("{} exited ({how}).", self.def.label), summary: None, detail, resend }));
                    self.set_status(Status::Error(msg), cx);
                }
            }
            Event::Warning(_) => {}
        }
        cx.notify();
    }

    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.record.worktree).unwrap_or(p).to_string_lossy().into_owned()
    }

    // -- rendering ---------------------------------------------------------------

    fn render_item(&self, ix: usize, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let pad = |d: gpui::Div| d.px(look.z(16.)).py(look.z(6.)).w_full();
        // Find in Chat: this row's matches marked, the current one stronger.
        let hl = self.row_hl(ix, look);
        let hl = hl.as_ref();
        let current_here = self.find_current().is_some_and(|(row, _)| row == ix);
        match &self.chat[ix] {
            ChatItem::User(t) => pad(div())
                .child(
                    div()
                        .text_size(look.text(0.))
                        .px(look.z(10.))
                        .py(look.z(6.))
                        .rounded(look.z(6.))
                        .bg(look.c(ui.active_bg))
                        .text_color(look.c(ui.text))
                        .child(marked(hl, t.clone())),
                )
                .into_any_element(),
            // Hover shows "Copy" (the raw Markdown).
            ChatItem::Agent(t) => pad(div())
                .relative()
                .group("agent-msg")
                .child(self.markdown_find(t, format!("md-{}-{ix}", cx.entity_id()), look, hl, cx))
                .child(
                    div()
                        .absolute()
                        .top(look.z(2.))
                        .right(look.z(12.))
                        .rounded(look.z(4.))
                        .bg(look.c(ui.panel_bg))
                        .invisible()
                        .group_hover("agent-msg", |s| s.visible())
                        .child(crate::markdown::copy_button(("copy-msg", ix), t.clone(), look)),
                )
                .into_any_element(),
            // Open while it streams (the last item of a working turn), then
            // folded to its first line; click to toggle (as in Zed). Find's
            // current match opens it too.
            ChatItem::Thought(t) => {
                let streaming = ix + 1 == self.chat.len() && matches!(self.status, Status::Working);
                let open = (streaming != self.toggled.contains(&format!("thought-{ix}"))) || current_here;
                let t = t.trim();
                let first = t.lines().next().unwrap_or("").to_string();
                pad(div())
                    .text_size(look.text(0.))
                    .text_color(look.c(ui.text_muted))
                    .child(
                        div()
                            .id(("thought", ix))
                            .flex()
                            .gap(look.z(6.))
                            .cursor_pointer()
                            .hover(|d| d.text_color(look.c(ui.text_secondary)))
                            .child(div().flex_none().child(if open { "▾ Thinking" } else { "▸ Thinking" }))
                            .when(!open, |d| d.child(div().min_w(px(0.)).truncate().italic().child(first)))
                            .on_click(cx.listener(move |t, _, _, cx| t.toggle(format!("thought-{ix}"), ix, cx))),
                    )
                    .when(open, |d| d.child(div().pl(look.z(14.)).pt(look.z(2.)).italic().child(marked(hl, t.to_string()))))
                    .into_any_element()
            }
            ChatItem::Notice(t) => pad(div()).text_size(look.text(0.)).text_color(look.c(ui.text_muted)).child(marked(hl, t.clone())).into_any_element(),
            ChatItem::Output(t) => pad(div())
                .child(
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
                        .child(marked(hl, t.clone())),
                )
                .into_any_element(),
            // At its own size, at most 480 high and never wider than the
            // chat; the aspect ratio keeps the height right when it shrinks.
            ChatItem::Image(image, (w, h)) => {
                let (w, h) = (*w as f32, *h as f32);
                let width = w * (480. / h).min(1.);
                pad(div())
                    .child(img(image.clone()).w(look.z(width)).max_w_full().aspect_ratio(w / h).rounded(look.z(4.)).border_1().border_color(look.c(ui.border)))
                    .into_any_element()
            }
            ChatItem::Link { uri, name } => {
                let target = uri.clone();
                pad(div())
                    .child(
                        div()
                            .id(("link", ix))
                            .flex()
                            .items_center()
                            .gap(look.z(6.))
                            .max_w_full()
                            .px(look.z(8.))
                            .py(look.z(2.))
                            .rounded(look.z(4.))
                            .border_1()
                            .border_color(look.c(ui.border))
                            .bg(look.c(ui.panel_bg))
                            .cursor_pointer()
                            .text_size(look.text(0.))
                            .text_color(look.c(ui.text_secondary))
                            .hover(|d| d.text_color(look.c(ui.text)).border_color(look.c(ui.border_strong)))
                            .child(div().flex_none().text_color(look.c(ui.text_muted)).child(if uri.starts_with("file://") { "▤" } else { "↗" }))
                            .child(div().min_w(px(0.)).truncate().child(marked(hl, name.clone())))
                            .on_click(cx.listener(move |t, _, _, cx| t.open_link(&target, cx))),
                    )
                    .into_any_element()
            }
            ChatItem::Failure(f) => pad(div()).child(self.failure_callout(ix, f, look, hl, cx)).into_any_element(),
            ChatItem::Turn(n) => pad(div()).child(self.turn_line(ix, *n, look, cx)).into_any_element(),
            // Two or more calls in a row are one group: a summary line
            // ("Read 4 files, ran 2 commands ▸") that unfolds to the calls.
            // Folded, a call still shows while it runs, failed, waits for
            // permission or holds Find's current match.
            ChatItem::Tool(id) => {
                let Some(run) = tool_run(&self.chat, ix).filter(|r| r.len() >= 2) else {
                    return self.tool_row(ix, id, look, hl, current_here, cx).into_any_element();
                };
                let open = self.group_open(&run);
                let row = (open || self.tool_needs_eye(ix, id)).then(|| self.tool_row(ix, id, look, hl, current_here, cx).pl(look.z(30.)));
                if ix != run.start {
                    return row.map(|r| r.into_any_element()).unwrap_or_else(|| div().into_any_element());
                }
                div().w_full().flex().flex_col().child(self.group_header(run, open, look, cx)).children(row).into_any_element()
            }
        }
    }

    /// One tool call's row: kind, title, counts, status, and its body when
    /// unfolded.
    fn tool_row(&self, ix: usize, id: &ToolCallId, look: &Look, hl: Option<&FindHl>, current_here: bool, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let Some(t) = self.tools.get(id) else { return div() };
        let (glyph, color) = match t.status {
            ToolStatus::Pending | ToolStatus::InProgress => ("…", ui.text_muted),
            ToolStatus::Completed => ("✓", ui.success),
            ToolStatus::Failed => ("✕", ui.error),
        };
        let asking = self.permissions.iter().any(|p| p.tool.id == *id);
        let kind = kind_word(t.kind);
        // Real line counts (every line of old and new counted before).
        let diffs = tool_diffs(t);
        let add = diffs.iter().flat_map(|(_, d)| d).filter(|(k, _)| *k == '+').count();
        let del = diffs.iter().flat_map(|(_, d)| d).filter(|(k, _)| *k == '-').count();
        let loc = t.locations.first().cloned();
        let title = tool_title(t);
        let body = self.tool_body(t, &diffs, look, hl, cx);
        // Edits and failures start open; the rest folded (click toggles).
        // A command stays open while it runs, so its output shows as it
        // streams (`terminal_output_delta`), then folds.
        let running_cmd = t.content.iter().any(|c| matches!(c, ToolContent::Terminal(_))) && t.exit.is_none() && matches!(t.status, ToolStatus::Pending | ToolStatus::InProgress);
        let default_open = matches!(t.kind, ToolKind::Edit | ToolKind::Delete | ToolKind::Move) || t.status == ToolStatus::Failed || running_cmd;
        let open = body.is_some() && (default_open != self.toggled.contains(id.as_str()) || current_here);
        let expandable = body.is_some();
        let tid = id.clone();
        div()
            .px(look.z(16.))
            .py(look.z(2.))
            .w_full()
            .child(
                div()
                    .id(SharedString::from(format!("tool-{id}")))
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .text_size(look.text(0.))
                    .text_color(look.c(ui.text_secondary))
                    .when(expandable || loc.is_some(), |d| d.cursor_pointer().hover(|d| d.text_color(look.c(ui.text))))
                    .child(div().w(look.z(40.)).flex_none().text_color(look.c(ui.text_muted)).child(kind))
                    .child(div().min_w(px(0.)).truncate().child(marked(hl, title)))
                    .when(add + del > 0, |d| {
                        d.child(div().flex_none().text_color(look.c(look.theme.diff.added_text)).child(format!("+{add}")))
                            .child(div().flex_none().text_color(look.c(look.theme.diff.removed_text)).child(format!("−{del}")))
                    })
                    .when_some(t.exit.flatten().filter(|c| *c != 0), |d, c| d.child(div().flex_none().text_color(look.c(ui.error)).child(format!("exit {c}"))))
                    .when(asking, |d| d.child(div().flex_none().text_color(look.c(self.accent)).child("needs you")))
                    .child(div().flex_none().text_color(look.c(color)).child(glyph))
                    .when(expandable, |d| d.child(div().flex_none().text_color(look.c(ui.text_muted)).child(if open { "▾" } else { "▸" })))
                    .on_click(cx.listener(move |t, _, _, cx| {
                        if expandable {
                            t.toggle(tid.to_string(), ix, cx);
                        } else if let Some(l) = &loc {
                            cx.emit(AgentEvent::OpenFile(l.path.clone(), l.line));
                        }
                    })),
            )
            .when_some(body.filter(|_| open), |d, b| d.child(div().pl(look.z(48.)).pt(look.z(4.)).child(b)))
    }

    /// The tool calls of a group (`run` of chat rows).
    fn run_tools(&self, run: &Range<usize>) -> Vec<&ToolCall> {
        self.chat[run.clone()]
            .iter()
            .filter_map(|c| match c {
                ChatItem::Tool(id) => self.tools.get(id),
                _ => None,
            })
            .collect()
    }

    /// A group's fold key: its first call's id.
    fn group_key(&self, run: &Range<usize>) -> String {
        match &self.chat[run.start] {
            ChatItem::Tool(id) => format!("group-{id}"),
            _ => String::new(),
        }
    }

    /// Groups start folded; a click on the summary unfolds them.
    fn group_open(&self, run: &Range<usize>) -> bool {
        self.toggled.contains(&self.group_key(run))
    }

    /// A call that shows even in a folded group.
    fn tool_needs_eye(&self, ix: usize, id: &ToolCallId) -> bool {
        let status = self.tools.get(id).map(|t| t.status);
        matches!(status, Some(ToolStatus::Pending | ToolStatus::InProgress | ToolStatus::Failed))
            || self.permissions.iter().any(|p| p.tool.id == *id)
            || self.find_current().is_some_and(|(row, _)| row == ix)
    }

    fn toggle_group(&mut self, run: Range<usize>, cx: &mut Context<Self>) {
        let key = self.group_key(&run);
        if !self.toggled.remove(&key) {
            self.toggled.insert(key);
        }
        self.list.remeasure_items(run);
        cx.notify();
    }

    /// "Read 4 files, ran 2 commands  ✓ ▸".
    fn group_header(&self, run: Range<usize>, open: bool, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let tools = self.run_tools(&run);
        let failed = tools.iter().filter(|t| t.status == ToolStatus::Failed).count();
        let running = tools.iter().any(|t| matches!(t.status, ToolStatus::Pending | ToolStatus::InProgress));
        let (glyph, color) = if running {
            ("…", ui.text_muted)
        } else if failed > 0 {
            ("✕", ui.error)
        } else {
            ("✓", ui.success)
        };
        let summary = group_summary(&tools);
        let n = tools.len();
        div().px(look.z(16.)).py(look.z(2.)).w_full().child(
            div()
                .id(("tool-group", run.start))
                .flex()
                .items_center()
                .gap(look.z(8.))
                .text_size(look.text(0.))
                .text_color(look.c(ui.text_secondary))
                .cursor_pointer()
                .hover(|d| d.text_color(look.c(ui.text)))
                .child(div().w(look.z(40.)).flex_none().text_color(look.c(ui.text_muted)).child(format!("{n} ×")))
                .child(div().min_w(px(0.)).truncate().child(summary))
                .when(failed > 0, |d| d.child(div().flex_none().text_color(look.c(ui.error)).child(format!("{failed} failed"))))
                .child(div().flex_none().text_color(look.c(color)).child(glyph))
                .child(div().flex_none().text_color(look.c(ui.text_muted)).child(if open { "▾" } else { "▸" }))
                .on_click(cx.listener(move |t, _, _, cx| t.toggle_group(run.clone(), cx))),
        )
    }

    /// Fold or unfold a chat item (tool call `key` = its id, thought
    /// `thought-<ix>`) and re-measure its row.
    fn toggle(&mut self, key: String, ix: usize, cx: &mut Context<Self>) {
        if !self.toggled.remove(&key) {
            self.toggled.insert(key);
        }
        self.list.splice(ix..ix + 1, 1);
        cx.notify();
    }

    /// An error-coloured callout: what went wrong, the full error (bounded,
    /// scrolls), then Retry / Edit & resend when a message can be sent
    /// again, and Copy error.
    fn failure_callout(&self, ix: usize, f: &Failure, look: &Look, hl: Option<&FindHl>, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let button = |id: &'static str, label: &'static str| {
            div()
                .id((id, ix))
                .px(look.z(10.))
                .py(look.z(3.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_size(look.text(0.))
                .bg(look.c(ui.button_bg))
                .text_color(look.c(ui.button_text))
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .child(label)
        };
        let copy = [Some(f.title.clone()), f.summary.clone(), f.detail.clone()].into_iter().flatten().collect::<Vec<_>>().join("\n\n");
        let (retry, edit) = (f.resend.clone(), f.resend.clone());
        div()
            .px(look.z(12.))
            .py(look.z(8.))
            .rounded(look.z(6.))
            .border_1()
            .border_color(look.c(ui.error))
            .flex()
            .flex_col()
            .gap(look.z(6.))
            .text_size(look.text(0.))
            .child(div().text_color(look.c(ui.error)).child(marked(hl, f.title.clone())))
            .children(f.summary.clone().map(|t| div().text_color(look.c(ui.text)).child(marked(hl, t))))
            // The raw error, small and scrollable, for when the summary isn't enough.
            .children(f.detail.clone().map(|d| {
                div()
                    .id(("failure-detail", ix))
                    .max_h(look.z(if f.summary.is_some() { 90. } else { 180. }))
                    .overflow_y_scroll()
                    .child(code_block_find(&d, look, hl))
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(look.z(6.))
                    .children(retry.map(|out| button("failure-retry", "Retry").on_click(cx.listener(move |t, _, _, cx| t.retry(out.clone(), cx)))))
                    .children(edit.map(|out| button("failure-edit", "Edit & resend").on_click(cx.listener(move |t, _, window, cx| t.edit_resend(&out, window, cx)))))
                    .child(button("failure-copy", "Copy error").on_click(move |_, _, cx: &mut App| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())))),
            )
    }

    /// What an unfolded tool call shows: its text (code blocks in the code
    /// font), each file's diff, a command's output and exit, or else its raw
    /// input/output. None when there's nothing to show.
    fn tool_body(&self, t: &ToolCall, diffs: &[(&PathBuf, Vec<(char, &str)>)], look: &Look, hl: Option<&FindHl>, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let ui = &look.theme.ui;
        let mut parts: Vec<gpui::AnyElement> = Vec::new();
        for c in &t.content {
            match c {
                ToolContent::Text(s) if !s.trim().is_empty() => parts.push(self.markdown_find(s, format!("md-{}-{}-{}", cx.entity_id(), t.id, parts.len()), look, hl, cx).into_any_element()),
                ToolContent::Terminal(_) => {
                    let out = t.output.as_deref().unwrap_or("").trim_end();
                    let status = match t.exit {
                        None => "running…".to_string(),
                        Some(Some(0)) => "exit 0".into(),
                        Some(Some(c)) => format!("exit {c}"),
                        Some(None) => "ended".into(),
                    };
                    let failed = matches!(t.exit, Some(Some(c)) if c != 0);
                    // Still running with nothing yet: just "running…" (the
                    // Claude adapter sends the output when the command ends).
                    let block = (!out.is_empty() || t.exit.is_some()).then(|| code_block_find(&tail_lines(out, MAX_TOOL_LINES), look, hl).when(out.is_empty(), |d| d.child("(no output)")));
                    parts.push(
                        div()
                            .flex()
                            .flex_col()
                            .gap(look.z(2.))
                            .children(block)
                            .child(div().text_size(look.text(0.)).text_color(look.c(if failed { ui.error } else { ui.text_muted })).child(status))
                            .into_any_element(),
                    );
                }
                _ => {}
            }
        }
        for (path, lines) in diffs {
            let path = (*path).clone();
            let shown = lines.iter().filter(|(k, _)| *k != ' ').count().min(MAX_TOOL_LINES);
            let rows = lines.iter().filter(|(k, _)| *k != ' ').take(MAX_TOOL_LINES).map(|(k, l)| {
                let (bg, fg) = if *k == '+' { (look.theme.diff.added_line, look.theme.diff.added_text) } else { (look.theme.diff.removed_line, look.theme.diff.removed_text) };
                div().px(look.z(6.)).bg(look.c(bg)).child(div().text_color(look.c(fg)).child(format!("{k} {l}")))
            });
            let more = lines.iter().filter(|(k, _)| *k != ' ').count() - shown;
            parts.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.z(2.))
                    .child(
                        div()
                            .id(SharedString::from(format!("diff-{}", path.display())))
                            .text_size(look.text(0.))
                            .text_color(look.c(ui.text_muted))
                            .cursor_pointer()
                            .hover(|d| d.text_color(look.c(ui.text)))
                            .child(self.rel(&path))
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(AgentEvent::OpenFile(path.clone(), None)))),
                    )
                    .child(
                        div()
                            .rounded(look.z(4.))
                            .border_1()
                            .border_color(look.c(ui.border))
                            .overflow_hidden()
                            .font_family(look.code_font.clone())
                            .text_size(look.text(0.))
                            .children(rows)
                            .when(more > 0, |d| d.child(div().px(look.z(6.)).text_color(look.c(ui.text_muted)).child(format!("… {more} more changed lines")))),
                    )
                    .into_any_element(),
            );
        }
        if parts.is_empty() {
            // Nothing structured: the raw input, then the raw result.
            for v in [&t.raw_input, &t.raw_output].into_iter().flatten() {
                let text = match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Object(m) if m.is_empty() => continue,
                    v => serde_json::to_string_pretty(v).unwrap_or_default(),
                };
                if !text.trim().is_empty() {
                    parts.push(code_block(&tail_lines(text.trim_end(), MAX_TOOL_LINES), look).into_any_element());
                }
            }
        }
        (!parts.is_empty()).then(|| div().flex().flex_col().gap(look.z(6.)).children(parts).into_any_element())
    }

    /// A tool call of the running turn: remember it and recount the files
    /// the turn changed (the strip above the prompt).
    fn track_turn_tool(&mut self, id: ToolCallId) {
        if !matches!(self.status, Status::Working | Status::NeedsYou) {
            return;
        }
        if !self.turn_tools.contains(&id) {
            self.turn_tools.push(id);
        }
        let tools: Vec<&ToolCall> = self.turn_tools.iter().filter_map(|id| self.tools.get(id)).collect();
        self.turn_files = turn_changes(&tools);
    }

    /// "3 files changed · +40 −12 · Review", then a chip per file (opens it).
    fn files_strip(&self, look: &Look, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.turn_files.is_empty() {
            return None;
        }
        let ui = &look.theme.ui;
        let (add, del) = change_totals(&self.turn_files);
        let n = self.turn_files.len();
        let counts = |d: gpui::Stateful<gpui::Div>, add: usize, del: usize| {
            d.when(add > 0, |d| d.child(div().flex_none().text_color(look.c(look.theme.diff.added_text)).child(format!("+{add}"))))
                .when(del > 0, |d| d.child(div().flex_none().text_color(look.c(look.theme.diff.removed_text)).child(format!("−{del}"))))
        };
        let chips: Vec<_> = self
            .turn_files
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let path = f.path.clone();
                let name = f.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.rel(&f.path));
                let chip = div()
                    .id(("turn-file", i))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(look.z(4.))
                    .px(look.z(6.))
                    .py(look.z(1.))
                    .rounded(look.z(4.))
                    .cursor_pointer()
                    .bg(look.c(ui.active_bg))
                    .text_color(look.c(ui.text_secondary))
                    .hover(|d| d.text_color(look.c(ui.text)))
                    .child(div().max_w(look.z(220.)).truncate().child(name));
                counts(chip, f.add, f.del).on_click(cx.listener(move |_, _, _, cx| cx.emit(AgentEvent::OpenFile(path.clone(), None))))
            })
            .collect();
        let summary = div()
            .id("turn-files")
            .flex()
            .flex_none()
            .items_center()
            .gap(look.z(6.))
            .text_color(look.c(ui.text_secondary))
            .child(format!("{n} file{} changed", if n == 1 { "" } else { "s" }));
        Some(
            div()
                .flex_none()
                .mx(look.z(12.))
                .mb(look.z(6.))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(look.z(6.))
                .text_size(look.text(0.))
                .child(counts(summary, add, del))
                .child(
                    div()
                        .id("turn-review")
                        .flex_none()
                        .px(look.z(6.))
                        .rounded(look.z(4.))
                        .cursor_pointer()
                        .text_color(look.c(self.accent))
                        .hover(|d| d.bg(look.c(ui.hover_bg)))
                        .child("Review")
                        .on_click(cx.listener(|t, _, _, cx| t.show_changes(cx))),
                )
                .children(chips),
        )
    }

    fn plan_strip(&self, look: &Look) -> Option<impl IntoElement> {
        if self.plan.is_empty() {
            return None;
        }
        let ui = &look.theme.ui;
        let done = self.plan.iter().filter(|p| p.status == PlanStatus::Completed).count();
        Some(
            div()
                .flex_none()
                .px(look.z(16.))
                .py(look.z(8.))
                .border_b_1()
                .border_color(look.c(ui.border))
                .flex()
                .flex_col()
                .gap(look.z(3.))
                .text_size(look.text(0.))
                .child(div().text_size(look.text(0.)).font_weight(gpui::FontWeight::MEDIUM).text_color(look.c(ui.text_muted)).child(format!("TO-DO  {done}/{}", self.plan.len())))
                .children(self.plan.iter().map(|p| {
                    let (g, c) = match p.status {
                        PlanStatus::Completed => ("●", ui.text_muted),
                        PlanStatus::InProgress => ("◐", ui.text),
                        PlanStatus::Pending => ("○", ui.text_secondary),
                    };
                    div()
                        .flex()
                        .gap(look.z(8.))
                        .text_color(look.c(c))
                        .when(p.status == PlanStatus::InProgress, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                        .child(div().flex_none().text_color(look.c(self.accent)).child(g))
                        .child(div().min_w(px(0.)).child(p.content.clone()))
                })),
        )
    }

    fn permission_bar(&self, look: &Look, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = self.permissions.front()?;
        let waiting = self.permissions.len();
        let ui = &look.theme.ui;
        let what = p.tool.title.clone().unwrap_or_else(|| "use a tool".into());
        let file = p.tool.locations.as_ref().and_then(|l| l.first()).map(|l| self.rel(&l.path));
        // What's being approved: the diff, text or command, as in the chat.
        let tool = self.tools.get(&p.tool.id).cloned().map(|mut t| {
            t.apply(&p.tool);
            t
        });
        let tool = tool.unwrap_or_else(|| p.tool.to_tool_call());
        let diffs = tool_diffs(&tool);
        let detail = self.tool_body(&tool, &diffs, look, None, cx);
        // Every option the agent offers, in its words ("Yes, and auto-accept
        // edits", "No, keep planning"…); the first allow is the primary one.
        let primary = p.options.iter().position(|o| o.kind == PermissionKind::AllowOnce).or_else(|| p.options.iter().position(|o| matches!(o.kind, PermissionKind::AllowAlways)));
        // Cmd+Y / Cmd+Opt+Y / Cmd+Opt+Z answer with the first option of their kind.
        let key_for = |i: usize, o: &PermissionOption| -> Option<String> {
            let action: Box<dyn gpui::Action> = match o.kind {
                PermissionKind::AllowOnce => Box::new(aa::AllowOnce),
                PermissionKind::AllowAlways => Box::new(aa::AllowAlways),
                PermissionKind::RejectOnce => Box::new(aa::Deny),
                PermissionKind::RejectAlways => return None,
            };
            let first = p.options.iter().position(|x| x.kind == o.kind) == Some(i);
            let keys = crate::commands::find(action.as_ref())?.first_keys();
            (first && !keys.is_empty()).then(|| crate::commands::display_keys(&keys))
        };
        let buttons: Vec<_> = p
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let is_primary = Some(i) == primary;
                let reject = matches!(o.kind, PermissionKind::RejectOnce | PermissionKind::RejectAlways);
                let label = if o.name.trim().is_empty() {
                    match o.kind {
                        PermissionKind::AllowOnce => "Allow once",
                        PermissionKind::AllowAlways => "Allow for this task",
                        PermissionKind::RejectOnce => "Deny",
                        PermissionKind::RejectAlways => "Always deny",
                    }
                    .to_string()
                } else {
                    o.name.clone()
                };
                let id = o.id.clone();
                let key = key_for(i, o);
                div()
                    .id(("perm-option", i))
                    .px(look.z(10.))
                    .py(look.z(3.))
                    .rounded(look.z(4.))
                    .cursor_pointer()
                    .text_size(look.text(0.))
                    .bg(look.c(if is_primary { ui.button_primary_bg } else { ui.button_bg }))
                    .text_color(look.c(if is_primary { ui.button_primary_text } else if reject { ui.text_secondary } else { ui.button_text }))
                    .flex()
                    .items_center()
                    .gap(look.z(6.))
                    .child(label)
                    .children(key.map(|k| div().text_size(look.text(-2.)).opacity(0.6).child(k)))
                    .on_click(cx.listener(move |t, _, _, cx| t.answer_option(&id, cx)))
            })
            .collect();
        // More waiting: allow them all at once (each "allow once").
        let allow_all = (waiting > 1).then(|| {
            div()
                .id("perm-allow-all")
                .px(look.z(10.))
                .py(look.z(3.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_size(look.text(0.))
                .border_1()
                .border_color(look.c(ui.border_strong))
                .text_color(look.c(ui.text))
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .child(format!("Allow all {waiting} once"))
                .on_click(cx.listener(|t, _, _, cx| {
                    t.allow_all_pending(cx);
                }))
        });
        Some(
            div()
                .flex_none()
                .mx(look.z(12.))
                .mb(look.z(6.))
                .px(look.z(12.))
                .py(look.z(8.))
                .rounded(look.z(6.))
                .border_1()
                .border_color(look.c(self.accent))
                .bg(look.c(ui.permission_bar_bg))
                .flex()
                .flex_col()
                .gap(look.z(6.))
                // The buttons come right after the title, so a long command or
                // diff can't push them off screen; both of those scroll in
                // their own boxes instead of growing the bar over the chat.
                .child(
                    div()
                        .id("perm-title")
                        .min_w(px(0.))
                        .max_h(look.z(90.))
                        .overflow_y_scroll()
                        .text_color(look.c(ui.text))
                        .child(format!("{} wants to {what}", self.def.label))
                        .when(waiting > 1, |d| d.child(div().text_size(look.text(-1.)).text_color(look.c(ui.text_muted)).child(format!("1 of {waiting} requests waiting")))),
                )
                .child(div().flex().flex_wrap().gap(look.z(6.)).children(buttons).children(allow_all))
                .when(file.is_some() || detail.is_some(), |d| {
                    d.child(
                        div()
                            .id("perm-what")
                            .min_w(px(0.))
                            .max_h(look.z(220.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(look.z(4.))
                            .children(file.map(|f| div().truncate().text_color(look.c(ui.text_muted)).child(f)))
                            .children(detail),
                    )
                }),
        )
    }
}

/// Lines an unfolded tool call shows of a diff, output or raw input.
const MAX_TOOL_LINES: usize = 200;
/// Lines of an exited agent's stderr an error callout keeps (of ~4 KB).
const MAX_STDERR_LINES: usize = 80;

/// `s` quoted for a POSIX shell when it needs it.
/// A reviewer's session settings: its read-only mode (`plan`, or Codex's
/// read-only), then the model picked for it. Returns the settings as the
/// agent now has them. A refusal is logged; Qur's deny still applies.
async fn review_setup(client: &Client, session: &SessionId, mut settings: Vec<ConfigOption>, role: &ReviewRole) -> Vec<ConfigOption> {
    let mut wanted: Vec<(String, String)> = Vec::new();
    match review::read_only_mode(&settings) {
        Some(m) => wanted.push(m),
        None => println!("REVIEW {session} no read-only mode offered: Qur's deny is all there is"),
    }
    if let Some(m) = &role.model
        && let Some(o) = review::model_option(&settings)
        && o.current != *m
        && o.values.iter().any(|v| v.value == *m)
    {
        wanted.push((o.id.clone(), m.clone()));
    }
    for (id, value) in wanted {
        let Some(opt) = settings.iter().find(|o| o.id == id).cloned() else { continue };
        match client.set_option(session, &opt, &value).await {
            Ok(Ok(Some(list))) => {
                let legacy = settings.iter().find(|o| o.legacy_mode).cloned().filter(|_| !list.iter().any(ConfigOption::is_mode));
                settings = legacy.into_iter().chain(list).collect();
                println!("REVIEW {session} set {id}={value}");
            }
            Ok(Ok(None)) => {
                if let Some(o) = settings.iter_mut().find(|o| o.id == id) {
                    o.current = value.clone();
                }
                println!("REVIEW {session} set {id}={value}");
            }
            Ok(Err(e)) => eprintln!("REVIEW {session} couldn't set {id}={value}: {e}"),
            Err(_) => {}
        }
    }
    settings
}

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

impl AgentTask {
    // -- find in chat ------------------------------------------------------------

    /// Find in Chat (Cmd+F): open the query field above the chat, or focus it.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_subagent = None;
        Docks::update(cx, Kind::Acp, |d| d.activate(Panel::Session));
        if let Some(f) = &self.find {
            let input = f.input.clone();
            input.update(cx, |i, cx| i.select_text(cx));
            let focus = input.read(cx).focus.clone();
            window.focus(&focus, cx);
            return cx.notify();
        }
        let input = cx.new(|cx| TextInput::new("Find in chat · ↩ older · ⇧↩ newer · esc", cx));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Changed => {
                let q = input.read(cx).text.clone();
                this.set_query(q, cx);
            }
            InputEvent::Confirm { alt, .. } => this.find_step(!*alt, cx),
            InputEvent::Cancel => this.close_find(window, cx),
            _ => {}
        });
        let focus = input.read(cx).focus.clone();
        self.find = Some(ChatFind { input: input.clone(), query: String::new(), matches: Vec::new(), current: None, dirty: false, _sub: sub });
        window.focus(&focus, cx);
        // Dev/test: QUR_DEBUG_FIND="text" searches for it at once.
        if let Ok(q) = std::env::var("QUR_DEBUG_FIND") {
            input.update(cx, |i, cx| i.set_text(&q, cx));
        }
        cx.notify();
    }

    /// Esc: close Find; the prompt gets the keyboard back.
    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.find_current().map(|c| c.0);
        self.find = None;
        // A folded item opened for the current match folds again.
        if let Some(r) = row.filter(|r| *r < self.chat.len()) {
            self.list.remeasure_items(r..r + 1);
        }
        let f = self.prompt.read(cx).focus.clone();
        window.focus(&f, cx);
        cx.notify();
    }

    /// (row, number within the row) of Find's current match.
    fn find_current(&self) -> Option<(usize, usize)> {
        let f = self.find.as_ref()?;
        f.matches.get(f.current?).copied()
    }

    /// Find's marks for row `ix`, when it has matches.
    fn row_hl(&self, ix: usize, look: &Look) -> Option<FindHl> {
        let f = self.find.as_ref().filter(|f| !f.query.is_empty())?;
        let at = f.matches.partition_point(|m| m.0 < ix);
        if !matches!(f.matches.get(at), Some(m) if m.0 == ix) {
            return None;
        }
        let current = self.find_current().filter(|c| c.0 == ix).map(|c| c.1);
        let ed = &look.theme.editor;
        let bg = |c: Rgba| HighlightStyle { background_color: Some(look.c(c)), ..Default::default() };
        Some(FindHl::new(f.query.clone(), current, bg(ed.search_match), bg(ed.search_match_active)))
    }

    /// How many matches row `ix` has, counted in the order `render_item`
    /// draws them (a tool call: its title, then its text and output).
    fn row_matches(&self, ix: usize, q: &str, eid: EntityId) -> usize {
        let n = |s: &str| find_all(s, q).len();
        let root = Some(self.record.worktree.as_path());
        match &self.chat[ix] {
            ChatItem::User(t) | ChatItem::Notice(t) | ChatItem::Output(t) => n(t),
            ChatItem::Agent(t) => crate::markdown::count_matches(t, format!("md-{eid}-{ix}"), root, q),
            ChatItem::Thought(t) => n(t.trim()),
            ChatItem::Link { name, .. } => n(name),
            ChatItem::Failure(f) => n(&f.title) + f.summary.as_deref().map_or(0, n) + f.detail.as_deref().map_or(0, n),
            ChatItem::Tool(id) => {
                let Some(t) = self.tools.get(id) else { return 0 };
                let mut count = n(&tool_title(t));
                // As `tool_body` numbers its parts.
                let mut parts = 0;
                for c in &t.content {
                    match c {
                        ToolContent::Text(s) if !s.trim().is_empty() => {
                            count += crate::markdown::count_matches(s, format!("md-{eid}-{}-{parts}", t.id), root, q);
                            parts += 1;
                        }
                        ToolContent::Terminal(_) => {
                            let out = t.output.as_deref().unwrap_or("").trim_end();
                            if !out.is_empty() || t.exit.is_some() {
                                count += n(&tail_lines(out, MAX_TOOL_LINES));
                            }
                            parts += 1;
                        }
                        _ => {}
                    }
                }
                count
            }
            ChatItem::Image(..) | ChatItem::Turn(_) => 0,
        }
    }

    /// Count every match again (the query or the chat changed), keeping
    /// the current one when it's still there.
    fn refind(&mut self, eid: EntityId) {
        let Some(q) = self.find.as_ref().map(|f| f.query.clone()) else { return };
        let mut matches = Vec::new();
        if !q.is_empty() {
            for ix in 0..self.chat.len() {
                matches.extend((0..self.row_matches(ix, &q, eid)).map(|k| (ix, k)));
            }
        }
        let Some(f) = self.find.as_mut() else { return };
        let was = f.current.and_then(|c| f.matches.get(c).copied());
        f.current = was.and_then(|c| matches.iter().position(|m| *m == c));
        f.matches = matches;
        f.dirty = false;
        if let Some((row, _)) = was.filter(|_| f.current.is_none())
            && row < self.chat.len()
        {
            self.list.remeasure_items(row..row + 1);
        }
    }

    /// The query changed: find it, the newest match first.
    fn set_query(&mut self, q: String, cx: &mut Context<Self>) {
        let old = self.find_current().map(|c| c.0);
        let Some(f) = self.find.as_mut() else { return };
        f.query = q;
        f.current = None;
        self.refind(cx.entity_id());
        let Some(f) = self.find.as_mut() else { return };
        f.current = f.matches.len().checked_sub(1);
        println!("FIND {} {:?}: {} matches", self.record.id, f.query, f.matches.len());
        self.moved_to(old, cx);
    }

    /// Enter (older) / Shift+Enter (newer): the next match, round the ends.
    fn find_step(&mut self, older: bool, cx: &mut Context<Self>) {
        if self.find.as_ref().is_some_and(|f| f.dirty) {
            self.refind(cx.entity_id());
        }
        let old = self.find_current().map(|c| c.0);
        let Some(f) = self.find.as_mut() else { return };
        let n = f.matches.len();
        if n == 0 {
            return cx.notify();
        }
        f.current = Some(match f.current {
            None if older => n - 1,
            None => 0,
            Some(c) if older => (c + n - 1) % n,
            Some(c) => (c + 1) % n,
        });
        self.moved_to(old, cx);
    }

    /// Find's current match moved off row `old`: re-measure both rows (a
    /// folded thought, tool call or group opens to show it) and scroll to it.
    fn moved_to(&mut self, old: Option<usize>, cx: &mut Context<Self>) {
        let new = self.find_current();
        for row in [old, new.map(|c| c.0)].into_iter().flatten().filter(|r| *r < self.chat.len()) {
            self.list.remeasure_items(row..row + 1);
        }
        if let Some((row, k)) = new {
            self.list.set_follow_mode(gpui::FollowMode::Normal);
            self.list.scroll_to_reveal_item(row);
            let total = self.find.as_ref().map_or(0, |f| f.matches.len());
            let at = self.find.as_ref().and_then(|f| f.current).map_or(0, |c| c + 1);
            println!("FIND {} match {at} of {total}: row {row} #{k}", self.record.id);
        }
        cx.notify();
    }

    /// The query field, the match count, ↑ ↓ and ✕ (above the chat).
    fn find_bar(&self, look: &Look, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let f = self.find.as_ref()?;
        let ui = &look.theme.ui;
        let n = f.matches.len();
        let none = !f.query.is_empty() && n == 0;
        let count = match (f.query.is_empty(), f.current) {
            (true, _) => String::new(),
            (false, _) if n == 0 => "no match".to_string(),
            (false, Some(c)) => format!("{} of {n}", c + 1),
            (false, None) => format!("{n} match{}", if n == 1 { "" } else { "es" }),
        };
        let button = |id: &'static str, label: &'static str| {
            div().id(id).flex_none().px(look.z(4.)).rounded(look.z(4.)).cursor_pointer().text_color(look.c(ui.text_muted)).hover(|d| d.bg(look.c(ui.hover_bg)).text_color(look.c(ui.text))).child(label)
        };
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(look.z(6.))
                .mx(look.z(12.))
                .mt(look.z(6.))
                .mb(look.z(2.))
                .px(look.z(8.))
                .py(look.z(3.))
                .rounded(look.z(4.))
                .border_1()
                .border_color(look.c(if none { ui.error } else { ui.border }))
                .bg(look.c(ui.overlay_bg))
                .text_size(look.text(-1.))
                .child(div().flex_1().min_w(px(0.)).child(f.input.clone()))
                .child(div().flex_none().text_color(look.c(ui.text_muted)).child(count))
                .child(button("find-older", "↑").on_click(cx.listener(|t, _, _, cx| t.find_step(true, cx))))
                .child(button("find-newer", "↓").on_click(cx.listener(|t, _, _, cx| t.find_step(false, cx))))
                .child(button("find-close", "✕").on_click(cx.listener(|t, _, window, cx| t.close_find(window, cx)))),
        )
    }

    /// Copy Chat as Markdown (Agent menu, palette, the sidebar's right-click menu).
    pub fn chat_markdown(&self) -> String {
        chat_markdown(&self.record.title, &self.def.label, &self.chat, &self.tools, &self.record.turns)
    }

    /// `text` as Markdown; file links open in the code modal. `id`: unique
    /// and stable per chat item (see `markdown::render`).
    fn markdown(&self, text: &str, id: String, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        self.markdown_find(text, id, look, None, cx)
    }

    /// `markdown`, with Find in Chat's matches marked.
    fn markdown_find(&self, text: &str, id: String, look: &Look, hl: Option<&FindHl>, cx: &mut Context<Self>) -> gpui::Div {
        let me = cx.entity().downgrade();
        let open: crate::markdown::OnFile = std::rc::Rc::new(move |path, line, _, cx| {
            me.update(cx, |_, cx| cx.emit(AgentEvent::OpenFile(path, line))).ok();
        });
        crate::markdown::render(text, id, Some(&self.record.worktree), look, open, hl)
    }
}

impl Focusable for AgentTask {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AgentTask {
    /// To-do + Chat + permission bar + prompt (the "side notes").
    /// Show a subagent's work in place of the chat (`None`: back to the chat).
    pub fn show_subagent(&mut self, id: Option<SessionId>, cx: &mut Context<Self>) {
        if id.is_some() {
            self.open_review = false;
        }
        self.open_subagent = id;
        cx.notify();
    }

    /// Stop a running subagent (the agent grants `cancel` on its session).
    pub fn stop_subagent(&mut self, id: &SessionId, cx: &mut Context<Self>) {
        if let Some(c) = &self.client {
            c.cancel(id);
        }
        if let Some(s) = self.subagents.iter_mut().find(|s| s.id == *id) {
            s.state = "stopped".into();
        }
        cx.emit(AgentEvent::Subagents);
        cx.notify();
    }

    fn subagent_view(&self, sub: &Subagent, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let running = sub.state == "running";
        let id = sub.id.clone();
        let button = |label: &'static str, key: &'static str| {
            div().id(key).flex_none().px(look.z(8.)).py(look.z(2.)).rounded(look.z(4.)).cursor_pointer().bg(look.c(ui.button_bg)).text_color(look.c(ui.button_text)).child(label)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px(look.z(16.))
                    .py(look.z(8.))
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .border_b_1()
                    .border_color(look.c(ui.border))
                    .child(button("← Chat", "sub-back").on_click(cx.listener(|t, _, _, cx| t.show_subagent(None, cx))))
                    .child(div().flex_1().min_w(px(0.)).truncate().font_weight(gpui::FontWeight::MEDIUM).child(format!("↳ {}", sub.title)))
                    .child(div().flex_none().text_color(look.c(ui.text_muted)).child(sub.state.replace('_', " ")))
                    .when(running, |d| d.child(button("Stop", "sub-stop").on_click(cx.listener(move |t, _, _, cx| t.stop_subagent(&id, cx))))),
            )
            .child(
                div()
                    .id("sub-log")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .px(look.z(16.))
                    .py(look.z(8.))
                    .flex()
                    .flex_col()
                    .gap(look.z(6.))
                    .when(sub.log.is_empty(), |d| d.child(div().text_color(look.c(ui.text_muted)).child("Nothing from this subagent yet.")))
                    .children(sub.log.iter().enumerate().map(|(n, (tool, text))| {
                        if *tool {
                            div().text_color(look.c(ui.text_secondary)).child(format!("· {}", text.trim()))
                        } else {
                            self.markdown(text.trim(), format!("md-{}-{}-{n}", cx.entity_id(), sub.id), look, cx)
                        }
                    })),
            )
    }

    fn notes(&self, show_plan: bool, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        if self.open_review
            && let Some(r) = &self.reviewer
        {
            return self.review_view(r, look, cx);
        }
        if let Some(sub) = self.open_subagent.as_ref().and_then(|id| self.subagents.iter().find(|s| s.id == *id)) {
            return self.subagent_view(sub, look, cx);
        }
        let ui = look.theme.ui.clone();
        let me = cx.entity();
        let look2 = look.clone();
        // Sized here too, so nothing in a row falls back to GPUI's default size.
        let chat = list(self.list.clone(), move |ix, _, cx| me.update(cx, |t, cx| t.render_item(ix, &look2, cx))).size_full().text_size(look.text(0.));
        // Scrolled up: a "↓" to jump back to the newest message.
        let jump = (!self.list.is_following_tail() && self.list.is_scrolled_to_end() == Some(false)).then(|| {
            div()
                .id("chat-bottom")
                .absolute()
                .right(look.z(16.))
                .bottom(look.z(8.))
                .px(look.z(9.))
                .py(look.z(3.))
                .rounded(look.z(12.))
                .cursor_pointer()
                .border_1()
                .border_color(look.c(ui.border_strong))
                .bg(look.c(ui.overlay_bg))
                .text_size(look.text(0.))
                .text_color(look.c(ui.text_secondary))
                .hover(|d| d.text_color(look.c(ui.text)))
                .child("↓")
                .on_click(cx.listener(|t, _, _, cx| {
                    t.follow_bottom();
                    cx.notify();
                }))
        });
        let chat = div().flex_1().min_h(px(0.)).relative().child(chat).children(jump);
        let working = matches!(self.status, Status::Working | Status::NeedsYou);
        let empty = self.chat.is_empty();
        let prompt_bar = div()
            .flex_none()
            .mx(look.z(12.))
            .mb(look.z(12.))
            .px(look.z(12.))
            .py(look.z(8.))
            .rounded(look.z(6.))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .bg(look.c(ui.editor_bg))
            .child(self.prompt.clone());
        let busy = working || !self.pending.is_empty();
        // "24k / 1M · $0.31": context used of the window (warning colour
        // past 80 %, as Zed does), and the session's cost.
        let usage = self.usage.filter(|u| u.1 > 0).map(|(used, size, cost)| {
            let k = |n: u64| if n >= 1_000_000 { format!("{}M", (n as f64 / 1e6 * 10.).round() / 10.) } else { format!("{}k", n.div_ceil(1000)) };
            let text = format!("{} / {}{}", k(used), k(size), cost.map(|c| format!(" · ${c:.2}")).unwrap_or_default());
            let hot = used as f64 >= size as f64 * 0.8;
            div().flex_none().mr(look.z(6.)).when(hot, |d| d.text_color(look.c(ui.warning))).child(text)
        });
        // Just above the field, on the right: usage, then queue and Esc
        // hints while working. Settings are in the `/` menu, not here.
        let hints = (busy || usage.is_some()).then(|| {
            div()
                .flex_none()
                .mx(look.z(12.))
                .mb(look.z(3.))
                .flex()
                .justify_end()
                .items_center()
                .gap(look.z(2.))
                .text_size(look.text(0.))
                .text_color(look.c(ui.text_muted))
                .children(usage)
                .when(!self.pending.is_empty(), |d| d.child(div().flex_none().mr(look.z(6.)).text_color(look.c(ui.text_secondary)).child(format!("{} queued", self.pending.len()))))
                .when(working, |d| d.child(div().min_w(px(0.)).mr(look.z(2.)).truncate().child("esc to stop")))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(show_plan, |d| d.children(self.plan_strip(look)))
            .children(self.find_bar(look, cx))
            .when(empty, |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px(look.z(16.))
                        .text_color(look.c(ui.text_muted))
                        .child(match self.status {
                            _ if self.setting_up => "Setting up the worktree…".to_string(),
                            Status::Starting => "Starting the agent…".to_string(),
                            Status::Stopped => format!("{} is stopped. Send a message to resume.", self.def.label),
                            _ => "No messages yet.".to_string(),
                        }),
                )
            })
            .when(!empty, |d| d.child(chat))
            .children(self.sign_in_bar(look, cx))
            .children(self.permission_bar(look, cx))
            .children(self.files_strip(look, cx))
            .when(!self.pending.is_empty(), |d| d.child(self.queue_list(look, cx)))
            .children(hints)
            .child(prompt_bar)
    }

    /// Queued messages, oldest (next to send) first, each removable.
    fn queue_list(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let row_button = |label: &'static str, id: (&'static str, usize)| {
            div()
                .id(id)
                .flex_none()
                .px(look.z(6.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_size(look.text(-1.))
                .text_color(look.c(ui.text_muted))
                .hover(|d| d.bg(look.c(ui.hover_bg)).text_color(look.c(ui.text)))
                .child(label)
        };
        div()
            .flex_none()
            .mx(look.z(12.))
            .mb(look.z(6.))
            .flex()
            .flex_col()
            .gap(look.z(4.))
            .child(div().text_size(look.text(0.)).font_weight(gpui::FontWeight::MEDIUM).text_color(look.c(ui.text_muted)).child(format!("QUEUED  {}", self.pending.len())))
            .children(self.pending.iter().enumerate().map(|(i, o)| {
                let labels = if o.labels.is_empty() { String::new() } else { format!("  [{}]", o.labels.join(", ")) };
                div()
                    .flex()
                    .items_start()
                    .gap(look.z(8.))
                    .px(look.z(10.))
                    .py(look.z(4.))
                    .rounded(look.z(4.))
                    .bg(look.c(ui.active_bg))
                    .text_color(look.c(ui.text_secondary))
                    .child(div().flex_1().min_w(px(0.)).child(format!("{}{labels}", o.text.trim())))
                    .child(row_button("Send now", ("queue-send", i)).on_click(cx.listener(move |t, _, _, cx| t.send_queued_now(i, cx))))
                    .child(row_button("Edit", ("queue-edit", i)).on_click(cx.listener(move |t, _, window, cx| {
                        t.edit_queued(i, cx);
                        let f = t.prompt.read(cx).focus.clone();
                        window.focus(&f, cx);
                    })))
                    .child(
                        div()
                            .id(("queue-x", i))
                            .flex_none()
                            .cursor_pointer()
                            .text_color(look.c(ui.text_muted))
                            .hover(|d| d.text_color(look.c(ui.text)))
                            .child("✕")
                            .on_click(cx.listener(move |t, _, _, cx| t.unqueue(i, cx))),
                    )
            }))
    }

    fn header(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = look.theme.ui.clone();
        let working = matches!(self.status, Status::Working | Status::NeedsYou);
        let elapsed = self.turn_started.filter(|_| working).map(|t| format!("  {}s", t.elapsed().as_secs())).unwrap_or_default();
        div()
            .flex_none()
            .h(look.z(36.))
            .px(look.z(16.))
            .flex()
            .items_center()
            .gap(look.z(10.))
            .border_b_1()
            .border_color(look.c(ui.border))
            .child(div().flex_none().size(look.z(10.)).rounded(look.z(2.5)).bg(look.c(self.accent)))
            .child(div().min_w(px(0.)).truncate().font_weight(gpui::FontWeight::MEDIUM).child(self.record.title.clone()))
            .child(
                div()
                    .flex_none()
                    .text_size(look.text(-1.))
                    .text_color(look.c(if matches!(self.status, Status::Error(_)) { ui.error } else { ui.text_muted }))
                    .child(format!("{}{elapsed}", self.status.label())),
            )
            .child(div().flex_1())
            .child(div().flex_none().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(self.record.branch.clone()))
            .when(working, |d| {
                d.child(
                    div()
                        .id("stop")
                        .flex_none()
                        .px(look.z(8.))
                        .py(look.z(2.))
                        .rounded(look.z(4.))
                        .border_1()
                        .border_color(look.c(ui.border_strong))
                        .cursor_pointer()
                        .text_size(look.text(-1.))
                        .child("Stop")
                        .on_click(cx.listener(|t, _, _, cx| t.stop(cx))),
                )
            })
    }

    /// Cmd+1..5: bring a panel to the front of its pane.
    pub fn set_view(&mut self, v: View, cx: &mut Context<Self>) {
        if v == View::Changes {
            self.changes.update(cx, |c, cx| c.refresh(cx));
        }
        Docks::update(cx, Kind::Acp, |d| d.activate(v.panel()));
        cx.notify();
    }

    pub fn set_layout(&mut self, l: Layout, cx: &mut Context<Self>) {
        self.layout = l;
        cx.notify();
    }

}

impl AgentTask {
    /// Does the stage show this task's browser right now?
    pub fn shows_browser(&self, cx: &App) -> bool {
        Docks::get(cx, Kind::Acp).is_visible(Panel::Browser)
    }

    pub fn show_browser(&mut self, on: bool, cx: &mut Context<Self>) {
        Docks::update(cx, Kind::Acp, |d| d.activate(if on { Panel::Browser } else { Panel::Live }));
        cx.notify();
    }

    /// Review Changes (Cmd+4 / Agent menu): show the diff, freshly computed.
    pub fn show_changes(&mut self, cx: &mut Context<Self>) {
        Docks::update(cx, Kind::Acp, |d| d.activate(Panel::Changes));
        self.changes.update(cx, |c, cx| c.show_all(cx));
        cx.notify();
    }

    // -- turn checkpoints ------------------------------------------------------

    /// A prompt is about to go out: record the turn with the worktree's tree.
    fn begin_turn(&mut self, start: Result<String, String>, cx: &mut Context<Self>) {
        let start = match start {
            Ok(t) => t,
            Err(e) => {
                if self.review.is_none() {
                    eprintln!("TURN {} no checkpoint: {e}", self.record.id);
                }
                self.turn_n = None;
                return;
            }
        };
        let n = self.record.next_turn_number();
        println!("TURN {} {n} start {}", self.record.id, short(&start));
        self.pin(n, Some(("start", start.clone())), cx);
        self.record.turns.push(qur_agents::Turn { n, start, ..Default::default() });
        self.turn_n = Some(n);
        cx.emit(AgentEvent::Changed);
        cx.emit(AgentEvent::TurnsChanged);
    }

    /// Pin a checkpoint under `refs/qur/turns/<task>/<n>/<side>`, or (None)
    /// drop turn N's refs.
    fn pin(&self, n: u32, side: Option<(&'static str, String)>, cx: &mut Context<Self>) {
        let (wt, id) = (self.record.worktree.clone(), self.record.id.clone());
        cx.background_executor()
            .spawn(async move {
                let r = match side {
                    Some((side, tree)) => qur_git::pin_turn(&wt, &id, n, side, &tree),
                    None => qur_git::unpin_turn(&wt, &id, n),
                };
                if let Err(e) = r {
                    eprintln!("TURN {id} {n} ref: {e}");
                }
            })
            .detach();
    }

    /// The turn ended (any way): take the end checkpoint, add the chat's
    /// turn line, then send what's queued. A turn that changed nothing
    /// leaves no checkpoint.
    fn end_turn(&mut self, cx: &mut Context<Self>) {
        let Some((n, start)) = self.turn_n.take().and_then(|n| self.record.turn(n).map(|t| (n, t.start.clone()))) else {
            return self.after_turn(cx);
        };
        let wt = self.record.worktree.clone();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let r = bg
                .spawn(async move {
                    let end = qur_git::snapshot_tree(&wt)?;
                    let stats = qur_git::tree_stats(&wt, &start, &end)?;
                    Ok::<_, String>((end, stats))
                })
                .await;
            this.update(cx, |t, cx| {
                let id = t.record.id.clone();
                match r {
                    Ok((end, (files, added, removed))) if files > 0 => {
                        println!("TURN {id} {n} end {} files={files} +{added} -{removed}", short(&end));
                        t.pin(n, Some(("end", end.clone())), cx);
                        if let Some(turn) = t.record.turns.iter_mut().find(|x| x.n == n) {
                            turn.end = Some(end);
                            (turn.files, turn.added, turn.removed) = (files, added, removed);
                        }
                        t.push(ChatItem::Turn(n));
                    }
                    Ok(_) => {
                        println!("TURN {id} {n} end no changes");
                        t.record.turns.retain(|x| x.n != n);
                        t.pin(n, None, cx);
                    }
                    // The start checkpoint stays (Rewind still works).
                    Err(e) => eprintln!("TURN {id} {n} no end checkpoint: {e}"),
                }
                cx.emit(AgentEvent::Changed);
                cx.emit(AgentEvent::TurnsChanged);
                t.after_turn(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After a turn: a Rewind that was waiting for the stop, else the queue.
    fn after_turn(&mut self, cx: &mut Context<Self>) {
        if let Some(n) = self.rewind_after_stop.take() {
            return self.do_rewind(n, cx);
        }
        self.drain_pending(cx);
    }

    /// Is a turn running (Rewind stops it first)?
    pub fn turn_running(&self) -> bool {
        matches!(self.status, Status::Working | Status::NeedsYou) || self.turn_n.is_some()
    }

    /// Rewind to before turn N (asked already): stop the agent if it's
    /// working, then put the worktree's files back.
    pub fn rewind(&mut self, n: u32, cx: &mut Context<Self>) {
        if self.rewinding {
            return;
        }
        if self.turn_running() {
            self.rewind_after_stop = Some(n);
            self.notice(format!("Stopping {} to rewind…", self.def.label), cx);
            if matches!(self.status, Status::Working | Status::NeedsYou) {
                self.stop(cx);
            }
            return;
        }
        self.do_rewind(n, cx);
    }

    fn do_rewind(&mut self, n: u32, cx: &mut Context<Self>) {
        let Some(start) = self.record.turn(n).map(|t| t.start.clone()) else {
            self.notice(format!("Nothing to rewind: turn {n} changed no files."), cx);
            return self.drain_pending(cx);
        };
        self.rewinding = true;
        let (root, wt) = (self.project_root.clone(), self.record.worktree.clone());
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let r = bg
                .spawn(async move {
                    let now = qur_git::snapshot_tree(&wt)?;
                    let undone = qur_git::changed_paths_between(&wt, &start, &now)?;
                    qur_git::restore_tree(&root, &wt, &start)?;
                    Ok::<_, String>(undone)
                })
                .await;
            this.update(cx, |t, cx| {
                t.rewinding = false;
                match r {
                    Ok(paths) => {
                        let undone: Vec<u32> = t.record.turns.iter().filter(|x| x.n >= n && !x.rewound).map(|x| x.n).collect();
                        for x in t.record.turns.iter_mut().filter(|x| x.n >= n) {
                            x.rewound = true;
                        }
                        println!("REWIND {} turn {n}: {} files restored", t.record.id, paths.len());
                        let turns = match undone.as_slice() {
                            [] | [_] => format!("turn {n}"),
                            more => format!("turns {n}–{}", more[more.len() - 1]),
                        };
                        let k = paths.len();
                        t.push(ChatItem::Notice(format!(
                            "Rewound {turns}: {k} file{} put back to how they were before it. {} will be told with your next message.",
                            if k == 1 { "" } else { "s" },
                            t.def.label
                        )));
                        let mut list = paths.iter().take(20).map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
                        if k > 20 {
                            list.push_str(&format!(" and {} more", k - 20));
                        }
                        t.rewind_note = Some(format!(
                            "[Note from Qur, the editor: the user rewound your work. The files in your working directory are back to how they were before {turns} of this conversation, so the edits you made since then are gone ({}). Re-read files before you change them.]",
                            if list.is_empty() { "no files differed".to_string() } else { format!("changed back: {list}") }
                        ));
                        // The turn lines now say "rewound".
                        for ix in 0..t.chat.len() {
                            if matches!(t.chat[ix], ChatItem::Turn(m) if m >= n) {
                                t.list.splice(ix..ix + 1, 1);
                            }
                        }
                        t.changes.update(cx, |c, cx| c.refresh(cx));
                    }
                    Err(e) => {
                        eprintln!("REWIND {} turn {n} failed: {e}", t.record.id);
                        t.push(ChatItem::Notice(format!("Couldn't rewind: {e}")));
                    }
                }
                cx.emit(AgentEvent::Changed);
                cx.emit(AgentEvent::TurnsChanged);
                t.drain_pending(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Show what turn N changed in the Changes view (comments work as usual).
    pub fn show_turn(&mut self, n: u32, cx: &mut Context<Self>) {
        let Some(t) = self.record.turn(n).cloned() else { return };
        Docks::update(cx, Kind::Acp, |d| d.activate(Panel::Changes));
        self.changes.update(cx, |c, cx| c.show_turn(n, t.start, t.end, cx));
        cx.notify();
    }

    /// "3 files · +12 −4 · View · Rewind" at the end of a turn.
    fn turn_line(&self, ix: usize, n: u32, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let Some(t) = self.record.turn(n) else { return div() };
        let link = |id: &'static str, label: &'static str| {
            div().id((id, ix)).cursor_pointer().text_color(look.c(ui.text_secondary)).hover(|d| d.text_color(look.c(ui.text))).child(label)
        };
        let dot = || div().child("·");
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.z(6.))
            .text_size(look.text(-1.))
            .text_color(look.c(ui.text_muted))
            .child(format!("{} file{}", t.files, if t.files == 1 { "" } else { "s" }))
            .child(dot())
            .child(
                div()
                    .flex()
                    .gap(look.z(4.))
                    .child(div().text_color(look.c(look.theme.diff.added_text)).child(format!("+{}", t.added)))
                    .child(div().text_color(look.c(look.theme.diff.removed_text)).child(format!("−{}", t.removed))),
            )
            .child(dot())
            .child(link("turn-view", "View").on_click(cx.listener(move |t, _, _, cx| t.show_turn(n, cx))))
            .child(dot())
            .when(t.rewound, |d| d.child("rewound"))
            .when(!t.rewound, |d| d.child(link("turn-rewind", "Rewind").on_click(cx.listener(move |_, _, _, cx| cx.emit(AgentEvent::Rewind(n))))))
    }
}

// -- second-opinion review ------------------------------------------------------

impl AgentTask {
    /// The chat for a reviewer: messages and tool calls, oldest first.
    pub fn thread_entries(&self) -> Vec<review::Entry> {
        self.chat
            .iter()
            .filter_map(|c| match c {
                ChatItem::User(t) => Some(review::Entry::new(review::Role::User, t.clone())),
                ChatItem::Agent(t) => Some(review::Entry::new(review::Role::Agent, t.clone())),
                ChatItem::Tool(id) => self.tools.get(id).map(|t| {
                    let at = t.locations.first().map(|l| format!(" ({})", self.rel(&l.path))).unwrap_or_default();
                    review::Entry::new(review::Role::Tool, format!("{}{at}", t.title))
                }),
                _ => None,
            })
            .collect()
    }

    /// The agent's plan (its To-do), one line per step; None without one.
    pub fn plan_text(&self) -> Option<String> {
        (!self.plan.is_empty()).then(|| {
            self.plan
                .iter()
                .map(|e| {
                    let mark = match e.status {
                        PlanStatus::Completed => "[x]",
                        PlanStatus::InProgress => "[~]",
                        PlanStatus::Pending => "[ ]",
                    };
                    format!("- {mark} {}", e.content.trim())
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// A reviewer's findings: what it said since the last message it was
    /// sent. None while it's working or before it has said anything.
    pub fn findings(&self) -> Option<String> {
        if matches!(self.status, Status::Working | Status::NeedsYou | Status::Starting) {
            return None;
        }
        let from = self.chat.iter().rposition(|c| matches!(c, ChatItem::User(_))).map_or(0, |i| i + 1);
        let text = self.chat[from..]
            .iter()
            .filter_map(|c| match c {
                ChatItem::Agent(t) => Some(t.trim()),
                _ => None,
            })
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        (!text.is_empty()).then_some(text)
    }

    /// Review Agent's Work… / Plan…: start a reviewer (replacing an earlier
    /// one) and send it the task's diff, chat (and plan) once its session is open.
    pub fn start_review(&mut self, def: AgentDef, model: Option<(String, String)>, subject: Subject, instruction: String, window: &mut Window, cx: &mut Context<Self>) {
        self.close_review(cx);
        let (model, model_name) = model.unzip();
        let link = ReviewLink { agent: def.id.clone(), model, model_name, subject: subject.as_str().into(), session_id: None, created: qur_agents::now_secs() };
        self.record.review = Some(link.clone());
        let r = self.attach_reviewer(&link, def, window, cx);
        let thread = review::serialize_thread(&self.thread_entries(), review::THREAD_CAP);
        let plan = if subject == Subject::Plan { self.plan_text() } else { None };
        let (wt, base) = (self.record.worktree.clone(), self.record.base.clone());
        println!("REVIEW {} start {} subject={} thread={}B", self.record.id, r.read(cx).record.title, subject.as_str(), thread.len());
        self.open_review = true;
        self.review_unread = false;
        cx.emit(AgentEvent::Review);
        cx.notify();
        let weak = r.downgrade();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |_, cx| {
            let files = {
                let (wt, base) = (wt.clone(), base.clone());
                bg.spawn(async move { qur_git::task_diff(&wt, &base) }).await
            };
            let (diff, summary) = match &files {
                Ok(f) => {
                    let (a, d) = f.iter().fold((0, 0), |(a, d), f| (a + f.added, d + f.removed));
                    (review::diff_text(f, review::DIFF_CAP), format!("diff: {} file(s) +{a} −{d}", f.len()))
                }
                Err(e) => (format!("(Qur couldn't compute the diff: {e})"), "no diff".to_string()),
            };
            let prompt = review::review_prompt(subject, &thread, &base, &diff, plan.as_deref(), &instruction);
            let mut text = match subject {
                Subject::Work => "Review the agent's work.".to_string(),
                Subject::Plan => "Review the agent's plan.".to_string(),
            };
            if !instruction.trim().is_empty() {
                text = format!("{text} {}", instruction.trim());
            }
            let labels = vec![summary, format!("chat: {} KB", thread.len().div_ceil(1024))];
            println!("REVIEW prompt {} bytes ({})", prompt.len(), labels.join(", "));
            weak.update(cx, |r, cx| r.send_out(Outgoing { text, mentions: Vec::new(), context: vec![prompt], labels, images: Vec::new() }, cx)).ok();
        })
        .detach();
    }

    /// Make the reviewer for `link` (a new review, or one saved in tasks.toml,
    /// which stays stopped until you send it something).
    pub fn attach_reviewer(&mut self, link: &ReviewLink, def: AgentDef, window: &mut Window, cx: &mut Context<Self>) -> Entity<AgentTask> {
        let short = review::short_name(&def, link.model_name.as_deref());
        let role = ReviewRole { subject: Subject::parse(&link.subject), model: link.model.clone(), short: short.clone() };
        let mut record = self.record.clone();
        record.id = format!("{}~review", self.record.id);
        record.agent = def.id.clone();
        record.title = format!("review · {short}");
        record.renamed = true;
        record.session_id = link.session_id.clone();
        record.queued.clear();
        record.turns.clear();
        record.review = None;
        let r = cx.new(|cx| AgentTask::new_reviewer(self, record, def, role, window, cx));
        let sub = cx.subscribe(&r, |this, r, ev: &AgentEvent, cx| match ev {
            AgentEvent::Changed => {
                let (sid, status, bytes) = {
                    let r = r.read(cx);
                    (r.record.session_id.clone(), r.status.clone(), r.findings().map_or(0, |f| f.len()))
                };
                if let Some(link) = &mut this.record.review {
                    link.session_id = sid;
                }
                if status == Status::Done {
                    println!("REVIEW {} done: findings {bytes}B", this.record.id);
                    if !this.open_review {
                        this.review_unread = true;
                    }
                }
                cx.emit(AgentEvent::Review);
                cx.notify();
            }
            AgentEvent::OpenFile(p, line) => cx.emit(AgentEvent::OpenFile(p.clone(), *line)),
            AgentEvent::RunInTerminal { label, command } => cx.emit(AgentEvent::RunInTerminal { label: label.clone(), command: command.clone() }),
            _ => {}
        });
        let obs = cx.observe(&r, |_, _, cx| cx.notify());
        self._review_subs = vec![sub, obs];
        self.reviewer = Some(r.clone());
        r
    }

    /// Stop the reviewer and forget it (its row and its link in tasks.toml).
    pub fn close_review(&mut self, cx: &mut Context<Self>) {
        self._review_subs.clear();
        if let Some(r) = self.reviewer.take() {
            // Shuts down on a thread of its own; nothing to wait for.
            drop(r.update(cx, |r, cx| r.shutdown(cx)));
        }
        self.open_review = false;
        self.review_unread = false;
        if self.record.review.take().is_some() {
            cx.emit(AgentEvent::Review);
        }
        cx.notify();
    }

    /// Show the reviewer in place of the chat (its sidebar row), or the chat.
    pub fn show_review(&mut self, on: bool, cx: &mut Context<Self>) {
        self.open_review = on && self.reviewer.is_some();
        if self.open_review {
            self.open_subagent = None;
            self.review_unread = false;
            // A reviewer saved from last time: start it so its chat loads
            // (session/load). Nothing is sent.
            if let Some(r) = &self.reviewer {
                r.update(cx, |r, cx| {
                    if r.status == Status::Stopped && r.record.session_id.is_some() {
                        r.start(cx);
                    }
                });
            }
        }
        cx.notify();
    }

    /// The reviewer has findings to hand over (Send Findings to Agent).
    pub fn has_findings(&self, cx: &App) -> bool {
        self.reviewer.as_ref().is_some_and(|r| r.read(cx).findings().is_some())
    }

    /// Send Findings to Agent: the handoff prompt goes into this agent's
    /// prompt, to edit and send (queued if the agent is busy). Before
    /// anything you'd typed.
    pub fn send_findings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(r) = self.reviewer.clone() else { return false };
        let (findings, role) = {
            let r = r.read(cx);
            (r.findings(), r.review.clone())
        };
        let (Some(findings), Some(role)) = (findings, role) else { return false };
        let text = review::handoff_prompt(role.subject, &role.short, &findings);
        println!("REVIEW {} handoff {}B into the prompt", self.record.id, text.len());
        self.prompt.update(cx, |p, cx| {
            let typed = p.text.trim().to_string();
            let text = if typed.is_empty() { text } else { format!("{text}\n\n{typed}") };
            p.set_text(&text, cx);
        });
        self.open_review = false;
        self.follow_bottom();
        let f = self.prompt.read(cx).focus.clone();
        window.focus(&f, cx);
        cx.notify();
        true
    }

    /// The reviewer in the chat panel: a bar (back, its name and state,
    /// Send Findings, Stop or Close), the read-only note, then its chat.
    fn review_view(&self, r: &Entity<AgentTask>, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let (title, status, findings) = {
            let r = r.read(cx);
            (r.record.title.clone(), r.status.clone(), r.findings().is_some())
        };
        let working = matches!(status, Status::Working | Status::NeedsYou | Status::Starting);
        let button = |label: &'static str, key: &'static str| {
            div().id(key).flex_none().px(look.z(8.)).py(look.z(2.)).rounded(look.z(4.)).cursor_pointer().bg(look.c(ui.button_bg)).text_color(look.c(ui.button_text)).child(label)
        };
        let mut tint = look.c(self.accent);
        tint.a = 0.22;
        let reviewer = r.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px(look.z(16.))
                    .py(look.z(8.))
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .border_b_1()
                    .border_color(look.c(ui.border))
                    .child(button("← Chat", "review-back").on_click(cx.listener(|t, _, _, cx| t.show_review(false, cx))))
                    .child(div().flex_1().min_w(px(0.)).truncate().font_weight(gpui::FontWeight::MEDIUM).child(format!("↳ {title}")))
                    .child(div().flex_none().text_color(look.c(if matches!(status, Status::Error(_)) { ui.error } else { ui.text_muted })).child(status.label()))
                    .when(findings && !working, |d| {
                        d.child(
                            div()
                                .id("review-send")
                                .flex_none()
                                .px(look.z(8.))
                                .py(look.z(2.))
                                .rounded(look.z(4.))
                                .cursor_pointer()
                                .border_1()
                                .border_color(look.c(self.accent))
                                .bg(tint)
                                .text_color(look.c(ui.text))
                                .child("Send Findings to Agent")
                                .on_click(cx.listener(|t, _, window, cx| {
                                    t.send_findings(window, cx);
                                })),
                        )
                    })
                    .when(working, |d| d.child(button("Stop", "review-stop").on_click(move |_, _, cx| reviewer.update(cx, |r, cx| r.stop(cx)))))
                    .when(!working, |d| d.child(button("Close", "review-close").on_click(cx.listener(|t, _, _, cx| t.close_review(cx))))),
            )
            .child(
                div()
                    .flex_none()
                    .px(look.z(16.))
                    .py(look.z(4.))
                    .text_size(look.text(-2.))
                    .text_color(look.c(ui.text_muted))
                    .child("Read-only review: Qur denies every edit or command it asks to run, and starts it in its plan or read-only mode. This is Qur's own rule, not an OS sandbox."),
            )
            .child(div().flex_1().min_h(px(0.)).child(r.clone()))
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
            .bg(look.bg(ui.editor_bg))
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
    fn tail_lines_keeps_the_end() {
        assert_eq!(tail_lines("1\n2\n3", 5), "1\n2\n3");
        assert_eq!(tail_lines("1\n2\n3\n4", 2), "… 2 earlier lines\n3\n4");
    }
}
