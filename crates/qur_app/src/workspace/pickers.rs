//! Pickers: the palette, Quick Open, themes, projects, go to line, mentions.

use super::*;

impl Workspace {
    // -- overlays ----------------------------------------------------------------------

    /// A session setting's values (mode, model…), above the agent's prompt.
    fn pick_setting(&mut self, a: Entity<AgentTask>, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(o) = a.read(cx).settings.iter().find(|o| o.id == id).cloned() else { return };
        let values = o.values.iter().map(|v| v.value.clone()).collect();
        let items = o
            .values
            .iter()
            .map(|v| {
                let item = PickItem::new(v.name.clone()).detail(v.description.clone().unwrap_or_default());
                if v.value == o.current { item.right("current") } else { item }
            })
            .collect();
        let title = if o.is_mode() { format!("{}…  (⇧⇥ next)", o.name) } else { format!("{}…", o.name) };
        self.open_picker(&title, items, PickKind::Setting { agent: a, id: id.to_string(), values }, false, window, cx);
    }

    pub(super) fn open_picker(&mut self, placeholder: &str, items: Vec<PickItem>, kind: PickKind, paths: bool, window: &mut Window, cx: &mut Context<Self>) -> Entity<Picker> {
        self.close_overlays(window, cx);
        self.prev_focus = window.focused(cx);
        let p = cx.new(|cx| Picker::new(placeholder, items, paths, window, cx));
        let sub = cx.subscribe_in(&p, window, Self::on_picker_event);
        self._subs.push(sub);
        let f = p.read(cx).input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::Picker(p.clone(), kind));
        cx.notify();
        p
    }

    fn on_picker_event(&mut self, picker: &Entity<Picker>, ev: &PickerEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Picker(p, kind)) = &mut self.overlay else { return };
        if p != picker {
            return;
        }
        match (ev, kind) {
            (PickerEvent::Confirm(i), PickKind::Commands(cmds)) => {
                let action = (cmds[*i].action)();
                self.close_overlays(window, cx);
                window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
            }
            (PickerEvent::Confirm(i), PickKind::Files { root, files }) => {
                let path = root.join(&files[*i]);
                self.close_overlays(window, cx);
                self.open_path(&path, window, cx);
            }
            (PickerEvent::Confirm(i), PickKind::Mention { root, files, target }) => {
                let path = root.join(&files[*i]);
                let target = target.clone();
                self.overlay = None;
                self.prev_focus = None;
                target.update(cx, |t, cx| t.insert_mention(path, cx));
                let f = target.read(cx).focus.clone();
                window.focus(&f, cx);
                cx.notify();
            }
            (PickerEvent::Confirm(i), PickKind::Slash { agent, settings, target, .. }) if *i < settings.len() => {
                let (agent, id, target) = (agent.clone(), settings[*i].clone(), target.clone());
                target.update(cx, |t, cx| t.set_text("", cx));
                self.pick_setting(agent, &id, window, cx);
            }
            (PickerEvent::Confirm(i), PickKind::Slash { settings, names, target, .. }) => {
                let text = format!("/{} ", names[*i - settings.len()]);
                let target = target.clone();
                self.overlay = None;
                self.prev_focus = None;
                target.update(cx, |t, cx| t.set_text(&text, cx));
                let f = target.read(cx).focus.clone();
                window.focus(&f, cx);
                cx.notify();
            }
            (PickerEvent::Confirm(i), PickKind::Setting { agent, id, values }) => {
                let (agent, id, value) = (agent.clone(), id.clone(), values.get(*i).cloned());
                self.close_overlays(window, cx);
                if let Some(v) = value {
                    agent.update(cx, |a, cx| a.set_setting(&id, &v, cx));
                }
                let f = agent.read(cx).prompt.read(cx).focus.clone();
                window.focus(&f, cx);
            }
            (PickerEvent::Confirm(i), PickKind::Sessions(order)) => {
                let target = order.get(*i).cloned();
                self.close_overlays(window, cx);
                if target.is_some() {
                    self.select(target, window, cx);
                }
            }
            (PickerEvent::Confirm(i), PickKind::Projects(list)) => {
                let target = list.get(*i).cloned();
                self.close_overlays(window, cx);
                match target {
                    Some(p) => crate::open_project(p, None, cx),
                    None => crate::prompt_open_folder(cx),
                }
            }
            (PickerEvent::Highlight(i), PickKind::Themes { names, .. }) => {
                self.preview_theme = Some(names[*i].clone());
                self.apply_look(window, cx);
            }
            (PickerEvent::Confirm(i), PickKind::Themes { names, project_scope }) => {
                let name = names[*i].clone();
                let project_scope = *project_scope;
                self.overlay = None;
                self.save_theme(&name, project_scope, window, cx);
                self.restore_focus(window, cx);
            }
            (PickerEvent::Tab, PickKind::Themes { project_scope, .. }) if !self.home => {
                *project_scope = !*project_scope;
                let header = theme_scope_header(*project_scope, &self.project_name());
                picker.update(cx, |p, cx| {
                    p.header = Some(header);
                    cx.notify();
                });
            }
            (PickerEvent::Dismiss, PickKind::Themes { .. }) => {
                self.preview_theme = None;
                self.apply_look(window, cx);
                self.close_overlays(window, cx);
            }
            (PickerEvent::Highlight(i), PickKind::Symbols { targets, .. }) => {
                if let (Some(r), Some(code)) = (targets.get(*i).cloned(), self.code.clone()) {
                    code.update(cx, |c, cx| c.select_bytes(r, cx));
                }
            }
            (PickerEvent::Confirm(i), PickKind::Symbols { targets, .. }) => {
                let r = targets.get(*i).cloned();
                self.close_overlays(window, cx);
                if let (Some(r), Some(code)) = (r, self.code.clone()) {
                    code.update(cx, |c, cx| c.select_bytes(r, cx));
                    if std::env::var_os("QUR_DEBUG_ACTIONS").is_some() {
                        println!("SYMBOL {}", code.read(cx).selected_text().unwrap_or_default());
                    }
                }
            }
            (PickerEvent::Dismiss, PickKind::Symbols { restore, .. }) => {
                let restore = restore.clone();
                self.close_overlays(window, cx);
                if let Some(code) = self.code.clone() {
                    code.update(cx, |c, cx| c.restore_selections(restore, cx));
                }
            }
            (PickerEvent::Dismiss, _) => self.close_overlays(window, cx),
            _ => {}
        }
    }

    pub(super) fn files_in(&mut self, root: &Path) -> Vec<PathBuf> {
        self.file_lists.entry(root.to_path_buf()).or_insert_with(|| qur_tree::list_files(root, 200_000)).clone()
    }

    pub(super) fn file_items(files: &[PathBuf]) -> Vec<PickItem> {
        files
            .iter()
            .map(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                PickItem::new(name).detail(dir)
            })
            .collect()
    }

    pub(super) fn mention_picker(&mut self, root: PathBuf, target: Entity<PromptInput>, window: &mut Window, cx: &mut Context<Self>) {
        let files = self.files_in(&root);
        let items = Self::file_items(&files);
        self.open_picker("Mention a file…", items, PickKind::Mention { root, files, target }, true, window, cx);
    }

    pub(super) fn palette(&mut self, _: &qur::CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(&self.overlay, Some(Overlay::Picker(_, PickKind::Commands(_)))) {
            return self.close_overlays(window, cx);
        }
        let cmds: Vec<_> = commands::palette_commands().collect();
        let turns = self.selected_last_turn(cx).is_some();
        let pending = self.agents.iter().any(|a| a.read(cx).has_permission());
        let review = self.review_state(cx);
        let plan = self.plan_state(cx);
        let items = cmds
            .iter()
            .map(|c| {
                let right = match c.tier {
                    commands::Tier::Ready => commands::display_keys(&c.first_keys()),
                    commands::Tier::Later(m) => format!("coming in {m}"),
                };
                let available = (turns || !commands::needs_turn(c)) && (pending || !commands::needs_pending(c)) && commands::review_available(c, review) && commands::plan_available(c, plan) && commands::available_in(c, !self.home, self.repo);
                PickItem::new(format!("{}: {}", c.category, c.title)).right(right).enabled(c.ready() && available)
            })
            .collect();
        self.open_picker("Type a command…", items, PickKind::Commands(cmds), false, window, cx);
    }

    pub(super) fn quick_open(&mut self, _: &qur::QuickOpen, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.current_root(cx);
        let files = self.files_in(&root);
        // Recently opened files under this root first.
        let recent: Vec<PathBuf> = self
            .buffers
            .paths(cx)
            .into_iter()
            .filter_map(|p| p.strip_prefix(&root).ok().map(Path::to_path_buf))
            .collect();
        let mut ordered = recent.clone();
        ordered.extend(files.into_iter().filter(|f| !recent.contains(f)));
        let items = Self::file_items(&ordered);
        let placeholder = if root == self.root {
            "Go to file…".to_string()
        } else {
            format!("Go to file in {}…", root.file_name().unwrap_or_default().to_string_lossy())
        };
        self.open_picker(&placeholder, items, PickKind::Files { root, files: ordered }, true, window, cx);
    }

    pub(super) fn theme_picker(&mut self, _: &qur::ThemePicker, window: &mut Window, cx: &mut Context<Self>) {
        let eff = self.effective();
        let mut names = vec![qur_config::THEME_AUTO.to_string()];
        let mut items = vec![PickItem::new("Auto").detail("follows macOS: Qur Light / Qur Dark")];
        for t in Registry::global().all() {
            names.push(t.name.clone());
            items.push(PickItem::new(t.name.clone()).detail(t.group));
        }
        let current = names.iter().position(|n| *n == eff.theme).unwrap_or(0);
        // The empty window has no project: its theme is for all projects.
        let project_scope = !self.home;
        let header = theme_scope_header(project_scope, &self.project_name());
        let p = self.open_picker("Select a theme…", items, PickKind::Themes { names, project_scope }, false, window, cx);
        p.update(cx, |p, cx| {
            p.header = Some(header).filter(|_| project_scope);
            p.select_item(current, cx);
        });
    }

    pub(super) fn project_switcher(&mut self, _: &qur::ProjectSwitcher, window: &mut Window, cx: &mut Context<Self>) {
        let home = std::env::var("HOME").unwrap_or_default();
        let mut list = qur_config::State::load().recent_projects;
        list.retain(|p| p != &self.root);
        let mut items: Vec<PickItem> = list
            .iter()
            .map(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                PickItem::new(name).detail(p.to_string_lossy().replacen(&home, "~", 1))
            })
            .collect();
        items.push(PickItem::new("Open Folder…").right(commands::display_keys("cmd-o")));
        self.open_picker("Switch project…", items, PickKind::Projects(list), false, window, cx);
    }

    /// Go to Symbol in File… (Cmd+Shift+O): the code modal's tree-sitter
    /// outline in a picker, indented by nesting; arrows preview, Esc goes back.
    pub(super) fn go_to_symbol(&mut self, _: &qur::GoToSymbol, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.code.clone() else {
            return self.flash("Open a file first (Cmd+P), then Go to Symbol", cx);
        };
        let (symbols, has_language, cursor, restore) = {
            let a = active.read(cx);
            (a.symbols(), a.language.is_some(), a.model.point_to_byte(a.cursor()), a.selections())
        };
        if symbols.is_empty() {
            let why = if has_language { "No symbols in this file" } else { "No symbols: Qur doesn't parse this kind of file" };
            return self.flash(why, cx);
        }
        let current = symbols.iter().rposition(|s| s.range.start <= cursor && cursor <= s.range.end).unwrap_or(0);
        let items = symbols
            .iter()
            .map(|s| {
                let line = active.read(cx).model.byte_to_point(s.name_range.start).line + 1;
                PickItem::new(format!("{}{}", "  ".repeat(s.depth.min(8)), s.name)).detail(s.kind).right(line.to_string())
            })
            .collect();
        let targets = symbols.into_iter().map(|s| s.name_range).collect();
        let p = self.open_picker("Go to symbol…", items, PickKind::Symbols { targets, restore }, false, window, cx);
        p.update(cx, |p, cx| p.select_item(current, cx));
        // Esc puts the cursor back where it was (previews move it).
        self.prev_focus = Some(active.read(cx).focus.clone());
    }

    /// Expand/Shrink Selection when the code modal isn't the focused view
    /// (menu or scripted runs): act on the modal's editor.
    pub(super) fn expand_selection(&mut self, _: &qur::ExpandSelection, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(code) = self.code.clone() {
            code.update(cx, |c, cx| c.expand_selection(true, cx));
        }
    }
    pub(super) fn shrink_selection(&mut self, _: &qur::ShrinkSelection, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(code) = self.code.clone() {
            code.update(cx, |c, cx| c.expand_selection(false, cx));
        }
    }

    pub(super) fn goto_line(&mut self, _: &qur::GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.code.clone() else { return };
        self.close_overlays(window, cx);
        self.prev_focus = Some(active.read(cx).focus.clone());
        let n = active.read(cx).model.line_count();
        let input = cx.new(|cx| TextInput::new(format!("Go to line (1–{n})"), cx));
        let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Confirm { .. } => {
                let t = input.read(cx).text.trim().to_string();
                let mut it = t.split(':');
                let line = it.next().and_then(|l| l.parse::<usize>().ok());
                let col = it.next().and_then(|c| c.parse::<usize>().ok());
                this.close_overlays(window, cx);
                if let (Some(l), Some(a)) = (line, this.code.clone()) {
                    a.update(cx, |a, cx| {
                        a.goto_line(l, cx);
                        if let Some(c) = col {
                            a.set_cursor(Point { line: l.saturating_sub(1), col: c.saturating_sub(1) }, cx);
                        }
                    });
                }
            }
            InputEvent::Cancel => this.close_overlays(window, cx),
            _ => {}
        });
        self._subs.push(sub);
        let f = input.read(cx).focus.clone();
        window.focus(&f, cx);
        self.overlay = Some(Overlay::GoTo(input));
        cx.notify();
    }
}

fn theme_scope_header(project_scope: bool, project: &str) -> SharedString {
    if project_scope {
        format!("Theme for {project} only  ·  ⇥ for all projects").into()
    } else {
        "Theme for all projects  ·  ⇥ for this project only".into()
    }
}
