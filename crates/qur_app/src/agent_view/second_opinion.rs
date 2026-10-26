//! Second-opinion review: the reviewer's thread, findings and its view.

use super::*;

impl AgentTask {
// -- second-opinion review ------------------------------------------------------

    /// The chat for a reviewer: messages and tool calls, oldest first (the
    /// part on disk too).
    pub fn thread_entries(&self) -> Vec<review::Entry> {
        let (chat, tools) = self.full_chat();
        chat.iter()
            .filter_map(|c| match c {
                ChatItem::User(t) => Some(review::Entry::new(review::Role::User, t.clone())),
                ChatItem::Agent(t) => Some(review::Entry::new(review::Role::Agent, t.clone())),
                ChatItem::Tool(id) => tools.get(id).map(|t| {
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
            weak.update(cx, |r, cx| r.send_out(Outgoing { text, mentions: Vec::new(), context: vec![prompt], labels, images: Vec::new(), files: Vec::new() }, cx)).ok();
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
        let chat = self.history_dir().map(|d| d.with_file_name(&record.id));
        let r = cx.new(|cx| AgentTask::new_reviewer(self, record, def, role, window, cx));
        if let Some(chat) = chat {
            r.update(cx, |r, cx| r.restore_chat(chat, cx));
        }
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
            // Shuts down on a thread of its own; nothing to wait for. Its
            // saved chat goes.
            drop(r.update(cx, |r, cx| {
                let h = r.shutdown(cx);
                r.delete_chat(cx);
                h
            }));
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
    pub(super) fn review_view(&self, r: &Entity<AgentTask>, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
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
