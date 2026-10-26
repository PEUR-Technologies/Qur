//! Turn checkpoints for Agent Terminals. A terminal agent can't say when a
//! turn starts or ends, so "busy, then quiet" (decision 30) stands in: the
//! worktree's tree is taken when the terminal goes busy and again when it
//! goes quiet having changed files, and kept like an ACP turn (the task's
//! `turns`, refs under `refs/qur/turns/<task>/<n>/`). Stretches that changed
//! nothing leave no turn and, when the watcher saw no write, cost no snapshot.
//! Setup and startup don't count (tasks.rs: `setup_marker`, `pending_type`).

use super::*;

impl Workspace {
    /// The terminal went busy: note the worktree's tree as the turn's start.
    /// Free when nothing changed since the last snapshot; else taken off the
    /// UI thread (a turn that goes quiet before it's in is dropped).
    pub(super) fn term_turn_began(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(t) = self.term_tasks.iter_mut().find(|t| t.record.id == id) else { return };
        t.turn_gen += 1;
        t.turn_start = t.known.clone();
        if t.turn_start.is_some() {
            return;
        }
        let (wt, run, edits) = (t.record.worktree.clone(), t.turn_gen, t.edits);
        let id = id.to_string();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let r = bg.spawn(async move { qur_git::snapshot_tree(&wt) }).await;
            this.update(cx, |this, _| {
                let Some(t) = this.term_tasks.iter_mut().find(|t| t.record.id == id) else { return };
                match r {
                    Ok(tree) => {
                        if t.edits == edits {
                            t.known = Some(tree.clone());
                        }
                        if t.turn_gen == run {
                            t.turn_start = Some(tree);
                        }
                    }
                    Err(e) => eprintln!("TURN {id} no checkpoint: {e}"),
                }
            })
            .ok();
        })
        .detach();
    }

    /// The terminal went quiet: if files were written meanwhile, take the end
    /// tree and keep the turn when it differs from the start.
    pub(super) fn term_turn_ended(&mut self, id: &str, wrote: bool, cx: &mut Context<Self>) {
        let Some(t) = self.term_tasks.iter_mut().find(|t| t.record.id == id) else { return };
        t.turn_gen += 1;
        let Some(start) = t.turn_start.take() else { return };
        if !wrote {
            return;
        }
        let (wt, edits) = (t.record.worktree.clone(), t.edits);
        let id = id.to_string();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let from = start.clone();
            let r = bg
                .spawn(async move {
                    let end = qur_git::snapshot_tree(&wt)?;
                    let stats = qur_git::tree_stats(&wt, &from, &end)?;
                    Ok::<_, String>((end, stats))
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(t) = this.term_tasks.iter_mut().find(|t| t.record.id == id) else { return };
                let (end, (files, added, removed)) = match r {
                    Ok(x) => x,
                    Err(e) => return eprintln!("TURN {id} no end checkpoint: {e}"),
                };
                if t.edits == edits {
                    t.known = Some(end.clone());
                }
                if files == 0 {
                    return println!("TURN {id} end no changes");
                }
                let n = t.record.next_turn_number();
                println!("TURN {id} {n} start {} end {} files={files} +{added} -{removed}", short(&start), short(&end));
                t.record.turns.push(qur_agents::Turn { n, start: start.clone(), end: Some(end.clone()), files, added, removed, rewound: false });
                let wt = t.record.worktree.clone();
                let turns = t.record.turns.clone();
                t.changes.update(cx, |c, cx| {
                    c.set_turns(turns, cx);
                    // Like an ACP turn's end: the file list is stale until it's recomputed.
                    c.refresh(cx);
                });
                let pin_id = id.clone();
                cx.background_executor()
                    .spawn(async move {
                        for (side, tree) in [("start", &start), ("end", &end)] {
                            if let Err(e) = qur_git::pin_turn(&wt, &pin_id, n, side, tree) {
                                eprintln!("TURN {pin_id} {n} ref: {e}");
                            }
                        }
                    })
                    .detach();
                this.save_tasks(cx);
                this.sync_menus(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Show what an Agent Terminal's turn N changed in its Changes tab.
    pub(super) fn show_term_turn(&mut self, id: &str, n: u32, cx: &mut Context<Self>) {
        let Some(t) = self.term_task(id) else { return };
        let Some(turn) = t.record.turn(n).cloned() else { return };
        Docks::update(cx, dock::Kind::Terminal, |d| d.activate(dock::Panel::Changes));
        t.changes.update(cx, |c, cx| c.show_turn(n, turn.start, turn.end, cx));
        cx.notify();
    }

    /// Is the Agent Terminal working (output in the last 2 s, or a busy
    /// stretch not yet over)?
    fn term_session_busy(&self, id: &str, cx: &App) -> bool {
        let Some(t) = self.term_task(id) else { return false };
        t.was_busy || t.term_id.and_then(|tid| self.terms.read(cx).session_by_id(tid)).is_some_and(|s| s.is_busy())
    }

    /// Rewind for an Agent Terminal: like an ACP agent's (ask first, warn
    /// when files changed since the turn ended, never the project folder),
    /// but Qur can't stop a terminal agent without ending it, so it waits
    /// for you while the agent is busy. Afterwards a one-line note goes into
    /// the agent's input line, not sent (see `term_rewind_done`).
    pub(super) fn rewind_term_turn(&mut self, id: &str, n: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.term_task(id) else { return };
        if t.record.in_root {
            return self.flash("Rewind works in an agent's own worktree; this Agent Terminal works in your project folder (Show Last Turn's Changes still shows what it did)", cx);
        }
        if t.rewinding {
            return;
        }
        let r = &t.record;
        let Some(turn) = r.turn(n).cloned() else { return };
        if turn.rewound {
            return self.flash(format!("Turn {n} was already rewound"), cx);
        }
        if self.term_session_busy(id, cx) {
            return self.flash("The agent is busy: rewind once it's quiet (interrupt it in its terminal first if you need to)", cx);
        }
        let later = r.turns.iter().filter(|x| x.n > n && !x.rewound).count();
        let (wt, last_end) = (r.worktree.clone(), r.last_turn().and_then(|x| x.end.clone()));
        let id = id.to_string();
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let by_hand = match last_end {
                Some(end) => bg.spawn(async move { qur_git::snapshot_tree(&wt).map(|now| now != end).unwrap_or(false) }).await,
                None => false,
            };
            this.update_in(cx, |_, window, cx| {
                let detail = rewind_detail(n, &turn, later, by_hand);
                let title = if later == 0 { "Rewind the last turn?".to_string() } else { format!("Rewind to before turn {n}?") };
                let answer = ask(window, PromptLevel::Warning, &title, Some(&detail), &["Rewind", "Cancel"], cx);
                cx.spawn_in(window, async move |this, cx| {
                    if answer.await.ok() == Some(0) {
                        this.update(cx, |this, cx| this.do_term_rewind(&id, n, cx)).ok();
                    }
                })
                .detach();
            })
            .ok();
        })
        .detach();
    }

    fn do_term_rewind(&mut self, id: &str, n: u32, cx: &mut Context<Self>) {
        // It may have started working while the dialog was up.
        if self.term_session_busy(id, cx) {
            return self.flash("The agent started working: rewind once it's quiet", cx);
        }
        let root = self.root.clone();
        let Some(t) = self.term_tasks.iter_mut().find(|t| t.record.id == id) else { return };
        let Some(start) = t.record.turn(n).map(|x| x.start.clone()) else { return };
        t.rewinding = true;
        let wt = t.record.worktree.clone();
        let id = id.to_string();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let r = bg
                .spawn(async move {
                    let now = qur_git::snapshot_tree(&wt)?;
                    let undone = qur_git::changed_paths_between(&wt, &start, &now)?;
                    qur_git::restore_tree(&root, &wt, &start)?;
                    Ok::<_, String>(undone)
                })
                .await;
            this.update(cx, |this, cx| this.term_rewind_done(&id, n, r, cx)).ok();
        })
        .detach();
    }

    /// After a Rewind: mark the turns, refresh Changes, and put a note for
    /// the agent in its input line without Enter. A terminal agent can't be
    /// told any other way; you read it and send it with your next message (or
    /// delete it). Only when the terminal is at an input line that takes
    /// pastes: typed into a menu, a letter could pick an answer.
    fn term_rewind_done(&mut self, id: &str, n: u32, r: Result<Vec<PathBuf>, String>, cx: &mut Context<Self>) {
        let Some(t) = self.term_tasks.iter_mut().find(|t| t.record.id == id) else { return };
        t.rewinding = false;
        let paths = match r {
            Ok(p) => p,
            Err(e) => {
                eprintln!("REWIND {id} turn {n} failed: {e}");
                return self.flash(format!("Couldn't rewind: {e}"), cx);
            }
        };
        let undone: Vec<u32> = t.record.turns.iter().filter(|x| x.n >= n && !x.rewound).map(|x| x.n).collect();
        for x in t.record.turns.iter_mut().filter(|x| x.n >= n) {
            x.rewound = true;
        }
        // Our own restore just wrote files: the next turn takes a fresh snapshot.
        t.known = None;
        println!("REWIND {id} turn {n}: {} files restored", paths.len());
        let label = turns_label(n, &undone);
        let turns = t.record.turns.clone();
        let tid = t.term_id;
        t.changes.update(cx, |c, cx| {
            c.set_turns(turns, cx);
            c.refresh(cx);
        });
        let k = paths.len();
        let mut msg = format!("Rewound {label}: {k} file{} put back", if k == 1 { "" } else { "s" });
        let typed = tid.filter(|tid| self.terms.read(cx).session_by_id(*tid).is_some_and(|s| s.takes_paste()));
        match typed {
            Some(tid) => {
                let note = rewind_note(&label, &paths);
                self.terms.update(cx, |p, cx| p.type_text(tid, &note, cx));
                msg.push_str("; a note for the agent is in its input line (add to it or delete it, then press Enter)");
            }
            None => msg.push_str("; tell the agent before it carries on (its terminal isn't at an input line)"),
        }
        self.save_tasks(cx);
        self.sync_menus(cx);
        self.flash(msg, cx);
    }
}

/// "turn 3", or "turns 3–5" when later turns go too.
fn turns_label(n: u32, undone: &[u32]) -> String {
    match undone {
        [] | [_] => format!("turn {n}"),
        more => format!("turns {n}–{}", more[more.len() - 1]),
    }
}

/// The dialog's text: what goes back, what else is lost, what happens next.
fn rewind_detail(n: u32, turn: &qur_agents::Turn, later: usize, by_hand: bool) -> String {
    let mut detail = format!(
        "The files in this agent's worktree go back to how they were before turn {n} ({} file{}, +{} −{}).",
        turn.files,
        if turn.files == 1 { "" } else { "s" },
        turn.added,
        turn.removed
    );
    if later > 0 {
        detail.push_str(&format!(" This also undoes the {later} turn{} after it.", if later == 1 { "" } else { "s" }));
    }
    if by_hand {
        detail.push_str("\n\nFiles in the worktree changed after the turn ended (by you or another program). Rewinding loses those changes too.");
    }
    detail.push_str("\n\nCommits stay. A note for the agent is typed into its input line, not sent.");
    detail
}

/// One line for the agent's input (no newline: some agents send on one).
fn rewind_note(label: &str, paths: &[PathBuf]) -> String {
    let mut list = paths.iter().take(10).map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
    if paths.len() > 10 {
        list.push_str(&format!(" and {} more", paths.len() - 10));
    }
    if list.is_empty() {
        list = "no files differed".into();
    }
    format!("[Note from Qur, the editor: the user rewound your work to before {label}; your edits since are gone (changed back: {list}). Re-read files before you change them.] ")
}

fn short(tree: &str) -> &str {
    &tree[..tree.len().min(10)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewind_texts() {
        assert_eq!(turns_label(3, &[3]), "turn 3");
        assert_eq!(turns_label(3, &[3, 4, 6]), "turns 3–6");
        let note = rewind_note("turn 2", &[PathBuf::from("src/a.rs"), PathBuf::from("b.txt")]);
        assert!(note.contains("before turn 2") && note.contains("src/a.rs, b.txt"), "{note}");
        assert!(!note.contains('\n') && note.ends_with(' '), "one line, ready to type after: {note:?}");
        let many: Vec<PathBuf> = (0..13).map(|i| PathBuf::from(format!("f{i}"))).collect();
        assert!(rewind_note("turn 1", &many).contains("f9 and 3 more"));
        assert!(rewind_note("turn 1", &[]).contains("no files differed"));
        let turn = qur_agents::Turn { n: 2, start: "a".into(), end: Some("b".into()), files: 1, added: 3, removed: 0, rewound: false };
        let d = rewind_detail(2, &turn, 1, true);
        assert!(d.contains("before turn 2 (1 file, +3 −0)") && d.contains("the 1 turn after it") && d.contains("changed after the turn ended"), "{d}");
        assert!(!rewind_detail(2, &turn, 0, false).contains("after it"));
    }
}
