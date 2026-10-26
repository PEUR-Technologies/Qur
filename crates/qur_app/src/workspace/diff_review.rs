//! Inline diff review in the code modal: which base a file's change
//! markers compare with (its task's base or last turn, or HEAD for your own
//! checkout), and the review commands (Next / Previous Change, Show, Revert,
//! Accept).

use super::*;
use crate::editor_view::DiffBase;

impl Workspace {
    /// Where `path`'s markers come from: (checkout, revision, label, in a
    /// task). A task's file compares with the task's base commit (or, with
    /// "Last Turn Only", the tree its last turn started from); a file in the
    /// main checkout with HEAD; anything else has no markers.
    fn diff_source(&self, path: &Path, cx: &App) -> Option<(PathBuf, String, String, bool)> {
        let owner = self
            .agents
            .iter()
            .map(|a| &a.read(cx).record)
            .chain(self.term_tasks.iter().map(|t| &t.record))
            .find(|r| path.starts_with(&r.worktree));
        match owner {
            Some(r) => Some(match r.last_turn().filter(|_| self.diff_last_turn) {
                Some(t) => (r.worktree.clone(), t.start.clone(), format!("turn {}'s start", t.n), true),
                None => (r.worktree.clone(), r.base.clone(), "the task's base".to_string(), true),
            }),
            None if path.starts_with(&self.root) => Some((self.root.clone(), "HEAD".to_string(), "HEAD".to_string(), false)),
            None => None,
        }
    }

    /// Read the file's base text off the UI thread and hand it to the
    /// buffer, which diffs lazily from then on. A file new in a task shows
    /// as all added; one that's untracked in your checkout shows nothing.
    pub(super) fn load_diff_base(&mut self, buf: &Entity<EditorView>, cx: &mut Context<Self>) {
        let Some(path) = buf.read(cx).path().map(Path::to_path_buf) else { return };
        let Some((dir, rev, label, in_task)) = self.diff_source(&path, cx) else {
            return buf.update(cx, |b, cx| b.set_diff_base(None, cx));
        };
        let key = format!("{}\0{rev}\0{}", dir.display(), path.display());
        let bg = cx.background_executor().clone();
        let buf = buf.downgrade();
        cx.spawn(async move |_, cx| {
            let base = bg
                .spawn(async move {
                    match qur_git::file_at(&dir, &rev, &path) {
                        Ok(Some(text)) => Some(DiffBase::new(label, key, &text)),
                        Ok(None) if in_task => Some(DiffBase::new(label, key, "")),
                        // Untracked in your checkout, binary, or not a repo.
                        Ok(None) | Err(_) => None,
                    }
                })
                .await;
            buf.update(cx, |b, cx| b.set_diff_base(base, cx)).ok();
        })
        .detach();
    }

    fn with_review(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut EditorView, &mut Context<EditorView>)) {
        match self.code.clone() {
            Some(c) => c.update(cx, f),
            None => self.flash("Open a file to review its changes", cx),
        }
    }

    pub(super) fn next_change(&mut self, _: &qur::NextChange, _: &mut Window, cx: &mut Context<Self>) {
        self.with_review(cx, |e, cx| e.step_change(true, cx));
    }

    pub(super) fn prev_change(&mut self, _: &qur::PrevChange, _: &mut Window, cx: &mut Context<Self>) {
        self.with_review(cx, |e, cx| e.step_change(false, cx));
    }

    pub(super) fn show_change(&mut self, _: &qur::ShowChange, _: &mut Window, cx: &mut Context<Self>) {
        self.with_review(cx, |e, cx| e.toggle_peek(cx));
    }

    pub(super) fn revert_change(&mut self, _: &qur::RevertChange, _: &mut Window, cx: &mut Context<Self>) {
        self.with_review(cx, |e, cx| e.revert_change(cx));
    }

    pub(super) fn accept_change(&mut self, _: &qur::AcceptChange, _: &mut Window, cx: &mut Context<Self>) {
        self.with_review(cx, |e, cx| e.accept_change(cx));
    }

    /// Changes > Last Turn Only: a task's markers show only what its last
    /// turn changed (its start snapshot as the base), or everything since
    /// the task started.
    pub(super) fn changes_last_turn(&mut self, _: &qur::ChangesLastTurn, _: &mut Window, cx: &mut Context<Self>) {
        self.diff_last_turn = !self.diff_last_turn;
        if let Some(c) = self.code.clone() {
            self.load_diff_base(&c, cx);
        }
        self.flash(
            if self.diff_last_turn { "Change markers: the last turn's changes only" } else { "Change markers: everything since the task started" },
            cx,
        );
        self.sync_menus(cx);
    }
}
