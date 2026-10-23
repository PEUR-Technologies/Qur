//! One ACP agent task: its process (via `qur_acp`), chat transcript, plan,
//! pending permission and status, plus the stage view that shows it:
//! header, To-do strip, Chat, permission bar and prompt bar.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, ListAlignment, ListState, SharedString,
    Subscription, Task, Window, div, list, prelude::*, px,
};
use qur_acp::{
    AgentCommand, Client, Event, PermissionKind, PermissionOption, PermissionOutcome, PlanEntry, PlanStatus,
    PromptBlock, RequestId, SessionUpdate, StopReason, ToolCall, ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
};
use qur_agents::{AgentDef, TaskRecord};
use qur_theme::Rgba;

use crate::look::Look;
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

pub struct AgentTask {
    pub record: TaskRecord,
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
    queued: Option<(String, Vec<PathBuf>)>,
    /// Messages sent during a turn, sent in order when it ends (decision 22).
    pending: Vec<(String, Vec<PathBuf>)>,
    /// Slash commands the agent advertised.
    pub commands: Vec<qur_acp::AvailableCommand>,
    /// Setup command still running for a new worktree.
    pub setting_up: bool,
    focus: FocusHandle,
    turn_started: Option<Instant>,
    _events: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<AgentEvent> for AgentTask {}

impl AgentTask {
    pub fn new(record: TaskRecord, def: AgentDef, accent: Rgba, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let wt = record.worktree.clone();
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
        AgentTask {
            record,
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
            pending: Vec::new(),
            commands: Vec::new(),
            setting_up: false,
            focus: cx.focus_handle(),
            turn_started: None,
            _events: None,
            _subs: vec![sub],
        }
    }

    // -- chat list -------------------------------------------------------------

    fn push(&mut self, item: ChatItem) {
        let n = self.chat.len();
        self.chat.push(item);
        self.list.splice(n..n, 1);
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
            this.update(cx, |t, _| t.load_session = can_load).ok();
            let session = match saved.filter(|_| can_load) {
                Some(id) => match client.load_session(&id, &cwd).await {
                    Ok(Ok(())) => Some(id),
                    _ => None,
                },
                None => None,
            };
            let session = match session {
                Some(s) => s,
                None => match client.new_session(&cwd).await {
                    Ok(Ok(id)) => id,
                    Ok(Err(e)) => return this.update(cx, |t, cx| t.fail(format!("session/new: {e}"), cx)).ok(),
                    Err(_) => return this.update(cx, |t, cx| t.fail("agent exited during start".into(), cx)).ok(),
                },
            };
            this.update(cx, |t, cx| {
                t.session_id = Some(session.clone());
                t.record.session_id = Some(session);
                t.set_status(Status::Idle, cx);
                if let Some((text, mentions)) = t.queued.take() {
                    t.send(text, mentions, cx);
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
        if self.status != s {
            self.status = s;
            cx.emit(AgentEvent::Changed);
        }
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let (text, mentions) = self.prompt.update(cx, |p, cx| p.take(cx));
        if text.trim().is_empty() {
            return;
        }
        if matches!(self.status, Status::Working | Status::NeedsYou | Status::Starting) || self.setting_up {
            // One turn at a time: queue it and send when the turn ends.
            self.pending.push((text, mentions));
            cx.notify();
            return;
        }
        self.send(text, mentions, cx);
    }

    /// Send the next queued message, if any (turn ended).
    fn drain_pending(&mut self, cx: &mut Context<Self>) {
        if !self.pending.is_empty() && matches!(self.status, Status::Done | Status::Idle) {
            let (text, mentions) = self.pending.remove(0);
            self.send(text, mentions, cx);
        }
    }

    pub fn output(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.push(ChatItem::Output(text.into()));
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
            ChatItem::Agent(t) => t.lines().map(str::trim).rfind(|l| !l.is_empty()).map(str::to_string),
            ChatItem::Notice(t) => Some(t.clone()),
            _ => None,
        })
    }

    /// Send a prompt (starting the agent first when needed).
    pub fn send(&mut self, text: String, mentions: Vec<PathBuf>, cx: &mut Context<Self>) {
        let (Some(client), Some(session)) = (self.client.clone(), self.session_id.clone()) else {
            self.queued = Some((text.clone(), mentions));
            self.push(ChatItem::User(text));
            self.start(cx);
            return;
        };
        if self.queued.is_none() && !matches!(self.chat.last(), Some(ChatItem::User(t)) if *t == text) {
            self.push(ChatItem::User(text.clone()));
        }
        let mut blocks = vec![PromptBlock::Text(text)];
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
        let outcome = match p.options.iter().find(|o| o.kind == kind) {
            Some(o) => PermissionOutcome::Selected(o.id.clone()),
            None => PermissionOutcome::Cancelled,
        };
        if let Some(c) = &self.client {
            c.respond_permission(p.id, outcome);
        }
        self.set_status(Status::Working, cx);
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
                    self.tools.insert(id.clone(), t);
                    self.push(ChatItem::Tool(id));
                }
                SessionUpdate::ToolCallUpdate(u) => {
                    let id = u.id.clone();
                    match self.tools.get_mut(&id) {
                        Some(t) => t.apply(&u),
                        None => {
                            let mut t = ToolCall {
                                id: id.clone(),
                                title: String::new(),
                                kind: ToolKind::Other,
                                status: ToolStatus::Pending,
                                locations: Vec::new(),
                                content: Vec::new(),
                                raw_input: None,
                            };
                            t.apply(&u);
                            self.tools.insert(id.clone(), t);
                            self.push(ChatItem::Tool(id.clone()));
                        }
                    }
                    self.refresh_tool(&id);
                }
                SessionUpdate::Plan(p) => self.plan = p,
                SessionUpdate::AvailableCommands(c) => self.commands = c,
                _ => {}
            },
            Event::Permission { id, tool_call, options, .. } => {
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
            ChatItem::Thought(t) => pad(div())
                .text_size(look.text(-1.))
                .italic()
                .text_color(look.c(ui.text_muted))
                .child(t.trim().to_string())
                .into_any_element(),
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
                let (mut add, mut del) = (0usize, 0usize);
                for c in &t.content {
                    if let ToolContent::Diff { old_text, new_text, .. } = c {
                        add += new_text.lines().count();
                        del += old_text.as_deref().map(|o| o.lines().count()).unwrap_or(0);
                    }
                }
                let loc = t.locations.first().cloned();
                let title = if t.title.is_empty() { kind.to_string() } else { t.title.clone() };
                let diff = add + del > 0;
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
                            .when(loc.is_some(), |d| d.cursor_pointer().hover(|d| d.text_color(look.c(ui.text))))
                            .child(div().w(look.z(40.)).flex_none().text_color(look.c(ui.text_muted)).child(kind))
                            .child(div().min_w(px(0.)).truncate().child(title))
                            .when(diff, |d| {
                                d.child(div().flex_none().text_color(look.c(look.theme.diff.added_text)).child(format!("+{add}")))
                                    .child(div().flex_none().text_color(look.c(look.theme.diff.removed_text)).child(format!("−{del}")))
                            })
                            .child(div().flex_none().text_color(look.c(color)).child(glyph))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                if let Some(l) = &loc {
                                    cx.emit(AgentEvent::OpenFile(l.path.clone(), l.line));
                                }
                            })),
                    )
                    .into_any_element()
            }
        }
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
                .items_center()
                .gap(look.z(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .child(div().truncate().text_color(look.c(ui.text)).child(format!("{} wants to {what}", self.def.label)))
                        .children(file.map(|f| div().truncate().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(f))),
                )
                .when(has(PermissionKind::AllowOnce), |d| d.child(button("Allow once", PermissionKind::AllowOnce, true, "perm-once")))
                .when(has(PermissionKind::AllowAlways), |d| d.child(button("Allow for this task", PermissionKind::AllowAlways, false, "perm-always")))
                .when(has(PermissionKind::RejectOnce), |d| d.child(button("Deny", PermissionKind::RejectOnce, false, "perm-deny"))),
        )
    }
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

impl Render for AgentTask {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let me = cx.entity();
        let look2 = look.clone();
        let chat = list(self.list.clone(), move |ix, window, cx| {
            let _ = window;
            me.update(cx, |t, cx| t.render_item(ix, &look2, cx))
        })
        .flex_1()
        .min_h(px(0.));
        let working = matches!(self.status, Status::Working | Status::NeedsYou);
        let elapsed = self.turn_started.filter(|_| working).map(|t| format!("  {}s", t.elapsed().as_secs())).unwrap_or_default();
        let header = div()
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
            .child(div().flex_none().text_size(look.text(-1.)).text_color(look.c(if matches!(self.status, Status::Error(_)) { ui.error } else { ui.text_muted })).child(format!("{}{elapsed}", self.status.label())))
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
            });
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
            .flex()
            .items_end()
            .gap(look.z(8.))
            .child(div().flex_1().min_w(px(0.)).child(self.prompt.clone()))
            .when(!self.pending.is_empty(), |d| {
                d.child(div().flex_none().px(look.z(6.)).rounded(look.z(3.)).bg(look.c(ui.active_bg)).text_size(look.text(-2.)).text_color(look.c(ui.text_secondary)).child(format!("{} queued", self.pending.len())))
            })
            .child(div().flex_none().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(if working { "↩ queue · esc to stop" } else { "↩ send" }));
        div()
            .key_context("AgentView")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(look.bg(ui.editor_bg))
            .child(header)
            .children(self.plan_strip(&look))
            .when(empty, |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(look.c(ui.text_muted))
                        .child(match self.status {
                            _ if self.setting_up => "Setting up the worktree…".to_string(),
                            Status::Starting => "Starting the agent…".to_string(),
                            Status::Stopped => format!("{} is stopped. Send a message to resume.", self.def.label),
                            _ => format!("Worktree: {}", self.record.worktree.display()),
                        }),
                )
            })
            .when(!empty, |d| d.child(chat))
            .children(self.permission_bar(&look, cx))
            .child(prompt_bar)
    }
}
