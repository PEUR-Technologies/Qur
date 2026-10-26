//! The selected agent's turns: review, Show Last Turn and Rewind, and its
//! chat actions (permissions, find, copy).

use super::*;

impl Workspace {
    pub(super) fn review_changes(&mut self, _: &qur::ReviewChanges, _: &mut Window, cx: &mut Context<Self>) {
        match self.selected.clone() {
            Some(Selected::Agent(id)) => {
                if let Some(a) = self.agent(&id).cloned() {
                    a.update(cx, |a, cx| a.show_changes(cx));
                }
            }
            Some(Selected::TermTask(id)) => {
                if let Some(t) = self.term_tasks.iter_mut().find(|t| t.record.id == id) {
                    Docks::update(cx, dock::Kind::Terminal, |d| d.activate(dock::Panel::Changes));
                    t.changes.update(cx, |c, cx| c.refresh(cx));
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// The selected agent (ACP or Agent Terminal) and its newest turn that
    /// can be shown or rewound.
    pub(super) fn selected_last_turn(&self, cx: &App) -> Option<(String, u32)> {
        let record = match &self.selected {
            Some(Selected::Agent(id)) => &self.agent(id)?.read(cx).record,
            Some(Selected::TermTask(id)) => &self.term_task(id)?.record,
            _ => return None,
        };
        Some((record.id.clone(), record.last_turn()?.n))
    }

    pub(super) fn show_last_turn(&mut self, _: &qur::ShowLastTurn, _: &mut Window, cx: &mut Context<Self>) {
        match self.selected_last_turn(cx) {
            Some((id, n)) => match self.agent(&id).cloned() {
                Some(a) => a.update(cx, |a, cx| a.show_turn(n, cx)),
                None => self.show_term_turn(&id, n, cx),
            },
            None => self.no_turn_message(cx),
        }
    }

    pub(super) fn rewind_last_turn(&mut self, _: &qur::RewindLastTurn, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected_last_turn(cx) {
            Some((id, n)) => self.rewind_turn(&id, n, window, cx),
            None => self.no_turn_message(cx),
        }
    }

    // -- second-opinion review ---------------------------------------------------

    pub(super) fn review_state(&self, cx: &App) -> commands::ReviewState {
        let Some(a) = self.selected_agent() else { return commands::ReviewState::default() };
        let a = a.read(cx);
        commands::ReviewState { work: true, plan: a.plan_text().is_some(), findings: a.has_findings(cx) }
    }

    pub(super) fn review_work(&mut self, _: &qur::ReviewWork, window: &mut Window, cx: &mut Context<Self>) {
        self.open_review_picker(Subject::Work, window, cx);
    }

    pub(super) fn review_plan(&mut self, _: &qur::ReviewPlan, window: &mut Window, cx: &mut Context<Self>) {
        self.open_review_picker(Subject::Plan, window, cx);
    }

    pub(super) fn send_review_findings(&mut self, _: &qur::SendReviewFindings, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.selected_agent() else { return self.flash("Select an agent first", cx) };
        if !a.update(cx, |a, cx| a.send_findings(window, cx)) {
            self.flash("The reviewer has no findings yet", cx);
        }
    }

    /// Review Agent's Work… / Plan…: a small picker for the reviewer (agent
    /// and model) and an optional instruction. Up/Down or Tab change the
    /// reviewer, Enter starts it.
    fn open_review_picker(&mut self, subject: Subject, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.selected_agent() else {
            let msg = match self.selected {
                Some(Selected::TermTask(_)) => "Agent Terminals can't be reviewed yet: reviews are for structured agents",
                _ => "Select an agent first",
            };
            return self.flash(msg, cx);
        };
        if subject == Subject::Plan && a.read(cx).plan_text().is_none() {
            return self.flash("This agent has no plan yet", cx);
        }
        let (id, task_agent, models) = {
            let a = a.read(cx);
            (a.record.id.clone(), a.def.id.clone(), review::model_option(&a.settings).cloned())
        };
        let choices = review::choices(&self.roster.agents, &task_agent, models.as_ref(), |d| qur_agents::which(&d.command).is_some());
        if choices.is_empty() {
            return self.flash("No structured agent is installed to review with (Qur > Setup…)", cx);
        }
        // Dev/test: QUR_DEBUG_REVIEW="<agent id>[:<model>][|instruction]" starts
        // it without the picker (model: part of its name, e.g. "haiku").
        if let Ok(spec) = std::env::var("QUR_DEBUG_REVIEW") {
            let (who, instruction) = spec.split_once('|').unwrap_or((spec.as_str(), ""));
            let (agent, model) = who.split_once(':').unwrap_or((who, ""));
            let model = model.to_lowercase();
            let fits = |c: &review::Choice| {
                self.roster.agents[c.agent].id == agent
                    && (model.is_empty() || c.model.as_ref().is_some_and(|(v, n)| v.to_lowercase().contains(&model) || n.to_lowercase().contains(&model)))
            };
            println!("REVIEW choices {:?}", choices.iter().map(|c| c.label(&self.roster.agents)).collect::<Vec<_>>());
            let pick = choices.iter().position(fits).unwrap_or(0);
            let choice = choices[pick].clone();
            return self.start_review(&id, choice, subject, instruction.to_string(), window, cx);
        }
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let input = cx.new(|cx| TextInput::new("Optional: what should the reviewer look at?", cx));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Confirm { .. } => {
                let Some(Overlay::Review { task, subject, choices, chosen, .. }) = this.overlay.take() else { return };
                let instruction = input.read(cx).text.clone();
                this.prev_focus = None;
                this.start_review(&task, choices[chosen].clone(), subject, instruction, window, cx);
            }
            InputEvent::Cancel => this.close_overlays(window, cx),
            InputEvent::Up | InputEvent::Down | InputEvent::Tab => {
                if let Some(Overlay::Review { choices, chosen, touched, .. }) = &mut this.overlay {
                    let n = choices.len();
                    *chosen = if matches!(ev, InputEvent::Up) { (*chosen + n - 1) % n } else { (*chosen + 1) % n };
                    *touched = true;
                    cx.notify();
                }
            }
            _ => {}
        });
        self._subs.push(sub);
        let logins = self.review_logins.clone();
        let signed_in = |d: &AgentDef| logins.as_ref().is_some_and(|l| l.get(&review::vendor(d)) == Some(&true));
        let chosen = review::default_choice(&choices, &self.roster.agents, &task_agent, signed_in);
        // First time: ask the CLIs who's signed in (they're slow), then move
        // the default to the other vendor if you haven't picked yet.
        if self.review_logins.is_none() {
            let mut clis: Vec<String> = choices.iter().map(|c| review::vendor(&self.roster.agents[c.agent])).collect();
            clis.dedup();
            let bg = cx.background_executor().clone();
            cx.spawn_in(window, async move |this, cx| {
                let logins = bg
                    .spawn(async move {
                        clis.into_iter()
                            .filter(|c| c == "claude" || c == "codex")
                            .map(|c| {
                                let ok = matches!(crate::doctor::login(&c), crate::doctor::Login::In(_));
                                (c, ok)
                            })
                            .collect::<HashMap<_, _>>()
                    })
                    .await;
                this.update(cx, |this, cx| {
                    println!("REVIEW logins {logins:?}");
                    let roster = this.roster.agents.clone();
                    if let Some(Overlay::Review { choices, chosen, touched: false, task, .. }) = &mut this.overlay {
                        let task_agent = this.agents.iter().find(|a| a.read(cx).record.id == *task).map(|a| a.read(cx).def.id.clone()).unwrap_or_default();
                        *chosen = review::default_choice(choices, &roster, &task_agent, |d| logins.get(&review::vendor(d)) == Some(&true));
                    }
                    this.review_logins = Some(logins);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        let f = input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::Review { input, task: id, subject, choices, chosen, touched: false });
        cx.notify();
    }

    /// Start the reviewer and show it.
    fn start_review(&mut self, id: &str, choice: review::Choice, subject: Subject, instruction: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.agent(id).cloned() else { return };
        let Some(def) = self.roster.agents.get(choice.agent).cloned() else { return };
        let label = choice.label(&self.roster.agents);
        self.select(Some(Selected::Agent(id.to_string())), window, cx);
        a.update(cx, |a, cx| a.start_review(def, choice.model, subject, instruction, window, cx));
        Docks::update(cx, dock::Kind::Acp, |d| d.activate(dock::Panel::Session));
        self.flash(format!("Reviewing with {label}: read-only (Qur denies its edits and commands)"), cx);
    }

    fn no_turn_message(&mut self, cx: &mut Context<Self>) {
        let msg = match &self.selected {
            Some(Selected::TermTask(_) | Selected::Agent(_)) => "No turn has changed files yet",
            _ => "Select an agent first",
        };
        self.flash(msg, cx);
    }

    /// Rewind: ask (warning when files changed since the turn ended), then
    /// the agent stops if needed and its worktree goes back to before turn N.
    pub(super) fn rewind_turn(&mut self, id: &str, n: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.agent(id).cloned() else { return self.rewind_term_turn(id, n, window, cx) };
        let (wt, label, running, turn, later, last_end) = {
            let t = a.read(cx);
            let r = &t.record;
            let Some(turn) = r.turn(n).cloned() else { return };
            let later = r.turns.iter().filter(|x| x.n > n && !x.rewound).count();
            (r.worktree.clone(), t.def.label.clone(), t.turn_running(), turn, later, r.last_turn().and_then(|x| x.end.clone()))
        };
        if turn.rewound {
            return self.flash(format!("Turn {n} was already rewound"), cx);
        }
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            // Changed since the last turn ended (by hand, or another program)?
            let by_hand = match (running, last_end) {
                (false, Some(end)) => bg.spawn(async move { qur_git::snapshot_tree(&wt).map(|now| now != end).unwrap_or(false) }).await,
                _ => false,
            };
            this.update_in(cx, |_, window, cx| {
                let mut detail = format!(
                    "The files in this agent's worktree go back to how they were before turn {n} ({} file{}, +{} −{}).",
                    turn.files,
                    if turn.files == 1 { "" } else { "s" },
                    turn.added,
                    turn.removed
                );
                if later > 0 {
                    detail.push_str(&format!(" This also undoes the {later} turn{} after it.", if later == 1 { "" } else { "s" }));
                }
                if running {
                    detail.push_str(&format!(" {label} is working: it will be stopped first."));
                }
                if by_hand {
                    detail.push_str("

Files in the worktree changed after the turn ended (by you or another program). Rewinding loses those changes too.");
                }
                detail.push_str(&format!("

Commits stay; {label} is told with your next message."));
                let title = if later == 0 { "Rewind the last turn?".to_string() } else { format!("Rewind to before turn {n}?") };
                let answer = ask(window, PromptLevel::Warning, &title, Some(&detail), &["Rewind", "Cancel"], cx);
                let a = a.clone();
                cx.spawn(async move |_, cx| {
                    if answer.await.ok() == Some(0) {
                        a.update(cx, |a, cx| a.rewind(n, cx));
                    }
                })
                .detach();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn keep_working(&mut self, _: &qur::KeepWorking, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.selected, Some(Selected::TermTask(_))) {
            Docks::update(cx, dock::Kind::Terminal, |d| d.activate(dock::Panel::Session));
        }
        self.focus_prompt(&qur::FocusPrompt, window, cx);
    }

    pub(super) fn stop_agent(&mut self, _: &qur::StopAgent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.selected_agent() {
            a.update(cx, |a, cx| a.stop(cx));
        }
    }

    pub(super) fn allow_once(&mut self, _: &aa::AllowOnce, _: &mut Window, cx: &mut Context<Self>) {
        self.answer(PermissionKind::AllowOnce, cx)
    }
    pub(super) fn allow_always(&mut self, _: &aa::AllowAlways, _: &mut Window, cx: &mut Context<Self>) {
        self.answer(PermissionKind::AllowAlways, cx)
    }
    pub(super) fn deny(&mut self, _: &aa::Deny, _: &mut Window, cx: &mut Context<Self>) {
        self.answer(PermissionKind::RejectOnce, cx)
    }
    /// Allow All Pending: every request the selected agent (else the first
    /// agent waiting) has queued, each "allow once".
    pub(super) fn allow_all_pending(&mut self, _: &aa::AllowAllPending, _: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .selected_agent()
            .filter(|a| a.read(cx).has_permission())
            .or_else(|| self.agents.iter().find(|a| a.read(cx).has_permission()).cloned());
        let Some(a) = target else { return self.flash("No permission requests are waiting", cx) };
        let (n, left) = a.update(cx, |a, cx| (a.allow_all_pending(cx), a.pending_permissions()));
        let mut msg = format!("Allowed {n} request{} once", if n == 1 { "" } else { "s" });
        if left > 0 {
            msg.push_str(&format!("; {left} without an Allow once still need{} an answer", if left == 1 { "s" } else { "" }));
        }
        self.flash(msg, cx);
    }
    /// Find in Chat (Cmd+F in a chat agent).
    pub(super) fn find_in_chat(&mut self, _: &aa::FindInChat, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected_agent() {
            Some(a) => a.update(cx, |a, cx| a.open_find(window, cx)),
            None => self.flash("Find in Chat needs a chat agent: select one first", cx),
        }
    }
    /// Copy Chat as Markdown: the selected chat agent's whole chat.
    pub(super) fn copy_chat(&mut self, _: &aa::CopyChatMarkdown, _: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.selected_agent() else {
            let msg = match &self.selected {
                Some(Selected::TermTask(_)) => "Agent Terminals have no chat to copy: select the text and press Cmd+C",
                _ => "Select a chat agent first",
            };
            return self.flash(msg, cx);
        };
        let md = a.read(cx).chat_markdown();
        let lines = md.lines().count();
        cx.write_to_clipboard(ClipboardItem::new_string(md));
        self.flash(format!("Copied the chat as Markdown ({lines} lines)"), cx);
    }
    /// Answer the selected agent's request, else the first agent waiting.
    fn answer(&mut self, kind: PermissionKind, cx: &mut Context<Self>) {
        let target = self
            .selected_agent()
            .filter(|a| a.read(cx).has_permission())
            .or_else(|| self.agents.iter().find(|a| a.read(cx).has_permission()).cloned());
        if let Some(a) = target {
            a.update(cx, |a, cx| a.answer_permission(kind, cx));
        }
    }

    // -- plan mode -----------------------------------------------------------------

    pub(super) fn plan_state(&self, cx: &App) -> commands::PlanState {
        if let Some(Overlay::NewTask { plan, agent, .. }) = &self.overlay {
            let acp = self.roster.agents.get(*agent).is_some_and(|a| a.mode == Mode::Acp);
            return commands::PlanState { agent: acp, on: *plan && acp, ..Default::default() };
        }
        let Some(a) = self.selected_agent() else { return commands::PlanState::default() };
        let a = a.read(cx);
        let plan = a.has_plan_to_review();
        commands::PlanState { agent: a.review.is_none(), on: a.in_plan_mode(), plan, comments: plan && a.plan_comments() > 0 }
    }

    /// The New Agent Task dialog's Plan first.
    pub(super) fn toggle_plan_first(&mut self, cx: &mut Context<Self>) {
        if let Some(Overlay::NewTask { plan, .. }) = &mut self.overlay {
            *plan = !*plan;
        }
        self.sync_menus(cx);
        cx.notify();
    }

    /// Plan Mode (Cmd+Opt+P): the selected agent into its plan mode or back
    /// out; in the New Agent Task dialog, Plan first.
    pub(super) fn plan_mode(&mut self, _: &qur::PlanMode, _: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Some(Overlay::NewTask { .. })) {
            return self.toggle_plan_first(cx);
        }
        let Some(a) = self.selected_agent() else {
            let msg = match self.selected {
                Some(Selected::TermTask(_)) => "In an Agent Terminal, press Shift+Tab to cycle Claude Code's modes",
                _ => "Select a chat agent first",
            };
            return self.flash(msg, cx);
        };
        let on = a.update(cx, |a, cx| {
            a.toggle_plan_mode(cx);
            a.in_plan_mode()
        });
        self.flash(if on { "Plan mode: the agent plans and asks before it changes anything" } else { "Plan mode off" }, cx);
        self.sync_menus(cx);
    }

    pub(super) fn comment_on_plan(&mut self, _: &qur::CommentOnPlan, _: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.selected_agent() else { return self.flash("Select a chat agent first", cx) };
        if !a.update(cx, |a, cx| a.show_plan(cx)) {
            self.flash("No plan to comment on yet: turn on Plan Mode (Cmd+Opt+P) and ask for one", cx);
        }
    }

    pub(super) fn approve_plan(&mut self, _: &qur::ApprovePlan, _: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.selected_agent().filter(|a| a.read(cx).has_plan_to_review()) else { return self.flash("No plan is waiting for approval", cx) };
        a.update(cx, |a, cx| a.approve_plan(cx));
    }

    pub(super) fn send_plan_comments(&mut self, _: &qur::SendPlanComments, _: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.selected_agent().filter(|a| a.read(cx).plan_comments() > 0) else { return self.flash("No plan comments to send: click a line of the plan to comment", cx) };
        a.update(cx, |a, cx| a.send_plan_comments(cx));
    }
}
