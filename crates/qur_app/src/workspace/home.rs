//! The empty window (no project) and plain folders (no git).
//!
//! The empty window opens when Qur has no project to show: launched without
//! one, the last one gone, `qur ~`, the last project window closed, or File >
//! New Window. Its stage lists recent projects and what you can start from
//! there; terminals work in the home folder, files open in the code modal,
//! and agents ask for the folder to work in (it opens as a project). A folder
//! without git opens as a project too; agents that need their own worktree
//! offer to initialize git first.

use super::*;

/// Recent projects listed in the empty window.
const RECENT_SHOWN: usize = 8;

impl Workspace {
    /// An empty window nothing depends on (no terminals, no unsaved files):
    /// reused for File > New Window, closed once a project opens.
    pub fn idle_home(&self, cx: &App) -> bool {
        self.home && self.shells.is_empty() && self.buffers.dirty(cx).is_empty()
    }

    /// The terminals, for the project this empty window is becoming (main.rs
    /// `become_project`). None when it isn't one, or has unsaved files (they'd be lost).
    pub fn hand_over(&self, cx: &App) -> Option<Carry> {
        if !self.home || !self.buffers.dirty(cx).is_empty() {
            return None;
        }
        let shown = match self.selected {
            Some(Selected::Shell(tid)) => Some(tid),
            _ => self.shells.first().copied(),
        };
        Some((self.terms.clone(), self.shells.clone(), shown))
    }

    /// A shell in the empty window cd'd into a project (a git work tree): the
    /// window becomes that project's, terminals and all.
    pub(super) fn follow_cd(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.home {
            return;
        }
        let panel = self.terms.read(cx);
        let root = self
            .shells
            .iter()
            .filter_map(|&tid| crate::terminal::cwd_of(panel.session_by_id(tid)?.pid()))
            .filter_map(|d| d.ancestors().find(|a| a.join(".git").exists()).map(Path::to_path_buf))
            .find(|r| !crate::too_broad(r));
        let (Some(root), Some(handle)) = (root, window.window_handle().downcast::<Workspace>()) else { return };
        cx.defer(move |cx| _ = crate::become_project(handle, root, None, cx));
    }

    /// A message in the top bar (main.rs: why the empty window opened).
    pub fn note(&mut self, msg: String, cx: &mut Context<Self>) {
        self.flash(msg, cx);
    }

    /// Recent projects that still exist, re-read when the window comes forward.
    pub(super) fn refresh_recent(&mut self) {
        if self.home {
            self.recent = qur_config::State::load().recent_projects.into_iter().filter(|p| p.is_dir()).take(RECENT_SHOWN).collect();
        }
    }

    /// File > New Agent Terminal in the empty window: pick the folder, open
    /// it as a project, and start the agent there (in the folder itself,
    /// so a folder without git works too).
    pub(super) fn home_new_agent_terminal(&mut self, _: &qur::NewAgentTerminal, _: &mut Window, cx: &mut Context<Self>) {
        crate::pick_folder("Start Agent Here", |p, cx| crate::open_project_then(p, Some(Box::new(|ws, window, cx| ws.agent_terminal_here(window, cx))), cx), cx);
    }

    /// File > New Agent Task… in the empty window: pick the folder, then the
    /// task dialog in that project's window.
    pub(super) fn home_new_agent_task(&mut self, _: &qur::NewAgentTask, _: &mut Window, cx: &mut Context<Self>) {
        crate::pick_folder("Choose Project", |p, cx| {
            crate::open_project_then(
                p,
                Some(Box::new(|ws, window, cx| {
                    if ws.home {
                        return ws.flash("Choose a project folder, not your home folder", cx);
                    }
                    ws.new_agent_task(&qur::NewAgentTask, window, cx)
                })),
                cx,
            )
        }, cx);
    }

    fn agent_terminal_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.home {
            return self.flash("Choose a project folder, not your home folder", cx);
        }
        self.new_agent_terminal(&qur::NewAgentTerminal, window, cx);
    }

    /// Agents that need their own worktree in a folder without git: offer to
    /// make it a repository (`git init` and a first commit of its files), then
    /// carry on with `then`. True when it asked (the caller stops there).
    pub(super) fn offer_git(&mut self, then: fn(&mut Self, &mut Window, &mut Context<Self>), window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.repo {
            return false;
        }
        let msg = format!("{} isn't a git repository", self.project_name());
        let detail = "Agents work in their own git worktree, so this needs git. Initialize Git here and commit the folder's files (minus any .gitignore'd) as the first commit?\n\nThe first Agent Terminal works in the folder itself without git.";
        let answer = ask(window, PromptLevel::Info, &msg, Some(detail), &["Initialize Git", "Cancel"], cx);
        let root = self.root.clone();
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            let done = bg.spawn(async move { qur_worktree::init_repo(&root) }).await;
            this.update_in(cx, |this, window, cx| match done {
                Ok(()) => {
                    this.became_repo(cx);
                    this.flash("Initialized Git with a first commit", cx);
                    then(this, window, cx);
                }
                Err(e) => this.flash(format!("Couldn't initialize Git: {e}"), cx),
            })
            .ok();
        })
        .detach();
        true
    }

    /// The folder is a repository now: git features come on, and its settings,
    /// tasks and open files move to the id a repository gets (its first commit).
    fn became_repo(&mut self, cx: &mut Context<Self>) {
        self.repo = true;
        let id = qur_config::project_id(&self.root);
        if id != self.project_id {
            let moves = [
                (qur_config::project_settings_path(&self.project_id), qur_config::project_settings_path(&id)),
                (qur_config::project_state_path(&self.project_id), qur_config::project_state_path(&id)),
                (qur_agents::tasks_path(&self.project_id), qur_agents::tasks_path(&id)),
            ];
            for (from, to) in moves {
                if from.exists() && !to.exists() {
                    std::fs::rename(&from, &to).ok();
                }
            }
            self.project_id = id;
        }
        self.sync_menus(cx);
        cx.notify();
    }

    /// The empty window's stage: what to start, and recent projects.
    pub(super) fn home_stage(&self, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let ui = &look.theme.ui;
        let header = |label: &'static str| {
            div()
                .px(look.z(8.))
                .pt(look.z(16.))
                .pb(look.z(4.))
                .text_size(look.text(-2.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(look.c(ui.text_muted))
                .child(label)
        };
        let row = |id: &'static str, label: &'static str, keys: &str| {
            div()
                .id(id)
                .relative()
                .children(crate::script::mark(|| format!("home:{id}")))
                .flex()
                .justify_between()
                .gap(look.z(32.))
                .px(look.z(8.))
                .py(look.z(4.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .child(div().text_color(look.c(ui.text)).child(label))
                .child(div().text_color(look.c(ui.text_muted)).child(commands::display_keys(keys)))
        };
        let home = std::env::var("HOME").unwrap_or_default();
        let recent = self.recent.iter().enumerate().map(|(i, p)| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let path = p.to_string_lossy().replacen(&home, "~", 1);
            let p = p.clone();
            div()
                .id(("recent", i))
                .relative()
                .children(crate::script::mark(|| format!("home:recent:{name}")))
                .flex()
                .items_baseline()
                .gap(look.z(10.))
                .px(look.z(8.))
                .py(look.z(4.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .child(div().flex_none().text_color(look.c(ui.text)).child(name))
                .child(div().min_w(px(0.)).truncate().text_size(look.text(-1.)).text_color(look.c(ui.text_muted)).child(path))
                .on_click(cx.listener(move |_, _, _, cx| crate::open_project(p.clone(), None, cx)))
        });
        let more = self.recent.len() >= RECENT_SHOWN;
        let column = div()
            .w(look.z(440.))
            .max_w_full()
            .flex()
            .flex_col()
            .child(header("START"))
            .child(row("open-folder", "Open Folder…", "cmd-o").on_click(cx.listener(|this, _, window, cx| this.open_folder(&qur::OpenFolder, window, cx))))
            .child(row("new-terminal", "New Terminal", "cmd-t").on_click(cx.listener(|this, _, window, cx| this.new_terminal(&qur::NewTerminal, window, cx))))
            .child(
                row("new-agent-terminal", "New Agent Terminal in…", "cmd-shift-t")
                    .on_click(cx.listener(|this, _, window, cx| this.home_new_agent_terminal(&qur::NewAgentTerminal, window, cx))),
            )
            .child(row("new-agent-task", "New Agent Task in…", "cmd-shift-n").on_click(cx.listener(|this, _, window, cx| this.home_new_agent_task(&qur::NewAgentTask, window, cx))))
            .when(!self.recent.is_empty(), |d| d.child(header("RECENT PROJECTS")).children(recent))
            .when(more, |d| d.child(row("switch-project", "More…", "cmd-alt-o").on_click(cx.listener(|this, _, window, cx| this.project_switcher(&qur::ProjectSwitcher, window, cx)))));
        look.tile(div(), ui.editor_bg)
            .id("home")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .overflow_y_scroll()
            .p(look.z(16.))
            .text_size(look.text(0.))
            .child(column)
            .into_any_element()
    }
}
