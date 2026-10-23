//! One task's live picture of its worktree: a file watcher (FSEvents) plus
//! the heatmap model. ACP tool calls (intent, they arrive first) and watcher
//! batches (truth: shell commands, formatters, codegen) both feed it. The
//! Files tree paints from it and the Live view follows its focus.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui::{Context, EventEmitter, Task};
use qur_heat::{Heatmap, ToolKind, ToolStatus};
use qur_theme::Rgba;

pub enum MonitorEvent {
    /// Heat changed: repaint the tree.
    Changed,
    /// Files changed on disk (absolute paths): reload open views.
    Files(Vec<PathBuf>),
}

pub struct TaskMonitor {
    pub worktree: PathBuf,
    pub accent: Rgba,
    pub heat: Heatmap,
    base: String,
    /// The agent is mid-turn: writes count as edits.
    pub turn_active: bool,
    _watcher: Option<qur_watch::Watcher>,
    _reader: Option<Task<()>>,
    recompute: Option<Task<()>>,
    tick: Option<(Instant, Task<()>)>,
    /// First watcher event seen at (QUR_BENCH=glow).
    pub last_event_at: Option<Instant>,
}

impl EventEmitter<MonitorEvent> for TaskMonitor {}

/// Debounce for watcher batches (handoff section 6).
const DEBOUNCE: Duration = Duration::from_millis(50);

impl TaskMonitor {
    pub fn new(worktree: PathBuf, base: String, accent: Rgba, cx: &mut Context<Self>) -> Self {
        let (watcher, reader) = match qur_watch::Watcher::new(&worktree, DEBOUNCE) {
            Ok((w, mut changes)) => {
                let task = cx.spawn(async move |this, cx| {
                    while let Some(batch) = changes.next().await {
                        if this.update(cx, |m, cx| m.on_files(batch, cx)).is_err() {
                            break;
                        }
                    }
                });
                (Some(w), Some(task))
            }
            Err(_) => (None, None),
        };
        let mut m = TaskMonitor {
            heat: Heatmap::new(worktree.clone(), base.clone()),
            worktree,
            accent,
            base,
            turn_active: false,
            _watcher: watcher,
            _reader: reader,
            recompute: None,
            tick: None,
            last_event_at: None,
        };
        m.full_scan(cx);
        m
    }

    /// Diff stats for everything that already differs from the base (restored tasks).
    fn full_scan(&mut self, cx: &mut Context<Self>) {
        let (wt, base) = (self.worktree.clone(), self.base.clone());
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let stats = bg.spawn(async move { qur_heat::full_scan(&wt, &base) }).await;
            this.update(cx, |m, cx| {
                for (rel, s) in stats {
                    m.heat.set_stat(&rel, Some(s));
                }
                cx.emit(MonitorEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn on_tool(&mut self, id: &str, kind: Option<ToolKind>, status: Option<ToolStatus>, paths: &[PathBuf], cx: &mut Context<Self>) {
        self.heat.on_tool(id, kind, status, paths, Instant::now());
        // A completed edit also schedules a diff-stat recompute.
        self.schedule_recompute(cx);
        self.changed(cx);
    }

    /// A line number from an ACP location (Live scrolls there).
    pub fn tool_line(&mut self, id: &str, path: &Path, line: u32) {
        self.heat.set_tool_line(id, path, line);
    }

    pub fn on_permission(&mut self, paths: &[PathBuf], pending: bool, cx: &mut Context<Self>) {
        self.heat.on_permission(paths, pending);
        self.changed(cx);
    }

    pub fn set_turn_active(&mut self, on: bool) {
        self.turn_active = on;
    }

    fn on_files(&mut self, batch: Vec<PathBuf>, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.last_event_at = Some(now);
        self.heat.on_fs_change(&batch, self.turn_active, now);
        cx.emit(MonitorEvent::Files(batch));
        self.schedule_recompute(cx);
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(MonitorEvent::Changed);
        cx.notify();
        self.schedule_tick(cx);
    }

    /// Recompute `+n -m` for files 150 ms after their last write (never the
    /// whole tree per event).
    fn schedule_recompute(&mut self, cx: &mut Context<Self>) {
        if self.recompute.is_some() {
            return;
        }
        let bg = cx.background_executor().clone();
        self.recompute = Some(cx.spawn(async move |this, cx| {
            loop {
                bg.timer(Duration::from_millis(160)).await;
                let job = this.update(cx, |m, _| {
                    let due = m.heat.take_due(Instant::now());
                    (due, m.worktree.clone(), m.base.clone())
                });
                let Ok((due, wt, base)) = job else { return };
                if due.is_empty() {
                    let more = this.update(cx, |m, _| m.heat.next_deadline(Instant::now()).is_some()).unwrap_or(false);
                    if !more {
                        break;
                    }
                    continue;
                }
                let stats = bg.spawn(async move { qur_heat::diff_stats(&wt, &base, &due) }).await;
                let _ = this.update(cx, |m, cx| {
                    for (rel, s) in stats {
                        m.heat.set_stat(&rel, s);
                    }
                    cx.emit(MonitorEvent::Changed);
                    cx.notify();
                });
            }
            let _ = this.update(cx, |m, _| m.recompute = None);
        }));
    }

    /// One timer for the next visual change (fades, editing timeouts), so the
    /// tree repaints exactly when needed instead of polling.
    fn schedule_tick(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let Some(at) = self.heat.next_deadline(now) else { return };
        if let Some((t, _)) = &self.tick
            && *t <= at
            && *t > now
        {
            return;
        }
        let bg = cx.background_executor().clone();
        // At most ~30 repaints a second from heat timers, however close the deadline.
        let wait = at.saturating_duration_since(now).max(Duration::from_millis(33));
        self.tick = Some((
            at,
            cx.spawn(async move |this, cx| {
                bg.timer(wait).await;
                let _ = this.update(cx, |m, cx| {
                    m.tick = None;
                    cx.emit(MonitorEvent::Changed);
                    cx.notify();
                    m.schedule_tick(cx);
                });
            }),
        ));
    }

    pub fn rel(&self, abs: &Path) -> Option<PathBuf> {
        abs.strip_prefix(&self.worktree).ok().map(Path::to_path_buf)
    }
}

/// ACP kinds and statuses to the heat model's.
pub fn heat_kind(k: qur_acp::ToolKind) -> ToolKind {
    use qur_acp::ToolKind as A;
    match k {
        A::Read => ToolKind::Read,
        A::Search => ToolKind::Search,
        A::Edit => ToolKind::Edit,
        A::Delete => ToolKind::Delete,
        A::Move => ToolKind::Move,
        A::Execute => ToolKind::Execute,
        _ => ToolKind::Other,
    }
}

pub fn heat_status(s: qur_acp::ToolStatus) -> ToolStatus {
    use qur_acp::ToolStatus as A;
    match s {
        A::Pending => ToolStatus::Pending,
        A::InProgress => ToolStatus::InProgress,
        A::Completed => ToolStatus::Completed,
        A::Failed => ToolStatus::Failed,
    }
}
