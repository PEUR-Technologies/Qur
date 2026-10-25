//! Turn checkpoints: recording turns, Rewind and Show Turn.

use super::*;

impl AgentTask {
    // -- turn checkpoints ------------------------------------------------------

    /// A prompt is about to go out: record the turn with the worktree's tree.
    pub(super) fn begin_turn(&mut self, start: Result<String, String>, cx: &mut Context<Self>) {
        let start = match start {
            Ok(t) => t,
            Err(e) => {
                if self.review.is_none() {
                    eprintln!("TURN {} no checkpoint: {e}", self.record.id);
                }
                self.turn_n = None;
                return;
            }
        };
        let n = self.record.next_turn_number();
        println!("TURN {} {n} start {}", self.record.id, short(&start));
        self.pin(n, Some(("start", start.clone())), cx);
        self.record.turns.push(qur_agents::Turn { n, start, ..Default::default() });
        self.turn_n = Some(n);
        cx.emit(AgentEvent::Changed);
        cx.emit(AgentEvent::TurnsChanged);
    }

    /// Pin a checkpoint under `refs/qur/turns/<task>/<n>/<side>`, or (None)
    /// drop turn N's refs.
    fn pin(&self, n: u32, side: Option<(&'static str, String)>, cx: &mut Context<Self>) {
        let (wt, id) = (self.record.worktree.clone(), self.record.id.clone());
        cx.background_executor()
            .spawn(async move {
                let r = match side {
                    Some((side, tree)) => qur_git::pin_turn(&wt, &id, n, side, &tree),
                    None => qur_git::unpin_turn(&wt, &id, n),
                };
                if let Err(e) = r {
                    eprintln!("TURN {id} {n} ref: {e}");
                }
            })
            .detach();
    }

    /// The turn ended (any way): take the end checkpoint, add the chat's
    /// turn line, then send what's queued. A turn that changed nothing
    /// leaves no checkpoint.
    pub(super) fn end_turn(&mut self, cx: &mut Context<Self>) {
        let Some((n, start)) = self.turn_n.take().and_then(|n| self.record.turn(n).map(|t| (n, t.start.clone()))) else {
            return self.after_turn(cx);
        };
        let wt = self.record.worktree.clone();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let r = bg
                .spawn(async move {
                    let end = qur_git::snapshot_tree(&wt)?;
                    let stats = qur_git::tree_stats(&wt, &start, &end)?;
                    Ok::<_, String>((end, stats))
                })
                .await;
            this.update(cx, |t, cx| {
                let id = t.record.id.clone();
                match r {
                    Ok((end, (files, added, removed))) if files > 0 => {
                        println!("TURN {id} {n} end {} files={files} +{added} -{removed}", short(&end));
                        t.pin(n, Some(("end", end.clone())), cx);
                        if let Some(turn) = t.record.turns.iter_mut().find(|x| x.n == n) {
                            turn.end = Some(end);
                            (turn.files, turn.added, turn.removed) = (files, added, removed);
                        }
                        t.push(ChatItem::Turn(n));
                    }
                    Ok(_) => {
                        println!("TURN {id} {n} end no changes");
                        t.record.turns.retain(|x| x.n != n);
                        t.pin(n, None, cx);
                    }
                    // The start checkpoint stays (Rewind still works).
                    Err(e) => eprintln!("TURN {id} {n} no end checkpoint: {e}"),
                }
                cx.emit(AgentEvent::Changed);
                cx.emit(AgentEvent::TurnsChanged);
                t.after_turn(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After a turn: a Rewind that was waiting for the stop, else the queue.
    fn after_turn(&mut self, cx: &mut Context<Self>) {
        if let Some(n) = self.rewind_after_stop.take() {
            return self.do_rewind(n, cx);
        }
        self.drain_pending(cx);
    }

    /// Is a turn running (Rewind stops it first)?
    pub fn turn_running(&self) -> bool {
        matches!(self.status, Status::Working | Status::NeedsYou) || self.turn_n.is_some()
    }

    /// Rewind to before turn N (asked already): stop the agent if it's
    /// working, then put the worktree's files back.
    pub fn rewind(&mut self, n: u32, cx: &mut Context<Self>) {
        if self.rewinding {
            return;
        }
        if self.turn_running() {
            self.rewind_after_stop = Some(n);
            self.notice(format!("Stopping {} to rewind…", self.def.label), cx);
            if matches!(self.status, Status::Working | Status::NeedsYou) {
                self.stop(cx);
            }
            return;
        }
        self.do_rewind(n, cx);
    }

    fn do_rewind(&mut self, n: u32, cx: &mut Context<Self>) {
        let Some(start) = self.record.turn(n).map(|t| t.start.clone()) else {
            self.notice(format!("Nothing to rewind: turn {n} changed no files."), cx);
            return self.drain_pending(cx);
        };
        self.rewinding = true;
        let (root, wt) = (self.project_root.clone(), self.record.worktree.clone());
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
            this.update(cx, |t, cx| {
                t.rewinding = false;
                match r {
                    Ok(paths) => {
                        let undone: Vec<u32> = t.record.turns.iter().filter(|x| x.n >= n && !x.rewound).map(|x| x.n).collect();
                        for x in t.record.turns.iter_mut().filter(|x| x.n >= n) {
                            x.rewound = true;
                        }
                        println!("REWIND {} turn {n}: {} files restored", t.record.id, paths.len());
                        let turns = match undone.as_slice() {
                            [] | [_] => format!("turn {n}"),
                            more => format!("turns {n}–{}", more[more.len() - 1]),
                        };
                        let k = paths.len();
                        t.push(ChatItem::Notice(format!(
                            "Rewound {turns}: {k} file{} put back to how they were before it. {} will be told with your next message.",
                            if k == 1 { "" } else { "s" },
                            t.def.label
                        )));
                        let mut list = paths.iter().take(20).map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
                        if k > 20 {
                            list.push_str(&format!(" and {} more", k - 20));
                        }
                        t.rewind_note = Some(format!(
                            "[Note from Qur, the editor: the user rewound your work. The files in your working directory are back to how they were before {turns} of this conversation, so the edits you made since then are gone ({}). Re-read files before you change them.]",
                            if list.is_empty() { "no files differed".to_string() } else { format!("changed back: {list}") }
                        ));
                        // The turn lines now say "rewound".
                        for ix in 0..t.chat.len() {
                            if matches!(t.chat[ix], ChatItem::Turn(m) if m >= n) {
                                t.list.splice(ix..ix + 1, 1);
                            }
                        }
                        t.changes.update(cx, |c, cx| c.refresh(cx));
                    }
                    Err(e) => {
                        eprintln!("REWIND {} turn {n} failed: {e}", t.record.id);
                        t.push(ChatItem::Notice(format!("Couldn't rewind: {e}")));
                    }
                }
                cx.emit(AgentEvent::Changed);
                cx.emit(AgentEvent::TurnsChanged);
                t.drain_pending(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Show what turn N changed in the Changes view (comments work as usual).
    pub fn show_turn(&mut self, n: u32, cx: &mut Context<Self>) {
        let Some(t) = self.record.turn(n).cloned() else { return };
        Docks::update(cx, Kind::Acp, |d| d.activate(Panel::Changes));
        self.changes.update(cx, |c, cx| c.show_turn(n, t.start, t.end, cx));
        cx.notify();
    }

    /// "3 files · +12 −4 · View · Rewind" at the end of a turn.
    pub(super) fn turn_line(&self, ix: usize, n: u32, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let Some(t) = self.record.turn(n) else { return div() };
        let link = |id: &'static str, label: &'static str| {
            div().id((id, ix)).cursor_pointer().text_color(look.c(ui.text_secondary)).hover(|d| d.text_color(look.c(ui.text))).child(label)
        };
        let dot = || div().child("·");
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(look.z(6.))
            .text_size(look.text(-1.))
            .text_color(look.c(ui.text_muted))
            .child(format!("{} file{}", t.files, if t.files == 1 { "" } else { "s" }))
            .child(dot())
            .child(
                div()
                    .flex()
                    .gap(look.z(4.))
                    .child(div().text_color(look.c(look.theme.diff.added_text)).child(format!("+{}", t.added)))
                    .child(div().text_color(look.c(look.theme.diff.removed_text)).child(format!("−{}", t.removed))),
            )
            .child(dot())
            .child(link("turn-view", "View").on_click(cx.listener(move |t, _, _, cx| t.show_turn(n, cx))))
            .child(dot())
            .when(t.rewound, |d| d.child("rewound"))
            .when(!t.rewound, |d| d.child(link("turn-rewind", "Rewind").on_click(cx.listener(move |_, _, _, cx| cx.emit(AgentEvent::Rewind(n))))))
    }
}
