//! Agent tasks and Agent Terminals: restoring, starting, watching, naming,
//! removing and discarding them.

use super::*;

impl Workspace {
    pub(super) fn restore_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = TaskStore::load(&self.project_id);
        for r in store.tasks {
            match r.mode {
                Mode::Acp => {
                    let def = self.roster.get(&r.agent).cloned().unwrap_or_else(|| Roster::defaults().agents[0].clone());
                    self.add_agent(r, def, window, cx);
                }
                Mode::Terminal => self.add_term_task(r, window, cx),
            }
        }
        self.apply_look(window, cx);
    }

    pub(super) fn add_term_task(&mut self, record: TaskRecord, window: &mut Window, cx: &mut Context<Self>) {
        let todo_id = record.id.clone();
        let accent = Look::of(window, cx).theme.accents[0].base;
        let monitor = cx.new(|cx| {
            let mut m = TaskMonitor::new(record.worktree.clone(), record.base.clone(), accent, record.in_root, cx);
            // No turns in a terminal: in its own worktree every write counts as the
            // agent's edit. In the project folder, only while it's busy (writing output;
            // see terminal_activity), so your edits there stay yours.
            m.set_turn_active(!record.in_root);
            m
        });
        self.watch_monitor(&monitor, window, cx);
        let (wrap, tab) = (self.config.editor.soft_wrap, self.config.editor.tab_width);
        let live = cx.new(|cx| LivePane::new(monitor.clone(), "the agent".into(), wrap, tab, window, cx));
        let sub = cx.subscribe_in(&live, window, |this, _, ev: &LiveEvent, window, cx| {
            if let LiveEvent::Open(p) = ev {
                let p = p.clone();
                this.open_path(&p, window, cx);
            }
        });
        self._subs.push(sub);
        let changes = cx.new(|cx| ChangesView::new(self.root.clone(), record.worktree.clone(), record.base.clone(), record.branch.clone(), cx));
        let id = record.id.clone();
        let sub = cx.subscribe_in(&changes, window, move |this, _, ev: &ChangesEvent, window, cx| this.on_changes_event(&id, ev, window, cx));
        self._subs.push(sub);
        let browser = cx.new(|cx| BrowserPane::new(record.browser_url.clone(), accent, window, cx));
        let id = record.id.clone();
        let sub = cx.subscribe_in(&browser, window, move |this, _, ev: &BrowserPaneEvent, window, cx| match ev {
            BrowserPaneEvent::Attach(a) => this.paste_attachment(&id, a, window, cx),
            BrowserPaneEvent::Navigated(url) => {
                if let Some(t) = this.term_tasks.iter_mut().find(|t| t.record.id == id) {
                    t.record.browser_url = Some(url.clone());
                    this.save_tasks(cx);
                }
            }
        });
        self._subs.push(sub);
        self.term_tasks.push(TermTask {
            record,
            term_id: None,
            accent,
            monitor,
            live,
            changes,
            browser,
            first_run: None,
            pending_type: None,
            was_busy: false,
            busy_since: None,
            wrote: false,
            todos: Default::default(),
        });
        self.refresh_overlaps(cx);
        self.poll_todos(todo_id, cx);
    }

    /// Follow a terminal Claude's to-do list in its transcript, every second,
    /// until the task goes away.
    fn poll_todos(&mut self, id: String, cx: &mut Context<Self>) {
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                bg.timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| {
                    let Some(t) = this.term_tasks.iter_mut().find(|t| t.record.id == id) else { return false };
                    let dir = qur_agents::todos::claude_project_dir(&t.record.worktree);
                    if t.todos.refresh(&dir) {
                        cx.notify();
                    }
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// An Inspect pick in an Agent Terminal's Browser: paste it into the
    /// agent's terminal (not sent), with the screenshot saved to a file so a
    /// CLI agent can read it.
    fn paste_attachment(&mut self, id: &str, a: &Attachment, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tid) = self.term_task(id).and_then(|t| t.term_id) else {
            return self.flash("Start the agent's terminal first", cx);
        };
        let mut text = a.context.trim_end().to_string();
        if let Some(img) = &a.image {
            let path = std::env::temp_dir().join(format!("qur-pick-{}.png", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())));
            if std::fs::write(&path, img.bytes()).is_ok() {
                text.push_str(&format!("\nScreenshot: {}", path.display()));
            }
        }
        text.push('\n');
        self.terms.update(cx, |p, cx| {
            p.type_text(tid, &text, cx);
            p.activate_id(tid, cx);
        });
        self.release_browser_focus(cx);
        let f = self.terms.read(cx).focus.clone();
        window.focus(&f, cx);
    }

    /// A task's watcher saw files change: refresh its tree and open copies.
    fn watch_monitor(&mut self, monitor: &Entity<TaskMonitor>, window: &mut Window, cx: &mut Context<Self>) {
        let sub = cx.subscribe_in(monitor, window, |this, m, ev: &MonitorEvent, window, cx| match ev {
            MonitorEvent::Files(paths) => {
                let root = m.read(cx).worktree.clone();
                if let Some(t) = this.term_tasks.iter_mut().find(|t| t.record.worktree == root) {
                    t.wrote = true;
                }
                let paths = paths.clone();
                this.files_changed(&root, &paths, window, cx);
            }
            MonitorEvent::Changed => cx.notify(),
        });
        self._subs.push(sub);
    }

    /// Watch the main checkout (FSEvents): tree structure and open buffers.
    pub(super) fn watch_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok((w, mut changes)) = qur_watch::Watcher::new(&self.root, Duration::from_millis(50)) else { return };
        self._root_watcher = Some(w);
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt;
            while let Some(batch) = changes.next().await {
                if this.update_in(cx, |this, window, cx| this.files_changed(&root, &batch, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn files_changed(&mut self, root: &Path, paths: &[PathBuf], _window: &mut Window, cx: &mut Context<Self>) {
        // New or removed files change the tree's shape; plain writes don't.
        if let Some(tree) = self.trees.get(root).cloned() {
            let structural = paths.iter().any(|p| {
                let rel = p.strip_prefix(root).unwrap_or(p);
                // An ignore file changed: what the tree hides may have too.
                let rules = matches!(rel.file_name().and_then(|n| n.to_str()), Some(".gitignore" | ".ignore"));
                rules || tree.read(cx).tree.on_disk(rel) != p.exists()
            });
            if structural {
                tree.update(cx, |t, cx| t.refresh(cx));
            }
        }
        self.file_lists.remove(root);
        if root == self.root.as_path()
            && let Some(g) = &self.git
        {
            g.update(cx, |g, cx| g.refresh(cx));
        }
        // Open copies: clean ones reload, dirty ones get the banner.
        for b in self.buffers.list.clone() {
            let hit = b.read(cx).path().is_some_and(|bp| paths.iter().any(|p| p == bp));
            if hit {
                b.update(cx, |b, cx| {
                    b.disk_change(cx);
                });
            }
        }
        cx.notify();
    }

    pub(super) fn save_tasks(&self, cx: &App) {
        let mut store = TaskStore::default();
        for a in &self.agents {
            store.upsert(a.read(cx).record.clone());
        }
        for t in &self.term_tasks {
            store.upsert(t.record.clone());
        }
        let _ = store.save(&self.project_id);
    }

    fn add_agent(&mut self, record: TaskRecord, def: AgentDef, window: &mut Window, cx: &mut Context<Self>) -> Entity<AgentTask> {
        let id = record.id.clone();
        let accent = Look::of(window, cx).theme.accents[0].base;
        let (layout, wrap, tab) = (self.layout, self.config.editor.soft_wrap, self.config.editor.tab_width);
        let root = self.root.clone();
        let a = cx.new(|cx| AgentTask::new(root, record, def, accent, layout, wrap, tab, window, cx));
        // Messages still queued when Qur last quit: start the agent so they go out.
        a.update(cx, |a, cx| {
            if !a.record.queued.is_empty() && a.status == Status::Stopped {
                a.start(cx);
            }
        });
        // Its reviewer from last time: back under it, stopped (session/load
        // shows its findings again once you send it something).
        let link = a.read(cx).record.review.clone();
        if let Some(link) = link {
            match self.roster.get(&link.agent).cloned() {
                Some(def) => {
                    println!("REVIEW {} restored ({} {})", a.read(cx).record.id, link.agent, link.model.as_deref().unwrap_or("default model"));
                    a.update(cx, |a, cx| a.attach_reviewer(&link, def, window, cx));
                }
                None => a.update(cx, |a, _| a.record.review = None),
            }
        }
        AGENT_IDS.with(|m| m.borrow_mut().insert(a.entity_id(), id));
        let monitor = a.read(cx).monitor.clone();
        self.watch_monitor(&monitor, window, cx);
        let sub = cx.subscribe_in(&a, window, |this, a, ev: &AgentEvent, window, cx| match ev {
            AgentEvent::Changed => {
                let (id, status) = {
                    let a = a.read(cx);
                    (a.record.id.clone(), a.status.clone())
                };
                let attention = matches!(status, Status::Done | Status::NeedsYou | Status::Error(_));
                if status == Status::Working {
                    this.checks.remove(&id);
                    this.alerted.remove(&id);
                }
                let looking = window.is_window_active() && this.selected == Some(Selected::Agent(id.clone()));
                if attention && !looking {
                    this.unread.insert(id.clone());
                    if this.alerted.get(&id) != Some(&status) {
                        this.alerted.insert(id.clone(), status.clone());
                        let (title, last) = {
                            let a = a.read(cx);
                            (a.record.title.clone(), a.last_line())
                        };
                        let (kind, detail) = match &status {
                            Status::NeedsYou => (alerts::Kind::NeedsApproval, None),
                            Status::Error(e) => (alerts::Kind::Failed, Some(e.clone())),
                            _ => (alerts::Kind::Finished, last),
                        };
                        let tag = alert_tag(window, &id);
                        alerts::fire(cx, &this.config.alerts, tag, &title, kind, detail.as_deref());
                    }
                }
                this.sync_badge(window);
                this.save_tasks(cx);
                // A plan may have arrived: Review Agent's Plan… follows.
                this.sync_menus(cx);
                cx.notify();
            }
            AgentEvent::Subagents => cx.notify(),
            AgentEvent::Review => {
                this.save_tasks(cx);
                this.sync_menus(cx);
                cx.notify();
            }
            AgentEvent::TurnsChanged | AgentEvent::PermissionsChanged | AgentEvent::PlanChanged => this.sync_menus(cx),
            AgentEvent::Rewind(n) => {
                let id = a.read(cx).record.id.clone();
                this.rewind_turn(&id, *n, window, cx);
            }
            AgentEvent::SlashRequested => {
                let (cmds, opts, target) = {
                    let a = a.read(cx);
                    (a.commands.clone(), a.settings.clone(), a.prompt.clone())
                };
                if !cmds.is_empty() || !opts.is_empty() {
                    // Settings first ("/model  Opus 5.5"): the prompt bar no
                    // longer shows them, and ACP agents don't offer them as commands.
                    let settings = opts.iter().map(|o| o.id.clone()).collect();
                    let names = cmds.iter().map(|c| c.name.clone()).collect();
                    let items = opts
                        .iter()
                        .map(|o| PickItem::new(format!("/{}", o.name.to_lowercase().replace(' ', "-"))).right(o.current_name().to_string()))
                        .chain(cmds.iter().map(|c| PickItem::new(format!("/{}", c.name)).detail(c.description.clone())))
                        .collect();
                    this.open_picker("Agent command…", items, PickKind::Slash { agent: a.clone(), settings, names, target }, false, window, cx);
                }
            }
            AgentEvent::OpenFile(p, line) => {
                let p = p.clone();
                let at = line.map(|l| Point { line: (l as usize).saturating_sub(1), col: 0 });
                this.open_path_at(&p, at, window, cx);
            }
            AgentEvent::Preview(p) => this.preview(p.clone(), window, cx),
            AgentEvent::Merge => {
                let id = a.read(cx).record.id.clone();
                this.merge_task(&id, window, cx);
            }
            AgentEvent::TurnChangedFiles => {
                let id = a.read(cx).record.id.clone();
                this.run_checks(&id, false, window, cx);
            }
            AgentEvent::Discard => {
                let id = a.read(cx).record.id.clone();
                this.select(Some(Selected::Agent(id)), window, cx);
                this.discard_task(&qur::DiscardTask, window, cx);
            }
            AgentEvent::MentionRequested => {
                let (root, target) = {
                    let a = a.read(cx);
                    (a.record.worktree.clone(), a.prompt.clone())
                };
                this.mention_picker(root, target, window, cx);
            }
            // Sign-in: the login command in a terminal of its own, shown now;
            // the agent's callout waits for Continue.
            AgentEvent::RunInTerminal { label, command } => {
                let wt = a.read(cx).record.worktree.clone();
                let kind = Kind::Agent { task: label.clone(), command: command.clone() };
                let Some(tid) = this.terms.update(cx, |p, cx| p.open(&wt, kind, label.clone(), cx)) else { return };
                this.shells.push(tid);
                this.select(Some(Selected::Shell(tid)), window, cx);
                this.flash(format!("{label}: when you've signed in, go back to the agent and click Continue"), cx);
            }
        });
        self._subs.push(sub);
        self.agents.push(a.clone());
        self.refresh_overlaps(cx);
        a
    }

    /// New Agent Task: worktree, then spawn the agent with the task as its
    /// first prompt (`plan`: sent in the agent's plan mode).
    pub(super) fn start_task(&mut self, agent_ix: usize, text: String, mentions: Vec<PathBuf>, plan: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(def) = self.roster.agents.get(agent_ix).cloned() else { return };
        let title = first_line(&text);
        let slug = qur_worktree::slugify(&title);
        let root = self.root.clone();
        let dir = qur_agents::worktrees_dir(&self.project_id);
        let bg = cx.background_executor().clone();
        let agent_id = def.id.clone();
        let terminal_mode = def.mode == Mode::Terminal;
        let setup = self.project.worktree.setup.clone().filter(|s| !s.trim().is_empty());
        self.flash(format!("Creating worktree for “{title}”…"), cx);
        cx.spawn_in(window, async move |this, cx| {
            let created = bg
                .spawn(async move {
                    let c = qur_worktree::create(&root, &dir, &agent_id, &slug)?;
                    // .worktreeinclude (decision 22): .env, node_modules, … via APFS clones.
                    let copied = qur_worktree::copy_includes(&root, &c.path).unwrap_or_default();
                    if terminal_mode {
                        let _ = qur_agents::trust_folder_for_claude(&c.path);
                    }
                    Ok::<_, String>((c, copied))
                })
                .await;
            this.update_in(cx, |this, window, cx| match created {
                Ok((c, copied)) => {
                    this.message = None;
                    let record = TaskRecord {
                        id: c.slug.clone(),
                        agent: def.id.clone(),
                        title,
                        branch: c.branch,
                        worktree: c.path,
                        base: c.base,
                        session_id: None,
                        created: qur_agents::now_secs(),
                        mode: def.mode,
                        // Dev/test: QUR_DEBUG_BROWSER_URL="http://localhost:8765/{slug}/index.html".
                        browser_url: std::env::var("QUR_DEBUG_BROWSER_URL").ok().map(|u| u.replace("{slug}", &c.slug)),
                        renamed: false,
                        in_root: false,
                        queued: Vec::new(),
                        turns: Vec::new(), review: None,
                    };
                    let debug_browser = std::env::var_os("QUR_DEBUG_BROWSER_URL").is_some();
                    match def.mode {
                        Mode::Acp => {
                            let a = this.add_agent(record, def, window, cx);
                            a.update(cx, |a, _| a.plan_first = plan);
                            if debug_browser {
                                a.update(cx, |a, cx| a.show_browser(true, cx));
                            }
                            // Mentions were picked in the main checkout; point
                            // them at the same files in the worktree.
                            let wt = a.read(cx).record.worktree.clone();
                            let mentions = mentions
                                .into_iter()
                                .map(|m| m.strip_prefix(&this.root).map(|r| wt.join(r)).unwrap_or(m))
                                .collect();
                            if !copied.is_empty() {
                                let list = copied.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join(", ");
                                a.update(cx, |a, cx| a.notice(format!("Copied into the worktree (.worktreeinclude): {list}"), cx));
                            }
                            match setup.clone() {
                                None => a.update(cx, |a, cx| a.send(text, mentions, cx)),
                                Some(cmd) => {
                                    a.update(cx, |a, cx| {
                                        a.setting_up = true;
                                        a.notice(format!("Running setup: {cmd}"), cx);
                                    });
                                    let a2 = a.clone();
                                    let bg = cx.background_executor().clone();
                                    cx.spawn(async move |_, cx| {
                                        let path = qur_agents::login_path().to_string();
                                        let (ok, out) = bg.spawn(async move { qur_worktree::run_setup(&wt, &cmd, &path) }).await;
                                        a2.update(cx, |a, cx| {
                                            a.setting_up = false;
                                            if !out.trim().is_empty() {
                                                a.output(out, cx);
                                            }
                                            if ok {
                                                a.send(text, mentions, cx);
                                            } else {
                                                a.set_error("setup failed; fix it and send the task again".into(), cx);
                                            }
                                        });
                                    })
                                    .detach();
                                }
                            }
                            this.apply_look(window, cx);
                            this.save_tasks(cx);
                            this.select(Some(Selected::Agent(c.slug)), window, cx);
                        }
                        Mode::Terminal => {
                            this.add_term_task(record, window, cx);
                            if let Some(t) = this.term_tasks.last_mut() {
                                t.first_run = Some(FirstRun { prompt: text.clone(), setup: setup.clone() });
                            }
                            this.apply_look(window, cx);
                            this.save_tasks(cx);
                            this.select(Some(Selected::TermTask(c.slug)), window, cx);
                        }
                    }
                }
                Err(e) => this.flash(e, cx),
            })
            .ok();
        })
        .detach();
    }

    /// New Agent Terminal (Cmd+Shift+T, decisions 12, 17): a terminal-mode
    /// Claude task in its own worktree, titled by its first typed line.
    pub(super) fn new_agent_terminal(&mut self, _: &qur::NewAgentTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.term_tasks.len() + 1;
        // Like a normal terminal, the first one works in the project folder
        // itself. While one does, more get their own worktree so two agents
        // never edit the same files.
        let root_busy = self.term_tasks.iter().any(|t| t.record.in_root) || self.agents.iter().any(|a| a.read(cx).record.in_root);
        if !root_busy {
            let root = self.root.clone();
            let git = |args: &[&str]| qur_worktree::git(&root, args).ok();
            let record = TaskRecord {
                id: format!("root-{n}"),
                agent: "claude-terminal".into(),
                title: format!("agent terminal {n}"),
                branch: git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default(),
                worktree: root.clone(),
                base: git(&["rev-parse", "HEAD"]).unwrap_or_default(),
                session_id: None,
                created: qur_agents::now_secs(),
                mode: Mode::Terminal,
                browser_url: None,
                renamed: false,
                in_root: true,
                queued: Vec::new(),
                turns: Vec::new(), review: None,
            };
            let id = record.id.clone();
            self.add_term_task(record, window, cx);
            self.apply_look(window, cx);
            self.save_tasks(cx);
            self.select(Some(Selected::TermTask(id)), window, cx);
            return;
        }
        let slug = format!("term-{n}");
        let root = self.root.clone();
        let dir = qur_agents::worktrees_dir(&self.project_id);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let created = bg
                .spawn(async move {
                    let c = qur_worktree::create(&root, &dir, "claude", &slug)?;
                    let _ = qur_worktree::copy_includes(&root, &c.path);
                    // Decision 24: skip Claude's "trust this folder?" prompt for the worktree.
                    let _ = qur_agents::trust_folder_for_claude(&c.path);
                    Ok::<_, String>(c)
                })
                .await;
            this.update_in(cx, |this, window, cx| match created {
                Ok(c) => {
                    let record = TaskRecord {
                        id: c.slug.clone(),
                        agent: "claude-terminal".into(),
                        title: format!("agent terminal {n}"),
                        branch: c.branch,
                        worktree: c.path,
                        base: c.base,
                        session_id: None,
                        created: qur_agents::now_secs(),
                        mode: Mode::Terminal,
                        browser_url: None,
                        renamed: false,
                        in_root: false,
                        queued: Vec::new(),
                        turns: Vec::new(), review: None,
                    };
                    this.add_term_task(record, window, cx);
                    this.apply_look(window, cx);
                    this.save_tasks(cx);
                    this.select(Some(Selected::TermTask(c.slug)), window, cx);
                }
                Err(e) => this.flash(e, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Show a terminal task, (re)starting its process when needed.
    pub(super) fn show_term_task(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.term_tasks.iter().position(|t| t.record.id == id) else { return };
        let alive = self.term_tasks[ix].term_id.is_some_and(|tid| self.terms.update(cx, |p, cx| p.activate_id(tid, cx)));
        if !alive {
            let def = self.roster.get(&self.term_tasks[ix].record.agent).filter(|d| d.mode == Mode::Terminal).cloned();
            let mut command = def.as_ref().map(shell_command).unwrap_or_else(|| "claude".into());
            // First start of a New Agent Task: pass the task text and run the
            // project's setup first (decision 22), like ACP tasks.
            if let Some(first) = self.term_tasks[ix].first_run.take() {
                let prompt = first.prompt.trim().to_string();
                if !prompt.is_empty() {
                    match def.as_ref().map(AgentDef::prompt_via).unwrap_or(qur_agents::PromptVia::Type) {
                        qur_agents::PromptVia::Arg => command = format!("{command} {}", shell_quote(&prompt)),
                        qur_agents::PromptVia::Flag(f) => command = format!("{command} {} {}", shell_quote(&f), shell_quote(&prompt)),
                        qur_agents::PromptVia::Type => self.term_tasks[ix].pending_type = Some(prompt),
                    }
                }
                if let Some(setup) = first.setup {
                    command = format!(
                        "echo {}; if {setup}; then {command}; else echo; echo {}; fi",
                        shell_quote(&format!("qur: running setup: {setup}")),
                        shell_quote("qur: setup failed. Fix it, then start the agent from this shell.")
                    );
                }
            }
            let t = &self.term_tasks[ix];
            let kind = Kind::Agent { task: t.record.title.clone(), command };
            let wt = t.record.worktree.clone();
            let label = t.record.title.clone();
            let tid = self.terms.update(cx, |p, cx| p.open(&wt, kind, label, cx));
            self.term_tasks[ix].term_id = tid;
        }
        let f = self.terms.read(cx).focus.clone();
        window.focus(&f, cx);
    }

    /// Agent Terminals have no turns, so "busy, then quiet" stands in for one
    /// (M5): deliver a typed first prompt once the agent is first quiet, and
    /// after a busy stretch that changed files, run the project's checks if
    /// `[checks] terminal = true`.
    pub(super) fn terminal_activity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut turns = Vec::new();
        let mut typed = Vec::new();
        let mut finished = Vec::new();
        let mut renamed = false;
        let mut busy_flags = Vec::new();
        {
            let panel = self.terms.read(cx);
            for t in &mut self.term_tasks {
                let Some(s) = t.term_id.and_then(|tid| panel.session_by_id(tid)) else { continue };
                // Name: the user's rename, else what the agent says it's doing
                // (its terminal title), else the first line typed.
                if !t.record.renamed
                    && let Some(name) = s.agent_title().or_else(|| s.first_line.clone())
                    && name != t.record.title
                {
                    t.record.title = name;
                    renamed = true;
                }
                let busy = s.is_busy();
                if t.record.in_root {
                    busy_flags.push((t.monitor.clone(), busy));
                }
                let started = s.has_output();
                if !busy && started && let Some(p) = t.pending_type.take() {
                    // Startup (and setup) isn't a turn: start counting from here.
                    typed.push((s.id, p));
                    t.was_busy = false;
                    t.wrote = false;
                    continue;
                }
                if t.pending_type.is_some() {
                    continue;
                }
                if busy {
                    t.was_busy = true;
                    t.busy_since.get_or_insert_with(Instant::now);
                } else {
                    // A long busy stretch that went quiet: the agent is
                    // probably done and waiting for you.
                    if t.busy_since.take().is_some_and(|b| b.elapsed() >= Duration::from_secs(7)) {
                        finished.push((t.record.id.clone(), t.record.title.clone()));
                    }
                    if std::mem::take(&mut t.was_busy) && std::mem::take(&mut t.wrote) {
                        turns.push(t.record.id.clone());
                    }
                }
            }
        }
        for (m, busy) in busy_flags {
            m.update(cx, |m, _| m.set_turn_active(busy));
        }
        if renamed {
            self.save_tasks(cx);
        }
        for (id, title) in finished {
            let looking = window.is_window_active() && self.selected == Some(Selected::TermTask(id.clone()));
            if !looking {
                self.unread.insert(id.clone());
                let tag = alert_tag(window, &id);
                alerts::fire(cx, &self.config.alerts, tag, &title, alerts::Kind::Finished, Some("Went quiet: probably waiting for you"));
            }
        }
        self.sync_badge(window);
        for (tid, p) in typed {
            self.terms.update(cx, |panel, cx| {
                panel.type_text(tid, &p, cx);
                panel.send_keys(tid, "\r");
            });
        }
        // The agent changed files: show the result in its Browser.
        for id in &turns {
            if let Some(t) = self.term_task(id) {
                t.browser.update(cx, |b, _| b.reload());
            }
        }
        if !turns.is_empty() && ProjectSettings::load(&self.project_id).checks.terminal {
            for id in turns {
                self.run_checks(&id, false, window, cx);
            }
        }
    }

    /// You looked at an agent: it no longer needs you (badge, notification).
    pub(super) fn seen(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.unread.remove(id) {
            alerts::dismiss(cx, &alert_tag(window, id));
        }
        self.sync_badge(window);
    }

    fn sync_badge(&self, window: &Window) {
        let n = self.unread.iter().filter(|id| self.task_exists(id)).count();
        alerts::set_needs(window.window_handle().window_id().as_u64(), n, self.config.alerts.badge);
    }

    fn task_exists(&self, id: &str) -> bool {
        self.agents.iter().any(|a| agent_id_matches(a, id)) || self.term_tasks.iter().any(|t| t.record.id == id)
    }

    /// A notification was clicked: show that agent (main.rs routes it here).
    pub fn reveal_task(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.agent(id).is_some() {
            self.select(Some(Selected::Agent(id.to_string())), window, cx);
        } else if self.term_tasks.iter().any(|t| t.record.id == id) {
            self.select(Some(Selected::TermTask(id.to_string())), window, cx);
        } else {
            return false;
        }
        window.activate_window();
        true
    }

    /// Stop a task's process and delete its worktree and branch (no prompt).
    /// Discard and post-merge cleanup: stop the task's agent, its terminal and
    /// its Run Project dev server, and drop it from the window. Returns the
    /// agent's shutdown thread to join before removing the worktree.
    fn stop_task(&mut self, id: &str, wt: &Path, cx: &mut Context<Self>) -> Option<std::thread::JoinHandle<()>> {
        let mut handle = None;
        if let Some(a) = self.agent(id).cloned() {
            handle = a.update(cx, |a, cx| a.shutdown(cx));
            self.agents.retain(|x| x != &a);
        }
        for t in self.term_task(id).and_then(|t| t.term_id).into_iter().chain(self.runs.remove(id)) {
            self.terms.update(cx, |p, cx| {
                if let Some(ix) = p.sessions.iter().position(|s| s.id == t) {
                    p.close(ix, cx);
                }
            });
        }
        self.term_tasks.retain(|t| t.record.id != id);
        self.checks.remove(id);
        self.trees.remove(wt);
        handle
    }

    pub(super) fn in_root(&self, id: &str, cx: &App) -> bool {
        self.agent(id).is_some_and(|a| a.read(cx).record.in_root) || self.term_task(id).is_some_and(|t| t.record.in_root)
    }

    pub(super) fn remove_task(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((wt, branch, _, _)) = self.task_parts(id, cx) else { return };
        let in_root = self.in_root(id, cx);
        let handle = self.stop_task(id, &wt, cx);
        self.save_tasks(cx);
        if in_root {
            // Its work is your folder: close it, delete nothing.
            return cx.notify();
        }
        let root = self.root.clone();
        let bg = cx.background_executor().clone();
        let id = id.to_string();
        cx.spawn_in(window, async move |this, cx| {
            let r = bg
                .spawn(async move {
                    if let Some(h) = handle {
                        let _ = h.join();
                    }
                    if let Err(e) = qur_git::delete_turn_refs(&root, &id) {
                        eprintln!("TURN {id} refs: {e}");
                    }
                    qur_worktree::remove(&root, &wt, &branch, true)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if let Err(e) = r {
                    this.flash(format!("Couldn't remove the worktree: {e}"), cx);
                }
                this.refresh_overlaps(cx);
                let next = this.session_order().into_iter().next();
                this.select(next, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Discard: stop the agent, remove its worktree and branch (asks first).
    pub(super) fn discard_task(&mut self, _: &qur::DiscardTask, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, wt, branch)) = self.selected_worktree(cx) else { return };
        if self.in_root(&id, cx) {
            // Works in the project folder: closing it stops it; files stay.
            let answer = ask(window, PromptLevel::Info, "Close this agent?", Some("It stops the agent. Its edits stay in your project folder."), &["Close Agent", "Cancel"], cx);
            cx.spawn_in(window, async move |this, cx| {
                if answer.await.ok() == Some(0) {
                    this.update_in(cx, |this, window, cx| {
                        this.remove_task(&id, window, cx);
                        let next = this.session_order().into_iter().next();
                        this.select(next, window, cx);
                    })
                    .ok();
                }
            })
            .detach();
            return;
        }
        let dirty = qur_worktree::is_dirty(&wt);
        let detail = format!(
            "This stops the agent and deletes its worktree and branch {branch}.{}",
            if dirty { " Its uncommitted changes will be lost." } else { "" }
        );
        let answer = ask(window, PromptLevel::Warning, "Discard this task?", Some(&detail), &["Discard", "Cancel"], cx);
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            let handle = this
                .update(cx, |this, cx| this.stop_task(&id, &wt, cx))
                .ok()
                .flatten();
            let removed = cx
                .background_executor()
                .spawn(async move {
                    if let Some(h) = handle {
                        let _ = h.join();
                    }
                    if let Err(e) = qur_git::delete_turn_refs(&root, &id) {
                        eprintln!("TURN {id} refs: {e}");
                    }
                    qur_worktree::remove(&root, &wt, &branch, true)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.save_tasks(cx);
                if let Err(e) = removed {
                    this.flash(format!("Couldn't remove the worktree: {e}"), cx);
                }
                let next = this.session_order().into_iter().next();
                this.select(next, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Rename Agent… (also double-click its button): the name sticks; an
    /// empty name goes back to following the agent.
    pub(super) fn rename_agent(&mut self, _: &qur::RenameAgent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Selected::Shell(tid)) = self.selected {
            return self.rename_shell(tid, window, cx);
        }
        let Some((id, ..)) = self.selected_worktree(cx) else {
            return self.flash("Select an agent first", cx);
        };
        let Some((_, _, current, _)) = self.task_parts(&id, cx) else { return };
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let input = cx.new(|cx| {
            let mut i = TextInput::new("Agent name (empty = let the agent name it)", cx);
            i.set_text(&current, cx);
            i
        });
        let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Confirm { .. } => {
                let name = input.read(cx).text.trim().to_string();
                this.close_overlays(window, cx);
                this.set_task_name(&id, name, cx);
            }
            InputEvent::Cancel => this.close_overlays(window, cx),
            _ => {}
        });
        self._subs.push(sub);
        let f = input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::Rename(input));
        cx.notify();
    }

    /// Rename for a shell under TERMINALS (double-click, or Rename with it selected).
    fn rename_shell(&mut self, tid: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self.terms.read(cx).session_by_id(tid).map(|s| if s.renamed { s.label.clone() } else { String::new() }) else { return };
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let input = cx.new(|cx| {
            let mut i = TextInput::new("Terminal name (empty = show what's running)", cx);
            i.set_text(&current, cx);
            i
        });
        let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Confirm { .. } => {
                let name = input.read(cx).text.trim().to_string();
                this.close_overlays(window, cx);
                this.terms.update(cx, |p, cx| p.rename(tid, &name, cx));
            }
            InputEvent::Cancel => this.close_overlays(window, cx),
            _ => {}
        });
        self._subs.push(sub);
        let f = input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::Rename(input));
        cx.notify();
    }

    fn set_task_name(&mut self, id: &str, name: String, cx: &mut Context<Self>) {
        let renamed = !name.is_empty();
        if let Some(a) = self.agent(id).cloned() {
            a.update(cx, |a, cx| {
                a.record.renamed = renamed;
                if renamed {
                    a.record.title = name.clone();
                }
                cx.emit(AgentEvent::Changed);
                cx.notify();
            });
        }
        if let Some(t) = self.term_tasks.iter_mut().find(|t| t.record.id == id) {
            t.record.renamed = renamed;
            if renamed {
                t.record.title = name;
            }
        }
        self.save_tasks(cx);
        cx.notify();
    }

    pub(super) fn new_agent_task(&mut self, _: &qur::NewAgentTask, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let input = cx.new(|cx| PromptInput::new("Describe the task…  (@ to mention a file, ⇧↩ for a new line)", self.root.clone(), cx));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &PromptEvent, window, cx| match ev {
            PromptEvent::Submit => {
                let Some(Overlay::NewTask { agent, plan, .. }) = &this.overlay else { return };
                let (agent, plan) = (*agent, *plan);
                let (text, mut mentions) = input.update(cx, |i, cx| i.take(cx));
                // A new task starts from text: attached files go as mentions,
                // a pasted image as a file in the temp folder.
                for a in input.update(cx, |i, _| i.take_attachments()) {
                    match (a.path, a.image) {
                        (Some(p), _) => mentions.push(p),
                        (None, Some(img)) => mentions.extend(pasted_image_file(&img)),
                        (None, None) => {}
                    }
                }
                this.overlay = None;
                this.prev_focus = None;
                this.start_task(agent, text, mentions, plan, window, cx);
            }
            PromptEvent::Cancel => {
                if this.mention_over_dialog.take().is_none() {
                    this.close_overlays(window, cx);
                }
                cx.notify();
            }
            PromptEvent::MentionRequested => {
                let root = this.root.clone();
                let target = input.clone();
                // The mention picker opens on top of the dialog.
                let files = this.files_in(&root);
                let items = Self::file_items(&files);
                let p = cx.new(|cx| Picker::new("Mention a file…", items, true, window, cx));
                let sub = cx.subscribe_in(&p, window, move |this, _, ev: &PickerEvent, window, cx| {
                    if let PickerEvent::Confirm(i) = ev {
                        target.update(cx, |t, cx| t.insert_mention(root.join(&files[*i]), cx));
                    }
                    if matches!(ev, PickerEvent::Confirm(_) | PickerEvent::Dismiss) {
                        this.mention_over_dialog = None;
                        let f = target.read(cx).focus.clone();
                        window.focus(&f, cx);
                        cx.notify();
                    }
                });
                this._subs.push(sub);
                let f = p.read(cx).input.read(cx).focus.clone();
                window.focus(&f, cx);
                this.mention_over_dialog = Some(p);
                cx.notify();
            }
            PromptEvent::Preview(p) => this.preview(p.clone(), window, cx),
            PromptEvent::Changed | PromptEvent::SlashRequested | PromptEvent::UpInEmpty => {}
        });
        self._subs.push(sub);
        let f = input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::NewTask { input, agent: 0, plan: false });
        cx.notify();
    }
}

/// The command line a terminal-mode agent runs in its PTY.
fn shell_command(def: &AgentDef) -> String {
    std::iter::once(def.command.as_str()).chain(def.args.iter().map(String::as_str)).map(shell_quote).collect::<Vec<_>>().join(" ")
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn first_line(text: &str) -> String {
    let l = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("task");
    let mut s: String = l.chars().take(80).collect();
    if l.chars().count() > 80 {
        s.push('…');
    }
    s
}

/// Notification tag for an agent: `qur:<window>:<task id>` (main.rs parses it).
fn alert_tag(window: &Window, id: &str) -> String {
    format!("qur:{}:{id}", window.window_handle().window_id().as_u64())
}

/// A pasted image saved where an agent can read it (`qur-paste-<ms>.png`).
fn pasted_image_file(img: &gpui::Image) -> Option<PathBuf> {
    let ext = img.format.extension();
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let path = std::env::temp_dir().join(format!("qur-paste-{ms}.{ext}"));
    std::fs::write(&path, img.bytes()).ok().map(|_| path)
}
