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
use crate::alerts;
use crate::changes_view::{ChangesEvent, ChangesView};
use crate::dock::{self, Docks};
use crate::git_panel::{GitEvent, GitPanel};
use crate::live::{LiveEvent, LivePane};
use crate::monitor::{MonitorEvent, TaskMonitor};
use crate::assets::{self, icon};
use crate::browser_view::{Attachment, BrowserPane, BrowserPaneEvent};
use crate::commands::{self, MenuState, agent as aa, edit, editor as ea, qur, terminal as ta};
use crate::editor_view::{Buffers, EditorEvent, EditorView};
use crate::input::{InputEvent, TextInput};
use crate::look::Look;
use crate::picker::{PickItem, Picker, PickerEvent};
use crate::prompt::{PromptEvent, PromptInput};
use crate::search_view::{SearchEvent, SearchPanel};
use crate::setup_view::{SetupEvent, SetupView};
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
    changes: Entity<ChangesView>,
    browser: Entity<BrowserPane>,
    /// New Agent Task text and setup command, used on the first start only.
    first_run: Option<FirstRun>,
    /// Task text to type once the agent first goes quiet (`prompt = "type"`).
    pending_type: Option<String>,
    /// Turn detection (M5): the terminal was busy, and files changed meanwhile.
    was_busy: bool,
    /// When the current busy stretch began (alerts: only long ones count).
    busy_since: Option<Instant>,
    wrote: bool,
    /// The agent's to-do list, from Claude's transcript (To-do tab).
    todos: qur_agents::todos::TodoTracker,
}

struct FirstRun {
    prompt: String,
    setup: Option<String>,
}

/// A merge stopped on conflicts (resolved in the code modal, then Finish).
struct MergeState {
    task: String,
    title: String,
    branch: String,
    files: Vec<PathBuf>,
}

/// What the stage shows.
/// A task's latest turn-end check (decision 29).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Check {
    Running,
    Passed,
    Failed,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Selected {
    Agent(String),
    TermTask(String),
    Shell(usize),
    /// The Git panel for the main checkout.
    Git,
    /// Project search (M6) over `search`'s root.
    Search,
}

enum PickKind {
    Commands(Vec<&'static commands::Cmd>),
    /// Open a file (relative to `root`) in the code modal.
    Files { root: PathBuf, files: Vec<PathBuf> },
    /// Insert an `@file` mention into a prompt.
    Mention { root: PathBuf, files: Vec<PathBuf>, target: Entity<PromptInput> },
    Themes { names: Vec<String>, project_scope: bool },
    Projects(Vec<PathBuf>),
    /// Agents, Agent Terminals and shells (not subagents), for Ctrl+T.
    Sessions(Vec<Selected>),
    /// The agent's session settings (`settings`, their ids: each opens its
    /// values), then its slash commands (`names`, inserted into its prompt).
    Slash { agent: Entity<AgentTask>, settings: Vec<String>, names: Vec<String>, target: Entity<PromptInput> },
    /// A session setting's values (mode, model…) for one agent.
    Setting { agent: Entity<AgentTask>, id: String, values: Vec<String> },
}

enum Overlay {
    Picker(Entity<Picker>, PickKind),
    GoTo(Entity<TextInput>),
    /// Rename Agent…: the selected agent's name.
    Rename(Entity<TextInput>),
    /// The first-run picker (M6), also Qur > Setup….
    Setup(Entity<SetupView>),
    /// Qur > Accent Color….
    Accent,
    /// Right-click on an agent's button: its commands, at the pointer.
    AgentMenu { id: String, at: gpui::Point<Pixels> },
    /// View > Choose Stage Layout….
    Layouts,
    /// About Qur (M7).
    About,
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
    /// Accent being previewed in the Accent Color popover (not saved yet).
    preview_accent: Option<usize>,
    /// Accent hue and brightness while a slider is dragged (saved on release).
    preview_tune: Option<(f32, f32)>,
    /// The Accent Color sliders (hue, brightness) as last painted, and the one being dragged.
    tune_sliders: [Rc<Cell<Bounds<Pixels>>>; 2],
    tuning: Option<usize>,
    /// The chosen accent before hue and brightness (the sliders' gradients).
    accent_base: Rgba,
    roster: Roster,
    agents: Vec<Entity<AgentTask>>,
    term_tasks: Vec<TermTask>,
    /// Plain shells (terminal ids), in creation order.
    shells: Vec<usize>,
    terms: Entity<TerminalPanel>,
    selected: Option<Selected>,
    /// Sessions that finished, errored or need you while not selected (Orca's unread).
    unread: HashSet<String>,
    checks: HashMap<String, Check>,
    /// The status each agent was last alerted for (one alert per change).
    alerted: HashMap<String, Status>,
    /// Run Project sessions (terminal ids) per task.
    runs: HashMap<String, usize>,
    /// One file tree per root (main checkout and each worktree).
    trees: HashMap<PathBuf, Entity<TreeView>>,
    buffers: Buffers,
    /// The code modal (decision 18).
    code: Option<Entity<EditorView>>,
    code_prev_focus: Option<FocusHandle>,
    sidebar: bool,
    sidebar_w: f32,
    /// `caffeinate` while Keep Awake is on (agents keep working with the lid
    /// open and the screen idle). It watches Qur's pid, so it ends with Qur.
    caffeinate: Option<std::process::Child>,
    /// FILES is the one collapsible section in the left column.
    files_open: bool,
    resizing: bool,
    overlay: Option<Overlay>,
    /// File picker shown over the New Agent Task dialog (`@` mentions).
    mention_over_dialog: Option<Entity<Picker>>,
    prev_focus: Option<FocusHandle>,
    /// Setup, set aside while its install or sign-in runs in a terminal.
    parked_setup: Option<Entity<SetupView>>,
    /// Back/forward history of files opened in the modal: (file, cursor).
    nav: Vec<(PathBuf, Point)>,
    nav_ix: usize,
    message: Option<(SharedString, Instant)>,
    message_timer: Option<Task<()>>,
    file_lists: HashMap<PathBuf, Vec<PathBuf>>,
    layout: Layout,
    _root_watcher: Option<qur_watch::Watcher>,
    git: Option<Entity<GitPanel>>,
    search: Option<Entity<SearchPanel>>,
    merge: Option<MergeState>,
    slider: Rc<Cell<Bounds<Pixels>>>,
    sliding: bool,
    title_drag: bool,
    /// Full screen hides the traffic lights, so the title bar starts at the edge.
    fullscreen: bool,
    first_frame: bool,
    _subs: Vec<Subscription>,
}

impl Workspace {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::mark("ws_new");
        let (config, err) = Config::load();
        if !cx.has_global::<Docks>() {
            Docks::load(&config, cx);
        }
        let project_id = qur_config::project_id(&root);
        crate::mark("project_id");
        let project = ProjectSettings::load(&project_id);
        let (roster, roster_err) = Roster::load();
        let terms = cx.new(TerminalPanel::new);
        let subs = vec![
            cx.subscribe_in(&terms, window, |this, _, ev: &PanelEvent, window, cx| {
                match ev {
                    PanelEvent::Closed => this.prune_closed_terms(window, cx),
                    PanelEvent::Activity => this.terminal_activity(window, cx),
                    // Cmd-click in a terminal (M5): the user asked for it.
                    PanelEvent::OpenUrl(url) => {
                        if let Err(e) = std::process::Command::new("/usr/bin/open").arg(url).spawn() {
                            this.flash(format!("Couldn't open {url}: {e}"), cx);
                        }
                    }
                    PanelEvent::OpenPath(path, at) => {
                        let at = at.map(|(l, c)| Point { line: l.saturating_sub(1) as usize, col: c.saturating_sub(1) as usize });
                        let path = path.clone();
                        this.open_path_at(&path, at, window, cx);
                    }
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
                    // Coming back to the agent you were on counts as seeing it.
                    if let Some(Selected::Agent(id) | Selected::TermTask(id)) = this.selected.clone() {
                        this.seen(&id, window, cx);
                    }
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
        // First run (M6): no config.toml yet. A home that already has one
        // predates the picker and is left alone.
        let first_run = !state.setup_done && !qur_config::config_path().exists();
        state.setup_done = true;
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
            preview_accent: None,
            preview_tune: None,
            tune_sliders: Default::default(),
            tuning: None,
            accent_base: Rgba(0),
            roster,
            agents: Vec::new(),
            term_tasks: Vec::new(),
            shells: Vec::new(),
            terms,
            selected: None,
            unread: HashSet::new(),
            checks: HashMap::new(),
            alerted: HashMap::new(),
            runs: HashMap::new(),
            trees: HashMap::new(),
            buffers: Buffers { list: Vec::new() },
            code: None,
            code_prev_focus: None,
            sidebar: true,
            sidebar_w: SIDEBAR_DEFAULT_W,
            caffeinate: None,
            files_open: true,
            resizing: false,
            overlay: None,
            mention_over_dialog: None,
            prev_focus: None,
            parked_setup: None,
            nav: Vec::new(),
            nav_ix: 0,
            message: None,
            message_timer: None,
            file_lists: HashMap::new(),
            layout: Layout::A,
            _root_watcher: None,
            git: None,
            search: None,
            merge: None,
            slider: Rc::new(Cell::new(Bounds::default())),
            sliding: false,
            title_drag: false,
            fullscreen: false,
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
        ws.restore_open_files(window, cx);
        crate::mark("files_restored");
        // The first-run picker, once (not in scripted runs, unless asked).
        let scripted = ["QUR_DEBUG_ACTIONS", "QUR_DEBUG_TASKS", "QUR_BENCH"].iter().any(|v| std::env::var_os(v).is_some());
        if (first_run && !scripted) || std::env::var_os("QUR_DEBUG_SETUP").is_some() {
            ws.open_setup(&qur::CheckSetup, window, cx);
        }
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
        // Your accent first ([ui] accent): agents take the palette in order,
        // tuned by the Accent Color sliders; focus outlines follow it.
        let k = self.preview_accent.unwrap_or(self.config.ui.accent) % 6;
        let (hue, brightness) = self.preview_tune.unwrap_or((self.config.ui.accent_hue, self.config.ui.accent_brightness));
        self.accent_base = theme.accents[k].base;
        let theme = std::sync::Arc::new(theme.with_accent(k, hue, brightness));
        let min = qur_theme::MIN_OPACITY;
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
            t.browser.update(cx, |b, _| b.accent = accent);
            slot += 1;
        }
        window.refresh();
        cx.notify();
    }

    /// Re-read config.toml and this project's settings.
    pub fn reload_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (config, err) = Config::load();
        self.config = config;
        Docks::load(&self.config, cx);
        self.project = ProjectSettings::load(&self.project_id);
        if let Some(e) = err {
            self.flash(e, cx);
        }
        let wrap = self.config.editor.soft_wrap;
        for b in &self.buffers.list {
            b.update(cx, |b, cx| b.set_soft_wrap(wrap, cx));
            let vim = self.config.keys.vim;
            b.update(cx, |b, cx| b.set_vim(vim, cx));
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
        let msg = msg.into();
        // Scripted runs (QUR_DEBUG_ACTIONS) log the top-bar messages.
        if std::env::var_os("QUR_DEBUG_ACTIONS").is_some() {
            println!("FLASH {msg}");
        }
        self.message = Some((msg, Instant::now()));
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
            merging: self.merge.is_some(),
            vim: self.config.keys.vim,
            keep_awake: self.config.ui.keep_awake,
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
            Some(Selected::Search) => self.search.as_ref().map(|s| s.read(cx).root.clone()).unwrap_or(self.root.clone()),
            _ => self.root.clone(),
        }
    }

    fn agent(&self, id: &str) -> Option<&Entity<AgentTask>> {
        self.agents.iter().find(|a| agent_id_matches(a, id))
    }

    /// The page keeps the keyboard until told otherwise: typing went to it
    /// after you'd clicked back into Qur or picked with Inspect.
    fn release_browser_focus(&self, cx: &App) {
        for a in &self.agents {
            a.read(cx).browser.read(cx).release_focus();
        }
        for t in &self.term_tasks {
            t.browser.read(cx).release_focus();
        }
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
                let id = id.clone();
                self.seen(&id, window, cx);
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
            Some(Selected::Git) => {
                self.git_panel(window, cx);
                window.focus(&self.focus, cx);
            }
            Some(Selected::Search) => {
                if let Some(s) = &self.search {
                    s.update(cx, |s, cx| s.focus_query(window, cx));
                }
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
        let todo_id = record.id.clone();
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
        if let Some(png) = &a.png {
            let path = std::env::temp_dir().join(format!("qur-pick-{}.png", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())));
            if std::fs::write(&path, png).is_ok() {
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
        let root = self.root.clone();
        let a = cx.new(|cx| AgentTask::new(root, record, def, accent, layout, wrap, tab, window, cx));
        // Messages still queued when Qur last quit: start the agent so they go out.
        a.update(cx, |a, cx| {
            if !a.record.queued.is_empty() && a.status == Status::Stopped {
                a.start(cx);
            }
        });
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
                cx.notify();
            }
            AgentEvent::Subagents => cx.notify(),
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
                        renamed: false,
                        in_root: false,
                        queued: Vec::new(),
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
    fn new_agent_terminal(&mut self, _: &qur::NewAgentTerminal, window: &mut Window, cx: &mut Context<Self>) {
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
    fn terminal_activity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut turns = Vec::new();
        let mut typed = Vec::new();
        let mut finished = Vec::new();
        let mut renamed = false;
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
    fn seen(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Always the project folder; Open Worktree in Terminal is the way into an agent's worktree.
    fn new_terminal(&mut self, _: &qur::NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.new_terminal_in(self.root.clone(), window, cx)
    }

    /// Split the selected terminal (a shell or an Agent Terminal): a new shell
    /// in the project folder, like Cmd+T. Below it when `vertical` is
    /// `Some(true)`; `None` halves it along its longer side (dwindle).
    fn split_terminal(&mut self, vertical: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
        let live = match &self.selected {
            Some(Selected::Shell(_)) => true,
            Some(Selected::TermTask(id)) => self.term_task(id).and_then(|t| t.term_id).is_some(),
            _ => false,
        };
        if !live {
            return self.flash("Select a terminal to split", cx);
        }
        let root = self.root.clone();
        self.terms.update(cx, |p, cx| match vertical {
            Some(v) => p.split_active(&root, v, false, cx),
            None => p.dwindle_active(&root, cx),
        });
        let f = self.terms.read(cx).focus.clone();
        window.focus(&f, cx);
    }

    /// Cmd+Enter (like Mod+Enter in i3): another terminal, the ones on screen
    /// laid out as a grid of near-square panes; with no terminal shown, a new one.
    fn tile_terminal(&mut self, _: &ta::Tile, window: &mut Window, cx: &mut Context<Self>) {
        let live = match &self.selected {
            Some(Selected::Shell(_)) => true,
            Some(Selected::TermTask(id)) => self.term_task(id).and_then(|t| t.term_id).is_some(),
            _ => false,
        };
        if !live {
            return self.new_terminal(&qur::NewTerminal, window, cx);
        }
        let root = self.root.clone();
        self.terms.update(cx, |p, cx| p.grid_active(&root, cx));
        let f = self.terms.read(cx).focus.clone();
        window.focus(&f, cx);
    }

    fn split_right(&mut self, _: &ta::SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        self.split_terminal(None, window, cx)
    }
    fn split_down(&mut self, _: &ta::SplitDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split_terminal(Some(true), window, cx)
    }

    /// A terminal closed (exit or Cmd+W): forget shells; terminal tasks keep
    /// their row (the task persists) and restart when selected again.
    fn prune_closed_terms(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.terms.read(cx);
        let live: Vec<usize> = panel.sessions.iter().map(|s| s.id).collect();
        // A split shell's row moves to the pane beside it when its own closes.
        for s in &mut self.shells {
            if !live.contains(s)
                && let Some(n) = panel.successor(*s)
            {
                if self.selected == Some(Selected::Shell(*s)) {
                    self.selected = Some(Selected::Shell(n));
                }
                *s = n;
            }
        }
        self.shells.retain(|s| live.contains(s));
        self.runs.retain(|_, s| live.contains(s));
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

    fn close_terminal(&mut self, _: &qur::CloseTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let tid = match &self.selected {
            Some(Selected::Shell(s)) => Some(*s),
            Some(Selected::TermTask(id)) => self.term_task(id).and_then(|t| t.term_id),
            Some(Selected::Agent(_)) => return self.discard_task(&qur::DiscardTask, window, cx),
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

    /// Ctrl+T: pick an agent or terminal by name. The list starts after the
    /// one you're in (so Ctrl+T, Return goes to the next), which comes last.
    fn switch_session(&mut self, _: &qur::SwitchSession, window: &mut Window, cx: &mut Context<Self>) {
        let mut order = self.session_order();
        if order.is_empty() {
            return self.flash("No agents or terminals yet", cx);
        }
        if let Some(i) = self.selected.as_ref().and_then(|s| order.iter().position(|o| o == s)) {
            order.rotate_left(i + 1);
        }
        let panel = self.terms.read(cx);
        let items = order
            .iter()
            .map(|s| {
                let (title, kind) = match s {
                    Selected::Agent(id) => (self.agent(id).map(|a| a.read(cx).record.title.clone()).unwrap_or_default(), "Agent"),
                    Selected::TermTask(id) => (self.term_task(id).map(|t| t.record.title.clone()).unwrap_or_default(), "Agent Terminal"),
                    Selected::Shell(tid) => (panel.session_by_id(*tid).map(|s| s.label.clone()).unwrap_or_default(), "Terminal"),
                    _ => (String::new(), ""),
                };
                let item = PickItem::new(title).detail(kind);
                if Some(s) == self.selected.as_ref() { item.right("current") } else { item }
            })
            .collect();
        self.open_picker("Switch to an agent or terminal…", items, PickKind::Sessions(order), false, window, cx);
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

    /// "/": put the keyboard in the input field of what's on screen (the
    /// agent's prompt, the search box, the terminal), like a browser's search.
    fn focus_input(&mut self, _: &qur::FocusInput, window: &mut Window, cx: &mut Context<Self>) {
        let f = match &self.selected {
            Some(Selected::Agent(id)) => self.agent(id).map(|a| a.read(cx).prompt.read(cx).focus.clone()),
            Some(Selected::Search) => self.search.as_ref().map(|s| s.read(cx).query.read(cx).focus.clone()),
            Some(Selected::Shell(_) | Selected::TermTask(_)) => Some(self.terms.read(cx).focus.clone()),
            _ => None,
        };
        if let Some(f) = f {
            window.focus(&f, cx);
        }
    }

    /// View > Peek Layout A/B/C: reset both docks to that preset.
    fn set_layout(&mut self, l: Layout, cx: &mut Context<Self>) {
        self.layout = l;
        Docks::update(cx, dock::Kind::Acp, |d| *d = dock::Dock::preset(dock::Kind::Acp, l.as_str()));
        Docks::update(cx, dock::Kind::Terminal, |d| *d = dock::Dock::preset(dock::Kind::Terminal, l.as_str()));
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
    /// Cmd+1..5: bring that panel to the front, wherever it's docked.
    fn peek(&mut self, v: View, cx: &mut Context<Self>) {
        match self.selected.clone() {
            Some(Selected::Agent(id)) => {
                if let Some(a) = self.agent(&id).cloned() {
                    a.update(cx, |a, cx| a.set_view(v, cx));
                }
            }
            Some(Selected::TermTask(id))
                if self.term_task(&id).is_some() && dock::Kind::Terminal.panels().contains(&v.panel()) =>
            {
                Docks::update(cx, dock::Kind::Terminal, |d| d.activate(v.panel()));
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
                let on = !a.shows_browser(cx);
                a.show_browser(on, cx)
            });
        } else if matches!(self.selected, Some(Selected::TermTask(_))) {
            let on = !Docks::get(cx, dock::Kind::Terminal).is_visible(dock::Panel::Browser);
            Docks::update(cx, dock::Kind::Terminal, |d| d.activate(if on { dock::Panel::Browser } else { dock::Panel::Session }));
            cx.notify();
        }
    }
    /// Cmd+Shift+C: open the Browser if needed and toggle Inspect.
    fn toggle_inspect(&mut self, _: &qur::ToggleInspect, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.selected_agent() {
            a.update(cx, |a, cx| {
                if !a.shows_browser(cx) {
                    a.show_browser(true, cx);
                }
                a.browser.update(cx, |b, cx| b.toggle_inspect(cx));
            });
        } else if let Some(Selected::TermTask(id)) = &self.selected
            && let Some(browser) = self.term_task(id).map(|t| t.browser.clone())
        {
            Docks::update(cx, dock::Kind::Terminal, |d| d.activate(dock::Panel::Browser));
            browser.update(cx, |b, cx| b.toggle_inspect(cx));
            cx.notify();
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

    fn on_changes_event(&mut self, id: &str, ev: &ChangesEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            ChangesEvent::Merge => self.merge_task(id, window, cx),
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
        }
    }

    /// Worktree, branch, title and Changes view of a task by id.
    fn task_parts(&self, id: &str, cx: &App) -> Option<(PathBuf, String, String, Entity<ChangesView>)> {
        if let Some(a) = self.agent(id) {
            let a = a.read(cx);
            return Some((a.record.worktree.clone(), a.record.branch.clone(), a.record.title.clone(), a.changes.clone()));
        }
        self.term_task(id).map(|t| (t.record.worktree.clone(), t.record.branch.clone(), t.record.title.clone(), t.changes.clone()))
    }

    /// Merge (handoff section 10, M0 recipe): commit the task's work on its
    /// branch, then merge it into the main checkout's current branch without
    /// touching the user's index or stash. Conflicts open in the editor.
    fn merge_task(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
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

    fn in_root(&self, id: &str, cx: &App) -> bool {
        self.agent(id).is_some_and(|a| a.read(cx).record.in_root) || self.term_task(id).is_some_and(|t| t.record.in_root)
    }

    fn remove_task(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
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
        cx.spawn_in(window, async move |this, cx| {
            let r = bg
                .spawn(async move {
                    if let Some(h) = handle {
                        let _ = h.join();
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

    /// Merge > Accept Ours/Theirs/Both, Next/Previous Conflict: act on the
    /// conflicted file open in the code modal.
    /// The workspace's own focus node (scripted action dispatch, main.rs).
    pub fn root_focus(&self) -> FocusHandle {
        self.focus.clone()
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

    fn accept_ours(&mut self, _: &qur::AcceptOurs, _: &mut Window, cx: &mut Context<Self>) {
        self.with_conflict_editor(cx, |e, cx| e.resolve_current_conflict(qur_git::Resolution::Ours, cx));
    }

    fn accept_theirs(&mut self, _: &qur::AcceptTheirs, _: &mut Window, cx: &mut Context<Self>) {
        self.with_conflict_editor(cx, |e, cx| e.resolve_current_conflict(qur_git::Resolution::Theirs, cx));
    }

    fn accept_both(&mut self, _: &qur::AcceptBoth, _: &mut Window, cx: &mut Context<Self>) {
        self.with_conflict_editor(cx, |e, cx| e.resolve_current_conflict(qur_git::Resolution::Both, cx));
    }

    fn next_conflict(&mut self, _: &qur::NextConflict, _: &mut Window, cx: &mut Context<Self>) {
        self.with_conflict_editor(cx, |e, cx| e.step_conflict(true, cx));
    }

    fn prev_conflict(&mut self, _: &qur::PrevConflict, _: &mut Window, cx: &mut Context<Self>) {
        self.with_conflict_editor(cx, |e, cx| e.step_conflict(false, cx));
    }

    fn finish_merge_action(&mut self, _: &qur::FinishMerge, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_merge(window, cx);
    }

    fn abort_merge_action(&mut self, _: &qur::AbortMerge, _: &mut Window, cx: &mut Context<Self>) {
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

    /// Find in Project (Cmd+Shift+F), Replace in Project (Cmd+Shift+H), View >
    /// Search: the Search view over the tree you're looking at (the selected
    /// agent's worktree, or the main checkout).
    fn show_search(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.current_root(cx);
        // Seed the query with a one-line selection in the code modal.
        let seed = self
            .code
            .as_ref()
            .and_then(|c| c.read(cx).selected_text())
            .filter(|t| !t.is_empty() && !t.contains('\n'))
            // Dev/test: QUR_DEBUG_SEARCH=<query> fills the field.
            .or_else(|| std::env::var("QUR_DEBUG_SEARCH").ok());
        let panel = match &self.search {
            Some(s) => s.clone(),
            None => {
                let s = cx.new(|cx| SearchPanel::new(root.clone(), window, cx));
                let sub = cx.subscribe_in(&s, window, |this, panel, ev: &SearchEvent, window, cx| match ev {
                    SearchEvent::Open(path, line, col) => {
                        let path = path.clone();
                        this.open_path_at(&path, Some(Point { line: *line, col: *col }), window, cx);
                    }
                    SearchEvent::Message(m) => this.flash(m.clone(), cx),
                    SearchEvent::ConfirmReplace { files, matches } => {
                        let detail = format!(
                            "Replace {matches} match{} in {files} file{} under {}? Open files reload from disk.",
                            if *matches == 1 { "" } else { "es" },
                            if *files == 1 { "" } else { "s" },
                            panel.read(cx).root.display()
                        );
                        let answer = ask(window, PromptLevel::Warning, "Replace All", Some(&detail), &["Replace", "Cancel"], cx);
                        let panel = panel.clone();
                        cx.spawn_in(window, async move |_, cx| {
                            if answer.await.ok() == Some(0) {
                                panel.update(cx, |p, cx| p.replace_all(cx));
                            }
                        })
                        .detach();
                    }
                });
                self._subs.push(sub);
                self.search = Some(s.clone());
                s
            }
        };
        panel.update(cx, |p, cx| {
            if let Some(seed) = seed {
                p.query.update(cx, |q, cx| q.set_text(&seed, cx));
                p.run(cx);
            }
            p.set_root(root, cx);
        });
        self.select(Some(Selected::Search), window, cx);
        if replace {
            panel.update(cx, |p, cx| p.focus_replace(window, cx));
        }
    }

    fn find_in_project(&mut self, _: &qur::FindInProject, window: &mut Window, cx: &mut Context<Self>) {
        self.show_search(false, window, cx);
    }

    fn replace_in_project(&mut self, _: &qur::ReplaceInProject, window: &mut Window, cx: &mut Context<Self>) {
        self.show_search(true, window, cx);
    }

    fn search_panel(&mut self, _: &qur::SearchPanel, window: &mut Window, cx: &mut Context<Self>) {
        self.show_search(false, window, cx);
    }

    fn git_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<GitPanel> {
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

    fn show_git(&mut self, _: &qur::GitPanel, window: &mut Window, cx: &mut Context<Self>) {
        let g = self.git_panel(window, cx);
        g.update(cx, |g, cx| g.refresh(cx));
        self.select(Some(Selected::Git), window, cx);
    }

    fn git_stage_all(&mut self, _: &qur::GitStageAll, window: &mut Window, cx: &mut Context<Self>) {
        let g = self.git_panel(window, cx);
        g.update(cx, |g, cx| g.stage_everything(cx));
    }

    fn git_unstage_all(&mut self, _: &qur::GitUnstageAll, window: &mut Window, cx: &mut Context<Self>) {
        let g = self.git_panel(window, cx);
        g.update(cx, |g, cx| g.unstage_everything(cx));
    }

    fn git_commit(&mut self, _: &qur::GitCommit, window: &mut Window, cx: &mut Context<Self>) {
        let g = self.git_panel(window, cx);
        g.update(cx, |g, cx| g.refresh(cx));
        self.select(Some(Selected::Git), window, cx);
        g.update(cx, |g, cx| g.commit_or_focus(window, cx));
    }

    fn merge_action(&mut self, _: &qur::Merge, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, ..)) = self.selected_worktree(cx) {
            self.merge_task(&id, window, cx);
        }
    }

    fn review_changes(&mut self, _: &qur::ReviewChanges, _: &mut Window, cx: &mut Context<Self>) {
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

    fn keep_working(&mut self, _: &qur::KeepWorking, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.selected, Some(Selected::TermTask(_))) {
            Docks::update(cx, dock::Kind::Terminal, |d| d.activate(dock::Panel::Session));
        }
        self.focus_prompt(&qur::FocusPrompt, window, cx);
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

    fn run_checks_action(&mut self, _: &qur::RunChecks, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, ..)) = self.selected_worktree(cx) {
            self.run_checks(&id, true, window, cx);
        }
    }

    /// Run the project's `[checks] command` in a task's worktree (decision 29):
    /// after an ACP turn that changed files, or by hand (Run Checks Now, also
    /// for Agent Terminals, which have no turn boundaries). A failure goes back
    /// to the agent as text to review: the prompt (ACP) or the terminal's input
    /// line (Agent Terminal), not sent unless `auto_send` is on.
    fn run_checks(&mut self, id: &str, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
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
    fn run_project(&mut self, _: &qur::RunProject, window: &mut Window, cx: &mut Context<Self>) {
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
        self.save_open_files(cx);
    }

    // -- the code modal ------------------------------------------------------------

    pub fn open_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.open_path_at(path, None, window, cx)
    }

    fn open_path_at(&mut self, path: &Path, at: Option<Point>, window: &mut Window, cx: &mut Context<Self>) {
        self.record_nav(cx);
        let buf = match self.buffers.find(path, cx) {
            Some(b) => b,
            None => match self.load_buffer(path, window, cx) {
                Ok(b) => b,
                Err(e) => {
                    self.flash(format!("Couldn't open {}: {e}", path.display()), cx);
                    return;
                }
            },
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

    fn load_buffer(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> std::io::Result<Entity<EditorView>> {
        let (wrap, tab, vim) = (self.config.editor.soft_wrap, self.config.editor.tab_width, self.config.keys.vim);
        let loaded = EditorView::load(path)?;
        let view = cx.new(|cx| {
            let mut v = EditorView::new(loaded, wrap, tab, cx);
            v.set_vim(vim, cx);
            v
        });
        let sub = cx.subscribe_in(&view, window, Self::on_editor_event);
        self._subs.push(sub);
        Ok(view)
    }

    /// Reopen the files the project had open last time (quietly: a file that's
    /// gone is skipped). The code view comes back if it was showing. Clones and
    /// worktrees share a project id, so files outside this root are skipped too.
    fn restore_open_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = qur_config::ProjectState::load(&self.project_id);
        // Oldest first, so the list ends up most recent first.
        for f in state.files.iter().take(qur_config::MAX_OPEN_FILES).rev() {
            if !f.path.starts_with(&self.root) || !f.path.is_file() || self.buffers.find(&f.path, cx).is_some() {
                continue;
            }
            let Ok(buf) = self.load_buffer(&f.path, window, cx) else { continue };
            let at = Point { line: f.line, col: f.col };
            buf.update(cx, |b, cx| b.set_cursor(at, cx));
            self.buffers.touch(&buf);
        }
        if state.code_open
            && let Some(p) = self.buffers.list.first().and_then(|b| b.read(cx).path().map(Path::to_path_buf))
        {
            self.open_path(&p, window, cx);
        }
    }

    fn save_open_files(&self, cx: &App) {
        let files = self
            .buffers
            .list
            .iter()
            .filter_map(|b| {
                let b = b.read(cx);
                let at = b.cursor();
                b.path().map(|p| qur_config::OpenFile { path: p.to_path_buf(), line: at.line, col: at.col })
            })
            .collect();
        let state = qur_config::ProjectState { files, code_open: self.code.is_some() };
        if let Err(e) = state.save(&self.project_id) {
            crate::log(&format!("saving open files: {e}"));
        }
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
            Some(Selected::Search) => {
                if let Some(s) = &self.search {
                    let f = s.read(cx).query.read(cx).focus.clone();
                    window.focus(&f, cx);
                }
            }
            Some(Selected::Git) | None => window.focus(&self.focus, cx),
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

    /// Keep Awake: run `caffeinate -dims` (no display, idle or system sleep)
    /// until toggled off or Qur exits.
    fn caffeinate(&mut self, _: &qur::Caffeinate, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut c) = self.caffeinate.take() {
            let _ = c.kill();
            let _ = c.wait();
            self.flash("Keep Awake off", cx);
        } else {
            match std::process::Command::new("/usr/bin/caffeinate").args(["-dims", "-w", &std::process::id().to_string()]).spawn() {
                Ok(c) => {
                    self.caffeinate = Some(c);
                    self.flash("Keep Awake on: the Mac won't sleep while Qur is open", cx);
                }
                Err(e) => self.flash(format!("Couldn't start caffeinate: {e}"), cx),
            }
        }
        cx.notify();
    }

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
        let answer = ask(window, PromptLevel::Warning, &msg, None, &["Save All", "Don't Save", "Cancel"], cx);
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
        let answer = ask(window, PromptLevel::Warning, &msg, None, &["Save All", "Don't Save", "Cancel"], cx);
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

    /// A session setting's values (mode, model…), above the agent's prompt.
    fn pick_setting(&mut self, a: Entity<AgentTask>, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(o) = a.read(cx).settings.iter().find(|o| o.id == id).cloned() else { return };
        let values = o.values.iter().map(|v| v.value.clone()).collect();
        let items = o
            .values
            .iter()
            .map(|v| {
                let item = PickItem::new(v.name.clone()).detail(v.description.clone().unwrap_or_default());
                if v.value == o.current { item.right("current") } else { item }
            })
            .collect();
        let title = if o.is_mode() { format!("{}…  (⇧⇥ next)", o.name) } else { format!("{}…", o.name) };
        self.open_picker(&title, items, PickKind::Setting { agent: a, id: id.to_string(), values }, false, window, cx);
    }

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
            (PickerEvent::Confirm(i), PickKind::Slash { agent, settings, target, .. }) if *i < settings.len() => {
                let (agent, id, target) = (agent.clone(), settings[*i].clone(), target.clone());
                target.update(cx, |t, cx| t.set_text("", cx));
                self.pick_setting(agent, &id, window, cx);
            }
            (PickerEvent::Confirm(i), PickKind::Slash { settings, names, target, .. }) => {
                let text = format!("/{} ", names[*i - settings.len()]);
                let target = target.clone();
                self.overlay = None;
                self.prev_focus = None;
                target.update(cx, |t, cx| t.set_text(&text, cx));
                let f = target.read(cx).focus.clone();
                window.focus(&f, cx);
                cx.notify();
            }
            (PickerEvent::Confirm(i), PickKind::Setting { agent, id, values }) => {
                let (agent, id, value) = (agent.clone(), id.clone(), values.get(*i).cloned());
                self.close_overlays(window, cx);
                if let Some(v) = value {
                    agent.update(cx, |a, cx| a.set_setting(&id, &v, cx));
                }
                let f = agent.read(cx).prompt.read(cx).focus.clone();
                window.focus(&f, cx);
            }
            (PickerEvent::Confirm(i), PickKind::Sessions(order)) => {
                let target = order.get(*i).cloned();
                self.close_overlays(window, cx);
                if target.is_some() {
                    self.select(target, window, cx);
                }
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

    /// Rename Agent… (also double-click its button): the name sticks; an
    /// empty name goes back to following the agent.
    fn rename_agent(&mut self, _: &qur::RenameAgent, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Qur > Setup… (and the first launch): the first-run picker.
    /// Help > Run Doctor: `qur doctor`'s checks, in a sheet.
    fn run_doctor(&mut self, _: &qur::RunDoctor, window: &mut Window, cx: &mut Context<Self>) {
        self.flash("Running the doctor…", cx);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let checks = bg.spawn(async move { crate::doctor::run() }).await;
            this.update_in(cx, |this, window, cx| {
                let fails = checks.iter().filter(|c| c.level == crate::doctor::Level::Fail).count();
                let text = crate::doctor::render(&checks);
                crate::log(&format!("doctor:\n{text}"));
                let title = if fails == 0 { "Qur Doctor: all good".to_string() } else { format!("Qur Doctor: {fails} problem(s)") };
                drop(ask(window, if fails == 0 { PromptLevel::Info } else { PromptLevel::Warning }, &title, Some(&text), &["OK"], cx));
                this.message = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn keyboard_shortcuts(&mut self, _: &qur::KeyboardShortcuts, window: &mut Window, cx: &mut Context<Self>) {
        // The palette lists every command with its keys.
        self.palette(&qur::CommandPalette, window, cx);
    }

    fn show_logs(&mut self, _: &qur::ShowLogs, _: &mut Window, cx: &mut Context<Self>) {
        let dir = crate::log_dir();
        std::fs::create_dir_all(&dir).ok();
        if let Err(e) = std::process::Command::new("/usr/bin/open").arg(&dir).spawn() {
            self.flash(format!("Couldn't open {}: {e}", dir.display()), cx);
        }
    }

    fn report_problem(&mut self, _: &qur::ReportProblem, _: &mut Window, cx: &mut Context<Self>) {
        // Opens the browser; Qur itself makes no network call.
        if let Err(e) = std::process::Command::new("/usr/bin/open").arg("https://github.com/PEUR-Technologies/Qur/issues/new").spawn() {
            self.flash(format!("Couldn't open the browser: {e}"), cx);
        }
    }

    fn open_agent_menu(&mut self, which: Selected, at: gpui::Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let id = match &which {
            Selected::Agent(id) | Selected::TermTask(id) => id.clone(),
            _ => return,
        };
        self.close_overlays(window, cx);
        self.select(Some(which), window, cx);
        self.prev_focus = window.focused(cx);
        self.overlay = Some(Overlay::AgentMenu { id, at });
        cx.stop_propagation();
        cx.notify();
    }

    fn agent_menu(&self, id: &str, at: gpui::Point<Pixels>, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let in_root = self.in_root(id, cx);
        let acp = self.agent(id).is_some();
        type Make = fn() -> Box<dyn gpui::Action>;
        let mut items: Vec<(&'static str, Make, bool, bool)> = vec![
            ("Rename…", || Box::new(qur::RenameAgent), true, false),
            ("Review Changes", || Box::new(qur::ReviewChanges), true, false),
            ("Merge into My Branch", || Box::new(qur::Merge), !in_root, false),
            ("Run Checks Now", || Box::new(qur::RunChecks), true, false),
            ("Run Project", || Box::new(qur::RunProject), true, false),
            ("Reveal in Finder", || Box::new(qur::RevealWorktree), true, false),
            ("Open Folder in Terminal", || Box::new(qur::OpenWorktreeInTerminal), true, false),
        ];
        if acp {
            items.push(("Stop", || Box::new(qur::StopAgent), true, false));
        }
        items.push((if in_root { "Close Agent…" } else { "Delete Agent…" }, || Box::new(qur::DiscardTask), true, true));
        let n = items.len();
        div()
            .absolute()
            .left(at.x)
            .top(at.y)
            .child(
                div()
                    .id("agent-menu")
                    .occlude()
                    .min_w(look.z(210.))
                    .py(look.z(4.))
                    .flex()
                    .flex_col()
                    .bg(look.c(ui.overlay_bg))
                    .border_1()
                    .border_color(look.c(ui.border_strong))
                    .rounded(look.z(6.))
                    .shadow_md()
                    .children(items.into_iter().enumerate().flat_map(|(i, (label, make, enabled, danger))| {
                        let sep = (danger && i + 1 == n).then(|| div().my(look.z(4.)).h(px(1.)).bg(look.c(ui.border)).into_any_element());
                        let row = div()
                            .id(("agent-menu-item", i))
                            .mx(look.z(4.))
                            .px(look.z(10.))
                            .py(look.z(4.))
                            .rounded(look.z(4.))
                            .text_size(look.text(-1.))
                            .text_color(look.c(if !enabled { ui.text_muted } else if danger { ui.error } else { ui.text }))
                            .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(look.c(ui.hover_bg))))
                            .child(label)
                            .when(enabled, |d| {
                                d.on_click(cx.listener(move |this, _, window, cx| {
                                    this.overlay = None;
                                    this.restore_focus(window, cx);
                                    window.dispatch_action(make(), cx);
                                    cx.notify();
                                }))
                            })
                            .into_any_element();
                        sep.into_iter().chain(std::iter::once(row))
                    })),
            )
            .into_any_element()
    }

    /// Qur > Accent Color…: pick which of the theme's accents leads (hover to
    /// preview, click to keep).
    fn accent_color(&mut self, _: &qur::AccentColor, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        window.focus(&self.focus, cx);
        self.overlay = Some(Overlay::Accent);
        cx.notify();
    }

    fn set_accent(&mut self, k: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_accent = None;
        self.write_setting(false, "ui.accent", Some(Value::Int(k as i64)), cx);
        self.reload_settings(window, cx);
        self.broadcast_reload(cx);
    }

    /// Drag on an Accent Color slider (0 hue, 1 brightness): preview it.
    fn tune_to(&mut self, which: usize, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let b = self.tune_sliders[which].get();
        if b.size.width <= px(0.) {
            return;
        }
        let t = ((x - b.left()) / b.size.width).clamp(0., 1.);
        let (mut hue, mut brightness) = self.preview_tune.unwrap_or((self.config.ui.accent_hue, self.config.ui.accent_brightness));
        if which == 0 {
            hue = (-180. + 360. * t).round();
        } else {
            brightness = ((-1. + 2. * t) * 100.).round() / 100.;
        }
        self.preview_tune = Some((hue, brightness));
        self.apply_look(window, cx);
        cx.notify();
    }

    /// Save the sliders' hue and brightness (on release, or Reset).
    fn save_tune(&mut self, tune: (f32, f32), window: &mut Window, cx: &mut Context<Self>) {
        self.preview_tune = None;
        self.write_setting(false, "ui.accent_hue", Some(Value::from(tune.0)), cx);
        self.write_setting(false, "ui.accent_brightness", Some(Value::from(tune.1)), cx);
        self.reload_settings(window, cx);
        self.broadcast_reload(cx);
    }

    fn accent_box(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let (hue, brightness) = self.preview_tune.unwrap_or((self.config.ui.accent_hue, self.config.ui.accent_brightness));
        let base_color = self.accent_base;
        // A track painted with the colors it gives, and a thumb at `t` (0..=1).
        let slider = |which: usize, t: f32, color_at: &dyn Fn(f32) -> Rgba| {
            const STEPS: usize = 24;
            let bounds = self.tune_sliders[which].clone();
            let track_w = look.z(220.);
            div()
                .id(("accent-slider", which))
                .relative()
                .w(track_w)
                .h(look.z(16.))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                    this.tuning = Some(which);
                    this.tune_to(which, ev.position.x, window, cx);
                }))
                .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().size_full())
                .child(
                    div()
                        .absolute()
                        .top(look.z(4.))
                        .left_0()
                        .w(track_w)
                        .h(look.z(8.))
                        .rounded(look.z(4.))
                        .overflow_hidden()
                        .flex()
                        .children((0..STEPS).map(|i| div().flex_1().h_full().bg(look.c(color_at(i as f32 / (STEPS - 1) as f32))))),
                )
                .child(
                    div()
                        .absolute()
                        .top(look.z(1.))
                        .left(track_w * t - look.z(7.))
                        .size(look.z(14.))
                        .rounded_full()
                        .border_2()
                        .border_color(look.c(ui.text))
                        .bg(look.c(look.theme.accents[0].base)),
                )
        };
        let label = |name: &'static str, value: String| {
            div().w(look.z(220.)).flex().justify_between().text_size(look.text(-2.)).text_color(look.c(ui.text_secondary)).child(name).child(div().text_color(look.c(ui.text_muted)).child(value))
        };
        let tuned = hue != 0. || brightness != 0.;
        // The theme's own order: undo the rotation the look carries.
        let shown = self.preview_accent.unwrap_or(self.config.ui.accent) % 6;
        let saved = self.config.ui.accent % 6;
        let mut base = look.theme.accents;
        base.rotate_right(shown);
        div()
            .id("accent")
            .occlude()
            .p(look.z(16.))
            .flex()
            .flex_col()
            .gap(look.z(10.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(10.))
            .child(div().text_size(look.text(-1.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(look.c(ui.text)).child("Accent color"))
            .child(div().flex().gap(look.z(10.)).children(base.iter().enumerate().map(|(i, a)| {
                let on = i == saved;
                div()
                    .id(("accent-swatch", i))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(look.z(4.))
                    .cursor_pointer()
                    .child(div().size(look.z(30.)).rounded_full().bg(look.c(a.base)).border_2().border_color(look.c(if on { ui.text } else { ui.overlay_bg })))
                    .child(div().text_size(look.text(-3.)).text_color(look.c(if on { ui.text } else { ui.text_muted })).child(hue_name(a.base)))
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        let want = if *hovered { Some(i) } else { None };
                        if this.preview_accent != want {
                            this.preview_accent = want;
                            this.apply_look(window, cx);
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_accent(i, window, cx);
                        this.close_overlays(window, cx);
                    }))
            })))
            .child(label("Color", format!("{hue:+.0}°")))
            .child(slider(0, (hue + 180.) / 360., &|t| base_color.tuned(-180. + 360. * t, brightness)))
            .child(label("Brightness", format!("{:+.0}%", brightness * 100.)))
            .child(slider(1, (brightness + 1.) / 2., &|t| base_color.tuned(hue, -1. + 2. * t)))
            .when(tuned, |d| {
                d.child(
                    div()
                        .id("accent-reset")
                        .text_size(look.text(-2.))
                        .text_color(look.c(ui.text_secondary))
                        .cursor_pointer()
                        .hover(|d| d.text_color(look.c(ui.text)))
                        .child("Reset color and brightness")
                        .on_click(cx.listener(|this, _, window, cx| this.save_tune((0., 0.), window, cx))),
                )
            })
            .child(div().text_size(look.text(-3.)).text_color(look.c(ui.text_muted)).child("Agents take the theme's colors in order, starting here; the sliders tune them all. Focus outlines use your color. Saved as [ui] accent, accent_hue, accent_brightness."))
            .into_any_element()
    }

    /// View > Choose Stage Layout…: the presets (and yours) as diagrams.
    fn choose_layout(&mut self, _: &qur::ChooseLayout, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        window.focus(&self.focus, cx);
        self.overlay = Some(Overlay::Layouts);
        cx.notify();
    }

    fn layouts_box(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let preset = self.layout.as_str();
        let custom = Docks::get(cx, dock::Kind::Acp) != dock::Dock::preset(dock::Kind::Acp, preset) || Docks::get(cx, dock::Kind::Terminal) != dock::Dock::preset(dock::Kind::Terminal, preset);
        let selected = if custom { "yours" } else { preset };
        div()
            .id("layouts")
            .occlude()
            .p(look.z(16.))
            .flex()
            .flex_col()
            .gap(look.z(10.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(10.))
            .child(div().text_size(look.text(-1.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(look.c(ui.text)).child("Stage layout"))
            .child(dock::layout_cards(selected, look, cx, |this: &mut Workspace, key, cx| {
                if matches!(key, "A" | "B" | "C") {
                    this.set_layout(Layout::parse(key), cx);
                }
                this.overlay = None;
                cx.notify();
            }))
            .child(div().text_size(look.text(-3.)).text_color(look.c(ui.text_muted)).child("Then drag tabs and dividers to make it yours; it's saved as you go."))
            .into_any_element()
    }

    fn about(&mut self, _: &qur::About, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        window.focus(&self.focus, cx);
        self.overlay = Some(Overlay::About);
        cx.notify();
    }

    fn about_box(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let credit = look.theme.credit.as_ref().map(|c| format!("Theme “{}” by {} ({})", look.theme.name, c.author, c.license));
        let line = |s: String| div().text_size(look.text(-1.)).text_color(look.c(ui.text_secondary)).child(s);
        div()
            .id("about")
            .occlude()
            .w(look.z(380.))
            .p(look.z(22.))
            .flex()
            .flex_col()
            .items_center()
            .gap(look.z(6.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(10.))
            .child(
                svg()
                    .path(if look.logo_white() { "brand/svg/qur-horizontal-white.svg" } else { "brand/svg/qur-horizontal-black.svg" })
                    .w(look.z(132.))
                    .h(look.z(44.))
                    // Pure black or white only: the logo is never tinted.
                    .text_color(if look.logo_white() { gpui::white() } else { gpui::black() }),
            )
            .child(div().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child("by PEUR Technologies"))
            .child(div().h(look.z(8.)))
            .child(line(format!("Version {}", env!("CARGO_PKG_VERSION"))))
            .child(line("GPL-3.0-or-later · Inter (SIL OFL)".into()))
            .children(credit.map(line))
            .child(
                div()
                    .id("about-ok")
                    .mt(look.z(12.))
                    .px(look.z(16.))
                    .py(look.z(4.))
                    .rounded(look.z(5.))
                    .cursor_pointer()
                    .bg(look.c(ui.button_bg))
                    .text_color(look.c(ui.button_text))
                    .text_size(look.text(-1.))
                    .child("OK")
                    .on_click(cx.listener(|this, _, window, cx| this.close_overlays(window, cx))),
            )
            .into_any_element()
    }

    /// Qur > Vim Mode: on/off for every editor, saved as `[keys] vim`.
    fn toggle_vim(&mut self, _: &qur::VimMode, window: &mut Window, cx: &mut Context<Self>) {
        let on = !self.config.keys.vim;
        self.write_setting(false, "keys.vim", Some(Value::Bool(on)), cx);
        self.reload_settings(window, cx);
        self.broadcast_reload(cx);
        self.sync_menus(cx);
        self.flash(if on { "Vim mode on (Esc for normal mode)" } else { "Vim mode off" }, cx);
    }

    /// View > Show Keep Awake in Sidebar, saved as `[ui] keep_awake`.
    fn toggle_keep_awake(&mut self, _: &qur::ShowKeepAwake, window: &mut Window, cx: &mut Context<Self>) {
        let on = !self.config.ui.keep_awake;
        self.write_setting(false, "ui.keep_awake", Some(Value::Bool(on)), cx);
        self.reload_settings(window, cx);
        self.broadcast_reload(cx);
        self.sync_menus(cx);
    }

    fn open_setup(&mut self, _: &qur::CheckSetup, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        // Back from an install or sign-in: the same choices, looked at again.
        if let Some(view) = self.parked_setup.take() {
            view.update(cx, |v, cx| v.refresh(cx));
            let theme = view.read(cx).theme().to_string();
            if theme != self.config.theme {
                self.preview_theme = Some(theme);
                self.apply_look(window, cx);
            }
            let f = view.read(cx).focus.clone();
            window.focus(&f, cx);
            self.overlay = Some(Overlay::Setup(view));
            cx.notify();
            return;
        }
        let view = cx.new(|cx| SetupView::new(&self.config, &self.roster, cx));
        let sub = cx.subscribe_in(&view, window, |this, view, ev: &SetupEvent, window, cx| match ev {
            // Install or sign in, in a terminal of its own; Setup steps aside
            // and comes back by itself when the step has worked.
            SetupEvent::RunInTerminal { label, command } => {
                this.close_overlays(window, cx);
                this.parked_setup = Some(view.clone());
                let root = this.root.clone();
                let kind = Kind::Agent { task: label.clone(), command: command.clone() };
                let Some(tid) = this.terms.update(cx, |p, cx| p.open(&root, kind, label.clone(), cx)) else { return };
                this.shells.push(tid);
                this.select(Some(Selected::Shell(tid)), window, cx);
                this.flash(format!("{label}: Setup comes back when it's done (or Qur > Setup…)"), cx);
            }
            SetupEvent::Ready(msg) => {
                if this.parked_setup.is_some() && this.overlay.is_none() {
                    this.open_setup(&qur::CheckSetup, window, cx);
                }
                this.flash(msg.clone(), cx);
            }
            SetupEvent::PreviewTheme(name) => {
                this.preview_theme = Some(name.clone());
                this.apply_look(window, cx);
            }
            SetupEvent::PreviewAccent(k) => {
                this.preview_accent = Some(*k);
                this.apply_look(window, cx);
            }
            SetupEvent::Cancel => {
                this.parked_setup = None;
                this.close_overlays(window, cx);
            }
            SetupEvent::Done(c) => {
                this.parked_setup = None;
                this.preview_theme = None;
                this.write_setting(false, "theme", Some(c.theme.as_str().into()), cx);
                let preset = matches!(c.layout.as_str(), "A" | "B" | "C");
                if preset {
                    this.write_setting(false, "peek_layout", Some(c.layout.as_str().into()), cx);
                }
                this.write_setting(false, "keys.vim", Some(Value::Bool(c.vim)), cx);
                this.write_setting(false, "ui.accent", Some(Value::Int(c.accent as i64)), cx);
                this.preview_accent = None;
                if !c.agents.is_empty() {
                    // Keep agents added by hand; replace the ones the picker knows.
                    let mut roster = qur_agents::Roster::load().0;
                    roster.agents.retain(|a| !matches!(a.id.as_str(), "claude" | "claude-cli" | "codex" | "codex-cli" | "aider"));
                    let mut agents = c.agents.clone();
                    agents.extend(roster.agents);
                    roster.agents = agents;
                    if let Err(e) = roster.save() {
                        this.flash(format!("Couldn't save agents.toml: {e}"), cx);
                    }
                    this.roster = roster;
                }
                this.overlay = None;
                this.reload_settings(window, cx);
                // "Yours" keeps the arrangement as it is.
                if preset {
                    this.set_layout(Layout::parse(&c.layout), cx);
                }
                this.broadcast_reload(cx);
                this.restore_focus(window, cx);
                this.flash("Setup saved", cx);
            }
        });
        self._subs.push(sub);
        let f = view.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::Setup(view));
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
            PromptEvent::Changed | PromptEvent::SlashRequested | PromptEvent::UpInEmpty => {}
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
        self.tuning = None;
        if self.preview_accent.take().is_some() | self.preview_tune.take().is_some() {
            self.apply_look(window, cx);
        }
        if matches!(&self.overlay, Some(Overlay::Picker(_, PickKind::Themes { .. })) | Some(Overlay::Setup(_)))
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
        let min = qur_theme::MIN_OPACITY;
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
        let min = qur_theme::MIN_OPACITY;
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
                    renamed: false,
                    in_root: false,
                    queued: Vec::new(),
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
            Some(Selected::Git) => Some("Source control".into()),
            Some(Selected::Search) => Some("Search".into()),
            None => None,
        }
    }

    /// Where title bar content starts: past the traffic lights, or near the edge in full screen.
    fn title_lead(&self, look: &Look) -> Pixels {
        if self.fullscreen { look.z(8.) } else { px(TRAFFIC_LIGHTS_W) }
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
            .pl(self.title_lead(look))
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

    /// An agent in the AGENTS section: a button that fills with the agent's
    /// accent while it is active (working, or its terminal is busy). Clicking it
    /// opens the agent's session.
    #[allow(clippy::too_many_arguments)]
    fn agent_button(
        &self,
        id: SharedString,
        accent: Rgba,
        title: String,
        badge: String,
        active: bool,
        selected: bool,
        look: &Look,
    ) -> gpui::Stateful<gpui::Div> {
        let ui = &look.theme.ui;
        let tint = |a: f32| {
            let mut h = look.c(accent);
            h.a = a;
            h
        };
        div()
            .id(id)
            .mx(look.z(6.))
            .mb(look.z(4.))
            .px(look.z(8.))
            .py(look.z(5.))
            .rounded(look.z(5.))
            .border_1()
            .flex()
            .items_center()
            .gap(look.z(8.))
            .cursor_pointer()
            .map(|d| match (active, selected) {
                (true, _) => d.bg(tint(if selected { 0.32 } else { 0.22 })).border_color(look.c(accent)),
                (false, true) => d.bg(look.c(ui.active_bg)).border_color(tint(0.6)),
                (false, false) => d.border_color(look.c(ui.border)).hover(|d| d.bg(look.c(ui.hover_bg))),
            })
            .child(
                div()
                    .flex_none()
                    .size(look.z(8.))
                    .rounded_full()
                    .map(|d| if active { d.bg(look.c(accent)) } else { d.border_1().border_color(look.c(accent)) }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(look.text(-1.))
                    .text_color(look.c(if active || selected { ui.text } else { ui.text_secondary }))
                    .child(title),
            )
            .when(!badge.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(look.text(-2.))
                        .text_color(look.c(if badge.ends_with('!') { accent } else { ui.text_muted }))
                        .child(badge),
                )
            })
    }

    fn sidebar(&self, look: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = look.theme.ui.clone();
        let mut agents: Vec<gpui::AnyElement> = Vec::new();
        for a in &self.agents {
            let t = a.read(cx);
            let id = t.record.id.clone();
            let sel = self.selected == Some(Selected::Agent(id.clone()));
            let ch = t.changes.read(cx);
            let (add, del, conflicts) = (ch.totals.0, ch.totals.1, !ch.conflicts.is_empty());
            let active = matches!(t.status, Status::Working | Status::Starting | Status::NeedsYou);
            let mut badge = if add + del > 0 { format!("+{add} −{del}{}", if conflicts { " ⚠" } else { "" }) } else { String::new() };
            if !matches!(t.status, Status::Working | Status::Starting) {
                let g = t.status.glyph();
                if g != "·" {
                    badge = if badge.is_empty() { g.to_string() } else { format!("{badge} {g}") };
                }
            }
            badge = with_check(badge, self.checks.get(&id));
            let unread = self.unread.contains(&id);
            let row = self
                .agent_button(format!("agent-{id}").into(), t.accent, t.record.title.clone(), badge, active, sel, look)
                .on_mouse_down(MouseButton::Right, {
                    let id = id.clone();
                    cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| this.open_agent_menu(Selected::Agent(id.clone()), ev.position, window, cx))
                })
                .when(unread, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                    this.select(Some(Selected::Agent(id.clone())), window, cx);
                    // The agent's own row shows its chat, not a subagent's log.
                    if let Some(a) = this.agent(&id).cloned() {
                        a.update(cx, |a, cx| a.show_subagent(None, cx));
                    }
                    if ev.click_count() == 2 {
                        this.rename_agent(&qur::RenameAgent, window, cx);
                    }
                }));
            agents.push(row.into_any_element());
            // Its subagents, indented under it; a click shows the agent.
            let subs: Vec<_> = a.read(cx).subagents.iter().map(|s| (s.id.clone(), s.title.clone(), s.state.clone(), s.last.clone())).collect();
            for (sid, title, state, last) in subs {
                let (glyph, detail) = match state.as_str() {
                    "running" => ("~", last.unwrap_or_default()),
                    "requires_action" => ("!", "needs you".to_string()),
                    "idle" => ("✓", "done".to_string()),
                    other => ("", other.to_string()),
                };
                let id = a.read(cx).record.id.clone();
                let open = sel && a.read(cx).open_subagent.as_deref() == Some(sid.as_str());
                let running = state == "running";
                let agent = a.clone();
                let row = self
                    .session_row(format!("sub-{sid}").into(), None, format!("↳ {title}"), detail, glyph, open, look)
                    .pl(look.z(22.))
                    .on_click({
                        let (agent, sid) = (agent.clone(), sid.clone());
                        cx.listener(move |this, _, window, cx| {
                            // Show its work in the agent's chat panel.
                            this.select(Some(Selected::Agent(id.clone())), window, cx);
                            agent.update(cx, |a, cx| a.show_subagent(Some(sid.clone()), cx));
                            Docks::update(cx, dock::Kind::Acp, |d| d.activate(dock::Panel::Session));
                        })
                    })
                    .when(running, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("sub-stop-{sid}")))
                                .flex_none()
                                .px(look.z(4.))
                                .rounded(look.z(3.))
                                .cursor_pointer()
                                .hover(|d| d.bg(look.c(ui.hover_bg)))
                                .text_color(look.c(ui.text_muted))
                                .child("✕")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    agent.update(cx, |a, cx| a.stop_subagent(&sid, cx));
                                })),
                        )
                    });
                agents.push(row.into_any_element());
            }
        }
        let panel = self.terms.read(cx);
        for t in &self.term_tasks {
            let id = t.record.id.clone();
            let sel = self.selected == Some(Selected::TermTask(id.clone()));
            let s = t.term_id.and_then(|tid| panel.session_by_id(tid));
            let busy = s.is_some_and(|s| s.is_busy());
            let title = t.record.title.clone();
            let badge = with_check(if s.is_some() { String::new() } else { "stopped".to_string() }, self.checks.get(&id));
            let row = self
                .agent_button(format!("tt-{id}").into(), t.accent, title, badge, busy, sel, look)
                .on_mouse_down(MouseButton::Right, {
                    let id = id.clone();
                    cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| this.open_agent_menu(Selected::TermTask(id.clone()), ev.position, window, cx))
                })
                .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                    this.select(Some(Selected::TermTask(id.clone())), window, cx);
                    if ev.click_count() == 2 {
                        this.rename_agent(&qur::RenameAgent, window, cx);
                    }
                }));
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
                        .group("shell-row")
                        .on_click(cx.listener(move |this, _, window, cx| this.select(Some(Selected::Shell(tid)), window, cx)))
                        // Close the terminal (and its splits), shown on hover.
                        .child(
                            div()
                                .id(SharedString::from(format!("sh-close-{tid}")))
                                .flex_none()
                                .px(look.z(4.))
                                .rounded(look.z(3.))
                                .invisible()
                                .group_hover("shell-row", |d| d.visible())
                                .hover(|d| d.bg(look.c(ui.hover_bg)))
                                .text_color(look.c(ui.text_muted))
                                .child("✕")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.terms.update(cx, |p, cx| p.close_row(tid, cx));
                                })),
                        )
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
        // Scrolls on its own once there are many agents and subagents, so
        // Source Control and the file tree stay on screen below it.
        let agents = div().id("sidebar-agents").pt(look.z(2.)).flex().flex_col().flex_shrink_0().max_h(gpui::relative(0.6)).overflow_y_scroll().children(agents);
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
            .when(self.config.ui.keep_awake, |d| d.child({
                // Keep Awake toggle, above the agents (View > Show Keep Awake in Sidebar).
                let on = self.caffeinate.is_some();
                div()
                    .id("caffeinate")
                    .mx(look.z(6.))
                    .mt(look.z(8.))
                    .px(look.z(8.))
                    .py(look.z(4.))
                    .rounded(look.z(4.))
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .cursor_pointer()
                    .hover(|d| d.bg(look.c(ui.hover_bg)))
                    .text_size(look.text(-1.))
                    .text_color(look.c(if on { ui.text } else { ui.text_muted }))
                    .child(
                        // A small switch: filled with the accent when on.
                        div()
                            .flex_none()
                            .w(look.z(22.))
                            .h(look.z(12.))
                            .rounded(look.z(6.))
                            .p(look.z(2.))
                            .flex()
                            .when(on, |d| d.justify_end())
                            .bg(look.c(if on { look.theme.accents[0].base } else { ui.border_strong }))
                            .child(div().size(look.z(8.)).rounded(look.z(4.)).bg(look.c(ui.editor_bg))),
                    )
                    .child(if on { "Keep awake · on" } else { "Keep awake" })
                    .on_click(cx.listener(|this, _, window, cx| this.caffeinate(&qur::Caffeinate, window, cx)))
            }))
            // Like TERMINALS: the section only appears once there's something in it
            // (Cmd+Shift+N or File > New Agent Task starts one).
            .when(!empty_agents, |d| {
                d.child(
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
                .child(agents)
            })
            .when(!shells.is_empty(), |d| d.child(self.section_header("TERMINALS".into(), look)).children(shells))
            .child(self.section_header("SOURCE CONTROL".into(), look))
            .child({
                let (branch, n) = self
                    .git
                    .as_ref()
                    .map(|g| {
                        let st = &g.read(cx).status;
                        (st.branch.clone().unwrap_or_else(|| "detached".into()), st.staged.len() + st.unstaged.len() + st.untracked.len())
                    })
                    .unwrap_or_else(|| ("Git".into(), 0));
                let detail = match (&self.merge, n) {
                    (Some(m), _) => format!("merging · {} conflicted", m.files.len()),
                    (None, 0) => "clean".to_string(),
                    (None, n) => format!("{n} changed"),
                };
                self.session_row("git".into(), None, branch, detail, if self.merge.is_some() { "!" } else { "" }, self.selected == Some(Selected::Git), look)
                    .on_click(cx.listener(|this, _, window, cx| this.show_git(&qur::GitPanel, window, cx)))
            })
            .child(
                div()
                    .id("files-toggle")
                    .cursor_pointer()
                    .child(
                        self.section_header(files_label, look)
                            .pt(look.z(14.))
                            .gap(look.z(4.))
                            .child(
                                svg()
                                    .path(if self.files_open { icon::CHEVRON_DOWN } else { icon::CHEVRON_RIGHT })
                                    .size(look.z(12.))
                                    .flex_none()
                                    .text_color(look.c(ui.text_muted)),
                            ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.files_open = !this.files_open;
                        cx.notify();
                    })),
            )
            .when(self.files_open, |d| d.children(tree.map(|t| div().flex_1().min_h(px(0.)).flex().flex_col().child(t))))
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

    fn stage(&self, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        match &self.selected {
            Some(Selected::Agent(id)) => {
                if let Some(a) = self.agent(id) {
                    return a.clone().into_any_element();
                }
            }
            Some(Selected::TermTask(id)) => {
                if let Some(t) = self.term_task(id) {
                    let term = div().size_full().bg(look.surface(ui.panel_bg)).pt(look.z(6.)).child(self.terms.clone());
                    // The agent's terminal is the whole stage (decision 27). Review
                    // Changes still swaps it for the diff until Keep Working.
                    // An arrangeable dock too (dock.rs); by default the
                    // terminal fills it (decision 27).
                    let mut term = Some(term);
                    let dock = dock::render(dock::Kind::Terminal, look, cx, &mut |p, _| match p {
                        dock::Panel::Session => term.take().map(|t| t.into_any_element()).unwrap_or_else(|| div().into_any_element()),
                        dock::Panel::Live => div().size_full().child(t.live.clone()).into_any_element(),
                        dock::Panel::Changes => div().size_full().child(t.changes.clone()).into_any_element(),
                        dock::Panel::Browser => div().size_full().child(t.browser.clone()).into_any_element(),
                        dock::Panel::Todo => todo_panel(&t.todos.todos, t.accent, look).into_any_element(),
                    });
                    // The stage region's one translucent layer.
                    return div().size_full().bg(look.bg(ui.panel_bg)).child(dock).into_any_element();
                }
            }
            Some(Selected::Shell(_)) => {
                return div().size_full().bg(look.bg(ui.panel_bg)).pt(look.z(6.)).child(self.terms.clone()).into_any_element();
            }
            Some(Selected::Git) => {
                if let Some(g) = &self.git {
                    return g.clone().into_any_element();
                }
            }
            Some(Selected::Search) => {
                if let Some(s) = &self.search {
                    return s.clone().into_any_element();
                }
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
        let vim = code.read(cx).vim_status();
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
                                .children(vim.map(|m| div().flex_none().ml(look.z(8.)).px(look.z(5.)).rounded(look.z(3.)).bg(look.c(ui.active_bg)).text_size(look.text(-2.)).font_family(look.code_font.clone()).text_color(look.c(ui.text_secondary)).child(m)))
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
        // Pickers opened from a prompt (agent commands, @ mentions, the mode
        // and model chips) sit just above that prompt, not at the top.
        let above_prompt = |target: &Entity<PromptInput>, child: gpui::AnyElement| -> Option<gpui::AnyElement> {
            let b = target.read(cx).bounds?;
            Some(
                gpui::anchored()
                    .position(gpui::point(b.left() - look.z(12.), b.top() - look.z(14.)))
                    .anchor(gpui::Anchor::BottomLeft)
                    .snap_to_window_with_margin(look.z(8.))
                    .child(div().occlude().child(child))
                    .into_any_element(),
            )
        };
        match self.overlay.as_ref()? {
            Overlay::Picker(p, PickKind::Slash { target, .. } | PickKind::Mention { target, .. }) => {
                let el = || p.clone().into_any_element();
                Some(above_prompt(target, el()).unwrap_or_else(|| centered(el())))
            }
            Overlay::Picker(p, PickKind::Setting { agent, .. }) => {
                let target = agent.read(cx).prompt.clone();
                let el = || p.clone().into_any_element();
                Some(above_prompt(&target, el()).unwrap_or_else(|| centered(el())))
            }
            Overlay::Picker(p, _) => Some(centered(p.clone().into_any_element())),
            Overlay::Setup(s) => Some(centered(s.clone().into_any_element())),
            Overlay::About => Some(centered(self.about_box(look, cx))),
            Overlay::Accent => Some(centered(self.accent_box(look, cx))),
            Overlay::AgentMenu { id, at } => Some(self.agent_menu(id, *at, look, cx)),
            Overlay::Layouts => Some(centered(self.layouts_box(look, cx))),
            Overlay::GoTo(g) | Overlay::Rename(g) => Some(centered(
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
                            .child(if a.mode == Mode::Terminal && !a.label.to_lowercase().contains("terminal") { format!("{} (terminal)", a.label) } else { a.label.clone() })
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
        let min = qur_theme::MIN_OPACITY;
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
            .left(self.title_lead(look))
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
# opacity = 1.0             # 0.6 to 1.0
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
        self.fullscreen = window.is_fullscreen();
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
                (t.record.id.clone(), t.shows_browser(cx), t.browser.clone())
            };
            let show = wants && !covered && self.selected == Some(Selected::Agent(id));
            browser.update(cx, |b, cx| b.set_shown(show, window, cx));
        }
        let term_wants = Docks::get(cx, dock::Kind::Terminal).is_visible(dock::Panel::Browser);
        for (id, browser) in self.term_tasks.iter().map(|t| (t.record.id.clone(), t.browser.clone())).collect::<Vec<_>>() {
            let show = term_wants && !covered && self.selected == Some(Selected::TermTask(id));
            browser.update(cx, |b, cx| b.set_shown(show, window, cx));
        }
        let body = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .when(self.sidebar, |d| d.child(self.sidebar(&look, cx)))
            .child(div().flex_1().min_w(px(0.)).h_full().child(self.stage(&look, cx)));
        let any_overlay = self.overlay.is_some();
        div()
            .key_context("Workspace")
            .track_focus(&self.focus)
            // A click anywhere in Qur (clicks in a page go to the page, not
            // here) takes the keyboard back from the browsers.
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| this.release_browser_focus(cx)))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .font_family(look.ui_font.clone())
            .text_color(look.c(ui.text))
            // Regions paint their own (translucent) backgrounds.
            .bg(look.surface(ui.window_bg))
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
            .on_action(cx.listener(Self::tile_terminal))
            .on_action(cx.listener(Self::caffeinate))
            .on_action(cx.listener(Self::focus_input))
            .on_action(cx.listener(Self::split_right))
            .on_action(cx.listener(Self::split_down))
            .on_action(cx.listener(Self::next_terminal))
            .on_action(cx.listener(Self::prev_terminal))
            .on_action(cx.listener(Self::next_session))
            .on_action(cx.listener(Self::switch_session))
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
            .on_action(cx.listener(Self::show_git))
            .on_action(cx.listener(Self::merge_action))
            .on_action(cx.listener(Self::run_checks_action))
            .on_action(cx.listener(Self::run_project))
            .on_action(cx.listener(Self::rename_agent))
            .on_action(cx.listener(Self::open_setup))
            .on_action(cx.listener(Self::toggle_vim))
            .on_action(cx.listener(Self::toggle_keep_awake))
            .on_action(cx.listener(Self::run_doctor))
            .on_action(cx.listener(Self::keyboard_shortcuts))
            .on_action(cx.listener(Self::show_logs))
            .on_action(cx.listener(Self::report_problem))
            .on_action(cx.listener(Self::about))
            .on_action(cx.listener(Self::accent_color))
            .on_action(cx.listener(Self::choose_layout))
            // Scripted runs may reach here before the picker is drawn.
            .on_action(cx.listener(|this, _: &commands::setup::Done, _, cx| {
                if let Some(Overlay::Setup(v)) = &this.overlay {
                    v.clone().update(cx, |v, cx| v.done(cx));
                }
            }))
            .on_action(cx.listener(Self::find_in_project))
            .on_action(cx.listener(Self::replace_in_project))
            .on_action(cx.listener(Self::search_panel))
            .on_action(cx.listener(Self::git_stage_all))
            .on_action(cx.listener(Self::git_unstage_all))
            .on_action(cx.listener(Self::git_commit))
            .on_action(cx.listener(Self::accept_ours))
            .on_action(cx.listener(Self::accept_theirs))
            .on_action(cx.listener(Self::accept_both))
            .on_action(cx.listener(Self::next_conflict))
            .on_action(cx.listener(Self::prev_conflict))
            .on_action(cx.listener(Self::finish_merge_action))
            .on_action(cx.listener(Self::abort_merge_action))
            .on_action(cx.listener(Self::review_changes))
            .on_action(cx.listener(Self::keep_working))
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
            // Sidebar resizing and the accent sliders follow the mouse anywhere in the window.
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                if let Some(which) = this.tuning {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        this.tune_to(which, ev.position.x, window, cx);
                    } else {
                        this.tuning = None;
                    }
                }
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
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.resizing = false;
                if this.tuning.take().is_some()
                    && let Some(tune) = this.preview_tune
                {
                    this.save_tune(tune, window, cx);
                }
            }))
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

/// A native dialog. Dev/test: QUR_DEBUG_PROMPT="0,1" answers dialogs in order
/// with those button indexes (printed as `PROMPT <message> -> <i>`), so
/// scripted runs can get past them.
fn ask(window: &mut Window, level: PromptLevel, message: &str, detail: Option<&str>, answers: &[&str], cx: &mut App) -> futures::channel::oneshot::Receiver<usize> {
    static QUEUE: std::sync::Mutex<Option<std::collections::VecDeque<usize>>> = std::sync::Mutex::new(None);
    if let Ok(list) = std::env::var("QUR_DEBUG_PROMPT") {
        let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let queue = queue.get_or_insert_with(|| list.split(',').filter_map(|v| v.trim().parse().ok()).collect());
        if let Some(i) = queue.pop_front() {
            println!("PROMPT {message} -> {i}");
            let (tx, rx) = futures::channel::oneshot::channel();
            tx.send(i).ok();
            return rx;
        }
    }
    window.prompt(level, message, detail, answers, cx)
}

/// An agent button's badge plus its latest check (decision 29).
fn with_check(badge: String, check: Option<&Check>) -> String {
    let mark = match check {
        Some(Check::Running) => "checking…",
        Some(Check::Failed) => "✕ checks",
        Some(Check::Passed) | None => return badge,
    };
    if badge.is_empty() { mark.to_string() } else { format!("{badge} · {mark}") }
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

/// Notification tag for an agent: `qur:<window>:<task id>` (main.rs parses it).
fn alert_tag(window: &Window, id: &str) -> String {
    format!("qur:{}:{id}", window.window_handle().window_id().as_u64())
}

/// A plain name for an accent swatch, from its hue.
fn hue_name(c: Rgba) -> &'static str {
    let [r, g, b, _] = c.to_f32();
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max - min < 0.08 {
        return "gray";
    }
    let d = max - min;
    let h = if max == r { 60. * (((g - b) / d) % 6.) } else if max == g { 60. * ((b - r) / d + 2.) } else { 60. * ((r - g) / d + 4.) };
    let h = if h < 0. { h + 360. } else { h };
    match h as u32 {
        0..=14 | 345..=360 => "red",
        15..=39 => "orange",
        40..=69 => "yellow",
        70..=164 => "green",
        165..=199 => "teal",
        200..=254 => "blue",
        255..=289 => "purple",
        _ => "pink",
    }
}

/// An Agent Terminal's To-do tab (same look as a structured agent's).
fn todo_panel(todos: &[qur_agents::todos::Todo], accent: Rgba, look: &Look) -> gpui::Div {
    use qur_agents::todos::TodoStatus;
    let ui = &look.theme.ui;
    if todos.is_empty() {
        return div().size_full().flex().items_center().justify_center().text_color(look.c(ui.text_muted)).child("No plan yet.");
    }
    let done = todos.iter().filter(|t| t.status == TodoStatus::Completed).count();
    div()
        .size_full()
        .px(look.z(16.))
        .py(look.z(8.))
        .flex()
        .flex_col()
        .gap(look.z(3.))
        .text_size(look.text(0.))
        .child(div().font_weight(gpui::FontWeight::MEDIUM).text_color(look.c(ui.text_muted)).child(format!("TO-DO  {done}/{}", todos.len())))
        .children(todos.iter().map(|t| {
            let (g, c) = match t.status {
                TodoStatus::Completed => ("●", ui.text_muted),
                TodoStatus::InProgress => ("◐", ui.text),
                TodoStatus::Pending => ("○", ui.text_secondary),
            };
            div()
                .flex()
                .gap(look.z(8.))
                .text_color(look.c(c))
                .when(t.status == TodoStatus::InProgress, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                .child(div().flex_none().text_color(look.c(accent)).child(g))
                .child(div().min_w(px(0.)).child(t.content.clone()))
        }))
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
