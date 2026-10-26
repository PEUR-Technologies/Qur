//! Heatmap model for one task worktree (handoff section 9).
//!
//! Two sources feed it: ACP tool calls (intent, first) and the worktree file watcher
//! (shell commands, formatters, codegen). It is a pure state machine: time only enters
//! through the `now` parameters, so the UI decides when to re-render (`next_deadline`)
//! and tests are deterministic. The blocking git helpers live in [`git`].
//!
//! A project-folder agent shares its folder with you ([`Heatmap::set_shared`]): there
//! only files the agent touched glow, and a write counts as the agent's only when its tool
//! calls name the file, or (shell commands, Agent Terminals) while it plausibly made it.
//! Writes Qur itself made (editor saves, Replace All) are never the agent's anywhere: see
//! [`OwnWrites`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod git;
mod own;
pub use git::{diff_stats, full_scan};
pub use own::{OWN_WINDOW, OwnWrites, own_writes};

/// Whether the agent shares its folder with you, and how its writes are told from yours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Shared {
    /// Its own worktree: every write during a turn is the agent's.
    #[default]
    No,
    /// The project folder, structured (ACP) agent: a turn write is its when an edit tool
    /// names the file (locations or diffs), or when a shell command (or a tool of unknown
    /// kind) is running or just finished and the file isn't yours.
    Tools,
    /// The project folder, Agent Terminal (no tool calls, no paths): a write is its while
    /// the terminal is busy (the caller's `turn_active`), unless the file is yours.
    Busy,
}

/// How long Reading glows after its tool completes, and Editing after the last write.
pub const FADE: Duration = Duration::from_millis(1500);
/// Diff stat recompute delay after the last write to a file.
pub const RECOMPUTE: Duration = Duration::from_millis(150);
/// Heat level cap.
pub const MAX_LEVEL: u8 = 4;
/// Shared folders: a tool still accounts for writes this long after it finished (the
/// watcher reports a write ~50-300 ms after it lands; Claude names an edit's file 0.5-0.9 s
/// after the call), and a write no tool accounts for waits this long for an edit tool to
/// name its file.
pub const TOOL_GRACE: Duration = Duration::from_secs(3);

/// Per-file state, in priority order (lowest first, so `max` picks the strongest).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum HeatState {
    #[default]
    Idle,
    Changed,
    Deleted,
    Created,
    Reading,
    Editing,
    NeedsApproval,
}

/// Mirrors the ACP tool kinds the app passes in (`think`/`fetch`/... map to `Other`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolKind {
    Read,
    Search,
    Edit,
    Delete,
    Move,
    Execute,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ToolStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl ToolStatus {
    fn done(self) -> bool {
        matches!(self, ToolStatus::Completed | ToolStatus::Failed)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub added: u32,
    pub removed: u32,
    pub created: bool,
    pub deleted: bool,
    pub binary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub state: HeatState,
    /// 0..=4: how often the agent touched the file this task.
    pub level: u8,
    pub stat: Option<DiffStat>,
    /// 0..=1 intensity of a transient state: Reading fades 1 -> 0 over 1.5 s after its tool
    /// completes; write-driven Editing counts down the same way. 1 while a tool is active and
    /// for steady states; 0 for Idle. Pulsing itself is the UI's animation.
    pub fade: f32,
}

#[derive(Default)]
struct Tool {
    kind: Option<ToolKind>,
    status: ToolStatus,
    /// Relative paths, in arrival order.
    paths: Vec<PathBuf>,
    completed_at: Option<Instant>,
}

impl Tool {
    fn reads(&self) -> bool {
        matches!(self.kind, Some(ToolKind::Read | ToolKind::Search))
    }
    fn edits(&self) -> bool {
        matches!(self.kind, Some(ToolKind::Edit | ToolKind::Delete | ToolKind::Move))
    }
    /// Running, or finished within [`TOOL_GRACE`].
    fn recent(&self, now: Instant) -> bool {
        !self.status.done() || self.completed_at.is_some_and(|c| now < c + TOOL_GRACE)
    }
    /// Could write any file without naming it (shell commands; a kind never reported).
    fn writes_anywhere(&self) -> bool {
        matches!(self.kind, None | Some(ToolKind::Execute))
    }
}

#[derive(Default)]
struct File {
    touches: u32,
    stat: Option<DiffStat>,
    approval: bool,
    /// Active read/search tools naming this file.
    reading: HashSet<String>,
    /// Active edit/delete/move tools naming this file.
    editing: HashSet<String>,
    /// Latest completion of a read tool on this file.
    read_done: Option<Instant>,
    /// Latest write seen by the watcher during a turn.
    turn_write: Option<Instant>,
    /// Pending diff-stat recompute.
    due: Option<Instant>,
}

struct Focus {
    rel: PathBuf,
    line: Option<u32>,
    /// Set by an ACP edit (vs. a watcher write) and when.
    from_tool: Option<String>,
    at: Instant,
}

pub struct Heatmap {
    worktree: PathBuf,
    base: String,
    tools: HashMap<String, Tool>,
    files: HashMap<PathBuf, File>,
    focus: Option<Focus>,
    /// tool id -> rel path -> line (from ACP locations).
    lines: HashMap<String, HashMap<PathBuf, u32>>,
    /// The worktree is the project folder, shared with you (see `set_shared`).
    shared: Shared,
    /// Shared folders: turn writes no tool accounts for yet, while an edit tool hasn't
    /// named its files (rel path, when). Claimed by `on_tool`, dropped after [`TOOL_GRACE`].
    held: Vec<(PathBuf, Instant)>,
}

impl Heatmap {
    pub fn new(worktree: PathBuf, base: String) -> Self {
        Heatmap {
            worktree,
            base,
            tools: HashMap::new(),
            files: HashMap::new(),
            focus: None,
            lines: HashMap::new(),
            shared: Shared::No,
            held: Vec::new(),
        }
    }

    /// The agent works in the project folder, alongside you (and maybe your other
    /// editors). Then only files the agent touched (named by its tools, or written while
    /// it was plausibly the writer) show heat and diff stats: your own uncommitted
    /// changes stay quiet. How a turn write is attributed: see [`Shared`].
    pub fn set_shared(&mut self, mode: Shared) {
        self.shared = mode;
    }

    pub fn shared(&self) -> bool {
        self.shared != Shared::No
    }

    /// Steady states (diff stats) show for this file.
    fn owned(&self, f: &File) -> bool {
        self.shared == Shared::No || f.touches > 0
    }

    /// An edit tool, running or just finished, names this file.
    fn named_by_edit(&self, rel: &Path, now: Instant) -> bool {
        self.tools.values().any(|t| t.edits() && t.recent(now) && t.paths.iter().any(|p| p == rel))
    }

    /// Is a turn write to `rel` the agent's? `yours` = the file is yours (open with
    /// unsaved changes in Qur). See [`Shared`].
    fn agents_write(&self, rel: &Path, yours: bool, now: Instant) -> bool {
        match self.shared {
            Shared::No => true,
            Shared::Busy => !yours,
            Shared::Tools => {
                self.named_by_edit(rel, now) || (!yours && self.tools.values().any(|t| t.writes_anywhere() && t.recent(now)))
            }
        }
    }

    pub fn worktree(&self) -> &Path {
        &self.worktree
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn rel(&self, abs: &Path) -> Option<PathBuf> {
        let rel = abs.strip_prefix(&self.worktree).ok()?;
        (!rel.as_os_str().is_empty()).then(|| rel.to_path_buf())
    }

    /// ACP tool call or update (merged by id: the first call may have no paths; later
    /// updates add them). `None` fields leave the stored value alone; a finished tool
    /// never goes back to active.
    pub fn on_tool(
        &mut self,
        id: &str,
        kind: Option<ToolKind>,
        status: Option<ToolStatus>,
        abs_paths: &[PathBuf],
        now: Instant,
    ) {
        let rels: Vec<PathBuf> = abs_paths.iter().filter_map(|p| self.rel(p)).collect();
        let tool = self.tools.entry(id.to_string()).or_default();
        if kind.is_some() {
            tool.kind = kind;
        }
        let mut became_done = false;
        if let Some(s) = status
            && !tool.status.done()
        {
            tool.status = s;
            if s.done() {
                tool.completed_at = Some(now);
                became_done = true;
            }
        }
        let mut added = Vec::new();
        for r in rels {
            if !tool.paths.contains(&r) {
                tool.paths.push(r.clone());
                added.push(r);
            }
        }
        // Each (tool, file) pair is one touch.
        for r in &added {
            self.files.entry(r.clone()).or_default().touches += 1;
        }
        // An edit tool named its file late (Claude): the write it made, seen already, glows.
        if self.tools[id].edits() && !added.is_empty() {
            let claimed: Vec<(PathBuf, Instant)> = self.held.extract_if(.., |(r, _)| added.contains(r)).collect();
            for (r, at) in claimed {
                let f = self.files.entry(r).or_default();
                f.turn_write = f.turn_write.max(Some(at));
            }
        }
        let tool = &self.tools[id];
        let (reads, edits, done, completed_at) = (tool.reads(), tool.edits(), tool.status.done(), tool.completed_at);
        let paths = tool.paths.clone();
        for r in &paths {
            let f = self.files.entry(r.clone()).or_default();
            f.reading.remove(id);
            f.editing.remove(id);
            if reads {
                if done {
                    f.read_done = f.read_done.max(completed_at);
                } else {
                    f.reading.insert(id.to_string());
                }
            }
            if edits {
                if done {
                    // Edit landed: recompute +n -m shortly (the watcher usually agrees).
                    if became_done || added.contains(r) {
                        let at = now + RECOMPUTE;
                        f.due = Some(f.due.map_or(at, |d| d.max(at)));
                    }
                } else {
                    f.editing.insert(id.to_string());
                }
            }
        }
        if edits && let Some(r) = added.last() {
            let line = self.lines.get(id).and_then(|m| m.get(r)).copied();
            self.focus = Some(Focus { rel: r.clone(), line, from_tool: Some(id.to_string()), at: now });
        }
    }

    /// Line (1-based) of a tool location, when ACP reports one. Feeds `focus`.
    /// (Addition to the spec'd API: `on_tool` takes bare paths.)
    pub fn set_tool_line(&mut self, id: &str, abs_path: &Path, line: u32) {
        let Some(rel) = self.rel(abs_path) else { return };
        if let Some(f) = &mut self.focus
            && f.from_tool.as_deref() == Some(id)
            && f.rel == rel
        {
            f.line = Some(line);
        }
        self.lines.entry(id.to_string()).or_default().insert(rel, line);
    }

    /// A permission request names these files (`pending = false` clears them).
    pub fn on_permission(&mut self, abs_paths: &[PathBuf], pending: bool) {
        for p in abs_paths {
            if let Some(r) = self.rel(p) {
                if pending {
                    self.files.entry(r).or_default().approval = true;
                } else if let Some(f) = self.files.get_mut(&r) {
                    f.approval = false;
                }
            }
        }
    }

    /// Clears every pending approval (e.g. the turn was cancelled).
    pub fn clear_permissions(&mut self) {
        for f in self.files.values_mut() {
            f.approval = false;
        }
    }

    /// Watcher batch for this worktree. `turn_active` = the agent is mid-turn: writes show
    /// as Editing for 1.5 s after the last write, and count as touches. Every path gets a
    /// diff-stat recompute 150 ms after its last write. `.git/` paths are ignored. Existing
    /// directories only get the recompute (git expands them); they don't glow or take focus.
    /// Pass Qur's own writes (see [`OwnWrites`]) with `turn_active = false`. In a shared
    /// folder a turn write also has to be the agent's (see [`Shared`]).
    pub fn on_fs_change(&mut self, abs_paths: &[PathBuf], turn_active: bool, now: Instant) {
        self.on_fs_change_with(abs_paths, turn_active, |_| false, now);
    }

    /// [`Heatmap::on_fs_change`], where `yours(abs)` says a file is yours (open with unsaved
    /// changes in Qur): in a shared folder a write to it is the agent's only when one of its
    /// edit tools names it.
    pub fn on_fs_change_with(&mut self, abs_paths: &[PathBuf], turn_active: bool, yours: impl Fn(&Path) -> bool, now: Instant) {
        self.held.retain(|(_, t)| now < *t + TOOL_GRACE);
        for abs in abs_paths {
            let Some(rel) = self.rel(abs) else { continue };
            if rel.starts_with(".git") {
                continue;
            }
            let is_dir = abs.is_dir();
            self.files.entry(rel.clone()).or_default().due = Some(now + RECOMPUTE);
            if !turn_active || is_dir {
                continue;
            }
            let mine = self.shared != Shared::No && yours(abs);
            if !self.agents_write(&rel, mine, now) {
                // Claude names an edit's file after the call: hold the write until it does.
                let unnamed_edit = self.tools.values().any(|t| t.edits() && t.recent(now) && t.paths.is_empty());
                if self.shared == Shared::Tools && !mine && unnamed_edit {
                    self.held.push((rel, now));
                }
                continue;
            }
            let named = self.shared == Shared::Tools && self.named_by_edit(&rel, now);
            let f = self.files.entry(rel.clone()).or_default();
            // A burst of writes (or the write an ACP edit just made) is one touch.
            let in_window = f.turn_write.is_some_and(|t| now < t + FADE);
            let by_tool = !f.editing.is_empty() || named;
            if !in_window && !by_tool {
                f.touches += 1;
            }
            f.turn_write = Some(now);
            let tool_focus_fresh = self.focus.as_ref().is_some_and(|fo| {
                fo.from_tool.as_ref().is_some_and(|id| {
                    now < fo.at + FADE || self.tools.get(id).is_some_and(|t| !t.status.done())
                })
            });
            let any_edit_active = self.tools.values().any(|t| t.edits() && !t.status.done());
            if !tool_focus_fresh && !any_edit_active && abs.is_file() {
                self.focus = Some(Focus { rel, line: None, from_tool: None, at: now });
            }
        }
    }

    /// Files whose diff stat should be recomputed now (150 ms after their last write).
    pub fn take_due(&mut self, now: Instant) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .files
            .iter_mut()
            .filter(|(_, f)| f.due.is_some_and(|d| d <= now))
            .map(|(r, f)| {
                f.due = None;
                r.clone()
            })
            .collect();
        out.sort();
        out
    }

    /// `None` = same as base.
    pub fn set_stat(&mut self, rel: &Path, stat: Option<DiffStat>) {
        match self.files.get_mut(rel) {
            Some(f) => f.stat = stat,
            None if stat.is_some() => self.files.entry(rel.to_path_buf()).or_default().stat = stat,
            None => {}
        }
    }

    /// Every diff stat at once (a full scan, e.g. after the base moved): files missing
    /// from `stats` are now the same as base.
    pub fn replace_stats(&mut self, stats: Vec<(PathBuf, DiffStat)>) {
        for f in self.files.values_mut() {
            f.stat = None;
        }
        for (rel, s) in stats {
            self.set_stat(&rel, Some(s));
        }
    }

    pub fn cell(&self, rel: &Path, now: Instant) -> Cell {
        match self.files.get(rel) {
            Some(f) => file_cell(f, self.owned(f), now),
            None => Cell { state: HeatState::Idle, level: 0, stat: None, fade: 0.0 },
        }
    }

    /// Strongest state among files under a folder (the folder itself included; `""` is
    /// the root); `None` when all idle.
    pub fn folder_state(&self, rel_dir: &Path, now: Instant) -> Option<HeatState> {
        self.files
            .iter()
            .filter(|(r, _)| r.starts_with(rel_dir))
            .map(|(_, f)| file_cell(f, self.owned(f), now).state)
            .max()
            .filter(|s| *s != HeatState::Idle)
    }

    /// Files (relative) that differ from base, sorted. In a shared folder, only the ones
    /// the agent touched.
    pub fn changed(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|(_, f)| f.stat.is_some() && self.owned(f))
            .map(|(r, _)| r.clone())
            .collect();
        v.sort();
        v
    }

    /// Files (relative) deleted since base, sorted: the tree shows them as struck-through
    /// rows until they come back, or the task is merged or discarded. In a shared folder,
    /// only the ones the agent touched.
    pub fn deleted(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|(_, f)| f.stat.is_some_and(|s| s.deleted) && self.owned(f))
            .map(|(r, _)| r.clone())
            .collect();
        v.sort();
        v
    }

    /// Earliest instant at which some cell's appearance changes (fade end, editing
    /// timeout, due recompute). An overdue recompute returns `now`. Animate fades/pulses
    /// in between; don't poll.
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let mut best: Option<Instant> = None;
        let mut consider = |t: Instant| best = Some(best.map_or(t, |b| b.min(t)));
        for f in self.files.values() {
            if let Some(d) = f.due {
                consider(d.max(now));
            }
            for t in [f.read_done, f.turn_write].into_iter().flatten() {
                let end = t + FADE;
                if end > now {
                    consider(end);
                }
            }
        }
        best
    }

    /// The file the agent most recently edited (ACP edit with a location, else the most
    /// recent write during a turn) and the 1-based line if known.
    pub fn focus(&self) -> Option<(PathBuf, Option<u32>)> {
        self.focus.as_ref().map(|f| (self.worktree.join(&f.rel), f.line))
    }
}

/// `owned` = steady states (diff stats) show; false for files in a shared folder that
/// the agent never touched.
fn file_cell(f: &File, owned: bool, now: Instant) -> Cell {
    let remaining = |t: Option<Instant>| -> f32 {
        t.map_or(0.0, |t| {
            let left = (t + FADE).saturating_duration_since(now);
            left.as_secs_f32() / FADE.as_secs_f32()
        })
    };
    let (state, fade) = if f.approval {
        (HeatState::NeedsApproval, 1.0)
    } else if !f.editing.is_empty() {
        (HeatState::Editing, 1.0)
    } else if remaining(f.turn_write) > 0.0 {
        (HeatState::Editing, remaining(f.turn_write))
    } else if !f.reading.is_empty() {
        (HeatState::Reading, 1.0)
    } else if remaining(f.read_done) > 0.0 {
        (HeatState::Reading, remaining(f.read_done))
    } else {
        match f.stat.filter(|_| owned) {
            Some(s) if s.deleted => (HeatState::Deleted, 1.0),
            Some(s) if s.created => (HeatState::Created, 1.0),
            Some(_) => (HeatState::Changed, 1.0),
            None => (HeatState::Idle, 0.0),
        }
    };
    let stat = f.stat.filter(|_| owned);
    Cell { state, level: f.touches.min(MAX_LEVEL as u32) as u8, stat, fade }
}

#[cfg(test)]
mod tests;
