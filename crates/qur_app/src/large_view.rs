//! Large files (over `[editor] large_file_mb`), in the file viewer: read-only,
//! no highlighting, and never loaded whole. A background scan builds a
//! sparse line index (`qur_editor::large`) while the first lines already
//! show; each frame reads only the lines on screen. Find streams through
//! the file off the main thread (and counts every match); Go to Line, line
//! selection and Copy work as in the code modal.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, ScrollEvent, Subscription, Task, Window, div, prelude::*, px, svg,
};
use qur_editor::FindOptions;
use qur_editor::encoding::{self, Encoding};
use qur_editor::large::{self, Hit, Line, LineIndex};

use crate::commands::{edit, editor as ea, qur};
use crate::grid::{Grid, GridRow, GridSource, Metrics};
use crate::input::{InputEvent, TextInput};
use crate::look::Look;

/// Most lines (and bytes) Copy takes from a selection.
const MAX_COPY_LINES: u64 = 100_000;
const MAX_COPY_BYTES: usize = 16 << 20;
const TAB: usize = 4;

pub enum LargeEvent {
    Message(String),
}

/// A find's match count, filled in by a background scan.
#[derive(Default)]
struct Count {
    scanned: AtomicU64,
    found: AtomicU64,
    done: AtomicBool,
}

struct FindBar {
    query: Entity<TextInput>,
    opts: FindOptions,
    error: Option<String>,
    /// The current match.
    hit: Option<Hit>,
    searching: bool,
    not_found: bool,
    /// Counting matches for this query (its text and options).
    count: Option<(String, FindOptions, Arc<Count>)>,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    _sub: Subscription,
}

pub struct LargeView {
    pub focus: FocusHandle,
    path: PathBuf,
    len: u64,
    encoding: Encoding,
    index: Arc<Mutex<LineIndex>>,
    /// Stops the background scans when the view goes away.
    stop: Arc<AtomicBool>,
    _index: Task<()>,
    ticker: Option<Task<()>>,
    file: Option<File>,
    /// First line on screen, and the first column (horizontal scroll).
    top: u64,
    left: usize,
    scroll_acc: (Pixels, Pixels),
    metrics: Metrics,
    /// Lines read around the screen: (first line, lines, reached the end).
    cache: Option<(u64, Vec<Line>, bool)>,
    /// Selected lines (anchor, head).
    sel: Option<(u64, u64)>,
    dragging: bool,
    find: Option<FindBar>,
    goto: Option<(Entity<TextInput>, Subscription)>,
    error: Option<String>,
}

impl EventEmitter<LargeEvent> for LargeView {}

impl Focusable for LargeView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Drop for LargeView {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(f) = &self.find {
            f.cancel.store(true, Ordering::Relaxed);
            if let Some((_, _, c)) = &f.count {
                c.done.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// A line's chars with the byte each starts at, decoded as `enc` (UTF-16
/// lines split at the 0x0A byte lose their newline's other half here).
fn chars_of(bytes: &[u8], enc: Encoding, first_line: bool) -> Vec<(char, usize)> {
    let mut out = Vec::with_capacity(bytes.len());
    match enc {
        Encoding::Windows1252 => out.extend(bytes.iter().enumerate().map(|(i, &b)| (encoding::cp1252_char(b), i))),
        Encoding::Utf8 => {
            let mut at = 0;
            for chunk in bytes.utf8_chunks() {
                out.extend(chunk.valid().char_indices().map(|(i, c)| (c, at + i)));
                at += chunk.valid().len();
                if !chunk.invalid().is_empty() {
                    out.push((char::REPLACEMENT_CHARACTER, at));
                    at += chunk.invalid().len();
                }
            }
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let le = enc == Encoding::Utf16Le;
            // LE: "\n\0" leaves the 0 at the start of the next line; BE: "\0\n" at the end of this one.
            let skip = usize::from(le && !first_line && bytes.first() == Some(&0));
            let bom = if first_line && (bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF])) { 2 } else { 0 };
            let start = skip.max(bom);
            let body = &bytes[start.min(bytes.len())..];
            let units = body.as_chunks::<2>().0.iter().map(|p| if le { u16::from_le_bytes([p[0], p[1]]) } else { u16::from_be_bytes([p[0], p[1]]) });
            let mut at = start;
            for c in char::decode_utf16(units) {
                let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
                out.push((c, at));
                at += c.len_utf16() * 2;
            }
            while matches!(out.last(), Some(('\r' | '\0', _))) {
                out.pop();
            }
        }
    }
    out
}

/// What a char shows as: tabs as spaces to the next stop, other control
/// characters as their Control Pictures symbol.
fn shown(c: char, col: usize, out: &mut String) -> usize {
    match c {
        '\t' => {
            let n = TAB - col % TAB;
            out.extend(std::iter::repeat_n(' ', n));
            n
        }
        '\0'..='\x1F' => {
            out.push(char::from_u32(0x2400 + c as u32).unwrap_or('·'));
            1
        }
        '\x7F' => {
            out.push('␡');
            1
        }
        c => {
            out.push(c);
            1
        }
    }
}

impl LargeView {
    pub fn new(path: &Path, cx: &mut Context<Self>) -> Self {
        let (file, error) = match File::open(path) {
            Ok(f) => (Some(f), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let len = file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
        let mut head = vec![0u8; encoding::SNIFF_LEN];
        let n = file.as_ref().and_then(|f| {
            use std::os::unix::fs::FileExt;
            f.read_at(&mut head, 0).ok()
        });
        let encoding = encoding::guess(&head[..n.unwrap_or(0)]).0;
        let index = Arc::new(Mutex::new(LineIndex::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let task = {
            let (index, stop, path) = (index.clone(), stop.clone(), path.to_path_buf());
            cx.background_executor().spawn(async move {
                let t0 = std::time::Instant::now();
                let result = File::open(&path).and_then(|f| {
                    large::index_file(f, |chunk| {
                        index.lock().unwrap_or_else(|e| e.into_inner()).feed(chunk);
                        !stop.load(Ordering::Relaxed)
                    })
                });
                let mut ix = index.lock().unwrap_or_else(|e| e.into_inner());
                ix.done = true;
                if let Err(e) = result {
                    crate::log(&format!("indexing {}: {e}", path.display()));
                }
                crate::log(&format!("LARGE indexed {} lines ({} entries) in {} ms", ix.lines, ix.entries.len(), t0.elapsed().as_millis()));
            })
        };
        let mut v = LargeView {
            focus: cx.focus_handle(),
            path: path.to_path_buf(),
            len,
            encoding,
            index,
            stop,
            _index: task,
            ticker: None,
            file,
            top: 0,
            left: 0,
            scroll_acc: (px(0.), px(0.)),
            metrics: Metrics::default(),
            cache: None,
            sel: None,
            dragging: false,
            find: None,
            goto: None,
            error,
        };
        v.ensure_ticker(cx);
        v
    }

    fn index(&self) -> std::sync::MutexGuard<'_, LineIndex> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lines(&self) -> u64 {
        self.index().lines
    }

    fn busy(&self) -> bool {
        !self.index().done || self.find.as_ref().is_some_and(|f| f.searching || f.count.as_ref().is_some_and(|c| !c.2.done.load(Ordering::Relaxed)))
    }

    /// Repaint now and then while the index or a search is running.
    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(120)).await;
                let more = this
                    .update(cx, |v, cx| {
                        cx.notify();
                        let busy = v.busy();
                        if !busy {
                            v.ticker = None;
                        }
                        busy
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        }));
    }

    fn max_top(&self) -> u64 {
        let lines = self.lines();
        lines.saturating_sub(self.metrics.page() as u64 / 2).min(lines.saturating_sub(1))
    }

    fn scroll_to(&mut self, top: u64, cx: &mut Context<Self>) {
        self.top = top.min(self.max_top());
        cx.notify();
    }

    /// Lines `first..first + n` (fewer at the end), from the cache or disk.
    fn visible(&mut self, first: u64, n: usize) -> Vec<Line> {
        if let Some((c0, lines, eof)) = &self.cache {
            let covers = first >= *c0 && (first + n as u64 <= c0 + lines.len() as u64 || *eof);
            if covers {
                let from = ((first - c0) as usize).min(lines.len());
                return lines[from..(from + n).min(lines.len())].to_vec();
            }
        }
        let start = first.saturating_sub(n as u64);
        let want = n * 3 + 1;
        let read = match self.file.as_mut() {
            Some(f) => {
                let ix = self.index.lock().unwrap_or_else(|e| e.into_inner());
                large::read_lines(f, &ix, start, want).map(|l| (l, ix.done))
            }
            None => Ok((Vec::new(), true)),
        };
        match read {
            Ok((lines, done)) => {
                let eof = lines.len() < want && done;
                self.cache = Some((start, lines, eof));
            }
            Err(e) => {
                self.error = Some(e.to_string());
                self.cache = Some((first, Vec::new(), true));
            }
        }
        let Some((c0, lines, _)) = &self.cache else { return Vec::new() };
        let from = (first.saturating_sub(*c0) as usize).min(lines.len());
        lines[from..(from + n).min(lines.len())].to_vec()
    }

    fn sel_range(&self) -> Option<(u64, u64)> {
        self.sel.map(|(a, h)| (a.min(h), a.max(h)))
    }

    fn hit_line(&self, p: gpui::Point<Pixels>) -> u64 {
        let (row, _) = self.metrics.cell_at(p);
        (self.top + row as u64).min(self.lines().saturating_sub(1))
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let l = self.hit_line(ev.position);
        self.sel = match (ev.modifiers.shift, self.sel) {
            (true, Some((a, _))) => Some((a, l)),
            _ => Some((l, l)),
        };
        self.dragging = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.dragging || ev.pressed_button != Some(MouseButton::Left) {
            self.dragging = false;
            return;
        }
        let b = self.metrics.bounds;
        if ev.position.y < b.top() {
            self.top = self.top.saturating_sub(1);
        } else if ev.position.y > b.bottom() {
            self.top = (self.top + 1).min(self.max_top());
        }
        if let Some((a, _)) = self.sel {
            self.sel = Some((a, self.hit_line(ev.position)));
        }
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (lh, cw) = (self.metrics.line_h, self.metrics.cell_w);
        let d = ev.delta.pixel_delta(lh);
        self.scroll_acc.0 -= d.y;
        self.scroll_acc.1 -= d.x;
        let rows = (self.scroll_acc.0 / lh).trunc();
        if rows != 0. {
            self.scroll_acc.0 -= lh * rows;
            let top = if rows < 0. { self.top.saturating_sub((-rows) as u64) } else { self.top + rows as u64 };
            self.scroll_to(top, cx);
        }
        let cols = (self.scroll_acc.1 / cw).trunc();
        if cols != 0. {
            self.scroll_acc.1 -= cw * cols;
            self.left = (self.left as f32 + cols).clamp(0., large::MAX_LINE as f32) as usize;
            cx.notify();
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let page = self.metrics.page() as u64;
        let top = match (k.key.as_str(), k.modifiers.platform) {
            ("up", false) => self.top.saturating_sub(1),
            ("down", false) => self.top + 1,
            ("pageup", _) => self.top.saturating_sub(page),
            ("pagedown", _) | ("space", false) => self.top + page,
            ("home", _) | ("up", true) => 0,
            ("end", _) | ("down", true) => u64::MAX,
            ("left", _) => {
                self.left = if k.modifiers.platform { 0 } else { self.left.saturating_sub(8) };
                cx.notify();
                return cx.stop_propagation();
            }
            ("right", _) => {
                self.left += 8;
                cx.notify();
                return cx.stop_propagation();
            }
            _ => return,
        };
        self.scroll_to(top, cx);
        cx.stop_propagation();
    }

    // -- copy, go to line ---------------------------------------------------------

    fn copy(&mut self, _: &edit::Copy, _: &mut Window, cx: &mut Context<Self>) {
        let Some((a, b)) = self.sel_range() else { return };
        let n = (b - a + 1).min(MAX_COPY_LINES) as usize;
        let read = match self.file.as_mut() {
            Some(f) => {
                let ix = self.index.lock().unwrap_or_else(|e| e.into_inner());
                large::read_lines(f, &ix, a, n)
            }
            None => Ok(Vec::new()),
        };
        match read {
            Ok(lines) => {
                let mut text = String::new();
                for (i, l) in lines.iter().enumerate() {
                    if i > 0 {
                        text.push('\n');
                    }
                    text.extend(chars_of(&l.bytes, self.encoding, a + i as u64 == 0).into_iter().map(|(c, _)| c));
                    if text.len() > MAX_COPY_BYTES {
                        break;
                    }
                }
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                let cut = (b - a + 1) as usize > lines.len() || lines.iter().any(|l| l.cut);
                cx.emit(LargeEvent::Message(format!("Copied {} line{}{}", lines.len(), if lines.len() == 1 { "" } else { "s" }, if cut { " (shortened)" } else { "" })));
            }
            Err(e) => cx.emit(LargeEvent::Message(format!("Couldn't read {}: {e}", self.path.display()))),
        }
    }

    fn goto_action(&mut self, _: &qur::GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new("Go to line…", cx));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Confirm { .. } => {
                let text = input.read(cx).text.trim().replace([',', '_'], "");
                match text.parse::<u64>() {
                    Ok(n) if n > 0 => {
                        this.go_to_line(n - 1, cx);
                        this.close_bars(window, cx);
                    }
                    _ => cx.emit(LargeEvent::Message(format!("Not a line number: {text}"))),
                }
            }
            InputEvent::Cancel => this.close_bars(window, cx),
            _ => {}
        });
        let f = input.read(cx).focus.clone();
        self.goto = Some((input, sub));
        window.focus(&f, cx);
        cx.notify();
    }

    /// Select line `line` (0-based) and show it a third of the way down.
    pub fn go_to_line(&mut self, line: u64, cx: &mut Context<Self>) {
        let line = line.min(self.lines().saturating_sub(1));
        self.sel = Some((line, line));
        self.scroll_to(line.saturating_sub(self.metrics.page() as u64 / 3), cx);
    }

    fn close_bars(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.goto = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Esc: closes Go to Line, then Find; false when neither is open.
    pub fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.goto.is_some() {
            self.close_bars(window, cx);
        } else if self.find.is_some() {
            self.close_find(window, cx);
        } else {
            return false;
        }
        true
    }

    // -- find ---------------------------------------------------------------------

    fn find_action(&mut self, _: &ea::Find, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(window, cx);
    }

    fn find_next_action(&mut self, _: &ea::FindNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            return self.open_find(window, cx);
        }
        self.search(true, cx);
    }

    fn find_prev_action(&mut self, _: &ea::FindPrev, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            return self.open_find(window, cx);
        }
        self.search(false, cx);
    }

    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            let query = cx.new(|cx| TextInput::new("Find (streams through the file)", cx));
            let sub = cx.subscribe_in(&query, window, |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Changed => this.query_changed(cx),
                InputEvent::Confirm { alt, .. } => this.search(!*alt, cx),
                InputEvent::Cancel => this.close_find(window, cx),
                _ => {}
            });
            self.find = Some(FindBar {
                query,
                opts: FindOptions::default(),
                error: None,
                hit: None,
                searching: false,
                not_found: false,
                count: None,
                cancel: Arc::new(AtomicBool::new(false)),
                task: None,
                _sub: sub,
            });
        }
        if let Some(f) = &self.find {
            let focus = f.query.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.find.take() {
            f.cancel.store(true, Ordering::Relaxed);
            if let Some((_, _, c)) = &f.count {
                c.done.store(true, Ordering::Relaxed);
            }
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Typing in the find field: stop what the old query started.
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(f) = self.find.as_mut() {
            f.cancel.store(true, Ordering::Relaxed);
            f.cancel = Arc::new(AtomicBool::new(false));
            f.task = None;
            f.searching = false;
            f.not_found = false;
            f.error = None;
            if let Some((_, _, c)) = f.count.take() {
                c.done.store(true, Ordering::Relaxed);
            }
        }
        cx.notify();
    }

    fn toggle_opt(&mut self, f: fn(&mut FindOptions), cx: &mut Context<Self>) {
        if let Some(bar) = self.find.as_mut() {
            f(&mut bar.opts);
        }
        self.query_changed(cx);
    }

    fn find_regex(&self, cx: &gpui::App) -> Option<Result<large::Regex, String>> {
        let f = self.find.as_ref()?;
        let q = f.query.read(cx).text.clone();
        (!q.is_empty()).then(|| large::query_regex(&q, f.opts))
    }

    /// The next (or previous) match from the current one, or from the top
    /// line; searched in the background, the count alongside.
    fn search(&mut self, forward: bool, cx: &mut Context<Self>) {
        let re = match self.find_regex(cx) {
            None => return,
            Some(Err(e)) => {
                if let Some(f) = self.find.as_mut() {
                    f.error = Some(e);
                }
                return cx.notify();
            }
            Some(Ok(re)) => re,
        };
        let (from_line, col) = match self.find.as_ref().and_then(|f| f.hit.clone()) {
            Some((l, r)) => (l, if forward { r.end.max(r.start + 1) } else { r.start }),
            None => (self.sel_range().map_or(self.top, |s| s.0), 0),
        };
        let query = self.find.as_ref().map(|f| f.query.read(cx).text.clone()).unwrap_or_default();
        self.start_count(&query, re.clone(), cx);
        let Some(f) = self.find.as_mut() else { return };
        f.cancel.store(true, Ordering::Relaxed);
        f.cancel = Arc::new(AtomicBool::new(false));
        f.searching = true;
        f.not_found = false;
        f.error = None;
        let cancel = f.cancel.clone();
        let (path, len, index) = (self.path.clone(), self.len, self.index().clone());
        let bg = cx.background_executor().clone();
        let task = cx.spawn(async move |this, cx| {
            let found = bg
                .spawn(async move {
                    let mut file = File::open(&path)?;
                    if forward {
                        let off = large::line_offset(&mut file, &index, from_line)?;
                        large::find_next(&mut file, &re, (from_line, off), col, len, &cancel)
                    } else {
                        large::find_prev(&mut file, &re, &index, from_line, col, len, &cancel)
                    }
                })
                .await;
            this.update(cx, |v, cx| v.found(found, cx)).ok();
        });
        if let Some(f) = self.find.as_mut() {
            f.task = Some(task);
        }
        self.ensure_ticker(cx);
        cx.notify();
    }

    fn found(&mut self, found: std::io::Result<Option<Hit>>, cx: &mut Context<Self>) {
        let Some(f) = self.find.as_mut() else { return };
        f.searching = false;
        match found {
            Ok(Some(hit)) => {
                let line = hit.0;
                f.hit = Some(hit);
                self.sel = None;
                let page = self.metrics.page() as u64;
                if line < self.top || line >= self.top + page {
                    self.scroll_to(line.saturating_sub(page / 3), cx);
                }
                if std::env::var_os("QUR_DEBUG_ACTIONS").is_some() {
                    println!("LARGE_FIND line {}", line + 1);
                }
            }
            Ok(None) => f.not_found = true,
            Err(e) => f.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// Count every match of `query` in the background (once per query).
    fn start_count(&mut self, query: &str, re: large::Regex, cx: &mut Context<Self>) {
        let Some(f) = self.find.as_mut() else { return };
        if f.count.as_ref().is_some_and(|(q, o, _)| q == query && *o == f.opts) {
            return;
        }
        if let Some((_, _, old)) = f.count.take() {
            old.done.store(true, Ordering::Relaxed);
        }
        let count = Arc::new(Count::default());
        f.count = Some((query.to_string(), f.opts, count.clone()));
        let (path, len) = (self.path.clone(), self.len);
        cx.background_executor()
            .spawn(async move {
                let stop = AtomicBool::new(false);
                let result = File::open(&path).and_then(|mut file| {
                    large::count(&mut file, &re, len, &stop, |scanned, found| {
                        count.scanned.store(scanned, Ordering::Relaxed);
                        count.found.store(found, Ordering::Relaxed);
                        if count.done.load(Ordering::Relaxed) {
                            stop.store(true, Ordering::Relaxed);
                        }
                    })
                });
                if let Ok(Some(n)) = result {
                    count.found.store(n, Ordering::Relaxed);
                    count.scanned.store(len, Ordering::Relaxed);
                }
                count.done.store(true, Ordering::Relaxed);
            })
            .detach();
    }

    fn find_status(&self) -> String {
        let Some(f) = &self.find else { return String::new() };
        if let Some(e) = &f.error {
            return e.clone();
        }
        let count = f.count.as_ref().map(|(_, _, c)| {
            let n = c.found.load(Ordering::Relaxed);
            if c.done.load(Ordering::Relaxed) {
                format!("{n} match{}", if n == 1 { "" } else { "es" })
            } else {
                let pct = c.scanned.load(Ordering::Relaxed) * 100 / self.len.max(1);
                format!("{n}+ (counting {pct}%)")
            }
        });
        let here = if f.searching {
            Some("searching…".to_string())
        } else if f.not_found {
            Some("no results".to_string())
        } else {
            f.hit.as_ref().map(|(l, _)| format!("line {}", l + 1))
        };
        [here, count].into_iter().flatten().collect::<Vec<_>>().join(" · ")
    }

    /// Scripted runs' `state`: "large lines=… indexed=… top=… sel=… find=…".
    pub fn debug_state(&self, cx: &gpui::App) -> String {
        let ix = self.index();
        let sel = self.sel_range().map_or_else(|| "-".into(), |(a, b)| format!("{}-{}", a + 1, b + 1));
        let find = self.find.as_ref().map_or_else(|| "none".into(), |f| format!("{:?}:{}", f.query.read(cx).text, self.find_status()));
        format!("large len={} lines={} indexed={} encoding={} top={} sel={sel} find={find}", self.len, ix.lines, ix.done, encoding::label(self.encoding, false), self.top + 1)
    }
}

impl GridSource for LargeView {
    fn gutter_cols(&self) -> usize {
        self.lines().to_string().len().max(4)
    }

    fn grid_rows(&mut self, m: Metrics, look: &Look, cx: &mut Context<Self>) -> Vec<GridRow> {
        self.metrics = m;
        self.top = self.top.min(self.max_top());
        let ed = &look.theme.editor;
        let ui = &look.theme.ui;
        let (text_c, muted) = (look.c(ed.text), look.c(ui.text_muted));
        let (match_bg, active_bg, sel_bg) = (look.c(ed.search_match), look.c(ed.search_match_active), look.c(ui.selection));
        let digits = self.gutter_cols();
        let cols = ((m.bounds.size.width - m.text_x) / m.cell_w).ceil().max(1.) as usize + 1;
        let lines = self.visible(self.top, m.rows);
        let re = self.find_regex(cx).and_then(Result::ok);
        let hit = self.find.as_ref().and_then(|f| f.hit.clone());
        let sel = self.sel_range();
        let mut out = Vec::with_capacity(lines.len());
        for (i, line) in lines.iter().enumerate() {
            let n = self.top + i as u64;
            let mut row = GridRow::new(Some(format!("{:>digits$}", n + 1).into()));
            if sel.is_some_and(|(a, b)| (a..=b).contains(&n)) {
                row.bg = Some(sel_bg);
            }
            let matches: Vec<std::ops::Range<usize>> = re.as_ref().map(|re| re.find_iter(&line.bytes).map(|m| m.range()).collect()).unwrap_or_default();
            let active = hit.as_ref().filter(|h| h.0 == n).map(|h| h.1.clone());
            let mut col = 0;
            let mut piece = String::new();
            for (c, b) in chars_of(&line.bytes, self.encoding, n == 0) {
                if col >= self.left + cols {
                    break;
                }
                piece.clear();
                let w = shown(c, col, &mut piece);
                let start = col;
                col += w;
                if col <= self.left {
                    continue;
                }
                if start < self.left {
                    // A tab straddling the left edge.
                    piece = " ".repeat(col - self.left);
                }
                let bg = if active.as_ref().is_some_and(|r| r.contains(&b)) {
                    Some(active_bg)
                } else if matches.iter().any(|r| r.contains(&b)) {
                    Some(match_bg)
                } else {
                    None
                };
                let color = if c.is_control() { muted } else { text_c };
                row.push(&piece, color, bg);
            }
            if line.cut && col < self.left + cols {
                row.push(" …", muted, None);
            }
            out.push(row);
        }
        out
    }

    fn scroll_thumb(&self) -> Option<(f32, f32)> {
        let total = self.lines();
        let page = self.metrics.page() as u64;
        (total > page).then(|| (self.top as f32 / self.max_top().max(1) as f32, page as f32 / total as f32))
    }
}

impl Render for LargeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let (lines, done, scanned) = {
            let ix = self.index();
            (ix.lines, ix.done, ix.scanned)
        };
        let indexing = if done { String::new() } else { format!(" (indexing {}%)", scanned * 100 / self.len.max(1)) };
        let sel = match self.sel_range() {
            Some((a, b)) if a == b => format!(" · line {}", a + 1),
            Some((a, b)) => format!(" · lines {}–{} (⌘C copies)", a + 1, b + 1),
            None => String::new(),
        };
        let status = match &self.error {
            Some(e) => e.clone(),
            None => format!(
                "{lines} lines{indexing} · {} · read-only, no highlighting (large file){sel}",
                encoding::label(self.encoding, false)
            ),
        };
        let field = |input: &Entity<TextInput>| div().flex_1().px(look.z(6.)).border_1().border_color(look.c(ui.border)).rounded(look.z(4.)).child(input.clone());
        let find_bar = self.find.as_ref().map(|f| {
            let toggle = |label: &'static str, on: bool, id: &'static str, t: fn(&mut FindOptions)| {
                div()
                    .id(id)
                    .px(look.z(5.))
                    .rounded(look.z(3.))
                    .text_size(look.text(-2.))
                    .cursor_pointer()
                    .text_color(look.c(if on { ui.text } else { ui.text_muted }))
                    .when(on, |d| d.bg(look.c(ui.active_bg)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_opt(t, cx)))
            };
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(look.z(6.))
                .px(look.z(8.))
                .py(look.z(6.))
                .border_b_1()
                .border_color(look.c(ui.border))
                .child(field(&f.query))
                .child(toggle("Aa", f.opts.case_sensitive, "lfind-case", |o| o.case_sensitive = !o.case_sensitive))
                .child(toggle("W", f.opts.whole_word, "lfind-word", |o| o.whole_word = !o.whole_word))
                .child(toggle(".*", f.opts.regex, "lfind-regex", |o| o.regex = !o.regex))
                .child(
                    div()
                        .w(look.z(150.))
                        .flex_none()
                        .text_size(look.text(-2.))
                        .text_color(look.c(if f.error.is_some() { ui.error } else { ui.text_muted }))
                        .truncate()
                        .child(self.find_status()),
                )
                .child(
                    div()
                        .id("lfind-close")
                        .flex_none()
                        .cursor_pointer()
                        .child(svg().path(crate::assets::icon::CLOSE).size(look.z(12.)).text_color(look.c(ui.text_muted)))
                        .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
                )
        });
        div()
            .key_context("LargeView")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::goto_action))
            .on_action(cx.listener(Self::find_action))
            .on_action(cx.listener(Self::find_next_action))
            .on_action(cx.listener(Self::find_prev_action))
            .children(self.goto.as_ref().map(|(input, _)| div().flex_none().flex().px(look.z(8.)).py(look.z(6.)).border_b_1().border_color(look.c(ui.border)).child(field(input))))
            .children(find_bar)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .cursor_text()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
                    .on_scroll(cx.listener(Self::on_scroll))
                    .child(Grid { view: cx.entity() }),
            )
            .child(
                div()
                    .flex_none()
                    .px(look.z(10.))
                    .py(look.z(3.))
                    .border_t_1()
                    .border_color(look.c(ui.border))
                    .text_size(look.text(-2.))
                    .text_color(look.c(ui.text_muted))
                    .truncate()
                    .child(status),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_lines_with_byte_offsets() {
        assert_eq!(chars_of("aé".as_bytes(), Encoding::Utf8, true), vec![('a', 0), ('é', 1)]);
        assert_eq!(chars_of(b"a\xFFb", Encoding::Utf8, true), vec![('a', 0), ('\u{FFFD}', 1), ('b', 2)]);
        assert_eq!(chars_of(b"\x93x", Encoding::Windows1252, true), vec![('“', 0), ('x', 1)]);
        // UTF-16 LE split at 0x0A: the next line starts with the newline's 0 byte.
        assert_eq!(chars_of(b"\0h\0i\0\r\0", Encoding::Utf16Le, false), vec![('h', 1), ('i', 3)]);
        assert_eq!(chars_of(b"\xFF\xFEh\0", Encoding::Utf16Le, true), vec![('h', 2)]);
    }

    #[test]
    fn tabs_and_controls_display() {
        let mut s = String::new();
        assert_eq!(shown('\t', 1, &mut s), 3);
        assert_eq!(s, "   ");
        s.clear();
        shown('\0', 0, &mut s);
        shown('\x1B', 0, &mut s);
        assert_eq!(s, "␀␛");
    }
}
