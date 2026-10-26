//! The code editor view: renders a `qur_editor::Editor` with `qur_syntax`
//! highlighting, only for the visible visual rows. Incremental reparses run
//! on the background executor; the UI thread only applies edits to the tree.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollEvent, ShapedLine, SharedString, Style,
    Subscription, Task, TextRun, UTF16Selection, Window, div, fill, font, point, prelude::*, px, relative,
    size, svg,
};
use qur_editor::vim::{Key as VimKey, Mode as VimMode, Vim};
use qur_editor::{Command, Ctx, Editor, FindOptions, Outcome, Point, Rope, Selection};
use qur_syntax::{HIGHLIGHT_NAMES, HighlightSpan, Language, SyntaxState, TextEdit, TextSource};

use crate::commands::{edit, editor as ea};
use crate::input::{InputEvent, TextInput};
use crate::look::Look;

/// `qur_syntax::TextSource` over a rope (no flattening).
pub struct RopeText(pub Rope);

impl TextSource for RopeText {
    fn len(&self) -> usize {
        self.0.len_bytes()
    }
    fn chunk_at(&self, byte: usize) -> (&str, usize) {
        if byte >= self.0.len_bytes() {
            return ("", self.0.len_bytes());
        }
        let (chunk, start, _, _) = self.0.chunk_at_byte(byte);
        (chunk, start)
    }
}

pub struct Loaded {
    model: Editor,
    language: Option<Language>,
    syntax: Option<SyntaxState>,
}

pub enum EditorEvent {
    /// Dirty state, path or selection changed (title bar, menus).
    Changed,
    /// A user-facing error (save failed, ...).
    Error(String),
    /// Soft wrap toggled (menu checkmark).
    WrapToggled,
    /// Esc with nothing left to dismiss: close the code modal (decision 18).
    CloseRequested,
}

struct FindBar {
    query: Entity<TextInput>,
    replace: Option<Entity<TextInput>>,
    opts: FindOptions,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

#[derive(Default)]
struct Bench {
    first_frame: bool,
    autoscroll: bool,
    autotype: bool,
    keys_sent: usize,
    pending_key: Option<Instant>,
    key_outstanding: bool,
    scroll: Vec<f64>,
    last_frame: Option<Instant>,
    typing: Vec<f64>,
    done: bool,
}

pub struct EditorView {
    pub focus: FocusHandle,
    pub model: Editor,
    pub language: Option<Language>,
    syntax: Option<SyntaxState>,
    reparse: Option<Task<()>>,
    hl_cache: Option<(u64, Range<usize>, Vec<HighlightSpan>)>,
    soft_wrap: bool,
    tab_width: usize,
    scroll_y: Pixels,
    scroll_x: Pixels,
    // From the last prepaint.
    cell_w: Pixels,
    line_h: Pixels,
    text_x: Pixels,
    bounds: Option<Bounds<Pixels>>,
    wrap_cols: Option<usize>,
    content_w: Pixels,
    find: Option<FindBar>,
    dragging: bool,
    /// Last time the user scrolled or clicked (Live follow respects it).
    pub last_user_input: Option<Instant>,
    /// Live view: edits are ignored while the agent is editing this file.
    pub read_only: bool,
    /// Vim mode (`[keys] vim`, M6); None = VS Code keys only.
    pub vim: Option<Vim>,
    /// Lines the agent changed last (Live view), painted in its accent.
    pub agent_mark: Option<(Range<usize>, qur_theme::Rgba)>,
    /// The file changed on disk while this buffer had unsaved edits: ask
    /// before reloading (never overwrite what the user hasn't seen).
    pub disk_changed: bool,
    /// Opened during a merge with conflicts: show the resolve bar (M4).
    pub merge_conflicts: bool,
    conflict_cache: Option<(u64, Vec<qur_git::Conflict>)>,
    /// IME composition (marked text): drawn underlined at the primary
    /// cursor, not in the buffer until committed (then typed at every cursor).
    marked: Option<Marked>,
    bench: Bench,
}

/// The IME's text being composed, and the part it has selected (UTF-16
/// offsets into `text`; the clause being converted).
struct Marked {
    text: String,
    selected: Option<Range<usize>>,
}

impl EventEmitter<EditorEvent> for EditorView {}

impl EditorView {
    /// Read and parse a file (no UI context needed, so it can fail cleanly
    /// before a view exists). Then `EditorView::new(loaded, ..)`.
    pub fn load(path: &Path) -> std::io::Result<Loaded> {
        let t0 = Instant::now();
        let language = Language::for_path(path);
        let indent = language.map(|l| l.indent_unit()).unwrap_or("    ");
        let model = Editor::open(path, indent)?;
        if std::env::var_os("QUR_BENCH").is_some() {
            println!("METRIC open_ms={}", t0.elapsed().as_millis());
        }
        // Parsing happens in the background (`EditorView::new`): text shows
        // first, colors land a frame or two later.
        Ok(Loaded { model, language, syntax: None })
    }

    pub fn new(loaded: Loaded, soft_wrap: bool, tab_width: usize, cx: &mut Context<Self>) -> Self {
        let Loaded { model, language, syntax } = loaded;
        // Bytes that didn't decode (or a binary's stray CRs) can't be saved back.
        let read_only = model.is_lossy();
        let bench = Bench {
            autoscroll: std::env::var("QUR_BENCH").as_deref() == Ok("scroll"),
            autotype: std::env::var("QUR_BENCH").as_deref() == Ok("type"),
            ..Default::default()
        };
        let mut view = EditorView {
            focus: cx.focus_handle(),
            model,
            language,
            syntax,
            reparse: None,
            hl_cache: None,
            soft_wrap,
            tab_width,
            scroll_y: px(0.),
            scroll_x: px(0.),
            cell_w: px(8.),
            line_h: px(20.),
            text_x: px(40.),
            bounds: None,
            wrap_cols: None,
            content_w: px(0.),
            find: None,
            dragging: false,
            last_user_input: None,
            read_only,
            vim: None,
            agent_mark: None,
            disk_changed: false,
            merge_conflicts: false,
            conflict_cache: None,
            marked: None,
            bench,
        };
        if view.syntax.is_none() {
            view.parse_in_background(cx);
        }
        view
    }

    /// Full parse off the UI thread (file open). If the text changed
    /// meanwhile, parse again from the latest text.
    fn parse_in_background(&mut self, cx: &mut Context<Self>) {
        let Some(lang) = self.language else { return };
        let version = self.model.version();
        let text = RopeText(self.model.rope().clone());
        let bg = cx.background_executor().clone();
        self.reparse = Some(cx.spawn(async move |this, cx| {
            let state = bg
                .spawn(async move {
                    let mut s = SyntaxState::new(lang);
                    s.parse(&text);
                    s
                })
                .await;
            this.update(cx, |this, cx| {
                if this.model.version() == version {
                    this.syntax = Some(state);
                    this.hl_cache = None;
                    crate::mark("highlighted");
                    cx.notify();
                } else {
                    this.parse_in_background(cx);
                }
            })
            .ok();
        }));
    }

    pub fn path(&self) -> Option<&Path> {
        self.model.path()
    }

    pub fn title(&self) -> String {
        self.path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into())
    }

    pub fn is_dirty(&self) -> bool {
        self.model.is_dirty()
    }

    pub fn soft_wrap(&self) -> bool {
        self.soft_wrap
    }

    pub fn cursor(&self) -> Point {
        self.model.primary().head
    }

    /// The file changed on disk (watcher). Clean buffers reload and return the
    /// changed line range; dirty ones get the "changed on disk" banner.
    pub fn disk_change(&mut self, cx: &mut Context<Self>) -> Option<Range<usize>> {
        if self.model.is_dirty() {
            self.disk_changed = true;
            cx.notify();
            return None;
        }
        let new = self.model.read_disk_text().ok()?;
        let old = self.model.rope().to_string();
        if new == old {
            return None;
        }
        let range = changed_lines(&old, &new);
        let scroll = self.scroll_y;
        self.reload_from_disk(cx);
        self.scroll_y = scroll;
        Some(range)
    }

    /// The primary selection's text, if any (Find in Project seeds from it).
    pub fn selected_text(&self) -> Option<String> {
        Some(self.model.selected_text()).filter(|t| !t.is_empty())
    }

    /// Conflict markers in the buffer (cached per text version).
    pub fn conflicts(&mut self) -> Vec<qur_git::Conflict> {
        let v = self.model.version();
        if self.conflict_cache.as_ref().map(|c| c.0) != Some(v) {
            let text = self.model.rope().to_string();
            self.conflict_cache = Some((v, qur_git::find_conflicts(&text)));
        }
        self.conflict_cache.as_ref().map(|c| c.1.clone()).unwrap_or_default()
    }

    /// Resolve conflict #i in the buffer (one undoable edit; save to keep it).
    pub fn resolve_conflict(&mut self, i: usize, r: qur_git::Resolution, cx: &mut Context<Self>) {
        let text = self.model.rope().to_string();
        let start = self.conflicts().get(i).map(|c| c.start).unwrap_or(0);
        let new = qur_git::resolve(&text, i, r);
        if new == text {
            return;
        }
        let was_ro = std::mem::replace(&mut self.read_only, false);
        self.run(Command::SelectAll, cx);
        self.run(Command::Insert(new), cx);
        self.read_only = was_ro;
        let p = self.model.clamp(Point { line: start, col: 0 });
        self.model.set_selections(vec![Selection::caret(p)], 0);
        self.reveal_lines(start..start + 1, cx);
        cx.emit(EditorEvent::Changed);
    }

    /// The conflict at or after the cursor (else the last one).
    pub fn current_conflict(&mut self) -> usize {
        let conflicts = self.conflicts();
        let cur = self.cursor().line;
        conflicts.iter().position(|c| c.end >= cur).unwrap_or(conflicts.len().saturating_sub(1))
    }

    /// Resolve the conflict at the cursor (Merge > Accept Ours/Theirs/Both).
    pub fn resolve_current_conflict(&mut self, r: qur_git::Resolution, cx: &mut Context<Self>) {
        let i = self.current_conflict();
        self.resolve_conflict(i, r, cx);
    }

    /// Move to the next (`forward`) or previous conflict.
    pub fn step_conflict(&mut self, forward: bool, cx: &mut Context<Self>) {
        let i = self.current_conflict();
        let n = self.conflicts().len();
        let next = if forward { (i + 1).min(n.saturating_sub(1)) } else { i.saturating_sub(1) };
        self.jump_conflict(next, cx);
    }

    fn jump_conflict(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(c) = self.conflicts().get(i).cloned() {
            let p = Point { line: c.start, col: 0 };
            self.model.set_selections(vec![Selection::caret(p)], 0);
            self.reveal_lines(c.start..c.end + 1, cx);
        }
    }

    /// Scroll so `lines` are visible, centred when they're off screen.
    pub fn reveal_lines(&mut self, lines: Range<usize>, cx: &mut Context<Self>) {
        let rope = self.model.rope().clone();
        let (top, _) = self.model.wrap().point_to_row(&rope, Point { line: lines.start, col: 0 });
        let (bottom, _) = self.model.wrap().point_to_row(&rope, Point { line: lines.end.max(lines.start), col: 0 });
        let h = self.viewport_h();
        let y0 = self.line_h * top as f32;
        let y1 = self.line_h * (bottom + 1) as f32;
        if y0 < self.scroll_y || y1 > self.scroll_y + h {
            self.scroll_y = (y0 - (h - (y1 - y0)).max(px(0.)) / 2.).clamp(px(0.), self.max_scroll_y());
        }
        cx.notify();
    }

    pub fn user_active_within(&self, d: Duration) -> bool {
        self.last_user_input.is_some_and(|t| t.elapsed() < d)
    }

    pub fn reload_from_disk(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path().map(Path::to_path_buf) else { return };
        let indent = self.language.map(|l| l.indent_unit()).unwrap_or("    ");
        match Editor::open_with(&path, indent, self.model.forced_encoding()) {
            Ok(m) => {
                let cursor = self.cursor();
                self.model = m;
                let c = self.model.clamp(cursor);
                self.model.set_selections(vec![Selection::caret(c)], 0);
                self.syntax = None;
                self.parse_in_background(cx);
                self.hl_cache = None;
                self.wrap_cols = None;
                self.disk_changed = false;
                cx.emit(EditorEvent::Changed);
                cx.notify();
            }
            Err(e) => cx.emit(EditorEvent::Error(format!("revert failed: {e}"))),
        }
    }

    // -- running commands ---------------------------------------------------

    fn run(&mut self, cmd: Command, cx: &mut Context<Self>) {
        if self.read_only && mutates(&cmd) {
            return;
        }
        // Vim normal/visual: the typing keys act like Vim's (Enter = down,
        // Backspace = left, Delete = x). Cmd shortcuts still work.
        if self.vim.as_ref().is_some_and(|v| v.mode() != VimMode::Insert) {
            let mapped = match cmd {
                Command::Newline => Some(vec!['j', '^']),
                Command::Backspace => Some(vec!['h']),
                Command::Delete => Some(vec!['x']),
                Command::Insert(_) | Command::Tab | Command::Backtab | Command::DeleteWordLeft | Command::DeleteWordRight | Command::DeleteToLineStart => Some(vec![]),
                _ => None,
            };
            if let Some(keys) = mapped {
                for k in keys {
                    self.vim_key(VimKey::Char(k), cx);
                }
                return;
            }
        }
        let paste = matches!(cmd, Command::Paste);
        self.apply(paste, cx, move |m, ctx| m.run(cmd, ctx));
    }

    /// Turn Vim mode on or off (Edit > Vim Mode or Settings, `[keys] vim`).
    pub fn set_vim(&mut self, on: bool, cx: &mut Context<Self>) {
        if on != self.vim.is_some() {
            self.vim = on.then(Vim::new);
            cx.notify();
        }
    }

    /// "NORMAL", "INSERT 2d"… for the modal's header.
    pub fn vim_status(&self) -> Option<String> {
        let v = self.vim.as_ref()?;
        let p = v.pending_text();
        Some(if p.is_empty() { v.mode().label().to_string() } else { format!("{} {p}", v.mode().label()) })
    }

    /// Feed a key to Vim; false = not Vim's (type it normally).
    fn vim_key(&mut self, key: VimKey, cx: &mut Context<Self>) -> bool {
        if self.read_only {
            return false;
        }
        let Some(mut vim) = self.vim.take() else { return false };
        let mut used = false;
        self.apply(false, cx, |m, ctx| match vim.key(m, key, ctx) {
            Some(o) => {
                used = true;
                o
            }
            None => Outcome::default(),
        });
        self.vim = Some(vim);
        cx.notify();
        used
    }

    /// Run `f` on the model with this buffer's context, then refresh syntax,
    /// find, scroll and the dirty dot.
    fn apply(&mut self, paste: bool, cx: &mut Context<Self>, f: impl FnOnce(&mut Editor, &mut Ctx) -> Outcome) {
        let clip = paste.then(|| cx.read_from_clipboard().and_then(|c| c.text())).flatten();
        let lang = self.language;
        let page_rows = self.bounds.map(|b| (b.size.height / self.line_h).floor() as usize).unwrap_or(30).max(1);
        // Bracket lookup and Expand Selection go through the syntax tree when there is one.
        let syntax = self.syntax.clone();
        let text = RopeText(self.model.rope().clone());
        let bracket = |b: usize| syntax.as_ref().and_then(|s| s.matching_bracket(&text, b));
        let expand = |r: Range<usize>| syntax.as_ref().and_then(|s| s.expand_range(&text, r));
        let mut ctx = Ctx {
            page_rows,
            clipboard_in: clip.as_deref(),
            clipboard_out: None,
            line_comment: lang.and_then(|l| l.line_comment()),
            block_comment: lang.and_then(|l| l.block_comment()),
            indent_unit: lang.map(|l| l.indent_unit()).unwrap_or("    "),
            matching_bracket: Some(&bracket),
            expand: Some(&expand),
        };
        let was_dirty = self.model.is_dirty();
        let out = f(&mut self.model, &mut ctx);
        if let Some((text, _whole_line)) = ctx.clipboard_out.take() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        if out.text_changed {
            self.after_edit(cx);
        }
        if out.reveal {
            self.reveal();
        }
        if out.text_changed || out.selections_changed {
            if out.text_changed && self.find.is_some() {
                self.refresh_find(cx);
            }
            cx.notify();
        }
        // The workspace only shows the dirty dot; cursor moves stay local, so
        // typing doesn't repaint the sidebar and the heatmap tree.
        if was_dirty != self.model.is_dirty() {
            cx.emit(EditorEvent::Changed);
        }
    }

    /// Apply the model's edits to the syntax tree now, reparse in the background.
    fn after_edit(&mut self, cx: &mut Context<Self>) {
        let edits = self.model.take_edits();
        self.hl_cache = None;
        // Still parsing the file: that parse restarts itself on a version change.
        let Some(s) = self.syntax.as_mut() else { return };
        for e in &edits {
            s.edit(&TextEdit {
                start_byte: e.start_byte,
                old_end_byte: e.old_end_byte,
                new_end_byte: e.new_end_byte,
                start_point: e.start_point,
                old_end_point: e.old_end_point,
                new_end_point: e.new_end_point,
            });
        }
        self.schedule_reparse(cx);
    }

    fn schedule_reparse(&mut self, cx: &mut Context<Self>) {
        let Some(mut state) = self.syntax.clone() else { return };
        let version = self.model.version();
        let text = RopeText(self.model.rope().clone());
        let bg = cx.background_executor().clone();
        self.reparse = Some(cx.spawn(async move |this, cx| {
            let state = bg
                .spawn(async move {
                    state.reparse(&text);
                    state
                })
                .await;
            this.update(cx, |this, cx| {
                if this.model.version() == version {
                    this.syntax = Some(state);
                    this.hl_cache = None;
                    cx.notify();
                } else {
                    // More edits landed meanwhile; parse again from the latest text.
                    this.schedule_reparse(cx);
                }
            })
            .ok();
        }));
    }

    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.run(Command::Insert(text.to_string()), cx);
    }

    pub fn goto_line(&mut self, line: usize, cx: &mut Context<Self>) {
        self.run(Command::GoToLine(line), cx);
        self.center_cursor();
        cx.notify();
    }

    pub fn set_cursor(&mut self, p: Point, cx: &mut Context<Self>) {
        let p = self.model.clamp(p);
        self.model.set_selections(vec![Selection::caret(p)], 0);
        self.center_cursor();
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    /// The file's outline for Go to Symbol (empty while the first parse runs).
    pub fn symbols(&self) -> Vec<qur_syntax::Symbol> {
        match &self.syntax {
            Some(s) => s.symbols(&RopeText(self.model.rope().clone())),
            None => Vec::new(),
        }
    }

    /// Select a byte range (a symbol's name) and center it.
    pub fn select_bytes(&mut self, r: Range<usize>, cx: &mut Context<Self>) {
        let (a, b) = (self.model.byte_to_point(r.start), self.model.byte_to_point(r.end));
        self.model.set_selections(vec![Selection { anchor: a, head: b, goal: None }], 0);
        self.center_cursor();
        cx.notify();
    }

    /// The selections, to put back after a preview (Go to Symbol's Esc).
    pub fn selections(&self) -> (Vec<Selection>, usize) {
        (self.model.selections().to_vec(), self.model.primary_index())
    }

    pub fn restore_selections(&mut self, (sels, primary): (Vec<Selection>, usize), cx: &mut Context<Self>) {
        self.model.set_selections(sels, primary);
        self.center_cursor();
        cx.notify();
    }

    /// Expand (or shrink) every selection by syntax node (Selection menu).
    pub fn expand_selection(&mut self, grow: bool, cx: &mut Context<Self>) {
        self.run(if grow { Command::ExpandSelection } else { Command::ShrinkSelection }, cx);
        // Scripted runs (QUR_DEBUG_ACTIONS) log where the selections went.
        if std::env::var_os("QUR_DEBUG_ACTIONS").is_some() {
            let rope = self.model.rope();
            let shown: Vec<String> = self
                .model
                .selections()
                .iter()
                .map(|s| {
                    let (a, b) = (rope.line_to_char(s.start().line) + s.start().col, rope.line_to_char(s.end().line) + s.end().col);
                    format!("{}:{}-{}:{} {:?}", s.start().line + 1, s.start().col + 1, s.end().line + 1, s.end().col + 1, rope.slice(a..b).to_string())
                })
                .collect();
            println!("SELECTION {}", shown.join(" | "));
        }
    }

    pub fn toggle_soft_wrap(&mut self, cx: &mut Context<Self>) {
        self.soft_wrap = !self.soft_wrap;
        self.wrap_cols = None;
        self.scroll_x = px(0.);
        cx.emit(EditorEvent::WrapToggled);
        cx.notify();
    }

    pub fn set_soft_wrap(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.soft_wrap != on {
            self.toggle_soft_wrap(cx);
        }
    }

    // -- scrolling ---------------------------------------------------------

    fn viewport_h(&self) -> Pixels {
        self.bounds.map(|b| b.size.height).unwrap_or(px(600.))
    }

    fn max_scroll_y(&self) -> Pixels {
        // Allow scrolling so the last line can sit near the top (VS Code style).
        let rows = self.model.wrap().row_count() as f32;
        (self.line_h * rows - self.line_h * 3.).max(px(0.))
    }

    fn reveal(&mut self) {
        let head = self.model.primary().head;
        let (row, vcol) = self.model.wrap().point_to_row(self.model.rope(), head);
        let y = self.line_h * row as f32;
        let h = self.viewport_h();
        let margin = self.line_h * 2.;
        if y < self.scroll_y + margin {
            self.scroll_y = (y - margin).max(px(0.));
        } else if y + self.line_h > self.scroll_y + h - margin {
            self.scroll_y = (y + self.line_h - h + margin).min(self.max_scroll_y().max(y));
        }
        if !self.soft_wrap {
            let x = self.cell_w * vcol as f32;
            let w = self.bounds.map(|b| b.size.width).unwrap_or(px(800.)) - self.text_x - self.cell_w * 4.;
            if x < self.scroll_x {
                self.scroll_x = (x - self.cell_w * 4.).max(px(0.));
            } else if x > self.scroll_x + w {
                self.scroll_x = x - w;
            }
        }
    }

    fn center_cursor(&mut self) {
        let head = self.model.primary().head;
        let (row, _) = self.model.wrap().point_to_row(self.model.rope(), head);
        self.scroll_y = (self.line_h * row as f32 - self.viewport_h() / 2.).clamp(px(0.), self.max_scroll_y());
    }

    // -- find ----------------------------------------------------------------

    pub fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let seed = {
            let p = self.model.primary();
            (!p.is_empty() && p.anchor.line == p.head.line).then(|| self.model.selected_text())
        };
        if self.find.is_none() {
            let query = cx.new(|cx| TextInput::new("Find", cx));
            let sub = cx.subscribe_in(&query, window, |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Changed => this.refresh_find(cx),
                InputEvent::Confirm { alt, .. } => this.find_step(!*alt, cx),
                InputEvent::Cancel => this.close_find(window, cx),
                _ => {}
            });
            self.find = Some(FindBar { query, replace: None, opts: FindOptions::default(), error: None, _subs: vec![sub] });
        }
        if replace && self.find.as_ref().is_some_and(|f| f.replace.is_none()) {
            let r = cx.new(|cx| TextInput::new("Replace", cx));
            let sub = cx.subscribe_in(&r, window, |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Confirm { all: true, .. } => this.replace_all(cx),
                InputEvent::Confirm { .. } => this.replace_one(cx),
                InputEvent::Cancel => this.close_find(window, cx),
                _ => {}
            });
            let f = self.find.as_mut().unwrap();
            f.replace = Some(r);
            f._subs.push(sub);
        }
        let q = self.find.as_ref().unwrap().query.clone();
        if let Some(seed) = &seed {
            q.update(cx, |i, cx| i.set_text(seed, cx));
        }
        let target = if replace && seed.is_some() {
            self.find.as_ref().unwrap().replace.as_ref().unwrap().read(cx).focus.clone()
        } else {
            q.read(cx).focus.clone()
        };
        window.focus(&target, cx);
        self.refresh_find(cx);
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        self.model.clear_find();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn refresh_find(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.find.as_mut() else { return };
        let q = f.query.read(cx).text.clone();
        f.error = if q.is_empty() {
            self.model.clear_find();
            None
        } else {
            self.model.set_find(&q, f.opts).err()
        };
        cx.notify();
    }

    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let out = if forward { self.model.find_next() } else { self.model.find_prev() };
        if out.reveal {
            self.reveal();
        }
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn replace_text(&self, cx: &App) -> String {
        self.find.as_ref().and_then(|f| f.replace.as_ref()).map(|r| r.read(cx).text.clone()).unwrap_or_default()
    }

    fn replace_one(&mut self, cx: &mut Context<Self>) {
        let r = self.replace_text(cx);
        let out = self.model.replace_one(&r);
        if out.text_changed {
            self.after_edit(cx);
        }
        if out.reveal {
            self.reveal();
        }
        self.refresh_find(cx);
        cx.emit(EditorEvent::Changed);
    }

    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let r = self.replace_text(cx);
        if self.model.replace_all(&r) > 0 {
            self.after_edit(cx);
            cx.emit(EditorEvent::Changed);
        }
        self.refresh_find(cx);
    }

    fn toggle_opt(&mut self, f: impl FnOnce(&mut FindOptions), cx: &mut Context<Self>) {
        if let Some(bar) = self.find.as_mut() {
            f(&mut bar.opts);
            self.refresh_find(cx);
        }
    }

    pub fn find_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            return self.open_find(false, window, cx);
        }
        self.find_step(true, cx)
    }

    pub fn find_prev(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            return self.open_find(false, window, cx);
        }
        self.find_step(false, cx)
    }

    pub fn use_selection_for_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.primary().is_empty() {
            let head = self.cursor();
            self.model.select_word_at(head);
        }
        let text = self.model.selected_text();
        if text.is_empty() || text.contains('\n') {
            return;
        }
        let had = self.find.is_some();
        self.open_find(false, window, cx);
        if let Some(f) = &self.find {
            f.query.update(cx, |i, cx| i.set_text(&text, cx));
        }
        if !had {
            window.focus(&self.focus, cx);
        }
        self.refresh_find(cx);
    }

    // -- mouse ---------------------------------------------------------------

    fn hit(&self, p: gpui::Point<Pixels>) -> Point {
        let Some(b) = self.bounds else { return self.cursor() };
        let y = p.y - b.top() + self.scroll_y;
        let row = (y / self.line_h).floor().max(0.) as usize;
        let wrap = self.model.wrap();
        if row >= wrap.row_count() {
            let last = self.model.line_count().saturating_sub(1);
            return self.model.clamp(Point { line: last, col: usize::MAX });
        }
        let x = (p.x - b.left() - self.text_x + self.scroll_x).max(px(0.));
        let col = (x / self.cell_w).round() as usize;
        wrap.row_to_point(self.model.rope(), row, col)
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.last_user_input = Some(Instant::now());
        let p = self.hit(ev.position);
        match ev.click_count {
            2 => self.model.select_word_at(p),
            3 => self.model.select_line_at(p),
            _ => self.model.click(p, ev.modifiers.shift, ev.modifiers.alt),
        }
        self.dragging = ev.click_count == 1 && !ev.modifiers.alt;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.dragging && ev.pressed_button == Some(MouseButton::Left) {
            let p = self.hit(ev.position);
            self.model.drag_to(p);
            // Auto-scroll while dragging past the edges.
            if let Some(b) = self.bounds {
                if ev.position.y < b.top() {
                    self.scroll_y = (self.scroll_y - self.line_h).max(px(0.));
                } else if ev.position.y > b.bottom() {
                    self.scroll_y = (self.scroll_y + self.line_h).min(self.max_scroll_y());
                }
            }
            cx.notify();
        } else {
            self.dragging = false;
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.dragging {
            self.dragging = false;
            cx.emit(EditorEvent::Changed);
        }
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.last_user_input = Some(Instant::now());
        let d = ev.delta.pixel_delta(self.line_h);
        self.scroll_y = (self.scroll_y - d.y).clamp(px(0.), self.max_scroll_y());
        if !self.soft_wrap {
            let max_x = (self.content_w - self.bounds.map(|b| b.size.width / 2.).unwrap_or(px(400.))).max(px(0.));
            self.scroll_x = (self.scroll_x - d.x).clamp(px(0.), max_x);
        }
        cx.notify();
    }

    // -- bench hooks (scripts/budget.sh) --------------------------------------

    fn after_paint(&mut self, window_handle: gpui::AnyWindowHandle, cx: &mut Context<Self>) {
        let b = &mut self.bench;
        if b.done || !(b.autoscroll || b.autotype) {
            return;
        }
        if !b.first_frame {
            b.first_frame = true;
            if b.autotype {
                let line = (self.model.line_count() / 2).max(1);
                self.goto_line(line, cx);
            }
        }
        let entity = cx.entity();
        if self.bench.autoscroll {
            let now = Instant::now();
            if let Some(prev) = self.bench.last_frame.replace(now) {
                self.bench.scroll.push((now - prev).as_secs_f64() * 1000.0);
            }
            if self.scroll_y >= self.max_scroll_y() {
                report("scroll", &self.bench.scroll);
                self.bench.done = true;
                return cx.quit();
            }
            window_handle
                .update(cx, |_, window, _| {
                    window.on_next_frame(move |_, cx| {
                        entity.update(cx, |e, cx| {
                            e.scroll_y = (e.scroll_y + e.line_h * 20.).min(e.max_scroll_y());
                            cx.notify();
                        })
                    })
                })
                .ok();
        } else if self.bench.autotype {
            if let Some(t0) = self.bench.pending_key.take() {
                self.bench.typing.push(t0.elapsed().as_secs_f64() * 1000.0);
                self.bench.key_outstanding = false;
            }
            if self.bench.key_outstanding {
                return;
            }
            if self.bench.keys_sent >= 200 {
                report("type", &self.bench.typing);
                self.bench.done = true;
                return cx.quit();
            }
            let ch = ["a", "b", "c", "d", "e", "f", "g", "h"][self.bench.keys_sent % 8];
            self.bench.keys_sent += 1;
            self.bench.key_outstanding = true;
            window_handle
                .update(cx, |_, window, _| {
                    window.on_next_frame(move |window, cx| {
                        entity.update(cx, |e, _| e.bench.pending_key = Some(Instant::now()));
                        window.dispatch_keystroke(gpui::Keystroke::parse(ch).unwrap(), cx);
                    })
                })
                .ok();
        }
    }
}

fn report(prefix: &str, samples: &[f64]) {
    if samples.is_empty() {
        return;
    }
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    println!("METRIC {prefix}_n={}", v.len());
    println!("METRIC {prefix}_p50_ms={:.2}", pct(0.5));
    println!("METRIC {prefix}_p99_ms={:.2}", pct(0.99));
    println!("METRIC {prefix}_max_ms={:.2}", v[v.len() - 1]);
}

// -- actions ----------------------------------------------------------------

macro_rules! simple {
    ($($fn:ident: $action:path => $cmd:expr;)*) => {
        impl EditorView {
            $(fn $fn(&mut self, _: &$action, _: &mut Window, cx: &mut Context<Self>) {
                self.run($cmd, cx)
            })*
        }
    };
}

simple! {
    a_left: ea::Left => Command::Left { select: false };
    a_right: ea::Right => Command::Right { select: false };
    a_up: ea::Up => Command::Up { select: false };
    a_down: ea::Down => Command::Down { select: false };
    a_sel_left: ea::SelectLeft => Command::Left { select: true };
    a_sel_right: ea::SelectRight => Command::Right { select: true };
    a_sel_up: ea::SelectUp => Command::Up { select: true };
    a_sel_down: ea::SelectDown => Command::Down { select: true };
    a_word_left: ea::WordLeft => Command::WordLeft { select: false };
    a_word_right: ea::WordRight => Command::WordRight { select: false };
    a_sel_word_left: ea::SelectWordLeft => Command::WordLeft { select: true };
    a_sel_word_right: ea::SelectWordRight => Command::WordRight { select: true };
    a_line_start: ea::LineStart => Command::LineStart { select: false };
    a_line_end: ea::LineEnd => Command::LineEnd { select: false };
    a_sel_line_start: ea::SelectLineStart => Command::LineStart { select: true };
    a_sel_line_end: ea::SelectLineEnd => Command::LineEnd { select: true };
    a_doc_start: ea::DocStart => Command::DocStart { select: false };
    a_doc_end: ea::DocEnd => Command::DocEnd { select: false };
    a_sel_doc_start: ea::SelectDocStart => Command::DocStart { select: true };
    a_sel_doc_end: ea::SelectDocEnd => Command::DocEnd { select: true };
    a_page_up: ea::PageUp => Command::PageUp { select: false };
    a_page_down: ea::PageDown => Command::PageDown { select: false };
    a_sel_page_up: ea::SelectPageUp => Command::PageUp { select: true };
    a_sel_page_down: ea::SelectPageDown => Command::PageDown { select: true };
    a_backspace: ea::Backspace => Command::Backspace;
    a_delete: ea::Delete => Command::Delete;
    a_del_word_left: ea::DeleteWordLeft => Command::DeleteWordLeft;
    a_del_word_right: ea::DeleteWordRight => Command::DeleteWordRight;
    a_del_line_start: ea::DeleteToLineStart => Command::DeleteToLineStart;
    a_del_line: ea::DeleteLine => Command::DeleteLine;
    a_move_up: ea::MoveLineUp => Command::MoveLineUp;
    a_move_down: ea::MoveLineDown => Command::MoveLineDown;
    a_copy_up: ea::CopyLineUp => Command::CopyLineUp;
    a_copy_down: ea::CopyLineDown => Command::CopyLineDown;
    a_line_below: ea::InsertLineBelow => Command::InsertLineBelow;
    a_line_above: ea::InsertLineAbove => Command::InsertLineAbove;
    a_indent: ea::Indent => Command::Indent;
    a_outdent: ea::Outdent => Command::Outdent;
    a_tab: ea::Tab => Command::Tab;
    a_backtab: ea::ShiftTab => Command::Backtab;
    a_comment: ea::ToggleComment => Command::ToggleLineComment;
    a_block_comment: ea::ToggleBlockComment => Command::ToggleBlockComment;
    a_trim: ea::TrimWhitespace => Command::TrimTrailingWhitespace;
    a_select_line: ea::SelectLine => Command::SelectLine;
    a_next_match: ea::AddNextMatch => Command::AddNextMatch;
    a_all_matches: ea::SelectAllMatches => Command::SelectAllMatches;
    a_cursor_above: ea::AddCursorAbove => Command::AddCursorAbove;
    a_cursor_below: ea::AddCursorBelow => Command::AddCursorBelow;
    a_bracket: ea::JumpToBracket => Command::JumpToMatchingBracket;
    a_newline: ea::Newline => Command::Newline;
    a_undo: edit::Undo => Command::Undo;
    a_redo: edit::Redo => Command::Redo;
    a_cut: edit::Cut => Command::Cut;
    a_copy: edit::Copy => Command::Copy;
    a_paste: edit::Paste => Command::Paste;
    a_select_all: edit::SelectAll => Command::SelectAll;
}

impl EditorView {
    /// Esc steps out one layer at a time: find bar, extra cursors, then the modal.
    fn a_escape(&mut self, _: &ea::Escape, window: &mut Window, cx: &mut Context<Self>) {
        // Vim first: leave insert/visual or drop a pending command. Plain
        // normal mode falls through, so Esc still closes the modal.
        if let Some(v) = &self.vim
            && (v.mode() != VimMode::Normal || !v.pending_text().is_empty())
        {
            self.vim_key(VimKey::Esc, cx);
            return;
        }
        if self.find.is_some() {
            self.close_find(window, cx);
        } else if self.model.selections().len() > 1 {
            self.run(Command::SingleCursor, cx);
        } else {
            cx.emit(EditorEvent::CloseRequested);
        }
    }

    fn a_expand(&mut self, _: &crate::commands::qur::ExpandSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.expand_selection(true, cx);
    }
    fn a_shrink(&mut self, _: &crate::commands::qur::ShrinkSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.expand_selection(false, cx);
    }

    fn a_save(&mut self, _: &ea::Save, _: &mut Window, cx: &mut Context<Self>) {
        self.save(cx);
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        match self.model.save() {
            Ok(()) => {
                // Your save, not an agent's edit: the heatmap leaves it out.
                if let Some(p) = self.model.path() {
                    qur_heat::own_writes().lock().unwrap_or_else(|e| e.into_inner()).note(p, std::time::Instant::now());
                }
                cx.emit(EditorEvent::Changed)
            }
            Err(e) => cx.emit(EditorEvent::Error(format!("Couldn't save {}: {e}", self.title()))),
        }
        cx.notify();
    }

    fn a_case(&mut self, _: &ea::ToggleCase, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_opt(|o| o.case_sensitive = !o.case_sensitive, cx)
    }
    fn a_word(&mut self, _: &ea::ToggleWholeWord, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_opt(|o| o.whole_word = !o.whole_word, cx)
    }
    fn a_regex(&mut self, _: &ea::ToggleRegex, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_opt(|o| o.regex = !o.regex, cx)
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Committed text goes in at every cursor. While an IME composes, its
/// marked text is drawn underlined at the primary cursor (prepaint) and
/// only enters the buffer when committed. The "document" the IME sees is the
/// primary cursor's line, in UTF-16 offsets, with the marked text spliced in
/// at the cursor.
impl EditorView {
    /// UTF-16 offset of the primary cursor in its line.
    fn caret_utf16(&self) -> usize {
        let h = self.cursor();
        let line = self.model.rope().line(h.line);
        line.slice(..h.col.min(line.len_chars())).chars().map(char::len_utf16).sum()
    }

    /// The primary cursor's line (no newline), with the marked text at the cursor.
    fn ime_line(&self) -> String {
        let h = self.cursor();
        let mut line: String = self.model.rope().line(h.line).chars().filter(|&c| c != '\n' && c != '\r').collect();
        if let Some(m) = &self.marked {
            let at = line.char_indices().nth(h.col).map_or(line.len(), |(i, _)| i);
            line.insert_str(at, &m.text);
        }
        line
    }
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

impl EntityInputHandler for EditorView {
    fn text_for_range(&mut self, r: Range<usize>, adjusted: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let units: Vec<u16> = self.ime_line().encode_utf16().collect();
        let r = r.start.min(units.len())..r.end.min(units.len());
        *adjusted = Some(r.clone());
        Some(String::from_utf16_lossy(&units[r]))
    }
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let u = self.caret_utf16();
        let range = match &self.marked {
            Some(m) => {
                let n = utf16_len(&m.text);
                let sel = m.selected.clone().unwrap_or(n..n);
                u + sel.start.min(n)..u + sel.end.min(n)
            }
            None => u..u,
        };
        Some(UTF16Selection { range, reversed: false })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let m = self.marked.as_ref()?;
        let u = self.caret_utf16();
        Some(u..u + utf16_len(&m.text))
    }
    /// The IME gave up composing (focus moved…): keep what was typed.
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(m) = self.marked.take() {
            if !m.text.is_empty() && !self.read_only {
                self.insert_text(&m.text, cx);
            }
            cx.notify();
        }
    }
    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let composing = self.marked.take().is_some();
        if composing {
            cx.notify();
        }
        if text.is_empty() {
            return;
        }
        // Vim normal/visual: typed keys are commands, not text.
        if self.vim.as_ref().is_some_and(|v| v.mode() != VimMode::Insert) && !self.read_only {
            for c in text.chars() {
                self.vim_key(VimKey::Char(c), cx);
            }
            return;
        }
        // Press-and-hold accents replace the letter before the caret (one cursor).
        let u = self.caret_utf16();
        if let Some(r) = range
            && !composing
            && !self.read_only
            && self.model.selections().len() == 1
            && self.model.primary().is_empty()
            && r.end == u
            && r.start < u
        {
            let h = self.cursor();
            let line = self.model.rope().line(h.line);
            let mut acc = 0;
            let mut col = 0;
            for c in line.chars() {
                if acc >= r.start {
                    break;
                }
                acc += c.len_utf16();
                col += 1;
            }
            let from = Point { line: h.line, col };
            self.model.set_selections(vec![Selection { anchor: from, head: h, goal: None }], 0);
        }
        self.insert_text(text, cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Nothing to compose into: Vim normal mode and read-only Live views.
        if self.read_only || self.vim.as_ref().is_some_and(|v| v.mode() != VimMode::Insert) {
            return self.replace_text_in_range(range, text, window, cx);
        }
        self.marked = (!text.is_empty()).then(|| Marked { text: text.to_string(), selected });
        self.reveal();
        cx.notify();
    }
    fn bounds_for_range(&mut self, r: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let (row, mut col) = self.model.wrap().point_to_row(self.model.rope(), self.cursor());
        // Inside the marked text (the IME's candidate window follows the clause).
        if let Some(m) = &self.marked {
            let into = r.start.saturating_sub(self.caret_utf16());
            let mut acc = 0;
            for c in m.text.chars() {
                if acc >= into {
                    break;
                }
                acc += c.len_utf16();
                col += char_width(c);
            }
        }
        let y = bounds.top() + self.line_h * row as f32 - self.scroll_y;
        let x = bounds.left() + self.text_x + self.cell_w * col as f32 - self.scroll_x;
        Some(Bounds::new(point(x, y), size(px(2.), self.line_h)))
    }
    fn character_index_for_point(&mut self, _: gpui::Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        None
    }
}

// ---------------------------------------------------------------------------
// Element: shapes and paints only the visible visual rows
// ---------------------------------------------------------------------------

struct EditorElement {
    view: Entity<EditorView>,
}

struct Row {
    y: Pixels,
    line: usize,
    chars: Range<usize>,
    /// Visual column of each char boundary in the row (len = chars.len() + 1).
    vcols: Vec<usize>,
    /// True when this row ends its buffer line (the newline belongs here).
    line_end: bool,
    gutter: Option<ShapedLine>,
    text: ShapedLine,
}

struct Prepaint {
    rows: Vec<Row>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

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

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let look = Look::of(window, cx);
        let theme = look.theme.clone();
        let fs = look.code_px();
        let lh = look.code_line_h();
        let code_font = font(look.code_font.clone());
        let ts = window.text_system().clone();
        let cell_w = ts.advance(ts.resolve_font(&code_font), fs, 'm').map(|s| s.width).unwrap_or(px(8.));

        // Layout: 8px | right-aligned line numbers | 8px, then 8px before text.
        let digits = self.view.read(cx).model.line_count().to_string().len().max(3);
        let gutter_w = look.z(8.) * 2. + cell_w * digits as f32;
        let text_x = gutter_w + look.z(8.);
        let cols = ((bounds.size.width - text_x - look.z(12.)) / cell_w).floor().max(16.) as usize;

        self.view.update(cx, |v, _| {
            let want = v.soft_wrap.then_some(cols);
            if v.wrap_cols != want || v.cell_w != cell_w || v.line_h != lh {
                // Keep the top line anchored across re-wraps (resize, zoom).
                let top_row = if v.line_h > px(0.) { (v.scroll_y / v.line_h).floor() as usize } else { 0 };
                let top = v.model.wrap().row_to_point(v.model.rope(), top_row, 0);
                v.model.set_wrap_width(want);
                v.wrap_cols = want;
                v.line_h = lh;
                v.cell_w = cell_w;
                let (row, _) = v.model.wrap().point_to_row(v.model.rope(), Point { line: top.line, col: 0 });
                v.scroll_y = lh * row as f32;
            }
            v.text_x = text_x;
            v.bounds = Some(bounds);
            v.scroll_y = v.scroll_y.clamp(px(0.), v.max_scroll_y());
        });

        let v = self.view.read(cx);
        let rope = v.model.rope();
        let wrap = v.model.wrap();
        let first = (v.scroll_y / lh).floor() as usize;
        let offset = v.scroll_y - lh * first as f32;
        let count = (bounds.size.height / lh).ceil() as usize + 1;
        let rows = wrap.rows(first..first + count);

        // Highlights for the visible byte range (cached per text version).
        let mut new_cache = None;
        let spans: Vec<HighlightSpan> = match (&v.syntax, rows.first(), rows.last()) {
            (Some(s), Some(a), Some(b)) => {
                let start = rope.line_to_byte(a.line) + rope.line(a.line).char_to_byte(a.chars.start);
                let end = rope.line_to_byte(b.line) + rope.line(b.line).char_to_byte(b.chars.end);
                match &v.hl_cache {
                    Some((ver, r, sp)) if *ver == v.model.version() && r.start <= start && r.end >= end => sp.clone(),
                    _ => {
                        let sp = s.highlights(&RopeText(rope.clone()), start..end);
                        new_cache = Some((v.model.version(), start..end, sp.clone()));
                        sp
                    }
                }
            }
            _ => Vec::new(),
        };
        let styles: Vec<qur_theme::Style> = HIGHLIGHT_NAMES.iter().map(|n| theme.syntax_style(n)).collect();
        let default_color = look.c(theme.editor.text);
        let primary_line = v.model.primary().head.line;
        let primary_head = v.model.primary().head;
        let tab_w = v.tab_width.max(1);
        // View > Show Whitespace: spaces as ·, tabs as →, line ends as ¬.
        let (ws, ws_color) = (look.show_whitespace, look.c(theme.editor.whitespace));

        let mut out = Vec::with_capacity(rows.len());
        let mut span_ix = 0;
        let mut max_vcol = 0usize;
        for (i, r) in rows.iter().enumerate() {
            let line = rope.line(r.line);
            let line_byte0 = rope.line_to_byte(r.line);
            let line_chars = line.len_chars();
            let has_nl = line_chars > 0 && matches!(line.char(line_chars - 1), '\n');
            let content_end = if has_nl { line_chars - 1 } else { line_chars };
            let content_end = if content_end > 0 && line.char(content_end - 1) == '\r' { content_end - 1 } else { content_end };
            let end = r.chars.end.min(content_end);
            let line_end = r.chars.end >= content_end;
            let mut text = String::new();
            let mut runs: Vec<TextRun> = Vec::new();
            let mut vcols = Vec::with_capacity(end.saturating_sub(r.chars.start) + 1);
            // Visual column at the row start: wrapped rows start at 0.
            let mut vcol = 0usize;
            let mut byte = line_byte0 + line.char_to_byte(r.chars.start.min(line_chars));
            // The IME's marked text goes in before the char at the primary
            // cursor; that cursor (and everything after) shifts right.
            let ime_col = v.marked.as_ref().filter(|_| {
                let h = primary_head;
                h.line == r.line && h.col >= r.chars.start && (h.col < end || (h.col == end && line_end))
            });
            let ime = |text: &mut String, runs: &mut Vec<TextRun>, vcol: &mut usize| {
                let Some(m) = ime_col else { return };
                let mut u = 0;
                for c in m.text.chars() {
                    let in_clause = m.selected.as_ref().is_some_and(|s| !s.is_empty() && s.contains(&u));
                    u += c.len_utf16();
                    let mut b = [0u8; 4];
                    let piece = c.encode_utf8(&mut b);
                    let underline = gpui::UnderlineStyle { thickness: px(if in_clause { 2. } else { 1. }), color: Some(default_color), wavy: false };
                    match runs.last_mut() {
                        Some(run) if run.underline == Some(underline) => run.len += piece.len(),
                        _ => runs.push(TextRun {
                            len: piece.len(),
                            font: code_font.clone(),
                            color: default_color,
                            background_color: None,
                            underline: Some(underline),
                            strikethrough: None,
                        }),
                    }
                    text.push_str(piece);
                    *vcol += char_width(c);
                }
            };
            let mut col = r.chars.start;
            for c in line.slice(r.chars.start.min(end)..end).chars() {
                if ime_col.is_some() && col == primary_head.col {
                    ime(&mut text, &mut runs, &mut vcol);
                }
                col += 1;
                vcols.push(vcol);
                while span_ix < spans.len() && spans[span_ix].range.end <= byte {
                    span_ix += 1;
                }
                let style = spans
                    .get(span_ix)
                    .filter(|s| s.range.start <= byte)
                    .map(|s| styles[s.highlight])
                    .unwrap_or_default();
                let (piece, w): (std::borrow::Cow<str>, usize) = if c == '\t' {
                    let n = tab_w - (vcol % tab_w);
                    (if ws { format!("→{}", " ".repeat(n - 1)).into() } else { " ".repeat(n).into() }, n)
                } else if let Some(sym) = control_picture(c, ws) {
                    (sym.into(), 1)
                } else {
                    let mut b = [0u8; 4];
                    (c.encode_utf8(&mut b).to_string().into(), char_width(c))
                };
                let color = if ws && (c == ' ' || c == '\t') { ws_color } else { style.color.map(|c| look.c(c)).unwrap_or(default_color) };
                let weight = if style.bold { gpui::FontWeight::BOLD } else { gpui::FontWeight::NORMAL };
                let fstyle = if style.italic { gpui::FontStyle::Italic } else { gpui::FontStyle::Normal };
                match runs.last_mut() {
                    Some(run) if run.color == color && run.font.weight == weight && run.font.style == fstyle && run.underline.is_none() => {
                        run.len += piece.len()
                    }
                    _ => runs.push(TextRun {
                        len: piece.len(),
                        font: gpui::Font { weight, style: fstyle, ..code_font.clone() },
                        color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }),
                }
                text.push_str(&piece);
                vcol += w;
                byte += c.len_utf8();
            }
            if ime_col.is_some() && col == primary_head.col {
                ime(&mut text, &mut runs, &mut vcol);
            }
            vcols.push(vcol);
            max_vcol = max_vcol.max(vcol);
            if ws && line_end && has_nl {
                text.push('¬');
                runs.push(TextRun { len: '¬'.len_utf8(), font: code_font.clone(), color: ws_color, background_color: None, underline: None, strikethrough: None });
            }
            let shaped = ts.shape_line(SharedString::from(text), fs, &runs, None);
            let gutter = r.first.then(|| {
                let num = SharedString::from(format!("{:>w$}", r.line + 1, w = digits));
                let color = if r.line == primary_line { theme.editor.gutter_text_active } else { theme.editor.gutter_text };
                let run = TextRun { len: num.len(), font: code_font.clone(), color: look.c(color), background_color: None, underline: None, strikethrough: None };
                ts.shape_line(num, fs, &[run], None)
            });
            out.push(Row { y: bounds.top() + lh * i as f32 - offset, line: r.line, chars: r.chars.start..end, vcols, line_end, gutter, text: shaped });
        }
        let content_w = cell_w * max_vcol as f32;
        self.view.update(cx, |v, _| {
            v.content_w = v.content_w.max(content_w);
            if new_cache.is_some() {
                v.hl_cache = new_cache;
            }
        });
        Prepaint { rows: out }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        pre: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let look = Look::of(window, cx);
        let ed = look.theme.editor.clone();
        let ui = look.theme.ui.clone();
        let v = self.view.read(cx);
        let focus = v.focus.clone();
        let focused = focus.is_focused(window);
        let lh = v.line_h;
        let cw = v.cell_w;
        let block = v.vim.as_ref().is_some_and(|vim| vim.block_cursor());
        let gutter_x = bounds.left();
        let tx = bounds.left() + v.text_x - v.scroll_x;
        let text_clip = Bounds::from_corners(point(bounds.left() + v.text_x - look.z(4.), bounds.top()), bounds.bottom_right());

        let mut under = Vec::new();
        let mut carets = Vec::new();
        let sels = v.model.selections();
        let primary = v.model.primary();
        let x_at = |row: &Row, col: usize| -> Pixels {
            let i = col.saturating_sub(row.chars.start).min(row.vcols.len() - 1);
            tx + cw * row.vcols[i] as f32
        };
        // Find matches, by line.
        let (first_line, last_line) = (pre.rows.first().map(|r| r.line).unwrap_or(0), pre.rows.last().map(|r| r.line).unwrap_or(0));
        let matches = if v.find.is_some() { v.model.find_matches_in_lines(first_line..last_line + 1) } else { Vec::new() };
        for row in &pre.rows {
            let row_rect = |x0: Pixels, x1: Pixels| Bounds::from_corners(point(x0, row.y), point(x1, row.y + lh));
            if let Some((r, accent)) = &v.agent_mark
                && r.contains(&row.line)
            {
                let mut tint = look.c(*accent);
                tint.a = 0.16;
                under.push(fill(Bounds::new(point(bounds.left(), row.y), size(bounds.size.width, lh)), tint));
                under.push(fill(Bounds::new(point(bounds.left(), row.y), size(px(3.), lh)), look.c(*accent)));
            }
            if primary.is_empty() && sels.len() == 1 && row.line == primary.head.line {
                under.push(fill(Bounds::new(point(bounds.left(), row.y), size(bounds.size.width, lh)), look.c(ed.current_line)));
            }
            for (l, r) in &matches {
                if *l != row.line {
                    continue;
                }
                let (s, e) = (r.start.max(row.chars.start), r.end.min(row.chars.end));
                if s < e || (s == e && r.is_empty()) {
                    let active = primary.start() == (Point { line: *l, col: r.start }) && primary.end() == (Point { line: *l, col: r.end });
                    under.push(fill(row_rect(x_at(row, s), x_at(row, e)), look.c(if active { ed.search_match_active } else { ed.search_match })));
                }
            }
            for sel in sels {
                let (a, b) = (sel.start(), sel.end());
                if !sel.is_empty() && row.line >= a.line && row.line <= b.line {
                    let s = if row.line == a.line { a.col.max(row.chars.start) } else { row.chars.start };
                    let e = if row.line == b.line { b.col.min(row.chars.end) } else { row.chars.end };
                    let starts_after = row.line == a.line && a.col > row.chars.end && !row.line_end;
                    let ends_before = row.line == b.line && b.col < row.chars.start;
                    if !starts_after && !ends_before && s <= e {
                        let mut x1 = x_at(row, e);
                        if row.line < b.line && row.line_end {
                            x1 += cw; // the newline
                        }
                        let color = if focused { ui.selection } else { ui.selection_inactive };
                        under.push(fill(row_rect(x_at(row, s), x1), look.c(color)));
                    }
                }
                let h = sel.head;
                let in_row = h.line == row.line
                    && h.col >= row.chars.start
                    && (h.col < row.chars.end || row.line_end || (h.col == row.chars.end && row.chars.is_empty()));
                if focused && in_row {
                    let mut c = look.c(ed.cursor);
                    let w = if block {
                        // Vim normal/visual: a translucent block over the char.
                        c.a *= 0.45;
                        cw
                    } else {
                        px(2.)
                    };
                    carets.push(fill(Bounds::new(point(x_at(row, h.col), row.y), size(w, lh)), c));
                }
            }
        }

        window.handle_input(&focus, ElementInputHandler::new(bounds, self.view.clone()), cx);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, look.surface(look.theme.ui.editor_bg)));
            for q in under {
                window.paint_quad(q);
            }
            for row in &pre.rows {
                if let Some(g) = &row.gutter {
                    g.paint(point(gutter_x + look.z(8.), row.y), lh, gpui::TextAlign::Left, None, window, cx).ok();
                }
            }
            window.with_content_mask(Some(ContentMask { bounds: text_clip }), |window| {
                for row in &pre.rows {
                    row.text.paint(point(tx, row.y), lh, gpui::TextAlign::Left, None, window, cx).ok();
                }
                for c in carets {
                    window.paint_quad(c);
                }
            });
        });
        let view = self.view.clone();
        let handle = window.window_handle();
        cx.defer(move |cx| view.update(cx, |v, cx| v.after_paint(handle, cx)));
    }
}

/// Commands that change the text (ignored in a read-only Live view).
fn mutates(cmd: &Command) -> bool {
    !matches!(
        cmd,
        Command::Left { .. } | Command::Right { .. } | Command::Up { .. } | Command::Down { .. }
            | Command::WordLeft { .. } | Command::WordRight { .. } | Command::LineStart { .. }
            | Command::LineEnd { .. } | Command::DocStart { .. } | Command::DocEnd { .. }
            | Command::PageUp { .. } | Command::PageDown { .. } | Command::GoToLine(_)
            | Command::SelectAll | Command::SelectLine | Command::SingleCursor | Command::AddNextMatch
            | Command::SelectAllMatches | Command::AddCursorAbove | Command::AddCursorBelow
            | Command::JumpToMatchingBracket | Command::ExpandSelection | Command::ShrinkSelection
            | Command::Copy
    )
}

/// The line range (in the new text) that differs from the old text.
fn changed_lines(old: &str, new: &str) -> Range<usize> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let top = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let bottom = a.iter().rev().zip(b.iter().rev()).take_while(|(x, y)| x == y).count();
    let end = b.len().saturating_sub(bottom).max(top + 1).min(b.len().max(1));
    top.min(end.saturating_sub(1))..end
}

/// What a char is drawn as when it isn't itself: a space as · (Show
/// Whitespace), control characters (a binary opened as text) as their
/// Control Pictures symbol, one cell wide.
fn control_picture(c: char, ws: bool) -> Option<&'static str> {
    const C0: [&str; 32] = [
        "␀", "␁", "␂", "␃", "␄", "␅", "␆", "␇", "␈", "␉", "␊", "␋", "␌", "␍", "␎", "␏", "␐", "␑", "␒", "␓", "␔", "␕", "␖", "␗", "␘", "␙", "␚", "␛", "␜", "␝", "␞", "␟",
    ];
    match c {
        ' ' if ws => Some("·"),
        '\0'..='\x1F' => Some(C0[c as usize]),
        '\x7F' => Some("␡"),
        _ => None,
    }
}

/// Display width in cells (CJK and emoji are 2 wide).
fn char_width(c: char) -> usize {
    let u = c as u32;
    let wide = matches!(u,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF |
        0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 |
        0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD);
    if wide { 2 } else { 1 }
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let find_bar = self.find.as_ref().map(|f| {
            let field = |input: &Entity<TextInput>| {
                div()
                    .flex_1()
                    .px(look.z(6.))
                    .border_1()
                    .border_color(look.c(ui.border))
                    .rounded(look.z(4.))
                    .child(input.clone())
            };
            let toggle = |label: &'static str, on: bool, id: &'static str, f: fn(&mut FindOptions)| {
                div()
                    .id(id)
                    .px(look.z(5.))
                    .rounded(look.z(3.))
                    .text_size(look.text(-2.))
                    .cursor_pointer()
                    .text_color(look.c(if on { ui.text } else { ui.text_muted }))
                    .when(on, |d| d.bg(look.c(ui.active_bg)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_opt(f, cx)))
            };
            let (cur, total) = self.model.find_status();
            let q_empty = f.query.read(cx).text.is_empty();
            let status = match (&f.error, q_empty) {
                (Some(e), _) => e.clone(),
                (None, true) => String::new(),
                (None, false) if total == 0 => "No results".into(),
                (None, false) if cur == 0 => format!("{total} results"),
                (None, false) => format!("{cur} of {total}"),
            };
            div()
                .flex()
                .flex_col()
                .gap(look.z(4.))
                .px(look.z(8.))
                .py(look.z(6.))
                .bg(look.surface(ui.panel_bg))
                .border_b_1()
                .border_color(look.c(ui.border))
                .text_size(look.text(0.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(look.z(6.))
                        .child(field(&f.query))
                        .child(toggle("Aa", f.opts.case_sensitive, "find-case", |o| o.case_sensitive = !o.case_sensitive))
                        .child(toggle("W", f.opts.whole_word, "find-word", |o| o.whole_word = !o.whole_word))
                        .child(toggle(".*", f.opts.regex, "find-regex", |o| o.regex = !o.regex))
                        .child(
                            div()
                                .w(look.z(90.))
                                .flex_none()
                                .text_size(look.text(-2.))
                                .text_color(look.c(if f.error.is_some() { ui.error } else { ui.text_muted }))
                                .truncate()
                                .child(status),
                        )
                        .child(
                            div()
                                .id("find-close")
                                .flex_none()
                                .cursor_pointer()
                                .child(svg().path(crate::assets::icon::CLOSE).size(look.z(12.)).text_color(look.c(ui.text_muted)))
                                .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
                        ),
                )
                .children(f.replace.as_ref().map(|r| {
                    div()
                        .flex()
                        .items_center()
                        .gap(look.z(6.))
                        .child(field(r))
                        .child(div().flex_none().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child("↩ one · ⌘↩ all"))
                }))
        });
        let conflict_bar = self.merge_conflicts.then(|| {
            let conflicts = self.conflicts();
            let i = self.current_conflict();
            let n = conflicts.len();
            let act = |label: &'static str, id: &'static str, r: Option<qur_git::Resolution>, primary: bool| {
                div()
                    .id(id)
                    .px(look.z(8.))
                    .py(look.z(1.))
                    .rounded(look.z(4.))
                    .cursor_pointer()
                    .bg(look.c(if primary { ui.button_primary_bg } else { ui.button_bg }))
                    .text_color(look.c(if primary { ui.button_primary_text } else { ui.button_text }))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| match r {
                        Some(r) => this.resolve_conflict(i, r, cx),
                        None => this.step_conflict(label == "Next", cx),
                    }))
            };
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(look.z(6.))
                .px(look.z(10.))
                .py(look.z(5.))
                .bg(look.c(ui.permission_bar_bg))
                .border_b_1()
                .border_color(look.c(ui.border))
                .text_size(look.text(-1.))
                .child(div().flex_1().child(if n == 0 {
                    "No conflicts left in this file. Save it (⌘S), then Finish merge.".to_string()
                } else {
                    format!("Conflict {} of {n}: {} ↔ {}", i + 1, conflicts[i].ours_label, conflicts[i].theirs_label)
                }))
                .when(n > 0, |d| {
                    d.child(act("Prev", "c-prev", None, false))
                        .child(act("Next", "c-next", None, false))
                        .child(act("Accept ours", "c-ours", Some(qur_git::Resolution::Ours), false))
                        .child(act("Accept theirs", "c-theirs", Some(qur_git::Resolution::Theirs), false))
                        .child(act("Accept both", "c-both", Some(qur_git::Resolution::Both), true))
                })
        });
        let banner = self.disk_changed.then(|| {
            let button = |label: &'static str, id: &'static str, reload: bool| {
                div()
                    .id(id)
                    .px(look.z(8.))
                    .py(look.z(1.))
                    .rounded(look.z(4.))
                    .cursor_pointer()
                    .bg(look.c(if reload { ui.button_primary_bg } else { ui.button_bg }))
                    .text_color(look.c(if reload { ui.button_primary_text } else { ui.button_text }))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if reload {
                            this.reload_from_disk(cx);
                        } else {
                            this.disk_changed = false;
                            cx.notify();
                        }
                    }))
            };
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(look.z(8.))
                .px(look.z(10.))
                .py(look.z(5.))
                .bg(look.c(ui.permission_bar_bg))
                .border_b_1()
                .border_color(look.c(ui.border))
                .text_size(look.text(-1.))
                .child(div().flex_1().child("This file changed on disk while you had unsaved edits."))
                .child(button("Reload", "disk-reload", true))
                .child(button("Keep mine", "disk-keep", false))
        });
        div()
            .key_context("Editor")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _, cx| {
                // Vim's redo (no other binding uses Ctrl+R in the editor).
                let k = &ev.keystroke;
                if k.modifiers.control && !k.modifiers.platform && k.key == "r" && this.vim.as_ref().is_some_and(|v| v.mode() != VimMode::Insert) {
                    this.vim_key(VimKey::Redo, cx);
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::a_left))
            .on_action(cx.listener(Self::a_right))
            .on_action(cx.listener(Self::a_up))
            .on_action(cx.listener(Self::a_down))
            .on_action(cx.listener(Self::a_sel_left))
            .on_action(cx.listener(Self::a_sel_right))
            .on_action(cx.listener(Self::a_sel_up))
            .on_action(cx.listener(Self::a_sel_down))
            .on_action(cx.listener(Self::a_word_left))
            .on_action(cx.listener(Self::a_word_right))
            .on_action(cx.listener(Self::a_sel_word_left))
            .on_action(cx.listener(Self::a_sel_word_right))
            .on_action(cx.listener(Self::a_line_start))
            .on_action(cx.listener(Self::a_line_end))
            .on_action(cx.listener(Self::a_sel_line_start))
            .on_action(cx.listener(Self::a_sel_line_end))
            .on_action(cx.listener(Self::a_doc_start))
            .on_action(cx.listener(Self::a_doc_end))
            .on_action(cx.listener(Self::a_sel_doc_start))
            .on_action(cx.listener(Self::a_sel_doc_end))
            .on_action(cx.listener(Self::a_page_up))
            .on_action(cx.listener(Self::a_page_down))
            .on_action(cx.listener(Self::a_sel_page_up))
            .on_action(cx.listener(Self::a_sel_page_down))
            .on_action(cx.listener(Self::a_backspace))
            .on_action(cx.listener(Self::a_delete))
            .on_action(cx.listener(Self::a_del_word_left))
            .on_action(cx.listener(Self::a_del_word_right))
            .on_action(cx.listener(Self::a_del_line_start))
            .on_action(cx.listener(Self::a_del_line))
            .on_action(cx.listener(Self::a_move_up))
            .on_action(cx.listener(Self::a_move_down))
            .on_action(cx.listener(Self::a_copy_up))
            .on_action(cx.listener(Self::a_copy_down))
            .on_action(cx.listener(Self::a_line_below))
            .on_action(cx.listener(Self::a_line_above))
            .on_action(cx.listener(Self::a_indent))
            .on_action(cx.listener(Self::a_outdent))
            .on_action(cx.listener(Self::a_tab))
            .on_action(cx.listener(Self::a_backtab))
            .on_action(cx.listener(Self::a_comment))
            .on_action(cx.listener(Self::a_block_comment))
            .on_action(cx.listener(Self::a_trim))
            .on_action(cx.listener(Self::a_select_line))
            .on_action(cx.listener(Self::a_next_match))
            .on_action(cx.listener(Self::a_all_matches))
            .on_action(cx.listener(Self::a_cursor_above))
            .on_action(cx.listener(Self::a_cursor_below))
            .on_action(cx.listener(Self::a_bracket))
            .on_action(cx.listener(Self::a_expand))
            .on_action(cx.listener(Self::a_shrink))
            .on_action(cx.listener(Self::a_newline))
            .on_action(cx.listener(Self::a_undo))
            .on_action(cx.listener(Self::a_redo))
            .on_action(cx.listener(Self::a_cut))
            .on_action(cx.listener(Self::a_copy))
            .on_action(cx.listener(Self::a_paste))
            .on_action(cx.listener(Self::a_select_all))
            .on_action(cx.listener(Self::a_escape))
            .on_action(cx.listener(Self::a_save))
            .on_action(cx.listener(Self::a_case))
            .on_action(cx.listener(Self::a_word))
            .on_action(cx.listener(Self::a_regex))
            .children(conflict_bar)
            .children(banner)
            .children(find_bar)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .cursor_text()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_scroll(cx.listener(Self::on_scroll))
                    .child(EditorElement { view: cx.entity() }),
            )
    }
}

/// Opened files kept in memory (no tabs: the editor area shows one; Quick
/// Open and Ctrl+Tab switch). Most recent first.
pub struct Buffers {
    pub list: Vec<Entity<EditorView>>,
}

impl Buffers {
    pub fn find(&self, path: &Path, cx: &App) -> Option<Entity<EditorView>> {
        self.list.iter().find(|b| b.read(cx).path() == Some(path)).cloned()
    }

    pub fn touch(&mut self, b: &Entity<EditorView>) {
        self.list.retain(|x| x != b);
        self.list.insert(0, b.clone());
    }

    pub fn paths(&self, cx: &App) -> Vec<PathBuf> {
        self.list.iter().filter_map(|b| b.read(cx).path().map(Path::to_path_buf)).collect()
    }

    pub fn dirty(&self, cx: &App) -> Vec<Entity<EditorView>> {
        self.list.iter().filter(|b| b.read(cx).is_dirty()).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::changed_lines;

    #[test]
    fn changed_line_ranges() {
        assert_eq!(changed_lines("a\nb\nc\n", "a\nB\nc\n"), 1..2);
        assert_eq!(changed_lines("a\nc\n", "a\nb1\nb2\nc\n"), 1..3);
        assert_eq!(changed_lines("a\nb\n", "a\n"), 0..1);
        assert_eq!(changed_lines("", "x\n"), 0..1);
    }
}
