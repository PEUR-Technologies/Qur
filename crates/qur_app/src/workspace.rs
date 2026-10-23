//! One window = one project (decisions 18, 19). Left column: AGENTS (ACP
//! tasks and Agent Terminals), TERMINALS (shells) and FILES (the tree of the
//! selected session's worktree, or the main checkout). The stage shows the
//! selected session. Code opens in a modal over the window; Esc closes it.
//! Also applies the project's look (theme, transparency; decisions 8, 13).

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, Entity, FocusHandle, MouseButton, Pixels, PromptLevel,
    SharedString, Subscription, Task, Window, WindowAppearance, canvas, div, prelude::*, px, svg,
};
use qur_acp::PermissionKind;
use qur_agents::{AgentDef, Mode, Roster, TaskRecord, TaskStore};
use qur_config::{Config, ProjectSettings, Value};
use qur_editor::Point;
use qur_theme::{Registry, Rgba, ThemeChoice};

use crate::agent_view::{AgentEvent, AgentTask, Layout, Status, View};
use crate::live::{LiveEvent, LivePane};
use crate::monitor::{MonitorEvent, TaskMonitor};
use crate::assets::{self, icon};
use crate::commands::{self, MenuState, agent as aa, edit, editor as ea, qur};
use crate::editor_view::{Buffers, EditorEvent, EditorView};
use crate::input::{InputEvent, TextInput};
use crate::look::Look;
use crate::picker::{PickItem, Picker, PickerEvent};
use crate::prompt::{PromptEvent, PromptInput};
use crate::terminal::{Kind, PanelEvent, TerminalPanel};
use crate::tree_view::{TreeEvent, TreeView};

/// Height of our own top bar (the system titlebar is transparent underneath).
pub const TITLEBAR_H: f32 = 32.0;
/// Left inset that clears the traffic lights placed at (9, 9).
const TRAFFIC_LIGHTS_W: f32 = 78.0;
const SIDEBAR_DEFAULT_W: f32 = 280.0;
const SIDEBAR_MIN_W: f32 = 200.0;
const SIDEBAR_MAX_W: f32 = 560.0;

/// A terminal-mode agent task (New Agent Terminal, `mode = "terminal"`).
/// Heat and Live come from the file watcher only (no ACP).
struct TermTask {
    record: TaskRecord,
    term_id: Option<usize>,
    accent: Rgba,
    monitor: Entity<TaskMonitor>,
    live: Entity<LivePane>,
    /// Layout B: show Live instead of the terminal.
    show_live: bool,
}

/// What the stage shows.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Selected {
    Agent(String),
    TermTask(String),
    Shell(usize),
}

enum PickKind {
    Commands(Vec<&'static commands::Cmd>),
    /// Open a file (relative to `root`) in the code modal.
    Files { root: PathBuf, files: Vec<PathBuf> },
    /// Insert an `@file` mention into a prompt.
    Mention { root: PathBuf, files: Vec<PathBuf>, target: Entity<PromptInput> },
    Themes { names: Vec<String>, project_scope: bool },
    Projects(Vec<PathBuf>),
    /// The agent's slash commands, inserted into its prompt.
    Slash { names: Vec<String>, target: Entity<PromptInput> },
}

enum Overlay {
    Picker(Entity<Picker>, PickKind),
    GoTo(Entity<TextInput>),
    Transparency { project_scope: bool },
    NewTask { input: Entity<PromptInput>, agent: usize },
}

pub struct Workspace {
    focus: FocusHandle,
    pub root: PathBuf,
    project_id: String,
    config: Config,
    project: ProjectSettings,
    system_dark: bool,
    /// Theme being previewed in the picker (not saved yet).
    preview_theme: Option<String>,
    roster: Roster,
    agents: Vec<Entity<AgentTask>>,
    term_tasks: Vec<TermTask>,
    /// Plain shells (terminal ids), in creation order.
    shells: Vec<usize>,
    terms: Entity<TerminalPanel>,
    selected: Option<Selected>,
    /// Sessions that finished, errored or need you while not selected (Orca's unread).
    unread: HashSet<String>,
    /// One file tree per root (main checkout and each worktree).
    trees: HashMap<PathBuf, Entity<TreeView>>,
    buffers: Buffers,
    /// The code modal (decision 18).
    code: Option<Entity<EditorView>>,
    code_prev_focus: Option<FocusHandle>,
    sidebar: bool,
    sidebar_w: f32,
    resizing: bool,
    overlay: Option<Overlay>,
    /// File picker shown over the New Agent Task dialog (`@` mentions).
    mention_over_dialog: Option<Entity<Picker>>,
    prev_focus: Option<FocusHandle>,
    /// Back/forward history of files opened in the modal: (file, cursor).
    nav: Vec<(PathBuf, Point)>,
    nav_ix: usize,
    message: Option<(SharedString, Instant)>,
    message_timer: Option<Task<()>>,
    file_lists: HashMap<PathBuf, Vec<PathBuf>>,
    layout: Layout,
    _root_watcher: Option<qur_watch::Watcher>,
    slider: Rc<Cell<Bounds<Pixels>>>,
    sliding: bool,
    title_drag: bool,
    first_frame: bool,
    _subs: Vec<Subscription>,
}

impl Workspace {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::mark("ws_new");
        let (config, err) = Config::load();
        let project_id = qur_config::project_id(&root);
        crate::mark("project_id");
        let project = ProjectSettings::load(&project_id);
        let (roster, roster_err) = Roster::load();
        let terms = cx.new(TerminalPanel::new);
        let subs = vec![
            cx.subscribe_in(&terms, window, |this, _, ev: &PanelEvent, window, cx| {
                if let PanelEvent::Closed = ev {
                    this.prune_closed_terms(window, cx);
                }
                cx.notify();
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                this.system_dark = is_dark(window.appearance());
                this.apply_look(window, cx);
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.sync_menus(cx);
                    // Pick up files created outside Qur (the M3 watcher replaces this).
                    for t in this.trees.values() {
                        t.update(cx, |t, cx| t.refresh(cx));
                    }
                    this.file_lists.clear();
                }
            }),
        ];
        let mut state = qur_config::State::load();
        state.touch_project(&root);
        state.save().ok();
        crate::mark("state_saved");
        let mut ws = Workspace {
            focus: cx.focus_handle(),
            root: root.clone(),
            project_id,
            config,
            project,
            system_dark: is_dark(window.appearance()),
            preview_theme: None,
            roster,
            agents: Vec::new(),
            term_tasks: Vec::new(),
            shells: Vec::new(),
            terms,
            selected: None,
            unread: HashSet::new(),
            trees: HashMap::new(),
            buffers: Buffers { list: Vec::new() },
            code: None,
            code_prev_focus: None,
            sidebar: true,
            sidebar_w: SIDEBAR_DEFAULT_W,
            resizing: false,
            overlay: None,
            mention_over_dialog: None,
            prev_focus: None,
            nav: Vec::new(),
            nav_ix: 0,
            message: None,
            message_timer: None,
            file_lists: HashMap::new(),
            layout: Layout::A,
            _root_watcher: None,
            slider: Rc::new(Cell::new(Bounds::default())),
            sliding: false,
            title_drag: false,
            first_frame: false,
            _subs: subs,
        };
        ws.layout = Layout::parse(&ws.config.peek_layout);
        ws.tree_for(&root, window, cx);
        crate::mark("tree_loaded");
        ws.apply_look(window, cx);
        crate::mark("look_applied");
        ws.restore_tasks(window, cx);
        // Something must hold focus, or shortcuts have nowhere to go.
        window.focus(&ws.focus, cx);
        if let Some(e) = err.or(roster_err) {
            ws.flash(e, cx);
        }
        ws
    }

    pub fn project_name(&self) -> String {
        self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }

    // -- look (theme, transparency, zoom) -------------------------------------

    fn effective(&self) -> qur_config::Effective {
        qur_config::effective(&self.config, &self.project)
    }

    fn apply_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let eff = self.effective();
        let name = self.preview_theme.clone().unwrap_or(eff.theme);
        let choice = if name == qur_config::THEME_AUTO { ThemeChoice::Auto } else { ThemeChoice::Named(name) };
        let theme = Registry::global().resolve(&choice, self.system_dark);
        let min = theme.min_opacity().max(qur_theme::MIN_OPACITY);
        let code_font = if self.config.editor.font_family.is_empty() { default_code_font(cx) } else { self.config.editor.font_family.clone() };
        let look = Look {
            opacity: eff.opacity.clamp(min, 1.0),
            blur: eff.blur,
            zoom: self.config.ui.zoom,
            ui_font: assets::UI_FONT.into(),
            code_font: code_font.into(),
            ui_size: self.config.ui.font_size,
            code_size: self.config.editor.font_size,
            line_height: self.config.editor.line_height,
            theme: theme.clone(),
        };
        window.set_background_appearance(look.background_appearance());
        Look::set(window, look, cx);
        // Agent accents follow the theme's palette (unless agents.toml overrides).
        let n = theme.accents.len();
        let mut slot = 0;
        for a in &self.agents {
            let def = a.read(cx).def.clone();
            let accent = accent_for(&def, &theme.accents[slot % n]);
            a.update(cx, |a, cx| a.set_accent(accent, cx));
            slot += 1;
        }
        for t in &mut self.term_tasks {
            let def = self.roster.get(&t.record.agent).cloned();
            t.accent = def.map(|d| accent_for(&d, &theme.accents[slot % n])).unwrap_or(theme.accents[slot % n].base);
            let accent = t.accent;
            t.monitor.update(cx, |m, _| m.accent = accent);
            t.live.update(cx, |l, _| l.accent = accent);
            slot += 1;
        }
        window.refresh();
        cx.notify();
    }

    /// Re-read config.toml and this project's settings.
    pub fn reload_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (config, err) = Config::load();
        self.config = config;
        self.project = ProjectSettings::load(&self.project_id);
        if let Some(e) = err {
            self.flash(e, cx);
        }
        let wrap = self.config.editor.soft_wrap;
        for b in &self.buffers.list {
            b.update(cx, |b, cx| b.set_soft_wrap(wrap, cx));
        }
        self.apply_look(window, cx);
        self.sync_menus(cx);
    }

    fn write_setting(&mut self, project_scope: bool, key: &str, value: Option<Value>, cx: &mut Context<Self>) {
        let path = if project_scope { qur_config::project_settings_path(&self.project_id) } else { qur_config::config_path() };
        if let Err(e) = qur_config::write_key(&path, key, value) {
            self.flash(format!("Couldn't save {}: {e}", path.display()), cx);
        }
    }

    /// Tell every other window to re-read settings (a global value changed).
    fn broadcast_reload(&self, cx: &mut Context<Self>) {
        let me = cx.entity();
        for handle in cx.windows() {
            if let Some(ws) = handle.downcast::<Workspace>() {
                let _ = ws.update(cx, |ws, window, cx| {
                    if cx.entity() != me {
                        ws.reload_settings(window, cx);
                    }
                });
            }
        }
    }

    fn set_zoom(&mut self, z: f32, window: &mut Window, cx: &mut Context<Self>) {
        let z = (z * 10.).round() / 10.;
        self.config.ui.zoom = z.clamp(qur_config::MIN_ZOOM, qur_config::MAX_ZOOM);
        self.write_setting(false, "ui.zoom", Some(self.config.ui.zoom.into()), cx);
        self.apply_look(window, cx);
        self.broadcast_reload(cx);
    }

    fn flash(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.message = Some((msg.into(), Instant::now()));
        let bg = cx.background_executor().clone();
        self.message_timer = Some(cx.spawn(async move |this, cx| {
            bg.timer(Duration::from_secs(6)).await;
            this.update(cx, |this, cx| {
                this.message = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn menu_state(&self, cx: &App) -> MenuState {
        MenuState {
            layout: self.layout.as_str(),
            follow: self.selected_live(cx).map(|l| l.read(cx).follow).unwrap_or(true),
            sidebar: self.sidebar,
            soft_wrap: self.code.as_ref().map(|e| e.read(cx).soft_wrap()).unwrap_or(self.config.editor.soft_wrap),
            recent: qur_config::State::load().recent_projects,
        }
    }

    pub fn sync_menus(&self, cx: &mut Context<Self>) {
        crate::set_menus(self.menu_state(cx), cx);
    }

    // -- sessions ----------------------------------------------------------------

    fn tree_for(&mut self, root: &Path, window: &mut Window, cx: &mut Context<Self>) -> Entity<TreeView> {
        if let Some(t) = self.trees.get(root) {
            return t.clone();
        }
        let t = cx.new(|_| TreeView::new(root));
        let sub = cx.subscribe_in(&t, window, |this, _, ev: &TreeEvent, window, cx| match ev {
            TreeEvent::Open(p) => this.open_path(p, window, cx),
        });
        self._subs.push(sub);
        // A task worktree's tree is that task's heatmap.
        let monitor = self
            .agents
            .iter()
            .map(|a| a.read(cx).monitor.clone())
            .chain(self.term_tasks.iter().map(|t| t.monitor.clone()))
            .find(|m| m.read(cx).worktree == root);
        if let Some(m) = monitor {
            t.update(cx, |t, cx| t.set_monitor(m, cx));
        }
        self.trees.insert(root.to_path_buf(), t.clone());
        self.refresh_overlaps(cx);
        t
    }

    /// The main checkout's tree shows dots for files any task changed.
    fn refresh_overlaps(&self, cx: &mut Context<Self>) {
        let monitors: Vec<_> = self.agents.iter().map(|a| a.read(cx).monitor.clone()).chain(self.term_tasks.iter().map(|t| t.monitor.clone())).collect();
        if let Some(t) = self.trees.get(&self.root) {
            t.update(cx, |t, cx| {
                t.overlaps = monitors;
                cx.notify();
            });
        }
    }

    /// The folder the Files tree, Quick Open and mentions work in.
    fn current_root(&self, cx: &App) -> PathBuf {
        match &self.selected {
            Some(Selected::Agent(id)) => self.agent(id).map(|a| a.read(cx).record.worktree.clone()).unwrap_or(self.root.clone()),
            Some(Selected::TermTask(id)) => self.term_task(id).map(|t| t.record.worktree.clone()).unwrap_or(self.root.clone()),
            _ => self.root.clone(),
        }
    }

    fn agent(&self, id: &str) -> Option<&Entity<AgentTask>> {
        self.agents.iter().find(|a| agent_id_matches(a, id))
    }

    fn term_task(&self, id: &str) -> Option<&TermTask> {
        self.term_tasks.iter().find(|t| t.record.id == id)
    }

    fn selected_agent(&self) -> Option<Entity<AgentTask>> {
        match &self.selected {
            Some(Selected::Agent(id)) => self.agent(id).cloned(),
            _ => None,
        }
    }

    /// Every session in sidebar order (for Ctrl+1..9 and next/previous).
    fn session_order(&self) -> Vec<Selected> {
        let mut v: Vec<Selected> = self.agent_ids().into_iter().map(Selected::Agent).collect();
        v.extend(self.term_tasks.iter().map(|t| Selected::TermTask(t.record.id.clone())));
        v.extend(self.shells.iter().map(|&s| Selected::Shell(s)));
        v
    }

    fn agent_ids(&self) -> Vec<String> {
        AGENT_IDS.with(|m| self.agents.iter().filter_map(|a| m.borrow().get(&a.entity_id()).cloned()).collect())
    }

    fn select(&mut self, s: Option<Selected>, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = s;
        match &self.selected {
            Some(Selected::Agent(id)) | Some(Selected::TermTask(id)) => {
                self.unread.remove(id);
            }
            _ => {}
        }
        let root = self.current_root(cx);
        self.tree_for(&root, window, cx);
        match &self.selected {
            Some(Selected::Agent(id)) => {
                if let Some(a) = self.agent(id).cloned() {
                    // A stopped task with a saved session: start it so its
                    // history loads. No prompt is sent.
                    a.update(cx, |a, cx| {
                        if a.status == Status::Stopped && a.record.session_id.is_some() {
                            a.start(cx);
                        }
                    });
                    let f = a.read(cx).prompt.read(cx).focus.clone();
                    window.focus(&f, cx);
                }
            }
            Some(Selected::TermTask(id)) => {
                let id = id.clone();
                self.show_term_task(&id, window, cx);
            }
            Some(Selected::Shell(tid)) => {
                let tid = *tid;
                self.terms.update(cx, |p, cx| p.activate_id(tid, cx));
                let f = self.terms.read(cx).focus.clone();
                window.focus(&f, cx);
            }
            None => window.focus(&self.focus, cx),
        }
        self.sync_menus(cx);
        cx.notify();
    }

    fn restore_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn add_term_task(&mut self, record: TaskRecord, window: &mut Window, cx: &mut Context<Self>) {
        let accent = Look::of(window, cx).theme.accents[0].base;
        let monitor = cx.new(|cx| {
            let mut m = TaskMonitor::new(record.worktree.clone(), record.base.clone(), accent, cx);
            // No turns in a terminal: every write counts as the agent's edit.
            m.set_turn_active(true);
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
        self.term_tasks.push(TermTask { record, term_id: None, accent, monitor, live, show_live: false });
        self.refresh_overlaps(cx);
    }

    /// A task's watcher saw files change: refresh its tree and open copies.
    fn watch_monitor(&mut self, monitor: &Entity<TaskMonitor>, window: &mut Window, cx: &mut Context<Self>) {
        let sub = cx.subscribe_in(monitor, window, |this, m, ev: &MonitorEvent, window, cx| match ev {
            MonitorEvent::Files(paths) => {
                let root = m.read(cx).worktree.clone();
                let paths = paths.clone();
                this.files_changed(&root, &paths, window, cx);
            }
            MonitorEvent::Changed => cx.notify(),
        });
        self._subs.push(sub);
    }

    /// Watch the main checkout (FSEvents): tree structure and open buffers.
    fn watch_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                let known = tree.read(cx).tree.find(rel).is_some();
                known != p.exists()
            });
            if structural {
                tree.update(cx, |t, cx| t.refresh(cx));
            }
        }
        self.file_lists.remove(root);
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

    fn save_tasks(&self, cx: &App) {
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
        let a = cx.new(|cx| AgentTask::new(record, def, accent, layout, wrap, tab, window, cx));
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
                if attention && this.selected != Some(Selected::Agent(id.clone())) {
                    this.unread.insert(id);
                }
                this.save_tasks(cx);
                cx.notify();
            }
            AgentEvent::SlashRequested => {
                let (cmds, target) = {
                    let a = a.read(cx);
                    (a.commands.clone(), a.prompt.clone())
                };
                if !cmds.is_empty() {
                    let names = cmds.iter().map(|c| c.name.clone()).collect();
                    let items = cmds.iter().map(|c| PickItem::new(format!("/{}", c.name)).detail(c.description.clone())).collect();
                    this.open_picker("Agent command…", items, PickKind::Slash { names, target }, false, window, cx);
                }
            }
            AgentEvent::OpenFile(p, line) => {
                let p = p.clone();
                let at = line.map(|l| Point { line: (l as usize).saturating_sub(1), col: 0 });
                this.open_path_at(&p, at, window, cx);
            }
            AgentEvent::MentionRequested => {
                let (root, target) = {
                    let a = a.read(cx);
                    (a.record.worktree.clone(), a.prompt.clone())
                };
                this.mention_picker(root, target, window, cx);
            }
        });
        self._subs.push(sub);
        self.agents.push(a.clone());
        self.refresh_overlaps(cx);
        a
    }

    /// New Agent Task: worktree, then spawn the agent with the task as its first prompt.
    fn start_task(&mut self, agent_ix: usize, text: String, mentions: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
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
                    };
                    let debug_browser = std::env::var_os("QUR_DEBUG_BROWSER_URL").is_some();
                    match def.mode {
                        Mode::Acp => {
                            let a = this.add_agent(record, def, window, cx);
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
    fn new_agent_terminal(&mut self, _: &qur::NewAgentTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.term_tasks.len() + 1;
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
    fn show_term_task(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.term_tasks.iter().position(|t| t.record.id == id) else { return };
        let alive = self.term_tasks[ix].term_id.is_some_and(|tid| self.terms.update(cx, |p, cx| p.activate_id(tid, cx)));
        if !alive {
            let t = &self.term_tasks[ix];
            let command = self.roster.get(&t.record.agent).filter(|d| d.mode == Mode::Terminal).map(shell_command).unwrap_or_else(|| "claude".into());
            let kind = Kind::Agent { task: t.record.title.clone(), command };
            let wt = t.record.worktree.clone();
            let label = t.record.title.clone();
            let tid = self.terms.update(cx, |p, cx| p.open(&wt, kind, label, cx));
            self.term_tasks[ix].term_id = tid;
        }
        let f = self.terms.read(cx).focus.clone();
        window.focus(&f, cx);
    }

    fn new_terminal_in(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let tid = self.terms.update(cx, |p, cx| {
            let label = p.label_for(&Kind::Shell);
            p.open(&dir, Kind::Shell, label, cx)
        });
        if let Some(tid) = tid {
            self.shells.push(tid);
            self.select(Some(Selected::Shell(tid)), window, cx);
        }
    }

    fn new_terminal(&mut self, _: &qur::NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.current_root(cx);
        self.new_terminal_in(dir, window, cx)
    }

    /// A terminal closed (exit or Cmd+W): forget shells; terminal tasks keep
    /// their row (the task persists) and restart when selected again.
    fn prune_closed_terms(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.terms.read(cx);
        let live: Vec<usize> = panel.sessions.iter().map(|s| s.id).collect();
        self.shells.retain(|s| live.contains(s));
        for t in &mut self.term_tasks {
            if t.term_id.is_some_and(|id| !live.contains(&id)) {
                t.term_id = None;
            }
        }
        let sel_gone = matches!(&self.selected, Some(Selected::Shell(s)) if !self.shells.contains(s));
        if sel_gone {
            let next = self.session_order().into_iter().last();
            self.select(next, window, cx);
        }
    }

    fn toggle_terminal(&mut self, _: &qur::ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        match self.shells.last().copied() {
            Some(s) => self.select(Some(Selected::Shell(s)), window, cx),
            None => self.new_terminal(&qur::NewTerminal, window, cx),
        }
    }

    fn close_terminal(&mut self, _: &qur::CloseTerminal, _: &mut Window, cx: &mut Context<Self>) {
        let tid = match &self.selected {
            Some(Selected::Shell(s)) => Some(*s),
            Some(Selected::TermTask(id)) => self.term_task(id).and_then(|t| t.term_id),
            _ => None,
        };
        if let Some(tid) = tid {
            self.terms.update(cx, |p, cx| {
                if let Some(ix) = p.sessions.iter().position(|s| s.id == tid) {
                    p.close(ix, cx);
                }
            });
        }
    }

    fn step_session(&mut self, d: isize, window: &mut Window, cx: &mut Context<Self>) {
        let order = self.session_order();
        if order.is_empty() {
            return;
        }
        let cur = self.selected.as_ref().and_then(|s| order.iter().position(|o| o == s));
        let n = order.len() as isize;
        let next = match cur {
            Some(i) => (i as isize + d).rem_euclid(n) as usize,
            None => 0,
        };
        self.select(Some(order[next].clone()), window, cx);
    }

    fn next_session(&mut self, _: &qur::NextAgent, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(1, window, cx)
    }
    fn prev_session(&mut self, _: &qur::PrevAgent, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(-1, window, cx)
    }
    fn next_terminal(&mut self, _: &qur::NextTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(1, window, cx)
    }
    fn prev_terminal(&mut self, _: &qur::PrevTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.step_session(-1, window, cx)
    }
    fn select_session(&mut self, a: &qur::SelectSession, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.session_order().get(a.index).cloned() {
            self.select(Some(s), window, cx);
        }
    }

    fn focus_prompt(&mut self, _: &qur::FocusPrompt, window: &mut Window, cx: &mut Context<Self>) {
        let target = match &self.selected {
            Some(Selected::Agent(id)) => Some(id.clone()),
            _ => self.agent_ids().into_iter().next(),
        };
        if let Some(id) = target {
            if self.selected != Some(Selected::Agent(id.clone())) {
                self.select(Some(Selected::Agent(id.clone())), window, cx);
            }
            if let Some(a) = self.agent(&id) {
                let f = a.read(cx).prompt.read(cx).focus.clone();
                window.focus(&f, cx);
            }
        }
    }

    fn set_layout(&mut self, l: Layout, cx: &mut Context<Self>) {
        self.layout = l;
        for a in &self.agents {
            a.update(cx, |a, cx| a.set_layout(l, cx));
        }
        self.config.peek_layout = l.as_str().into();
        self.write_setting(false, "peek_layout", Some(l.as_str().into()), cx);
        let name = match l {
            Layout::A => "A · watch + side notes",
            Layout::B => "B · one view at a time",
            Layout::C => "C · watch + drawer",
        };
        self.flash(format!("Peek layout {name}"), cx);
        self.sync_menus(cx);
        cx.notify();
    }
    fn cycle_layout(&mut self, _: &qur::CyclePeekLayout, _: &mut Window, cx: &mut Context<Self>) {
        let l = self.layout.next();
        self.set_layout(l, cx)
    }
    fn layout_a(&mut self, _: &qur::PeekLayoutA, _: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(Layout::A, cx)
    }
    fn layout_b(&mut self, _: &qur::PeekLayoutB, _: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(Layout::B, cx)
    }
    fn layout_c(&mut self, _: &qur::PeekLayoutC, _: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(Layout::C, cx)
    }
    /// Cmd+1..4 (layout B): switches the selected agent's view; Live/terminal for terminal tasks.
    fn peek(&mut self, v: View, cx: &mut Context<Self>) {
        if self.layout != Layout::B {
            self.set_layout(Layout::B, cx);
        }
        match self.selected.clone() {
            Some(Selected::Agent(id)) => {
                if let Some(a) = self.agent(&id).cloned() {
                    a.update(cx, |a, cx| a.set_view(v, cx));
                }
            }
            Some(Selected::TermTask(id)) => {
                if let Some(t) = self.term_tasks.iter_mut().find(|t| t.record.id == id) {
                    t.show_live = v == View::Live;
                }
            }
            _ => {}
        }
        cx.notify();
    }
    fn peek_live(&mut self, _: &qur::PeekLive, _: &mut Window, cx: &mut Context<Self>) {
        self.peek(View::Live, cx)
    }
    fn peek_todo(&mut self, _: &qur::PeekTodo, _: &mut Window, cx: &mut Context<Self>) {
        self.peek(View::Todo, cx)
    }
    fn peek_chat(&mut self, _: &qur::PeekChat, _: &mut Window, cx: &mut Context<Self>) {
        self.peek(View::Chat, cx)
    }
    fn peek_changes(&mut self, _: &qur::PeekChanges, _: &mut Window, cx: &mut Context<Self>) {
        self.peek(View::Changes, cx)
    }
    /// Cmd+5: the selected agent's Browser (any layout).
    fn peek_browser(&mut self, _: &qur::PeekBrowser, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.selected_agent() {
            a.update(cx, |a, cx| {
                let on = !a.shows_browser();
                a.show_browser(on, cx)
            });
        }
    }
    /// Cmd+Shift+C: open the Browser if needed and toggle Inspect.
    fn toggle_inspect(&mut self, _: &qur::ToggleInspect, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.selected_agent() {
            a.update(cx, |a, cx| {
                if !a.shows_browser() {
                    a.show_browser(true, cx);
                }
                a.browser.update(cx, |b, cx| b.toggle_inspect(cx));
            });
        }
    }
    fn selected_live(&self, cx: &App) -> Option<Entity<LivePane>> {
        match &self.selected {
            Some(Selected::Agent(id)) => self.agent(id).map(|a| a.read(cx).live.clone()),
            Some(Selected::TermTask(id)) => self.term_task(id).map(|t| t.live.clone()),
            _ => None,
        }
    }
    fn follow_agent(&mut self, _: &qur::FollowAgent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(l) = self.selected_live(cx) {
            l.update(cx, |l, cx| {
                l.follow = !l.follow;
                cx.notify();
            });
        }
        self.sync_menus(cx);
    }
    fn take_over(&mut self, _: &qur::TakeOver, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.selected_agent() {
            a.update(cx, |a, cx| a.stop(cx));
        }
    }

    fn stop_agent(&mut self, _: &qur::StopAgent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.selected_agent() {
            a.update(cx, |a, cx| a.stop(cx));
        }
    }

    fn allow_once(&mut self, _: &aa::AllowOnce, _: &mut Window, cx: &mut Context<Self>) {
        self.answer(PermissionKind::AllowOnce, cx)
    }
    fn allow_always(&mut self, _: &aa::AllowAlways, _: &mut Window, cx: &mut Context<Self>) {
        self.answer(PermissionKind::AllowAlways, cx)
    }
    fn deny(&mut self, _: &aa::Deny, _: &mut Window, cx: &mut Context<Self>) {
        self.answer(PermissionKind::RejectOnce, cx)
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

    fn selected_worktree(&self, cx: &App) -> Option<(String, PathBuf, String)> {
        match &self.selected {
            Some(Selected::Agent(id)) => self.agent(id).map(|a| {
                let r = &a.read(cx).record;
                (r.id.clone(), r.worktree.clone(), r.branch.clone())
            }),
            Some(Selected::TermTask(id)) => self.term_task(id).map(|t| (t.record.id.clone(), t.record.worktree.clone(), t.record.branch.clone())),
            _ => None,
        }
    }

    fn reveal_worktree(&mut self, _: &qur::RevealWorktree, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, wt, _)) = self.selected_worktree(cx) {
            let _ = std::process::Command::new("open").arg(&wt).spawn();
        }
    }

    fn worktree_terminal(&mut self, _: &qur::OpenWorktreeInTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, wt, _)) = self.selected_worktree(cx) {
            self.new_terminal_in(wt, window, cx);
        }
    }

    /// Discard: stop the agent, remove its worktree and branch (asks first).
    fn discard_task(&mut self, _: &qur::DiscardTask, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, wt, branch)) = self.selected_worktree(cx) else { return };
        let dirty = qur_worktree::is_dirty(&wt);
        let detail = format!(
            "This stops the agent and deletes its worktree and branch {branch}.{}",
            if dirty { " Its uncommitted changes will be lost." } else { "" }
        );
        let answer = window.prompt(PromptLevel::Warning, "Discard this task?", Some(&detail), &["Discard", "Cancel"], cx);
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            let handle = this
                .update(cx, |this, cx| {
                    let mut h = None;
                    if let Some(a) = this.agent(&id).cloned() {
                        h = a.update(cx, |a, cx| a.shutdown(cx));
                        this.agents.retain(|x| x != &a);
                    }
                    if let Some(t) = this.term_task(&id).and_then(|t| t.term_id) {
                        this.terms.update(cx, |p, cx| {
                            if let Some(ix) = p.sessions.iter().position(|s| s.id == t) {
                                p.close(ix, cx);
                            }
                        });
                    }
                    this.term_tasks.retain(|t| t.record.id != id);
                    this.trees.remove(&wt);
                    h
                })
                .ok()
                .flatten();
            let removed = cx
                .background_executor()
                .spawn(async move {
                    if let Some(h) = handle {
                        let _ = h.join();
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

    /// Window closing or app quitting: stop every agent and terminal (no
    /// orphans). Agents shut down in parallel; this waits for all of them.
    pub fn shutdown(&mut self, cx: &mut Context<Self>) {
        let handles: Vec<_> = self.agents.iter().filter_map(|a| a.update(cx, |a, cx| a.shutdown(cx))).collect();
        self.terms.update(cx, |p, _| p.close_all_sync());
        for h in handles {
            let _ = h.join();
        }
        self.save_tasks(cx);
    }

    // -- the code modal ------------------------------------------------------------

    pub fn open_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.open_path_at(path, None, window, cx)
    }

    fn open_path_at(&mut self, path: &Path, at: Option<Point>, window: &mut Window, cx: &mut Context<Self>) {
        self.record_nav(cx);
        let buf = match self.buffers.find(path, cx) {
            Some(b) => b,
            None => {
                let wrap = self.config.editor.soft_wrap;
                let tab = self.config.editor.tab_width;
                let view = match EditorView::load(path) {
                    Ok(loaded) => cx.new(|cx| EditorView::new(loaded, wrap, tab, cx)),
                    Err(e) => {
                        self.flash(format!("Couldn't open {}: {e}", path.display()), cx);
                        return;
                    }
                };
                let sub = cx.subscribe_in(&view, window, Self::on_editor_event);
                self._subs.push(sub);
                view
            }
        };
        self.buffers.touch(&buf);
        if self.code.is_none() {
            self.code_prev_focus = window.focused(cx);
        }
        self.code = Some(buf.clone());
        if let Some(p) = at {
            buf.update(cx, |b, cx| b.set_cursor(p, cx));
        }
        for t in self.trees.values() {
            t.update(cx, |t, cx| {
                t.current = Some(path.to_path_buf());
                cx.notify();
            });
        }
        self.push_nav(cx);
        let f = buf.read(cx).focus.clone();
        window.focus(&f, cx);
        self.sync_menus(cx);
        cx.notify();
    }

    fn close_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.code.take().is_some() {
            match self.code_prev_focus.take() {
                Some(f) => window.focus(&f, cx),
                None => self.focus_stage(window, cx),
            }
            self.sync_menus(cx);
            cx.notify();
        }
    }

    fn focus_stage(&self, window: &mut Window, cx: &mut App) {
        match &self.selected {
            Some(Selected::Agent(id)) => {
                if let Some(a) = self.agent(id) {
                    let f = a.read(cx).prompt.read(cx).focus.clone();
                    window.focus(&f, cx);
                }
            }
            Some(Selected::Shell(_)) | Some(Selected::TermTask(_)) => {
                let f = self.terms.read(cx).focus.clone();
                window.focus(&f, cx);
            }
            None => window.focus(&self.focus, cx),
        }
    }

    fn on_editor_event(&mut self, view: &Entity<EditorView>, ev: &EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            EditorEvent::Changed => {
                let v = view.read(cx);
                let path = v.path().map(Path::to_path_buf);
                let clean = !v.is_dirty();
                if clean && path.as_deref() == Some(qur_config::config_path().as_path()) {
                    self.reload_settings(window, cx);
                    self.broadcast_reload(cx);
                }
                if clean && path.as_deref() == Some(qur_agents::roster_path().as_path()) {
                    let (r, err) = Roster::load();
                    self.roster = r;
                    if let Some(e) = err {
                        self.flash(e, cx);
                    }
                }
                cx.notify();
            }
            EditorEvent::Error(e) => self.flash(e.clone(), cx),
            EditorEvent::WrapToggled => self.sync_menus(cx),
            EditorEvent::CloseRequested => self.close_code(window, cx),
        }
    }

    fn record_nav(&mut self, cx: &App) {
        if let Some(a) = &self.code {
            let a = a.read(cx);
            if let (Some(p), Some(entry)) = (a.path(), self.nav.get_mut(self.nav_ix))
                && entry.0 == p
            {
                entry.1 = a.cursor();
            }
        }
    }

    fn push_nav(&mut self, cx: &App) {
        let Some(a) = &self.code else { return };
        let a = a.read(cx);
        let Some(p) = a.path() else { return };
        if self.nav.get(self.nav_ix).is_some_and(|e| e.0 == p) {
            return;
        }
        self.nav.truncate(self.nav_ix + 1);
        self.nav.push((p.to_path_buf(), a.cursor()));
        self.nav_ix = self.nav.len() - 1;
    }

    fn nav_to(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.record_nav(cx);
        let Some((p, at)) = self.nav.get(ix).cloned() else { return };
        let saved = std::mem::take(&mut self.nav);
        self.open_path_at(&p, Some(at), window, cx);
        self.nav = saved;
        self.nav_ix = ix;
    }

    // -- actions: app & window ---------------------------------------------------

    fn quit(&mut self, _: &qur::Quit, window: &mut Window, cx: &mut Context<Self>) {
        let dirty: Vec<_> = cx
            .windows()
            .into_iter()
            .filter_map(|h| h.downcast::<Workspace>())
            .flat_map(|w| w.read(cx).map(|w| w.buffers.dirty(cx)).unwrap_or_default())
            .collect();
        if dirty.is_empty() {
            return cx.quit();
        }
        let msg = format!("Save changes to {} file{} before quitting?", dirty.len(), if dirty.len() == 1 { "" } else { "s" });
        let answer = window.prompt(PromptLevel::Warning, &msg, None, &["Save All", "Don't Save", "Cancel"], cx);
        cx.spawn(async move |_, cx| match answer.await {
            Ok(0) => cx.update(|cx| {
                for b in &dirty {
                    b.update(cx, |b, cx| b.save(cx));
                }
                cx.quit()
            }),
            Ok(1) => cx.update(|cx| cx.quit()),
            _ => {}
        })
        .detach();
    }

    /// Window close (button or Cmd+Shift+W): ask about unsaved files first.
    pub fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let dirty = self.buffers.dirty(cx);
        if dirty.is_empty() {
            self.shutdown(cx);
            return true;
        }
        let msg = format!("Save changes to {} file{}?", dirty.len(), if dirty.len() == 1 { "" } else { "s" });
        let answer = window.prompt(PromptLevel::Warning, &msg, None, &["Save All", "Don't Save", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            let a = answer.await.unwrap_or(2);
            if a == 2 {
                return;
            }
            this.update_in(cx, |this, window, cx| {
                if a == 0 {
                    for b in &dirty {
                        b.update(cx, |b, cx| b.save(cx));
                    }
                    if !this.buffers.dirty(cx).is_empty() {
                        return;
                    }
                } else {
                    this.buffers.list.retain(|b| !b.read(cx).is_dirty());
                    this.code = None;
                }
                this.shutdown(cx);
                window.remove_window();
            })
            .ok();
        })
        .detach();
        false
    }

    fn hide(&mut self, _: &qur::Hide, _: &mut Window, cx: &mut Context<Self>) {
        cx.hide()
    }
    fn hide_others(&mut self, _: &qur::HideOthers, _: &mut Window, cx: &mut Context<Self>) {
        cx.hide_other_apps()
    }
    fn show_all(&mut self, _: &qur::ShowAll, _: &mut Window, cx: &mut Context<Self>) {
        cx.unhide_other_apps()
    }
    fn close_window(&mut self, _: &qur::CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_close(window, cx) {
            window.remove_window()
        }
    }
    fn minimize(&mut self, _: &qur::Minimize, window: &mut Window, _: &mut Context<Self>) {
        window.minimize_window()
    }
    fn zoom_window(&mut self, _: &qur::Zoom, window: &mut Window, _: &mut Context<Self>) {
        window.zoom_window()
    }
    fn fullscreen(&mut self, _: &qur::ToggleFullScreen, window: &mut Window, _: &mut Context<Self>) {
        window.toggle_fullscreen()
    }
    fn toggle_sidebar(&mut self, _: &qur::ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar = !self.sidebar;
        self.sync_menus(cx);
        cx.notify();
    }
    fn reveal_files(&mut self, _: &qur::RevealInFiles, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar = true;
        let root = self.current_root(cx);
        let tree = self.tree_for(&root, window, cx);
        if let Some(p) = self.code.as_ref().and_then(|a| a.read(cx).path().map(Path::to_path_buf)) {
            tree.update(cx, |t, cx| {
                t.reveal(&p, cx);
            });
        }
        self.sync_menus(cx);
        cx.notify();
    }
    fn zoom_in(&mut self, _: &qur::ZoomIn, window: &mut Window, cx: &mut Context<Self>) {
        let z = self.config.ui.zoom + 0.1;
        self.set_zoom(z, window, cx)
    }
    fn zoom_out(&mut self, _: &qur::ZoomOut, window: &mut Window, cx: &mut Context<Self>) {
        let z = self.config.ui.zoom - 0.1;
        self.set_zoom(z, window, cx)
    }
    fn zoom_reset(&mut self, _: &qur::ZoomReset, window: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(1.0, window, cx)
    }

    fn open_config(&mut self, _: &qur::OpenConfig, window: &mut Window, cx: &mut Context<Self>) {
        let path = qur_config::config_path();
        if !path.exists() {
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            let _ = std::fs::write(&path, CONFIG_TEMPLATE);
        }
        self.open_path(&path, window, cx);
    }
    fn open_agents(&mut self, _: &qur::OpenAgents, window: &mut Window, cx: &mut Context<Self>) {
        let _ = Roster::load(); // writes the defaults when missing
        self.open_path(&qur_agents::roster_path(), window, cx);
    }
    fn new_window(&mut self, _: &qur::NewWindow, _: &mut Window, cx: &mut Context<Self>) {
        crate::prompt_open_folder(cx);
    }
    fn open_folder(&mut self, _: &qur::OpenFolder, _: &mut Window, cx: &mut Context<Self>) {
        crate::prompt_open_folder(cx);
    }
    fn open_project(&mut self, a: &qur::OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        crate::open_project(a.path.clone(), None, cx);
    }

    fn save_all(&mut self, _: &qur::SaveAll, _: &mut Window, cx: &mut Context<Self>) {
        for b in self.buffers.dirty(cx) {
            b.update(cx, |b, cx| b.save(cx));
        }
    }
    fn revert(&mut self, _: &qur::RevertFile, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = &self.code {
            a.update(cx, |a, cx| a.reload_from_disk(cx));
        }
    }
    fn recent_file(&mut self, _: &qur::RecentFile, window: &mut Window, cx: &mut Context<Self>) {
        let skip = usize::from(self.code.is_some());
        if let Some(p) = self.buffers.list.get(skip).and_then(|b| b.read(cx).path().map(Path::to_path_buf)) {
            self.open_path(&p, window, cx);
        }
    }
    fn nav_back(&mut self, _: &qur::NavBack, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav_ix > 0 {
            let ix = self.nav_ix - 1;
            self.nav_to(ix, window, cx);
        }
    }
    fn nav_forward(&mut self, _: &qur::NavForward, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav_ix + 1 < self.nav.len() {
            let ix = self.nav_ix + 1;
            self.nav_to(ix, window, cx);
        }
    }

    fn with_code(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut EditorView, &mut Context<EditorView>)) {
        if let Some(a) = self.code.clone() {
            a.update(cx, f);
        }
    }
    fn find(&mut self, _: &ea::Find, window: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.open_find(false, window, cx))
    }
    fn replace(&mut self, _: &ea::Replace, window: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.open_find(true, window, cx))
    }
    fn find_next(&mut self, _: &ea::FindNext, window: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.find_next(window, cx))
    }
    fn find_prev(&mut self, _: &ea::FindPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.find_prev(window, cx))
    }
    fn use_selection(&mut self, _: &ea::UseSelectionForFind, window: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.use_selection_for_find(window, cx))
    }
    fn soft_wrap(&mut self, _: &ea::ToggleSoftWrap, _: &mut Window, cx: &mut Context<Self>) {
        self.with_code(cx, |e, cx| e.toggle_soft_wrap(cx));
        self.sync_menus(cx);
    }
    // Edit-menu items with nothing focused that handles them stay enabled.
    fn noop_undo(&mut self, _: &edit::Undo, _: &mut Window, _: &mut Context<Self>) {}
    fn noop_redo(&mut self, _: &edit::Redo, _: &mut Window, _: &mut Context<Self>) {}

    // -- overlays ----------------------------------------------------------------------

    fn open_picker(&mut self, placeholder: &str, items: Vec<PickItem>, kind: PickKind, paths: bool, window: &mut Window, cx: &mut Context<Self>) -> Entity<Picker> {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let p = cx.new(|cx| Picker::new(placeholder, items, paths, window, cx));
        let sub = cx.subscribe_in(&p, window, Self::on_picker_event);
        self._subs.push(sub);
        let f = p.read(cx).input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::Picker(p.clone(), kind));
        cx.notify();
        p
    }

    fn on_picker_event(&mut self, picker: &Entity<Picker>, ev: &PickerEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Picker(p, kind)) = &mut self.overlay else { return };
        if p != picker {
            return;
        }
        match (ev, kind) {
            (PickerEvent::Confirm(i), PickKind::Commands(cmds)) => {
                let action = (cmds[*i].action)();
                self.close_overlays(window, cx);
                window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
            }
            (PickerEvent::Confirm(i), PickKind::Files { root, files }) => {
                let path = root.join(&files[*i]);
                self.close_overlays(window, cx);
                self.open_path(&path, window, cx);
            }
            (PickerEvent::Confirm(i), PickKind::Mention { root, files, target }) => {
                let path = root.join(&files[*i]);
                let target = target.clone();
                self.overlay = None;
                self.prev_focus = None;
                target.update(cx, |t, cx| t.insert_mention(path, cx));
                let f = target.read(cx).focus.clone();
                window.focus(&f, cx);
                cx.notify();
            }
            (PickerEvent::Confirm(i), PickKind::Slash { names, target }) => {
                let text = format!("/{} ", names[*i]);
                let target = target.clone();
                self.overlay = None;
                self.prev_focus = None;
                target.update(cx, |t, cx| t.set_text(&text, cx));
                let f = target.read(cx).focus.clone();
                window.focus(&f, cx);
                cx.notify();
            }
            (PickerEvent::Confirm(i), PickKind::Projects(list)) => {
                let target = list.get(*i).cloned();
                self.close_overlays(window, cx);
                match target {
                    Some(p) => crate::open_project(p, None, cx),
                    None => crate::prompt_open_folder(cx),
                }
            }
            (PickerEvent::Highlight(i), PickKind::Themes { names, .. }) => {
                self.preview_theme = Some(names[*i].clone());
                self.apply_look(window, cx);
            }
            (PickerEvent::Confirm(i), PickKind::Themes { names, project_scope }) => {
                let name = names[*i].clone();
                let project_scope = *project_scope;
                self.preview_theme = None;
                if project_scope {
                    self.write_setting(true, "theme", Some(name.as_str().into()), cx);
                } else {
                    self.write_setting(false, "theme", Some(name.as_str().into()), cx);
                    self.write_setting(true, "theme", None, cx);
                }
                self.overlay = None;
                self.reload_settings(window, cx);
                if !project_scope {
                    self.broadcast_reload(cx);
                }
                self.restore_focus(window, cx);
            }
            (PickerEvent::Tab, PickKind::Themes { project_scope, .. }) => {
                *project_scope = !*project_scope;
                let header = theme_scope_header(*project_scope, &self.project_name());
                picker.update(cx, |p, cx| {
                    p.header = Some(header);
                    cx.notify();
                });
            }
            (PickerEvent::Dismiss, PickKind::Themes { .. }) => {
                self.preview_theme = None;
                self.apply_look(window, cx);
                self.close_overlays(window, cx);
            }
            (PickerEvent::Dismiss, _) => self.close_overlays(window, cx),
            _ => {}
        }
    }

    fn files_in(&mut self, root: &Path) -> Vec<PathBuf> {
        self.file_lists.entry(root.to_path_buf()).or_insert_with(|| qur_tree::list_files(root, 200_000)).clone()
    }

    fn file_items(files: &[PathBuf]) -> Vec<PickItem> {
        files
            .iter()
            .map(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                PickItem::new(name).detail(dir)
            })
            .collect()
    }

    fn mention_picker(&mut self, root: PathBuf, target: Entity<PromptInput>, window: &mut Window, cx: &mut Context<Self>) {
        let files = self.files_in(&root);
        let items = Self::file_items(&files);
        self.open_picker("Mention a file…", items, PickKind::Mention { root, files, target }, true, window, cx);
    }

    fn palette(&mut self, _: &qur::CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(&self.overlay, Some(Overlay::Picker(_, PickKind::Commands(_)))) {
            return self.close_overlays(window, cx);
        }
        let cmds: Vec<_> = commands::palette_commands().collect();
        let items = cmds
            .iter()
            .map(|c| {
                let right = match c.tier {
                    commands::Tier::Ready => commands::display_keys(c.first_keys()),
                    commands::Tier::Later(m) => format!("coming in {m}"),
                };
                PickItem::new(format!("{}: {}", c.category, c.title)).right(right).enabled(c.ready())
            })
            .collect();
        self.open_picker("Type a command…", items, PickKind::Commands(cmds), false, window, cx);
    }

    fn quick_open(&mut self, _: &qur::QuickOpen, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.current_root(cx);
        let files = self.files_in(&root);
        // Recently opened files under this root first.
        let recent: Vec<PathBuf> = self
            .buffers
            .paths(cx)
            .into_iter()
            .filter_map(|p| p.strip_prefix(&root).ok().map(Path::to_path_buf))
            .collect();
        let mut ordered = recent.clone();
        ordered.extend(files.into_iter().filter(|f| !recent.contains(f)));
        let items = Self::file_items(&ordered);
        let placeholder = if root == self.root {
            "Go to file…".to_string()
        } else {
            format!("Go to file in {}…", root.file_name().unwrap_or_default().to_string_lossy())
        };
        self.open_picker(&placeholder, items, PickKind::Files { root, files: ordered }, true, window, cx);
    }

    fn theme_picker(&mut self, _: &qur::ThemePicker, window: &mut Window, cx: &mut Context<Self>) {
        let eff = self.effective();
        let mut names = vec![qur_config::THEME_AUTO.to_string()];
        let mut items = vec![PickItem::new("Auto").detail("follows macOS: Qur Light / Qur Dark")];
        for t in Registry::global().all() {
            names.push(t.name.clone());
            items.push(PickItem::new(t.name.clone()).detail(t.group));
        }
        let current = names.iter().position(|n| *n == eff.theme).unwrap_or(0);
        let project_scope = true;
        let header = theme_scope_header(project_scope, &self.project_name());
        let p = self.open_picker("Select a theme…", items, PickKind::Themes { names, project_scope }, false, window, cx);
        p.update(cx, |p, cx| {
            p.header = Some(header);
            p.select_item(current, cx);
        });
    }

    fn use_default_theme(&mut self, _: &qur::UseDefaultTheme, window: &mut Window, cx: &mut Context<Self>) {
        self.write_setting(true, "theme", None, cx);
        self.reload_settings(window, cx);
        self.flash(format!("{} now uses the default theme", self.project_name()), cx);
    }

    fn project_switcher(&mut self, _: &qur::ProjectSwitcher, window: &mut Window, cx: &mut Context<Self>) {
        let home = std::env::var("HOME").unwrap_or_default();
        let mut list = qur_config::State::load().recent_projects;
        list.retain(|p| p != &self.root);
        let mut items: Vec<PickItem> = list
            .iter()
            .map(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                PickItem::new(name).detail(p.to_string_lossy().replacen(&home, "~", 1))
            })
            .collect();
        items.push(PickItem::new("Open Folder…").right(commands::display_keys("cmd-o")));
        self.open_picker("Switch project…", items, PickKind::Projects(list), false, window, cx);
    }

    fn goto_line(&mut self, _: &qur::GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.code.clone() else { return };
        self.close_overlays(window, cx);
        self.prev_focus = Some(active.read(cx).focus.clone());
        let n = active.read(cx).model.line_count();
        let input = cx.new(|cx| TextInput::new(format!("Go to line (1–{n})"), cx));
        let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Confirm { .. } => {
                let t = input.read(cx).text.trim().to_string();
                let mut it = t.split(':');
                let line = it.next().and_then(|l| l.parse::<usize>().ok());
                let col = it.next().and_then(|c| c.parse::<usize>().ok());
                this.close_overlays(window, cx);
                if let (Some(l), Some(a)) = (line, this.code.clone()) {
                    a.update(cx, |a, cx| {
                        a.goto_line(l, cx);
                        if let Some(c) = col {
                            a.set_cursor(Point { line: l.saturating_sub(1), col: c.saturating_sub(1) }, cx);
                        }
                    });
                }
            }
            InputEvent::Cancel => this.close_overlays(window, cx),
            _ => {}
        });
        self._subs.push(sub);
        let f = input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::GoTo(input));
        cx.notify();
    }

    fn new_agent_task(&mut self, _: &qur::NewAgentTask, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let input = cx.new(|cx| PromptInput::new("Describe the task…  (@ to mention a file, ⇧↩ for a new line)", self.root.clone(), cx));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &PromptEvent, window, cx| match ev {
            PromptEvent::Submit => {
                let Some(Overlay::NewTask { agent, .. }) = &this.overlay else { return };
                let agent = *agent;
                let (text, mentions) = input.update(cx, |i, cx| i.take(cx));
                this.overlay = None;
                this.prev_focus = None;
                this.start_task(agent, text, mentions, window, cx);
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
            PromptEvent::Changed | PromptEvent::SlashRequested => {}
        });
        self._subs.push(sub);
        let f = input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::NewTask { input, agent: 0 });
        cx.notify();
    }

    fn transparency(&mut self, _: &qur::Transparency, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let project_scope = self.effective().window_is_project;
        self.overlay = Some(Overlay::Transparency { project_scope });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.prev_focus.take() {
            Some(f) => window.focus(&f, cx),
            None => self.focus_stage(window, cx),
        }
        cx.notify();
    }

    fn close_overlays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Picker(_, PickKind::Themes { .. })) = &self.overlay
            && self.preview_theme.take().is_some()
        {
            self.apply_look(window, cx);
        }
        self.mention_over_dialog = None;
        if self.overlay.take().is_some() {
            self.restore_focus(window, cx);
        }
        cx.notify();
    }

    fn set_opacity(&mut self, opacity: f32, blur: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Transparency { project_scope }) = self.overlay else { return };
        let min = Look::of(window, cx).theme.min_opacity().max(qur_theme::MIN_OPACITY);
        let opacity = ((opacity.clamp(min, 1.0)) * 100.).round() / 100.;
        if project_scope {
            self.project.window.opacity = Some(opacity);
            self.project.window.blur = Some(blur);
        } else {
            self.config.window.opacity = opacity;
            self.config.window.blur = blur;
        }
        self.apply_look(window, cx);
    }

    fn save_transparency(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Transparency { project_scope }) = self.overlay else { return };
        let eff = self.effective();
        if project_scope {
            self.write_setting(true, "window.opacity", Some(eff.opacity.into()), cx);
            self.write_setting(true, "window.blur", Some(eff.blur.into()), cx);
        } else {
            self.write_setting(false, "window.opacity", Some(eff.opacity.into()), cx);
            self.write_setting(false, "window.blur", Some(eff.blur.into()), cx);
            self.write_setting(true, "window.opacity", None, cx);
            self.write_setting(true, "window.blur", None, cx);
            self.broadcast_reload(cx);
        }
    }

    fn slide_to(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let b = self.slider.get();
        if b.size.width <= px(0.) {
            return;
        }
        let min = Look::of(window, cx).theme.min_opacity().max(qur_theme::MIN_OPACITY);
        let t = ((x - b.left()) / b.size.width).clamp(0., 1.);
        let blur = self.effective().blur;
        self.set_opacity(min + (1. - min) * t, blur, window, cx);
    }

    /// QUR_BENCH=glow: a terminal task's worktree gets 20 probe writes from a
    /// background thread (like a formatter or codegen); each is timed from the
    /// write until its row is painted glowing (acceptance: < 100 ms).
    fn bench_glow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let dir = qur_agents::worktrees_dir(&self.project_id);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let created = bg.spawn(async move { qur_worktree::create(&root, &dir, "bench", "glow") }).await;
            let Ok(c) = created else { return };
            let wt = c.path.clone();
            this.update_in(cx, |this, window, cx| {
                let record = TaskRecord {
                    id: c.slug.clone(),
                    agent: "bench".into(),
                    title: "glow bench".into(),
                    branch: c.branch.clone(),
                    worktree: c.path.clone(),
                    base: c.base.clone(),
                    session_id: None,
                    created: 0,
                    mode: Mode::Terminal,
                    browser_url: None,
                };
                this.add_term_task(record, window, cx);
                // Show the tree without starting a terminal process.
                this.selected = Some(Selected::TermTask(c.slug.clone()));
                let root = this.current_root(cx);
                this.tree_for(&root, window, cx);
                cx.notify();
            })
            .ok();
            bg.timer(Duration::from_millis(1500)).await;
            for i in 0..20 {
                let p = wt.join(format!("glow-{i}.txt"));
                std::fs::write(&p, "x\n").ok();
                crate::tree_view::GLOW_WRITES.lock().unwrap().push((p, Instant::now()));
                bg.timer(Duration::from_millis(400)).await;
            }
            bg.timer(Duration::from_millis(800)).await;
            let mut v = crate::tree_view::GLOW_SEEN.lock().unwrap().clone();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if !v.is_empty() {
                println!("METRIC glow_n={}", v.len());
                println!("METRIC glow_p50_ms={:.1}", v[v.len() / 2]);
                println!("METRIC glow_max_ms={:.1}", v[v.len() - 1]);
            }
            println!("METRIC glow_missed={}", 20 - v.len());
            this.update(cx, |this, cx| {
                let _ = qur_worktree::remove(&this.root, &c.path, &c.branch, true);
                cx.quit();
            })
            .ok();
        })
        .detach();
    }

    /// QUR_BENCH=theme: switch through every theme, one per frame.
    fn bench_theme_step(&mut self, i: usize, mut samples: Vec<f64>, window: &mut Window, cx: &mut Context<Self>) {
        let themes = Registry::global().all();
        if i >= themes.len() * 2 {
            samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let pct = |p: f64| samples[((samples.len() - 1) as f64 * p).round() as usize];
            println!("METRIC theme_switch_n={}", samples.len());
            println!("METRIC theme_switch_p50_ms={:.2}", pct(0.5));
            println!("METRIC theme_switch_max_ms={:.2}", samples[samples.len() - 1]);
            return cx.quit();
        }
        let t0 = Instant::now();
        self.preview_theme = Some(themes[i % themes.len()].name.clone());
        self.apply_look(window, cx);
        let me = cx.entity();
        window.on_next_frame(move |window, cx| {
            samples.push(t0.elapsed().as_secs_f64() * 1000.0);
            me.update(cx, |ws, cx| ws.bench_theme_step(i + 1, samples, window, cx));
        });
    }

    // -- render pieces -------------------------------------------------------------------

    fn session_title(&self, cx: &App) -> Option<String> {
        match &self.selected {
            Some(Selected::Agent(id)) => self.agent(id).map(|a| a.read(cx).record.title.clone()),
            Some(Selected::TermTask(id)) => self.term_task(id).map(|t| t.record.title.clone()),
            Some(Selected::Shell(s)) => self.terms.read(cx).session_by_id(*s).map(|s| s.label.clone()),
            None => None,
        }
    }

    fn title_bar(&self, look: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = &look.theme.ui;
        let session = self.session_title(cx);
        let message = self.message.as_ref().map(|(m, _)| m.clone());
        div()
            .id("titlebar")
            .window_control_area(gpui::WindowControlArea::Drag)
            .h(look.z(TITLEBAR_H))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(look.z(8.))
            .pl(px(TRAFFIC_LIGHTS_W))
            .pr(look.z(12.))
            .bg(look.bg(ui.top_bar_bg))
            .border_b_1()
            .border_color(look.c(ui.border))
            .text_size(look.text(0.))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, _| this.title_drag = true))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.title_drag = false))
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.title_drag = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.title_drag {
                    this.title_drag = false;
                    window.start_window_move();
                }
            }))
            .on_click(|ev, window, _| {
                if ev.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(
                svg()
                    .path(assets::symbol(look.logo_white(), 16. * look.zoom))
                    .size(look.z(16.))
                    .flex_none()
                    // Pure black or white only: the logo is never tinted.
                    .text_color(if look.logo_white() { gpui::white() } else { gpui::black() }),
            )
            .child(
                div()
                    .id("project")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(look.z(2.))
                    .px(look.z(4.))
                    .rounded(look.z(4.))
                    .cursor_pointer()
                    .hover(|d| d.bg(look.c(ui.hover_bg)))
                    .text_color(look.c(ui.text))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(self.project_name())
                    .child(svg().path(icon::CHEVRON_DOWN).size(look.z(12.)).text_color(look.c(ui.text_muted)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, window, cx| this.project_switcher(&qur::ProjectSwitcher, window, cx))),
            )
            .children(session.map(|s| div().min_w(px(0.)).truncate().text_color(look.c(ui.text_secondary)).child(s)))
            .child(div().flex_1())
            .children(message.map(|m| div().flex_none().max_w(look.z(420.)).truncate().text_size(look.text(-1.)).text_color(look.c(ui.text_secondary)).child(m)))
    }

    fn section_header(&self, label: SharedString, look: &Look) -> gpui::Div {
        let ui = &look.theme.ui;
        div()
            .flex()
            .items_center()
            .px(look.z(10.))
            .pt(look.z(10.))
            .pb(look.z(4.))
            .text_size(look.text(-2.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(look.c(ui.text_muted))
            .child(div().flex_1().truncate().child(label))
    }

    #[allow(clippy::too_many_arguments)]
    fn session_row(
        &self,
        id: SharedString,
        accent: Option<Rgba>,
        title: String,
        detail: String,
        glyph: &'static str,
        selected: bool,
        look: &Look,
    ) -> gpui::Stateful<gpui::Div> {
        let ui = &look.theme.ui;
        div()
            .id(id)
            .mx(look.z(4.))
            .px(look.z(6.))
            .py(look.z(4.))
            .rounded(look.z(4.))
            .flex()
            .items_center()
            .gap(look.z(8.))
            .cursor_pointer()
            .when(selected, |d| d.bg(look.c(ui.active_bg)))
            .hover(|d| d.bg(look.c(ui.hover_bg)))
            .child(match accent {
                Some(a) => div().flex_none().size(look.z(9.)).rounded(look.z(2.5)).bg(look.c(a)),
                None => div().flex_none().size(look.z(9.)).rounded(look.z(2.5)).border_1().border_color(look.c(ui.text_muted)),
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .child(div().truncate().text_size(look.text(-1.)).text_color(look.c(ui.text)).child(title))
                    .when(!detail.is_empty(), |d| d.child(div().truncate().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(detail))),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(look.text(-1.))
                    .text_color(look.c(if glyph == "!" { accent.unwrap_or(ui.warning) } else { ui.text_muted }))
                    .child(glyph),
            )
    }

    fn sidebar(&self, look: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = look.theme.ui.clone();
        let mut agents: Vec<gpui::AnyElement> = Vec::new();
        for a in &self.agents {
            let t = a.read(cx);
            let id = t.record.id.clone();
            let sel = self.selected == Some(Selected::Agent(id.clone()));
            let detail = match (&t.status, t.last_line()) {
                (Status::Working | Status::Done | Status::Idle, Some(l)) => l,
                (Status::Error(_), _) => format!("{} · error", t.def.label),
                (s, _) => format!("{} · {}", t.def.label, s.label()),
            };
            let unread = self.unread.contains(&id);
            let row = self
                .session_row(format!("agent-{id}").into(), Some(t.accent), t.record.title.clone(), detail, t.status.glyph(), sel, look)
                .when(unread, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                .on_click(cx.listener(move |this, _, window, cx| this.select(Some(Selected::Agent(id.clone())), window, cx)));
            agents.push(row.into_any_element());
        }
        let panel = self.terms.read(cx);
        for t in &self.term_tasks {
            let id = t.record.id.clone();
            let sel = self.selected == Some(Selected::TermTask(id.clone()));
            let s = t.term_id.and_then(|tid| panel.session_by_id(tid));
            let busy = s.is_some_and(|s| s.is_busy());
            let title = s.and_then(|s| s.first_line.clone()).unwrap_or_else(|| t.record.title.clone());
            let detail = if s.is_some() { "terminal".to_string() } else { "terminal · stopped".to_string() };
            let row = self
                .session_row(format!("tt-{id}").into(), Some(t.accent), title, detail, if busy { "~" } else { "·" }, sel, look)
                .on_click(cx.listener(move |this, _, window, cx| this.select(Some(Selected::TermTask(id.clone())), window, cx)));
            agents.push(row.into_any_element());
        }
        let shells: Vec<gpui::AnyElement> = self
            .shells
            .iter()
            .filter_map(|&tid| {
                let s = panel.session_by_id(tid)?;
                let sel = self.selected == Some(Selected::Shell(tid));
                Some(
                    self.session_row(format!("sh-{tid}").into(), None, s.label.clone(), String::new(), if s.is_busy() { "~" } else { "" }, sel, look)
                        .on_click(cx.listener(move |this, _, window, cx| this.select(Some(Selected::Shell(tid)), window, cx)))
                        .into_any_element(),
                )
            })
            .collect();
        let root = self.current_root(cx);
        let tree = self.trees.get(&root).cloned();
        let files_label: SharedString = if root == self.root {
            "FILES".into()
        } else {
            format!("FILES · {}", root.file_name().unwrap_or_default().to_string_lossy()).into()
        };
        let empty_agents = agents.is_empty();
        div()
            .relative()
            .w(look.z(self.sidebar_w))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(look.bg(ui.sidebar_bg))
            .border_r_1()
            .border_color(look.c(ui.border))
            .child(
                self.section_header("AGENTS".into(), look).child(
                    div()
                        .id("new-task")
                        .flex_none()
                        .p(look.z(2.))
                        .rounded(look.z(3.))
                        .cursor_pointer()
                        .hover(|d| d.bg(look.c(ui.hover_bg)))
                        .child(svg().path(icon::PLUS).size(look.z(12.)).text_color(look.c(ui.text_secondary)))
                        .on_click(cx.listener(|this, _, window, cx| this.new_agent_task(&qur::NewAgentTask, window, cx))),
                ),
            )
            .children(agents)
            .when(empty_agents, |d| {
                d.child(
                    div()
                        .px(look.z(10.))
                        .py(look.z(4.))
                        .text_size(look.text(-2.))
                        .text_color(look.c(ui.text_muted))
                        .child(format!("{}  new agent task", commands::display_keys("cmd-shift-n"))),
                )
            })
            .when(!shells.is_empty(), |d| d.child(self.section_header("TERMINALS".into(), look)).children(shells))
            .child(self.section_header(files_label, look).pt(look.z(14.)))
            .children(tree.map(|t| div().flex_1().min_h(px(0.)).flex().flex_col().child(t)))
            // Drag handle on the right edge.
            .child(
                div()
                    .id("sidebar-resize")
                    .absolute()
                    .top_0()
                    .right(px(-3.))
                    .w(px(6.))
                    .h_full()
                    .cursor_col_resize()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.resizing = true;
                        cx.stop_propagation();
                    })),
            )
    }

    fn stage(&self, look: &Look) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        match &self.selected {
            Some(Selected::Agent(id)) => {
                if let Some(a) = self.agent(id) {
                    return a.clone().into_any_element();
                }
            }
            Some(Selected::TermTask(id)) => {
                if let Some(t) = self.term_task(id) {
                    let term = div().size_full().bg(look.bg(ui.panel_bg)).pt(look.z(6.)).child(self.terms.clone());
                    return match self.layout {
                        Layout::A => div()
                            .size_full()
                            .flex()
                            .child(div().flex_1().min_w(px(0.)).child(t.live.clone()))
                            .child(div().w(gpui::relative(0.45)).flex_none().h_full().border_l_1().border_color(look.c(ui.border)).child(term))
                            .into_any_element(),
                        Layout::B if t.show_live => div().size_full().child(t.live.clone()).into_any_element(),
                        Layout::B => term.into_any_element(),
                        Layout::C => div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .child(div().flex_1().min_h(px(0.)).child(t.live.clone()))
                            .child(div().h(gpui::relative(0.42)).flex_none().border_t_1().border_color(look.c(ui.border)).child(term))
                            .into_any_element(),
                    };
                }
            }
            Some(Selected::Shell(_)) => {
                return div().size_full().bg(look.bg(ui.panel_bg)).pt(look.z(6.)).child(self.terms.clone()).into_any_element();
            }
            None => {}
        }
        let hint = |label: &'static str, keys: &str| {
            div()
                .flex()
                .justify_between()
                .gap(look.z(32.))
                .child(div().text_color(look.c(ui.text_secondary)).child(label))
                .child(div().text_color(look.c(ui.text_muted)).child(commands::display_keys(keys)))
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(look.bg(ui.editor_bg))
            .text_size(look.text(0.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.z(8.))
                    .child(hint("New Agent Task", "cmd-shift-n"))
                    .child(hint("New Agent Terminal", "cmd-shift-t"))
                    .child(hint("New Terminal", "cmd-t"))
                    .child(hint("Go to File", "cmd-p"))
                    .child(hint("Command Palette", "cmd-shift-p")),
            )
            .into_any_element()
    }

    fn code_modal(&self, look: &Look, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let code = self.code.clone()?;
        let ui = &look.theme.ui;
        let (title, dirty, owner) = {
            let c = code.read(cx);
            let path = c.path().map(Path::to_path_buf).unwrap_or_default();
            // Which tree is this file in: the main checkout or a task's worktree?
            let owner = self
                .agents
                .iter()
                .map(|a| (a.read(cx).record.worktree.clone(), a.read(cx).record.title.clone()))
                .chain(self.term_tasks.iter().map(|t| (t.record.worktree.clone(), t.record.title.clone())))
                .find(|(wt, _)| path.starts_with(wt));
            let rel = match &owner {
                Some((wt, _)) => path.strip_prefix(wt).unwrap_or(&path).to_string_lossy().into_owned(),
                None => path.strip_prefix(&self.root).unwrap_or(&path).to_string_lossy().into_owned(),
            };
            (rel, c.is_dirty(), owner.map(|(_, t)| t))
        };
        Some(
            div()
                .absolute()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.35))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_code(window, cx)))
                .child(
                    div()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .w(gpui::relative(0.86))
                        .h(gpui::relative(0.88))
                        .flex()
                        .flex_col()
                        .rounded(look.z(8.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(look.c(ui.border_strong))
                        .bg(look.c(ui.editor_bg))
                        .child(
                            div()
                                .flex_none()
                                .h(look.z(32.))
                                .px(look.z(12.))
                                .flex()
                                .items_center()
                                .gap(look.z(8.))
                                .border_b_1()
                                .border_color(look.c(ui.border))
                                .text_size(look.text(-1.))
                                .child(div().min_w(px(0.)).truncate().text_color(look.c(ui.text)).child(title))
                                .when(dirty, |d| d.child(div().size(look.z(6.)).flex_none().rounded(look.z(1.5)).bg(look.c(ui.text_secondary))))
                                .children(owner.map(|w| div().flex_none().text_color(look.c(ui.text_muted)).child(format!("in {w}"))))
                                .child(div().flex_1())
                                .child(div().flex_none().text_color(look.c(ui.text_muted)).child("esc to close")),
                        )
                        .child(div().flex_1().min_h(px(0.)).child(code)),
                )
                .into_any_element(),
        )
    }

    fn overlay_el(&self, look: &Look, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let ui = &look.theme.ui;
        let centered = |child: gpui::AnyElement| {
            div()
                .absolute()
                .top(look.z(TITLEBAR_H + 40.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(div().occlude().child(child))
                .into_any_element()
        };
        match self.overlay.as_ref()? {
            Overlay::Picker(p, _) => Some(centered(p.clone().into_any_element())),
            Overlay::GoTo(g) => Some(centered(
                div()
                    .w(look.z(360.))
                    .px(look.z(10.))
                    .py(look.z(6.))
                    .bg(look.c(ui.overlay_bg))
                    .border_1()
                    .border_color(look.c(ui.border_strong))
                    .rounded(look.z(6.))
                    .child(g.clone())
                    .into_any_element(),
            )),
            Overlay::Transparency { project_scope } => Some(self.transparency_popover(*project_scope, look, cx)),
            Overlay::NewTask { input, agent } => {
                let agents = self.roster.agents.clone();
                let chosen = *agent;
                let chips: Vec<_> = agents
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        div()
                            .id(("agent-chip", i))
                            .px(look.z(8.))
                            .py(look.z(2.))
                            .rounded(look.z(4.))
                            .cursor_pointer()
                            .text_size(look.text(-1.))
                            .text_color(look.c(if i == chosen { ui.text } else { ui.text_muted }))
                            .when(i == chosen, |d| d.bg(look.c(ui.active_bg)))
                            .child(format!("{}{}", a.label, if a.mode == Mode::Terminal { " (terminal)" } else { "" }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(Overlay::NewTask { agent, .. }) = &mut this.overlay {
                                    *agent = i;
                                    cx.notify();
                                }
                            }))
                    })
                    .collect();
                let mention = self.mention_over_dialog.clone();
                let agent_id = agents.get(chosen).map(|a| a.id.clone()).unwrap_or_else(|| "agent".into());
                Some(
                    div()
                        .absolute()
                        .top(look.z(TITLEBAR_H + 40.))
                        .left_0()
                        .right_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(look.z(8.))
                        .child(
                            div()
                                .occlude()
                                .w(look.z(600.))
                                .p(look.z(12.))
                                .flex()
                                .flex_col()
                                .gap(look.z(10.))
                                .bg(look.c(ui.overlay_bg))
                                .border_1()
                                .border_color(look.c(ui.border_strong))
                                .rounded(look.z(8.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(look.z(6.))
                                        .child(div().font_weight(gpui::FontWeight::MEDIUM).child("New agent task"))
                                        .child(div().flex_1())
                                        .children(chips),
                                )
                                .child(
                                    div()
                                        .px(look.z(8.))
                                        .py(look.z(6.))
                                        .rounded(look.z(4.))
                                        .border_1()
                                        .border_color(look.c(ui.border))
                                        .bg(look.c(ui.editor_bg))
                                        .child(input.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(look.text(-2.))
                                        .text_color(look.c(ui.text_muted))
                                        .child(format!("↩ start · runs in its own worktree on qur/{agent_id}/<task> · esc cancel")),
                                ),
                        )
                        .children(mention.map(|m| div().occlude().child(m)))
                        .into_any_element(),
                )
            }
        }
    }

    fn transparency_popover(&self, project_scope: bool, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let min = look.theme.min_opacity().max(qur_theme::MIN_OPACITY);
        let pct = (look.opacity * 100.).round() as i32;
        let t = if min < 1. { (look.opacity - min) / (1. - min) } else { 1. };
        let slider = self.slider.clone();
        let track_w = look.z(200.);
        let check = |on: bool| {
            div()
                .size(look.z(12.))
                .flex_none()
                .rounded(look.z(3.))
                .border_1()
                .border_color(look.c(ui.text_secondary))
                .flex()
                .items_center()
                .justify_center()
                .when(on, |d| d.child(svg().path(icon::CHECK).size(look.z(10.)).text_color(look.c(ui.text))))
        };
        let scope = |label: &'static str, on: bool, id: &'static str, project: bool| {
            div()
                .id(id)
                .px(look.z(8.))
                .py(look.z(2.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_color(look.c(if on { ui.text } else { ui.text_muted }))
                .when(on, |d| d.bg(look.c(ui.active_bg)))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.overlay = Some(Overlay::Transparency { project_scope: project });
                    let eff = this.effective();
                    this.set_opacity(eff.opacity, eff.blur, window, cx);
                }))
        };
        div()
            .absolute()
            .top(look.z(TITLEBAR_H + 4.))
            .left(px(TRAFFIC_LIGHTS_W))
            .occlude()
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.sliding = false))
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                if this.sliding && ev.pressed_button == Some(MouseButton::Left) {
                    this.slide_to(ev.position.x, window, cx);
                } else {
                    this.sliding = false;
                }
            }))
            .w(look.z(248.))
            .p(look.z(12.))
            .flex()
            .flex_col()
            .gap(look.z(10.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(6.))
            .text_size(look.text(-1.))
            .text_color(look.c(ui.text))
            .child(div().flex().gap(look.z(4.)).child(scope("This project", project_scope, "scope-project", true)).child(scope("All projects", !project_scope, "scope-all", false)))
            .child(div().flex().justify_between().child("Window opacity").child(div().text_color(look.c(ui.text_muted)).child(format!("{pct}%"))))
            .child(
                div()
                    .id("slider")
                    .relative()
                    .w(track_w)
                    .h(look.z(16.))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &gpui::MouseDownEvent, window, cx| {
                        this.sliding = true;
                        this.slide_to(ev.position.x, window, cx);
                    }))
                    .child(canvas(move |b, _, _| slider.set(b), |_, _, _, _| {}).absolute().size_full())
                    .child(div().absolute().top(look.z(7.)).left_0().w(track_w).h(look.z(2.)).bg(look.c(ui.border_strong)))
                    .child(div().absolute().top(look.z(7.)).left_0().w(track_w * t).h(look.z(2.)).bg(look.c(ui.text)))
                    .child(div().absolute().top(look.z(2.)).left(track_w * t - look.z(6.)).size(look.z(12.)).rounded(look.z(3.)).bg(look.c(ui.text))),
            )
            .child(
                div()
                    .id("blur")
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .cursor_pointer()
                    .child(check(look.blur))
                    .child("Blur background")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let eff = this.effective();
                        this.set_opacity(eff.opacity, !eff.blur, window, cx);
                    })),
            )
            .into_any_element()
    }
}

thread_local! {
    /// Task id per agent entity (lets render paths find agents by id without
    /// borrowing the App).
    static AGENT_IDS: std::cell::RefCell<HashMap<gpui::EntityId, String>> = std::cell::RefCell::new(HashMap::new());
}

fn agent_id_matches(a: &Entity<AgentTask>, id: &str) -> bool {
    AGENT_IDS.with(|m| m.borrow().get(&a.entity_id()).is_some_and(|x| x == id))
}

fn accent_for(def: &AgentDef, slot: &qur_theme::Accent) -> Rgba {
    def.accent.as_deref().and_then(Rgba::parse).unwrap_or(slot.base)
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

fn theme_scope_header(project_scope: bool, project: &str) -> SharedString {
    if project_scope {
        format!("Theme for {project} only  ·  ⇥ for all projects").into()
    } else {
        "Theme for all projects  ·  ⇥ for this project only".into()
    }
}

fn is_dark(a: WindowAppearance) -> bool {
    matches!(a, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

/// SF Mono when installed (Xcode, Terminal), else Menlo; nothing extra ships.
/// A direct lookup (listing every font costs ~40 ms at startup).
fn default_code_font(cx: &App) -> String {
    static FOUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let sf = *FOUND.get_or_init(|| {
        let ts = cx.text_system();
        let id = ts.resolve_font(&gpui::font("SF Mono"));
        ts.get_font_for_id(id).is_some_and(|f| f.family.as_ref() == "SF Mono")
    });
    if sf { "SF Mono".into() } else { "Menlo".into() }
}

const CONFIG_TEMPLATE: &str = r#"# Qur settings. Saved changes apply right away.
# Per-project overrides live in projects/<id>.toml (set them from Qur > Theme… / Transparency…).
# Agents are configured in agents.toml (Qur > Settings > Agents).

# theme = "Auto"            # Auto follows macOS: Qur Light / Qur Dark

[window]
# opacity = 1.0             # 0.5 to 1.0
# blur = true

[editor]
# font_family = ""          # empty = SF Mono if installed, else Menlo
# font_size = 13
# line_height = 1.5
# soft_wrap = true
# tab_width = 4

[ui]
# font_size = 13
# zoom = 1.0

[panels]
# terminal = true
# git = true
# search = true

[keys]
# vim = false
"#;

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        if !self.first_frame {
            self.first_frame = true;
            let handle: AnyWindowHandle = window.window_handle();
            let me = cx.entity();
            window.on_next_frame(move |window, cx| {
                crate::first_frame(handle, cx);
                // Menus and the main-checkout watcher start after the first frame.
                me.update(cx, |ws, cx| {
                    ws.sync_menus(cx);
                    ws.watch_root(window, cx);
                });
            });
            // Dev/test: QUR_DEBUG_TASKS="prompt one|prompt two" starts agent tasks at launch.
            if let Ok(list) = std::env::var("QUR_DEBUG_TASKS") {
                for p in list.split('|').map(str::trim).filter(|p| !p.is_empty()) {
                    self.start_task(0, p.to_string(), Vec::new(), window, cx);
                }
            }
            if std::env::var("QUR_BENCH").as_deref() == Ok("glow") {
                self.bench_glow(window, cx);
            }
            if std::env::var("QUR_BENCH").as_deref() == Ok("theme") {
                let me = cx.entity();
                window.on_next_frame(move |window, cx| me.update(cx, |ws, cx| ws.bench_theme_step(0, Vec::new(), window, cx)));
            }
        }
        // Native web views draw above GPUI: only the selected task's browser
        // shows, and never under a modal, picker or popover (decision 26).
        let covered = self.overlay.is_some() || self.code.is_some();
        for a in self.agents.clone() {
            let (id, wants, browser) = {
                let t = a.read(cx);
                (t.record.id.clone(), t.shows_browser(), t.browser.clone())
            };
            let show = wants && !covered && self.selected == Some(Selected::Agent(id));
            browser.update(cx, |b, cx| b.set_shown(show, window, cx));
        }
        let body = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .when(self.sidebar, |d| d.child(self.sidebar(&look, cx)))
            .child(div().flex_1().min_w(px(0.)).h_full().child(self.stage(&look)));
        let any_overlay = self.overlay.is_some();
        div()
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .font_family(look.ui_font.clone())
            .text_color(look.c(ui.text))
            .bg(look.bg(ui.window_bg))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::hide))
            .on_action(cx.listener(Self::hide_others))
            .on_action(cx.listener(Self::show_all))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::minimize))
            .on_action(cx.listener(Self::zoom_window))
            .on_action(cx.listener(Self::fullscreen))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::reveal_files))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::open_config))
            .on_action(cx.listener(Self::open_agents))
            .on_action(cx.listener(Self::new_window))
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::save_all))
            .on_action(cx.listener(Self::revert))
            .on_action(cx.listener(Self::recent_file))
            .on_action(cx.listener(Self::nav_back))
            .on_action(cx.listener(Self::nav_forward))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::replace))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::use_selection))
            .on_action(cx.listener(Self::soft_wrap))
            .on_action(cx.listener(Self::noop_undo))
            .on_action(cx.listener(Self::noop_redo))
            .on_action(cx.listener(Self::palette))
            .on_action(cx.listener(Self::quick_open))
            .on_action(cx.listener(Self::theme_picker))
            .on_action(cx.listener(Self::use_default_theme))
            .on_action(cx.listener(Self::project_switcher))
            .on_action(cx.listener(Self::goto_line))
            .on_action(cx.listener(Self::transparency))
            .on_action(cx.listener(Self::new_terminal))
            .on_action(cx.listener(Self::new_agent_terminal))
            .on_action(cx.listener(Self::new_agent_task))
            .on_action(cx.listener(Self::close_terminal))
            .on_action(cx.listener(Self::next_terminal))
            .on_action(cx.listener(Self::prev_terminal))
            .on_action(cx.listener(Self::next_session))
            .on_action(cx.listener(Self::prev_session))
            .on_action(cx.listener(Self::select_session))
            .on_action(cx.listener(Self::focus_prompt))
            .on_action(cx.listener(Self::stop_agent))
            .on_action(cx.listener(Self::allow_once))
            .on_action(cx.listener(Self::allow_always))
            .on_action(cx.listener(Self::deny))
            .on_action(cx.listener(Self::reveal_worktree))
            .on_action(cx.listener(Self::worktree_terminal))
            .on_action(cx.listener(Self::discard_task))
            .on_action(cx.listener(Self::cycle_layout))
            .on_action(cx.listener(Self::layout_a))
            .on_action(cx.listener(Self::layout_b))
            .on_action(cx.listener(Self::layout_c))
            .on_action(cx.listener(Self::peek_live))
            .on_action(cx.listener(Self::peek_todo))
            .on_action(cx.listener(Self::peek_chat))
            .on_action(cx.listener(Self::peek_changes))
            .on_action(cx.listener(Self::peek_browser))
            .on_action(cx.listener(Self::toggle_inspect))
            .on_action(cx.listener(Self::follow_agent))
            .on_action(cx.listener(Self::take_over))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" && matches!(this.overlay, Some(Overlay::Transparency { .. })) {
                    this.save_transparency(cx);
                    this.close_overlays(window, cx);
                }
            }))
            // Sidebar resizing follows the mouse anywhere in the window.
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                if this.resizing {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        let zoom = Look::of(window, cx).zoom;
                        this.sidebar_w = (f32::from(ev.position.x) / zoom).clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W);
                        cx.notify();
                    } else {
                        this.resizing = false;
                    }
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.resizing = false))
            .child(self.title_bar(&look, cx))
            .child(body)
            .children(self.code_modal(&look, cx))
            .when(any_overlay, |d| {
                d.child(div().absolute().size_full().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.save_transparency(cx);
                        this.close_overlays(window, cx);
                    }),
                ))
            })
            .children(self.overlay_el(&look, cx))
    }
}
