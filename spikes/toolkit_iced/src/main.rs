//! M0 toolkit spike: iced candidate.
//!
//! Top bar, sidebar (agents + virtualized file tree), a custom virtualized code
//! view (`editor.rs`), bottom terminal panel (`terminal.rs`), native menu
//! (`menu.rs`), command palette and find bar, all driven by one command table
//! (`commands.rs`). Shared work (highlighting, tree model, metrics) lives in
//! `spike_common` so the GPUI spike does exactly the same thing.

mod commands;
mod editor;
mod menu;
mod terminal;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::advanced::text::{self as core_text, Paragraph as _};
use iced::futures::channel::mpsc::{self, UnboundedSender};
use iced::keyboard::{self, key::Named, Key};
use iced::widget::{
    checkbox, column, container, mouse_area, operation, row, scrollable, slider, stack, text, text_input, Id, Space,
};
use iced::{border, window, Color, Element, Font, Length, Subscription, Task};
use spike_common::{
    metric, qur_dark, sample_agents, since_start, Config, FileTree, Highlighter, Samples, AUTOSCROLL_LINES_PER_FRAME,
    AUTOTYPE_KEYS, AUTOTYPE_LINE,
};

use commands::{Ctx, KeyPress, Tier, COMMANDS};
use editor::{CodeView, Doc, LINE_H};
use terminal::{Kind, TermView, Terminal};

const SIDEBAR_W: f32 = 220.0;
const SIDE_PAD: f32 = 8.0;
const ROW_H: f32 = 22.0;
/// Extra tree rows built above/below the viewport.
const ROW_OVERSCAN: usize = 8;
/// Custom top bar drawn under the transparent macOS titlebar.
const TOPBAR_H: f32 = 38.0;
/// Left inset that clears the traffic lights.
const TRAFFIC_LIGHTS_W: f32 = 78.0;
const PANEL_H: f32 = 200.0;
const TABSTRIP_H: f32 = 24.0;
const UI_SIZE: f32 = 12.0;
/// An agent shows "working" this long after its PTY last printed.
const ACTIVE_FOR: Duration = Duration::from_secs(2);

pub fn rgb(c: u32) -> Color {
    Color::from_rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Large background fills use the window opacity; text and lines never do.
pub fn rgba(c: u32, a: f32) -> Color {
    Color { a, ..rgb(c) }
}

fn env_flag(k: &str) -> bool {
    std::env::var(k).as_deref() == Ok("1")
}

fn main() -> iced::Result {
    spike_common::mark_start();
    iced::application(App::new, App::update, App::view)
        .title("Qur spike (iced)")
        .subscription(App::subscription)
        .theme(|_: &App| iced::Theme::Dark)
        .style(|_, _| iced::theme::Style { background_color: Color::TRANSPARENT, text_color: rgb(qur_dark::TEXT) })
        .scale_factor(|s: &App| s.zoom)
        .exit_on_close_request(false)
        .window(window::Settings {
            size: iced::Size::new(1400.0, 900.0),
            // Always transparent so the opacity slider works live.
            transparent: true,
            // winit only applies blur at creation (private CGS call inside winit).
            blur: env_flag("QUR_SPIKE_BLUR"),
            platform_specific: {
                // Spike-only A/B switch: QUR_SPIKE_NATIVE_TITLEBAR=1 keeps the stock titlebar.
                let custom = !env_flag("QUR_SPIKE_NATIVE_TITLEBAR");
                window::settings::PlatformSpecific {
                    title_hidden: custom,
                    titlebar_transparent: custom,
                    fullsize_content_view: custom,
                }
            },
            ..Default::default()
        })
        .run()
}

#[derive(Debug, Clone)]
pub enum Message {
    /// `window::frames()`: fires during each redraw; handled after present.
    Frame(Instant),
    TreeScrolled { offset: f32, height: f32 },
    Toggle(usize),
    ToggleFiles,
    DragWindow,
    ToggleMaximize,
    EditorScroll(f32),
    EditorMouse { row: usize, x: usize, drag: bool },
    /// Key from a focused editor/terminal widget.
    Key(KeyPress),
    /// Any key press (for overlays and unfocused shortcuts).
    GlobalKey { kp: KeyPress, captured: bool },
    Modifiers(keyboard::Modifiers),
    Highlighted { edit_gen: u64, lines: Vec<Vec<(std::ops::Range<usize>, u32)>> },
    // Terminals
    TermWakeup(usize),
    TermResize { id: usize, cols: usize, rows: usize },
    TermScroll { id: usize, lines: i32 },
    TermMouse { id: usize, row: usize, col: usize, right: bool, drag: bool },
    SelectTerm(usize),
    CloseTerm(usize),
    FocusTerminal,
    // Agent tasks
    TaskCreated(Result<TaskInfo, String>),
    SelectTask(usize),
    Tick,
    // Commands (menu, palette, keys)
    Command(String),
    // Find / go to line / palette
    FindChanged(String),
    ReplaceChanged(String),
    FindSubmit,
    ReplaceSubmit,
    GotoChanged(String),
    GotoSubmit,
    PaletteChanged(String),
    PaletteSubmit,
    PaletteRun(&'static str),
    Paste { text: Option<String>, target: Focus },
    // Transparency
    OpacityChanged(f32),
    CloseRequested,
    /// Spike-only: run step `n` of `QUR_SPIKE_COMMANDS`.
    ScriptStep(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Terminal,
    Find,
    Replace,
    Goto,
    Palette,
}

/// State the code view writes during layout/draw that the app reads back.
#[derive(Default)]
pub struct Shared {
    pub editor_h: Cell<f32>,
    pub editor_cols: Cell<usize>,
    pub char_w: Cell<f32>,
    pub last_draw: Cell<Option<Instant>>,
    pub term_metric_done: Cell<bool>,
}

#[derive(Debug, Clone)]
pub struct TaskInfo {
    n: usize,
    path: PathBuf,
    branch: String,
}

/// A Claude agent task: its own worktree + branch, shown in the sidebar.
struct AgentTask {
    info: TaskInfo,
    accent: u32,
    term: Option<usize>,
}

#[derive(Default)]
struct FindState {
    open: bool,
    replace: bool,
    query: String,
    replacement: String,
    count: usize,
}

struct Palette {
    query: String,
    selected: usize,
}

struct App {
    cfg: Config,
    tree: FileTree,
    rows: Vec<usize>,
    tree_off: f32,
    tree_h: f32,
    files_open: bool,
    sidebar: bool,
    doc: Doc,
    shared: Shared,
    hl: Arc<Mutex<Highlighter>>,
    hl_inflight: bool,
    focus: Focus,
    shift: bool,
    /// Clipboard text that came from a whole-line copy (pastes above the line).
    line_clip: Option<String>,
    find: FindState,
    goto: Option<String>,
    palette: Option<Palette>,
    // Terminals
    terms: Vec<Terminal>,
    active: Option<usize>,
    next_term: usize,
    panel: bool,
    panel_msg: Option<String>,
    term_tx: UnboundedSender<usize>,
    cell_w: f32,
    closing: Vec<std::thread::JoinHandle<()>>,
    // Agent tasks
    tasks: Vec<AgentTask>,
    next_task: usize,
    tick_pending: bool,
    // Window
    zoom: f32,
    opacity: f32,
    blur: bool,
    transparency_open: bool,
    menu: Option<menu::NativeMenu>,
    menu_tx: UnboundedSender<String>,
    trunc_cache: RefCell<HashMap<(String, u32), String>>,
    // Metrics
    first_frame_done: bool,
    scroll: Samples,
    typed: usize,
    pending_key: Option<Instant>,
    type_present: Samples,
    type_draw: Samples,
}

type UiParagraph = <iced::Renderer as core_text::Renderer>::Paragraph;

impl App {
    fn new() -> (Self, Task<Message>) {
        let cfg = Config::from_env();

        let mut tree = FileTree::load(&cfg.tree_root);
        if cfg.expand_all {
            tree.set_all_expanded(true);
        }
        let rows = tree.visible();
        metric("tree_files", tree.file_count());
        metric("tree_visible_rows", rows.len());

        let t = Instant::now();
        let src = std::fs::read_to_string(&cfg.file).expect("read spike file");
        let mut hl = Highlighter::new();
        let spans = hl.highlight(&src);
        let doc = Doc::new(&src, &spans);
        metric("open_10k_ms", t.elapsed().as_millis());

        let (term_tx, term_rx) = mpsc::unbounded();
        let (menu_tx, menu_rx) = mpsc::unbounded();
        let cell_w = editor::cell_width::<UiParagraph>();

        let mut app = App {
            tree,
            rows,
            tree_off: 0.0,
            tree_h: 900.0,
            files_open: true,
            sidebar: true,
            doc,
            shared: Shared::default(),
            hl: Arc::new(Mutex::new(hl)),
            hl_inflight: false,
            focus: Focus::Editor,
            shift: false,
            line_clip: None,
            find: FindState::default(),
            goto: None,
            palette: None,
            terms: Vec::new(),
            active: None,
            next_term: 0,
            panel: true,
            panel_msg: None,
            term_tx,
            cell_w,
            closing: Vec::new(),
            tasks: Vec::new(),
            next_task: 1,
            tick_pending: false,
            zoom: 1.0,
            opacity: std::env::var("QUR_SPIKE_OPACITY").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0f32).clamp(0.5, 1.0),
            blur: env_flag("QUR_SPIKE_BLUR"),
            transparency_open: false,
            menu: None,
            menu_tx,
            trunc_cache: RefCell::default(),
            first_frame_done: false,
            scroll: Samples::default(),
            typed: 0,
            pending_key: None,
            type_present: Samples::default(),
            type_draw: Samples::default(),
            cfg,
        };
        app.open_terminal(Kind::Shell, None);
        app.focus = Focus::Editor;
        let wakeups = Task::run(term_rx, Message::TermWakeup);
        let menu_events = Task::run(menu_rx, Message::Command);
        (app, Task::batch([wakeups, menu_events]))
    }

    fn measuring(&self) -> bool {
        let waiting_for_term = self.cfg.exit_after_first_frame
            && !self.shared.term_metric_done.get()
            && since_start() < Duration::from_secs(5);
        !self.first_frame_done || self.cfg.autoscroll || self.cfg.autotype || waiting_for_term
    }

    fn subscription(&self) -> Subscription<Message> {
        let keys = iced::event::listen_with(|event, status, _| match event {
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, physical_key, modifiers, text, .. }) => {
                Some(Message::GlobalKey {
                    kp: KeyPress {
                        latin: key.to_latin(physical_key),
                        key,
                        mods: modifiers,
                        text: text.map(|t| t.to_string()),
                    },
                    captured: status == iced::event::Status::Captured,
                })
            }
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => Some(Message::Modifiers(m)),
            _ => None,
        });
        let close = window::close_requests().map(|_| Message::CloseRequested);
        // Every message triggers a redraw in iced, so frames() is only
        // subscribed while measuring; otherwise the app would spin.
        let frames = if self.measuring() { window::frames().map(Message::Frame) } else { Subscription::none() };
        Subscription::batch([keys, close, frames])
    }

    fn view_h(&self) -> f32 {
        self.shared.editor_h.get()
    }

    fn cols(&self) -> usize {
        self.shared.editor_cols.get().max(8)
    }

    fn update(&mut self, msg: Message) -> Task<Message> {
        match msg {
            Message::Frame(_) => return self.on_frame(),
            Message::TreeScrolled { offset, height } => {
                self.tree_off = offset;
                self.tree_h = height;
            }
            Message::Toggle(id) => {
                self.tree.toggle(id);
                self.rows = self.tree.visible();
            }
            Message::ToggleFiles => self.files_open = !self.files_open,
            Message::DragWindow => return window::latest().and_then(window::drag),
            Message::ToggleMaximize => return window::latest().and_then(window::toggle_maximize),
            Message::EditorScroll(y) => self.doc.scroll_y = y,
            Message::EditorMouse { row, x, drag } => {
                let extend = drag || self.shift;
                self.doc.click(row, x, self.cols(), extend);
                let was = self.focus;
                self.focus = Focus::Editor;
                self.transparency_open = false;
                if was != Focus::Editor {
                    return unfocus_inputs();
                }
            }
            Message::Key(kp) => return self.route_key(kp),
            Message::GlobalKey { kp, captured } => return self.on_global_key(kp, captured),
            Message::Modifiers(m) => self.shift = m.shift(),
            Message::Highlighted { edit_gen, lines } => {
                self.hl_inflight = false;
                self.doc.set_spans(lines);
                if edit_gen != self.doc.edit_gen {
                    return self.rehighlight();
                }
            }
            Message::TermWakeup(id) => {
                let task = self.terms.iter().position(|t| t.id == id).map(|i| {
                    self.terms[i].on_wakeup();
                    self.terms[i].kind == Kind::Claude
                });
                if task == Some(true) && !self.tick_pending {
                    // Re-render agent glyphs once activity goes stale.
                    self.tick_pending = true;
                    return Task::perform(async { std::thread::sleep(ACTIVE_FOR + Duration::from_millis(50)) }, |_| Message::Tick);
                }
            }
            Message::Tick => {
                self.tick_pending = false;
                let busy = self.terms.iter().any(|t| t.kind == Kind::Claude && t.last_output.elapsed() < ACTIVE_FOR);
                if busy {
                    self.tick_pending = true;
                    return Task::perform(async { std::thread::sleep(Duration::from_millis(500)) }, |_| Message::Tick);
                }
            }
            Message::TermResize { id, cols, rows } => {
                let cw = self.cell_w;
                if let Some(t) = self.term_mut(id) {
                    t.resize(cols, rows, cw);
                }
            }
            Message::TermScroll { id, lines } => {
                if let Some(t) = self.term_mut(id) {
                    t.scroll(lines);
                }
            }
            Message::TermMouse { id, row, col, right, drag } => {
                if let Some(t) = self.term_mut(id) {
                    if drag { t.select_update(row, col, right) } else { t.select_start(row, col, right) }
                }
                if !drag {
                    return self.focus_terminal();
                }
            }
            Message::SelectTerm(id) => {
                if let Some(i) = self.terms.iter().position(|t| t.id == id) {
                    self.active = Some(i);
                    return self.focus_terminal();
                }
            }
            Message::CloseTerm(id) => self.close_terminal(id),
            Message::FocusTerminal => return self.focus_terminal(),
            Message::TaskCreated(res) => match res {
                Ok(info) => {
                    self.next_task = self.next_task.max(info.n + 1);
                    let accent = qur_dark::ACCENTS[(info.n - 1) % qur_dark::ACCENTS.len()];
                    let label = format!("claude {}", info.n);
                    let path = info.path.clone();
                    self.tasks.push(AgentTask { info, accent, term: None });
                    let term = self.open_terminal(Kind::Claude, Some((label, &path)));
                    if let Some(t) = self.tasks.last_mut() {
                        t.term = term;
                    }
                    return self.focus_terminal();
                }
                Err(e) => {
                    self.panel = true;
                    self.panel_msg = Some(e);
                }
            },
            Message::SelectTask(n) => {
                let term = self.tasks.iter().find(|t| t.info.n == n).and_then(|t| t.term);
                if let Some(i) = term.and_then(|id| self.terms.iter().position(|t| t.id == id)) {
                    self.active = Some(i);
                    self.panel = true;
                    return self.focus_terminal();
                }
            }
            Message::Command(id) => return self.run_command(&id),
            Message::FindChanged(q) => {
                self.find.count = self.doc.count_matches(&q);
                self.find.query = q;
                // Incremental: jump to the first match from the selection start.
                let (a, _) = self.doc.sel();
                self.doc.anchor = a;
                self.doc.head = a;
                let (h, c) = (self.view_h(), self.cols());
                self.doc.find(&self.find.query, true, h, c);
            }
            Message::ReplaceChanged(r) => self.find.replacement = r,
            Message::FindSubmit => return self.run_command(if self.shift { "find_prev" } else { "find_next" }),
            Message::ReplaceSubmit => {
                let (h, c) = (self.view_h(), self.cols());
                self.doc.replace_one(&self.find.query, &self.find.replacement, h, c);
                self.find.count = self.doc.count_matches(&self.find.query);
                return self.rehighlight();
            }
            Message::GotoChanged(s) => self.goto = Some(s.chars().filter(char::is_ascii_digit).collect()),
            Message::GotoSubmit => {
                if let Some(n) = self.goto.take().and_then(|s| s.parse().ok()) {
                    let (h, c) = (self.view_h(), self.cols());
                    self.doc.goto_line(n, h, c);
                }
                return self.focus_editor();
            }
            Message::PaletteChanged(q) => {
                if let Some(p) = &mut self.palette {
                    p.query = q;
                    p.selected = 0;
                }
            }
            Message::PaletteSubmit => {
                let id = self.palette.as_ref().and_then(|p| palette_items(&p.query).get(p.selected).map(|c| c.id));
                if let Some(id) = id {
                    return self.update(Message::PaletteRun(id));
                }
            }
            Message::PaletteRun(id) => {
                if commands::get(id).tier != Tier::S {
                    return Task::none();
                }
                self.palette = None;
                let back = self.focus_editor_or_terminal();
                return back.chain(self.run_command(id));
            }
            Message::Paste { text, target } => {
                let Some(s) = text else { return Task::none() };
                match target {
                    Focus::Editor => {
                        let line = self.line_clip.as_deref() == Some(s.as_str());
                        self.doc.paste(&s, line);
                        let (h, c) = (self.view_h(), self.cols());
                        self.doc.reveal_caret(h, c);
                        return self.rehighlight();
                    }
                    Focus::Terminal => {
                        if let Some(t) = self.active_term() {
                            t.input(bracketed(&s, t));
                        }
                    }
                    Focus::Find => return self.update(Message::FindChanged(format!("{}{s}", self.find.query))),
                    Focus::Replace => self.find.replacement.push_str(&s),
                    Focus::Goto => return self.update(Message::GotoChanged(format!("{}{s}", self.goto.clone().unwrap_or_default()))),
                    Focus::Palette => {
                        let q = format!("{}{s}", self.palette.as_ref().map_or("", |p| &p.query));
                        return self.update(Message::PaletteChanged(q));
                    }
                }
            }
            Message::OpacityChanged(v) => self.opacity = (v / 100.0).clamp(0.5, 1.0),
            Message::CloseRequested => return self.quit(),
            Message::ScriptStep(i) => return self.script_step(i),
        }
        Task::none()
    }

    // ----- focus -------------------------------------------------------------------

    fn focus_editor(&mut self) -> Task<Message> {
        self.focus = Focus::Editor;
        unfocus_inputs()
    }

    fn focus_terminal(&mut self) -> Task<Message> {
        if self.active.is_none() {
            return Task::none();
        }
        self.panel = true;
        self.focus = Focus::Terminal;
        self.transparency_open = false;
        unfocus_inputs()
    }

    fn focus_editor_or_terminal(&mut self) -> Task<Message> {
        if self.focus == Focus::Terminal { self.focus_terminal() } else { self.focus_editor() }
    }

    fn focus_input(&mut self, f: Focus, id: &'static str) -> Task<Message> {
        self.focus = f;
        operation::focus(Id::new(id)).chain(operation::select_all(Id::new(id)))
    }

    // ----- keys --------------------------------------------------------------------

    /// Keys from the focused editor or terminal widget.
    fn route_key(&mut self, kp: KeyPress) -> Task<Message> {
        let ctxs: &[Ctx] = match self.focus {
            Focus::Terminal => &[Ctx::Terminal, Ctx::Global],
            _ => &[Ctx::Editor, Ctx::Global],
        };
        if let Some(cmd) = commands::lookup(&kp, ctxs) {
            // Terminals keep their own meaning for keys that only edit text.
            let to_terminal = self.focus == Focus::Terminal && cmd.ctx == Ctx::Editor;
            if !to_terminal {
                return self.run_command(cmd.id);
            }
        }
        match self.focus {
            Focus::Terminal => {
                if let Some(t) = self.active_term()
                    && let Some(bytes) = terminal::key_bytes(&kp, t.app_cursor())
                {
                    t.input(bytes);
                }
                Task::none()
            }
            _ => {
                let m = kp.mods;
                if let Some(t) = kp.text.filter(|t| !t.chars().any(char::is_control)) {
                    if !m.logo() && !m.control() {
                        self.doc.insert(&t);
                        let (h, c) = (self.view_h(), self.cols());
                        self.doc.reveal_caret(h, c);
                        return self.rehighlight();
                    }
                }
                Task::none()
            }
        }
    }

    /// All key presses, including ones a widget (text input) captured.
    fn on_global_key(&mut self, kp: KeyPress, captured: bool) -> Task<Message> {
        let named = |n: Named| kp.key == Key::Named(n);
        match self.focus {
            Focus::Palette => {
                let n = self.palette.as_ref().map_or(0, |p| palette_items(&p.query).len());
                if let Some(p) = &mut self.palette {
                    if named(Named::ArrowDown) {
                        p.selected = (p.selected + 1).min(n.saturating_sub(1));
                    } else if named(Named::ArrowUp) {
                        p.selected = p.selected.saturating_sub(1);
                    }
                }
                if named(Named::Escape) {
                    self.palette = None;
                    return self.focus_editor();
                }
                Task::none()
            }
            Focus::Find | Focus::Replace | Focus::Goto if named(Named::Escape) => self.run_command("escape"),
            Focus::Find | Focus::Replace | Focus::Goto => Task::none(),
            // Keys no widget took (e.g. nothing focused yet): window shortcuts.
            _ if !captured => match commands::lookup(&kp, &[Ctx::Global]) {
                Some(cmd) => self.run_command(cmd.id),
                None => Task::none(),
            },
            _ => Task::none(),
        }
    }

    // ----- commands ------------------------------------------------------------------

    fn run_command(&mut self, id: &str) -> Task<Message> {
        let (h, c) = (self.view_h(), self.cols());
        let input_focus = matches!(self.focus, Focus::Find | Focus::Replace | Focus::Goto | Focus::Palette);
        let task = match id {
            "quit" | "close_window" => return self.quit(),
            "minimize" => window::latest().and_then(|w| window::minimize(w, true)),
            "zoom_window" => window::latest().and_then(window::toggle_maximize),
            "fullscreen" => window::latest().and_then(|w| {
                window::mode(w).then(move |m| {
                    window::set_mode(w, if m == window::Mode::Fullscreen { window::Mode::Windowed } else { window::Mode::Fullscreen })
                })
            }),
            // AppKit-only (standard selectors in the menu); iced has no API.
            "hide" | "hide_others" | "show_all" => Task::none(),
            "palette" => {
                self.palette = Some(Palette { query: String::new(), selected: 0 });
                self.focus_input(Focus::Palette, "palette")
            }
            "transparency" => {
                self.transparency_open = !self.transparency_open;
                Task::none()
            }
            "toggle_sidebar" => {
                self.sidebar = !self.sidebar;
                Task::none()
            }
            "reveal_files" => {
                self.sidebar = true;
                self.files_open = true;
                Task::none()
            }
            "toggle_panel" => {
                self.panel = !self.panel;
                if !self.panel && self.focus == Focus::Terminal {
                    return self.focus_editor();
                }
                Task::none()
            }
            "toggle_terminal" => {
                if self.focus == Focus::Terminal {
                    self.panel = false;
                    self.focus_editor()
                } else {
                    if self.terms.is_empty() {
                        self.open_terminal(Kind::Shell, None);
                    }
                    self.focus_terminal()
                }
            }
            "zoom_in" => {
                self.zoom = (self.zoom + 0.1).min(2.0);
                Task::none()
            }
            "zoom_out" => {
                self.zoom = (self.zoom - 0.1).max(0.6);
                Task::none()
            }
            "zoom_reset" => {
                self.zoom = 1.0;
                Task::none()
            }
            "save" => {
                // Never touch the fixtures unless explicitly allowed.
                if env_flag("QUR_SPIKE_ALLOW_SAVE") {
                    if let Err(e) = std::fs::write(&self.cfg.file, self.doc.text()) {
                        eprintln!("save failed: {e}");
                        return Task::none();
                    }
                }
                self.doc.dirty = false;
                Task::none()
            }
            "goto_line" => {
                self.goto = Some(String::new());
                self.focus_input(Focus::Goto, "goto")
            }
            // Clipboard: depends on what has focus.
            "copy" | "cut" if self.focus == Focus::Terminal => {
                match self.active_term().and_then(|t| t.selection_text()) {
                    Some(s) => iced::clipboard::write(s),
                    None => Task::none(),
                }
            }
            "paste" if self.focus == Focus::Terminal => paste(Focus::Terminal),
            "copy" | "cut" | "paste" | "select_all" if input_focus => self.input_clipboard(id),
            "copy" => {
                let (s, line) = self.doc.copy_text();
                self.line_clip = line.then(|| s.clone());
                iced::clipboard::write(s)
            }
            "cut" => {
                let (s, line) = self.doc.cut();
                self.line_clip = line.then(|| s.clone());
                iced::clipboard::write(s).chain(self.rehighlight())
            }
            "paste" => paste(Focus::Editor),
            "select_all" if self.focus == Focus::Terminal => Task::none(),
            // Find
            "find" | "replace" => {
                self.find.open = true;
                self.find.replace = id == "replace";
                if self.doc.has_selection() && self.doc.sel().0.line == self.doc.sel().1.line {
                    self.find.query = self.doc.selected_text();
                    self.find.count = self.doc.count_matches(&self.find.query);
                }
                if id == "replace" && !self.find.query.is_empty() {
                    self.focus_input(Focus::Replace, "replace")
                } else {
                    self.focus_input(Focus::Find, "find")
                }
            }
            "find_next" | "find_prev" => {
                self.find.open = true;
                self.doc.find(&self.find.query, id == "find_next", h, c);
                Task::none()
            }
            "find_use_selection" => {
                if self.doc.has_selection() {
                    self.find.query = self.doc.selected_text();
                    self.find.count = self.doc.count_matches(&self.find.query);
                }
                Task::none()
            }
            "replace_all" => {
                self.doc.replace_all(&self.find.query, &self.find.replacement);
                self.find.count = self.doc.count_matches(&self.find.query);
                self.rehighlight()
            }
            "escape" => {
                if self.transparency_open {
                    self.transparency_open = false;
                } else if self.palette.take().is_some() || self.goto.take().is_some() {
                    return self.focus_editor_or_terminal();
                } else if self.find.open {
                    self.find.open = false;
                    return self.focus_editor();
                } else {
                    self.doc.run("escape", h, c);
                }
                Task::none()
            }
            "soft_wrap" => {
                self.doc.run(id, h, c);
                if let Some(m) = &self.menu {
                    m.set_checked("soft_wrap", self.doc.soft_wrap);
                }
                Task::none()
            }
            // Terminals
            "new_terminal" => {
                self.open_terminal(Kind::Shell, None);
                self.focus_terminal()
            }
            "new_claude_terminal" => self.new_claude_task(),
            "close_terminal" => {
                if self.focus == Focus::Terminal
                    && let Some(id) = self.active_term().map(|t| t.id)
                {
                    self.close_terminal(id);
                }
                Task::none()
            }
            "clear_terminal" => {
                if let Some(t) = self.active_term() {
                    t.clear();
                }
                Task::none()
            }
            "next_terminal" | "prev_terminal" => {
                if let Some(i) = self.active {
                    let n = self.terms.len();
                    self.active = Some(if id == "next_terminal" { (i + 1) % n } else { (i + n - 1) % n });
                    return self.focus_terminal();
                }
                Task::none()
            }
            // Editor commands typed while a terminal has focus: send what a
            // terminal would get for the same keys.
            _ if self.focus == Focus::Terminal && commands::get(id).ctx == Ctx::Editor => {
                let bytes: &[u8] = match id {
                    "move_line_up" => b"\x1b\x1b[A",
                    "move_line_down" => b"\x1b\x1b[B",
                    "copy_line_up" => b"\x1b[1;4A",
                    "copy_line_down" => b"\x1b[1;4B",
                    "soft_wrap" => b"\x1bz",
                    "goto_line" => b"\x07",
                    _ => b"",
                };
                if !bytes.is_empty()
                    && let Some(t) = self.active_term()
                {
                    t.input(bytes.to_vec());
                }
                Task::none()
            }
            _ if input_focus => Task::none(),
            _ => {
                let before = self.doc.edit_gen;
                if self.doc.run(id, h, c) && self.doc.edit_gen != before {
                    return self.rehighlight();
                }
                Task::none()
            }
        };
        self.sync_menu_checks();
        task
    }

    fn sync_menu_checks(&self) {
        if let Some(m) = &self.menu {
            m.set_checked("toggle_sidebar", self.sidebar);
            m.set_checked("toggle_panel", self.panel);
            m.set_checked("soft_wrap", self.doc.soft_wrap);
        }
    }

    /// Clipboard commands while a text field has focus (the menu takes the
    /// keys before the text input would see them).
    fn input_clipboard(&mut self, id: &str) -> Task<Message> {
        let (input_id, value) = match self.focus {
            Focus::Find => ("find", self.find.query.clone()),
            Focus::Replace => ("replace", self.find.replacement.clone()),
            Focus::Goto => ("goto", self.goto.clone().unwrap_or_default()),
            _ => ("palette", self.palette.as_ref().map(|p| p.query.clone()).unwrap_or_default()),
        };
        match id {
            "select_all" => operation::select_all(Id::new(input_id)),
            "paste" => paste(self.focus),
            "copy" => iced::clipboard::write(value),
            _ => {
                let clear = match self.focus {
                    Focus::Find => self.update(Message::FindChanged(String::new())),
                    Focus::Replace => self.update(Message::ReplaceChanged(String::new())),
                    Focus::Goto => self.update(Message::GotoChanged(String::new())),
                    _ => self.update(Message::PaletteChanged(String::new())),
                };
                iced::clipboard::write(value).chain(clear)
            }
        }
    }

    /// Full re-highlight on the executor's thread pool, at most one in flight.
    /// The UI thread only patches span offsets for the edited line.
    fn rehighlight(&mut self) -> Task<Message> {
        if self.hl_inflight {
            return Task::none();
        }
        self.hl_inflight = true;
        let edit_gen = self.doc.edit_gen;
        let src = self.doc.text();
        let hl = self.hl.clone();
        Task::perform(
            async move {
                let spans = hl.lock().unwrap().highlight(&src);
                editor::split_spans(&src, &spans)
            },
            move |lines| Message::Highlighted { edit_gen, lines },
        )
    }

    // ----- terminals -------------------------------------------------------------------

    fn term_mut(&mut self, id: usize) -> Option<&mut Terminal> {
        self.terms.iter_mut().find(|t| t.id == id)
    }

    fn active_term(&mut self) -> Option<&mut Terminal> {
        self.active.and_then(|i| self.terms.get_mut(i))
    }

    /// Opens a terminal tab (`task` = label + worktree for agent tasks).
    fn open_terminal(&mut self, kind: Kind, task: Option<(String, &Path)>) -> Option<usize> {
        let id = self.next_term;
        self.next_term += 1;
        let (label, cwd) = match task {
            Some((label, cwd)) => (label, cwd.to_path_buf()),
            None => {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "zsh".into());
                let name = shell.rsplit('/').next().unwrap_or("zsh").to_string();
                let same = self.terms.iter().filter(|t| t.label.split(' ').next() == Some(&name)).count();
                (if same == 0 { name } else { format!("{name} {}", same + 1) }, self.cfg.tree_root.clone())
            }
        };
        match Terminal::spawn(id, kind, label, &cwd, self.term_tx.clone()) {
            Ok(t) => {
                self.terms.push(t);
                self.active = Some(self.terms.len() - 1);
                self.panel = true;
                self.panel_msg = None;
                Some(id)
            }
            Err(e) => {
                self.panel_msg = Some(format!("could not start terminal: {e}"));
                None
            }
        }
    }

    fn close_terminal(&mut self, id: usize) {
        let Some(i) = self.terms.iter().position(|t| t.id == id) else { return };
        let t = self.terms.remove(i);
        self.closing.push(t.close());
        for task in &mut self.tasks {
            if task.term == Some(id) {
                task.term = None;
            }
        }
        if self.terms.is_empty() {
            self.active = None;
            self.panel = false;
            if self.focus == Focus::Terminal {
                self.focus = Focus::Editor;
            }
        } else {
            self.active = Some(i.min(self.terms.len() - 1));
        }
        self.closing.retain(|h| !h.is_finished());
    }

    // ----- agent tasks -----------------------------------------------------------------

    fn new_claude_task(&mut self) -> Task<Message> {
        let n = self.next_task;
        self.next_task += 1;
        let root = self.cfg.tree_root.clone();
        Task::perform(async move { create_worktree(&root, n) }, Message::TaskCreated)
    }

    fn quit(&mut self) -> Task<Message> {
        for t in std::mem::take(&mut self.terms) {
            self.closing.push(t.close());
        }
        for h in self.closing.drain(..) {
            let _ = h.join();
        }
        if env_flag("QUR_SPIKE_CLEAN_WORKTREES") {
            for t in &self.tasks {
                let root = &self.cfg.tree_root;
                let _ = git(root, &["worktree", "remove", "--force", &t.info.path.to_string_lossy()]);
                let _ = git(root, &["branch", "-D", &t.info.branch]);
            }
        }
        iced::exit()
    }

    /// Spike-only verification hook: `QUR_SPIKE_COMMANDS="new_terminal,wait,close_terminal,quit"`
    /// runs commands through `run_command` (same path as menu and keys), 1 s apart.
    fn script_step(&mut self, i: usize) -> Task<Message> {
        let script = std::env::var("QUR_SPIKE_COMMANDS").unwrap_or_default();
        let Some(step) = script.split(',').nth(i).map(str::trim) else { return Task::none() };
        eprintln!("SCRIPT {step}");
        let task = match step {
            "wait" | "" => Task::none(),
            "focus_terminal" => self.focus_terminal(),
            "dump" => {
                for t in &self.terms {
                    eprintln!("SCRIPT term id={} label={} kind={:?}", t.id, t.label, t.kind);
                }
                for t in &self.tasks {
                    eprintln!("SCRIPT task n={} branch={} path={} term={:?}", t.info.n, t.info.branch, t.info.path.display(), t.term);
                }
                Task::none()
            }
            id => self.run_command(id),
        };
        let next = Task::perform(async { std::thread::sleep(Duration::from_secs(1)) }, move |_| Message::ScriptStep(i + 1));
        task.chain(next)
    }

    // ----- metrics -----------------------------------------------------------------------

    fn on_frame(&mut self) -> Task<Message> {
        if !self.first_frame_done {
            // Handled after the first frame's present() returned.
            self.first_frame_done = true;
            metric("first_frame_ms", since_start().as_millis());
            // winit installs its default menu at launch; ours goes in after.
            self.menu = Some(menu::NativeMenu::install(self.menu_tx.clone()));
            self.sync_menu_checks();
            if std::env::var("QUR_SPIKE_COMMANDS").is_ok() {
                return Task::done(Message::ScriptStep(0));
            }
            if self.cfg.autotype {
                let cols = self.cols();
                self.doc.goto_line(AUTOTYPE_LINE, self.view_h(), cols);
                let l = self.doc.head.line;
                self.doc.click(self.doc.visual(editor::Pos::new(l, self.doc.lines[l].len()), cols).0, usize::MAX, cols, false);
            }
        }
        if self.cfg.exit_after_first_frame {
            if self.shared.term_metric_done.get() || since_start() > Duration::from_secs(5) {
                return self.quit();
            }
            return Task::none();
        }

        if self.cfg.autoscroll {
            self.scroll.tick();
            let max = self.doc.max_scroll(self.view_h(), self.cols());
            if self.doc.scroll_y >= max {
                self.scroll.report("scroll");
                return self.quit();
            }
            self.doc.scroll_y = (self.doc.scroll_y + AUTOSCROLL_LINES_PER_FRAME as f32 * LINE_H).min(max);
            return Task::none();
        }

        if self.cfg.autotype {
            let now = Instant::now();
            if let Some(t0) = self.pending_key.take() {
                self.type_present.push(now - t0);
                if let Some(d) = self.shared.last_draw.get().filter(|d| *d > t0) {
                    self.type_draw.push(d - t0);
                }
            }
            if self.typed == AUTOTYPE_KEYS {
                self.type_present.report("type");
                self.type_draw.report("type_draw");
                return self.quit();
            }
            let ch = b"qur agent "[self.typed % 10] as char;
            self.typed += 1;
            self.pending_key = Some(Instant::now());
            // Same path as a real key from the editor widget.
            return self.update(Message::Key(KeyPress {
                key: Key::Character(ch.to_string().into()),
                latin: Some(ch),
                mods: keyboard::Modifiers::empty(),
                text: Some(ch.to_string()),
            }));
        }
        Task::none()
    }

    // ----- view ----------------------------------------------------------------------------

    /// Truncate `s` with "…" to fit `max_w` at the UI font size (cached).
    fn trunc(&self, s: &str, size: f32, max_w: f32) -> String {
        let key = (format!("{s}\u{0}{size}"), max_w as u32);
        if let Some(v) = self.trunc_cache.borrow().get(&key) {
            return v.clone();
        }
        let width = |t: &str| {
            UiParagraph::with_text(core_text::Text {
                content: t,
                bounds: iced::Size::INFINITE,
                size: iced::Pixels(size),
                line_height: core_text::LineHeight::default(),
                font: Font::DEFAULT,
                align_x: core_text::Alignment::Default,
                align_y: iced::alignment::Vertical::Top,
                shaping: core_text::Shaping::Basic,
                wrapping: core_text::Wrapping::None,
            })
            .min_bounds()
            .width
        };
        let out = if width(s) <= max_w {
            s.to_string()
        } else {
            let chars: Vec<char> = s.chars().collect();
            let (mut lo, mut hi) = (0, chars.len());
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                let cand: String = chars[..mid].iter().collect::<String>() + "…";
                if width(&cand) <= max_w { lo = mid } else { hi = mid - 1 }
            }
            chars[..lo].iter().collect::<String>() + "…"
        };
        self.trunc_cache.borrow_mut().insert(key, out.clone());
        out
    }

    fn view(&self) -> Element<'_, Message> {
        let main_row = if self.sidebar { row![self.sidebar(), self.editor_area()] } else { row![self.editor_area()] };
        let mut body = column![self.top_bar(), container(main_row).height(Length::Fill)];
        if self.panel {
            body = body.push(self.panel());
        }
        let mut layers = stack![body];
        if let Some(p) = &self.palette {
            layers = layers.push(self.palette_view(p));
        }
        if self.transparency_open {
            layers = layers.push(self.transparency_view());
        }
        layers.into()
    }

    fn bg(&self, c: u32) -> impl Fn(&iced::Theme) -> container::Style + 'static {
        let color = rgba(c, self.opacity);
        move |_| container::Style { background: Some(color.into()), ..Default::default() }
    }

    fn top_bar(&self) -> Element<'_, Message> {
        // Placeholder slot for the Qur symbol (SVG feature not enabled in the spike).
        let symbol = container(Space::new().width(14).height(14)).style(|_| container::Style {
            border: iced::Border { color: rgb(qur_dark::TEXT), width: 2.0, radius: 3.0.into() },
            ..Default::default()
        });
        let dirty = if self.doc.dirty { " •" } else { "" };
        let bar = container(row![symbol, text(format!("kliper{dirty}")).size(13)].spacing(8).align_y(iced::Alignment::Center))
            .width(Length::Fill)
            .height(TOPBAR_H)
            .align_y(iced::alignment::Vertical::Center)
            .padding(iced::Padding { left: TRAFFIC_LIGHTS_W, ..Default::default() })
            .style(self.bg(qur_dark::BG));
        mouse_area(column![bar, hairline()])
            .on_press(Message::DragWindow)
            .on_double_click(Message::ToggleMaximize)
            .into()
    }

    fn agent_row<'a>(&self, accent: u32, name: &str, task: &str, status: &str, on_press: Option<Message>) -> Element<'a, Message> {
        let muted = rgb(qur_dark::TEXT_MUTED);
        let dot = container(Space::new().width(10).height(10)).style(move |_| container::Style {
            background: Some(rgb(accent).into()),
            border: border::rounded(3),
            ..Default::default()
        });
        let inner = SIDEBAR_W - 2.0 * SIDE_PAD;
        let name = self.trunc(name, 13.0, inner - 10.0 - 8.0 - 8.0 - 18.0);
        let task = self.trunc(task, UI_SIZE, inner - 18.0);
        let content = column![
            row![dot, text(name).size(13).width(Length::Fill), text(status.to_string()).size(UI_SIZE).color(muted)]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            container(text(task).size(UI_SIZE).color(muted)).padding(iced::Padding { left: 18.0, ..Default::default() }),
        ]
        .spacing(2);
        match on_press {
            Some(m) => mouse_area(content).on_press(m).into(),
            None => content.into(),
        }
    }

    fn sidebar(&self) -> Element<'_, Message> {
        let muted = rgb(qur_dark::TEXT_MUTED);
        let mut agents = column![].spacing(6).padding(SIDE_PAD);
        for a in sample_agents() {
            agents = agents.push(self.agent_row(a.accent, a.name, a.task, a.status, None));
        }
        for t in &self.tasks {
            let term = t.term.and_then(|id| self.terms.iter().find(|x| x.id == id));
            let status = match term {
                Some(x) if x.last_output.elapsed() < ACTIVE_FOR => "~",
                Some(_) => "·",
                None => "×",
            };
            let path = t.info.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            agents = agents.push(self.agent_row(
                t.accent,
                &format!("claude terminal {}", t.info.n),
                &format!("{} · {path}", t.info.branch),
                status,
                Some(Message::SelectTask(t.info.n)),
            ));
        }

        let chevron = if self.files_open { "v" } else { ">" };
        let header = mouse_area(container(text(format!("{chevron} Files")).size(UI_SIZE).color(muted)).padding([4.0, SIDE_PAD]))
            .on_press(Message::ToggleFiles);

        let mut col = column![agents, header];
        if self.files_open {
            col = col.push(self.tree_view());
        }
        container(col)
            .width(SIDEBAR_W)
            .height(Length::Fill)
            .style(self.bg(qur_dark::SIDEBAR))
            .into()
    }

    fn tree_view(&self) -> Element<'_, Message> {
        // Windowed tree: spacer + only rows near the viewport + spacer.
        let n = self.rows.len();
        let first = ((self.tree_off / ROW_H) as usize).saturating_sub(ROW_OVERSCAN).min(n);
        let last = (first + (self.tree_h / ROW_H) as usize + 2 * ROW_OVERSCAN).min(n);
        let mut list = column![Space::new().height(first as f32 * ROW_H)];
        for &id in &self.rows[first..last] {
            let node = &self.tree.nodes[id];
            let glyph = if !node.is_dir { "  " } else if node.expanded { "v " } else { "> " };
            let indent = SIDE_PAD + node.depth as f32 * 12.0;
            let label = self.trunc(&format!("{glyph}{}", node.name), UI_SIZE, SIDEBAR_W - indent - SIDE_PAD - 10.0);
            let label = text(label)
                .size(UI_SIZE)
                .wrapping(core_text::Wrapping::None)
                .color(if node.is_dir { rgb(qur_dark::TEXT) } else { rgb(0xC8C8C8) });
            let cell = container(label)
                .height(ROW_H)
                .width(Length::Fill)
                .padding(iced::Padding { left: indent, top: 3.0, ..Default::default() });
            list = list.push(mouse_area(cell).on_press(Message::Toggle(id)));
        }
        list = list.push(Space::new().height((n - last) as f32 * ROW_H));
        scrollable(list)
            .height(Length::Fill)
            .on_scroll(|v| Message::TreeScrolled { offset: v.absolute_offset().y, height: v.bounds().height })
            .into()
    }

    fn editor_area(&self) -> Element<'_, Message> {
        let mut col = column![];
        if self.find.open {
            let count = if self.find.query.is_empty() { String::new() } else { format!("{} matches", self.find.count) };
            let find = text_input("Find", &self.find.query)
                .id(Id::new("find"))
                .on_input(Message::FindChanged)
                .on_submit(Message::FindSubmit)
                .size(UI_SIZE)
                .padding(4)
                .width(260);
            let mut bar = row![find, text(count).size(UI_SIZE).color(rgb(qur_dark::TEXT_MUTED))]
                .spacing(8)
                .align_y(iced::Alignment::Center);
            if self.find.replace {
                bar = bar.push(
                    text_input("Replace", &self.find.replacement)
                        .id(Id::new("replace"))
                        .on_input(Message::ReplaceChanged)
                        .on_submit(Message::ReplaceSubmit)
                        .size(UI_SIZE)
                        .padding(4)
                        .width(220),
                );
            }
            col = col.push(container(bar).padding([4.0, SIDE_PAD]).width(Length::Fill).style(self.bg(qur_dark::SIDEBAR)));
            col = col.push(hairline());
        }
        if let Some(g) = &self.goto {
            let input = text_input("Go to line…", g)
                .id(Id::new("goto"))
                .on_input(Message::GotoChanged)
                .on_submit(Message::GotoSubmit)
                .size(UI_SIZE)
                .padding(4)
                .width(160);
            let hint = text(format!("1–{}", self.doc.lines.len())).size(UI_SIZE).color(rgb(qur_dark::TEXT_MUTED));
            col = col.push(
                container(row![input, hint].spacing(8).align_y(iced::Alignment::Center))
                    .padding([4.0, SIDE_PAD])
                    .width(Length::Fill)
                    .style(self.bg(qur_dark::SIDEBAR)),
            );
            col = col.push(hairline());
        }
        let find = (self.find.open && !self.find.query.is_empty()).then_some(self.find.query.as_str());
        col.push(CodeView::new(&self.doc, &self.shared, self.focus == Focus::Editor, find, self.opacity)).into()
    }

    fn panel(&self) -> Element<'_, Message> {
        let muted = rgb(qur_dark::TEXT_MUTED);
        let mut tabs = row![].spacing(2).padding([0.0, SIDE_PAD]).align_y(iced::Alignment::Center);
        for (i, t) in self.terms.iter().enumerate() {
            let active = self.active == Some(i);
            let close = mouse_area(text("×").size(UI_SIZE).color(muted)).on_press(Message::CloseTerm(t.id));
            let tab = container(row![text(t.label.clone()).size(UI_SIZE), close].spacing(6))
                .padding([2.0, 8.0])
                .style(move |_| container::Style {
                    background: active.then(|| rgb(0x1A1A1A).into()),
                    border: border::rounded(3),
                    text_color: Some(if active { rgb(qur_dark::TEXT) } else { muted }),
                    ..Default::default()
                });
            tabs = tabs.push(mouse_area(tab).on_press(Message::SelectTerm(t.id)));
        }
        let strip = container(tabs).height(TABSTRIP_H).width(Length::Fill).align_y(iced::alignment::Vertical::Center).style(self.bg(qur_dark::BG));
        let body: Element<'_, Message> = match (self.active.and_then(|i| self.terms.get(i)), &self.panel_msg) {
            (_, Some(msg)) => container(text(msg.clone()).size(UI_SIZE).color(muted))
                .padding(SIDE_PAD)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(self.bg(qur_dark::BG))
                .into(),
            (Some(t), None) => TermView::new(t, self.focus == Focus::Terminal, self.cell_w, self.opacity, &self.shared).into(),
            (None, None) => container(text("No terminal. ⌘T opens one.").size(UI_SIZE).color(muted))
                .padding(SIDE_PAD)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(self.bg(qur_dark::BG))
                .into(),
        };
        column![hairline(), strip, hairline(), body].height(PANEL_H).into()
    }

    fn palette_view<'a>(&'a self, p: &'a Palette) -> Element<'a, Message> {
        let muted = rgb(qur_dark::TEXT_MUTED);
        let input = text_input("Type a command", &p.query)
            .id(Id::new("palette"))
            .on_input(Message::PaletteChanged)
            .on_submit(Message::PaletteSubmit)
            .size(13)
            .padding(6);
        let items = palette_items(&p.query);
        let start = p.selected.saturating_sub(11);
        let mut list = column![].spacing(0);
        for (i, c) in items.iter().enumerate().skip(start).take(12) {
            let enabled = c.tier == Tier::S;
            let keys = c.keys.first().map(|k| commands::pretty(k)).unwrap_or_default();
            let title = if enabled { c.title.to_string() } else { format!("{} — not in spike", c.title) };
            let selected = i == p.selected;
            let fg = if enabled { rgb(qur_dark::TEXT) } else { rgb(0x5A5A5A) };
            let r = container(row![text(title).size(13).color(fg).width(Length::Fill), text(keys).size(UI_SIZE).color(muted)])
                .padding([4.0, 8.0])
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: selected.then(|| rgb(0x262626).into()),
                    border: border::rounded(3),
                    ..Default::default()
                });
            list = list.push(mouse_area(r).on_press(Message::PaletteRun(c.id)));
        }
        let panel = container(column![input, list].spacing(6))
            .padding(8)
            .width(560)
            .style(|_| container::Style {
                background: Some(rgb(0x0E0E0E).into()),
                border: iced::Border { color: rgb(qur_dark::BORDER), width: 1.0, radius: 6.0.into() },
                ..Default::default()
            });
        container(panel)
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .padding(iced::Padding { top: TOPBAR_H + 24.0, ..Default::default() })
            .into()
    }

    fn transparency_view(&self) -> Element<'_, Message> {
        let muted = rgb(qur_dark::TEXT_MUTED);
        let pct = (self.opacity * 100.0).round();
        // iced 0.14 exposes blur only as a window-creation setting.
        let blur = checkbox(self.blur).label("Blur").size(14).text_size(UI_SIZE);
        let pop = container(
            column![
                text(format!("Opacity {pct}%")).size(UI_SIZE),
                slider(50.0..=100.0, pct, Message::OpacityChanged).step(1.0f32).width(200),
                blur,
                text("Blur is set at launch (QUR_SPIKE_BLUR=1); iced can't toggle it live.").size(11).color(muted).width(220),
            ]
            .spacing(8),
        )
        .padding(10)
        .style(|_| container::Style {
            background: Some(rgb(0x0E0E0E).into()),
            border: iced::Border { color: rgb(qur_dark::BORDER), width: 1.0, radius: 6.0.into() },
            ..Default::default()
        });
        container(pop).padding(iced::Padding { top: TOPBAR_H + 4.0, left: TRAFFIC_LIGHTS_W, ..Default::default() }).into()
    }
}

fn hairline<'a>() -> Element<'a, Message> {
    // iced borders are all-sides, so hairlines are their own 1px rows.
    container(Space::new().width(Length::Fill).height(1))
        .style(|_| container::Style { background: Some(rgb(qur_dark::BORDER).into()), ..Default::default() })
        .into()
}

fn unfocus_inputs() -> Task<Message> {
    // Focusing an id nothing has unfocuses every text input.
    operation::focus(Id::new("__none"))
}

fn paste(target: Focus) -> Task<Message> {
    iced::clipboard::read().map(move |text| Message::Paste { text, target })
}

/// Wrap pasted text in bracketed-paste markers when the shell asked for them.
fn bracketed(s: &str, t: &Terminal) -> Vec<u8> {
    let on = t.term.lock().mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
    if on {
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(s.replace('\x1b', "").as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        s.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// Palette entries for a query: every keymap command, best match first.
fn palette_items(q: &str) -> Vec<&'static commands::Command> {
    let mut v: Vec<(i32, usize, &commands::Command)> = COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, c)| c.palette)
        .filter_map(|(i, c)| commands::fuzzy(q, c.title).map(|s| (s + if c.tier == Tier::S { 2 } else { 0 }, i, c)))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    // De-duplicate commands listed twice in menus (same id).
    let mut seen = std::collections::HashSet::new();
    v.into_iter().filter(|(_, _, c)| seen.insert(c.id)).map(|(_, _, c)| c).collect()
}

// ---------------------------------------------------------------------------
// Worktrees (Cmd+Shift+T agent tasks)
// ---------------------------------------------------------------------------

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Proc::new("git").arg("-C").arg(root).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn qur_home() -> PathBuf {
    std::env::var_os("QUR_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap_or(Path::new(".")).join("out/qur-home"))
}

/// Runs off the UI thread (executor pool).
fn create_worktree(root: &Path, n: usize) -> Result<TaskInfo, String> {
    if git(root, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return Err(format!("{} is not a git repository, so agent tasks can't get a worktree.", root.display()));
    }
    let first = git(root, &["rev-list", "--max-parents=0", "HEAD"]).map_err(|e| format!("git: {e}"))?;
    let repo_id: String = first.lines().next().unwrap_or("").chars().take(12).collect();
    // Skip numbers already taken (earlier sessions, or another Qur on this repo).
    let base = qur_home().join("worktrees").join(&repo_id);
    let mut n = n;
    while base.join(format!("term-{n}")).exists()
        || git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/qur/claude/term-{n}")]).is_ok()
    {
        n += 1;
    }
    let branch = format!("qur/claude/term-{n}");
    let path = base.join(format!("term-{n}"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    git(root, &["worktree", "add", "-b", &branch, &path.to_string_lossy(), "HEAD"]).map_err(|e| format!("git worktree add: {e}"))?;
    Ok(TaskInfo { n, path, branch })
}
