//! Git panel (M4, Ctrl+Shift+G): the main checkout's status, stage and
//! unstage per file, the selected file's diff, and commit. Also shows a merge
//! in progress (conflicts) with Finish / Abort.

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{App, Context, Entity, EventEmitter, SharedString, Subscription, Task, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list};
use qur_git::{FileDiff, Status};

use crate::changes_view::{Row, diff_row, rows_for};
use crate::input::{InputEvent, TextInput};
use crate::look::Look;

pub enum GitEvent {
    Open(PathBuf),
    FinishMerge,
    AbortMerge,
    Error(String),
    Committed(String),
}

#[derive(Clone, PartialEq)]
struct Pick {
    path: PathBuf,
    staged: bool,
}

pub struct GitPanel {
    pub root: PathBuf,
    pub status: Status,
    selected: Option<Pick>,
    diff: Vec<FileDiff>,
    message: Entity<TextInput>,
    scroll: UniformListScrollHandle,
    refreshing: Option<Task<()>>,
    error: Option<String>,
    _sub: Subscription,
}

impl EventEmitter<GitEvent> for GitPanel {}

impl GitPanel {
    /// QUR_DEBUG_ACTIONS `state`.
    pub fn debug_state(&self, cx: &App) -> String {
        let s = &self.status;
        let pick = match &self.selected {
            Some(p) => format!("{}:{}", if p.staged { "staged" } else { "unstaged" }, p.path.display()),
            None => "-".into(),
        };
        let diff: Vec<String> = self.diff.iter().map(|d| format!("{}+{}-{}", d.path.display(), d.added, d.removed)).collect();
        format!(
            "message={:?} head={} staged={} unstaged={} untracked={} selected={pick} diff=[{}]",
            self.message.read(cx).text,
            s.head.as_deref().map(|h| &h[..h.len().min(8)]).unwrap_or("-"),
            s.staged.len(),
            s.unstaged.len(),
            s.untracked.len(),
            diff.join(" ")
        )
    }

    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| TextInput::new("Commit message (⌘↩ commit)", cx));
        let sub = cx.subscribe_in(&message, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Confirm { all: true, .. } = ev {
                this.commit(cx)
            }
        });
        let mut p = GitPanel {
            root,
            status: Status::default(),
            selected: None,
            diff: Vec::new(),
            message,
            scroll: UniformListScrollHandle::new(),
            refreshing: None,
            error: None,
            _sub: sub,
        };
        p.refresh(cx);
        p
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let pick = self.selected.clone();
        let bg = cx.background_executor().clone();
        self.refreshing = Some(cx.spawn(async move |this, cx| {
            let (status, diff) = bg
                .spawn(async move {
                    let status = qur_git::status(&root);
                    let diff = pick.and_then(|p| qur_git::file_diff(&root, &p.path, p.staged).ok().flatten());
                    (status, diff)
                })
                .await;
            let _ = this.update(cx, |p, cx| {
                match status {
                    Ok(s) => {
                        p.status = s;
                        p.error = None;
                    }
                    Err(e) => p.error = Some(e),
                }
                p.diff = diff.into_iter().collect();
                // Staging or unstaging moves the selected file between lists:
                // follow it there, or drop it once it has no changes left.
                if let Some(pick) = p.selected.as_mut() {
                    let st = &p.status;
                    let in_staged = st.staged.iter().any(|e| e.path == pick.path);
                    let in_unstaged = st.unstaged.iter().any(|e| e.path == pick.path) || st.untracked.contains(&pick.path);
                    match (pick.staged, in_staged, in_unstaged) {
                        (true, false, true) | (false, true, false) => {
                            pick.staged = !pick.staged;
                            p.refresh(cx);
                        }
                        (_, false, false) => p.selected = None,
                        _ => {}
                    }
                }
                cx.notify();
            });
        }));
    }

    fn run(&mut self, f: impl FnOnce(&Path) -> Result<(), String> + Send + 'static, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let r = bg.spawn(async move { f(&root) }).await;
            let _ = this.update(cx, |p, cx| {
                if let Err(e) = r {
                    cx.emit(GitEvent::Error(e));
                }
                p.refresh(cx);
            });
        })
        .detach();
    }

    fn stage(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.run(move |r| qur_git::stage(r, &[path]), cx)
    }
    fn unstage(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.run(move |r| qur_git::unstage(r, &[path]), cx)
    }
    fn stage_all(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.status.unstaged.iter().map(|e| e.path.clone()).chain(self.status.untracked.iter().cloned()).collect();
        if !paths.is_empty() {
            self.run(move |r| qur_git::stage(r, &paths), cx)
        }
    }

    /// Palette/menu: stage or unstage everything, from fresh status (the
    /// panel's may be stale when it isn't shown).
    pub fn stage_everything(&mut self, cx: &mut Context<Self>) {
        self.run(
            |r| {
                let st = qur_git::status(r)?;
                let paths: Vec<PathBuf> = st.unstaged.iter().map(|e| e.path.clone()).chain(st.untracked.iter().cloned()).collect();
                if paths.is_empty() { Ok(()) } else { qur_git::stage(r, &paths) }
            },
            cx,
        )
    }

    pub fn unstage_everything(&mut self, cx: &mut Context<Self>) {
        self.run(
            |r| {
                let st = qur_git::status(r)?;
                let paths: Vec<PathBuf> = st.staged.iter().map(|e| e.path.clone()).collect();
                if paths.is_empty() { Ok(()) } else { qur_git::unstage(r, &paths) }
            },
            cx,
        )
    }

    /// Palette/menu Commit: commits with the typed message, or focuses the
    /// message field when it's empty.
    pub fn commit_or_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.message.read(cx).text.trim().is_empty() {
            let f = self.message.read(cx).focus.clone();
            window.focus(&f, cx);
            return cx.emit(GitEvent::Error("Write a commit message, then ⌘↩".into()));
        }
        self.commit(cx)
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let msg = self.message.read(cx).text.trim().to_string();
        if msg.is_empty() {
            return cx.emit(GitEvent::Error("Write a commit message first".into()));
        }
        let root = self.root.clone();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let r = bg.spawn(async move { qur_git::commit(&root, &msg) }).await;
            let _ = this.update(cx, |p, cx| {
                match r {
                    Ok(sha) => {
                        p.message.update(cx, |m, cx| m.set_text("", cx));
                        cx.emit(GitEvent::Committed(sha));
                    }
                    Err(e) => cx.emit(GitEvent::Error(e)),
                }
                p.refresh(cx);
            });
        })
        .detach();
    }

    fn select(&mut self, path: PathBuf, staged: bool, cx: &mut Context<Self>) {
        self.selected = Some(Pick { path, staged });
        self.refresh(cx);
    }
}

impl Render for GitPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(window, cx);
        let ui = look.theme.ui.clone();
        let st = self.status.clone();
        let section = |label: String| {
            div().px(look.z(12.)).pt(look.z(10.)).pb(look.z(4.)).text_size(look.text(-2.)).font_weight(gpui::FontWeight::MEDIUM).text_color(look.c(ui.text_muted)).child(label)
        };
        let entry = |path: PathBuf, code: char, staged: bool, id: SharedString, cx: &mut Context<Self>| {
            let sel = self.selected.as_ref().is_some_and(|p| p.path == path && p.staged == staged);
            let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
            let name = format!("{}:{}", if staged { "staged" } else { "unstaged" }, path.display());
            div()
                .id(id)
                .relative()
                .children(crate::script::mark(|| format!("git-row:{name}")))
                .mx(look.z(4.))
                .px(look.z(8.))
                .h(look.z(22.))
                .rounded(look.z(4.))
                .flex()
                .items_center()
                .gap(look.z(8.))
                .cursor_pointer()
                .text_size(look.text(-1.))
                .when(sel, |d| d.bg(look.c(ui.active_bg)))
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .child(div().w(look.z(12.)).flex_none().text_color(look.c(ui.text_muted)).child(code.to_string()))
                .child(div().flex_1().min_w(px(0.)).truncate().text_color(look.c(ui.text_secondary)).child(path.display().to_string()))
                .child(
                    div()
                        .id(SharedString::from(format!("act-{}-{}", staged, path.display())))
                        .flex_none()
                        .px(look.z(6.))
                        .rounded(look.z(3.))
                        .text_color(look.c(ui.text_muted))
                        .hover(|d| d.bg(look.c(ui.active_bg)).text_color(look.c(ui.text)))
                        .relative()
                        .child(if staged { "−" } else { "+" })
                        .children(crate::script::mark(|| format!("git-act:{}:{}", if staged { "staged" } else { "unstaged" }, p1.display())))
                        .on_click(cx.listener(move |p, _, _, cx| {
                            cx.stop_propagation();
                            if staged { p.unstage(p1.clone(), cx) } else { p.stage(p1.clone(), cx) }
                        })),
                )
                .on_click(cx.listener(move |p, ev: &gpui::ClickEvent, _, cx| {
                    if ev.modifiers().platform {
                        cx.emit(GitEvent::Open(p.root.join(&p2)));
                    } else {
                        p.select(p3.clone(), staged, cx);
                    }
                }))
        };
        let mut list: Vec<gpui::AnyElement> = Vec::new();
        if st.merging || !st.conflicted.is_empty() {
            list.push(section(format!("MERGE IN PROGRESS · {} CONFLICTED", st.conflicted.len())).into_any_element());
            for (i, p) in st.conflicted.iter().enumerate() {
                list.push(entry(p.clone(), 'U', false, format!("c{i}").into(), cx).into_any_element());
            }
            list.push(
                div()
                    .px(look.z(12.))
                    .py(look.z(6.))
                    .flex()
                    .gap(look.z(6.))
                    .child(
                        div()
                            .id("finish-merge")
                            .px(look.z(10.))
                            .py(look.z(3.))
                            .rounded(look.z(4.))
                            .cursor_pointer()
                            .text_size(look.text(-1.))
                            .bg(look.c(ui.button_primary_bg))
                            .text_color(look.c(ui.button_primary_text))
                            .child("Finish merge")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(GitEvent::FinishMerge))),
                    )
                    .child(
                        div()
                            .id("abort-merge")
                            .px(look.z(10.))
                            .py(look.z(3.))
                            .rounded(look.z(4.))
                            .cursor_pointer()
                            .text_size(look.text(-1.))
                            .bg(look.c(ui.button_bg))
                            .text_color(look.c(ui.button_text))
                            .child("Abort")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(GitEvent::AbortMerge))),
                    )
                    .into_any_element(),
            );
        }
        list.push(section(format!("STAGED · {}", st.staged.len())).into_any_element());
        for (i, e) in st.staged.iter().enumerate() {
            list.push(entry(e.path.clone(), e.code, true, format!("s{i}").into(), cx).into_any_element());
        }
        list.push(
            section(format!("CHANGES · {}", st.unstaged.len() + st.untracked.len()))
                .flex()
                .child(div().flex_1())
                .child(
                    div()
                        .id("stage-all")
                        .cursor_pointer()
                        .text_color(look.c(ui.text_secondary))
                        .child("stage all")
                        .on_click(cx.listener(|p, _, _, cx| p.stage_all(cx))),
                )
                .into_any_element(),
        );
        for (i, e) in st.unstaged.iter().enumerate() {
            list.push(entry(e.path.clone(), e.code, false, format!("u{i}").into(), cx).into_any_element());
        }
        for (i, p) in st.untracked.iter().enumerate() {
            list.push(entry(p.clone(), '?', false, format!("n{i}").into(), cx).into_any_element());
        }
        let branch = match (&st.branch, &st.head) {
            (Some(b), _) => b.clone(),
            (None, Some(h)) => format!("detached at {}", &h[..h.len().min(8)]),
            _ => "no commits yet".into(),
        };
        let left = div()
            .w(look.z(320.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(look.c(ui.border))
            .child(
                div()
                    .flex_none()
                    .px(look.z(12.))
                    .py(look.z(8.))
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .border_b_1()
                    .border_color(look.c(ui.border))
                    .child(div().font_weight(gpui::FontWeight::MEDIUM).child(branch))
                    .when(st.ahead + st.behind > 0, |d| d.child(div().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(format!("↑{} ↓{}", st.ahead, st.behind))))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("git-refresh")
                            .cursor_pointer()
                            .text_size(look.text(-1.))
                            .text_color(look.c(ui.text_muted))
                            .child("↻")
                            .on_click(cx.listener(|p, _, _, cx| p.refresh(cx))),
                    ),
            )
            .child(div().id("git-list").flex_1().min_h(px(0.)).overflow_y_scroll().children(list))
            .children(self.error.clone().map(|e| div().px(look.z(12.)).py(look.z(4.)).text_size(look.text(-2.)).text_color(look.c(ui.error)).child(e)))
            .child(
                div()
                    .flex_none()
                    .p(look.z(10.))
                    .flex()
                    .flex_col()
                    .gap(look.z(6.))
                    .border_t_1()
                    .border_color(look.c(ui.border))
                    .child(div().px(look.z(8.)).rounded(look.z(4.)).border_1().border_color(look.c(ui.border)).bg(look.c(ui.editor_bg)).child(self.message.clone()))
                    .child(
                        div()
                            .id("commit")
                            .px(look.z(10.))
                            .py(look.z(4.))
                            .rounded(look.z(4.))
                            .cursor_pointer()
                            .text_size(look.text(-1.))
                            .bg(look.c(ui.button_primary_bg))
                            .text_color(look.c(ui.button_primary_text))
                            .flex()
                            .justify_center()
                            .child(format!("Commit {} staged", st.staged.len()))
                            .on_click(cx.listener(|p, _, _, cx| p.commit(cx))),
                    ),
            );
        let rows = rows_for(&self.diff, &HashSet::new(), &[], None);
        let n = rows.len();
        let right = if self.diff.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(look.c(ui.text_muted))
                .child(if self.selected.is_some() { "No differences." } else { "Select a file to see its diff." })
                .into_any_element()
        } else {
            uniform_list(
                "git-diff",
                n,
                cx.processor(move |this, range: Range<usize>, window, cx| {
                    let look = Look::of(window, cx);
                    let rows = rows_for(&this.diff, &HashSet::new(), &[], None);
                    range.map(|i| diff_row(&this.diff, rows[i], &look, false).w_full().into_any_element()).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .flex_1()
            .into_any_element()
        };
        let _ = Row::Draft;
        div().size_full().flex().bg(look.region_bg(ui.editor_bg)).child(left).child(div().flex_1().min_w(px(0.)).flex().flex_col().child(right))
    }
}
