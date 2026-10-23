//! Bottom panel terminals: alacritty_terminal for PTY + emulation, rendered as
//! a monospace grid. PTY I/O runs on alacritty's own thread; the UI redraws
//! only when the active terminal has new output.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as TPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{ClearMode, Color, CursorShape, Handler, NamedColor};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyDownEvent,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ScrollEvent, ShapedLine,
    SharedString, Style, Task, TextRun, UTF16Selection, Window, div, fill, font, point,
    prelude::*, px, relative, size,
};
use spike_common::{self as sc, qur_dark};

use crate::commands::{edit, qur, terminal as ta};
use crate::ui::{CODE_FONT, Ui, hsla};

// ---------------------------------------------------------------------------
// Palette (Qur Dark-ish)
// ---------------------------------------------------------------------------

const ANSI: [u32; 16] = [
    0x1A1A1A, 0xFF5F5F, 0x5FD787, 0xF0C674, 0x6CA0FF, 0xC792EA, 0x5FD7D7, 0xC8C8C8, // normal
    0x6A6A6A, 0xFF8787, 0x87FFAF, 0xFFE08A, 0x8FB8FF, 0xDDB0FF, 0x87F0F0, 0xFFFFFF, // bright
];
const FG: u32 = 0xE6E6E6;
const CURSOR: u32 = 0xFFFFFF;

fn indexed(i: u8) -> u32 {
    match i {
        0..=15 => ANSI[i as usize],
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

/// `None` = the default background (not painted, so window opacity shows).
fn resolve(c: Color, term: &Term<Listener>, is_bg: bool, dim: bool) -> Option<u32> {
    let rgb = |r: alacritty_terminal::vte::ansi::Rgb| ((r.r as u32) << 16) | ((r.g as u32) << 8) | r.b as u32;
    let colors = term.colors();
    let v = match c {
        Color::Spec(r) => rgb(r),
        Color::Indexed(i) => colors[i as usize].map(rgb).unwrap_or_else(|| indexed(i)),
        Color::Named(n) => {
            if let Some(r) = colors[n] {
                rgb(r)
            } else {
                match n {
                    NamedColor::Background => return if is_bg { None } else { Some(qur_dark::BG) },
                    NamedColor::Foreground | NamedColor::BrightForeground => FG,
                    NamedColor::DimForeground => 0x9A9A9A,
                    NamedColor::Cursor => CURSOR,
                    n if (n as usize) < 16 => ANSI[n as usize],
                    n => {
                        // Dim colors: halve the normal ones.
                        let base = ANSI[(n as usize).saturating_sub(NamedColor::DimBlack as usize) % 8];
                        ((base >> 1) & 0x7F7F7F) | 0x000000
                    }
                }
            }
        }
    };
    Some(if dim { (v >> 1) & 0x7F7F7F } else { v })
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Listener(UnboundedSender<()>);

impl EventListener for Listener {
    fn send_event(&self, ev: TermEvent) {
        if matches!(ev, TermEvent::Wakeup | TermEvent::ChildExit(_) | TermEvent::Exit | TermEvent::Title(_)) {
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Shell,
    /// Runs `claude` in the worktree of agent task `n`.
    Claude(usize),
}

pub struct Session {
    pub id: usize,
    pub kind: Kind,
    pub label: String,
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    pid: i32,
    size: (usize, usize),
    pub last_output: Option<Instant>,
    spawned: Instant,
    /// Text typed since the last Enter (for the task title).
    typed: String,
    pub first_line: Option<String>,
    _reader: Task<()>,
}

pub fn shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
}

fn spawn_pty(cwd: &Path, run_claude: bool, id: usize) -> std::io::Result<(Arc<FairMutex<Term<Listener>>>, EventLoopSender, i32, futures::channel::mpsc::UnboundedReceiver<()>)> {
    let (tx, rx) = unbounded();
    let listener = Listener(tx);
    let (cols, lines) = (100, 12);
    // `env -u` strips vendor API keys so the CLIs use their own logins.
    let mut args = vec!["-u".into(), "ANTHROPIC_API_KEY".into(), "-u".into(), "OPENAI_API_KEY".into(), shell(), "-l".into()];
    if run_claude {
        args.push("-c".into());
        args.push("claude; exec \"$SHELL\" -l".into());
    }
    let opts = tty::Options {
        shell: Some(tty::Shell::new("/usr/bin/env".into(), args)),
        working_directory: Some(cwd.to_path_buf()),
        drain_on_exit: false,
        env: HashMap::from([
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("TERM_PROGRAM".into(), "qur".into()),
        ]),
    };
    let term = Arc::new(FairMutex::new(Term::new(TermConfig::default(), &Dims { cols, lines }, listener.clone())));
    let pty = tty::new(&opts, WindowSize { num_lines: lines as u16, num_cols: cols as u16, cell_width: 8, cell_height: 16 }, id as u64)?;
    let pid = pty.child().id() as i32;
    let ev = EventLoop::new(term.clone(), listener, pty, false, false)?;
    let sender = ev.channel();
    ev.spawn();
    Ok((term, sender, pid, rx))
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

    pub fn is_busy(&self) -> bool {
        self.last_output.is_some_and(|t| t.elapsed() < Duration::from_secs(2))
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
}

static SPAWN_METRIC_DONE: AtomicBool = AtomicBool::new(false);

pub struct TerminalPanel {
    pub focus: FocusHandle,
    pub sessions: Vec<Session>,
    pub active: usize,
    next_id: usize,
    pub message: Option<String>,
    cell: (Pixels, Pixels),
    last_bounds: Option<Bounds<Pixels>>,
    selecting: bool,
    idle_check: Option<Task<()>>,
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl TerminalPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        TerminalPanel {
            focus: cx.focus_handle(),
            sessions: Vec::new(),
            active: 0,
            next_id: 1,
            message: None,
            cell: (px(8.), px(16.)),
            last_bounds: None,
            selecting: false,
            idle_check: None,
        }
    }

    pub fn open(&mut self, cwd: &Path, kind: Kind, label: String, cx: &mut Context<Self>) -> Option<usize> {
        let id = self.next_id;
        self.next_id += 1;
        let (term, sender, pid, mut rx) = match spawn_pty(cwd, matches!(kind, Kind::Claude(_)), id) {
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
                        if ix == p.active {
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
            term,
            sender,
            pid,
            size: (100, 12),
            last_output: None,
            spawned: Instant::now(),
            typed: String::new(),
            first_line: None,
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

    pub fn label_for(&self, kind: Kind) -> String {
        let base = match kind {
            Kind::Shell => shell().rsplit('/').next().unwrap_or("sh").to_string(),
            Kind::Claude(n) => return format!("claude {n}"),
        };
        let dup = self.sessions.iter().filter(|s| s.kind == Kind::Shell).count();
        if dup == 0 { base } else { format!("{base} {}", dup + 1) }
    }

    pub fn close(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.sessions.len() {
            return;
        }
        let s = self.sessions.remove(ix);
        s.shutdown(false);
        if self.active >= self.sessions.len() {
            self.active = self.sessions.len().saturating_sub(1);
        }
        cx.emit(PanelEvent::Closed);
        cx.notify();
    }

    pub fn close_all_sync(&mut self) {
        for s in self.sessions.drain(..) {
            s.shutdown(true);
        }
    }


    pub fn session_by_id(&self, id: usize) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn activate_id(&mut self, id: usize, cx: &mut Context<Self>) -> bool {
        if let Some(ix) = self.sessions.iter().position(|s| s.id == id) {
            self.active = ix;
            cx.notify();
            true
        } else {
            false
        }
    }

    pub fn cycle(&mut self, d: isize, cx: &mut Context<Self>) {
        let n = self.sessions.len() as isize;
        if n > 0 {
            self.active = ((self.active as isize + d).rem_euclid(n)) as usize;
            cx.notify();
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
        if s.first_line.is_none() && matches!(s.kind, Kind::Claude(_)) {
            for &b in &bytes {
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
        if let Some(s) = self.active_session() {
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
    fn copy(&mut self, _: &edit::Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.active_session().and_then(|s| s.term.lock().selection_to_string()) {
            cx.write_to_clipboard(ClipboardItem::new_string(t));
        }
    }
    fn paste(&mut self, _: &edit::Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        let bracketed = self
            .active_session()
            .map(|s| s.term.lock().mode().contains(TermMode::BRACKETED_PASTE))
            .unwrap_or(false);
        let t = t.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if bracketed { format!("\x1b[200~{t}\x1b[201~") } else { t };
        self.write_active(bytes.into_bytes(), cx);
    }
    fn select_all(&mut self, _: &edit::SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.active_session() {
            let mut t = s.term.lock();
            let top = t.topmost_line();
            let bottom = t.bottommost_line();
            let last = t.last_column();
            let mut sel = Selection::new(SelectionType::Simple, TPoint::new(top, Column(0)), Side::Left);
            sel.update(TPoint::new(bottom, last), Side::Right);
            t.selection = Some(sel);
        }
        cx.notify();
    }
    fn close_terminal(&mut self, _: &qur::CloseTerminal, _: &mut Window, cx: &mut Context<Self>) {
        let ix = self.active;
        self.close(ix, cx);
    }

    fn grid_point(&self, p: gpui::Point<Pixels>, display_offset: usize) -> (TPoint, Side) {
        let b = self.last_bounds.unwrap_or_default();
        let x = (p.x - b.left()).max(px(0.));
        let col = (x / self.cell.0).floor() as usize;
        let side = if (x / self.cell.0).fract() < 0.5 { Side::Left } else { Side::Right };
        let row = ((p.y - b.top()).max(px(0.)) / self.cell.1).floor() as i32;
        (TPoint::new(Line(row - display_offset as i32), Column(col)), side)
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if let Some(s) = self.sessions.get(self.active) {
            let mut t = s.term.lock();
            let off = t.grid().display_offset();
            let (p, side) = self.grid_point(ev.position, off);
            let p = TPoint::new(p.line, p.column.min(t.last_column()));
            t.selection = Some(Selection::new(SelectionType::Simple, p, side));
            self.selecting = true;
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !(self.selecting && ev.pressed_button == Some(MouseButton::Left)) {
            self.selecting = false;
            return;
        }
        if let Some(s) = self.sessions.get(self.active) {
            let mut t = s.term.lock();
            let off = t.grid().display_offset();
            let (p, side) = self.grid_point(ev.position, off);
            let p = TPoint::new(p.line, p.column.min(t.last_column()));
            if let Some(sel) = t.selection.as_mut() {
                sel.update(p, side);
            }
        }
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lh = self.cell.1;
        if let Some(s) = self.active_session() {
            let mut t = s.term.lock();
            if !t.mode().contains(TermMode::ALT_SCREEN) {
                let lines = (ev.delta.pixel_delta(lh).y / lh).round() as i32;
                if lines != 0 {
                    t.scroll_display(Scroll::Delta(lines));
                }
            }
        }
        cx.notify();
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
        if let Some(s) = self.sessions.get(self.active) {
            s.term.lock().selection = None;
        }
        self.write_active(text.as_bytes().to_vec(), cx);
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
}

struct GridRow {
    y: Pixels,
    text: ShapedLine,
    bgs: Vec<(usize, usize, u32)>,
}

struct GridPrepaint {
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
        let ui = Ui::get(cx);
        let fs = ui.z(12.);
        let f = font(CODE_FONT);
        let ts = window.text_system().clone();
        let cw = ts.advance(ts.resolve_font(&f), fs, 'm').map(|s| s.width).unwrap_or(px(7.));
        let lh = ui.z(16.);
        let cols = ((bounds.size.width / cw).floor() as usize).max(2);
        let lines = ((bounds.size.height / lh).floor() as usize).max(1);
        let mut out = GridPrepaint { rows: Vec::new(), cursor: None, has_content: false };
        self.panel.update(cx, |p, _| {
            p.cell = (cw, lh);
            p.last_bounds = Some(bounds);
            if let Some(s) = p.sessions.get_mut(p.active)
                && s.size != (cols, lines)
            {
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
        let Some(s) = p.sessions.get(p.active) else { return out };
        let term = s.term.lock();
        let content = term.renderable_content();
        let off = content.display_offset as i32;
        let sel = content.selection;
        let n_lines = term.screen_lines();
        let mut text: Vec<String> = vec![String::new(); n_lines];
        let mut runs: Vec<Vec<TextRun>> = vec![Vec::new(); n_lines];
        let mut bgs: Vec<Vec<(usize, usize, u32)>> = vec![Vec::new(); n_lines];
        let mk = |len: usize, color: u32, bold: bool| TextRun {
            len,
            font: if bold { gpui::Font { weight: gpui::FontWeight::BOLD, ..f.clone() } } else { f.clone() },
            color: hsla(color),
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
            let mut fg = resolve(cell.fg, &term, false, dim).unwrap_or(FG);
            let mut bg = resolve(cell.bg, &term, true, false);
            if cell.flags.contains(Flags::INVERSE) {
                let old_bg = bg.unwrap_or(qur_dark::BG);
                bg = Some(fg);
                fg = old_bg;
            }
            if sel.is_some_and(|r| r.contains(ic.point)) {
                bg = Some(qur_dark::SELECTION);
            }
            if let Some(b) = bg {
                match bgs[row].last_mut() {
                    Some((_, end, c)) if *end == col && *c == b => *end = col + 1,
                    _ => bgs[row].push((col, col + 1, b)),
                }
            }
            let ch = if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c };
            if ch != ' ' {
                out.has_content = true;
            }
            // Pad skipped columns (after wide chars) to keep the grid aligned.
            let bold = cell.flags.contains(Flags::BOLD);
            let len = ch.len_utf8();
            text[row].push(ch);
            match runs[row].last_mut() {
                Some(r) if r.color == hsla(fg) && (r.font.weight == gpui::FontWeight::BOLD) == bold => r.len += len,
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
            out.rows.push(GridRow { y: bounds.top() + lh * i as f32, text: shaped, bgs: std::mem::take(&mut bgs[i]) });
        }
        out
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), pre: &mut GridPrepaint, window: &mut Window, cx: &mut App) {
        let p = self.panel.read(cx);
        let focus = p.focus.clone();
        let focused = focus.is_focused(window);
        let (cw, lh) = p.cell;
        let spawned = p
            .sessions
            .get(p.active)
            .filter(|s| s.last_output.is_some())
            .map(|s| s.spawned);
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.panel.clone()), cx);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for row in &pre.rows {
                for &(a, b, c) in &row.bgs {
                    window.paint_quad(fill(
                        Bounds::new(point(bounds.left() + cw * a as f32, row.y), size(cw * (b - a) as f32, lh)),
                        hsla(c),
                    ));
                }
                row.text.paint(point(bounds.left(), row.y), lh, gpui::TextAlign::Left, None, window, cx).ok();
            }
            if let Some((r, c, shape)) = pre.cursor {
                let origin = point(bounds.left() + cw * c as f32, bounds.top() + lh * r as f32);
                let q = match (focused, shape) {
                    (true, CursorShape::Beam) => fill(Bounds::new(origin, size(px(2.), lh)), hsla(CURSOR)),
                    (true, CursorShape::Underline) => fill(Bounds::new(point(origin.x, origin.y + lh - px(2.)), size(cw, px(2.))), hsla(CURSOR)),
                    (true, _) => {
                        let mut h = hsla(CURSOR);
                        h.a = 0.55;
                        fill(Bounds::new(origin, size(cw, lh)), h)
                    }
                    (false, _) => gpui::outline(Bounds::new(origin, size(cw, lh)), hsla(CURSOR), gpui::BorderStyle::Solid),
                };
                window.paint_quad(q);
            }
        });
        if pre.has_content
            && let Some(spawned) = spawned
            && !SPAWN_METRIC_DONE.swap(true, Ordering::Relaxed)
        {
            sc::metric("terminal_spawn_ms", spawned.elapsed().as_millis());
        }
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let tabs = self.sessions.iter().enumerate().map(|(i, s)| {
            let active = i == self.active;
            let label = match (&s.kind, &s.first_line) {
                (Kind::Claude(_), _) => s.label.clone(),
                _ => s.label.clone(),
            };
            div()
                .id(("tab", s.id))
                .flex()
                .items_center()
                .gap(ui.z(6.))
                .px(ui.z(8.))
                .h_full()
                .text_size(ui.z(12.))
                .text_color(hsla(if active { qur_dark::TEXT } else { qur_dark::TEXT_MUTED }))
                .when(active, |d| d.bg(ui.bg(qur_dark::CURRENT_LINE)))
                .cursor_pointer()
                .on_click(cx.listener(move |p, _, window, cx| {
                    p.active = i;
                    window.focus(&p.focus, cx);
                    cx.notify();
                }))
                .child(div().whitespace_nowrap().child(label))
                .child(
                    div()
                        .id(("close", s.id))
                        .text_color(hsla(qur_dark::TEXT_MUTED))
                        .hover(|d| d.text_color(hsla(qur_dark::TEXT)))
                        .child("×")
                        .on_click(cx.listener(move |p, _, _, cx| {
                            cx.stop_propagation();
                            p.close(i, cx);
                        })),
                )
        });
        let strip = div()
            .flex()
            .flex_none()
            .items_center()
            .h(ui.z(26.))
            .border_b_1()
            .border_color(hsla(qur_dark::BORDER))
            .children(tabs)
            .children(self.message.clone().map(|m| {
                div().px(ui.z(8.)).text_size(ui.z(12.)).text_color(hsla(qur_dark::TEXT_MUTED)).child(m)
            }));
        div()
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end))
            .on_action(cx.listener(Self::kill_line))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::close_terminal))
            .child(strip)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .px(ui.z(8.))
                    .py(ui.z(4.))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_scroll(cx.listener(Self::on_scroll))
                    .when(!self.sessions.is_empty(), |d| d.child(GridElement { panel: cx.entity() })),
            )
    }
}

pub fn qur_home() -> PathBuf {
    std::env::var_os("QUR_HOME").map(PathBuf::from).unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../out/qur-home")
    })
}
