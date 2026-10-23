//! One ACP agent task: its process (via `qur_acp`), chat transcript, plan,
//! pending permission and status, plus the stage view that shows it:
//! header, To-do strip, Chat, permission bar and prompt bar.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, ListAlignment, ListState, SharedString,
    Subscription, Task, Window, div, list, prelude::*, px,
};
use qur_acp::{
    AgentCommand, Client, ConfigOption, Event, PermissionKind, PermissionOption, PermissionOutcome, PlanEntry, PlanStatus,
    PromptBlock, RequestId, SessionUpdate, StopReason, ToolCall, ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
};
use qur_agents::{AgentDef, QueuedMessage, TaskRecord};
use qur_theme::Rgba;

use crate::browser_view::{BrowserPane, BrowserPaneEvent, base64};
use crate::commands::agent as aa;
use crate::changes_view::{ChangesEvent, ChangesView};
use crate::dock::{self, Docks, Kind, Panel};
use crate::live::{LiveEvent, LivePane};
use crate::look::Look;
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

pub enum AgentEvent {
    /// Status, title or session changed (sidebar, task store).
    Changed,
    /// The user clicked a file in a tool call: open it in the code modal.
    OpenFile(PathBuf, Option<u32>),
    /// `@` typed in the prompt: open the mention picker for this worktree.
    MentionRequested,
    /// `/` typed: offer this agent's slash commands.
    SlashRequested,
    /// Changes view: merge this task's branch into the user's branch.
    Merge,
    /// Changes view: discard the task.
    Discard,
    /// A turn ended after changing files: run the project's checks.
    TurnChangedFiles,
    /// A setting chip (mode, model…) was clicked: offer its values.
    PickSetting(String),
}

#[derive(Clone)]
enum ChatItem {
    User(String),
    Agent(String),
    Thought(String),
    Tool(String),
    Notice(String),
    /// Command output (worktree setup), shown in the code font.
    Output(String),
}

struct Permission {
    id: RequestId,
    tool: ToolCallUpdate,
    options: Vec<PermissionOption>,
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
    session_id: Option<String>,
    load_session: bool,
    chat: Vec<ChatItem>,
    tools: HashMap<String, ToolCall>,
    list: ListState,
    plan: Vec<PlanEntry>,
    permission: Option<Permission>,
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
    /// Setup command still running for a new worktree.
    pub setting_up: bool,
    focus: FocusHandle,
    turn_started: Option<Instant>,
    _events: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<AgentEvent> for AgentTask {}

impl AgentTask {
    #[allow(clippy::too_many_arguments)]
    pub fn new(root: PathBuf, record: TaskRecord, def: AgentDef, accent: Rgba, layout: Layout, wrap: bool, tab: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        let wt = record.worktree.clone();
        let monitor = cx.new(|cx| TaskMonitor::new(record.worktree.clone(), record.base.clone(), accent, cx));
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
        let prompt = cx.new(|cx| PromptInput::new(format!("Ask {}…  (@ to mention a file)", def.label), wt, cx));
        let sub = cx.subscribe_in(&prompt, window, |this, _, ev: &PromptEvent, _, cx| match ev {
            PromptEvent::Submit => this.submit(cx),
            PromptEvent::Cancel => this.stop(cx),
            PromptEvent::MentionRequested => cx.emit(AgentEvent::MentionRequested),
            PromptEvent::SlashRequested => cx.emit(AgentEvent::SlashRequested),
            PromptEvent::Changed => {}
        });
        let list = ListState::new(0, ListAlignment::Bottom, px(400.));
        list.set_follow_mode(gpui::FollowMode::Tail);
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
            chat: Vec::new(),
            tools: HashMap::new(),
            list,
            plan: Vec::new(),
            permission: None,
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
            setting_up: false,
            focus: cx.focus_handle(),
            turn_started: None,
            _events: None,
            _subs: vec![sub, live_sub, monitor_sub, browser_sub, files_sub, changes_sub],
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
    }

    fn refresh_tool(&mut self, id: &str) {
        if let Some(ix) = self.chat.iter().rposition(|c| matches!(c, ChatItem::Tool(t) if t == id)) {
            self.list.splice(ix..ix + 1, 1);
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
        let saved = self.record.session_id.clone();
        let cwd = self.record.worktree.clone();
        cx.spawn(async move |this, cx| {
            let init = client.initialize().await;
            let init = match init {
                Ok(Ok(i)) => i,
                Ok(Err(e)) => return this.update(cx, |t, cx| t.fail(format!("initialize: {e}"), cx)).ok(),
                Err(_) => return this.update(cx, |t, cx| t.fail("agent exited during start".into(), cx)).ok(),
            };
            let can_load = init.load_session;
            let images = init.raw["agentCapabilities"]["promptCapabilities"]["image"].as_bool().unwrap_or(false);
            this.update(cx, |t, _| {
                t.load_session = can_load;
                t.images_ok = images;
            })
            .ok();
            let session = match saved.filter(|_| can_load) {
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
                    Ok(Err(e)) => return this.update(cx, |t, cx| t.fail(format!("session/new: {e}"), cx)).ok(),
                    Err(_) => return this.update(cx, |t, cx| t.fail("agent exited during start".into(), cx)).ok(),
                },
            };
            this.update(cx, |t, cx| {
                t.settings = settings;
                t.session_id = Some(session.clone());
                t.record.session_id = Some(session);
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

    fn fail(&mut self, msg: String, cx: &mut Context<Self>) {
        self.push(ChatItem::Notice(msg.clone()));
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
        self.monitor.update(cx, |m, _| m.set_turn_active(active));
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
        if matches!(self.status, Status::Working | Status::NeedsYou | Status::Starting) || self.setting_up {
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
        if !self.pending.is_empty() && matches!(self.status, Status::Done | Status::Idle) {
            let out = self.pending.remove(0);
            self.save_queue(cx);
            self.send_out(out, cx);
        }
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
            _ => None,
        })
    }

    /// Send a prompt (starting the agent first when needed).
    pub fn send(&mut self, text: String, mentions: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.send_out(Outgoing::plain(text, mentions), cx)
    }

    fn send_out(&mut self, out: Outgoing, cx: &mut Context<Self>) {
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
        self.turn_started = Some(Instant::now());
        self.set_status(Status::Working, cx);
        let rx = client.prompt(&session, blocks);
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            this.update(cx, |t, cx| {
                t.permission = None;
                match r {
                    Ok(Ok(StopReason::Cancelled)) => {
                        t.push(ChatItem::Notice("Stopped.".into()));
                        t.set_status(Status::Done, cx)
                    }
                    Ok(Ok(StopReason::EndTurn)) => t.set_status(Status::Done, cx),
                    Ok(Ok(other)) => {
                        t.push(ChatItem::Notice(format!("Turn ended: {other:?}")));
                        t.set_status(Status::Done, cx)
                    }
                    Ok(Err(e)) => {
                        t.push(ChatItem::Notice(format!("Error: {e}")));
                        t.set_status(Status::Error(e.to_string()), cx)
                    }
                    Err(_) => t.set_status(Status::Error("agent exited".into()), cx),
                }
                t.drain_pending(cx);
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
            if let Some(p) = self.permission.take() {
                c.respond_permission(p.id, PermissionOutcome::Cancelled);
            }
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
        self.permission = None;
        self._events = None;
        if !matches!(self.status, Status::Error(_)) {
            self.set_status(Status::Stopped, cx);
        }
        handle
    }

    pub fn answer_permission(&mut self, kind: PermissionKind, cx: &mut Context<Self>) {
        let Some(p) = self.permission.take() else { return };
        let paths: Vec<PathBuf> = p.tool.locations.iter().flatten().map(|l| l.path.clone()).collect();
        self.monitor.update(cx, |m, cx| m.on_permission(&paths, false, cx));
        let outcome = match p.options.iter().find(|o| o.kind == kind) {
            Some(o) => PermissionOutcome::Selected(o.id.clone()),
            None => PermissionOutcome::Cancelled,
        };
        if let Some(c) = &self.client {
            c.respond_permission(p.id, outcome);
        }
        self.set_status(Status::Working, cx);
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
        let Some(o) = self.settings.iter().find(|o| o.is_mode()) else { return };
        if let Some(next) = o.next_value().map(str::to_string) {
            let id = o.id.clone();
            self.set_setting(&id, &next, cx);
        }
    }

    pub fn has_permission(&self) -> bool {
        self.permission.is_some()
    }

    fn on_event(&mut self, ev: Event, cx: &mut Context<Self>) {
        match ev {
            Event::Update { update, .. } => match update {
                SessionUpdate::AgentMessage(c) => self.append(false, &c.text),
                SessionUpdate::AgentThought(c) => self.append(true, &c.text),
                SessionUpdate::UserMessage(c) => {
                    // Replayed history (session/load).
                    match self.chat.last_mut() {
                        Some(ChatItem::User(s)) if self.status == Status::Starting => s.push_str(&c.text),
                        _ => self.push(ChatItem::User(c.text)),
                    }
                }
                SessionUpdate::ToolCall(t) => {
                    let id = t.id.clone();
                    let paths: Vec<PathBuf> = t.locations.iter().map(|l| l.path.clone()).collect();
                    let (k, st) = (heat_kind(t.kind), heat_status(t.status));
                    let lines: Vec<(PathBuf, u32)> = t.locations.iter().filter_map(|l| Some((l.path.clone(), l.line?))).collect();
                    self.monitor.update(cx, |m, cx| {
                        m.on_tool(&id, Some(k), Some(st), &paths, cx);
                        for (p, l) in &lines {
                            m.tool_line(&id, p, *l);
                        }
                    });
                    self.tools.insert(id.clone(), t);
                    self.push(ChatItem::Tool(id));
                }
                SessionUpdate::ToolCallUpdate(u) => {
                    let id = u.id.clone();
                    let paths: Vec<PathBuf> = u.locations.iter().flatten().map(|l| l.path.clone()).collect();
                    let (k, st) = (u.kind.map(heat_kind), u.status.map(heat_status));
                    let lines: Vec<(PathBuf, u32)> = u.locations.iter().flatten().filter_map(|l| Some((l.path.clone(), l.line?))).collect();
                    self.monitor.update(cx, |m, cx| {
                        m.on_tool(&id, k, st, &paths, cx);
                        for (p, l) in &lines {
                            m.tool_line(&id, p, *l);
                        }
                    });
                    match self.tools.get_mut(&id) {
                        Some(t) => t.apply(&u),
                        None => {
                            let t = u.to_tool_call();
                            self.tools.insert(id.clone(), t);
                            self.push(ChatItem::Tool(id.clone()));
                        }
                    }
                    self.refresh_tool(&id);
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
                _ => {}
            },
            Event::Permission { id, tool_call, options, .. } => {
                let paths: Vec<PathBuf> = tool_call.locations.iter().flatten().map(|l| l.path.clone()).collect();
                self.monitor.update(cx, |m, cx| m.on_permission(&paths, true, cx));
                self.permission = Some(Permission { id, tool: tool_call, options });
                self.set_status(Status::NeedsYou, cx);
            }
            Event::Exited { code, stderr_tail } => {
                self.client = None;
                self.session_id = None;
                self.permission = None;
                if !matches!(self.status, Status::Stopped) {
                    let tail = stderr_tail.lines().last().unwrap_or("").to_string();
                    let msg = format!("agent exited ({}){}", code.map(|c| c.to_string()).unwrap_or("signal".into()), if tail.is_empty() { String::new() } else { format!(": {tail}") });
                    self.push(ChatItem::Notice(msg.clone()));
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
        match &self.chat[ix] {
            ChatItem::User(t) => pad(div())
                .child(
                    div()
                        .px(look.z(10.))
                        .py(look.z(6.))
                        .rounded(look.z(6.))
                        .bg(look.c(ui.active_bg))
                        .text_color(look.c(ui.text))
                        .child(t.clone()),
                )
                .into_any_element(),
            ChatItem::Agent(t) => pad(div()).child(markdownish(t, look)).into_any_element(),
            // Open while it streams (the last item of a working turn), then
            // folded to its first line; click to toggle (as in Zed).
            ChatItem::Thought(t) => {
                let streaming = ix + 1 == self.chat.len() && matches!(self.status, Status::Working);
                let open = streaming != self.toggled.contains(&format!("thought-{ix}"));
                let t = t.trim();
                let first = t.lines().next().unwrap_or("").to_string();
                pad(div())
                    .text_size(look.text(-1.))
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
                    .when(open, |d| d.child(div().pl(look.z(14.)).pt(look.z(2.)).italic().child(t.to_string())))
                    .into_any_element()
            }
            ChatItem::Notice(t) => pad(div()).text_size(look.text(-1.)).text_color(look.c(ui.text_muted)).child(t.clone()).into_any_element(),
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
                        .text_size(look.text(-2.))
                        .text_color(look.c(ui.text_secondary))
                        .child(t.clone()),
                )
                .into_any_element(),
            ChatItem::Tool(id) => {
                let Some(t) = self.tools.get(id) else { return div().into_any_element() };
                let (glyph, color) = match t.status {
                    ToolStatus::Pending | ToolStatus::InProgress => ("…", ui.text_muted),
                    ToolStatus::Completed => ("✓", ui.success),
                    ToolStatus::Failed => ("✕", ui.error),
                };
                let kind = match t.kind {
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
                };
                // Real line counts (every line of old and new counted before).
                let diffs: Vec<(&PathBuf, Vec<(char, &str)>)> = t
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ToolContent::Diff { path, old_text, new_text } => Some((path, line_diff(old_text.as_deref().unwrap_or(""), new_text))),
                        _ => None,
                    })
                    .collect();
                let add = diffs.iter().flat_map(|(_, d)| d).filter(|(k, _)| *k == '+').count();
                let del = diffs.iter().flat_map(|(_, d)| d).filter(|(k, _)| *k == '-').count();
                let loc = t.locations.first().cloned();
                let title = if t.title.is_empty() { kind.to_string() } else { t.title.clone() };
                let body = self.tool_body(t, &diffs, look, cx);
                // Edits and failures start open; the rest folded (click toggles).
                let default_open = matches!(t.kind, ToolKind::Edit | ToolKind::Delete | ToolKind::Move) || t.status == ToolStatus::Failed;
                let open = body.is_some() && default_open != self.toggled.contains(id);
                let expandable = body.is_some();
                let tid = id.clone();
                pad(div())
                    .py(look.z(2.))
                    .child(
                        div()
                            .id(SharedString::from(format!("tool-{id}")))
                            .flex()
                            .items_center()
                            .gap(look.z(8.))
                            .text_size(look.text(-1.))
                            .text_color(look.c(ui.text_secondary))
                            .when(expandable || loc.is_some(), |d| d.cursor_pointer().hover(|d| d.text_color(look.c(ui.text))))
                            .child(div().w(look.z(40.)).flex_none().text_color(look.c(ui.text_muted)).child(kind))
                            .child(div().min_w(px(0.)).truncate().child(title))
                            .when(add + del > 0, |d| {
                                d.child(div().flex_none().text_color(look.c(look.theme.diff.added_text)).child(format!("+{add}")))
                                    .child(div().flex_none().text_color(look.c(look.theme.diff.removed_text)).child(format!("−{del}")))
                            })
                            .when_some(t.exit.flatten().filter(|c| *c != 0), |d, c| d.child(div().flex_none().text_color(look.c(ui.error)).child(format!("exit {c}"))))
                            .child(div().flex_none().text_color(look.c(color)).child(glyph))
                            .when(expandable, |d| d.child(div().flex_none().text_color(look.c(ui.text_muted)).child(if open { "▾" } else { "▸" })))
                            .on_click(cx.listener(move |t, _, _, cx| {
                                if expandable {
                                    t.toggle(tid.clone(), ix, cx);
                                } else if let Some(l) = &loc {
                                    cx.emit(AgentEvent::OpenFile(l.path.clone(), l.line));
                                }
                            })),
                    )
                    .when_some(body.filter(|_| open), |d, b| d.child(div().pl(look.z(48.)).pt(look.z(4.)).child(b)))
                    .into_any_element()
            }
        }
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

    /// What an unfolded tool call shows: its text (code blocks in the code
    /// font), each file's diff, a command's output and exit, or else its raw
    /// input/output. None when there's nothing to show.
    fn tool_body(&self, t: &ToolCall, diffs: &[(&PathBuf, Vec<(char, &str)>)], look: &Look, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let ui = &look.theme.ui;
        let mut parts: Vec<gpui::AnyElement> = Vec::new();
        for c in &t.content {
            match c {
                ToolContent::Text(s) if !s.trim().is_empty() => parts.push(markdownish(s, look).into_any_element()),
                ToolContent::Terminal(_) => {
                    let out = t.output.as_deref().unwrap_or("").trim_end();
                    let status = match t.exit {
                        None => "running…".to_string(),
                        Some(Some(0)) => "exit 0".into(),
                        Some(Some(c)) => format!("exit {c}"),
                        Some(None) => "ended".into(),
                    };
                    let failed = matches!(t.exit, Some(Some(c)) if c != 0);
                    let block = code_block(&tail_lines(out, MAX_TOOL_LINES), look).when(out.is_empty(), |d| d.child("(no output)"));
                    parts.push(
                        div()
                            .flex()
                            .flex_col()
                            .gap(look.z(2.))
                            .child(block)
                            .child(div().text_size(look.text(-2.)).text_color(look.c(if failed { ui.error } else { ui.text_muted })).child(status))
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
                            .text_size(look.text(-2.))
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
                            .text_size(look.text(-2.))
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
                .text_size(look.text(-1.))
                .child(div().text_size(look.text(-2.)).font_weight(gpui::FontWeight::MEDIUM).text_color(look.c(ui.text_muted)).child(format!("TO-DO  {done}/{}", self.plan.len())))
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
        let p = self.permission.as_ref()?;
        let ui = &look.theme.ui;
        let what = p.tool.title.clone().unwrap_or_else(|| "use a tool".into());
        let file = p.tool.locations.as_ref().and_then(|l| l.first()).map(|l| self.rel(&l.path));
        let button = |label: &'static str, kind: PermissionKind, primary: bool, id: &'static str| {
            div()
                .id(id)
                .px(look.z(10.))
                .py(look.z(3.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_size(look.text(-1.))
                .bg(look.c(if primary { ui.button_primary_bg } else { ui.button_bg }))
                .text_color(look.c(if primary { ui.button_primary_text } else { ui.button_text }))
                .child(label)
                .on_click(cx.listener(move |t, _, _, cx| t.answer_permission(kind, cx)))
        };
        let has = |k: PermissionKind| p.options.iter().any(|o| o.kind == k);
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
                // A long command or edit as the title scrolls here instead of
                // growing the bar over the chat (which then couldn't scroll).
                .child(
                    div()
                        .id("perm-what")
                        .min_w(px(0.))
                        .max_h(look.z(140.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .child(div().text_color(look.c(ui.text)).child(format!("{} wants to {what}", self.def.label)))
                        .children(file.map(|f| div().truncate().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(f))),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(look.z(6.))
                        .when(has(PermissionKind::AllowOnce), |d| d.child(button("Allow once", PermissionKind::AllowOnce, true, "perm-once")))
                        .when(has(PermissionKind::AllowAlways), |d| d.child(button("Allow for this task", PermissionKind::AllowAlways, false, "perm-always")))
                        .when(has(PermissionKind::RejectOnce), |d| d.child(button("Deny", PermissionKind::RejectOnce, false, "perm-deny"))),
                ),
        )
    }
}

/// Lines an unfolded tool call shows of a diff, output or raw input.
const MAX_TOOL_LINES: usize = 200;

/// The last `max` lines of `text`, with a note of how many were left out.
fn tail_lines(text: &str, max: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max {
        return text.to_string();
    }
    format!("… {} earlier lines\n{}", lines.len() - max, lines[lines.len() - max..].join("\n"))
}

fn code_block(text: &str, look: &Look) -> gpui::Div {
    let ui = &look.theme.ui;
    div()
        .px(look.z(10.))
        .py(look.z(6.))
        .rounded(look.z(4.))
        .bg(look.c(ui.panel_bg))
        .border_1()
        .border_color(look.c(ui.border))
        .font_family(look.code_font.clone())
        .text_size(look.text(-2.))
        .text_color(look.c(ui.text_secondary))
        .when(!text.is_empty(), |d| d.child(text.to_string()))
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

/// Plain text with fenced code blocks in the code font (enough for agent
/// replies; real markdown rendering can come later).
fn markdownish(text: &str, look: &Look) -> gpui::Div {
    let ui = &look.theme.ui;
    let mut out = div().flex().flex_col().gap(look.z(6.)).text_color(look.c(ui.text));
    let mut in_code = false;
    let mut buf = String::new();
    let flush = |out: gpui::Div, buf: &mut String, code: bool| -> gpui::Div {
        let t = std::mem::take(buf);
        let t = t.trim_matches('\n').to_string();
        if t.is_empty() {
            return out;
        }
        if code {
            out.child(
                div()
                    .px(look.z(10.))
                    .py(look.z(6.))
                    .rounded(look.z(4.))
                    .bg(look.c(ui.panel_bg))
                    .border_1()
                    .border_color(look.c(ui.border))
                    .font_family(look.code_font.clone())
                    .text_size(look.text(-1.))
                    .child(t),
            )
        } else {
            out.child(div().child(t))
        }
    };
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            out = flush(out, &mut buf, in_code);
            in_code = !in_code;
            continue;
        }
        buf.push_str(line);
    }
    flush(out, &mut buf, in_code)
}

impl Focusable for AgentTask {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AgentTask {
    /// To-do + Chat + permission bar + prompt (the "side notes").
    fn notes(&self, show_plan: bool, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = look.theme.ui.clone();
        let me = cx.entity();
        let look2 = look.clone();
        let chat = list(self.list.clone(), move |ix, _, cx| me.update(cx, |t, cx| t.render_item(ix, &look2, cx)))
            .flex_1()
            .min_h(px(0.));
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
        // Just above the field, outside it (inside, they squeezed it): the
        // session's settings on the left (click to choose; Shift+Tab cycles
        // the mode), queue and Esc hints on the right while working.
        let chips: Vec<_> = self
            .settings
            .iter()
            .map(|o| {
                let id = o.id.clone();
                div()
                    .id(SharedString::from(format!("setting-{}", o.id)))
                    .flex_none()
                    .px(look.z(6.))
                    .py(look.z(1.))
                    .rounded(look.z(4.))
                    .cursor_pointer()
                    .text_color(look.c(if o.is_mode() && o.current != o.values.first().map(|v| v.value.clone()).unwrap_or_default() { self.accent } else { ui.text_secondary }))
                    .hover(|d| d.bg(look.c(ui.hover_bg)))
                    // Mode and model read on their own; others need their name
                    // ("Effort: high", "Fast: off").
                    .child(match o.category.as_deref() {
                        Some("mode" | "model") => format!("{} ▾", o.current_name()),
                        _ => format!("{}: {} ▾", o.name, o.current_name()),
                    })
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(AgentEvent::PickSetting(id.clone()))))
            })
            .collect();
        let busy = working || !self.pending.is_empty();
        let hints = (busy || !chips.is_empty()).then(|| {
            div()
                .flex_none()
                .mx(look.z(12.))
                .mb(look.z(3.))
                .flex()
                .items_center()
                .gap(look.z(2.))
                .text_size(look.text(-3.))
                .text_color(look.c(ui.text_muted))
                .children(chips)
                .child(div().flex_1())
                .when(!self.pending.is_empty(), |d| d.child(div().flex_none().mr(look.z(6.)).text_color(look.c(ui.text_secondary)).child(format!("{} queued", self.pending.len()))))
                .when(working, |d| d.child(div().min_w(px(0.)).mr(look.z(2.)).truncate().child("esc to stop")))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(show_plan, |d| d.children(self.plan_strip(look)))
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
            .children(self.permission_bar(look, cx))
            .children(hints)
            .child(prompt_bar)
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
        self.changes.update(cx, |c, cx| c.refresh(cx));
        cx.notify();
    }
}

impl Render for AgentTask {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
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
    }
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
    fn tail_lines_keeps_the_end() {
        assert_eq!(tail_lines("1\n2\n3", 5), "1\n2\n3");
        assert_eq!(tail_lines("1\n2\n3\n4", 2), "… 2 earlier lines\n3\n4");
    }
}
