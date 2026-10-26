//! Plan mode: switch a task into the agent's plan (read-only) mode, show
//! the plan it proposes, comment on its lines, then Approve or Send
//! Comments.
//!
//! Claude (claude-agent-acp) proposes a plan by calling ExitPlanMode: a
//! `switch_mode` tool call whose `rawInput.plan` is the plan's Markdown,
//! asked as a permission ("Yes, and use auto mode", "Yes, manually approve
//! edits"…, "No, keep planning"). Approve answers it with the option that
//! goes back to the mode the task was in before plan mode, and the agent
//! carries on in the same turn. Send Comments answers "No, keep planning"
//! (the turn ends) and sends the comments as the next message, still in
//! plan mode. Agents without such a tool (Codex's read-only mode) just
//! reply with a plan: its lines get the same comments, and Approve switches
//! the mode back and tells the agent to go ahead.

use super::*;

use crate::changes_view::{COMMENT_PLACEHOLDER, comments_prompt};

/// Your comments on the plan shown, and whether it's shown.
#[derive(Default)]
pub(super) struct PlanReview {
    /// The plan view is shown in place of the chat.
    pub open: bool,
    /// What's being reviewed: the proposal's tool call id, or `reply-<chat
    /// row>` for a plan the agent only wrote as a reply.
    key: String,
    /// The plan's text, kept so comments point at fixed lines.
    text: String,
    comments: Vec<PlanComment>,
    /// The comment being written, under this line (0-based).
    draft: Option<(usize, Entity<TextInput>)>,
    /// The last proposal opened by itself (each opens once).
    seen: Option<ToolCallId>,
    _sub: Option<Subscription>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PlanComment {
    /// 0-based line of the plan.
    pub line: usize,
    pub quote: String,
    pub text: String,
}

/// The plan an agent asks you to approve: the Markdown of a `switch_mode`
/// tool call (Claude's ExitPlanMode puts it in `rawInput.plan` and in its
/// text content). None for any other tool call, or one without a plan.
pub(super) fn proposed_plan(t: &ToolCall) -> Option<String> {
    if t.kind != ToolKind::SwitchMode {
        return None;
    }
    let raw = t.raw_input.as_ref().and_then(|v| v.get("plan")).and_then(serde_json::Value::as_str).map(str::to_string);
    let text = raw.unwrap_or_else(|| {
        t.content
            .iter()
            .filter_map(|c| match c {
                ToolContent::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    });
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Which of the agent's answers Approve picks: the one that goes back to
/// `before` (the mode the task was in before plan mode) when the agent
/// offers it, else its first "allow once" ("Yes, manually approve edits",
/// the most careful), else an "always" one that doesn't clear the context.
pub(super) fn approve_option(options: &[PermissionOption], before: Option<&str>) -> Option<String> {
    let wanted = before.and_then(|m| match m {
        "auto" => Some("exit-plan-auto"),
        "bypassPermissions" => Some("exit-plan-bypass"),
        "acceptEdits" => Some("exit-plan-accept-edits"),
        "default" => Some("exit-plan-default"),
        _ => None,
    });
    wanted
        .and_then(|w| options.iter().find(|o| o.id == w))
        .or_else(|| options.iter().find(|o| o.kind == PermissionKind::AllowOnce))
        .or_else(|| options.iter().find(|o| o.kind == PermissionKind::AllowAlways && !o.name.to_lowercase().contains("clear")))
        .map(|o| o.id.clone())
}

/// The mode to leave plan mode for: `before` when it's offered (and isn't
/// the plan mode), else Accept Edits, else Default, else the first other
/// mode. As (setting id, value).
pub(super) fn exit_mode(settings: &[ConfigOption], before: Option<&str>) -> Option<(String, String)> {
    let mode = settings.iter().find(|o| o.is_mode())?;
    let (_, plan) = review::read_only_mode(settings)?;
    let ok = |v: &str| v != plan && mode.values.iter().any(|x| x.value == v);
    let pick = before
        .filter(|b| ok(b))
        .map(str::to_string)
        .or_else(|| ["acceptEdits", "default"].into_iter().find(|v| ok(v)).map(str::to_string))
        .or_else(|| mode.values.iter().find(|v| v.value != plan).map(|v| v.value.clone()))?;
    Some((mode.id.clone(), pick))
}

/// Comments on a plan as one message. `approved`: the agent may start,
/// taking them into account; otherwise it revises the plan and shows it again.
pub(super) fn plan_message(comments: &[PlanComment], approved: bool) -> String {
    if comments.is_empty() {
        return "Your plan is approved. Go ahead and implement it.".into();
    }
    let intro = if approved {
        "Your plan is approved with these changes. Implement it, addressing each one:"
    } else {
        "Comments on your plan. Revise the plan to address each one, then present it again:"
    };
    comments_prompt(intro, comments.iter().map(|c| (format!("line {}", c.line + 1), c.quote.as_str(), c.text.as_str())))
}

impl AgentTask {
    /// The plan waiting for approval: the first request's tool call, when
    /// it carries a plan.
    fn plan_proposal(&self) -> Option<(ToolCallId, String)> {
        let p = self.permissions.front()?;
        let mut tool = self.tools.get(&p.tool.id).cloned().unwrap_or_else(|| p.tool.to_tool_call());
        tool.apply(&p.tool);
        proposed_plan(&tool).map(|plan| (p.tool.id.clone(), plan))
    }

    /// The plan waiting for approval, if any (Review Agent's Plan… reads it).
    pub(super) fn proposed_plan_text(&self) -> Option<String> {
        self.plan_proposal().map(|(_, p)| p)
    }

    /// The setting and value of the agent's plan (read-only) mode.
    fn plan_mode_value(&self) -> Option<(String, String)> {
        review::read_only_mode(&self.settings)
    }

    /// The session is in plan mode (or will start in it: Plan first).
    pub fn in_plan_mode(&self) -> bool {
        if self.session_id.is_none() {
            return self.plan_first;
        }
        let Some((id, value)) = self.plan_mode_value() else { return false };
        self.settings.iter().any(|o| o.id == id && o.current == value)
    }

    /// The agent's latest reply, as a plan to comment on: (chat row, text).
    fn last_reply(&self) -> Option<(usize, String)> {
        let ix = self.chat.iter().rposition(|c| matches!(c, ChatItem::User(_) | ChatItem::Agent(_)))?;
        match &self.chat[ix] {
            ChatItem::Agent(t) if !t.trim().is_empty() => Some((ix, t.trim().to_string())),
            _ => None,
        }
    }

    /// A plan to comment on: one waiting for approval, or, in plan mode
    /// between turns, the agent's latest reply. (key, text).
    fn plan_source(&self) -> Option<(String, String)> {
        if let Some((id, plan)) = self.plan_proposal() {
            return Some((id.to_string(), plan));
        }
        let idle = !matches!(self.status, Status::Working | Status::NeedsYou | Status::Starting);
        if idle && self.in_plan_mode() {
            return self.last_reply().map(|(ix, t)| (format!("reply-{ix}"), t));
        }
        None
    }

    /// Comment on Plan, Approve Plan and Send Plan Comments apply.
    pub fn has_plan_to_review(&self) -> bool {
        self.review.is_none() && (self.plan_source().is_some() || (self.plan_review.open && !self.plan_review.text.is_empty()))
    }

    /// Plan comments written and not sent yet.
    pub fn plan_comments(&self) -> usize {
        self.plan_review.comments.len()
    }

    /// Show `key`'s plan; comments stay when it's the same plan.
    fn load_plan(&mut self, key: String, text: String) {
        let r = &mut self.plan_review;
        if r.key != key || r.text != text {
            if r.text != text {
                r.comments.clear();
                r.draft = None;
            }
            r.key = key;
            r.text = text;
        }
        r.open = true;
    }

    /// Before each frame: a new proposal opens the plan view (once); one
    /// answered elsewhere (the permission bar, Cmd+Y) closes it unless you
    /// have comments in it.
    pub(super) fn sync_plan(&mut self) {
        if self.review.is_some() {
            return;
        }
        match self.plan_proposal() {
            Some((id, plan)) if self.plan_review.seen.as_ref() != Some(&id) => {
                println!("PLAN {} proposal {id}: {} lines", self.record.id, plan.lines().count());
                self.plan_review.seen = Some(id.clone());
                self.load_plan(id.to_string(), plan);
                self.open_subagent = None;
                self.open_review = false;
            }
            Some(_) => {}
            None => {
                let r = &self.plan_review;
                let was_proposal = r.seen.as_ref().is_some_and(|s| s.as_str() == r.key);
                if r.open && was_proposal && r.comments.is_empty() && r.draft.is_none() {
                    self.plan_review.open = false;
                }
            }
        }
    }

    /// Comment on Plan: show the plan view. False when there's no plan.
    pub fn show_plan(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((key, text)) = self.plan_source() else { return false };
        self.load_plan(key, text);
        self.open_subagent = None;
        self.open_review = false;
        cx.notify();
        true
    }

    fn close_plan(&mut self, cx: &mut Context<Self>) {
        self.plan_review.open = false;
        self.plan_review.draft = None;
        cx.notify();
    }

    /// Plan Mode: into the agent's plan (read-only) mode, or back to the
    /// mode it was in before. Before the session is open, the session
    /// starts in plan mode.
    pub fn toggle_plan_mode(&mut self, cx: &mut Context<Self>) {
        if self.review.is_some() {
            return;
        }
        if self.session_id.is_none() {
            self.plan_first = !self.plan_first;
            cx.emit(AgentEvent::PlanChanged);
            return cx.notify();
        }
        let Some((id, plan)) = self.plan_mode_value() else {
            return self.notice(format!("{} has no plan or read-only mode.", self.def.label), cx);
        };
        if self.in_plan_mode() {
            if let Some((id, value)) = exit_mode(&self.settings, self.pre_plan_mode.as_deref()) {
                self.set_setting(&id, &value, cx);
            }
        } else {
            self.pre_plan_mode = self.settings.iter().find(|o| o.id == id).map(|o| o.current.clone());
            self.set_setting(&id, &plan, cx);
        }
        cx.emit(AgentEvent::PlanChanged);
    }

    /// Send a message now, or after the turn (like the prompt).
    fn send_plan_message(&mut self, text: String, cx: &mut Context<Self>) {
        let out = Outgoing::plain(text, Vec::new());
        self.follow_bottom();
        if matches!(self.status, Status::Working | Status::NeedsYou | Status::Starting) || self.setting_up || self.sign_in.is_some() || self.rewinding {
            self.pending.push(out);
            self.save_queue(cx);
            return cx.notify();
        }
        self.send_out(out, cx);
    }

    /// Answer the waiting proposal "No, keep planning" (or cancel the turn
    /// when the agent offers no such answer); `then` goes out next.
    fn keep_planning_then(&mut self, then: String, cx: &mut Context<Self>) {
        self.pending.insert(0, Outgoing::plain(then, Vec::new()));
        self.save_queue(cx);
        let reject = self.permissions.front().and_then(|p| review::deny_option(&p.options));
        match reject {
            Some(id) => self.answer_option(&id, cx),
            None => self.stop(cx),
        }
    }

    /// Approve: the agent may start. Waiting for approval with no comments,
    /// it goes on in the same turn in the mode it was in before plan mode.
    /// With comments, or a plan written as a reply, the mode goes back and
    /// the comments (or a go-ahead) are sent as the next message.
    pub fn approve_plan(&mut self, cx: &mut Context<Self>) {
        if !self.has_plan_to_review() {
            return;
        }
        let comments = std::mem::take(&mut self.plan_review.comments);
        let waiting = self.plan_proposal().is_some();
        println!("PLAN {} approve ({} comments, {})", self.record.id, comments.len(), if waiting { "proposal" } else { "reply" });
        if waiting && comments.is_empty() {
            let option = self.permissions.front().and_then(|p| approve_option(&p.options, self.pre_plan_mode.as_deref()));
            if let Some(id) = option {
                self.answer_option(&id, cx);
            }
        } else {
            if let Some((id, value)) = exit_mode(&self.settings, self.pre_plan_mode.as_deref()).filter(|_| self.in_plan_mode()) {
                self.set_setting(&id, &value, cx);
            }
            let text = plan_message(&comments, true);
            if waiting { self.keep_planning_then(text, cx) } else { self.send_plan_message(text, cx) }
        }
        self.close_plan(cx);
        cx.emit(AgentEvent::PlanChanged);
    }

    /// Send Comments: the agent revises the plan, staying in plan mode.
    pub fn send_plan_comments(&mut self, cx: &mut Context<Self>) {
        if self.plan_review.comments.is_empty() {
            return;
        }
        let text = plan_message(&std::mem::take(&mut self.plan_review.comments), false);
        println!("PLAN {} comments sent", self.record.id);
        if self.plan_proposal().is_some() {
            self.keep_planning_then(text, cx);
        } else {
            self.send_plan_message(text, cx);
        }
        self.close_plan(cx);
        cx.emit(AgentEvent::PlanChanged);
    }

    fn start_plan_comment(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new(COMMENT_PLACEHOLDER, cx));
        let sub = cx.subscribe_in(&input, window, move |t, input, ev: &InputEvent, _, cx| match ev {
            InputEvent::Confirm { .. } => {
                let text = input.read(cx).text.trim().to_string();
                if !text.is_empty() {
                    let quote = t.plan_review.text.lines().nth(line).unwrap_or("").to_string();
                    t.plan_review.comments.push(PlanComment { line, quote, text });
                    cx.emit(AgentEvent::PlanChanged);
                }
                t.plan_review.draft = None;
                cx.notify();
            }
            InputEvent::Cancel => {
                t.plan_review.draft = None;
                cx.notify();
            }
            _ => {}
        });
        self.plan_review._sub = Some(sub);
        let f = input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.plan_review.draft = Some((line, input));
        cx.notify();
    }

    /// The plan view, in place of the chat: a bar (back, Send Comments,
    /// Approve), the plan's lines (click one to comment), and a hint.
    pub(super) fn plan_view(&self, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let r = &self.plan_review;
        let waiting = self.plan_proposal().is_some();
        let n = r.comments.len();
        let button = |label: SharedString, id: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .px(look.z(8.))
                .py(look.z(2.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .bg(look.c(if primary { ui.button_primary_bg } else { ui.button_bg }))
                .text_color(look.c(if primary { ui.button_primary_text } else { ui.button_text }))
                .child(label)
        };
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let state = if waiting { format!("{} asks to start", self.def.label) } else { "plan mode".to_string() };
        let bar = div()
            .flex_none()
            .px(look.z(16.))
            .py(look.z(8.))
            .flex()
            .items_center()
            .gap(look.z(8.))
            .border_b_1()
            .border_color(look.c(ui.border))
            .child(button("← Chat".into(), "plan-back", false).on_click(cx.listener(|t, _, _, cx| t.close_plan(cx))))
            .child(div().flex_none().font_weight(gpui::FontWeight::MEDIUM).child("Plan"))
            .child(div().flex_1().min_w(px(0.)).truncate().text_color(look.c(ui.text_muted)).child(state))
            .when(n > 0, |d| d.child(button(format!("Send {n} comment{}", plural(n)).into(), "plan-send", false).on_click(cx.listener(|t, _, _, cx| t.send_plan_comments(cx)))))
            .child(
                button(if n > 0 { format!("Approve with {n} comment{}", plural(n)) } else { "Approve".to_string() }.into(), "plan-approve", true)
                    .on_click(cx.listener(|t, _, _, cx| t.approve_plan(cx))),
            );
        let mut fence = false;
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        for (i, line) in r.text.lines().enumerate() {
            let code = fence || line.trim_start().starts_with("```");
            if line.trim_start().starts_with("```") {
                fence = !fence;
            }
            let heading = !code && line.starts_with('#');
            let shown = if heading { line.trim_start_matches('#').trim().to_string() } else { line.to_string() };
            let blank = line.trim().is_empty();
            let commented = r.comments.iter().any(|c| c.line == i);
            rows.push(
                div()
                    .id(("plan-line", i))
                    .flex()
                    .items_start()
                    .gap(look.z(8.))
                    .px(look.z(16.))
                    .py(look.z(if blank { 0. } else { 1. }))
                    .when(!blank, |d| d.cursor_pointer().hover(|d| d.bg(look.c(ui.hover_bg))))
                    .when(commented, |d| d.bg(look.c(ui.active_bg)))
                    .child(
                        div()
                            .w(look.z(24.))
                            .flex_none()
                            .text_align(gpui::TextAlign::Right)
                            .font_family(look.code_font.clone())
                            .text_size(look.text(-2.))
                            .text_color(look.c(look.theme.editor.gutter_text))
                            .child(if blank { String::new() } else { (i + 1).to_string() }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .when(heading, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                            .when(code, |d| d.font_family(look.code_font.clone()).text_color(look.c(ui.text_secondary)))
                            .child(shown),
                    )
                    .when(!blank, |d| d.on_click(cx.listener(move |t, _, window, cx| t.start_plan_comment(i, window, cx))))
                    .into_any_element(),
            );
            for (ci, c) in r.comments.iter().enumerate().filter(|(_, c)| c.line == i) {
                rows.push(
                    div()
                        .id(("plan-comment", ci))
                        .ml(look.z(48.))
                        .mr(look.z(16.))
                        .my(look.z(2.))
                        .px(look.z(8.))
                        .py(look.z(3.))
                        .rounded(look.z(4.))
                        .flex()
                        .items_start()
                        .gap(look.z(6.))
                        .bg(look.c(ui.permission_bar_bg))
                        .child(div().flex_none().text_color(look.c(ui.text_muted)).child("💬"))
                        .child(div().flex_1().min_w(px(0.)).text_color(look.c(ui.text)).child(c.text.clone()))
                        .child(
                            div()
                                .id(("plan-comment-x", ci))
                                .flex_none()
                                .px(look.z(4.))
                                .cursor_pointer()
                                .text_color(look.c(ui.text_muted))
                                .hover(|d| d.text_color(look.c(ui.text)))
                                .child("×")
                                .on_click(cx.listener(move |t, _, _, cx| {
                                    if ci < t.plan_review.comments.len() {
                                        t.plan_review.comments.remove(ci);
                                        cx.emit(AgentEvent::PlanChanged);
                                    }
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),
                );
            }
            if let Some((_, input)) = r.draft.as_ref().filter(|(l, _)| *l == i) {
                rows.push(
                    div()
                        .ml(look.z(48.))
                        .mr(look.z(16.))
                        .my(look.z(2.))
                        .px(look.z(8.))
                        .py(look.z(3.))
                        .rounded(look.z(4.))
                        .bg(look.c(ui.permission_bar_bg))
                        .child(input.clone())
                        .into_any_element(),
                );
            }
        }
        let hint = if waiting {
            "Click a line to comment · Send keeps it planning · Approve lets it start, back in the mode it was in"
        } else {
            "Click a line to comment · Send keeps it planning · Approve leaves plan mode and tells it to go ahead"
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(div().id("plan-lines").flex_1().min_h(px(0.)).overflow_y_scroll().py(look.z(8.)).flex().flex_col().children(rows))
            .child(
                div()
                    .flex_none()
                    .px(look.z(16.))
                    .py(look.z(4.))
                    .border_t_1()
                    .border_color(look.c(ui.border))
                    .text_size(look.text(-2.))
                    .text_color(look.c(ui.text_muted))
                    .child(hint),
            )
    }
}

/// Plan first: put a new session in plan mode before its first message.
/// Returns its settings as the agent now has them, and the mode it was in.
pub(super) async fn plan_setup(client: &Client, session: &SessionId, mut settings: Vec<ConfigOption>) -> (Vec<ConfigOption>, Option<String>) {
    let Some((id, value)) = review::read_only_mode(&settings) else {
        println!("PLAN {session} no plan mode offered");
        return (settings, None);
    };
    let Some(opt) = settings.iter().find(|o| o.id == id).cloned() else { return (settings, None) };
    let before = Some(opt.current.clone()).filter(|c| *c != value);
    match client.set_option(session, &opt, &value).await {
        Ok(Ok(Some(list))) => {
            let legacy = settings.iter().find(|o| o.legacy_mode).cloned().filter(|_| !list.iter().any(ConfigOption::is_mode));
            settings = legacy.into_iter().chain(list).collect();
        }
        Ok(Ok(None)) => {
            if let Some(o) = settings.iter_mut().find(|o| o.id == id) {
                o.current = value.clone();
            }
        }
        Ok(Err(e)) => {
            eprintln!("PLAN {session} couldn't set {id}={value}: {e}");
            return (settings, None);
        }
        Err(_) => return (settings, None),
    }
    println!("PLAN {session} started in {id}={value}");
    (settings, before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qur_acp::ConfigValue;

    fn opt(id: &str, name: &str, kind: PermissionKind) -> PermissionOption {
        PermissionOption { id: id.into(), name: name.into(), kind }
    }

    /// claude-agent-acp 0.94's ExitPlanMode answers, in its order.
    fn claude_options() -> Vec<PermissionOption> {
        vec![
            opt("exit-plan-default", "Yes, manually approve edits", PermissionKind::AllowOnce),
            opt("exit-plan-clear-auto", "Yes, clear context (3% used) and use auto mode", PermissionKind::AllowAlways),
            opt("exit-plan-auto", "Yes, and use auto mode", PermissionKind::AllowAlways),
            opt("exit-plan-bypass", "Yes, and bypass permissions", PermissionKind::AllowAlways),
            opt("reject", "No, keep planning", PermissionKind::RejectOnce),
        ]
    }

    #[test]
    fn approve_goes_back_to_the_mode_before() {
        let o = claude_options();
        assert_eq!(approve_option(&o, Some("auto")).as_deref(), Some("exit-plan-auto"));
        assert_eq!(approve_option(&o, Some("bypassPermissions")).as_deref(), Some("exit-plan-bypass"));
        assert_eq!(approve_option(&o, Some("default")).as_deref(), Some("exit-plan-default"));
        // Not offered (auto replaces it), or unknown: the careful one.
        assert_eq!(approve_option(&o, Some("acceptEdits")).as_deref(), Some("exit-plan-default"));
        assert_eq!(approve_option(&o, None).as_deref(), Some("exit-plan-default"));
        // Never the context-clearing answer.
        let always = vec![opt("exit-plan-clear-auto", "Yes, clear context and use auto mode", PermissionKind::AllowAlways), opt("exit-plan-auto", "Yes, and use auto mode", PermissionKind::AllowAlways)];
        assert_eq!(approve_option(&always, None).as_deref(), Some("exit-plan-auto"));
        assert_eq!(approve_option(&[opt("reject", "No", PermissionKind::RejectOnce)], None), None);
        assert_eq!(review::deny_option(&o).as_deref(), Some("reject"), "Send Comments keeps planning");
    }

    #[test]
    fn plan_comes_from_exit_plan_mode() {
        let plan = "## Plan\n1. Create `hello.txt`.\n2. Verify it.\n";
        let mut t = ToolCall { kind: ToolKind::SwitchMode, title: "Approve Plan".into(), raw_input: Some(serde_json::json!({ "plan": plan, "planFilePath": "/x.md" })), ..Default::default() };
        assert_eq!(proposed_plan(&t).as_deref(), Some(plan.trim()));
        // From the text content when the input has no plan.
        t.raw_input = None;
        t.content = vec![ToolContent::Text("Step one\nStep two".into())];
        assert_eq!(proposed_plan(&t).as_deref(), Some("Step one\nStep two"));
        t.content.clear();
        assert_eq!(proposed_plan(&t), None, "no plan yet (the input streams in later)");
        let edit = ToolCall { kind: ToolKind::Edit, raw_input: Some(serde_json::json!({ "plan": "x" })), ..Default::default() };
        assert_eq!(proposed_plan(&edit), None);
    }

    fn modes(current: &str, values: &[&str]) -> Vec<ConfigOption> {
        vec![ConfigOption {
            id: "mode".into(),
            name: "Mode".into(),
            category: Some("mode".into()),
            current: current.into(),
            values: values.iter().map(|v| ConfigValue { value: v.to_string(), name: v.to_string(), description: None }).collect(),
            legacy_mode: false,
        }]
    }

    #[test]
    fn leaving_plan_mode() {
        let claude = modes("plan", &["default", "acceptEdits", "plan", "auto", "bypassPermissions"]);
        assert_eq!(exit_mode(&claude, Some("auto")), Some(("mode".into(), "auto".into())));
        assert_eq!(exit_mode(&claude, None), Some(("mode".into(), "acceptEdits".into())));
        assert_eq!(exit_mode(&claude, Some("plan")), Some(("mode".into(), "acceptEdits".into())), "never back into plan");
        let codex = modes("read-only", &["read-only", "auto", "full-access"]);
        assert_eq!(exit_mode(&codex, None), Some(("mode".into(), "auto".into())));
        assert_eq!(exit_mode(&modes("x", &["x"]), None), None, "no plan mode at all");
    }

    #[test]
    fn plan_comments_as_one_message() {
        let c = vec![
            PlanComment { line: 0, quote: "## Context".into(), text: "Say why".into() },
            PlanComment { line: 3, quote: "1. Create `hello.txt` with `hi`.".into(), text: "Make it contain hello".into() },
        ];
        let keep = plan_message(&c, false);
        assert!(keep.starts_with("Comments on your plan. Revise the plan to address each one, then present it again:\n\n"), "{keep}");
        assert!(keep.contains("1. `line 1` — `## Context`\n   Say why\n\n2. `line 4` — `1. Create `hello.txt` with `hi`.`\n   Make it contain hello\n\n"), "{keep}");
        assert!(plan_message(&c, true).starts_with("Your plan is approved with these changes. Implement it, addressing each one:"));
        assert_eq!(plan_message(&[], true), "Your plan is approved. Go ahead and implement it.");
    }
}
