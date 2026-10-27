//! Bottom panel terminals: alacritty_terminal for PTY + emulation, rendered as
//! a monospace grid. PTY I/O runs on alacritty's own thread; the UI redraws
//! only when the active terminal has new output.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as TPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::index::Direction;
use alacritty_terminal::term::search::{Match, RegexSearch};
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{ClearMode, Color, CursorShape, Handler, NamedColor};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ScrollEvent, ShapedLine, TouchPhase,
    SharedString, Style, Task, TextRun, UTF16Selection, Window, div, fill, font, point,
    prelude::*, px, relative, size,
};
use qur_theme::{Rgba, TerminalColors};

use crate::commands::{edit, qur, terminal as ta};
use crate::input::{InputEvent, TextInput};
use crate::look::Look;
use crate::tiling::{self, Motion, Rect};

// ---------------------------------------------------------------------------
// Colors (from the theme's terminal palette)
// ---------------------------------------------------------------------------

fn rgb(c: Rgba) -> u32 {
    c.0 >> 8
}

fn indexed(i: u8, t: &TerminalColors) -> u32 {
    match i {
        0..=15 => rgb(t.ansi[i as usize]),
        16..=231 => {
            let i = i - 16;
            let v = |n: u8| if n == 0 { 0 } else { 55 + 40 * n as u32 };
            (v(i / 36) << 16) | (v((i / 6) % 6) << 8) | v(i % 6)
        }
        _ => {
            let g = 8 + 10 * (i as u32 - 232);
            (g << 16) | (g << 8) | g
        }
    }
}

fn dimmed(v: u32) -> u32 {
    (v >> 1) & 0x7F7F7F
}

/// `None` = the default background (not painted, so window opacity shows).
fn resolve(c: Color, term: &Term<Listener>, is_bg: bool, dim: bool, t: &TerminalColors) -> Option<u32> {
    let to = |r: alacritty_terminal::vte::ansi::Rgb| ((r.r as u32) << 16) | ((r.g as u32) << 8) | r.b as u32;
    let colors = term.colors();
    let v = match c {
        Color::Spec(r) => to(r),
        Color::Indexed(i) => colors[i as usize].map(to).unwrap_or_else(|| indexed(i, t)),
        Color::Named(n) => {
            if let Some(r) = colors[n] {
                to(r)
            } else {
                match n {
                    NamedColor::Background => return if is_bg { None } else { Some(rgb(t.background)) },
                    NamedColor::Foreground | NamedColor::BrightForeground => rgb(t.foreground),
                    NamedColor::DimForeground => dimmed(rgb(t.foreground)),
                    NamedColor::Cursor => rgb(t.cursor),
                    n if (n as usize) < 16 => rgb(t.ansi[n as usize]),
                    n => dimmed(rgb(t.ansi[(n as usize).saturating_sub(NamedColor::DimBlack as usize) % 8])),
                }
            }
        }
    };
    Some(if dim { dimmed(v) } else { v })
}

fn hex(c: u32) -> gpui::Hsla {
    gpui::rgb(c).into()
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// The latest title the program set (OSC 0/2), e.g. Claude Code's summary
/// of what it's working on.
type TitleCell = Arc<std::sync::Mutex<Option<String>>>;

/// The PTY's input side, for answers the terminal owes the program.
type Reply = Arc<std::sync::Mutex<Option<EventLoopSender>>>;

/// The theme's terminal colors, for color queries (OSC 10/11/4). One per
/// panel, set from its window's look, so each window answers with its own theme.
type ColorsCell = Arc<std::sync::Mutex<Option<TerminalColors>>>;

#[derive(Clone)]
pub struct Listener(UnboundedSender<()>, TitleCell, Reply, ColorsCell);

impl Listener {
    fn reply(&self, text: String) {
        if let Some(s) = self.2.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = s.send(Msg::Input(text.into_bytes().into()));
        }
    }

    /// The color a query for `index` gets: 0–255 the palette, 256 or 259+ the
    /// foreground, 257 the background, 258 the cursor. `None` before the
    /// panel has its window's colors.
    fn color_answer(&self, index: usize) -> Option<alacritty_terminal::vte::ansi::Rgb> {
        let colors = self.3.lock().unwrap_or_else(|e| e.into_inner());
        let t = colors.as_ref()?;
        let v = match index {
            i if i < 256 => indexed(i as u8, t),
            257 => rgb(t.background),
            258 => rgb(t.cursor),
            _ => rgb(t.foreground),
        };
        Some(alacritty_terminal::vte::ansi::Rgb { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 })
    }
}

impl EventListener for Listener {
    fn send_event(&self, ev: TermEvent) {
        match &ev {
            TermEvent::Title(t) => *self.1.lock().unwrap_or_else(|e| e.into_inner()) = Some(t.clone()),
            TermEvent::ResetTitle => *self.1.lock().unwrap_or_else(|e| e.into_inner()) = None,
            // Status and cursor reports (DSR, DA…) the program asked for.
            TermEvent::PtyWrite(text) => self.reply(text.clone()),
            // "What's your background?": programs such as Claude Code pick their
            // light or dark palette from this, so answer with the theme's colors.
            TermEvent::ColorRequest(index, format) => {
                if let Some(c) = self.color_answer(*index) {
                    self.reply(format(c));
                }
            }
            _ => {}
        }
        if matches!(ev, TermEvent::Wakeup | TermEvent::ChildExit(_) | TermEvent::Exit | TermEvent::Title(_) | TermEvent::ResetTitle) {
            let _ = self.0.unbounded_send(());
        }
    }
}

struct Dims {
    cols: usize,
    lines: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Kind {
    Shell,
    /// A terminal-mode agent task: runs `command` in the task's worktree
    /// (New Agent Terminal, or `mode = "terminal"` agents from agents.toml).
    Agent { task: String, command: String },
}

pub struct Session {
    pub id: usize,
    pub kind: Kind,
    pub label: String,
    /// The label was typed by you (Rename), so it isn't replaced by what's running.
    pub renamed: bool,
    /// Where it started (resolves relative paths on Cmd-click).
    pub cwd: PathBuf,
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    pid: i32,
    size: (usize, usize),
    pub last_output: Option<Instant>,
    /// Text typed since the last Enter (for the task title).
    typed: String,
    in_escape: bool,
    pub first_line: Option<String>,
    title: TitleCell,
    /// The selection you made, with the scrollback size at the time. Programs
    /// that redraw (Claude Code's spinner and prompt box) erase lines, which
    /// drops the terminal's selection; it's put back from here.
    kept: Option<(Selection, usize)>,
    /// What that selection said when you made it (what Cmd+C copies).
    selected_text: Option<String>,
    _reader: Task<()>,
}

pub fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
}

type Spawned = (Arc<FairMutex<Term<Listener>>>, EventLoopSender, i32, futures::channel::mpsc::UnboundedReceiver<()>);

/// Lines of scrollback for terminals opened from now on (`[terminal] scrollback`).
static SCROLLBACK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(qur_config::DEFAULT_SCROLLBACK);

pub fn set_scrollback(n: usize) {
    SCROLLBACK.store(n, std::sync::atomic::Ordering::Relaxed);
}

fn spawn_pty(cwd: &Path, run: Option<&str>, id: usize, title: TitleCell, colors: ColorsCell) -> std::io::Result<Spawned> {
    let (tx, rx) = unbounded();
    let reply = Reply::default();
    let listener = Listener(tx, title, reply.clone(), colors);
    let (cols, lines) = (100, 12);
    // `env -u` strips vendor API keys so the CLIs use their own logins.
    let mut args = vec!["-u".into(), "ANTHROPIC_API_KEY".into(), "-u".into(), "OPENAI_API_KEY".into(), shell(), "-l".into()];
    if let Some(cmd) = run {
        // The shell stays open after the agent exits.
        args.push("-c".into());
        args.push(format!("{cmd}; exec \"$SHELL\" -l"));
    }
    let opts = tty::Options {
        shell: Some(tty::Shell::new("/usr/bin/env".into(), args)),
        working_directory: Some(cwd.to_path_buf()),
        drain_on_exit: false,
        env: HashMap::from([
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("TERM_PROGRAM".into(), "qur".into()),
            // The user's full shell PATH (qur_agents::login_path), so `claude`
            // and friends are found even when Qur was opened from Finder.
            ("PATH".into(), qur_agents::login_path().into()),
        ]),
    };
    let term = Arc::new(FairMutex::new(Term::new(TermConfig { scrolling_history: SCROLLBACK.load(std::sync::atomic::Ordering::Relaxed), ..TermConfig::default() }, &Dims { cols, lines }, listener.clone())));
    let pty = tty::new(&opts, WindowSize { num_lines: lines as u16, num_cols: cols as u16, cell_width: 8, cell_height: 16 }, id as u64)?;
    let pid = pty.child().id() as i32;
    let ev = EventLoop::new(term.clone(), listener, pty, false, false)?;
    let sender = ev.channel();
    *reply.lock().unwrap_or_else(|e| e.into_inner()) = Some(sender.clone());
    ev.spawn();
    Ok((term, sender, pid, rx))
}

/// `kept` (a selection and the scrollback size when it was taken) as the
/// terminal's selection again if output erased it, moved up by the lines
/// that scrolled since. Returns what to keep from now on.
fn restore_selection<T: EventListener>(t: &mut Term<T>, (sel, hist): (Selection, usize)) -> Option<(Selection, usize)> {
    let now = t.history_size();
    let sel = match &t.selection {
        Some(s) => Some(s.clone()),
        None => {
            let range = Line(-(now as i32))..Line(t.screen_lines() as i32);
            let moved = sel.rotate(&*t, &range, now as i32 - hist as i32);
            t.selection = moved.clone();
            moved
        }
    };
    sel.map(|s| (s, now))
}

/// Every process in the session the PTY child leads (setsid: sid == child pid).
fn session_pids(sid: i32) -> Vec<i32> {
    let mut buf = vec![0i32; 8192];
    let n = unsafe { libc::proc_listallpids(buf.as_mut_ptr().cast(), (buf.len() * 4) as i32) };
    buf.truncate(n.max(0) as usize);
    buf.into_iter().filter(|&p| p > 0 && unsafe { libc::getsid(p) } == sid).collect()
}

/// SIGHUP the whole session, then SIGKILL whatever is left after `grace`.
pub fn kill_session(sid: i32, grace: Duration) {
    for p in session_pids(sid) {
        unsafe { libc::kill(p, libc::SIGHUP) };
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if session_pids(sid).is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    for p in session_pids(sid) {
        unsafe { libc::kill(p, libc::SIGKILL) };
    }
}

impl Session {
    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let b = bytes.into();
        if !b.is_empty() {
            let _ = self.sender.send(Msg::Input(b));
        }
    }

    fn shutdown(&self, sync: bool) {
        let _ = self.sender.send(Msg::Shutdown);
        let pid = self.pid;
        if sync {
            kill_session(pid, Duration::from_millis(300));
        } else {
            std::thread::spawn(move || kill_session(pid, Duration::from_secs(2)));
        }
    }

    /// The shell's pid (it leads the terminal's session).
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// What the agent says it's working on (its terminal title), when that's
    /// a real description rather than the program's or shell's name.
    pub fn agent_title(&self) -> Option<String> {
        let raw = self.title.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
        clean_title(&raw)
    }

    /// A shell's name in the sidebar: yours if you renamed it, else what's
    /// running in it (the program's title, or the foreground command such as
    /// `cargo` or `npm`), else its numbered shell name.
    pub fn display_label(&self) -> String {
        if self.renamed {
            return self.label.clone();
        }
        let Some(running) = foreground_name(self.pid) else { return self.label.clone() };
        match self.agent_title() {
            Some(t) => t,
            None => running,
        }
    }

    /// The program has printed something since it started.
    pub fn has_output(&self) -> bool {
        self.last_output.is_some()
    }

    pub fn is_busy(&self) -> bool {
        self.last_output.is_some_and(|t| t.elapsed() < Duration::from_secs(2))
    }

    /// The program asked for bracketed paste: it's at an input line that
    /// takes pasted text (an agent's prompt, a shell), not a menu where a
    /// typed letter could pick an answer.
    pub fn takes_paste(&self) -> bool {
        self.term.lock().mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Note the selection just made (see `kept`).
    fn remember_selection(&mut self) {
        let t = self.term.lock();
        let kept = t.selection.clone().map(|s| (s, t.history_size()));
        let text = t.selection_to_string().filter(|s| !s.is_empty());
        drop(t);
        self.kept = kept;
        self.selected_text = text;
    }

    /// Put back a selection that output erased, moved along with any lines
    /// that scrolled into the scrollback since.
    fn restore_selection(&mut self) {
        let Some(kept) = self.kept.take() else { return };
        let mut t = self.term.lock();
        self.kept = restore_selection(&mut t, kept);
    }

    fn forget_selection(&mut self) {
        self.term.lock().selection = None;
        self.kept = None;
        self.selected_text = None;
    }
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

pub enum PanelEvent {
    /// A session's busy/idle state or title changed (sidebar status).
    Activity,
    /// All tabs closed, or a tab was closed.
    Closed,
    /// Cmd-click on a URL.
    OpenUrl(String),
    /// Cmd-click on `path[:line[:col]]` that exists (absolute path).
    OpenPath(PathBuf, Option<(u32, u32)>),
}

/// Find in Terminal (Cmd+F): the query field and the current match.
struct Find {
    input: Entity<TextInput>,
    last: Option<Match>,
    error: bool,
    _sub: gpui::Subscription,
}

/// Split terminals (owner: "split terminals like in linux"): sessions shown
/// side by side, like iTerm or Terminator. A tree of panes, each one session.
#[derive(Clone, Debug, PartialEq)]
enum Tile {
    Term(usize),
    /// `vertical` = stacked top/bottom; `ratio` = the first child's share.
    Split { vertical: bool, ratio: f32, a: Box<Tile>, b: Box<Tile> },
}

impl Tile {
    fn ids(&self) -> Vec<usize> {
        match self {
            Tile::Term(id) => vec![*id],
            Tile::Split { a, b, .. } => [a.ids(), b.ids()].concat(),
        }
    }

    /// Put `new` beside `target` (right of it, or below when `vertical`).
    fn split_at(&mut self, target: usize, new: usize, vertical: bool) -> bool {
        match self {
            Tile::Term(id) if *id == target => {
                *self = Tile::Split { vertical, ratio: 0.5, a: Box::new(Tile::Term(target)), b: Box::new(Tile::Term(new)) };
                true
            }
            Tile::Term(_) => false,
            Tile::Split { a, b, .. } => a.split_at(target, new, vertical) || b.split_at(target, new, vertical),
        }
    }

    /// `ids` as a grid in a `w`×`h` area: as many rows as keep the panes
    /// closest to square, the panes shared evenly between them (fuller rows
    /// first), every row and pane the same size.
    fn grid(ids: &[usize], w: f32, h: f32) -> Tile {
        let n = ids.len().max(1);
        let off_square = |rows: usize| {
            let cols = n.div_ceil(rows);
            ((w / cols as f32) / (h / rows as f32)).ln().abs()
        };
        let rows = (1..=n).min_by(|&a, &b| off_square(a).total_cmp(&off_square(b))).unwrap_or(1);
        // Even shares in nested halves: the first of k takes 1/k.
        fn chain(mut tiles: Vec<Tile>, vertical: bool) -> Tile {
            let first = tiles.remove(0);
            if tiles.is_empty() {
                return first;
            }
            let ratio = 1. / (tiles.len() + 1) as f32;
            Tile::Split { vertical, ratio, a: Box::new(first), b: Box::new(chain(tiles, vertical)) }
        }
        let mut rest = ids;
        let rows: Vec<Tile> = (0..rows)
            .map(|r| {
                let take = n / rows + usize::from(r < n % rows);
                let (row, more) = rest.split_at(take);
                rest = more;
                chain(row.iter().map(|&id| Tile::Term(id)).collect(), false)
            })
            .collect();
        chain(rows, true)
    }

    /// Whether the split directly holding `id` stacks top/bottom.
    fn parent_vertical(&self, id: usize) -> Option<bool> {
        match self {
            Tile::Term(_) => None,
            Tile::Split { vertical, a, b, .. } => {
                if matches!(**a, Tile::Term(t) if t == id) || matches!(**b, Tile::Term(t) if t == id) {
                    Some(*vertical)
                } else {
                    a.parent_vertical(id).or_else(|| b.parent_vertical(id))
                }
            }
        }
    }

    /// Without `id`; a split left with one pane collapses into it.
    fn without(self, id: usize) -> Option<Tile> {
        match self {
            Tile::Term(t) => (t != id).then_some(self),
            Tile::Split { vertical, ratio, a, b } => match (a.without(id), b.without(id)) {
                (Some(a), Some(b)) => Some(Tile::Split { vertical, ratio, a: Box::new(a), b: Box::new(b) }),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            },
        }
    }

    fn set_ratio(&mut self, path: &[bool], r: f32) {
        match (self, path.split_first()) {
            (Tile::Split { ratio, .. }, None) => *ratio = r.clamp(0.12, 0.88),
            (Tile::Split { a, b, .. }, Some((&second, rest))) => if second { b } else { a }.set_ratio(rest, r),
            _ => {}
        }
    }

    fn is_vertical(&self, path: &[bool]) -> bool {
        match (self, path.split_first()) {
            (Tile::Split { vertical, .. }, None) => *vertical,
            (Tile::Split { a, b, .. }, Some((&second, rest))) => if second { b } else { a }.is_vertical(rest),
            _ => false,
        }
    }
}

/// A sidebar row's splits. `owner` is the session the row stands for; the
/// other panes are shells that belong to it and have no row of their own.
struct Group {
    owner: usize,
    tile: Tile,
    /// The pane last focused, focused again when the row is selected.
    last: usize,
    /// Laid out as a grid (Cmd+Enter): closing a pane lays it out again.
    /// Off once a pane is split by hand (Cmd+D, Cmd+Shift+D).
    grid: bool,
}

pub struct TerminalPanel {
    pub focus: FocusHandle,
    pub sessions: Vec<Session>,
    pub active: usize,
    next_id: usize,
    pub message: Option<String>,
    cell: (Pixels, Pixels),
    /// Each shown pane's grid, as last painted (mouse hits, focus moves).
    pane_bounds: HashMap<usize, Bounds<Pixels>>,
    selecting: bool,
    /// Scroll not yet turned into whole lines (see `on_scroll`).
    scroll_px: Pixels,
    find: Option<Find>,
    idle_check: Option<Task<()>>,
    groups: Vec<Group>,
    /// A group's owner closed and another pane took its row: old -> new.
    moved: HashMap<usize, usize>,
    split_bounds: Rc<RefCell<HashMap<Vec<bool>, Bounds<Pixels>>>>,
    /// The panes' glide between arrangements, and the area they're laid out in.
    glide: Motion<usize>,
    glide_area: Rc<std::cell::Cell<Bounds<Pixels>>>,
    /// A divider being dragged: its split's path and bounds.
    resizing: Option<(Vec<bool>, Bounds<Pixels>)>,
    /// Shown as the whole stage (a shell row), not inside an Agent
    /// Terminal's dock pane: with window gaps on, its panes are the tiles.
    /// Set by the workspace as it lays the stage out.
    pub standalone: std::cell::Cell<bool>,
    /// Zen's bar is out (sliding or down), pushing the panes down: each
    /// grid keeps the size it had with the bar away, pinned to its pane's
    /// bottom with its top rows clipped. Resizing as the bar moved sent the
    /// program a new size every few frames, and Claude Code redrew its input
    /// box each time: the bottom of the terminal jiggled. Set by the workspace.
    pub zen_hold: std::cell::Cell<bool>,
    /// Each pane's grid size with zen's bar away: width, height.
    rest_size: HashMap<usize, (Pixels, Pixels)>,
    /// How far above its pane a held grid starts (mouse to cell).
    grid_shift: HashMap<usize, Pixels>,
    /// This window's terminal colors, shared with its sessions' listeners.
    colors: ColorsCell,
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl TerminalPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        TerminalPanel {
            standalone: std::cell::Cell::new(false),
            focus: cx.focus_handle(),
            sessions: Vec::new(),
            active: 0,
            next_id: 1,
            message: None,
            cell: (px(8.), px(16.)),
            pane_bounds: HashMap::new(),
            zen_hold: std::cell::Cell::new(false),
            rest_size: HashMap::new(),
            grid_shift: HashMap::new(),
            selecting: false,
            scroll_px: px(0.),
            find: None,
            idle_check: None,
            groups: Vec::new(),
            moved: HashMap::new(),
            split_bounds: Rc::default(),
            glide: Motion::default(),
            glide_area: Rc::default(),
            resizing: None,
            colors: ColorsCell::default(),
        }
    }

    /// The theme's terminal colors for this window (set as its look changes),
    /// which its terminals answer color queries with.
    pub fn set_colors(&self, colors: TerminalColors) {
        *self.colors.lock().unwrap_or_else(|e| e.into_inner()) = Some(colors);
    }

    pub fn open(&mut self, cwd: &Path, kind: Kind, label: String, cx: &mut Context<Self>) -> Option<usize> {
        let id = self.next_id;
        self.next_id += 1;
        let run = match &kind {
            Kind::Agent { command, .. } => Some(command.clone()),
            Kind::Shell => None,
        };
        let title = TitleCell::default();
        let (term, sender, pid, mut rx) = match spawn_pty(cwd, run.as_deref(), id, title.clone(), self.colors.clone()) {
            Ok(x) => x,
            Err(e) => {
                self.message = Some(format!("terminal failed: {e}"));
                cx.notify();
                return None;
            }
        };
        let reader = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                // Coalesce bursts: drain whatever else is queued.
                while let Ok(()) = rx.try_recv() {}
                let alive = this
                    .update(cx, |p, cx| {
                        let Some(ix) = p.sessions.iter().position(|s| s.id == id) else { return };
                        let was_busy = p.sessions[ix].is_busy();
                        p.sessions[ix].last_output = Some(Instant::now());
                        if p.shown().contains(&id) {
                            cx.notify();
                        }
                        if !was_busy {
                            cx.emit(PanelEvent::Activity);
                            p.schedule_idle_check(cx);
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });
        self.sessions.push(Session {
            id,
            kind,
            label,
            renamed: false,
            cwd: cwd.to_path_buf(),
            term,
            sender,
            pid,
            size: (100, 12),
            last_output: None,
            typed: String::new(),
            in_escape: false,
            first_line: None,
            title,
            kept: None,
            selected_text: None,
            _reader: reader,
        });
        self.active = self.sessions.len() - 1;
        self.message = None;
        cx.notify();
        Some(id)
    }

    /// Flip sessions back to idle 2 s after their last output.
    fn schedule_idle_check(&mut self, cx: &mut Context<Self>) {
        if self.idle_check.is_some() {
            return;
        }
        let bg = cx.background_executor().clone();
        self.idle_check = Some(cx.spawn(async move |this, cx| {
            loop {
                bg.timer(Duration::from_millis(500)).await;
                let keep = this
                    .update(cx, |p, cx| {
                        cx.emit(PanelEvent::Activity);
                        let any_busy = p.sessions.iter().any(|s| s.is_busy());
                        if !any_busy {
                            p.idle_check = None;
                        }
                        any_busy
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    pub fn label_for(&self, kind: &Kind) -> String {
        let base = match kind {
            Kind::Shell => shell().rsplit('/').next().unwrap_or("sh").to_string(),
            Kind::Agent { task, .. } => return task.clone(),
        };
        // The lowest free number, so closing "zsh" and opening another doesn't
        // give two "zsh 2"s.
        let taken: Vec<&str> = self.sessions.iter().filter(|s| s.kind == Kind::Shell).map(|s| s.label.as_str()).collect();
        free_label(&base, &taken)
    }

    /// Rename a shell; empty goes back to the automatic name.
    pub fn rename(&mut self, id: usize, name: &str, cx: &mut Context<Self>) {
        let auto = if name.is_empty() {
            // Its own label shouldn't count as taken.
            if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
                s.label.clear();
            }
            Some(self.label_for(&Kind::Shell))
        } else {
            None
        };
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.renamed = auto.is_none();
            s.label = auto.unwrap_or_else(|| name.to_string());
        }
        cx.emit(PanelEvent::Activity);
        cx.notify();
    }

    pub fn close(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.sessions.len() {
            return;
        }
        let s = self.sessions.remove(ix);
        s.shutdown(false);
        let mut next = None;
        if let Some(g) = self.groups.iter().position(|g| g.tile.ids().contains(&s.id)) {
            let group = self.groups.remove(g);
            if group.owner == s.id && s.kind != Kind::Shell {
                // An agent or Run Project terminal ended: its splits go with it.
                for id in group.tile.ids() {
                    if let Some(i) = self.sessions.iter().position(|x| x.id == id) {
                        self.sessions.remove(i).shutdown(false);
                    }
                }
            } else if let Some(tile) = group.tile.without(s.id) {
                let rest = tile.ids();
                let owner = if group.owner == s.id {
                    // A shell's row lives on in the pane beside it.
                    self.moved.insert(s.id, rest[0]);
                    rest[0]
                } else {
                    group.owner
                };
                next = Some(if rest.contains(&group.last) { group.last } else { rest[0] });
                if rest.len() > 1 {
                    self.groups.push(Group { owner, tile, last: next.unwrap_or(owner), grid: group.grid });
                    if group.grid {
                        self.regrid(owner);
                    }
                }
            }
        }
        match next.and_then(|id| self.sessions.iter().position(|x| x.id == id)) {
            Some(i) => self.active = i,
            None if self.active >= self.sessions.len() => self.active = self.sessions.len().saturating_sub(1),
            None => {}
        }
        self.pane_bounds.remove(&s.id);
        self.rest_size.remove(&s.id);
        self.grid_shift.remove(&s.id);
        cx.emit(PanelEvent::Closed);
        cx.notify();
    }

    /// Cmd+W on session `id`: in a dwindled row (not a grid) the newest pane,
    /// the smallest, goes first, and focus stays where it was if it survives.
    pub fn close_newest(&mut self, id: usize, cx: &mut Context<Self>) {
        let focused = self.active_session().map(|s| s.id);
        let mut victim = id;
        if let Some(g) = self.groups.iter_mut().find(|g| !g.grid && g.tile.ids().contains(&id)) {
            let mut ids = g.tile.ids();
            ids.sort_unstable();
            victim = ids[ids.len() - 1];
            g.last = focused.filter(|f| *f != victim && ids.contains(f)).unwrap_or(ids[ids.len() - 2]);
        }
        if let Some(ix) = self.sessions.iter().position(|s| s.id == victim) {
            self.close(ix, cx);
        }
    }

    /// Close a sidebar row: the session `id` and every pane split beside it.
    pub fn close_row(&mut self, id: usize, cx: &mut Context<Self>) {
        let ids = self.groups.iter().find(|g| g.tile.ids().contains(&id)).map(|g| g.tile.ids()).unwrap_or_else(|| vec![id]);
        for id in ids {
            if let Some(ix) = self.sessions.iter().position(|s| s.id == id) {
                self.close(ix, cx);
            }
        }
    }

    pub fn close_all_sync(&mut self) {
        self.groups.clear();
        for s in self.sessions.drain(..) {
            s.shutdown(true);
        }
    }

    /// The row that `id` (a closed group owner) now lives in, if any.
    pub fn successor(&self, mut id: usize) -> Option<usize> {
        let mut found = None;
        while let Some(&n) = self.moved.get(&id) {
            found = Some(n);
            id = n;
        }
        found
    }

    /// Open a shell in `cwd` beside the focused terminal (below it when
    /// `vertical`) and focus it; with `grid`, lay the group out as a grid.
    pub fn split_active(&mut self, cwd: &Path, vertical: bool, grid: bool, cx: &mut Context<Self>) -> Option<usize> {
        let target = self.active_session()?.id;
        let label = self.label_for(&Kind::Shell);
        let new = self.open(cwd, Kind::Shell, label, cx)?;
        match self.groups.iter_mut().find(|g| g.tile.ids().contains(&target)) {
            Some(g) => {
                g.tile.split_at(target, new, vertical);
                g.last = new;
                g.grid = grid;
            }
            None => {
                let mut tile = Tile::Term(target);
                tile.split_at(target, new, vertical);
                self.groups.push(Group { owner: target, tile, last: new, grid });
            }
        }
        if grid {
            self.regrid(new);
        }
        cx.notify();
        Some(new)
    }

    /// Cmd+Enter: another shell, and the group laid out as a grid of
    /// near-square panes.
    pub fn grid_active(&mut self, cwd: &Path, cx: &mut Context<Self>) -> Option<usize> {
        self.split_active(cwd, false, true, cx)
    }

    /// Lay the group holding `id` out again as a grid of near-square panes,
    /// oldest first, so added terminals never end up as thin rows or columns.
    fn regrid(&mut self, id: usize) {
        let Some(g) = self.groups.iter_mut().find(|g| g.tile.ids().contains(&id)) else { return };
        let mut ids = g.tile.ids();
        ids.sort_unstable();
        // The area the group fills: its panes as last painted (a new pane
        // has no size yet; it shares the space its neighbours had).
        let area = ids.iter().filter_map(|i| self.pane_bounds.get(i)).copied().reduce(|a, b| a.union(&b));
        let (w, h) = area.map(|b| (f32::from(b.size.width), f32::from(b.size.height))).filter(|(w, h)| *w > 0. && *h > 0.).unwrap_or((2., 1.));
        g.tile = Tile::grid(&ids, w, h);
    }

    /// Cmd+D, tiling-WM style (dwindle, like Hyprland or bspwm): halve the
    /// focused pane along its longer side, so new terminals keep filling the view.
    pub fn dwindle_active(&mut self, cwd: &Path, cx: &mut Context<Self>) -> Option<usize> {
        let target = self.active_session()?.id;
        // A pane not drawn yet (just made, or the window is covered) has no
        // size: turn the other way from the split that made it.
        let vertical = match self.pane_bounds.get(&target) {
            Some(b) => b.size.height > b.size.width,
            None => self.groups.iter().find_map(|g| g.tile.parent_vertical(target)).is_some_and(|v| !v),
        };
        self.split_active(cwd, vertical, false, cx)
    }

    /// The group the focused session is in.
    fn active_group(&self) -> Option<&Group> {
        let id = self.active_session()?.id;
        self.groups.iter().find(|g| g.tile.ids().contains(&id))
    }

    /// The sessions on screen: the focused one's group, or just it.
    fn shown(&self) -> Vec<usize> {
        match self.active_group() {
            Some(g) => g.tile.ids(),
            None => self.active_session().map(|s| vec![s.id]).unwrap_or_default(),
        }
    }

    /// Focus the pane with session `id` (it's on screen).
    fn focus_pane(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(ix) = self.sessions.iter().position(|s| s.id == id) else { return };
        self.active = ix;
        if let Some(g) = self.groups.iter_mut().find(|g| g.tile.ids().contains(&id)) {
            g.last = id;
        }
        cx.notify();
    }

    /// The shown pane under the pointer.
    fn pane_at(&self, pos: gpui::Point<Pixels>) -> Option<usize> {
        self.shown().into_iter().find(|id| self.pane_bounds.get(id).is_some_and(|b| b.contains(&pos)))
    }

    /// Cmd+Opt+arrows: focus the nearest pane in that direction.
    fn focus_toward(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        let Some(cur) = self.active_session().map(|s| s.id) else { return };
        let Some(b) = self.pane_bounds.get(&cur).copied() else { return };
        let c = b.center();
        let best = self
            .shown()
            .into_iter()
            .filter(|id| *id != cur)
            .filter_map(|id| {
                let o = self.pane_bounds.get(&id)?;
                let beyond = if dx < 0. {
                    o.right() <= b.left() + px(1.)
                } else if dx > 0. {
                    o.left() >= b.right() - px(1.)
                } else if dy < 0. {
                    o.bottom() <= b.top() + px(1.)
                } else {
                    o.top() >= b.bottom() - px(1.)
                };
                let oc = o.center();
                // Mostly along the direction; a little sideways drift counts too.
                let (along, across) = if dx != 0. { (oc.x - c.x, oc.y - c.y) } else { (oc.y - c.y, oc.x - c.x) };
                beyond.then(|| (id, f32::from(along).abs() + 2. * f32::from(across).abs()))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, _)) = best {
            self.focus_pane(id, cx);
        }
    }

    fn focus_left(&mut self, _: &ta::FocusLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_toward(-1., 0., cx)
    }
    fn focus_right(&mut self, _: &ta::FocusRight, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_toward(1., 0., cx)
    }
    fn focus_up(&mut self, _: &ta::FocusUp, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_toward(0., -1., cx)
    }
    fn focus_down(&mut self, _: &ta::FocusDown, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_toward(0., 1., cx)
    }


    /// Type text into a session as if pasted (bracketed when the app asks
    /// for it); no Enter, so the user reviews before sending.
    pub fn type_text(&mut self, id: usize, text: &str, cx: &mut Context<Self>) {
        if let Some(s) = self.sessions.iter().find(|s| s.id == id) {
            let bracketed = s.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
            let t = if bracketed { format!("\x1b[200~{text}\x1b[201~") } else { text.to_string() };
            s.write(t.into_bytes());
        }
        cx.notify();
    }

    /// Send raw bytes (e.g. "\r" to submit what `type_text` put there).
    pub fn send_keys(&mut self, id: usize, keys: &str) {
        if let Some(s) = self.sessions.iter().find(|s| s.id == id) {
            s.write(keys.as_bytes().to_vec());
        }
    }

    pub fn session_by_id(&self, id: usize) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn activate_id(&mut self, id: usize, cx: &mut Context<Self>) -> bool {
        // A row with splits: back to the pane you were in.
        let id = self.groups.iter().find(|g| g.owner == id && self.session_by_id(g.last).is_some()).map(|g| g.last).unwrap_or(id);
        if let Some(ix) = self.sessions.iter().position(|s| s.id == id) {
            self.active = ix;
            cx.notify();
            true
        } else {
            false
        }
    }

    fn active_session(&self) -> Option<&Session> {
        self.sessions.get(self.active)
    }

    fn write_active(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let Some(s) = self.sessions.get_mut(self.active) else { return };
        // Typing snaps back to the bottom of the scrollback.
        s.term.lock().scroll_display(Scroll::Bottom);
        // Remember the first line typed (task title).
        if s.first_line.is_none() && matches!(s.kind, Kind::Agent { .. }) {
            for &b in &bytes {
                // Skip escape sequences (arrow keys, focus reports…): only
                // what you typed names the task.
                if s.in_escape {
                    if (0x40..=0x7e).contains(&b) && b != b'[' && b != b'O' {
                        s.in_escape = false;
                    }
                    continue;
                }
                if b == 0x1b {
                    s.in_escape = true;
                    continue;
                }
                match b {
                    b'\r' => {
                        let t = s.typed.trim().to_string();
                        if !t.is_empty() {
                            s.first_line = Some(t);
                            cx.emit(PanelEvent::Activity);
                        }
                        s.typed.clear();
                    }
                    0x7f => {
                        s.typed.pop();
                    }
                    b if b >= 0x20 => s.typed.push(b as char),
                    _ => {}
                }
            }
        }
        s.write(bytes);
    }

    // -- input --------------------------------------------------------------

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let app_cursor = self
            .active_session()
            .map(|s| s.term.lock().mode().contains(TermMode::APP_CURSOR))
            .unwrap_or(false);
        if let Some(b) = key_bytes(&ev.keystroke, app_cursor) {
            self.write_active(b, cx);
            cx.stop_propagation();
        }
    }

    fn clear(&mut self, _: &ta::Clear, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.sessions.get_mut(self.active) {
            s.forget_selection();
            s.term.lock().clear_screen(ClearMode::Saved);
            s.write(&b"\x0c"[..]);
        }
        cx.notify();
    }
    fn line_start(&mut self, _: &ta::SendLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.write_active(vec![0x01], cx)
    }
    fn line_end(&mut self, _: &ta::SendLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.write_active(vec![0x05], cx)
    }
    fn kill_line(&mut self, _: &ta::SendKillLine, _: &mut Window, cx: &mut Context<Self>) {
        self.write_active(vec![0x15], cx)
    }
    /// Shift+Tab (Claude Code cycles its modes with it). Bound as an action so
    /// it can't be lost on its way through the macOS text input system.
    fn back_tab(&mut self, _: &ta::SendBackTab, _: &mut Window, cx: &mut Context<Self>) {
        self.write_active(b"\x1b[Z".to_vec(), cx)
    }
    fn copy(&mut self, _: &edit::Copy, _: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.active_session() else { return };
        let text = s.selected_text.clone().or_else(|| s.term.lock().selection_to_string()).filter(|t| !t.is_empty());
        if let Some(t) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(t));
        }
    }
    fn paste(&mut self, _: &edit::Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(clip) = cx.read_from_clipboard() else { return };
        match clip.entries().first() {
            // An image (a screenshot, say): send Ctrl+V, which is how Claude Code
            // reads an image from the clipboard (Zed does the same).
            Some(gpui::ClipboardEntry::Image(img)) if !img.bytes().is_empty() => return self.write_active(vec![0x16], cx),
            // Files copied in Finder: their paths.
            Some(gpui::ClipboardEntry::ExternalPaths(paths)) => return self.paste_paths(paths.paths(), cx),
            _ => {}
        }
        let Some(t) = clip.text() else { return };
        // "Copy Image" in a browser: its address comes first, the image behind it.
        if crate::files::has_image_behind_text(&t) {
            return self.write_active(vec![0x16], cx);
        }
        let bracketed = self
            .active_session()
            .map(|s| s.term.lock().mode().contains(TermMode::BRACKETED_PASTE))
            .unwrap_or(false);
        let t = t.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if bracketed { format!("\x1b[200~{t}\x1b[201~") } else { t };
        self.write_active(bytes.into_bytes(), cx);
    }
    /// Shell-quoted paths, space-separated (pasted or dropped files). Claude
    /// Code attaches image files given as paths.
    fn paste_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let quoted: Vec<String> = paths.iter().map(|p| shell_quote(&p.to_string_lossy())).collect();
        if quoted.is_empty() {
            return;
        }
        let text = format!(" {} ", quoted.join(" "));
        let bracketed = self.active_session().map(|s| s.term.lock().mode().contains(TermMode::BRACKETED_PASTE)).unwrap_or(false);
        let bytes = if bracketed { format!("\x1b[200~{text}\x1b[201~") } else { text };
        self.write_active(bytes.into_bytes(), cx);
    }

    fn select_all(&mut self, _: &edit::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.sessions.get_mut(self.active) {
            let mut t = s.term.lock();
            let top = t.topmost_line();
            let bottom = t.bottommost_line();
            let last = t.last_column();
            let mut sel = Selection::new(SelectionType::Simple, TPoint::new(top, Column(0)), Side::Left);
            sel.update(TPoint::new(bottom, last), Side::Right);
            t.selection = Some(sel);
            drop(t);
            s.remember_selection();
        }
        cx.notify();
    }
    fn close_terminal(&mut self, _: &qur::CloseTerminal, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_session().map(|s| s.id) {
            self.close_newest(id, cx);
        }
    }

    fn grid_point(&self, p: gpui::Point<Pixels>, display_offset: usize) -> (TPoint, Side) {
        let id = self.active_session().map(|s| s.id);
        let b = id.and_then(|id| self.pane_bounds.get(&id)).copied().unwrap_or_default();
        let shift = id.and_then(|id| self.grid_shift.get(&id)).copied().unwrap_or_default();
        let x = (p.x - b.left()).max(px(0.));
        let col = (x / self.cell.0).floor() as usize;
        let side = if (x / self.cell.0).fract() < 0.5 { Side::Left } else { Side::Right };
        let row = ((p.y - b.top()).max(px(0.)) + shift) / self.cell.1;
        let row = row.floor() as i32;
        (TPoint::new(Line(row - display_offset as i32), Column(col)), side)
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if let Some(id) = self.pane_at(ev.position)
            && Some(id) != self.active_session().map(|s| s.id)
        {
            self.focus_pane(id, cx);
        }
        if ev.modifiers.platform && self.open_link_at(ev.position, cx) {
            return;
        }
        if let Some(s) = self.sessions.get(self.active) {
            let mut t = s.term.lock();
            let off = t.grid().display_offset();
            let (p, side) = self.grid_point(ev.position, off);
            let p = TPoint::new(p.line, p.column.min(t.last_column()));
            // Double-click selects a word, triple-click a line.
            let kind = match ev.click_count {
                2 => SelectionType::Semantic,
                3 => SelectionType::Lines,
                _ => SelectionType::Simple,
            };
            t.selection = Some(Selection::new(kind, p, side));
            drop(t);
            self.selecting = true;
            let ix = self.active;
            self.sessions[ix].remember_selection();
        }
        cx.notify();
    }

    /// Cmd-click: open the URL or `path:line` under the pointer.
    fn open_link_at(&mut self, pos: gpui::Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.sessions.get(self.active) else { return false };
        let (text, col) = {
            let t = s.term.lock();
            let (p, _) = self.grid_point(pos, t.grid().display_offset());
            if p.line.0 < -(t.history_size() as i32) || p.line.0 >= t.screen_lines() as i32 {
                return false;
            }
            let row = &t.grid()[p.line];
            let text: String = (0..t.columns()).map(|c| row[Column(c)].c).collect();
            (text, p.column.0)
        };
        let ev = match link_at(&text, col).and_then(|tok| classify_link(&tok)) {
            Some(Link::Url(u)) => PanelEvent::OpenUrl(u),
            Some(Link::Path(p, at)) => {
                let path = if Path::new(&p).is_absolute() { PathBuf::from(&p) } else { s.cwd.join(&p) };
                if !path.is_file() {
                    return false;
                }
                PanelEvent::OpenPath(path, at)
            }
            None => return false,
        };
        cx.emit(ev);
        true
    }

    /// QUR_DEBUG_ACTIONS `state`: sessions, the find box and the selection.
    pub fn debug_state(&self, cx: &App) -> String {
        let sel = self.sessions.get(self.active).and_then(|s| s.term.lock().selection_to_string()).unwrap_or_default();
        let find = match &self.find {
            Some(f) => format!("find={:?} found={}", f.input.read(cx).text, !f.error && f.last.is_some()),
            None => "find=none".into(),
        };
        // The split's first share, for checking a dragged divider.
        let split = match self.active_group().map(|g| &g.tile) {
            Some(Tile::Split { ratio, .. }) => format!(" split={ratio:.3}"),
            _ => String::new(),
        };
        format!("sessions={} active={} {find} selection={sel:?}{split}", self.sessions.len(), self.active)
    }

    fn find(&mut self, _: &ta::Find, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = &self.find {
            let focus = f.input.read(cx).focus.clone();
            return window.focus(&focus, cx);
        }
        let input = cx.new(|cx| TextInput::new("Find in terminal (regex) · ↩ older · ⇧↩ newer · esc", cx));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Confirm { alt, .. } => {
                let q = input.read(cx).text.clone();
                this.find_step(&q, if *alt { Direction::Right } else { Direction::Left }, cx);
            }
            InputEvent::Changed => {
                if let Some(f) = &mut this.find {
                    f.last = None;
                }
                let q = input.read(cx).text.clone();
                this.find_step(&q, Direction::Left, cx);
            }
            InputEvent::Cancel => {
                this.find = None;
                if let Some(s) = this.sessions.get_mut(this.active) {
                    s.forget_selection();
                }
                window.focus(&this.focus, cx);
                cx.notify();
            }
            _ => {}
        });
        let focus = input.read(cx).focus.clone();
        self.find = Some(Find { input, last: None, error: false, _sub: sub });
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Next match from the current one (older = up, towards the scrollback):
    /// select it and scroll it into view.
    fn find_step(&mut self, query: &str, dir: Direction, cx: &mut Context<Self>) {
        let Some(f) = self.find.as_mut() else { return };
        let Some(s) = self.sessions.get_mut(self.active) else { return };
        if query.is_empty() {
            f.error = false;
            s.forget_selection();
            return cx.notify();
        }
        let Ok(mut regex) = RegexSearch::new(query) else {
            f.error = true;
            return cx.notify();
        };
        let mut t = s.term.lock();
        let found = search_from(&t, &mut regex, f.last.as_ref(), dir);
        f.error = found.is_none();
        if let Some(m) = found {
            let mut sel = Selection::new(SelectionType::Simple, *m.start(), Side::Left);
            sel.update(*m.end(), Side::Right);
            t.selection = Some(sel);
            // Scroll so the match sits mid-screen when it's off screen.
            let off = t.grid().display_offset() as i32;
            let rows = t.screen_lines() as i32;
            let line = m.start().line.0;
            if line < -off || line >= rows - off {
                let target = (rows / 2 - line).clamp(0, t.history_size() as i32);
                t.scroll_display(Scroll::Delta(target - off));
            }
            f.last = Some(m);
        }
        drop(t);
        s.remember_selection();
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((path, b)) = self.resizing.clone() {
            if ev.pressed_button != Some(MouseButton::Left) {
                self.resizing = None;
                return;
            }
            let owner = self.active_group().map(|g| g.owner);
            if let Some(g) = self.groups.iter_mut().find(|g| Some(g.owner) == owner) {
                let r = if g.tile.is_vertical(&path) { (ev.position.y - b.top()) / b.size.height } else { (ev.position.x - b.left()) / b.size.width };
                g.tile.set_ratio(&path, r);
                cx.notify();
            }
            return;
        }
        if !(self.selecting && ev.pressed_button == Some(MouseButton::Left)) {
            self.selecting = false;
            return;
        }
        let ix = self.active;
        if let Some(s) = self.sessions.get_mut(ix) {
            // Output may have erased the selection mid-drag.
            s.restore_selection();
            let s = &self.sessions[ix];
            let mut t = s.term.lock();
            let off = t.grid().display_offset();
            let (p, side) = self.grid_point(ev.position, off);
            let p = TPoint::new(p.line, p.column.min(t.last_column()));
            if let Some(sel) = t.selection.as_mut() {
                sel.update(p, side);
            }
            drop(t);
            self.sessions[ix].remember_selection();
        }
        cx.notify();
    }

    /// Trackpads send a few pixels per event, so whole lines are taken from a
    /// running total. Programs that ask for the mouse get wheel reports,
    /// full-screen ones (alt screen) get arrow keys, else the scrollback moves.
    /// Shift always scrolls the scrollback.
    fn on_scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lh = self.cell.1;
        match ev.touch_phase {
            TouchPhase::Started => self.scroll_px = px(0.),
            TouchPhase::Moved => {}
            TouchPhase::Ended | TouchPhase::Cancelled => return,
        }
        let before = (self.scroll_px / lh) as i32;
        self.scroll_px += ev.delta.pixel_delta(lh).y;
        let lines = (self.scroll_px / lh) as i32 - before;
        // Keep the remainder small so a change of direction answers at once.
        self.scroll_px %= lh;
        if lines == 0 {
            return;
        }
        // The split pane under the pointer, else the focused one.
        let under = self.pane_at(ev.position).and_then(|id| self.session_by_id(id));
        let Some(s) = under.or_else(|| self.active_session()) else { return };
        let mut t = s.term.lock();
        let mode = *t.mode();
        let bytes = if mode.intersects(TermMode::MOUSE_MODE) && !ev.modifiers.shift {
            let (p, _) = self.grid_point(ev.position, 0);
            let (col, row) = (p.column.0.min(t.columns().saturating_sub(1)) + 1, (p.line.0.max(0) as usize).min(t.screen_lines().saturating_sub(1)) + 1);
            let button = if lines > 0 { 64 } else { 65 };
            let report = if mode.contains(TermMode::SGR_MOUSE) {
                format!("\x1b[<{button};{col};{row}M").into_bytes()
            } else {
                let enc = |v: usize| (32 + v).min(255) as u8;
                vec![0x1b, b'[', b'M', 32 + button, enc(col), enc(row)]
            };
            report.repeat(lines.unsigned_abs() as usize)
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) && !ev.modifiers.shift {
            let key: &[u8] = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
            key.repeat(lines.unsigned_abs() as usize)
        } else {
            t.scroll_display(Scroll::Delta(lines));
            drop(t);
            return cx.notify();
        };
        drop(t);
        s.write(bytes);
    }
}

/// Non-text keys -> bytes. Printable text comes through the input handler.
fn key_bytes(ks: &gpui::Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    if m.platform {
        return None;
    }
    let key = ks.key.as_str();
    let arrow = |c: u8| {
        if m.shift || m.control || m.alt {
            None
        } else if app_cursor {
            Some(vec![0x1b, b'O', c])
        } else {
            Some(vec![0x1b, b'[', c])
        }
    };
    if m.alt && !m.control {
        return match key {
            "left" => Some(b"\x1bb".to_vec()),
            "right" => Some(b"\x1bf".to_vec()),
            "backspace" => Some(b"\x1b\x7f".to_vec()),
            "enter" => Some(b"\x1b\r".to_vec()),
            _ => None,
        };
    }
    if m.control {
        let b = match key {
            k if k.len() == 1 && k.as_bytes()[0].is_ascii_lowercase() => k.as_bytes()[0] & 0x1f,
            "space" | "@" | "2" => 0,
            "[" => 0x1b,
            "\\" => 0x1c,
            "]" => 0x1d,
            "6" => 0x1e,
            "-" | "_" => 0x1f,
            _ => return None,
        };
        return Some(vec![b]);
    }
    Some(match key {
        // ESC CR: what Claude Code, Codex and zsh read as "insert newline".
        "enter" if m.shift => b"\x1b\r".to_vec(),
        "enter" => vec![b'\r'],
        "backspace" => vec![0x7f],
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => vec![b'\t'],
        "escape" => vec![0x1b],
        "up" => return arrow(b'A'),
        "down" => return arrow(b'B'),
        "right" => return arrow(b'C'),
        "left" => return arrow(b'D'),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        "delete" => b"\x1b[3~".to_vec(),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        _ => return None,
    })
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TerminalPanel {
    fn text_for_range(&mut self, _: Range<usize>, _: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        None
    }
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: 0..0, reversed: false })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}
    fn replace_text_in_range(&mut self, _: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.sessions.get_mut(self.active) {
            s.forget_selection();
        }
        // macOS can deliver Shift+Tab as its raw character (U+0019).
        let bytes = if text == "\u{19}" { b"\x1b[Z".to_vec() } else { text.as_bytes().to_vec() };
        self.write_active(bytes, cx);
    }
    fn replace_and_mark_text_in_range(&mut self, _: Option<Range<usize>>, _: &str, _: Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) {
        // IME composition isn't shown; the committed text arrives via replace_text_in_range.
    }
    fn bounds_for_range(&mut self, _: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }
    fn character_index_for_point(&mut self, _: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

// ---------------------------------------------------------------------------
// Grid element
// ---------------------------------------------------------------------------

struct GridElement {
    panel: Entity<TerminalPanel>,
    /// The session this pane shows.
    id: usize,
}

struct GridRow {
    y: Pixels,
    text: ShapedLine,
    bgs: Vec<(usize, usize, u32)>,
    /// Block and box-drawing characters, drawn as shapes (column, shape, color).
    shapes: Vec<(usize, Shape, u32)>,
}

struct GridPrepaint {
    /// Where the grid's first row is: the pane's top, or above it while zen
    /// holds the grid's size (`TerminalPanel::zen_hold`).
    top: Pixels,
    rows: Vec<GridRow>,
    cursor: Option<(usize, usize, CursorShape)>,
    has_content: bool,
}

impl IntoElement for GridElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for GridElement {
    type RequestLayoutState = ();
    type PrepaintState = GridPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> GridPrepaint {
        let look = Look::of(window, cx);
        let tc = look.theme.terminal.clone();
        let fs = look.z(look.code_size - 1.);
        let f = font(look.code_font.clone());
        let ts = window.text_system().clone();
        let cw = ts.advance(ts.resolve_font(&f), fs, 'm').map(|s| s.width).unwrap_or(px(7.));
        let lh = look.z(((look.code_size - 1.) * 1.3).round());
        let id = self.id;
        // Zen's bar out: the size from before it came, bottom edges together.
        let grid = self.panel.update(cx, |p, _| match p.rest_size.get(&id).copied() {
            Some((w, h)) if p.zen_hold.get() && w == bounds.size.width && h > bounds.size.height => {
                Bounds::new(point(bounds.left(), bounds.bottom() - h), size(w, h))
            }
            _ => {
                if !p.zen_hold.get() {
                    p.rest_size.insert(id, (bounds.size.width, bounds.size.height));
                }
                bounds
            }
        });
        let cols = ((grid.size.width / cw).floor() as usize).max(2);
        let lines = ((grid.size.height / lh).floor() as usize).max(1);
        let mut out = GridPrepaint { top: grid.top(), rows: Vec::new(), cursor: None, has_content: false };
        self.panel.update(cx, |p, _| {
            p.cell = (cw, lh);
            p.pane_bounds.insert(id, bounds);
            p.grid_shift.insert(id, bounds.top() - grid.top());
            crate::script::record(|| format!("term:{id}"), bounds);
            crate::script::record(|| format!("term-cell:{id}"), Bounds::new(bounds.origin, gpui::size(cw, lh)));
            let Some(s) = p.sessions.iter_mut().find(|s| s.id == id) else { return };
            s.restore_selection();
            if s.size != (cols, lines) {
                // New columns rewrap the lines: the old selection means nothing.
                if s.size.0 != cols {
                    s.kept = None;
                }
                s.size = (cols, lines);
                s.term.lock().resize(Dims { cols, lines });
                let _ = s.sender.send(Msg::Resize(WindowSize {
                    num_lines: lines as u16,
                    num_cols: cols as u16,
                    cell_width: f32::from(cw) as u16,
                    cell_height: f32::from(lh) as u16,
                }));
            }
        });
        let p = self.panel.read(cx);
        let Some(s) = p.session_by_id(id) else { return out };
        let term = s.term.lock();
        let content = term.renderable_content();
        let off = content.display_offset as i32;
        let sel = content.selection;
        let n_lines = term.screen_lines();
        let mut text: Vec<String> = vec![String::new(); n_lines];
        let mut runs: Vec<Vec<TextRun>> = vec![Vec::new(); n_lines];
        let mut bgs: Vec<Vec<(usize, usize, u32)>> = vec![Vec::new(); n_lines];
        let mut shapes: Vec<Vec<(usize, Shape, u32)>> = vec![Vec::new(); n_lines];
        let mk = |len: usize, color: u32, bold: bool| TextRun {
            len,
            font: if bold { gpui::Font { weight: gpui::FontWeight::BOLD, ..f.clone() } } else { f.clone() },
            color: hex(color),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        for ic in content.display_iter {
            let row = ic.point.line.0 + off;
            if row < 0 || row as usize >= n_lines {
                continue;
            }
            let row = row as usize;
            let cell = ic.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let col = ic.point.column.0;
            let dim = cell.flags.contains(Flags::DIM);
            let mut fg = resolve(cell.fg, &term, false, dim, &tc).unwrap_or(rgb(tc.foreground));
            let mut bg = resolve(cell.bg, &term, true, false, &tc);
            if cell.flags.contains(Flags::INVERSE) {
                let old_bg = bg.unwrap_or(rgb(tc.background));
                bg = Some(fg);
                fg = old_bg;
            }
            if sel.is_some_and(|r| r.contains(ic.point)) {
                bg = Some(rgb(tc.selection));
            }
            if let Some(b) = bg {
                match bgs[row].last_mut() {
                    Some((_, end, c)) if *end == col && *c == b => *end = col + 1,
                    _ => bgs[row].push((col, col + 1, b)),
                }
            }
            let mut ch = if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c };
            // Drawn edge to edge instead of with the font, whose glyphs don't
            // fill the taller line (so logos and boxes had gaps).
            if let Some(shape) = shape_of(ch) {
                shapes[row].push((col, shape, fg));
                ch = ' ';
                out.has_content = true;
            }
            if ch != ' ' {
                out.has_content = true;
            }
            // Pad skipped columns (after wide chars) to keep the grid aligned.
            let bold = cell.flags.contains(Flags::BOLD);
            let len = ch.len_utf8();
            text[row].push(ch);
            match runs[row].last_mut() {
                Some(r) if r.color == hex(fg) && (r.font.weight == gpui::FontWeight::BOLD) == bold => r.len += len,
                _ => runs[row].push(mk(len, fg, bold)),
            }
        }
        let cur = content.cursor;
        if off == 0 && cur.shape != CursorShape::Hidden {
            let r = cur.point.line.0;
            if r >= 0 && (r as usize) < n_lines {
                out.cursor = Some((r as usize, cur.point.column.0, cur.shape));
            }
        }
        drop(term);
        for (i, (t, r)) in text.into_iter().zip(runs).enumerate() {
            let shaped = ts.shape_line(SharedString::from(t), fs, &r, None);
            out.rows.push(GridRow { y: grid.top() + lh * i as f32, text: shaped, bgs: std::mem::take(&mut bgs[i]), shapes: std::mem::take(&mut shapes[i]) });
        }
        out
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), pre: &mut GridPrepaint, window: &mut Window, cx: &mut App) {
        let cursor_color = Look::of(window, cx).theme.terminal.cursor;
        let p = self.panel.read(cx);
        let focus = p.focus.clone();
        let mine = p.active_session().is_some_and(|s| s.id == self.id);
        let focused = mine && focus.is_focused(window);
        let (cw, lh) = p.cell;
        if mine {
            window.handle_input(&focus, ElementInputHandler::new(bounds, self.panel.clone()), cx);
        }
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for row in &pre.rows {
                for &(a, b, c) in &row.bgs {
                    window.paint_quad(fill(
                        Bounds::new(point(bounds.left() + cw * a as f32, row.y), size(cw * (b - a) as f32, lh)),
                        hex(c),
                    ));
                }
                for &(col, shape, c) in &row.shapes {
                    let cell = Bounds::new(point(bounds.left() + cw * col as f32, row.y), size(cw, lh));
                    for (r, alpha) in shape.rects(cell) {
                        let mut color = hex(c);
                        color.a *= alpha;
                        window.paint_quad(fill(r, color));
                    }
                }
                row.text.paint(point(bounds.left(), row.y), lh, gpui::TextAlign::Left, None, window, cx).ok();
            }
            if let Some((r, c, shape)) = pre.cursor {
                let origin = point(bounds.left() + cw * c as f32, pre.top + lh * r as f32);
                let q = match (focused, shape) {
                    (true, CursorShape::Beam) => fill(Bounds::new(origin, size(px(2.), lh)), crate::look::hsla(cursor_color)),
                    (true, CursorShape::Underline) => fill(Bounds::new(point(origin.x, origin.y + lh - px(2.)), size(cw, px(2.))), crate::look::hsla(cursor_color)),
                    (true, _) => {
                        let mut h = crate::look::hsla(cursor_color);
                        h.a = 0.55;
                        fill(Bounds::new(origin, size(cw, lh)), h)
                    }
                    (false, _) => gpui::outline(Bounds::new(origin, size(cw, lh)), crate::look::hsla(cursor_color), gpui::BorderStyle::Solid),
                };
                window.paint_quad(q);
            }
        });
        let _ = pre.has_content;
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let split = self.active_group().is_some();
        let tiles = match self.active_group().map(|g| g.tile.clone()) {
            Some(t) => Some(self.gliding(&t, true, &look, cx)),
            None => self.active_session().map(|s| s.id).map(|id| self.gliding(&Tile::Term(id), false, &look, cx)),
        };
        let tiled = look.tiles.is_some() && self.standalone.get();
        let message = self.message.clone().map(|m| {
            let d = if tiled { look.tile(div(), ui.panel_bg) } else { div() };
            d.flex_none().px(look.z(8.)).py(look.z(4.)).text_size(look.text(-1.)).text_color(look.c(ui.text_muted)).child(m)
        });
        div()
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .when(tiled, |d| d.gap(look.gap()))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end))
            .on_action(cx.listener(Self::kill_line))
            .on_action(cx.listener(Self::back_tab))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::close_terminal))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::focus_left))
            .on_action(cx.listener(Self::focus_right))
            .on_action(cx.listener(Self::focus_up))
            .on_action(cx.listener(Self::focus_down))
            .children(message)
            .children(self.find.as_ref().map(|f| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .when(!tiled, |d| d.mx(look.z(8.)).mt(look.z(4.)))
                    .px(look.z(8.))
                    .py(look.z(3.))
                    .rounded(look.z(4.))
                    .border_1()
                    .border_color(look.c(if f.error { ui.error } else { ui.border }))
                    .bg(look.c(ui.overlay_bg))
                    .text_size(look.text(-1.))
                    .child(div().flex_1().min_w(px(0.)).child(f.input.clone()))
                    .when(f.error, |d| d.child(div().flex_none().text_color(look.c(ui.text_muted)).child("no match")))
            }))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    // Split panes float as tiles with a gap all round (Hyprland).
                    // (With window gaps the stage's own gaps do that.)
                    .when(split && !tiled, |d| d.p(look.z(6.)))
                    // Files dragged in from Finder become their paths, as in Terminal.
                    .drag_over::<gpui::ExternalPaths>(|s, _, window, cx| {
                        let l = Look::of(window, cx);
                        let mut c = l.c(l.theme.ui.focus_ring);
                        c.a = 0.12;
                        s.bg(c)
                    })
                    .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                        window.focus(&this.focus, cx);
                        this.paste_paths(paths.paths(), cx);
                    }))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.resizing = None))
                    .on_scroll(cx.listener(Self::on_scroll))
                    .children(tiles),
            )
    }
}

impl TerminalPanel {
    /// The panes, gliding to their places when the split changes (Cmd+D's
    /// dwindle, Cmd+Enter's grid, a pane closed): drawn at absolute
    /// rectangles until they settle, as the dock does (decision 60).
    fn gliding(&mut self, t: &Tile, split: bool, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        fn shape(t: &Tile) -> tiling::Shape<usize> {
            match t {
                Tile::Term(id) => tiling::Shape::Leaf(*id),
                Tile::Split { vertical, ratio, a, b } => tiling::Shape::Split { vertical: *vertical, ratio: *ratio, a: Box::new(shape(a)), b: Box::new(shape(b)) },
            }
        }
        let s = shape(t);
        let a = self.glide_area.get();
        let gap = f32::from(if look.tiles.is_some() && self.standalone.get() { look.gap() } else { look.z(6.) });
        let leaves = tiling::lay(&s, Rect { x: 0., y: 0., w: f32::from(a.size.width), h: f32::from(a.size.height) }, gap);
        let (rects, moving) = self.glide.step(&s.signature(), &leaves);
        let area = self.glide_area.clone();
        let measure = gpui::canvas(move |b, _, _| area.set(b), |_, _, _, _| {}).absolute().size_full();
        if moving {
            tiling::next_frame(cx);
        }
        let body = if moving {
            div()
                .relative()
                .size_full()
                .children(leaves.iter().map(|l| {
                    let r = rects.get(&l.key).copied().unwrap_or(l.rect);
                    div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).overflow_hidden().child(self.tile(&Tile::Term(l.key), l.path.clone(), split, look, cx))
                }))
                .into_any_element()
        } else {
            self.tile(t, Vec::new(), split, look, cx)
        };
        div().relative().size_full().child(measure).child(body).into_any_element()
    }

    /// One pane per session in the tree, with draggable dividers between.
    fn tile(&self, t: &Tile, path: Vec<bool>, split: bool, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        match t {
            Tile::Term(id) => {
                let on = split && self.active_session().is_some_and(|s| s.id == *id);
                let tiled = look.tiles.is_some() && self.standalone.get();
                // With window gaps every pane is a tile with its own background.
                let d = if tiled { look.tile(div(), ui.panel_bg) } else { div() };
                let edge = if on {
                    look.c(look.theme.accents[0].base)
                } else if !tiled || look.tiles.is_some_and(|t| t.border) {
                    look.c(ui.border)
                } else {
                    gpui::transparent_black()
                };
                d.size_full()
                    .px(look.z(8.))
                    .py(look.z(4.))
                    // Tiling-WM look: each pane its own rounded box, the
                    // focused one outlined in the accent.
                    .when(split && !tiled, |d| d.rounded(look.z(6.)))
                    .when(split || tiled && look.tiles.is_some_and(|t| t.border), |d| d.border_1().border_color(edge))
                    .child(GridElement { panel: cx.entity(), id: *id })
                    .into_any_element()
            }
            Tile::Split { vertical, ratio, a, b } => {
                let (mut pa, mut pb) = (path.clone(), path.clone());
                pa.push(false);
                pb.push(true);
                let first = self.tile(a, pa, split, look, cx);
                let second = self.tile(b, pb, split, look, cx);
                let (sb, p2) = (self.split_bounds.clone(), path.clone());
                let measure = {
                    let (sb, path) = (sb.clone(), path.clone());
                    gpui::canvas(move |b, _, _| { sb.borrow_mut().insert(path.clone(), b); }, |_, _, _, _| {}).absolute().size_full()
                };
                let w = if look.tiles.is_some() && self.standalone.get() { look.gap() } else { look.z(6.) };
                let handle = div()
                    .id(SharedString::from(format!("term-split-{path:?}")))
                    .group(SharedString::from(format!("term-split-{path:?}")))
                    .flex_none()
                    .relative()
                    // The gap between tiles; the accent shows while hovered.
                    .when(*vertical, |d| d.h(w).w_full().cursor_row_resize())
                    .when(!*vertical, |d| d.w(w).h_full().cursor_col_resize())
                    .child(
                        div()
                            .absolute()
                            .when(*vertical, |d| d.left_0().right_0().top((w - px(1.)) / 2.).h(px(1.)))
                            .when(!*vertical, |d| d.top_0().bottom_0().left((w - px(1.)) / 2.).w(px(1.)))
                            .group_hover(SharedString::from(format!("term-split-{path:?}")), |d| d.bg(look.c(look.theme.accents[0].base))),
                    )
                    // A wider grab area than the gap.
                    .child(div().absolute().when(*vertical, |d| d.left_0().right_0().top(px(-3.)).h(px(7.))).when(!*vertical, |d| d.top_0().bottom_0().left(px(-3.)).w(px(7.))))
                    .children(crate::script::mark(|| format!("term-split:{}", if path.is_empty() { "root".to_string() } else { path.iter().map(|&b| if b { 'b' } else { 'a' }).collect() })))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        if let Some(b) = sb.borrow().get(&p2).copied() {
                            this.resizing = Some((p2.clone(), b));
                        }
                        cx.stop_propagation();
                    }));
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .when(*vertical, |d| d.flex_col())
                    .child(measure)
                    .child(div().flex_none().overflow_hidden().when(*vertical, |d| d.w_full().h(relative(*ratio))).when(!*vertical, |d| d.h_full().w(relative(*ratio))).child(first))
                    .child(handle)
                    .child(div().flex_1().min_w(px(0.)).min_h(px(0.)).overflow_hidden().child(second))
                    .into_any_element()
            }
        }
    }
}

/// Find in Terminal: the next match before (`Left`, older) or after
/// (`Right`, newer) the current one; the first search starts at the bottom.
fn search_from<T: EventListener>(t: &Term<T>, regex: &mut RegexSearch, last: Option<&Match>, dir: Direction) -> Option<Match> {
    let origin = match (last, dir) {
        (Some(m), Direction::Left) => *m.start(),
        (Some(m), Direction::Right) => *m.end(),
        (None, _) => TPoint::new(Line(t.screen_lines() as i32 - 1), t.last_column()),
    };
    let side = if dir == Direction::Left { Side::Left } else { Side::Right };
    let m = t.search_next(regex, origin, dir, side, None)?;
    // Searching from a match's own edge finds it again: step past it.
    if last.is_some_and(|l| l == &m) {
        let next = if dir == Direction::Left { m.start().sub(t, alacritty_terminal::index::Boundary::Grid, 1) } else { m.end().add(t, alacritty_terminal::index::Boundary::Grid, 1) };
        return t.search_next(regex, next, dir, side, None);
    }
    Some(m)
}

/// A block element or box-drawing character, drawn as rectangles.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    /// Filled parts of the cell, in cell fractions (x0, y0, x1, y1), and alpha.
    Block(&'static [(f32, f32, f32, f32)], f32),
    /// Lines from the center to the left, right, top and bottom edges
    /// (0 none, 1 light, 2 heavy).
    Lines { left: u8, right: u8, up: u8, down: u8 },
}

impl Shape {
    fn rects(self, cell: Bounds<Pixels>) -> Vec<(Bounds<Pixels>, f32)> {
        let (x, y, w, h) = (cell.left(), cell.top(), cell.size.width, cell.size.height);
        match self {
            Shape::Block(parts, alpha) => parts
                .iter()
                .map(|&(x0, y0, x1, y1)| {
                    // Round to whole pixels so neighbours meet without seams.
                    let l = (x + w * x0).round();
                    let t = (y + h * y0).round();
                    let r = (x + w * x1).round();
                    let b = (y + h * y1).round();
                    (Bounds::new(point(l, t), size(r - l, b - t)), alpha)
                })
                .collect(),
            Shape::Lines { left, right, up, down } => {
                let thick = |weight: u8| if weight == 2 { (w * 0.22).max(px(2.)).round() } else { (w * 0.11).max(px(1.)).round() };
                let cx = (x + w / 2.).round();
                let cy = (y + h / 2.).round();
                let mut out = Vec::new();
                let horiz = left.max(right).max(1);
                let vert = up.max(down).max(1);
                let (th, tv) = (thick(horiz), thick(vert));
                // Arms overlap the center by half a line so corners close.
                if left > 0 {
                    out.push((Bounds::new(point(x, cy - th / 2.), size(cx - x + tv / 2., th)), 1.0));
                }
                if right > 0 {
                    out.push((Bounds::new(point(cx - tv / 2., cy - th / 2.), size(x + w - cx + tv / 2., th)), 1.0));
                }
                if up > 0 {
                    out.push((Bounds::new(point(cx - tv / 2., y), size(tv, cy - y + th / 2.)), 1.0));
                }
                if down > 0 {
                    out.push((Bounds::new(point(cx - tv / 2., cy - th / 2.), size(tv, y + h - cy + th / 2.)), 1.0));
                }
                out
            }
        }
    }
}

fn shape_of(c: char) -> Option<Shape> {
    const H: f32 = 0.5;
    const UL: (f32, f32, f32, f32) = (0., 0., H, H);
    const UR: (f32, f32, f32, f32) = (H, 0., 1., H);
    const LL: (f32, f32, f32, f32) = (0., H, H, 1.);
    const LR: (f32, f32, f32, f32) = (H, H, 1., 1.);
    let b = |parts: &'static [(f32, f32, f32, f32)]| Some(Shape::Block(parts, 1.0));
    let l = |left: u8, right: u8, up: u8, down: u8| Some(Shape::Lines { left, right, up, down });
    match c {
        '▀' => b(&[(0., 0., 1., H)]),
        '▁' => b(&[(0., 0.875, 1., 1.)]),
        '▂' => b(&[(0., 0.75, 1., 1.)]),
        '▃' => b(&[(0., 0.625, 1., 1.)]),
        '▄' => b(&[(0., H, 1., 1.)]),
        '▅' => b(&[(0., 0.375, 1., 1.)]),
        '▆' => b(&[(0., 0.25, 1., 1.)]),
        '▇' => b(&[(0., 0.125, 1., 1.)]),
        '█' => b(&[(0., 0., 1., 1.)]),
        '▉' => b(&[(0., 0., 0.875, 1.)]),
        '▊' => b(&[(0., 0., 0.75, 1.)]),
        '▋' => b(&[(0., 0., 0.625, 1.)]),
        '▌' => b(&[(0., 0., H, 1.)]),
        '▍' => b(&[(0., 0., 0.375, 1.)]),
        '▎' => b(&[(0., 0., 0.25, 1.)]),
        '▏' => b(&[(0., 0., 0.125, 1.)]),
        '▐' => b(&[(H, 0., 1., 1.)]),
        '░' => Some(Shape::Block(&[(0., 0., 1., 1.)], 0.25)),
        '▒' => Some(Shape::Block(&[(0., 0., 1., 1.)], 0.5)),
        '▓' => Some(Shape::Block(&[(0., 0., 1., 1.)], 0.75)),
        '▔' => b(&[(0., 0., 1., 0.125)]),
        '▕' => b(&[(0.875, 0., 1., 1.)]),
        '▖' => b(&[LL]),
        '▗' => b(&[LR]),
        '▘' => b(&[UL]),
        '▙' => b(&[UL, LL, LR]),
        '▚' => b(&[UL, LR]),
        '▛' => b(&[UL, UR, LL]),
        '▜' => b(&[UL, UR, LR]),
        '▝' => b(&[UR]),
        '▞' => b(&[UR, LL]),
        '▟' => b(&[UR, LL, LR]),
        '─' => l(1, 1, 0, 0),
        '━' => l(2, 2, 0, 0),
        '│' => l(0, 0, 1, 1),
        '┃' => l(0, 0, 2, 2),
        '┌' | '╭' => l(0, 1, 0, 1),
        '┐' | '╮' => l(1, 0, 0, 1),
        '└' | '╰' => l(0, 1, 1, 0),
        '┘' | '╯' => l(1, 0, 1, 0),
        '├' => l(0, 1, 1, 1),
        '┤' => l(1, 0, 1, 1),
        '┬' => l(1, 1, 0, 1),
        '┴' => l(1, 1, 1, 0),
        '┼' => l(1, 1, 1, 1),
        '┏' => l(0, 2, 0, 2),
        '┓' => l(2, 0, 0, 2),
        '┗' => l(0, 2, 2, 0),
        '┛' => l(2, 0, 2, 0),
        '┣' => l(0, 2, 2, 2),
        '┫' => l(2, 0, 2, 2),
        '┳' => l(2, 2, 0, 2),
        '┻' => l(2, 2, 2, 0),
        '╋' => l(2, 2, 2, 2),
        '╴' => l(1, 0, 0, 0),
        '╶' => l(0, 1, 0, 0),
        '╵' => l(0, 0, 1, 0),
        '╷' => l(0, 0, 0, 1),
        _ => None,
    }
}

/// `base`, or `base 2`, `base 3`… : the first one not in `taken`.
fn free_label(base: &str, taken: &[&str]) -> String {
    (1..).map(|n| if n == 1 { base.to_string() } else { format!("{base} {n}") }).find(|l| !taken.contains(&l.as_str())).unwrap_or_default()
}

/// The program in the foreground of the shell `pid`'s terminal (its name,
/// like `cargo`), or None while the shell itself is at its prompt.
fn foreground_name(pid: i32) -> Option<String> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let got = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    let fg = info.e_tpgid as i32;
    if got != size || fg <= 0 || fg as u32 == info.pbi_pgid {
        return None;
    }
    let mut buf = [0u8; 256];
    let n = unsafe { libc::proc_name(fg, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

/// The folder process `pid` is in (a shell's, after its last `cd`).
pub fn cwd_of(pid: i32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
    let got = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&raw mut info).cast(), size) };
    if got != size {
        return None;
    }
    let path = unsafe { std::ffi::CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
    let path = std::ffi::OsStr::from_bytes(path.to_bytes());
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// A terminal title as a task name: without spinner/status glyphs, and None
/// for generic titles (the program's name, a shell, a path, user@host).
fn clean_title(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.starts_with('/') || raw.starts_with('~') || (raw.contains('@') && raw.contains(':')) {
        return None;
    }
    let t = raw.trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
    let lower = t.to_lowercase();
    let generic = ["claude", "claude code", "zsh", "bash", "fish", "sh", "aider", "-zsh", "login"];
    if t.is_empty() || generic.contains(&lower.as_str()) {
        return None;
    }
    let mut s: String = t.chars().take(80).collect();
    if t.chars().count() > 80 {
        s.push('…');
    }
    Some(s)
}

/// What Cmd-click opens.
#[derive(Debug, PartialEq)]
enum Link {
    Url(String),
    /// Path as printed, plus line and column (1-based) when given.
    Path(String, Option<(u32, u32)>),
}

/// The whitespace-free word at `col` in a terminal line, without wrapping
/// quotes/brackets or trailing punctuation.
fn link_at(text: &str, col: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let stop = |c: char| c.is_whitespace() || "\"'`()[]{}<>|".contains(c);
    if col >= chars.len() || stop(chars[col]) {
        return None;
    }
    let mut a = col;
    while a > 0 && !stop(chars[a - 1]) {
        a -= 1;
    }
    let mut b = col;
    while b + 1 < chars.len() && !stop(chars[b + 1]) {
        b += 1;
    }
    let tok: String = chars[a..=b].iter().collect();
    let tok = tok.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    (!tok.is_empty()).then(|| tok.to_string())
}

fn classify_link(tok: &str) -> Option<Link> {
    if tok.starts_with("http://") || tok.starts_with("https://") {
        return Some(Link::Url(tok.to_string()));
    }
    let mut parts = tok.splitn(3, ':');
    let path = parts.next()?;
    let line = parts.next().and_then(|l| l.parse::<u32>().ok());
    let col = parts.next().and_then(|c| c.split(|ch: char| !ch.is_ascii_digit()).next()?.parse::<u32>().ok());
    // Something path-like: a separator or a file extension.
    let looks = path.contains('/') || path.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty() && ext.len() <= 8);
    if !looks {
        return None;
    }
    Some(Link::Path(path.to_string(), line.map(|l| (l, col.unwrap_or(1)))))
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@+%,".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shift_enter_is_newline() {
        let k = |s| super::key_bytes(&gpui::Keystroke::parse(s).unwrap(), false);
        assert_eq!(k("enter"), Some(b"\r".to_vec()));
        assert_eq!(k("shift-enter"), Some(b"\x1b\r".to_vec()));
        assert_eq!(k("alt-enter"), Some(b"\x1b\r".to_vec()));
    }

    use super::*;

    #[test]
    fn color_queries_answer_with_each_panels_own_theme() {
        // Two windows' panels on themes with different backgrounds.
        let themes = qur_theme::Registry::global().all();
        let light = themes.iter().find(|t| t.appearance == qur_theme::Appearance::Light).expect("a light theme").terminal.clone();
        let dark = themes.iter().find(|t| t.appearance == qur_theme::Appearance::Dark).expect("a dark theme").terminal.clone();
        assert_ne!(rgb(light.background), rgb(dark.background));
        let listener = |colors: &ColorsCell| Listener(unbounded().0, TitleCell::default(), Reply::default(), colors.clone());
        let (a, b) = (ColorsCell::default(), ColorsCell::default());
        let (la, lb) = (listener(&a), listener(&b));
        // Nothing to answer before the panel has its window's colors.
        assert!(la.color_answer(257).is_none());
        *a.lock().unwrap() = Some(light.clone());
        *b.lock().unwrap() = Some(dark.clone());
        let hex = |c: alacritty_terminal::vte::ansi::Rgb| ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32;
        // OSC 11 (background), OSC 10 (foreground), OSC 4;1 (red), however the last drawn window looks.
        assert_eq!(la.color_answer(257).map(hex), Some(rgb(light.background)));
        assert_eq!(lb.color_answer(257).map(hex), Some(rgb(dark.background)));
        assert_eq!(la.color_answer(256).map(hex), Some(rgb(light.foreground)));
        assert_eq!(lb.color_answer(256).map(hex), Some(rgb(dark.foreground)));
        assert_eq!(la.color_answer(1).map(hex), Some(indexed(1, &light)));
        assert_eq!(lb.color_answer(1).map(hex), Some(indexed(1, &dark)));
    }

    #[test]
    fn selection_survives_redraws_and_scrolling() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::term::test::TermSize;
        let mut t = Term::new(TermConfig::default(), &TermSize::new(20, 4), VoidListener);
        let mut parser: alacritty_terminal::vte::ansi::Processor = alacritty_terminal::vte::ansi::Processor::new();
        parser.advance(&mut t, b"alpha\r\nbeta\r\n> prompt");
        let mut sel = Selection::new(SelectionType::Simple, TPoint::new(Line(1), Column(0)), Side::Left);
        sel.update(TPoint::new(Line(2), Column(7)), Side::Right);
        t.selection = Some(sel);
        let kept = (t.selection.clone().unwrap(), t.history_size());
        // A redraw like Claude Code's: back up and erase below, then repaint.
        parser.advance(&mut t, b"\x1b[1A\r\x1b[Jbeta\r\n> prompt");
        assert!(t.selection.is_none(), "the terminal drops it");
        let kept = restore_selection(&mut t, kept).unwrap();
        assert_eq!(t.selection_to_string().as_deref(), Some("beta\n> prompt"));
        // Two lines scroll into the scrollback; the selection moves with them.
        parser.advance(&mut t, b"\r\nmore\r\nmore");
        t.selection = None;
        restore_selection(&mut t, kept).unwrap();
        assert_eq!(t.selection_to_string().as_deref(), Some("beta\n> prompt"));
    }

    #[test]
    fn find_in_terminal_walks_matches() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::term::test::TermSize;
        let mut t = Term::new(TermConfig::default(), &TermSize::new(40, 5), VoidListener);
        let mut parser: alacritty_terminal::vte::ansi::Processor = alacritty_terminal::vte::ansi::Processor::new();
        // 8 lines on a 5-line screen: the first ones are in the scrollback.
        parser.advance(&mut t, b"error one\r\nok\r\nok\r\nerror two\r\nok\r\nok\r\nerror three\r\nok");
        let mut re = RegexSearch::new("error \\w+").unwrap();
        let text = |t: &Term<VoidListener>, m: &Match| t.bounds_to_string(*m.start(), *m.end());
        let a = search_from(&t, &mut re, None, Direction::Left).unwrap();
        assert_eq!(text(&t, &a), "error three");
        let b = search_from(&t, &mut re, Some(&a), Direction::Left).unwrap();
        assert_eq!(text(&t, &b), "error two");
        let c = search_from(&t, &mut re, Some(&b), Direction::Left).unwrap();
        assert_eq!(text(&t, &c), "error one", "reaches the scrollback");
        let d = search_from(&t, &mut re, Some(&c), Direction::Right).unwrap();
        assert_eq!(text(&t, &d), "error two", "and back towards newer");
    }

    #[test]
    fn terminal_titles_as_task_names() {
        assert_eq!(clean_title("✳ Fix login redirect").as_deref(), Some("Fix login redirect"));
        assert_eq!(clean_title("⠂ Add dark mode toggle").as_deref(), Some("Add dark mode toggle"));
        assert_eq!(clean_title("✳ Claude Code"), None);
        assert_eq!(clean_title("zsh"), None);
    }

    #[test]
    fn shell_labels_reuse_the_lowest_free_number() {
        assert_eq!(free_label("zsh", &[]), "zsh");
        assert_eq!(free_label("zsh", &["zsh"]), "zsh 2");
        // "zsh" closed while "zsh 2" stays open: the next one is "zsh" again, not a second "zsh 2".
        assert_eq!(free_label("zsh", &["zsh 2"]), "zsh");
        assert_eq!(free_label("zsh", &["zsh", "zsh 2", "zsh 4"]), "zsh 3");
    }

    #[test]
    fn foreground_name_is_none_for_a_missing_process() {
        assert_eq!(foreground_name(-1), None);
        assert_eq!(clean_title("yobi@mac:~/qur"), None);
        assert_eq!(clean_title("~/qur"), None);
        assert_eq!(clean_title("   "), None);
    }

    #[test]
    fn grid_keeps_panes_near_square() {
        // Rows of Term ids, read back from a grid.
        fn rows(t: &Tile) -> Vec<Vec<usize>> {
            match t {
                Tile::Split { vertical: true, a, b, .. } => [rows(a), rows(b)].concat(),
                t => vec![t.ids()],
            }
        }
        // A wide panel: two side by side, then a 2×2.
        assert_eq!(rows(&Tile::grid(&[1, 2], 1600., 800.)), vec![vec![1, 2]]);
        assert_eq!(rows(&Tile::grid(&[1, 2, 3, 4], 1600., 900.)), vec![vec![1, 2], vec![3, 4]]);
        // Five: three on top, two below, never five thin columns.
        assert_eq!(rows(&Tile::grid(&[1, 2, 3, 4, 5], 1600., 900.)), vec![vec![1, 2, 3], vec![4, 5]]);
        // A tall panel stacks them.
        assert_eq!(rows(&Tile::grid(&[1, 2], 600., 1200.)), vec![vec![1], vec![2]]);
        // Shares are even: a row of three gives the first a third.
        match Tile::grid(&[1, 2, 3], 3000., 1000.) {
            Tile::Split { vertical: false, ratio, .. } => assert!((ratio - 1. / 3.).abs() < 1e-6),
            t => panic!("{t:?}"),
        }
    }

    #[test]
    fn block_and_box_shapes_fill_the_cell() {
        let cell = Bounds::new(point(px(10.), px(20.)), size(px(8.), px(17.)));
        // Full block: the whole cell, so rows and columns meet.
        assert_eq!(shape_of('█').unwrap().rects(cell), vec![(cell, 1.0)]);
        // ▛ = three quadrants that tile without gaps.
        let r = shape_of('▛').unwrap().rects(cell);
        assert_eq!(r.len(), 3);
        let area: f32 = r.iter().map(|(b, _)| f32::from(b.size.width) * f32::from(b.size.height)).sum();
        assert!((area - 8. * 17. * 0.75).abs() < 9., "{area}");
        // A vertical line runs the full cell height.
        let v = shape_of('│').unwrap().rects(cell);
        let top = v.iter().map(|(b, _)| b.top()).fold(px(1e9), |a, t| a.min(t));
        let bottom = v.iter().map(|(b, _)| b.bottom()).fold(px(0.), |a, t| a.max(t));
        assert_eq!((top, bottom), (cell.top(), cell.bottom()));
        assert_eq!(shape_of('a'), None);
        assert_eq!(shape_of('░').map(|s| s.rects(cell)[0].1), Some(0.25));
    }

    #[test]
    fn parent_direction() {
        let mut t = Tile::Term(1);
        t.split_at(1, 2, false);
        assert_eq!(t.parent_vertical(2), Some(false));
        t.split_at(2, 3, true);
        assert_eq!((t.parent_vertical(1), t.parent_vertical(3)), (Some(false), Some(true)));
        assert_eq!(Tile::Term(1).parent_vertical(1), None);
    }

    #[test]
    fn split_tiles() {
        let mut t = Tile::Term(1);
        assert!(t.split_at(1, 2, false));
        assert!(t.split_at(2, 3, true));
        assert!(!t.split_at(9, 4, true), "no such pane");
        assert_eq!(t.ids(), vec![1, 2, 3]);
        assert!(!t.is_vertical(&[]) && t.is_vertical(&[true]));
        t.set_ratio(&[true], 0.99);
        assert!(matches!(&t, Tile::Split { b, .. } if matches!(**b, Tile::Split { ratio, .. } if ratio == 0.88)));
        // Closing a pane collapses its split into the one beside it.
        let t = t.without(2).unwrap();
        assert_eq!(t, Tile::Split { vertical: false, ratio: 0.5, a: Box::new(Tile::Term(1)), b: Box::new(Tile::Term(3)) });
        assert_eq!(t.without(1), Some(Tile::Term(3)));
        assert_eq!(Tile::Term(3).without(3), None);
    }

    #[test]
    fn cmd_click_links() {
        let line = "  --> src/main.rs:12:5 (see https://example.com/x).";
        let at = |needle: &str| line.find(needle).unwrap();
        assert_eq!(link_at(line, at("main")).as_deref(), Some("src/main.rs:12:5"));
        assert_eq!(classify_link("src/main.rs:12:5"), Some(Link::Path("src/main.rs".into(), Some((12, 5)))));
        assert_eq!(link_at(line, at("example")).as_deref(), Some("https://example.com/x"));
        assert_eq!(classify_link("https://example.com/x"), Some(Link::Url("https://example.com/x".into())));
        assert_eq!(link_at(line, 0), None);
        assert_eq!(classify_link("README.md"), Some(Link::Path("README.md".into(), None)));
        assert_eq!(classify_link("app.ts:7"), Some(Link::Path("app.ts".into(), Some((7, 1)))));
        assert_eq!(classify_link("hello"), None);
        assert_eq!(classify_link("-->"), None);
        assert_eq!(link_at("error: 'lib/a.py', line 3", 9).as_deref(), Some("lib/a.py"));
    }
}
