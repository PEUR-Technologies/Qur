//! The agent's session: starting it, sign-in, sending prompts and the queue,
//! permissions, session settings, and the ACP events.

use super::*;

impl AgentTask {
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
    pub(super) fn sign_in_bar(&self, look: &Look, cx: &mut Context<Self>) -> Option<impl IntoElement> {
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

    pub(super) fn submit(&mut self, cx: &mut Context<Self>) {
        let (text, mentions) = self.prompt.update(cx, |p, cx| p.take(cx));
        let atts = self.prompt.update(cx, |p, _| p.take_attachments());
        if text.trim().is_empty() && atts.is_empty() {
            return;
        }
        // Sending (or queueing) brings you back to the newest message.
        self.follow_bottom();
        let mut out = Outgoing::plain(if text.trim().is_empty() { "Adjust this.".into() } else { text }, mentions);
        for a in atts {
            // Picks show by name in the message; images and files as their own row under it.
            if !a.context.is_empty() {
                out.labels.push(a.label.to_string());
                out.context.push(a.context);
            }
            match (a.image, a.path) {
                (Some(image), path) => out.images.push(OutImage { image, thumb: a.thumb, path }),
                (None, Some(path)) => out.files.push(path),
                (None, None) => {}
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
    pub(super) fn drain_pending(&mut self, cx: &mut Context<Self>) {
        if !self.pending.is_empty() && matches!(self.status, Status::Done | Status::Idle) && !self.rewinding {
            let out = self.pending.remove(0);
            self.save_queue(cx);
            self.send_out(out, cx);
        }
    }

    /// Drop a queued message before it's sent.
    pub(super) fn unqueue(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.pending.len() {
            self.pending.remove(ix);
            self.save_queue(cx);
            cx.notify();
        }
    }

    /// Queue row "Send now": this message goes next, and the current turn
    /// stops (the stop path's `drain_pending` then sends it).
    pub(super) fn send_queued_now(&mut self, ix: usize, cx: &mut Context<Self>) {
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
    pub(super) fn edit_queued(&mut self, ix: usize, cx: &mut Context<Self>) {
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
            .map(|(i, c)| Attachment::pick(if named { o.labels[i].clone().into() } else { "Context".into() }, c.clone(), None))
            .collect();
        atts.extend(o.images.into_iter().map(|i| {
            let label = i.path.as_deref().map_or_else(|| "Image".into(), crate::files::chip_name);
            Attachment::image(label.into(), i.image, i.thumb, i.path)
        }));
        atts.extend(o.files.into_iter().map(Attachment::file));
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
    pub(super) fn save_queue(&mut self, cx: &mut Context<Self>) {
        self.record.queued = self
            .pending
            .iter()
            .map(|o| QueuedMessage {
                text: o.text.clone(),
                mentions: o.mentions.clone(),
                context: o.context.clone(),
                labels: o.labels.clone(),
                // An image file comes back as a link to it (images aren't saved).
                files: o.images.iter().filter_map(|i| i.path.clone()).chain(o.files.iter().cloned()).collect(),
            })
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

    pub(super) fn send_out(&mut self, out: Outgoing, cx: &mut Context<Self>) {
        // A Rewind is putting files back: this goes out once it's done.
        if self.rewinding {
            self.pending.insert(0, out);
            self.save_queue(cx);
            return cx.notify();
        }
        let shown = out.shown();
        let (Some(client), Some(session)) = (self.client.clone(), self.session_id.clone()) else {
            self.queued = Some(out.clone());
            self.push(ChatItem::User(shown));
            self.push_sent(&out, cx);
            self.start(cx);
            return;
        };
        // The last message, past its attachments row.
        let last = self.chat.iter().rev().find(|c| !matches!(c, ChatItem::Files(_)));
        if self.queued.is_none() && !matches!(last, Some(ChatItem::User(t)) if *t == shown) {
            self.push(ChatItem::User(shown));
            self.push_sent(&out, cx);
        }
        // Kept for Retry / Edit & resend, and re-queued if a sign-in is needed.
        self.last_sent = Some(out.clone());
        let sent = out.clone();
        let Outgoing { text, mentions, context, images, files, .. } = out;
        let mut blocks = vec![PromptBlock::Text(text)];
        for c in context {
            blocks.push(PromptBlock::Text(c));
        }
        // An image file also says where it is (the agent may want to use it);
        // an agent that takes no images gets just the link.
        let mut links = Vec::new();
        for OutImage { image: img, path, .. } in images {
            if self.images_ok {
                blocks.push(PromptBlock::Image { mime: img.format.mime_type().into(), data_base64: base64(img.bytes()) });
                if let Some(p) = path {
                    blocks.push(PromptBlock::Text(format!("(The image above is the file {}.)", p.display())));
                }
            } else {
                links.extend(path);
            }
        }
        for m in mentions.into_iter().chain(links).chain(files) {
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
    pub(super) fn answer_option(&mut self, option_id: &str, cx: &mut Context<Self>) {
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
    pub(super) fn cycle_mode(&mut self, _: &aa::CycleMode, _: &mut Window, cx: &mut Context<Self>) {
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
                SessionUpdate::AgentMessage(c) | SessionUpdate::UserMessage(c) if c.media.is_some() => self.push_media(c.media, cx),
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

    pub(super) fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.record.worktree).unwrap_or(p).to_string_lossy().into_owned()
    }
}

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
