//! Undo/redo transactions with typing groups.

use std::time::{Duration, Instant};

use crate::Selection;

/// Consecutive typing within this window becomes one undo step.
const GROUP_WINDOW: Duration = Duration::from_secs(1);

/// One applied replacement, in char offsets at the time it was applied.
#[derive(Clone, Debug)]
pub(crate) struct Applied {
    pub start: usize,
    pub removed: String,
    pub inserted: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Type,
    Backspace,
    Other,
}

pub(crate) struct Txn {
    id: u64,
    /// In application order; undo walks them backwards.
    pub changes: Vec<Applied>,
    pub before: (Vec<Selection>, usize),
    pub after: (Vec<Selection>, usize),
    kind: Kind,
    time: Instant,
}

#[derive(Default)]
pub(crate) struct History {
    pub undo: Vec<Txn>,
    pub redo: Vec<Txn>,
    next_id: u64,
}

impl History {
    /// Identifies the current text state (0 = as opened).
    pub fn state(&self) -> u64 {
        self.undo.last().map_or(0, |t| t.id)
    }

    pub fn push(
        &mut self,
        changes: Vec<Applied>,
        before: (Vec<Selection>, usize),
        after: (Vec<Selection>, usize),
        kind: Kind,
        now: Instant,
        break_group: bool,
    ) {
        self.redo.clear();
        if !break_group
            && kind != Kind::Other
            && let Some(top) = self.undo.last_mut()
            && top.kind == kind
            && now.saturating_duration_since(top.time) < GROUP_WINDOW
            && same_carets(&top.after.0, &before.0)
        {
            top.changes.extend(changes);
            top.after = after;
            top.time = now;
            return;
        }
        self.next_id += 1;
        self.undo.push(Txn { id: self.next_id, changes, before, after, kind, time: now });
    }
}

fn same_carets(a: &[Selection], b: &[Selection]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.anchor == y.anchor && x.head == y.head)
}
