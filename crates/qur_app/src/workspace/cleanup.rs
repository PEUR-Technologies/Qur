//! Agent > Clean Up Worktrees…: what Qur's task worktrees, `qur/*` branches
//! and turn checkpoints take on disk, per project, and removing the ones no
//! task owns. Orphans (no task, merged, clean) come preselected; anything
//! with unmerged commits or uncommitted changes asks again, by name, before
//! it goes. All git and disk work runs off the main thread.

use qur_worktree::cleanup::{self as wc, Item, Kind, OwnedTask};

use super::*;

pub(super) struct Cleanup {
    /// None while looking.
    groups: Option<Vec<Group>>,
    picked: HashSet<(usize, usize)>,
    busy: bool,
    /// The last removal's result.
    note: Option<String>,
}

struct Group {
    /// The main checkout (None: folders no repo knows).
    repo: Option<PathBuf>,
    items: Vec<Item>,
}

impl Cleanup {
    pub(super) fn debug(&self) -> String {
        match &self.groups {
            None => "cleanup:looking".into(),
            Some(g) => format!("cleanup:items={}:picked={}:busy={}", g.iter().map(|g| g.items.len()).sum::<usize>(), self.picked.len(), self.busy),
        }
    }

    fn set(&mut self, groups: Vec<Group>) {
        self.picked = groups.iter().enumerate().flat_map(|(g, gr)| gr.items.iter().enumerate().filter(|(_, it)| it.orphan()).map(move |(i, _)| (g, i))).collect();
        if crate::script::on() {
            for (g, gr) in groups.iter().enumerate() {
                for (i, it) in gr.items.iter().enumerate() {
                    println!("CLEANUP {} | {} | {} | {}", it.label(), status(it), crate::files::human_size(it.bytes), if self.picked.contains(&(g, i)) { "picked" } else { "-" });
                }
            }
        }
        self.groups = Some(groups);
        self.busy = false;
    }

    fn picked_items(&self) -> Vec<Item> {
        let Some(groups) = &self.groups else { return Vec::new() };
        let mut keys: Vec<_> = self.picked.iter().copied().collect();
        keys.sort_unstable();
        keys.into_iter().filter_map(|(g, i)| groups.get(g)?.items.get(i).cloned()).collect()
    }
}

/// Where Qur makes worktrees (`<qur home>/worktrees`).
fn qur_dir() -> PathBuf {
    qur_config::qur_home().join("worktrees")
}

fn owned_of(t: &TaskRecord) -> OwnedTask {
    OwnedTask { id: t.id.clone(), title: t.title.clone(), branch: t.branch.clone(), worktree: t.worktree.clone() }
}

/// Tasks of `repo`'s project that still exist: its saved task list, plus
/// this window's live tasks when it's this window's project.
fn owned_tasks(repo: &Path, current: &Path, live: &[OwnedTask]) -> Vec<OwnedTask> {
    let mut v: Vec<OwnedTask> = TaskStore::load(&qur_config::project_id(repo)).tasks.iter().filter(|t| !t.in_root).map(owned_of).collect();
    if wc::canon(repo) == wc::canon(current) {
        v.extend(live.iter().cloned());
    }
    v
}

fn toplevel(p: &Path) -> Option<PathBuf> {
    qur_worktree::git(p, &["rev-parse", "--show-toplevel"]).ok().map(|t| wc::canon(Path::new(&t)))
}

/// Look at every project Qur has worktrees, branches or turn refs in:
/// this window's, the repos behind the worktree folders, and recent
/// projects. Blocking (git and a disk walk).
fn survey_all(current: &Path, live: &[OwnedTask]) -> Vec<Group> {
    let q = qur_dir();
    let current = toplevel(current).unwrap_or_else(|| current.to_path_buf());
    let mut repos: Vec<PathBuf> = Vec::new();
    let candidates = std::iter::once(current.clone()).chain(wc::repos_in(&q)).chain(qur_config::State::load().recent_projects);
    for r in candidates {
        if let Some(top) = toplevel(&r)
            && !repos.contains(&top)
        {
            repos.push(top);
        }
    }
    let mut groups = Vec::new();
    let mut known = Vec::new();
    for repo in repos {
        let owned = owned_tasks(&repo, &current, live);
        let mut items = match wc::survey(&repo, &q, &owned) {
            Ok(items) => items,
            Err(e) => {
                crate::log(&format!("cleanup: {}: {e}", repo.display()));
                continue;
            }
        };
        known.extend(items.iter().filter_map(|i| i.path.clone()));
        wc::measure(&mut items);
        if !items.is_empty() {
            groups.push(Group { repo: Some(repo), items });
        }
    }
    let mut strays = wc::strays(&q, &known);
    wc::measure(&mut strays);
    if !strays.is_empty() {
        groups.push(Group { repo: None, items: strays });
    }
    groups
}

/// Remove items (`true`: the user agreed to lose its unsaved work). Returns
/// (removed, bytes freed, errors). Blocking.
fn remove_all(items: Vec<(Item, bool)>, current: &Path, live: &[OwnedTask]) -> (usize, u64, Vec<String>) {
    let q = qur_dir();
    let (mut n, mut freed, mut errors) = (0, 0, Vec::new());
    for (it, allow_loss) in items {
        // Ownership looked up again, right before removing.
        let owned = it.repo.as_deref().map(|r| owned_tasks(r, current, live)).unwrap_or_default();
        match wc::remove_item(&it, &q, &owned, allow_loss) {
            Ok(()) => {
                n += 1;
                freed += it.bytes;
                crate::log(&format!("cleanup: removed {}", it.label()));
            }
            Err(e) => errors.push(e),
        }
    }
    wc::prune_empty(&q);
    (n, freed, errors)
}

/// The row's state, for the user.
fn status(it: &Item) -> String {
    if let Some(t) = &it.owner {
        return format!("task “{t}”");
    }
    if it.locked {
        return "locked".into();
    }
    let mut s = it.at_risk().join(" · ");
    if s.is_empty() {
        s = match it.kind {
            Kind::TurnRefs => "no task".into(),
            Kind::Missing => "no task · merged".into(),
            _ => "no task · merged · clean".into(),
        };
    }
    if it.recent {
        s.push_str(" · new");
    }
    s
}

fn short(p: &Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let s = p.to_string_lossy();
    if !home.is_empty() && s.starts_with(&home) { s.replacen(&home, "~", 1) } else { s.into_owned() }
}

impl Workspace {
    fn live_tasks(&self, cx: &App) -> Vec<OwnedTask> {
        let agents = self.agents.iter().map(|a| owned_of(&a.read(cx).record));
        let terms = self.term_tasks.iter().map(|t| owned_of(&t.record));
        agents.chain(terms).collect()
    }

    /// Agent > Clean Up Worktrees….
    pub(super) fn clean_up_worktrees(&mut self, _: &qur::CleanUpWorktrees, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        window.focus(&self.focus, cx);
        self.overlay = Some(Overlay::Cleanup(Cleanup { groups: None, picked: HashSet::new(), busy: true, note: None }));
        cx.notify();
        self.cleanup_look(None, window, cx);
    }

    /// Look (again) in the background; `note` is shown with the result.
    fn cleanup_look(&mut self, note: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (root, live) = (self.root.clone(), self.live_tasks(cx));
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let groups = bg.spawn(async move { survey_all(&root, &live) }).await;
            this.update(cx, |this, cx| {
                if let Some(Overlay::Cleanup(c)) = &mut this.overlay {
                    c.set(groups);
                    c.note = note;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn cleanup_toggle(&mut self, key: (usize, usize), cx: &mut Context<Self>) {
        if let Some(Overlay::Cleanup(c)) = &mut self.overlay
            && !c.busy
            && c.groups.as_ref().and_then(|g| g.get(key.0)?.items.get(key.1)).is_some_and(Item::removable)
        {
            if !c.picked.remove(&key) {
                c.picked.insert(key);
            }
            cx.notify();
        }
    }

    /// Remove the picked rows. Those with unmerged commits or uncommitted
    /// changes need a second, separate yes that names them.
    fn cleanup_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Cleanup(c)) = &self.overlay else { return };
        if c.busy {
            return;
        }
        let picked = c.picked_items();
        if picked.is_empty() {
            return;
        }
        let risky: Vec<&Item> = picked.iter().filter(|i| !i.at_risk().is_empty()).collect();
        if risky.is_empty() {
            return self.cleanup_run(picked.into_iter().map(|i| (i, false)).collect(), window, cx);
        }
        let names: Vec<String> = risky
            .iter()
            .map(|i| format!("• {}{}: {}", i.label(), i.repo.as_deref().map(|r| format!(" in {}", short(r))).unwrap_or_default(), i.at_risk().join(", ")))
            .collect();
        let title = if risky.len() == 1 { "Remove 1 item with unsaved work?".to_string() } else { format!("Remove {} items with unsaved work?", risky.len()) };
        let detail = format!("Their work isn't merged or committed anywhere else, so removing them deletes it for good:\n\n{}", names.join("\n"));
        let answer = ask(window, PromptLevel::Warning, &title, Some(&detail), &["Remove Them Too", "Keep Them", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await.ok();
            this.update_in(cx, |this, window, cx| {
                let items: Vec<(Item, bool)> = match choice {
                    // Only the ones just named may lose work.
                    Some(0) => picked
                        .into_iter()
                        .map(|i| {
                            let named = !i.at_risk().is_empty();
                            (i, named)
                        })
                        .collect(),
                    Some(1) => picked.into_iter().filter(|i| i.at_risk().is_empty()).map(|i| (i, false)).collect(),
                    _ => return,
                };
                if !items.is_empty() {
                    this.cleanup_run(items, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn cleanup_run(&mut self, items: Vec<(Item, bool)>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Cleanup(c)) = &mut self.overlay {
            c.busy = true;
        }
        cx.notify();
        let (root, live) = (self.root.clone(), self.live_tasks(cx));
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let (n, freed, errors) = bg.spawn(async move { remove_all(items, &root, &live) }).await;
            this.update_in(cx, |this, window, cx| {
                let mut msg = format!("Removed {n}, freed {}", crate::files::human_size(freed));
                if let Some(e) = errors.first() {
                    msg.push_str(&format!("; {} not removed: {e}", errors.len()));
                }
                this.flash(msg.clone(), cx);
                if matches!(this.overlay, Some(Overlay::Cleanup(_))) {
                    this.cleanup_look(Some(msg), window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn cleanup_box(&self, c: &Cleanup, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let accent = look.theme.accents[0].base;
        let muted = |s: String| div().text_size(look.text(-2.)).text_color(look.c(ui.text_muted)).child(s);
        let mut list = div().id("cleanup-list").max_h(look.z(380.)).overflow_y_scroll().flex().flex_col();
        let (mut total, mut picked_bytes) = (0u64, 0u64);
        match &c.groups {
            None => list = list.child(muted("Looking at Qur's worktrees, branches and turn checkpoints…".into())),
            Some(groups) if groups.is_empty() => list = list.child(muted("Nothing to clean up: Qur has no worktrees, task branches or turn checkpoints left.".into())),
            Some(groups) => {
                for (g, group) in groups.iter().enumerate() {
                    let head = group.repo.as_deref().map(short).unwrap_or_else(|| "Folders no repo knows".into());
                    list = list.child(
                        div()
                            .pt(look.z(if g == 0 { 0. } else { 10. }))
                            .pb(look.z(4.))
                            .text_size(look.text(-2.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(look.c(ui.text_secondary))
                            .child(head),
                    );
                    for (i, it) in group.items.iter().enumerate() {
                        let key = (g, i);
                        let on = c.picked.contains(&key);
                        total += it.bytes;
                        if on {
                            picked_bytes += it.bytes;
                        }
                        let can = it.removable() && !c.busy;
                        let warn = !it.at_risk().is_empty();
                        let label = it.label();
                        let mark = label.clone();
                        list = list.child(
                            div()
                                .id(("cleanup-row", g * 10_000 + i))
                                .relative()
                                .flex()
                                .items_center()
                                .gap(look.z(8.))
                                .px(look.z(6.))
                                .py(look.z(3.))
                                .rounded(look.z(4.))
                                .when(can, |d| d.cursor_pointer().hover(|d| d.bg(look.c(ui.hover_bg))))
                                .child(
                                    div()
                                        .flex_none()
                                        .size(look.z(12.))
                                        .rounded(look.z(3.))
                                        .border_1()
                                        .border_color(look.c(if can { ui.border_strong } else { ui.border }))
                                        .when(on, |d| d.bg(look.c(accent)).border_color(look.c(accent))),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .child(div().truncate().text_size(look.text(-1.)).text_color(look.c(if can { ui.text } else { ui.text_muted })).child(label))
                                        .child(div().truncate().text_size(look.text(-3.)).text_color(look.c(if warn { ui.warning } else { ui.text_muted })).child(status(it))),
                                )
                                .child(div().flex_none().text_size(look.text(-2.)).text_color(look.c(ui.text_secondary)).child(if it.path.as_ref().is_some_and(|p| p.exists()) { crate::files::human_size(it.bytes) } else { "–".into() }))
                                .children(crate::script::mark(move || format!("cleanup:row:{mark}")))
                                .on_click(cx.listener(move |this, _, _, cx| this.cleanup_toggle(key, cx))),
                        );
                    }
                }
            }
        }
        let n = c.picked.len();
        let can_remove = n > 0 && !c.busy;
        let summary = match &c.groups {
            None => "Measuring…".to_string(),
            Some(g) => {
                let count: usize = g.iter().map(|g| g.items.len()).sum();
                format!("{} on disk · {count} item{}", crate::files::human_size(total), if count == 1 { "" } else { "s" })
            }
        };
        let button = |id: &'static str, text: String, primary: bool, enabled: bool| {
            div()
                .id(id)
                .relative()
                .px(look.z(12.))
                .py(look.z(4.))
                .rounded(look.z(5.))
                .text_size(look.text(-1.))
                .bg(look.c(if primary && enabled { ui.button_primary_bg } else { ui.button_bg }))
                .text_color(look.c(if primary && enabled { ui.button_primary_text } else if enabled { ui.button_text } else { ui.text_muted }))
                .when(enabled, |d| d.cursor_pointer())
                .child(text)
        };
        div()
            .id("cleanup")
            .occlude()
            .w(look.z(620.))
            .p(look.z(14.))
            .flex()
            .flex_col()
            .gap(look.z(10.))
            .bg(look.c(ui.overlay_bg))
            .border_1()
            .border_color(look.c(ui.border_strong))
            .rounded(look.z(8.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(look.z(8.))
                    .child(div().font_weight(gpui::FontWeight::MEDIUM).child("Clean Up Worktrees"))
                    .child(div().flex_1())
                    .child(muted(summary)),
            )
            .child(list)
            .children(c.note.clone().map(muted))
            .child(muted("Selected first: no task, nothing unmerged or uncommitted. Anything with unmerged commits or uncommitted changes asks again, by name, before it goes. A task's own worktree is removed with Discard Task….".into()))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(look.z(8.))
                    .child(button("cleanup-close", "Close".into(), false, true).on_click(cx.listener(|this, _, window, cx| this.close_overlays(window, cx))))
                    .child(
                        button("cleanup-remove", if n == 0 { "Remove".into() } else { format!("Remove {n} ({})", crate::files::human_size(picked_bytes)) }, true, can_remove)
                            .children(crate::script::mark(|| "cleanup:remove".to_string()))
                            .when(can_remove, |d| d.on_click(cx.listener(|this, _, window, cx| this.cleanup_remove(window, cx)))),
                    ),
            )
            .into_any_element()
    }
}
