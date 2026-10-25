//! One window = one project (decisions 18, 19). Left column: AGENTS (ACP
//! tasks and Agent Terminals), TERMINALS (shells) and FILES (the tree of the
//! selected session's worktree, or the main checkout). The stage shows the
//! selected session. Code opens in a modal over the window; Esc closes it.
//! Also applies the project's look (theme, transparency; decisions 8, 13).

mod actions;
mod checks;
mod code;
mod debug;
mod merge;
mod overlays;
mod pickers;
mod sessions;
mod settings;
mod sidebar;
mod tasks;
mod top_bar;
mod turns;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, AnyWindowHandle, App, AppContext, Bounds, ClipboardItem, Context, Entity, FocusHandle, MouseButton, Pixels, PromptLevel,
    SharedString, Subscription, Task, Window, WindowAppearance, canvas, div, prelude::*, px, svg,
};
use qur_acp::PermissionKind;
use qur_agents::{AgentDef, Mode, Roster, TaskRecord, TaskStore};
use qur_config::{Config, ProjectSettings, Value};
use qur_editor::Point;
use qur_theme::{Appearance, Registry, Rgba, ThemeChoice};

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
use crate::review::{self, Subject};
use crate::search_view::{SearchEvent, SearchPanel};
use crate::setup_view::{self, SetupEvent, SetupView};
use crate::terminal::{Kind, PanelEvent, TerminalPanel};
use crate::tree_view::{TreeEvent, TreeView};
use top_bar::{set_shadow, set_titlebar_hidden, set_traffic_lights_hidden};

/// Height of our own top bar (the system titlebar is transparent underneath).
pub const TITLEBAR_H: f32 = 32.0;
/// Zen mode: the strip at the top that shows the bar (thin, so reaching for
/// a tab or the sidebar's top row doesn't pop it, yet not the edge pixel
/// alone), and how far below the bar the mouse may wander before it slides away.
const ZEN_EDGE: f32 = 5.0;
const ZEN_SLACK: f32 = 8.0;
const ZEN_SLIDE: Duration = Duration::from_millis(160);
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
    /// Go to Symbol in the code modal: each symbol's name (bytes), and the
    /// selections to put back on Esc (highlighting previews a symbol).
    Symbols { targets: Vec<std::ops::Range<usize>>, restore: (Vec<qur_editor::Selection>, usize) },
}

enum Overlay {
    Picker(Entity<Picker>, PickKind),
    GoTo(Entity<TextInput>),
    /// Rename Agent…: the selected agent's name.
    Rename(Entity<TextInput>),
    /// Qur > Settings… (Cmd+,), also the first-run welcome (M6).
    Setup(Entity<SetupView>),
    /// View > Appearance > Accent Color….
    Accent,
    /// Right-click on an agent's button: its commands, at the pointer.
    AgentMenu { id: String, at: gpui::Point<Pixels> },
    /// View > Choose Stage Layout….
    Layouts,
    /// About Qur (M7).
    About,
    Transparency { project_scope: bool },
    /// Qur > Appearance > Gaps…: window gaps, live while you drag.
    Gaps,
    NewTask { input: Entity<PromptInput>, agent: usize },
    /// Review Agent's Work… / Plan…: the reviewer (agent and model) for
    /// task `task`, and an optional instruction line.
    Review { input: Entity<TextInput>, task: String, subject: Subject, choices: Vec<review::Choice>, chosen: usize, touched: bool },
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
    /// `[ui] sidebar = "auto"`: the sidebar is slid in because the mouse
    /// reached its edge of the window.
    sidebar_shown: bool,
    /// `caffeinate` while Keep Awake is on (agents keep working with the lid
    /// open and the screen idle). It watches Qur's pid, so it ends with Qur.
    caffeinate: Option<std::process::Child>,
    /// FILES is the one collapsible section in the left column.
    files_open: bool,
    resizing: bool,
    overlay: Option<Overlay>,
    /// File picker shown over the New Agent Task dialog (`@` mentions).
    mention_over_dialog: Option<Entity<Picker>>,
    /// Which CLIs ("claude", "codex") are signed in, asked once per window
    /// when the reviewer picker first opens (it picks the default reviewer).
    review_logins: Option<HashMap<String, bool>>,
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
    /// The Gaps popover's sliders (gap, corner radius) and which one is dragged.
    gap_sliders: Rc<[Cell<Bounds<Pixels>>; 2]>,
    gap_sliding: Option<usize>,
    /// The window is the key window (the traffic lights are grey otherwise).
    window_active: bool,
    title_drag: bool,
    /// Full screen hides the traffic lights, so the title bar starts at the edge.
    fullscreen: bool,
    /// Zen mode: the top bar is slid down because the mouse reached the top edge.
    zen_shown: bool,
    /// When the bar last started sliding (in or out), for its animation.
    zen_since: Instant,
    /// Whether the system's traffic lights have been hidden (Qur draws its own in the bar).
    lights_hidden: bool,
    /// Whether the system title bar strip is hidden (full screen only).
    titlebar_hidden: bool,
    /// Whether the window's shadow is off (window gaps).
    shadow_off: bool,
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
                // The traffic lights turn grey or back to color.
                cx.notify();
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
        let sidebar = config.ui.sidebar != "hidden";
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
            sidebar,
            sidebar_w: SIDEBAR_DEFAULT_W,
            sidebar_shown: false,
            caffeinate: None,
            files_open: true,
            resizing: false,
            overlay: None,
            mention_over_dialog: None,
            review_logins: None,
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
            gap_sliders: Rc::new([Cell::new(Bounds::default()), Cell::new(Bounds::default())]),
            gap_sliding: None,
            window_active: true,
            title_drag: false,
            fullscreen: false,
            zen_shown: false,
            zen_since: Instant::now().checked_sub(ZEN_SLIDE).unwrap_or_else(Instant::now),
            titlebar_hidden: false,
            shadow_off: false,
            lights_hidden: false,
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
            ws.show_settings(true, window, cx);
        }
        if let Some(e) = err.or(roster_err) {
            ws.flash(e, cx);
        }
        ws
    }

    pub fn project_name(&self) -> String {
        self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }

    fn effective(&self) -> qur_config::Effective {
        qur_config::effective(&self.config, &self.project)
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
            sidebar_auto: self.config.ui.sidebar == "auto",
            sidebar_right: self.sidebar_right(),
            soft_wrap: self.code.as_ref().map(|e| e.read(cx).soft_wrap()).unwrap_or(self.config.editor.soft_wrap),
            recent: qur_config::State::load().recent_projects,
            merging: self.merge.is_some(),
            turns: self.selected_last_turn(cx).is_some(),
            pending: self.agents.iter().any(|a| a.read(cx).has_permission()),
            review: self.review_state(cx),
            vim: self.config.keys.vim,
            keep_awake: self.config.ui.keep_awake,
            zen: self.config.ui.zen,
            gaps: self.config.ui.gaps,
            keymap: commands::keymap_generation(),
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

    /// Merge > Accept Ours/Theirs/Both, Next/Previous Conflict: act on the
    /// conflicted file open in the code modal.
    /// The workspace's own focus node (scripted action dispatch, main.rs).
    pub fn root_focus(&self) -> FocusHandle {
        self.focus.clone()
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
}

thread_local! {
    /// Task id per agent entity (lets render paths find agents by id without
    /// borrowing the App).
    static AGENT_IDS: std::cell::RefCell<HashMap<gpui::EntityId, String>> = std::cell::RefCell::new(HashMap::new());
}

fn agent_id_matches(a: &Entity<AgentTask>, id: &str) -> bool {
    AGENT_IDS.with(|m| m.borrow().get(&a.entity_id()).is_some_and(|x| x == id))
}

fn is_dark(a: WindowAppearance) -> bool {
    matches!(a, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.fullscreen = window.is_fullscreen();
        let zen_p = self.zen_progress(cx);
        self.window_active = window.is_window_active();
        // Qur draws the traffic lights in its bar; the system's stay hidden.
        if !self.lights_hidden {
            self.lights_hidden = true;
            set_traffic_lights_hidden(window, true);
        }
        if self.fullscreen != self.titlebar_hidden {
            self.titlebar_hidden = self.fullscreen;
            set_titlebar_hidden(window, self.fullscreen);
            // AppKit lays its buttons out again going in or out of full screen.
            set_traffic_lights_hidden(window, true);
        }
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
        // Nor while a tab or pane is dragged: a web view drawn over the dock
        // hid the drop preview and took the drag's mouse moves.
        let covered = self.overlay.is_some() || self.code.is_some() || cx.has_active_drag();
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
        let tiled = look.tiles.is_some();
        // With gaps the whole window is (all but) clear, so its shadow fell
        // under the gaps too: a dark rim around the tiles. Tiles go shadowless.
        if tiled != self.shadow_off {
            self.shadow_off = tiled;
            set_shadow(window, !tiled);
        }
        let sidebar = self.sidebar_visible();
        let body = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            // `[ui] sidebar_side = "right"`: the same row, mirrored.
            .when(self.sidebar_right(), |d| d.flex_row_reverse())
            .when(sidebar, |d| d.child(self.sidebar(&look, cx)))
            .when(sidebar && tiled, |d| d.child(self.sidebar_gap(&look, cx)))
            .child(div().flex_1().min_w(px(0.)).h_full().child(self.stage(&look, cx)));
        let zen = self.config.ui.zen;
        // With window gaps the regions sit a gap apart and in from the edges.
        let frame = div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            // In zen the bar's space (with its gap below) grows as it slides in.
            .when(tiled, |d| d.p(look.gap()).when(!zen, |d| d.gap(look.gap())))
            .when(!zen || zen_p > 0., |d| d.child(self.top(&look, zen_p, cx)))
            .child(body);
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
            // The base for any text that doesn't set its own size (it follows Text Size and zoom).
            .text_size(look.text(0.))
            .text_color(look.c(ui.text))
            // Regions paint their own (translucent) backgrounds; with window
            // gaps the root is all but clear (Look::root_bg).
            .bg(look.root_bg(ui.window_bg))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::hide))
            .on_action(cx.listener(Self::hide_others))
            .on_action(cx.listener(Self::show_all))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::minimize))
            .on_action(cx.listener(Self::zoom_window))
            .on_action(cx.listener(Self::fullscreen))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::auto_hide_sidebar))
            .on_action(cx.listener(Self::sidebar_on_right))
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
            .on_action(cx.listener(Self::go_to_symbol))
            .on_action(cx.listener(Self::expand_selection))
            .on_action(cx.listener(Self::shrink_selection))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::transparency))
            .on_action(cx.listener(Self::gaps))
            .on_action(cx.listener(Self::toggle_gaps))
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
            .on_action(cx.listener(Self::allow_all_pending))
            .on_action(cx.listener(Self::find_in_chat))
            .on_action(cx.listener(Self::copy_chat))
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
            .on_action(cx.listener(Self::toggle_zen))
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
                    v.clone().update(cx, |v, cx| v.close(cx));
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
            .on_action(cx.listener(Self::show_last_turn))
            .on_action(cx.listener(Self::rewind_last_turn))
            .on_action(cx.listener(Self::review_work))
            .on_action(cx.listener(Self::review_plan))
            .on_action(cx.listener(Self::send_review_findings))
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
                if ev.keystroke.key == "escape" && matches!(this.overlay, Some(Overlay::Transparency { .. } | Overlay::Gaps)) {
                    this.save_popover_transparency(cx);
                    this.save_gaps(cx);
                    this.close_overlays(window, cx);
                }
            }))
            // Before the focused view sees it (a terminal would take Esc for itself).
            .capture_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" && matches!(this.overlay, Some(Overlay::AgentMenu { .. } | Overlay::About)) {
                    this.close_overlays(window, cx);
                    cx.stop_propagation();
                }
                // Settings close on Esc wherever focus is (a slider drag, a terminal).
                if ev.keystroke.key == "escape"
                    && let Some(Overlay::Setup(v)) = &this.overlay
                {
                    v.clone().update(cx, |v, cx| v.close(cx));
                    cx.stop_propagation();
                }
            }))
            // Sidebar resizing and the accent sliders follow the mouse anywhere in the window.
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                // Neither slides while a button is held: pressing a tab at the
                // top edge to drag it brought the bar down under the pointer.
                if ev.pressed_button.is_none() {
                    this.zen_track(ev.position.y, window, cx);
                    this.sidebar_track(ev.position.x, window, cx);
                }
                if let Some(which) = this.tuning {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        this.tune_to(which, ev.position.x, window, cx);
                    } else {
                        this.tuning = None;
                    }
                }
                if this.resizing {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        let look = Look::of(window, cx);
                        // How far the mouse is in from the sidebar's edge of the window.
                        let x = if this.sidebar_right() { window.viewport_size().width - ev.position.x } else { ev.position.x };
                        this.sidebar_w = (f32::from(x - look.gap()) / look.zoom).clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W);
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
            .child(frame)
            .children(crate::script::mark(|| "window".to_string()))
            .children(self.code_modal(&look, cx))
            .when(any_overlay, |d| {
                d.child(div().absolute().size_full().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        // Settings close their own way (the first run keeps the agents ticked for you).
                        if let Some(Overlay::Setup(v)) = &this.overlay {
                            v.clone().update(cx, |v, cx| v.close(cx));
                            return;
                        }
                        this.save_popover_transparency(cx);
                        this.save_gaps(cx);
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
