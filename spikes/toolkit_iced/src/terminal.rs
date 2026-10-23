//! Bottom-panel terminals: `alacritty_terminal` does PTY + emulation on its own
//! reader thread; we render the visible grid and forward keys.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, State as LoopState};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::tty::{self, Pty, Shell};
use alacritty_terminal::vte::ansi::{ClearMode, Color as AnsiColor, CursorShape, Handler, NamedColor};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{self, Paragraph as _};
use iced::advanced::widget::{tree, Tree, Widget};
use iced::advanced::{Clipboard, Shell as IcedShell};
use iced::futures::channel::mpsc::UnboundedSender;
use iced::keyboard::{self, key::Named, Key};
use iced::mouse::{self, ScrollDelta};
use iced::{Element, Event, Font, Length, Point, Rectangle, Size};

use crate::commands::KeyPress;
use crate::editor::{mono_text, FONT_SIZE, LINE_H};
use crate::{rgb, rgba, Message, Shared};

const PAD: f32 = 8.0;

/// Qur Dark terminal palette: ANSI 0-15, then fg / bg / cursor.
const ANSI: [u32; 16] = [
    0x1A1A1A, 0xFF6B6B, 0x3DDC97, 0xFFCB6B, 0x5B9BFF, 0xC792EA, 0x5FD7D7, 0xD6D6D6,
    0x5A5A5A, 0xFF8A8A, 0x7CEBB5, 0xFFDD8E, 0x8AB8FF, 0xDDB4F5, 0x8CE6E6, 0xFFFFFF,
];
const FG: u32 = 0xE6E6E6;
const BG: u32 = 0x000000;
const CURSOR: u32 = 0xFFFFFF;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Shell,
    Claude,
}

struct Size2 {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size2 {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Bridges PTY-thread events to the UI. At most one wakeup is queued per
/// terminal until the UI handles it, so bursts of output coalesce.
#[derive(Clone)]
pub struct Listener {
    id: usize,
    ui: UnboundedSender<usize>,
    pending: Arc<AtomicBool>,
    pty: Arc<OnceLock<EventLoopSender>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Wakeup | TermEvent::ChildExit(_) | TermEvent::Exit => {
                if !self.pending.swap(true, Ordering::AcqRel) {
                    let _ = self.ui.unbounded_send(self.id);
                }
            }
            // Replies to terminal queries (cursor position, device attributes).
            TermEvent::PtyWrite(s) => {
                if let Some(tx) = self.pty.get() {
                    let _ = tx.send(Msg::Input(s.into_bytes().into()));
                }
            }
            _ => {}
        }
    }
}

pub struct Terminal {
    pub id: usize,
    pub kind: Kind,
    /// Tab label ("zsh", "claude 2").
    pub label: String,
    /// Last time the PTY produced output (agent activity glyph).
    pub last_output: Instant,
    pub term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    pending: Arc<AtomicBool>,
    pid: i32,
    io: Option<JoinHandle<(EventLoop<Pty, Listener>, LoopState)>>,
    pub size: (usize, usize),
    pub spawned: Instant,
    /// Bumped each time new output is handled; render cache key.
    pub version: u64,
    pub first_output: Option<Instant>,
}

impl Terminal {
    pub fn spawn(id: usize, kind: Kind, label: String, cwd: &Path, ui: UnboundedSender<usize>) -> std::io::Result<Self> {
        let spawned = Instant::now();
        let (cols, rows) = (120, 8);
        let user_shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let shell = match kind {
            Kind::Shell => Shell::new(user_shell, vec!["-l".into()]),
            // Agents run on the user's CLI login, never an API key: `env -u`
            // strips keys inherited from Qur, `unset` covers ones the login
            // profile exports.
            Kind::Claude => Shell::new(
                "/usr/bin/env".into(),
                vec![
                    "-u".into(),
                    "ANTHROPIC_API_KEY".into(),
                    "-u".into(),
                    "OPENAI_API_KEY".into(),
                    user_shell,
                    "-l".into(),
                    "-c".into(),
                    "unset ANTHROPIC_API_KEY OPENAI_API_KEY; claude; exec \"$SHELL\" -l".into(),
                ],
            ),
        };
        let options = tty::Options {
            shell: Some(shell),
            working_directory: Some(cwd.to_path_buf()),
            drain_on_exit: false,
            env: [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let ws = WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: 8,
            cell_height: LINE_H as u16,
        };
        let pty = tty::new(&options, ws, id as u64)?;
        let pid = pty.child().id() as i32;

        let pending = Arc::new(AtomicBool::new(false));
        let pty_tx = Arc::new(OnceLock::new());
        let listener = Listener { id, ui, pending: pending.clone(), pty: pty_tx.clone() };
        let term = Arc::new(FairMutex::new(Term::new(
            TermConfig { scrolling_history: 5000, ..Default::default() },
            &Size2 { cols, rows },
            listener.clone(),
        )));
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let sender = event_loop.channel();
        let _ = pty_tx.set(event_loop.channel());
        let io = event_loop.spawn();

        Ok(Terminal {
            id,
            kind,
            label,
            last_output: spawned,
            term,
            sender,
            pending,
            pid,
            io: Some(io),
            size: (cols, rows),
            spawned,
            version: 0,
            first_output: None,
        })
    }

    /// Called on the UI thread when the PTY thread signalled new output.
    pub fn on_wakeup(&mut self) {
        self.pending.store(false, Ordering::Release);
        self.version += 1;
        self.last_output = Instant::now();
        self.first_output.get_or_insert_with(Instant::now);
    }

    fn grid_point(&self, row: usize, col: usize) -> GridPoint {
        let off = self.term.lock().grid().display_offset() as i32;
        GridPoint::new(Line(row as i32 - off), Column(col.min(self.size.0.saturating_sub(1))))
    }

    pub fn select_start(&mut self, row: usize, col: usize, right_half: bool) {
        let p = self.grid_point(row, col);
        let side = if right_half { Side::Right } else { Side::Left };
        self.term.lock().selection = Some(Selection::new(SelectionType::Simple, p, side));
        self.version += 1;
    }

    pub fn select_update(&mut self, row: usize, col: usize, right_half: bool) {
        let p = self.grid_point(row, col);
        let side = if right_half { Side::Right } else { Side::Left };
        if let Some(sel) = self.term.lock().selection.as_mut() {
            sel.update(p, side);
        }
        self.version += 1;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    /// Cmd+K: drop scrollback and the screen, then let the shell redraw its prompt.
    pub fn clear(&mut self) {
        {
            let mut t = self.term.lock();
            t.selection = None;
            t.clear_screen(ClearMode::Saved);
            t.clear_screen(ClearMode::All);
        }
        let _ = self.sender.send(Msg::Input(b"\x0c".to_vec().into()));
        self.version += 1;
    }

    pub fn input(&self, bytes: Vec<u8>) {
        let mut term = self.term.lock();
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
        }
        drop(term);
        let _ = self.sender.send(Msg::Input(bytes.into()));
    }

    pub fn scroll(&mut self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
        self.version += 1;
    }

    pub fn resize(&mut self, cols: usize, rows: usize, cell_w: f32) {
        if self.size == (cols, rows) {
            return;
        }
        self.size = (cols, rows);
        self.term.lock().resize(Size2 { cols, rows });
        let _ = self.sender.send(Msg::Resize(WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: cell_w as u16,
            cell_height: LINE_H as u16,
        }));
        self.version += 1;
    }

    pub fn app_cursor(&self) -> bool {
        self.term.lock().mode().contains(TermMode::APP_CURSOR)
    }

    /// Hang up the whole process group (SIGKILL after 2 s), then stop the
    /// reader thread. The returned handle finishes once the child is reaped.
    pub fn close(mut self) -> JoinHandle<()> {
        let pid = self.pid;
        unsafe {
            libc::kill(-pid, libc::SIGHUP);
        }
        let io = self.io.take();
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            // Keep the reader draining until the shell is gone: on macOS an
            // exiting process blocks until its tty output drains, and
            // alacritty's `Pty::drop` waits for the child without a timeout.
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut status = 0;
            loop {
                let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
                if r != 0 {
                    break; // reaped (r == pid) or already reaped elsewhere (-1)
                }
                if Instant::now() >= deadline {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                        libc::kill(pid, libc::SIGKILL);
                        libc::waitpid(pid, &mut status, 0);
                    }
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            // Anything left in the group (e.g. claude's children) goes too.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            let _ = sender.send(Msg::Shutdown);
            if let Some(io) = io {
                let _ = io.join();
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Keys -> bytes
// ---------------------------------------------------------------------------

/// Bytes a key press sends to the PTY, with the Mac terminal conventions from
/// docs/keymap.md (Opt+arrows = word jump, Cmd+arrows = line start/end,
/// Cmd+Backspace = kill line). `None` for app shortcuts.
pub fn key_bytes(k: &KeyPress, app_cursor: bool) -> Option<Vec<u8>> {
    let (key, mods, text) = (&k.key, k.mods, k.text.as_deref());
    let arrow = |c: u8| {
        if app_cursor { vec![0x1b, b'O', c] } else { vec![0x1b, b'[', c] }
    };
    if mods.logo() {
        return match key {
            Key::Named(Named::ArrowLeft) => Some(vec![0x01]),
            Key::Named(Named::ArrowRight) => Some(vec![0x05]),
            Key::Named(Named::Backspace) => Some(vec![0x15]),
            _ => None,
        };
    }
    if mods.alt() {
        match key {
            Key::Named(Named::ArrowLeft) => return Some(b"\x1bb".to_vec()),
            Key::Named(Named::ArrowRight) => return Some(b"\x1bf".to_vec()),
            Key::Named(Named::Backspace) => return Some(b"\x1b\x7f".to_vec()),
            _ => {}
        }
    }
    Some(match key {
        Key::Named(Named::Enter) => vec![b'\r'],
        Key::Named(Named::Backspace) => vec![0x7f],
        Key::Named(Named::Tab) => vec![b'\t'],
        Key::Named(Named::Escape) => vec![0x1b],
        Key::Named(Named::ArrowUp) => arrow(b'A'),
        Key::Named(Named::ArrowDown) => arrow(b'B'),
        Key::Named(Named::ArrowRight) => arrow(b'C'),
        Key::Named(Named::ArrowLeft) => arrow(b'D'),
        Key::Named(Named::Home) => b"\x1b[H".to_vec(),
        Key::Named(Named::End) => b"\x1b[F".to_vec(),
        Key::Named(Named::Delete) => b"\x1b[3~".to_vec(),
        Key::Named(Named::PageUp) => b"\x1b[5~".to_vec(),
        Key::Named(Named::PageDown) => b"\x1b[6~".to_vec(),
        Key::Character(_) if mods.control() => {
            let b = k.latin?.to_ascii_lowercase() as u8;
            if !b.is_ascii_lowercase() {
                return None;
            }
            vec![b & 0x1f]
        }
        Key::Named(Named::Space) => vec![b' '],
        _ if mods.alt() => {
            // Opt as Meta (Apple Terminal "Use Option as Meta key").
            let mut v = vec![0x1b];
            v.push(k.latin? as u8);
            v
        }
        _ => text?.as_bytes().to_vec(),
    })
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn resolve(c: AnsiColor, colors: &Colors) -> u32 {
    let from = |o: Option<alacritty_terminal::vte::ansi::Rgb>, d: u32| {
        o.map_or(d, |c| (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32)
    };
    match c {
        AnsiColor::Spec(c) => (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32,
        AnsiColor::Named(n) => {
            let i = n as usize;
            let default = match n {
                NamedColor::Foreground | NamedColor::BrightForeground => FG,
                NamedColor::Background => BG,
                NamedColor::Cursor => CURSOR,
                NamedColor::DimForeground => 0x9A9A9A,
                _ if i < 16 => ANSI[i],
                // Dim variants: use the normal color.
                _ => ANSI[(i - NamedColor::DimBlack as usize).min(7)],
            };
            from(colors[i], default)
        }
        AnsiColor::Indexed(i) => {
            let i = i as usize;
            let default = match i {
                0..16 => ANSI[i],
                16..232 => {
                    let v = |x: usize| if x == 0 { 0 } else { 55 + x as u32 * 40 };
                    let n = i - 16;
                    v(n / 36) << 16 | v(n / 6 % 6) << 8 | v(n % 6)
                }
                _ => {
                    let g = 8 + (i as u32 - 232) * 10;
                    g << 16 | g << 8 | g
                }
            };
            from(colors[i], default)
        }
    }
}

struct Row {
    hash: u64,
    /// (column, text, fg) runs
    runs: Vec<(usize, String, u32)>,
    /// (column, width, bg) runs that differ from the panel background
    bgs: Vec<(usize, usize, u32)>,
}

/// Per-row selected column range.
type SelRows = Vec<Option<(usize, usize)>>;

fn snapshot(term: &Term<Listener>) -> (Vec<Row>, Option<(usize, usize)>, SelRows) {
    let content = term.renderable_content();
    let rows_n = term.screen_lines();
    let cols_n = term.columns();
    let offset = content.display_offset as i32;
    let mut sel: SelRows = vec![None; rows_n];
    if let Some(range) = content.selection {
        for (r, slot) in sel.iter_mut().enumerate() {
            let line = r as i32 - offset;
            if line < range.start.line.0 || line > range.end.line.0 {
                continue;
            }
            let c0 = if line == range.start.line.0 { range.start.column.0 } else { 0 };
            let c1 = if line == range.end.line.0 { range.end.column.0 } else { cols_n.saturating_sub(1) };
            *slot = Some((c0, c1));
        }
    }
    let mut rows: Vec<Row> = (0..rows_n).map(|_| Row { hash: 0, runs: Vec::new(), bgs: Vec::new() }).collect();
    for cell in content.display_iter {
        let r = (cell.point.line.0 + offset) as usize;
        let col = cell.point.column.0;
        if r >= rows_n || cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let (mut fg, mut bg) = (resolve(cell.fg, content.colors), resolve(cell.bg, content.colors));
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        let ch = if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c };
        let row = &mut rows[r];
        match row.runs.last_mut() {
            Some((_, s, f)) if *f == fg => s.push(ch),
            _ => row.runs.push((col, ch.to_string(), fg)),
        }
        if bg != BG {
            match row.bgs.last_mut() {
                Some((c, w, b)) if *b == bg && *c + *w == col => *w += 1,
                _ => row.bgs.push((col, 1, bg)),
            }
        }
    }
    for row in &mut rows {
        // Trailing blanks don't need shaping.
        if let Some((_, s, _)) = row.runs.last_mut() {
            s.truncate(s.trim_end().len());
        }
        let mut h = DefaultHasher::new();
        row.runs.hash(&mut h);
        row.hash = h.finish();
    }
    let cursor = (content.cursor.shape != CursorShape::Hidden && offset == 0).then(|| {
        (content.cursor.point.line.0.max(0) as usize, content.cursor.point.column.0)
    });
    (rows, cursor, sel)
}

pub struct TermView<'a> {
    term: &'a Terminal,
    focused: bool,
    cell_w: f32,
    opacity: f32,
    shared: &'a Shared,
}

impl<'a> TermView<'a> {
    pub fn new(term: &'a Terminal, focused: bool, cell_w: f32, opacity: f32, shared: &'a Shared) -> Self {
        TermView { term, focused, cell_w, opacity, shared }
    }

    fn cell_at(&self, p: Point, bounds: Rectangle) -> (usize, usize, bool) {
        let x = ((p.x - bounds.x - PAD) / self.cell_w).max(0.0);
        let row = ((p.y - bounds.y - PAD) / LINE_H).max(0.0) as usize;
        (row.min(self.term.size.1.saturating_sub(1)), x as usize, x.fract() > 0.5)
    }

    fn grid_size(&self, bounds: Rectangle) -> (usize, usize) {
        let cols = ((bounds.width - 2.0 * PAD) / self.cell_w).floor().max(2.0) as usize;
        let rows = ((bounds.height - 2.0 * PAD) / LINE_H).floor().max(1.0) as usize;
        (cols, rows)
    }
}

struct State<P> {
    /// Per screen row: (terminal id, content hash, shaped paragraph)
    rows: RefCell<Vec<Option<(usize, u64, P)>>>,
    dragging: bool,
}

impl<P> Default for State<P> {
    fn default() -> Self {
        State { rows: RefCell::new(Vec::new()), dragging: false }
    }
}

impl<'a, Theme, Renderer> Widget<Message, Theme, Renderer> for TermView<'a>
where
    Renderer: text::Renderer<Font = Font>,
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State<Renderer::Paragraph>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::<Renderer::Paragraph>::default())
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut IcedShell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let id = self.term.id;
        let size = self.grid_size(bounds);
        if size != self.term.size {
            shell.publish(Message::TermResize { id, cols: size.0, rows: size.1 });
        }
        let state = tree.state.downcast_mut::<State<Renderer::Paragraph>>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position_over(bounds) {
                    let (row, col, right) = self.cell_at(p, bounds);
                    state.dragging = true;
                    shell.publish(Message::TermMouse { id, row, col, right, drag: false });
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.dragging => {
                let (row, col, right) = self.cell_at(*position, bounds);
                shell.publish(Message::TermMouse { id, row, col, right, drag: true });
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => state.dragging = false,
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let lines = match *delta {
                    ScrollDelta::Lines { y, .. } => (y * 3.0) as i32,
                    ScrollDelta::Pixels { y, .. } => (y / LINE_H) as i32,
                };
                if lines != 0 {
                    shell.publish(Message::TermScroll { id, lines });
                }
                shell.capture_event();
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key, physical_key, modifiers, text, .. }) if self.focused => {
                shell.publish(Message::Key(KeyPress {
                    key: key.clone(),
                    latin: key.to_latin(*physical_key),
                    mods: *modifiers,
                    text: text.as_ref().map(|t| t.to_string()),
                }));
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State<Renderer::Paragraph>>();
        let bounds = layout.bounds();
        renderer.fill_quad(Quad { bounds, ..Default::default() }, rgba(BG, self.opacity));

        let (rows, cursor, sel) = snapshot(&self.term.term.lock());
        let origin = Point::new(bounds.x + PAD, bounds.y + PAD);
        let cw = self.cell_w;

        renderer.with_layer(bounds, |renderer| {
            let mut cache = state.rows.borrow_mut();
            cache.resize_with(rows.len(), || None);
            for (r, row) in rows.iter().enumerate() {
                let y = origin.y + r as f32 * LINE_H;
                if let Some((c0, c1)) = sel.get(r).copied().flatten() {
                    renderer.fill_quad(
                        Quad {
                            bounds: Rectangle::new(Point::new(origin.x + c0 as f32 * cw, y), Size::new((c1 + 1 - c0) as f32 * cw, LINE_H)),
                            ..Default::default()
                        },
                        rgb(0x3A3A3A),
                    );
                }
                for &(c, w, bg) in &row.bgs {
                    renderer.fill_quad(
                        Quad {
                            bounds: Rectangle::new(Point::new(origin.x + c as f32 * cw, y), Size::new(w as f32 * cw, LINE_H)),
                            ..Default::default()
                        },
                        rgb(bg),
                    );
                }
                let fresh = matches!(&cache[r], Some((id, h, _)) if *id == self.term.id && *h == row.hash);
                if !fresh {
                    // Pad runs with spaces so every run starts at its cell column.
                    let mut line = String::new();
                    let mut spans_at: Vec<(std::ops::Range<usize>, u32)> = Vec::new();
                    let mut col = 0;
                    for (c, s, fg) in &row.runs {
                        while col < *c {
                            line.push(' ');
                            col += 1;
                        }
                        let start = line.len();
                        line.push_str(s);
                        col += s.chars().count();
                        spans_at.push((start..line.len(), *fg));
                    }
                    let spans: Vec<text::Span<'_, (), Font>> = spans_at
                        .iter()
                        .map(|(rg, fg)| text::Span::new(&line[rg.clone()]).color(rgb(*fg)))
                        .collect();
                    let mut t = mono_text(&spans[..]);
                    t.size = iced::Pixels(FONT_SIZE);
                    cache[r] = Some((self.term.id, row.hash, Renderer::Paragraph::with_spans(t)));
                }
                if let Some((_, _, p)) = &cache[r] {
                    renderer.fill_paragraph(p, Point::new(origin.x, y), rgb(FG), bounds);
                }
            }
            if let Some((r, c)) = cursor {
                let rect = Rectangle::new(
                    Point::new(origin.x + c as f32 * cw, origin.y + r as f32 * LINE_H),
                    Size::new(cw, LINE_H),
                );
                if self.focused && self.term.term.lock().grid().display_offset() == 0 {
                    renderer.fill_quad(Quad { bounds: rect, ..Default::default() }, iced::Color { a: 0.55, ..rgb(CURSOR) });
                } else {
                    renderer.fill_quad(
                        Quad {
                            bounds: rect,
                            border: iced::Border { color: rgb(CURSOR), width: 1.0, radius: 0.0.into() },
                            ..Default::default()
                        },
                        iced::Color::TRANSPARENT,
                    );
                }
            }
        });
        // Spawn -> first output on screen (first terminal only).
        if !self.shared.term_metric_done.get() && self.term.first_output.is_some() {
            self.shared.term_metric_done.set(true);
            spike_common::metric("terminal_spawn_ms", self.term.spawned.elapsed().as_millis());
        }
    }
}

impl<'a> From<TermView<'a>> for Element<'a, Message> {
    fn from(v: TermView<'a>) -> Self {
        Element::new(v)
    }
}
