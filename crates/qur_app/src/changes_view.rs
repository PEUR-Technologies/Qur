//! Changes (handoff section 10, M4): the diff of everything a task changed
//! vs its base, with Merge / Keep working / Discard, a conflict pre-check,
//! and line comments that go back to the agent as one prompt (from Orca).
//! The diff renderer is shared with the Git panel.

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{
    Context, Entity, EventEmitter, Hsla, SharedString, Subscription, Task, UniformListScrollHandle, Window, div,
    prelude::*, px, uniform_list,
};
use qur_git::{FileDiff, FileStatus, LineKind};

use crate::input::{InputEvent, TextInput};
use crate::look::Look;

pub enum ChangesEvent {
    Merge,
    KeepWorking,
    Discard,
    /// Send these review comments to the agent as one prompt.
    SendComments(String),
    /// Open a file (absolute path) in the code modal at a line.
    Open(PathBuf, Option<u32>),
}

#[derive(Clone)]
pub struct Comment {
    pub path: PathBuf,
    pub line: u32,
    pub quote: String,
    pub text: String,
}

/// The line-comment field's hint (the Changes view and plan comments).
pub const COMMENT_PLACEHOLDER: &str = "Comment for the agent… (↩ add, esc cancel)";

/// Line comments as one message for the agent: `intro`, then each comment
/// numbered with where it is, the line's text and what you wrote.
pub fn comments_prompt<'a>(intro: &str, comments: impl IntoIterator<Item = (String, &'a str, &'a str)>) -> String {
    let mut s = format!("{intro}\n\n");
    for (i, (at, quote, text)) in comments.into_iter().enumerate() {
        s.push_str(&format!("{}. `{at}` — `{}`\n   {text}\n\n", i + 1, quote.trim()));
    }
    s
}

/// One visual row of a diff list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    File(usize),
    Hunk(usize, usize),
    Line(usize, usize, usize),
    /// A saved comment (index into comments) under its line.
    Comment(usize),
    /// The comment being written, under (file, hunk, line).
    Draft,
}

/// Flatten files into rows (collapsed files show only their header).
pub fn rows_for(files: &[FileDiff], collapsed: &HashSet<usize>, comments: &[Comment], draft: Option<(usize, usize, usize)>) -> Vec<Row> {
    let mut rows = Vec::new();
    for (f, file) in files.iter().enumerate() {
        rows.push(Row::File(f));
        if collapsed.contains(&f) {
            continue;
        }
        for (h, hunk) in file.hunks.iter().enumerate() {
            rows.push(Row::Hunk(f, h));
            for (l, line) in hunk.lines.iter().enumerate() {
                rows.push(Row::Line(f, h, l));
                let n = line.new_no.or(line.old_no).unwrap_or(0);
                for (ci, c) in comments.iter().enumerate() {
                    if c.path == file.path && c.line == n && line.kind != LineKind::Removed {
                        rows.push(Row::Comment(ci));
                    }
                }
                if draft == Some((f, h, l)) {
                    rows.push(Row::Draft);
                }
            }
        }
    }
    rows
}

/// Paint one diff row (shared with the Git panel). `on_line` is the click
/// handler for line rows.
pub fn diff_row(files: &[FileDiff], row: Row, look: &Look, collapsed: bool) -> gpui::Div {
    let ui = &look.theme.ui;
    let diff = &look.theme.diff;
    let mono = look.code_font.clone();
    let h = look.z(look.code_size * 1.55);
    match row {
        Row::File(f) => {
            let file = &files[f];
            let status = match file.status {
                FileStatus::Added => "added",
                FileStatus::Modified => "",
                FileStatus::Deleted => "deleted",
                FileStatus::Renamed => "renamed",
                FileStatus::Binary => "binary",
            };
            let name = match &file.old_path {
                Some(o) if file.status == FileStatus::Renamed => format!("{} → {}", o.display(), file.path.display()),
                _ => file.path.display().to_string(),
            };
            div()
                .h(look.z(28.))
                .px(look.z(12.))
                .flex()
                .items_center()
                .gap(look.z(8.))
                .bg(look.c(ui.panel_bg))
                .border_b_1()
                .border_t_1()
                .border_color(look.c(ui.border))
                .text_size(look.text(-1.))
                .child(div().flex_none().text_color(look.c(ui.text_muted)).child(if collapsed { "▸" } else { "▾" }))
                .child(div().min_w(px(0.)).truncate().text_color(look.c(ui.text)).font_weight(gpui::FontWeight::MEDIUM).child(name))
                .when(!status.is_empty(), |d| d.child(div().flex_none().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(status)))
                .child(div().flex_1())
                .child(div().flex_none().text_color(look.c(diff.added_text)).child(format!("+{}", file.added)))
                .child(div().flex_none().text_color(look.c(diff.removed_text)).child(format!("−{}", file.removed)))
        }
        Row::Hunk(f, hk) => {
            let hunk = &files[f].hunks[hk];
            div()
                .h(h)
                .px(look.z(12.))
                .flex()
                .items_center()
                .font_family(mono)
                .text_size(look.text(-2.))
                .text_color(look.c(ui.text_muted))
                .child(format!("@@ −{},{} +{},{} @@ {}", hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines, hunk.header))
        }
        Row::Line(f, hk, l) => {
            let line = &files[f].hunks[hk].lines[l];
            let (bg, word, sign): (Option<Hsla>, Hsla, &str) = match line.kind {
                LineKind::Added => (Some(look.c(diff.added_line)), look.c(diff.added_word), "+"),
                LineKind::Removed => (Some(look.c(diff.removed_line)), look.c(diff.removed_word), "−"),
                LineKind::Context => (None, look.c(ui.hover_bg), " "),
            };
            let num = |n: Option<u32>| div().w(look.z(36.)).flex_none().text_align(gpui::TextAlign::Right).pr(look.z(6.)).child(n.map(|n| n.to_string()).unwrap_or_default());
            // Word emphasis: split the text at the emphasized byte ranges.
            let text = &line.text;
            let mut pieces: Vec<gpui::AnyElement> = Vec::new();
            let mut at = 0;
            for r in &line.emphasis {
                if r.end > text.len() || r.start < at || !text.is_char_boundary(r.start) || !text.is_char_boundary(r.end) {
                    continue;
                }
                if r.start > at {
                    pieces.push(div().child(text[at..r.start].to_string()).into_any_element());
                }
                pieces.push(div().bg(word).rounded(px(2.)).child(text[r.clone()].to_string()).into_any_element());
                at = r.end;
            }
            if at < text.len() {
                pieces.push(div().child(text[at..].to_string()).into_any_element());
            }
            div()
                .h(h)
                .flex()
                .items_center()
                .font_family(mono)
                .text_size(look.code_px())
                .text_color(look.c(look.theme.editor.text))
                .when_some(bg, |d, bg| d.bg(bg))
                .child(div().flex_none().flex().text_size(look.text(-2.)).text_color(look.c(look.theme.editor.gutter_text)).child(num(line.old_no)).child(num(line.new_no)))
                .child(div().w(look.z(14.)).flex_none().text_color(look.c(ui.text_muted)).child(sign))
                .child(div().flex().whitespace_nowrap().overflow_hidden().children(pieces))
        }
        Row::Comment(_) | Row::Draft => div().h(h),
    }
}

/// Changes filtered to one agent turn (its checkpoints).
#[derive(Clone)]
pub struct TurnFilter {
    pub n: u32,
    pub start: String,
    /// None while the turn runs: up to the worktree as it is now.
    pub end: Option<String>,
}

pub struct ChangesView {
    pub worktree: PathBuf,
    /// Showing one turn's changes instead of the whole task's.
    pub turn: Option<TurnFilter>,
    base: String,
    pub files: Vec<FileDiff>,
    pub totals: (u32, u32),
    /// Files that would conflict merging now (pre-check).
    pub conflicts: Vec<PathBuf>,
    collapsed: HashSet<usize>,
    pub comments: Vec<Comment>,
    draft: Option<((usize, usize, usize), Entity<TextInput>)>,
    loading: bool,
    error: Option<String>,
    /// Main checkout + branch, for the pre-check.
    root: PathBuf,
    branch: String,
    scroll: UniformListScrollHandle,
    refresh_task: Option<Task<()>>,
    pub merged: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ChangesEvent> for ChangesView {}

impl ChangesView {
    pub fn new(root: PathBuf, worktree: PathBuf, base: String, branch: String, cx: &mut Context<Self>) -> Self {
        let mut v = ChangesView {
            worktree,
            turn: None,
            base,
            files: Vec::new(),
            totals: (0, 0),
            conflicts: Vec::new(),
            collapsed: HashSet::new(),
            comments: Vec::new(),
            draft: None,
            loading: false,
            error: None,
            root,
            branch,
            scroll: UniformListScrollHandle::new(),
            refresh_task: None,
            merged: false,
            _subs: Vec::new(),
        };
        v.refresh(cx);
        v
    }

    /// Show what one turn changed (the chat's View, Show Last Turn's Changes).
    pub fn show_turn(&mut self, n: u32, start: String, end: Option<String>, cx: &mut Context<Self>) {
        if self.turn.as_ref().map(|t| t.n) != Some(n) {
            self.files.clear();
            self.collapsed.clear();
            self.draft = None;
        }
        self.turn = Some(TurnFilter { n, start, end });
        self.refresh(cx);
    }

    /// Back to everything the task changed (Review Changes, Show All).
    pub fn show_all(&mut self, cx: &mut Context<Self>) {
        if self.turn.take().is_some() {
            self.files.clear();
            self.collapsed.clear();
            self.draft = None;
        }
        self.refresh(cx);
    }

    /// Recompute the diff and the conflict pre-check off the UI thread.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (wt, base, root, branch) = (self.worktree.clone(), self.base.clone(), self.root.clone(), self.branch.clone());
        let turn = self.turn.clone();
        let bg = cx.background_executor().clone();
        self.loading = true;
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let (diff, conflicts) = bg
                .spawn(async move {
                    let diff = match turn {
                        Some(t) => t.end.map(Ok).unwrap_or_else(|| qur_git::snapshot_tree(&wt)).and_then(|end| qur_git::tree_diff(&wt, &t.start, &end)),
                        None => qur_git::task_diff(&wt, &base),
                    };
                    // Pre-check what the merge would do, including uncommitted work:
                    // `git stash create` makes a commit object without touching anything.
                    let tip = qur_worktree::git(&wt, &["stash", "create"]).ok().filter(|s| !s.is_empty()).unwrap_or(branch);
                    let conflicts = qur_git::precheck(&root, &tip).unwrap_or_default();
                    (diff, conflicts)
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.loading = false;
                match diff {
                    Ok(files) => {
                        v.totals = files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
                        v.files = files;
                        v.error = None;
                    }
                    Err(e) => v.error = Some(e),
                }
                v.conflicts = conflicts;
                cx.notify();
            });
        }));
    }

    fn rows(&self) -> Vec<Row> {
        rows_for(&self.files, &self.collapsed, &self.comments, self.draft.as_ref().map(|(k, _)| *k))
    }

    fn start_comment(&mut self, key: (usize, usize, usize), window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new(COMMENT_PLACEHOLDER, cx));
        let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, _, cx| match ev {
            InputEvent::Confirm { .. } => {
                let text = input.read(cx).text.trim().to_string();
                if !text.is_empty() {
                    let (f, h, l) = key;
                    let file = &this.files[f];
                    let line = &file.hunks[h].lines[l];
                    this.comments.push(Comment {
                        path: file.path.clone(),
                        line: line.new_no.or(line.old_no).unwrap_or(0),
                        quote: line.text.clone(),
                        text,
                    });
                }
                this.draft = None;
                cx.notify();
            }
            InputEvent::Cancel => {
                this.draft = None;
                cx.notify();
            }
            _ => {}
        });
        self._subs.push(sub);
        let f = input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.draft = Some((key, input));
        cx.notify();
    }

    fn send_comments(&mut self, cx: &mut Context<Self>) {
        if self.comments.is_empty() {
            return;
        }
        let s = comments_prompt(
            "Review comments on your changes. Please address each one:",
            self.comments.iter().map(|c| (format!("{}:{}", c.path.display(), c.line), c.quote.as_str(), c.text.as_str())),
        );
        self.comments.clear();
        cx.emit(ChangesEvent::SendComments(s));
        cx.notify();
    }

    pub fn abs(&self, rel: &Path) -> PathBuf {
        self.worktree.join(rel)
    }
}

impl Render for ChangesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let button = |label: SharedString, id: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .px(look.z(10.))
                .py(look.z(3.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_size(look.text(-1.))
                .bg(look.c(if primary { ui.button_primary_bg } else { ui.button_bg }))
                .text_color(look.c(if primary { ui.button_primary_text } else { ui.button_text }))
                .child(label)
        };
        let n = self.files.len();
        let header = div()
            .flex_none()
            .px(look.z(12.))
            .py(look.z(8.))
            .flex()
            .items_center()
            .gap(look.z(8.))
            .border_b_1()
            .border_color(look.c(ui.border))
            .child(
                div()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .gap(look.z(6.))
                            .text_size(look.text(-1.))
                            .children(self.turn.as_ref().map(|t| div().text_color(look.c(ui.text)).font_weight(gpui::FontWeight::MEDIUM).child(format!("Turn {}:", t.n))))
                            .child(div().text_color(look.c(ui.text)).child(if self.loading && n == 0 { "Loading changes…".to_string() } else { format!("{n} file{} changed", if n == 1 { "" } else { "s" }) }))
                            .child(div().text_color(look.c(look.theme.diff.added_text)).child(format!("+{}", self.totals.0)))
                            .child(div().text_color(look.c(look.theme.diff.removed_text)).child(format!("−{}", self.totals.1))),
                    )
                    .when(!self.conflicts.is_empty(), |d| {
                        let names = self.conflicts.iter().take(3).map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
                        d.child(div().truncate().text_size(look.text(-2.)).text_color(look.c(ui.warning)).child(format!("⚠ would conflict with your branch: {names}{}", if self.conflicts.len() > 3 { "…" } else { "" })))
                    })
                    .when(self.merged, |d| d.child(div().text_size(look.text(-2.)).text_color(look.c(ui.success)).child("Merged into your branch.")))
                    .children(self.error.clone().map(|e| div().text_size(look.text(-2.)).text_color(look.c(ui.error)).child(e))),
            )
            .child(div().flex_1())
            .when(!self.comments.is_empty(), |d| {
                let c = self.comments.len();
                d.child(button(format!("Send {c} comment{}", if c == 1 { "" } else { "s" }).into(), "send-comments", false).on_click(cx.listener(|v, _, _, cx| v.send_comments(cx))))
            })
            .when(self.turn.is_some(), |d| d.child(button("All changes".into(), "changes-all", false).on_click(cx.listener(|v, _, _, cx| v.show_all(cx)))))
            .child(button("Refresh".into(), "changes-refresh", false).on_click(cx.listener(|v, _, _, cx| v.refresh(cx))))
            .child(button("Discard…".into(), "changes-discard", false).on_click(cx.listener(|_, _, _, cx| cx.emit(ChangesEvent::Discard))))
            .child(button("Keep working".into(), "changes-keep", false).on_click(cx.listener(|_, _, _, cx| cx.emit(ChangesEvent::KeepWorking))))
            .child(button("Merge".into(), "changes-merge", true).on_click(cx.listener(|_, _, _, cx| cx.emit(ChangesEvent::Merge))));
        let rows = self.rows();
        let count = rows.len();
        let body = if n == 0 && !self.loading {
            div().flex_1().flex().items_center().justify_center().text_color(look.c(ui.text_muted)).child(if self.turn.is_some() { "This turn changed no files." } else { "No changes yet." }).into_any_element()
        } else {
            uniform_list(
                "changes",
                count,
                cx.processor(move |this, range: Range<usize>, window, cx| {
                    let look = Look::of(window, cx);
                    let ui = look.theme.ui.clone();
                    let rows = this.rows();
                    range
                        .map(|i| {
                            let row = rows[i];
                            match row {
                                Row::Comment(ci) => {
                                    let c = &this.comments[ci];
                                    div()
                                        .id(("comment", i))
                                        .w_full()
                                        .h(look.z(look.code_size * 1.55))
                                        .pl(look.z(98.))
                                        .flex()
                                        .items_center()
                                        .gap(look.z(6.))
                                        .bg(look.c(ui.permission_bar_bg))
                                        .text_size(look.text(-1.))
                                        .child(div().flex_none().text_color(look.c(ui.text_muted)).child("💬"))
                                        .child(div().min_w(px(0.)).truncate().text_color(look.c(ui.text)).child(c.text.clone()))
                                        .child(
                                            div()
                                                .id(("comment-x", i))
                                                .flex_none()
                                                .px(look.z(8.))
                                                .cursor_pointer()
                                                .text_color(look.c(ui.text_muted))
                                                .child("×")
                                                .on_click(cx.listener(move |v, _, _, cx| {
                                                    if ci < v.comments.len() {
                                                        v.comments.remove(ci);
                                                    }
                                                    cx.notify();
                                                })),
                                        )
                                        .into_any_element()
                                }
                                Row::Draft => {
                                    let input = this.draft.as_ref().map(|(_, i)| i.clone());
                                    div()
                                        .id(("draft", i))
                                        .w_full()
                                        .h(look.z(look.code_size * 1.55))
                                        .pl(look.z(98.))
                                        .pr(look.z(12.))
                                        .flex()
                                        .items_center()
                                        .bg(look.c(ui.permission_bar_bg))
                                        .children(input)
                                        .into_any_element()
                                }
                                Row::File(f) => {
                                    let collapsed = this.collapsed.contains(&f);
                                    let path = this.abs(&this.files[f].path);
                                    diff_row(&this.files, row, &look, collapsed)
                                        .id(("file", i))
                                        .w_full()
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |v, ev: &gpui::ClickEvent, _, cx| {
                                            if ev.modifiers().platform {
                                                cx.emit(ChangesEvent::Open(path.clone(), None));
                                            } else if !v.collapsed.remove(&f) {
                                                v.collapsed.insert(f);
                                            }
                                            cx.notify();
                                        }))
                                        .into_any_element()
                                }
                                Row::Line(f, h, l) => {
                                    let path = this.abs(&this.files[f].path);
                                    let line_no = this.files[f].hunks[h].lines[l].new_no;
                                    diff_row(&this.files, row, &look, false)
                                        .id(("line", i))
                                        .w_full()
                                        .cursor_pointer()
                                        .hover(|d| d.opacity(0.85))
                                        .on_click(cx.listener(move |v, ev: &gpui::ClickEvent, window, cx| {
                                            if ev.modifiers().platform {
                                                cx.emit(ChangesEvent::Open(path.clone(), line_no));
                                            } else {
                                                v.start_comment((f, h, l), window, cx);
                                            }
                                        }))
                                        .into_any_element()
                                }
                                Row::Hunk(..) => diff_row(&this.files, row, &look, false).w_full().into_any_element(),
                            }
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .flex_1()
            .into_any_element()
        };
        let hint = div()
            .flex_none()
            .px(look.z(12.))
            .py(look.z(3.))
            .border_t_1()
            .border_color(look.c(ui.border))
            .text_size(look.text(-2.))
            .text_color(look.c(ui.text_muted))
            .child("Click a line to comment · ⌘-click to open the file · comments go to the agent in one message");
        div().size_full().flex().flex_col().bg(look.surface(ui.editor_bg)).child(header).child(body).child(hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_go_as_one_numbered_message() {
        let s = comments_prompt("Review comments on your changes. Please address each one:", [("src/a.rs:3".to_string(), "  let x = 1;", "Name it count"), ("b.md:1".into(), "# Title", "Shorter")]);
        assert_eq!(s, "Review comments on your changes. Please address each one:\n\n1. `src/a.rs:3` — `let x = 1;`\n   Name it count\n\n2. `b.md:1` — `# Title`\n   Shorter\n\n");
    }
}
