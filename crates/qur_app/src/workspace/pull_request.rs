//! Push and Open Pull Request… (Agent menu, the agent's right-click menu and
//! the Changes view): commit the task's leftover work, push its branch with
//! the user's own git, and open a pull request with their gh, or GitHub's
//! compare page in the browser. The git and gh work is `crate::pull_request`;
//! this is the UI around it. Remote branches are never deleted.

use super::*;
use crate::pull_request::{self as pr, Opened};

impl Workspace {
    pub(super) fn push_pull_request_action(&mut self, _: &qur::PushPullRequest, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected_worktree(cx) {
            Some((id, ..)) => self.push_pull_request(&id, window, cx),
            None => self.flash("Select an agent task to push", cx),
        }
    }

    pub(super) fn open_pull_request_action(&mut self, _: &qur::OpenPullRequest, _: &mut Window, cx: &mut Context<Self>) {
        match self.selected_worktree(cx).and_then(|(id, ..)| self.task_pr(&id, cx)) {
            Some(url) => self.open_external(&url, cx),
            None => self.flash("This task has no pull request yet: Agent > Push and Open Pull Request…", cx),
        }
    }

    fn task_pr(&self, id: &str, cx: &App) -> Option<String> {
        match self.agent(id) {
            Some(a) => a.read(cx).record.pr_url.clone(),
            None => self.term_task(id).and_then(|t| t.record.pr_url.clone()),
        }
    }

    /// Discard's dialog: a pull request and its pushed branch are left alone.
    pub(super) fn pr_discard_note(&self, id: &str, cx: &App) -> String {
        match self.task_pr(id, cx) {
            Some(url) => format!(" Its pull request ({url}) and the branch pushed to your remote stay; close or delete them there if you no longer want them."),
            None => String::new(),
        }
    }

    /// Ask first (it commits and pushes), then push and open. A task that
    /// already has a pull request pushes its new work to it instead.
    pub(super) fn push_pull_request(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((wt, branch, title, changes)) = self.task_parts(id, cx) else { return };
        if self.in_root(id, cx) {
            return self.flash("This agent works in your project folder: it has no task branch to push (use Source Control)", cx);
        }
        if changes.read(cx).pr_busy {
            return self.flash(format!("Already pushing {branch}"), cx);
        }
        let existing = self.task_pr(id, cx);
        let (message, detail, answers) = match &existing {
            Some(url) => (
                "This task already has a pull request",
                format!("{url}\n\nPush the task's new work to it? Uncommitted changes are committed first as “qur: {title}”."),
                vec!["Push New Commits", "Open Pull Request", "Cancel"],
            ),
            None => (
                "Push and open a pull request?",
                format!(
                    "The task's uncommitted changes are committed as “qur: {title}”, {branch} is pushed to your branch's remote (or origin) with your git, and a pull request is opened with gh, or on GitHub in your browser."
                ),
                vec!["Push and Open Pull Request", "Cancel"],
            ),
        };
        let answer = ask(window, PromptLevel::Info, message, Some(&detail), &answers, cx);
        let id = id.to_string();
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await.ok();
            this.update_in(cx, |this, window, cx| match (choice, existing) {
                (Some(0), existing) => this.run_pull_request(id, wt, branch, title, existing.is_some(), changes, window, cx),
                (Some(1), Some(url)) => this.open_external(&url, cx),
                _ => {}
            })
            .ok();
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn run_pull_request(&mut self, id: String, wt: PathBuf, branch: String, title: String, update: bool, changes: Entity<ChangesView>, window: &mut Window, cx: &mut Context<Self>) {
        // Like Merge: a working agent stops, so a half-done edit isn't committed.
        let prompts = match self.agent(&id).cloned() {
            Some(a) => {
                if matches!(a.read(cx).status, Status::Working | Status::NeedsYou) {
                    a.update(cx, |a, cx| a.stop(cx));
                }
                a.read(cx).user_prompts()
            }
            None => Vec::new(),
        };
        let root = self.root.clone();
        let bg = cx.background_executor().clone();
        self.pr_note(&changes, Some(format!("Pushing {branch}…")), false, true, cx);
        self.flash(format!("Pushing {branch}…"), cx);
        cx.spawn_in(window, async move |this, cx| {
            let pushed = {
                let (wt, branch) = (wt.clone(), branch.clone());
                bg.spawn(async move { pr::commit_and_push(&root, &wt, &branch, &title, &prompts, qur_agents::login_path()) }).await
            };
            let pushed = match pushed {
                Ok(p) => p,
                Err(e) => {
                    this.update(cx, |this, cx| this.pr_failed(&id, &changes, e, cx)).ok();
                    return;
                }
            };
            if update {
                this.update(cx, |this, cx| {
                    let msg = format!("Pushed {branch}: the pull request has the new work");
                    this.pr_note(&changes, Some(msg.clone()), false, false, cx);
                    this.flash(msg, cx);
                    this.after_push(&changes, cx);
                })
                .ok();
                return;
            }
            this.update(cx, |this, cx| {
                this.pr_note(&changes, Some(format!("Pushed {branch}; opening the pull request…")), false, true, cx);
                this.flash(format!("Pushed {branch}; opening the pull request…"), cx);
                this.after_push(&changes, cx);
            })
            .ok();
            let remote = pushed.target.remote.clone();
            let opened = bg.spawn(async move { pr::open(&wt, &pushed, qur_agents::login_path()) }).await;
            this.update(cx, |this, cx| match opened {
                Err(e) => this.pr_failed(&id, &changes, e, cx),
                Ok(Opened::Created(url) | Opened::Existing(url)) => this.pr_opened(&id, &changes, url, cx),
                Ok(Opened::Compare(url)) => {
                    let msg = format!("Pushed {branch}: finish the pull request on GitHub");
                    this.pr_note(&changes, Some(format!("{msg} ({url})")), false, false, cx);
                    this.flash_link(msg, url.clone(), cx);
                    this.open_external(&url, cx);
                }
                Ok(Opened::PushedOnly) => {
                    let msg = format!("Pushed {branch} to {remote}");
                    this.pr_note(&changes, Some(msg.clone()), false, false, cx);
                    this.flash(msg, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The commit of the leftover work changed the branch: views catch up.
    fn after_push(&mut self, changes: &Entity<ChangesView>, cx: &mut Context<Self>) {
        changes.update(cx, |c, cx| c.refresh(cx));
        if let Some(g) = &self.git {
            g.update(cx, |g, cx| g.refresh(cx));
        }
    }

    fn pr_note(&mut self, changes: &Entity<ChangesView>, note: Option<String>, error: bool, busy: bool, cx: &mut Context<Self>) {
        changes.update(cx, |c, cx| {
            c.pr_note = note.map(|n| (n, error));
            c.pr_busy = busy;
            cx.notify();
        });
    }

    fn pr_failed(&mut self, id: &str, changes: &Entity<ChangesView>, e: String, cx: &mut Context<Self>) {
        let first = e.lines().next().unwrap_or_default().to_string();
        self.pr_note(changes, Some(first.clone()), true, false, cx);
        self.flash(first, cx);
        if let Some(a) = self.agent(id).cloned() {
            a.update(cx, |a, cx| a.notice(format!("Pull request not opened: {e}"), cx));
        }
    }

    /// Record it on the task (so it isn't opened twice) and show it.
    fn pr_opened(&mut self, id: &str, changes: &Entity<ChangesView>, url: String, cx: &mut Context<Self>) {
        if let Some(a) = self.agent(id).cloned() {
            a.update(cx, |a, cx| {
                a.record.pr_url = Some(url.clone());
                a.notice_link("Opened a pull request for this task:", url.clone(), cx);
            });
        } else if let Some(t) = self.term_tasks.iter_mut().find(|t| t.record.id == id) {
            t.record.pr_url = Some(url.clone());
        }
        self.save_tasks(cx);
        changes.update(cx, |c, cx| {
            c.pr_url = Some(url.clone());
            c.pr_note = None;
            c.pr_busy = false;
            cx.notify();
        });
        self.flash_link(format!("Opened pull request {url}"), url, cx);
        self.sync_menus(cx);
    }

    /// A top-bar message that opens `url` when clicked; it stays a while longer.
    fn flash_link(&mut self, msg: String, url: String, cx: &mut Context<Self>) {
        self.flash(msg, cx);
        self.message_url = Some(url);
        let bg = cx.background_executor().clone();
        self.message_timer = Some(cx.spawn(async move |this, cx| {
            bg.timer(Duration::from_secs(20)).await;
            this.update(cx, |this, cx| {
                this.message = None;
                this.message_url = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// The default browser (scripted runs only log it).
    fn open_external(&self, url: &str, cx: &mut Context<Self>) {
        if std::env::var_os("QUR_DEBUG_ACTIONS").is_some() {
            println!("OPEN_URL {url}");
        } else {
            cx.open_url(url);
        }
    }
}
