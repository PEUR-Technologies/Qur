//! Turn-end checks and Run Project in Worktree (decision 29).

use super::*;

impl Workspace {
    pub(super) fn selected_worktree(&self, cx: &App) -> Option<(String, PathBuf, String)> {
        match &self.selected {
            Some(Selected::Agent(id)) => self.agent(id).map(|a| {
                let r = &a.read(cx).record;
                (r.id.clone(), r.worktree.clone(), r.branch.clone())
            }),
            Some(Selected::TermTask(id)) => self.term_task(id).map(|t| (t.record.id.clone(), t.record.worktree.clone(), t.record.branch.clone())),
            _ => None,
        }
    }

    pub(super) fn run_checks_action(&mut self, _: &qur::RunChecks, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, ..)) = self.selected_worktree(cx) {
            self.run_checks(&id, true, window, cx);
        }
    }

    /// Run the project's `[checks] command` in a task's worktree (decision 29):
    /// after an ACP turn that changed files, or by hand (Run Checks Now, also
    /// for Agent Terminals, whose turns are only "busy, then quiet"). A failure goes back
    /// to the agent as text to review: the prompt (ACP) or the terminal's input
    /// line (Agent Terminal), not sent unless `auto_send` is on.
    pub(super) fn run_checks(&mut self, id: &str, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        let settings = ProjectSettings::load(&self.project_id).checks;
        let Some(command) = settings.command.clone().filter(|c| !c.trim().is_empty()) else {
            if manual {
                let path = qur_config::project_settings_path(&self.project_id);
                self.flash(format!("No checks set: add [checks] command = \"…\" to {}", path.display()), cx);
            }
            return;
        };
        if self.checks.get(id) == Some(&Check::Running) {
            return;
        }
        let Some((wt, ..)) = self.task_parts(id, cx) else { return };
        let agent = self.agent(id).cloned();
        let term = self.term_task(id).and_then(|t| t.term_id);
        self.checks.insert(id.to_string(), Check::Running);
        if let Some(a) = &agent {
            a.update(cx, |a, cx| a.notice(format!("Running checks: {command}"), cx));
        }
        cx.notify();
        let id = id.to_string();
        let bg = cx.background_executor().clone();
        let timeout = Duration::from_secs(settings.timeout.max(1));
        cx.spawn_in(window, async move |this, cx| {
            let path = qur_agents::login_path().to_string();
            let (ok, out) = {
                let command = command.clone();
                bg.spawn(async move { qur_worktree::run_check(&wt, &command, &path, timeout) }).await
            };
            this.update_in(cx, |this, _, cx| {
                this.checks.insert(id.clone(), if ok { Check::Passed } else { Check::Failed });
                let repair = format!("Checks failed (`{command}`):\n```\n{}\n```\nPlease fix this.", out.trim_end());
                if let Some(a) = &agent {
                    a.update(cx, |a, cx| {
                        if !out.trim().is_empty() {
                            a.output(out.clone(), cx);
                        }
                        if ok {
                            a.notice("Checks passed", cx);
                        } else if settings.auto_send && matches!(a.status, Status::Done | Status::Idle) {
                            a.send(repair.clone(), Vec::new(), cx);
                        } else {
                            a.prefill(&repair, cx);
                        }
                    });
                } else if let Some(tid) = term.filter(|_| !ok) {
                    this.terms.update(cx, |p, cx| p.type_text(tid, &repair, cx));
                }
                this.flash(if ok { format!("Checks passed: {command}") } else { format!("Checks failed: {command}") }, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Run Project (decision 29): start the project's dev server in the
    /// selected task's worktree on a free port, under TERMINALS as
    /// "run · <task>", then open its URL in that agent's Browser. Localhost
    /// only. Running it again shows
    /// the live session.
    pub(super) fn run_project(&mut self, _: &qur::RunProject, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, wt, _)) = self.selected_worktree(cx) else {
            return self.flash("Select an agent first", cx);
        };
        if let Some(&tid) = self.runs.get(&id)
            && self.terms.read(cx).session_by_id(tid).is_some()
        {
            return self.select(Some(Selected::Shell(tid)), window, cx);
        }
        let run = ProjectSettings::load(&self.project_id).run;
        let Some(command) = run.command.filter(|c| !c.trim().is_empty()) else {
            let path = qur_config::project_settings_path(&self.project_id);
            return self.flash(format!("No run command: add [run] command = \"…\" and url = \"http://localhost:$PORT\" to {}", path.display()), cx);
        };
        let Some(port) = std::net::TcpListener::bind(("127.0.0.1", 0)).ok().and_then(|l| l.local_addr().ok()).map(|a| a.port()) else {
            return self.flash("Couldn't find a free port", cx);
        };
        let url = run.url.map(|u| u.replace("$PORT", &port.to_string()));
        let target = match url.as_deref().map(local_port) {
            None => None,
            Some(Some(p)) => Some(p),
            Some(None) => return self.flash("[run] url must be on localhost", cx),
        };
        let title = self.task_parts(&id, cx).map(|(_, _, t, _)| t).unwrap_or_default();
        let kind = Kind::Agent { task: format!("run · {title}"), command: format!("export PORT={port}; {command}") };
        let label = format!("run · {title}");
        let Some(tid) = self.terms.update(cx, |p, cx| p.open(&wt, kind, label, cx)) else { return };
        self.shells.push(tid);
        self.runs.insert(id.clone(), tid);
        let (Some(url), Some(target)) = (url, target) else {
            return self.select(Some(Selected::Shell(tid)), window, cx);
        };
        self.flash(format!("Starting {title} on port {port}…"), cx);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let up = bg
                .spawn(async move {
                    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], target));
                    let start = Instant::now();
                    while start.elapsed() < Duration::from_secs(30) {
                        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
                            return true;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    false
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if !up {
                    this.flash(format!("Nothing answered on {url} after 30 s; see the run terminal"), cx);
                    return this.select(Some(Selected::Shell(tid)), window, cx);
                }
                match this.agent(&id).cloned() {
                    Some(a) => {
                        a.update(cx, |a, cx| {
                            a.browser.update(cx, |b, cx| b.navigate(&url, cx));
                            a.show_browser(true, cx);
                        });
                        this.select(Some(Selected::Agent(id.clone())), window, cx);
                        // Start loading now rather than on the next frame; the
                        // render pass keeps `shown` in sync from here on.
                        if this.overlay.is_none() && this.code.is_none() {
                            a.update(cx, |a, cx| a.browser.update(cx, |b, cx| b.set_shown(true, window, cx)));
                        }
                    }
                    None => {
                        let Some(browser) = this.term_task(&id).map(|t| t.browser.clone()) else { return };
                        browser.update(cx, |b, cx| b.navigate(&url, cx));
                        Docks::update(cx, dock::Kind::Terminal, |d| d.activate(dock::Panel::Browser));
                        this.select(Some(Selected::TermTask(id.clone())), window, cx);
                        if this.overlay.is_none() && this.code.is_none() {
                            browser.update(cx, |b, cx| b.set_shown(true, window, cx));
                        }
                    }
                }
                this.flash(format!("Running at {url}"), cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn reveal_worktree(&mut self, _: &qur::RevealWorktree, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, wt, _)) = self.selected_worktree(cx) {
            let _ = std::process::Command::new("open").arg(&wt).spawn();
        }
    }

    pub(super) fn worktree_terminal(&mut self, _: &qur::OpenWorktreeInTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, wt, _)) = self.selected_worktree(cx) {
            self.new_terminal_in(wt, window, cx);
        }
    }
}

/// The port of a localhost URL (`http://localhost:5173/…`, `127.0.0.1`,
/// `[::1]`), or None for anything else: Run Project never opens remote URLs.
fn local_port(url: &str) -> Option<u16> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let (host, port) = authority.rsplit_once(':')?;
    if !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return None;
    }
    port.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::local_port;

    #[test]
    fn run_project_urls_are_local_only() {
        assert_eq!(local_port("http://localhost:5173"), Some(5173));
        assert_eq!(local_port("http://127.0.0.1:8080/app?x=1"), Some(8080));
        assert_eq!(local_port("localhost:3000"), Some(3000));
        assert_eq!(local_port("http://[::1]:4000/"), Some(4000));
        assert_eq!(local_port("https://example.com:443"), None);
        assert_eq!(local_port("http://localhost.evil.com:80"), None);
        assert_eq!(local_port("http://localhost"), None);
    }
}
