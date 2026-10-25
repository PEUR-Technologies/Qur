//! Project search (M6, Cmd+Shift+F): query, options (Aa case, ab whole word,
//! .* regex), replace, and results grouped by file. Searches the selected
//! agent's worktree, or the main checkout. Results open in the code modal.

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{Context, Entity, EventEmitter, HighlightStyle, SharedString, StyledText, Subscription, Task, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list};
use qur_search::{Query, Results};

use crate::input::{InputEvent, TextInput};
use crate::look::Look;

/// Stop after this many matches (the status line says so).
const MAX_MATCHES: usize = 5_000;

pub enum SearchEvent {
    /// Open this file (absolute) at 0-based line and column.
    Open(PathBuf, usize, usize),
    /// Replace All was asked for: the workspace confirms, then calls `replace_all`.
    ConfirmReplace { files: usize, matches: usize },
    Message(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Row {
    File(usize),
    Line(usize, usize),
}

pub struct SearchPanel {
    pub root: PathBuf,
    pub query: Entity<TextInput>,
    replace: Entity<TextInput>,
    regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    results: Results,
    collapsed: HashSet<usize>,
    error: Option<String>,
    took: Option<Duration>,
    searching: bool,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SearchEvent> for SearchPanel {}

impl SearchPanel {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextInput::new("Search (⌥C case · ⌥W word · ⌥R regex)", cx));
        let replace = cx.new(|cx| TextInput::new("Replace", cx));
        let subs = vec![
            cx.subscribe_in(&query, window, |this, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::Changed => this.schedule(cx),
                InputEvent::Confirm { .. } => this.run(cx),
                _ => {}
            }),
            cx.subscribe_in(&replace, window, |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Confirm { all: true, .. } = ev {
                    this.ask_replace(cx);
                }
            }),
        ];
        SearchPanel {
            root,
            query,
            replace,
            regex: false,
            case_sensitive: false,
            whole_word: false,
            results: Results::default(),
            collapsed: HashSet::new(),
            error: None,
            took: None,
            searching: false,
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
            scroll: UniformListScrollHandle::new(),
            _subs: subs,
        }
    }

    /// Search another tree (the selected agent changed); keeps the query.
    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if root != self.root {
            self.root = root;
            self.run(cx);
        }
    }

    pub fn focus_query(&self, window: &mut Window, cx: &mut Context<Self>) {
        let f = self.query.read(cx).focus.clone();
        window.focus(&f, cx);
    }

    pub fn focus_replace(&self, window: &mut Window, cx: &mut Context<Self>) {
        let f = self.replace.read(cx).focus.clone();
        window.focus(&f, cx);
    }

    fn current_query(&self, cx: &Context<Self>) -> Query {
        Query { pattern: self.query.read(cx).text.clone(), regex: self.regex, case_sensitive: self.case_sensitive, whole_word: self.whole_word }
    }

    /// Typing: search 120 ms after the last keystroke.
    fn schedule(&mut self, cx: &mut Context<Self>) {
        let bg = cx.background_executor().clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            bg.timer(Duration::from_millis(120)).await;
            this.update(cx, |this, cx| this.run(cx)).ok();
        }));
    }

    pub fn run(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let query = self.current_query(cx);
        let root = self.root.clone();
        let bg = cx.background_executor().clone();
        self.searching = !query.pattern.is_empty();
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            let t0 = Instant::now();
            let r = {
                let cancel = cancel.clone();
                bg.spawn(async move { qur_search::search(&root, &query, MAX_MATCHES, &cancel) }).await
            };
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            this.update(cx, |this, cx| {
                this.searching = false;
                this.took = Some(t0.elapsed());
                match r {
                    Ok(r) => {
                        this.results = r;
                        this.error = None;
                    }
                    Err(e) => {
                        this.results = Results::default();
                        this.error = Some(e);
                    }
                }
                this.collapsed.clear();
                if std::env::var_os("QUR_DEBUG_ACTIONS").is_some() {
                    let r = &this.results;
                    println!("SEARCH {} matches in {} files ({} searched) in {} ms", r.matches, r.files.len(), r.files_searched, this.took.map(|t| t.as_millis()).unwrap_or(0));
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn toggle(&mut self, which: char, cx: &mut Context<Self>) {
        match which {
            'c' => self.case_sensitive = !self.case_sensitive,
            'w' => self.whole_word = !self.whole_word,
            _ => self.regex = !self.regex,
        }
        self.run(cx);
    }

    fn ask_replace(&mut self, cx: &mut Context<Self>) {
        if self.results.files.is_empty() {
            return cx.emit(SearchEvent::Message("Nothing to replace".into()));
        }
        cx.emit(SearchEvent::ConfirmReplace { files: self.results.files.len(), matches: self.results.matches });
    }

    /// After the workspace confirmed: replace in the files of the last search.
    pub fn replace_all(&mut self, cx: &mut Context<Self>) {
        let query = self.current_query(cx);
        let with = self.replace.read(cx).text.clone();
        let files: Vec<PathBuf> = self.results.files.iter().map(|f| f.path.clone()).collect();
        let root = self.root.clone();
        // Your replace, not an agent's edit: the heatmap leaves these writes out (noted
        // before, as the watcher may report them before the replace returns, and after).
        let abs: Vec<PathBuf> = files.iter().map(|f| root.join(f)).collect();
        let note = move || {
            let mut own = qur_heat::own_writes().lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            for p in &abs {
                own.note(p, now);
            }
        };
        note();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let r = bg.spawn(async move { qur_search::replace_all(&root, &query, &with, &files) }).await;
            note();
            this.update(cx, |this, cx| {
                cx.emit(SearchEvent::Message(match r {
                    Ok((f, n)) => format!("Replaced {n} match{} in {f} file{}", if n == 1 { "" } else { "es" }, if f == 1 { "" } else { "s" }),
                    Err(e) => format!("Replace failed: {e}"),
                }));
                this.run(cx);
            })
            .ok();
        })
        .detach();
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (i, f) in self.results.files.iter().enumerate() {
            rows.push(Row::File(i));
            if !self.collapsed.contains(&i) {
                rows.extend((0..f.lines.len()).map(|j| Row::Line(i, j)));
            }
        }
        rows
    }

    fn row(&self, row: Row, ix: usize, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        match row {
            Row::File(i) => {
                let f = &self.results.files[i];
                let n: usize = f.lines.iter().map(|l| l.ranges.len()).sum();
                let open = !self.collapsed.contains(&i);
                div()
                    .id(("sf", ix))
                    .h(look.z(24.))
                    .px(look.z(10.))
                    .flex()
                    .items_center()
                    .gap(look.z(6.))
                    .cursor_pointer()
                    .hover(|d| d.bg(look.c(ui.hover_bg)))
                    .text_size(look.text(-1.))
                    .child(div().flex_none().text_color(look.c(ui.text_muted)).child(if open { "▾" } else { "▸" }))
                    .child(div().min_w(px(0.)).truncate().text_color(look.c(ui.text)).child(f.path.to_string_lossy().into_owned()))
                    .child(div().flex_none().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(n.to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&i) {
                            this.collapsed.insert(i);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }
            Row::Line(i, j) => {
                let f = &self.results.files[i];
                let l = &f.lines[j];
                // Trim leading indentation, keeping the highlight ranges aligned.
                let indent = l.text.len() - l.text.trim_start().len();
                let text: SharedString = l.text[indent..].to_string().into();
                let hl = HighlightStyle { background_color: Some(look.c(look.theme.editor.search_match_active)), ..Default::default() };
                let ranges: Vec<(Range<usize>, HighlightStyle)> = l
                    .ranges
                    .iter()
                    .filter(|r| r.start >= indent && r.end > r.start)
                    .map(|r| (r.start - indent..r.end - indent, hl))
                    .collect();
                let col = l.col;
                let (path, line) = (self.root.join(&f.path), l.line);
                div()
                    .id(("sl", ix))
                    .h(look.z(22.))
                    .pl(look.z(28.))
                    .pr(look.z(10.))
                    .flex()
                    .items_center()
                    .gap(look.z(10.))
                    .cursor_pointer()
                    .hover(|d| d.bg(look.c(ui.hover_bg)))
                    .child(div().flex_none().w(look.z(36.)).text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child((line + 1).to_string()))
                    .child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .font_family(look.code_font.clone())
                            .text_size(look.code_px())
                            .text_color(look.c(ui.text_secondary))
                            .child(StyledText::new(text).with_highlights(ranges)),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(SearchEvent::Open(path.clone(), line, col))))
                    .into_any_element()
            }
        }
    }
}

impl Render for SearchPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let chip = |id: &'static str, label: &'static str, on: bool, which: char, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex_none()
                .px(look.z(5.))
                .rounded(look.z(3.))
                .cursor_pointer()
                .text_size(look.text(-1.))
                .text_color(look.c(if on { ui.text } else { ui.text_muted }))
                .when(on, |d| d.bg(look.c(ui.active_bg)))
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(which, cx)))
        };
        let field = |input: Entity<TextInput>| div().flex_1().min_w(px(0.)).px(look.z(8.)).py(look.z(3.)).rounded(look.z(4.)).border_1().border_color(look.c(ui.border)).bg(look.c(ui.editor_bg)).child(input);
        let r = &self.results;
        let status = if let Some(e) = &self.error {
            e.clone()
        } else if self.searching {
            "Searching…".into()
        } else if self.query.read(cx).text.is_empty() {
            format!("Search in {}", self.root.file_name().unwrap_or_default().to_string_lossy())
        } else if r.matches == 0 {
            format!("No results · {} files searched", r.files_searched)
        } else {
            format!(
                "{}{} result{} in {} file{}{}",
                if r.truncated { "First " } else { "" },
                r.matches,
                if r.matches == 1 { "" } else { "s" },
                r.files.len(),
                if r.files.len() == 1 { "" } else { "s" },
                self.took.map(|t| format!(" · {} ms", t.as_millis())).unwrap_or_default()
            )
        };
        let rows = self.rows();
        let n = rows.len();
        div()
            .key_context("ProjectSearch")
            .size_full()
            .flex()
            .flex_col()
            .bg(look.region_bg(ui.editor_bg))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _, cx| {
                let k = &ev.keystroke;
                if k.modifiers.alt && !k.modifiers.platform && !k.modifiers.control {
                    // ⌥C / ⌥W / ⌥R as in VS Code (key, not the typed character).
                    if let Some(c @ ('c' | 'w' | 'r')) = k.key.chars().next().filter(|_| k.key.len() == 1) {
                        this.toggle(c, cx);
                        cx.stop_propagation();
                    }
                }
            }))
            .child(
                div()
                    .flex_none()
                    .p(look.z(10.))
                    .flex()
                    .flex_col()
                    .gap(look.z(6.))
                    .border_b_1()
                    .border_color(look.c(ui.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.z(4.))
                            .child(field(self.query.clone()))
                            .child(chip("s-case", "Aa", self.case_sensitive, 'c', cx))
                            .child(chip("s-word", "ab", self.whole_word, 'w', cx))
                            .child(chip("s-regex", ".*", self.regex, 'r', cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(look.z(6.))
                            .child(field(self.replace.clone()))
                            .child(
                                div()
                                    .id("s-replace-all")
                                    .flex_none()
                                    .px(look.z(8.))
                                    .py(look.z(2.))
                                    .rounded(look.z(4.))
                                    .cursor_pointer()
                                    .text_size(look.text(-1.))
                                    .bg(look.c(ui.button_bg))
                                    .text_color(look.c(ui.button_text))
                                    .child("Replace All")
                                    .on_click(cx.listener(|this, _, _, cx| this.ask_replace(cx))),
                            ),
                    )
                    .child(div().text_size(look.text(-2.)).text_color(look.c(if self.error.is_some() { ui.error } else { ui.text_muted })).child(status)),
            )
            .child(
                uniform_list(
                    "search-results",
                    n,
                    cx.processor(move |this, range: Range<usize>, window, cx| {
                        let look = Look::of(window, cx);
                        let rows = this.rows();
                        range.map(|i| this.row(rows[i], i, &look, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1(),
            )
    }
}
