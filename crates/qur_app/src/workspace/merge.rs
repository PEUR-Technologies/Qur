//! Merge, conflicts and the Git panel (decision 28).

use super::*;

impl Workspace {
    pub(super) fn on_changes_event(&mut self, id: &str, ev: &ChangesEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            ChangesEvent::Merge => self.merge_task(id, window, cx),
            ChangesEvent::PullRequest => self.push_pull_request(id, window, cx),
            ChangesEvent::Discard => {
                self.select(Some(Selected::TermTask(id.to_string())), window, cx);
                self.discard_task(&qur::DiscardTask, window, cx);
            }
            ChangesEvent::KeepWorking => {
                Docks::update(cx, dock::Kind::Terminal, |d| d.activate(dock::Panel::Session));
                self.select(Some(Selected::TermTask(id.to_string())), window, cx)
            }
            ChangesEvent::SendComments(text) => {
                // Terminal agents: type the review into the terminal (no Enter).
                if let Some(tid) = self.term_task(id).and_then(|t| t.term_id) {
                    let text = text.replace('\n', " ");
                    self.terms.update(cx, |p, cx| p.type_text(tid, &text, cx));
                }
                self.select(Some(Selected::TermTask(id.to_string())), window, cx);
            }
            ChangesEvent::Open(p, line) => {
                let at = line.map(|l| Point { line: (l as usize).saturating_sub(1), col: 0 });
                let p = p.clone();
                self.open_path_at(&p, at, window, cx);
            }
            ChangesEvent::Rewind(n) => self.rewind_turn(id, *n, window, cx),
        }
    }

    /// Worktree, branch, title and Changes view of a task by id.
    pub(super) fn task_parts(&self, id: &str, cx: &App) -> Option<(PathBuf, String, String, Entity<ChangesView>)> {
        if let Some(a) = self.agent(id) {
            let a = a.read(cx);
            return Some((a.record.worktree.clone(), a.record.branch.clone(), a.record.title.clone(), a.changes.clone()));
        }
        self.term_task(id).map(|t| (t.record.worktree.clone(), t.record.branch.clone(), t.record.title.clone(), t.changes.clone()))
    }

    /// Merge (handoff section 10, M0 recipe): commit the task's work on its
    /// branch, then merge it into the main checkout's current branch without
    /// touching the user's index or stash. Conflicts open in the editor.
    pub(super) fn merge_task(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((wt, branch, title, changes)) = self.task_parts(id, cx) else { return };
        if self.in_root(id, cx) {
            return self.flash("This agent works in your project folder: its changes are already there (commit them in Source Control)", cx);
        }
        if self.merge.is_some() {
            return self.flash("A merge is already in progress: finish or abort it first (Git panel)", cx);
        }
        if let Some(a) = self.agent(id).cloned()
            && matches!(a.read(cx).status, Status::Working | Status::NeedsYou)
        {
            a.update(cx, |a, cx| a.stop(cx));
        }
        let root = self.root.clone();
        let bg = cx.background_executor().clone();
        let id = id.to_string();
        self.flash(format!("Merging “{title}”…"), cx);
        cx.spawn_in(window, async move |this, cx| {
            let (t2, b2, root2) = (title.clone(), branch.clone(), root.clone());
            let outcome = bg
                .spawn(async move {
                    qur_git::commit_all(&wt, &format!("qur: {t2}"))?;
                    Ok::<_, String>(qur_git::merge_clean(&root2, &b2, &format!("Merge {b2}: {t2}")))
                })
                .await;
            this.update_in(cx, |this, window, cx| match outcome {
                Err(e) => this.flash(format!("Couldn't commit the task's work: {e}"), cx),
                Ok(qur_git::MergeOutcome::UpToDate) => this.flash("Nothing to merge: your branch already has these changes", cx),
                Ok(qur_git::MergeOutcome::Merged { commit }) => {
                    changes.update(cx, |c, cx| {
                        c.merged = true;
                        c.refresh(cx);
                    });
                    if let Some(g) = &this.git {
                        g.update(cx, |g, cx| g.refresh(cx));
                    }
                    // Its review was of work that's merged now: the reviewer goes.
                    if let Some(a) = this.agent(&id).cloned() {
                        a.update(cx, |a, cx| a.close_review(cx));
                    }
                    this.flash(format!("Merged “{title}” into your branch ({})", &commit[..commit.len().min(8)]), cx);
                    this.offer_cleanup(&id, window, cx);
                }
                Ok(qur_git::MergeOutcome::Refused { reason }) => {
                    drop(ask(window, PromptLevel::Warning, "Merge not started", Some(&reason), &["OK"], cx));
                }
                Ok(qur_git::MergeOutcome::Conflicts { files }) => {
                    let detail = format!(
                        "{} file{} conflict with your branch: {}.\n\nResolve them in Qur? Your checkout must have no uncommitted changes.",
                        files.len(),
                        if files.len() == 1 { "" } else { "s" },
                        files.iter().take(5).map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
                    );
                    let answer = ask(window, PromptLevel::Warning, "Merge conflicts", Some(&detail), &["Resolve in Qur", "Cancel"], cx);
                    let (root, branch, title, id) = (root.clone(), branch.clone(), title.clone(), id.clone());
                    cx.spawn_in(window, async move |this, cx| {
                        if answer.await.ok() != Some(0) {
                            return;
                        }
                        let started = {
                            let (r, b) = (root.clone(), branch.clone());
                            cx.background_executor().spawn(async move { qur_git::start_conflict_merge(&r, &b) }).await
                        };
                        this.update_in(cx, |this, window, cx| match started {
                            Err(e) => this.flash(e, cx),
                            Ok(files) => {
                                this.merge = Some(MergeState { task: id, title, branch, files: files.clone() });
                                this.sync_menus(cx);
                                if let Some(f) = files.first() {
                                    let abs = this.root.join(f);
                                    this.open_path(&abs, window, cx);
                                    if let Some(c) = &this.code {
                                        c.update(cx, |c, cx| {
                                            c.merge_conflicts = true;
                                            cx.notify();
                                        });
                                    }
                                }
                                if let Some(g) = &this.git {
                                    g.update(cx, |g, cx| g.refresh(cx));
                                }
                                cx.notify();
                            }
                        })
                        .ok();
                    })
                    .detach();
                }
            })
            .ok();
        })
        .detach();
    }

    /// After a merge: remove the task's worktree and branch now, or keep it.
    fn offer_cleanup(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let answer = ask(window, 
            PromptLevel::Info,
            "Merged. Remove the task?",
            Some("This deletes its worktree and branch (the changes are on your branch now)."),
            &["Remove task", "Keep it"],
            cx,
        );
        let id = id.to_string();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() == Some(0) {
                this.update_in(cx, |this, window, cx| this.remove_task(&id, window, cx)).ok();
            }
        })
        .detach();
    }

    fn finish_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = &self.merge else {
            return self.flash("No merge in progress", cx);
        };
        // Save any open conflicted files first.
        for b in self.buffers.dirty(cx) {
            let inside = b.read(cx).path().is_some_and(|p| m.files.iter().any(|f| self.root.join(f) == p));
            if inside {
                b.update(cx, |b, cx| b.save(cx));
            }
        }
        let (root, files, msg, id) = (self.root.clone(), m.files.clone(), format!("Merge {}: {}", m.branch, m.title), m.task.clone());
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = bg.spawn(async move { qur_git::finish_merge(&root, &files, &msg) }).await;
            this.update_in(cx, |this, window, cx| match r {
                Ok(sha) => {
                    let title = this.merge.take().map(|m| m.title).unwrap_or_default();
                    this.sync_menus(cx);
                    for b in &this.buffers.list {
                        b.update(cx, |b, _| b.merge_conflicts = false);
                    }
                    if let Some((_, _, _, changes)) = this.task_parts(&id, cx) {
                        changes.update(cx, |c, cx| {
                            c.merged = true;
                            c.refresh(cx);
                        });
                    }
                    if let Some(g) = &this.git {
                        g.update(cx, |g, cx| g.refresh(cx));
                    }
                    this.flash(format!("Merged “{title}” ({})", &sha[..sha.len().min(8)]), cx);
                    this.offer_cleanup(&id, window, cx);
                }
                Err(e) => this.flash(e, cx),
            })
            .ok();
        })
        .detach();
    }

    fn with_conflict_editor(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut EditorView, &mut Context<EditorView>)) {
        if self.merge.is_none() {
            return self.flash("No merge in progress", cx);
        }
        match self.code.clone().filter(|c| c.read(cx).merge_conflicts) {
            Some(c) => c.update(cx, f),
            None => self.flash("Open a conflicted file first (Git panel)", cx),
        }
    }

    pub(super) fn accept_ours(&mut self, _: &qur::AcceptOurs, _: &mut Window, cx: &mut Context<Self>) {
        self.with_conflict_editor(cx, |e, cx| e.resolve_current_conflict(qur_git::Resolution::Ours, cx));
    }

    pub(super) fn accept_theirs(&mut self, _: &qur::AcceptTheirs, _: &mut Window, cx: &mut Context<Self>) {
        self.with_conflict_editor(cx, |e, cx| e.resolve_current_conflict(qur_git::Resolution::Theirs, cx));
    }

    pub(super) fn accept_both(&mut self, _: &qur::AcceptBoth, _: &mut Window, cx: &mut Context<Self>) {
        self.with_conflict_editor(cx, |e, cx| e.resolve_current_conflict(qur_git::Resolution::Both, cx));
    }

    pub(super) fn next_conflict(&mut self, _: &qur::NextConflict, _: &mut Window, cx: &mut Context<Self>) {
        self.with_conflict_editor(cx, |e, cx| e.step_conflict(true, cx));
    }

    pub(super) fn prev_conflict(&mut self, _: &qur::PrevConflict, _: &mut Window, cx: &mut Context<Self>) {
        self.with_conflict_editor(cx, |e, cx| e.step_conflict(false, cx));
    }

    pub(super) fn finish_merge_action(&mut self, _: &qur::FinishMerge, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_merge(window, cx);
    }

    pub(super) fn abort_merge_action(&mut self, _: &qur::AbortMerge, _: &mut Window, cx: &mut Context<Self>) {
        if self.merge.is_none() {
            return self.flash("No merge in progress", cx);
        }
        self.abort_merge(cx);
    }

    fn abort_merge(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let r = bg.spawn(async move { qur_git::abort_merge(&root) }).await;
            let _ = this.update(cx, |this, cx| {
                this.merge = None;
                this.sync_menus(cx);
                for b in this.buffers.list.clone() {
                    b.update(cx, |b, cx| {
                        b.merge_conflicts = false;
                        b.disk_change(cx);
                    });
                }
                if let Some(g) = &this.git {
                    g.update(cx, |g, cx| g.refresh(cx));
                }
                match r {
                    Ok(()) => this.flash("Merge aborted; your branch is as it was", cx),
                    Err(e) => this.flash(e, cx),
                }
            });
        })
        .detach();
    }

    pub(super) fn git_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<GitPanel> {
        if let Some(g) = &self.git {
            return g.clone();
        }
        let root = self.root.clone();
        let g = cx.new(|cx| GitPanel::new(root, window, cx));
        let sub = cx.subscribe_in(&g, window, |this, _, ev: &GitEvent, window, cx| match ev {
            GitEvent::Open(p) => {
                let p = p.clone();
                this.open_path(&p, window, cx);
                if this.merge.as_ref().is_some_and(|m| m.files.iter().any(|f| this.root.join(f) == p))
                    && let Some(c) = &this.code
                {
                    c.update(cx, |c, cx| {
                        c.merge_conflicts = true;
                        cx.notify();
                    });
                }
            }
            GitEvent::FinishMerge => this.finish_merge(window, cx),
            GitEvent::AbortMerge => this.abort_merge(cx),
            GitEvent::Error(e) => this.flash(e.clone(), cx),
            GitEvent::Committed(sha) => this.flash(format!("Committed {}", &sha[..sha.len().min(8)]), cx),
        });
        self._subs.push(sub);
        self.git = Some(g.clone());
        g
    }

    pub(super) fn show_git(&mut self, _: &qur::GitPanel, window: &mut Window, cx: &mut Context<Self>) {
        let g = self.git_panel(window, cx);
        g.update(cx, |g, cx| g.refresh(cx));
        self.select(Some(Selected::Git), window, cx);
    }

    pub(super) fn git_stage_all(&mut self, _: &qur::GitStageAll, window: &mut Window, cx: &mut Context<Self>) {
        let g = self.git_panel(window, cx);
        g.update(cx, |g, cx| g.stage_everything(cx));
    }

    pub(super) fn git_unstage_all(&mut self, _: &qur::GitUnstageAll, window: &mut Window, cx: &mut Context<Self>) {
        let g = self.git_panel(window, cx);
        g.update(cx, |g, cx| g.unstage_everything(cx));
    }

    pub(super) fn git_commit(&mut self, _: &qur::GitCommit, window: &mut Window, cx: &mut Context<Self>) {
        let g = self.git_panel(window, cx);
        g.update(cx, |g, cx| g.refresh(cx));
        self.select(Some(Selected::Git), window, cx);
        g.update(cx, |g, cx| g.commit_or_focus(window, cx));
    }

    pub(super) fn merge_action(&mut self, _: &qur::Merge, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, ..)) = self.selected_worktree(cx) {
            self.merge_task(&id, window, cx);
        }
    }
}
