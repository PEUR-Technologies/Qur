//! Find in Chat, and the chat as Markdown.

use super::*;

impl AgentTask {
    // -- find in chat ------------------------------------------------------------

    /// Find in Chat (Cmd+F): open the query field above the chat, or focus it.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_subagent = None;
        Docks::update(cx, Kind::Acp, |d| d.activate(Panel::Session));
        if let Some(f) = &self.find {
            let input = f.input.clone();
            input.update(cx, |i, cx| i.select_text(cx));
            let focus = input.read(cx).focus.clone();
            window.focus(&focus, cx);
            return cx.notify();
        }
        let input = cx.new(|cx| TextInput::new("Find in chat · ↩ older · ⇧↩ newer · esc", cx));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Changed => {
                let q = input.read(cx).text.clone();
                this.set_query(q, cx);
            }
            InputEvent::Confirm { alt, .. } => this.find_step(!*alt, cx),
            InputEvent::Cancel => this.close_find(window, cx),
            _ => {}
        });
        let focus = input.read(cx).focus.clone();
        self.find = Some(ChatFind { input: input.clone(), query: String::new(), matches: Vec::new(), current: None, dirty: false, _sub: sub });
        window.focus(&focus, cx);
        // Dev/test: QUR_DEBUG_FIND="text" searches for it at once.
        if let Ok(q) = std::env::var("QUR_DEBUG_FIND") {
            input.update(cx, |i, cx| i.set_text(&q, cx));
        }
        cx.notify();
    }

    /// Esc: close Find; the prompt gets the keyboard back.
    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.find_current().map(|c| c.0);
        self.find = None;
        // A folded item opened for the current match folds again.
        if let Some(r) = row.filter(|r| *r < self.chat.len()) {
            self.list.remeasure_items(r..r + 1);
        }
        let f = self.prompt.read(cx).focus.clone();
        window.focus(&f, cx);
        cx.notify();
    }

    /// (row, number within the row) of Find's current match.
    pub(super) fn find_current(&self) -> Option<(usize, usize)> {
        let f = self.find.as_ref()?;
        f.matches.get(f.current?).copied()
    }

    /// Find's marks for row `ix`, when it has matches.
    pub(super) fn row_hl(&self, ix: usize, look: &Look) -> Option<FindHl> {
        let f = self.find.as_ref().filter(|f| !f.query.is_empty())?;
        let at = f.matches.partition_point(|m| m.0 < ix);
        if !matches!(f.matches.get(at), Some(m) if m.0 == ix) {
            return None;
        }
        let current = self.find_current().filter(|c| c.0 == ix).map(|c| c.1);
        let ed = &look.theme.editor;
        let bg = |c: Rgba| HighlightStyle { background_color: Some(look.c(c)), ..Default::default() };
        Some(FindHl::new(f.query.clone(), current, bg(ed.search_match), bg(ed.search_match_active)))
    }

    /// How many matches row `ix` has, counted in the order `render_item`
    /// draws them (a tool call: its title, then its text and output).
    fn row_matches(&self, ix: usize, q: &str, eid: EntityId) -> usize {
        let n = |s: &str| find_all(s, q).len();
        let root = Some(self.record.worktree.as_path());
        match &self.chat[ix] {
            ChatItem::User(t) | ChatItem::Notice(t) | ChatItem::Output(t) => n(t),
            ChatItem::Agent(t) => crate::markdown::count_matches(t, format!("md-{eid}-{ix}"), root, q),
            ChatItem::Thought(t) => n(t.trim()),
            ChatItem::Link { name, .. } => n(name),
            ChatItem::Failure(f) => n(&f.title) + f.summary.as_deref().map_or(0, n) + f.detail.as_deref().map_or(0, n),
            ChatItem::Tool(id) => {
                let Some(t) = self.tools.get(id) else { return 0 };
                let mut count = n(&tool_title(t));
                // As `tool_body` numbers its parts.
                let mut parts = 0;
                for c in &t.content {
                    match c {
                        ToolContent::Text(s) if !s.trim().is_empty() => {
                            count += crate::markdown::count_matches(s, format!("md-{eid}-{}-{parts}", t.id), root, q);
                            parts += 1;
                        }
                        ToolContent::Terminal(_) => {
                            let out = t.output.as_deref().unwrap_or("").trim_end();
                            if !out.is_empty() || t.exit.is_some() {
                                count += n(&tail_lines(out, MAX_TOOL_LINES));
                            }
                            parts += 1;
                        }
                        _ => {}
                    }
                }
                count
            }
            ChatItem::Image(..) | ChatItem::Turn(_) => 0,
        }
    }

    /// Count every match again (the query or the chat changed), keeping
    /// the current one when it's still there.
    pub(super) fn refind(&mut self, eid: EntityId) {
        let Some(q) = self.find.as_ref().map(|f| f.query.clone()) else { return };
        let mut matches = Vec::new();
        if !q.is_empty() {
            for ix in 0..self.chat.len() {
                matches.extend((0..self.row_matches(ix, &q, eid)).map(|k| (ix, k)));
            }
        }
        let Some(f) = self.find.as_mut() else { return };
        let was = f.current.and_then(|c| f.matches.get(c).copied());
        f.current = was.and_then(|c| matches.iter().position(|m| *m == c));
        f.matches = matches;
        f.dirty = false;
        if let Some((row, _)) = was.filter(|_| f.current.is_none())
            && row < self.chat.len()
        {
            self.list.remeasure_items(row..row + 1);
        }
    }

    /// The query changed: find it, the newest match first.
    fn set_query(&mut self, q: String, cx: &mut Context<Self>) {
        let old = self.find_current().map(|c| c.0);
        let Some(f) = self.find.as_mut() else { return };
        f.query = q;
        f.current = None;
        self.refind(cx.entity_id());
        let Some(f) = self.find.as_mut() else { return };
        f.current = f.matches.len().checked_sub(1);
        println!("FIND {} {:?}: {} matches", self.record.id, f.query, f.matches.len());
        self.moved_to(old, cx);
    }

    /// Enter (older) / Shift+Enter (newer): the next match, round the ends.
    fn find_step(&mut self, older: bool, cx: &mut Context<Self>) {
        if self.find.as_ref().is_some_and(|f| f.dirty) {
            self.refind(cx.entity_id());
        }
        let old = self.find_current().map(|c| c.0);
        let Some(f) = self.find.as_mut() else { return };
        let n = f.matches.len();
        if n == 0 {
            return cx.notify();
        }
        f.current = Some(match f.current {
            None if older => n - 1,
            None => 0,
            Some(c) if older => (c + n - 1) % n,
            Some(c) => (c + 1) % n,
        });
        self.moved_to(old, cx);
    }

    /// Find's current match moved off row `old`: re-measure both rows (a
    /// folded thought, tool call or group opens to show it) and scroll to it.
    fn moved_to(&mut self, old: Option<usize>, cx: &mut Context<Self>) {
        let new = self.find_current();
        for row in [old, new.map(|c| c.0)].into_iter().flatten().filter(|r| *r < self.chat.len()) {
            self.list.remeasure_items(row..row + 1);
        }
        if let Some((row, k)) = new {
            self.list.set_follow_mode(gpui::FollowMode::Normal);
            self.list.scroll_to_reveal_item(row);
            let total = self.find.as_ref().map_or(0, |f| f.matches.len());
            let at = self.find.as_ref().and_then(|f| f.current).map_or(0, |c| c + 1);
            println!("FIND {} match {at} of {total}: row {row} #{k}", self.record.id);
        }
        cx.notify();
    }

    /// The query field, the match count, ↑ ↓ and ✕ (above the chat).
    pub(super) fn find_bar(&self, look: &Look, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let f = self.find.as_ref()?;
        let ui = &look.theme.ui;
        let n = f.matches.len();
        let none = !f.query.is_empty() && n == 0;
        let count = match (f.query.is_empty(), f.current) {
            (true, _) => String::new(),
            (false, _) if n == 0 => "no match".to_string(),
            (false, Some(c)) => format!("{} of {n}", c + 1),
            (false, None) => format!("{n} match{}", if n == 1 { "" } else { "es" }),
        };
        let button = |id: &'static str, label: &'static str| {
            div().id(id).flex_none().px(look.z(4.)).rounded(look.z(4.)).cursor_pointer().text_color(look.c(ui.text_muted)).hover(|d| d.bg(look.c(ui.hover_bg)).text_color(look.c(ui.text))).child(label)
        };
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(look.z(6.))
                .mx(look.z(12.))
                .mt(look.z(6.))
                .mb(look.z(2.))
                .px(look.z(8.))
                .py(look.z(3.))
                .rounded(look.z(4.))
                .border_1()
                .border_color(look.c(if none { ui.error } else { ui.border }))
                .bg(look.c(ui.overlay_bg))
                .text_size(look.text(-1.))
                .child(div().flex_1().min_w(px(0.)).child(f.input.clone()))
                .child(div().flex_none().text_color(look.c(ui.text_muted)).child(count))
                .child(button("find-older", "↑").on_click(cx.listener(|t, _, _, cx| t.find_step(true, cx))))
                .child(button("find-newer", "↓").on_click(cx.listener(|t, _, _, cx| t.find_step(false, cx))))
                .child(button("find-close", "✕").on_click(cx.listener(|t, _, window, cx| t.close_find(window, cx)))),
        )
    }

    /// Copy Chat as Markdown (Agent menu, palette, the sidebar's right-click menu).
    pub fn chat_markdown(&self) -> String {
        chat_markdown(&self.record.title, &self.def.label, &self.chat, &self.tools, &self.record.turns)
    }

    /// `text` as Markdown; file links open in the code modal. `id`: unique
    /// and stable per chat item (see `markdown::render`).
    pub(super) fn markdown(&self, text: &str, id: String, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        self.markdown_find(text, id, look, None, cx)
    }

    /// `markdown`, with Find in Chat's matches marked.
    pub(super) fn markdown_find(&self, text: &str, id: String, look: &Look, hl: Option<&FindHl>, cx: &mut Context<Self>) -> gpui::Div {
        let me = cx.entity().downgrade();
        let open: crate::markdown::OnFile = std::rc::Rc::new(move |path, line, _, cx| {
            me.update(cx, |_, cx| cx.emit(AgentEvent::OpenFile(path, line))).ok();
        });
        crate::markdown::render(text, id, Some(&self.record.worktree), look, open, hl)
    }
}
