//! The chat list and drawing its rows: messages, tool calls and their
//! groups, failures, and the files, plan and permission strips.

use super::*;

impl AgentTask {
    // -- chat list -------------------------------------------------------------

    pub(super) fn push(&mut self, item: ChatItem) {
        // The thought before this item stops streaming: fold it.
        self.remeasure_last_thought();
        let n = self.chat.len();
        self.chat.push(item);
        self.list.splice(n..n, 1);
        self.chat_changed();
        // A tool call joining a run: the group's summary (on its first row)
        // and the row before it change.
        if let Some(run) = tool_run(&self.chat, n).filter(|r| r.len() >= 2) {
            self.list.remeasure_items(run.start..n);
        }
        // A long chat: its oldest rows move to disk.
        self.spill_check();
    }

    /// Find in Chat counts matches again before it next draws.
    pub(super) fn chat_changed(&mut self) {
        if let Some(f) = &mut self.find {
            f.dirty = true;
        }
    }

    /// Re-measure a tool call's row and, in a group, the whole group (its
    /// summary and which rows show depend on every call's status).
    fn remeasure_tool_rows(&mut self, ix: usize) {
        match tool_run(&self.chat, ix) {
            Some(run) => self.list.remeasure_items(run),
            None => self.list.remeasure_items(ix..ix + 1),
        }
    }

    /// Back to the newest message, following it again as it streams.
    pub(super) fn follow_bottom(&self) {
        self.list.set_follow_mode(gpui::FollowMode::Tail);
        self.list.scroll_to_end();
    }

    /// Rows are measured once; a thought's open/folded state depends on
    /// whether it's still the streaming last item, so re-measure it.
    pub(super) fn remeasure_last_thought(&mut self) {
        let last = self.chat.len().wrapping_sub(1);
        if matches!(self.chat.last(), Some(ChatItem::Thought(_))) {
            self.list.splice(last..last + 1, 1);
        }
    }

    /// Append streamed text to the last item of the same kind, or start one.
    pub(super) fn append(&mut self, thought: bool, text: &str) {
        let last = self.chat.len().wrapping_sub(1);
        match self.chat.last_mut() {
            Some(ChatItem::Agent(s)) if !thought => s.push_str(text),
            Some(ChatItem::Thought(s)) if thought => s.push_str(text),
            _ => {
                self.push(if thought { ChatItem::Thought(text.to_string()) } else { ChatItem::Agent(text.to_string()) });
                return;
            }
        }
        self.list.splice(last..last + 1, 1);
        self.chat_changed();
    }

    /// An image or link block from a message: its own chat row. An image
    /// is saved to the temp folder (click opens it) and decoded at twice
    /// the size it's drawn, both off the main thread; the chat keeps only
    /// that and the file's path.
    pub(super) fn push_media(&mut self, media: Option<Media>, cx: &mut Context<Self>) {
        match media {
            Some(Media::Image { mime, data }) => match gpui::ImageFormat::from_mime_type(&mime) {
                Some(f) => {
                    // The size now, from the header: the row is measured once.
                    let Some((w, h)) = crate::files::image_size(&data) else {
                        return self.push(ChatItem::Notice("[image that couldn't be read]".into()));
                    };
                    let path = crate::files::temp_image_path("image", f);
                    self.push(ChatItem::Image(None, (w, h), path.clone()));
                    // Drawn at most 480 high (see `render_item`).
                    let drawn_w = w as f32 * (480. / h as f32).min(1.);
                    let (max_w, max_h) = ((drawn_w * 2.).ceil() as u32, 960);
                    let to = path.clone();
                    let work = cx.background_executor().spawn(async move {
                        std::fs::write(&to, &data).ok();
                        crate::files::thumbnail(&data, max_w, max_h)
                    });
                    cx.spawn(async move |this, cx| {
                        let Some(thumb) = work.await else { return };
                        this.update(cx, |t, cx| {
                            if let Some(ChatItem::Image(slot, ..)) = t.chat.iter_mut().find(|c| matches!(c, ChatItem::Image(None, _, p) if *p == path)) {
                                *slot = Some(thumb);
                                cx.notify();
                            }
                        })
                        .ok();
                    })
                    .detach();
                }
                None => self.push(ChatItem::Notice(format!("[{mime} image]"))),
            },
            Some(Media::Link { uri, name }) => self.push(ChatItem::Link { uri, name }),
            None => {}
        }
    }

    /// The images and files attached to a message, as a row under it. The
    /// row keeps thumbnails and paths, not the images: a pasted image or a
    /// screenshot is written to the temp folder (off the main thread) so
    /// clicking it opens the file.
    pub(super) fn push_sent(&mut self, out: &Outgoing, cx: &mut Context<Self>) {
        let mut row = Vec::new();
        for i in &out.images {
            let path = match &i.path {
                Some(p) => p.clone(),
                None => {
                    let p = crate::files::temp_image_path("paste", i.image.format);
                    let (img, to) = (i.image.clone(), p.clone());
                    cx.background_executor().spawn(async move { std::fs::write(to, img.bytes()).ok() }).detach();
                    p
                }
            };
            row.push(Sent::Image(i.thumb.clone(), crate::files::image_size(i.image.bytes()), path));
        }
        row.extend(out.files.iter().cloned().map(Sent::File));
        if !row.is_empty() {
            self.push(ChatItem::Files(row));
        }
    }

    /// A link in a message: a file opens in the code modal, a local address
    /// in the task's Browser view, anything else in the default browser.
    fn open_link(&mut self, uri: &str, cx: &mut Context<Self>) {
        if let Some(path) = uri.strip_prefix("file://") {
            // `file:///abs/path#L12` or `…:12`: open at that line when given.
            let (path, line) = match path.rsplit_once("#L").or_else(|| path.rsplit_once(':')) {
                Some((p, l)) if l.parse::<u32>().is_ok() => (p, l.parse().ok()),
                _ => (path, None),
            };
            let path = PathBuf::from(path.replace("%20", " "));
            return cx.emit(AgentEvent::OpenFile(path, line));
        }
        let local = ["http://localhost", "http://127.0.0.1", "https://localhost"].iter().any(|p| uri.starts_with(p));
        if local {
            self.browser.update(cx, |b, cx| b.navigate(uri, cx));
            self.show_browser(true, cx);
        } else {
            cx.open_url(uri);
        }
    }

    /// Error callout: send that message again.
    fn retry(&mut self, out: Outgoing, cx: &mut Context<Self>) {
        self.follow_bottom();
        if matches!(self.status, Status::Working | Status::NeedsYou | Status::Starting) || self.setting_up || self.sign_in.is_some() || self.rewinding {
            self.pending.push(out);
            self.save_queue(cx);
            return cx.notify();
        }
        self.send_out(out, cx);
    }

    /// Error callout: the message back in the prompt to change and send.
    fn edit_resend(&mut self, out: &Outgoing, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt.update(cx, |p, cx| p.set_text(&out.text, cx));
        let f = self.prompt.read(cx).focus.clone();
        window.focus(&f, cx);
        cx.notify();
    }

    pub(super) fn refresh_tool(&mut self, id: &ToolCallId) {
        if let Some(ix) = self.chat.iter().rposition(|c| matches!(c, ChatItem::Tool(t) if t == id)) {
            self.remeasure_tool_rows(ix);
            self.chat_changed();
        }
    }

    // -- rendering ---------------------------------------------------------------

    pub(super) fn render_item(&self, ix: usize, look: &Look, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ui = &look.theme.ui;
        let pad = |d: gpui::Div| d.px(look.z(16.)).py(look.z(6.)).w_full();
        // Find in Chat: this row's matches marked, the current one stronger.
        let hl = self.row_hl(ix, look);
        let hl = hl.as_ref();
        let current_here = self.find_current().is_some_and(|(row, _)| row == ix);
        match &self.chat[ix] {
            ChatItem::User(t) => pad(div())
                .child(
                    div()
                        .text_size(look.text(0.))
                        .px(look.z(10.))
                        .py(look.z(6.))
                        .rounded(look.z(6.))
                        .bg(look.c(ui.active_bg))
                        .text_color(look.c(ui.text))
                        .child(marked(hl, t.clone())),
                )
                .into_any_element(),
            // Hover shows "Copy" (the raw Markdown).
            ChatItem::Agent(t) => pad(div())
                .relative()
                .group("agent-msg")
                .child(self.markdown_find(t, format!("md-{}-{ix}", cx.entity_id()), look, hl, cx))
                .child(
                    div()
                        .absolute()
                        .top(look.z(2.))
                        .right(look.z(12.))
                        .rounded(look.z(4.))
                        .bg(look.c(ui.panel_bg))
                        .invisible()
                        .group_hover("agent-msg", |s| s.visible())
                        .child(crate::markdown::copy_button(("copy-msg", ix), t.clone(), look)),
                )
                .into_any_element(),
            // Open while it streams (the last item of a working turn), then
            // folded to its first line; click to toggle (as in Zed). Find's
            // current match opens it too.
            ChatItem::Thought(t) => {
                let streaming = ix + 1 == self.chat.len() && matches!(self.status, Status::Working);
                let open = (streaming != self.toggled.contains(&format!("thought-{ix}"))) || current_here;
                let t = t.trim();
                let first = t.lines().next().unwrap_or("").to_string();
                pad(div())
                    .text_size(look.text(0.))
                    .text_color(look.c(ui.text_muted))
                    .child(
                        div()
                            .id(("thought", ix))
                            .flex()
                            .gap(look.z(6.))
                            .cursor_pointer()
                            .hover(|d| d.text_color(look.c(ui.text_secondary)))
                            .child(div().flex_none().child(if open { "▾ Thinking" } else { "▸ Thinking" }))
                            .when(!open, |d| d.child(div().min_w(px(0.)).truncate().italic().child(first)))
                            .on_click(cx.listener(move |t, _, _, cx| t.toggle(format!("thought-{ix}"), ix, cx))),
                    )
                    .when(open, |d| d.child(div().pl(look.z(14.)).pt(look.z(2.)).italic().child(marked(hl, t.to_string()))))
                    .into_any_element()
            }
            ChatItem::Notice(t) => pad(div()).text_size(look.text(0.)).text_color(look.c(ui.text_muted)).child(marked(hl, t.clone())).into_any_element(),
            ChatItem::Output(t) => pad(div())
                .child(
                    div()
                        .px(look.z(10.))
                        .py(look.z(6.))
                        .rounded(look.z(4.))
                        .bg(look.c(ui.panel_bg))
                        .border_1()
                        .border_color(look.c(ui.border))
                        .font_family(look.code_font.clone())
                        .text_size(look.text(0.))
                        .text_color(look.c(ui.text_secondary))
                        .child(marked(hl, t.clone())),
                )
                .into_any_element(),
            // At its own size, at most 480 high and never wider than the
            // chat; the aspect ratio keeps the height right when it shrinks.
            // Click shows it full size. A blank frame of its size until
            // it's decoded.
            ChatItem::Image(thumb, (w, h), path) => {
                let (w, h) = (*w as f32, *h as f32);
                let width = w * (480. / h).min(1.);
                let preview = Preview::File(path.clone());
                let frame = match thumb {
                    Some(t) => img(t.clone()).w_full().aspect_ratio(w / h).rounded(look.z(4.)).border_1().border_color(look.c(ui.border)).into_any_element(),
                    None => div().w_full().aspect_ratio(w / h).rounded(look.z(4.)).border_1().border_color(look.c(ui.border)).bg(look.c(ui.panel_bg)).into_any_element(),
                };
                pad(div())
                    .child(
                        div()
                            .id(("image", ix))
                            .w(look.z(width))
                            .max_w_full()
                            .cursor_pointer()
                            .child(frame)
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(AgentEvent::Preview(preview.clone())))),
                    )
                    .into_any_element()
            }
            ChatItem::Files(files) => pad(div()).pt(px(0.)).child(self.sent_row(ix, files, look, cx)).into_any_element(),
            ChatItem::Link { uri, name } => {
                let target = uri.clone();
                pad(div())
                    .child(
                        div()
                            .id(("link", ix))
                            .flex()
                            .items_center()
                            .gap(look.z(6.))
                            .max_w_full()
                            .px(look.z(8.))
                            .py(look.z(2.))
                            .rounded(look.z(4.))
                            .border_1()
                            .border_color(look.c(ui.border))
                            .bg(look.c(ui.panel_bg))
                            .cursor_pointer()
                            .text_size(look.text(0.))
                            .text_color(look.c(ui.text_secondary))
                            .hover(|d| d.text_color(look.c(ui.text)).border_color(look.c(ui.border_strong)))
                            .child(div().flex_none().text_color(look.c(ui.text_muted)).child(if uri.starts_with("file://") { "▤" } else { "↗" }))
                            .child(div().min_w(px(0.)).truncate().child(marked(hl, name.clone())))
                            .on_click(cx.listener(move |t, _, _, cx| t.open_link(&target, cx))),
                    )
                    .into_any_element()
            }
            ChatItem::Failure(f) => pad(div()).child(self.failure_callout(ix, f, look, hl, cx)).into_any_element(),
            ChatItem::Turn(n) => pad(div()).child(self.turn_line(ix, *n, look, cx)).into_any_element(),
            ChatItem::Earlier => {
                let n = self.history.hidden;
                pad(div())
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .id("show-earlier")
                            .relative()
                            .px(look.z(10.))
                            .py(look.z(3.))
                            .rounded(look.z(4.))
                            .cursor_pointer()
                            .text_size(look.text(-1.))
                            .text_color(look.c(ui.text_secondary))
                            .hover(|d| d.bg(look.c(ui.hover_bg)).text_color(look.c(ui.text)))
                            .child(format!("Show earlier messages ({n} more)"))
                            .children(crate::script::mark(|| "show-earlier".into()))
                            .on_click(cx.listener(|t, _, _, cx| t.show_earlier(cx))),
                    )
                    .into_any_element()
            }
            // Two or more calls in a row are one group: a summary line
            // ("Read 4 files, ran 2 commands ▸") that unfolds to the calls.
            // Folded, a call still shows while it runs, failed, waits for
            // permission or holds Find's current match.
            ChatItem::Tool(id) => {
                let Some(run) = tool_run(&self.chat, ix).filter(|r| r.len() >= 2) else {
                    return self.tool_row(ix, id, look, hl, current_here, cx).into_any_element();
                };
                let open = self.group_open(&run);
                let row = (open || self.tool_needs_eye(ix, id)).then(|| self.tool_row(ix, id, look, hl, current_here, cx).pl(look.z(30.)));
                if ix != run.start {
                    return row.map(|r| r.into_any_element()).unwrap_or_else(|| div().into_any_element());
                }
                div().w_full().flex().flex_col().child(self.group_header(run, open, look, cx)).children(row).into_any_element()
            }
        }
    }

    /// A message's attachments: images as thumbnails (at most 96 high), files
    /// as chips. Click shows one: the viewer, or the code modal for text.
    fn sent_row(&self, ix: usize, files: &[Sent], look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        div().flex().flex_wrap().gap(look.z(6.)).children(files.iter().enumerate().map(|(i, f)| {
            let (preview, el) = match f {
                Sent::Image(thumb, size, path) => {
                    let (w, h) = size.map_or((96., 96.), |(w, h)| (w as f32, h as f32));
                    let height = h.min(96.);
                    let width = (w * height / h).min(320.);
                    let el = match thumb {
                        Some(t) => img(t.clone())
                            .w(look.z(width))
                            .h(look.z(height))
                            .object_fit(gpui::ObjectFit::Cover)
                            .rounded(look.z(4.))
                            .border_1()
                            .border_color(look.c(ui.border))
                            .into_any_element(),
                        None => div().w(look.z(width)).h(look.z(height)).rounded(look.z(4.)).border_1().border_color(look.c(ui.border)).bg(look.c(ui.panel_bg)).into_any_element(),
                    };
                    (Preview::File(path.clone()), el)
                }
                Sent::File(p) => {
                    let el = div()
                        .flex()
                        .items_center()
                        .gap(look.z(6.))
                        .px(look.z(8.))
                        .py(look.z(2.))
                        .rounded(look.z(4.))
                        .border_1()
                        .border_color(look.c(ui.border))
                        .bg(look.c(ui.panel_bg))
                        .text_size(look.text(-1.))
                        .text_color(look.c(ui.text_secondary))
                        .hover(|d| d.text_color(look.c(ui.text)).border_color(look.c(ui.border_strong)))
                        .child(div().flex_none().text_color(look.c(ui.text_muted)).child("▤"))
                        .child(div().max_w(look.z(260.)).truncate().child(crate::files::chip_name(p)))
                        .into_any_element();
                    (Preview::File(p.clone()), el)
                }
            };
            div()
                .id(SharedString::from(format!("sent-{ix}-{i}")))
                .cursor_pointer()
                .child(el)
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(AgentEvent::Preview(preview.clone()))))
        }))
    }

    /// One tool call's row: kind, title, counts, status, and its body when
    /// unfolded.
    fn tool_row(&self, ix: usize, id: &ToolCallId, look: &Look, hl: Option<&FindHl>, current_here: bool, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let Some(t) = self.tools.get(id) else { return div() };
        let (glyph, color) = match t.status {
            ToolStatus::Pending | ToolStatus::InProgress => ("…", ui.text_muted),
            ToolStatus::Completed => ("✓", ui.success),
            ToolStatus::Failed => ("✕", ui.error),
        };
        let asking = self.permissions.iter().any(|p| p.tool.id == *id);
        let kind = kind_word(t.kind);
        // Real line counts (every line of old and new counted before).
        let diffs = tool_diffs(t);
        let add = diffs.iter().flat_map(|(_, d)| d).filter(|(k, _)| *k == '+').count();
        let del = diffs.iter().flat_map(|(_, d)| d).filter(|(k, _)| *k == '-').count();
        let loc = t.locations.first().cloned();
        let title = tool_title(t);
        let body = self.tool_body(t, &diffs, look, hl, cx);
        // Edits and failures start open; the rest folded (click toggles).
        // A command stays open while it runs, so its output shows as it
        // streams (`terminal_output_delta`), then folds.
        let running_cmd = t.content.iter().any(|c| matches!(c, ToolContent::Terminal(_))) && t.exit.is_none() && matches!(t.status, ToolStatus::Pending | ToolStatus::InProgress);
        let default_open = matches!(t.kind, ToolKind::Edit | ToolKind::Delete | ToolKind::Move) || t.status == ToolStatus::Failed || running_cmd;
        let open = body.is_some() && (default_open != self.toggled.contains(id.as_str()) || current_here);
        let expandable = body.is_some();
        let tid = id.clone();
        div()
            .px(look.z(16.))
            .py(look.z(2.))
            .w_full()
            .child(
                div()
                    .id(SharedString::from(format!("tool-{id}")))
                    .flex()
                    .items_center()
                    .gap(look.z(8.))
                    .text_size(look.text(0.))
                    .text_color(look.c(ui.text_secondary))
                    .when(expandable || loc.is_some(), |d| d.cursor_pointer().hover(|d| d.text_color(look.c(ui.text))))
                    .child(div().w(look.z(40.)).flex_none().text_color(look.c(ui.text_muted)).child(kind))
                    .child(div().min_w(px(0.)).truncate().child(marked(hl, title)))
                    .when(add + del > 0, |d| {
                        d.child(div().flex_none().text_color(look.c(look.theme.diff.added_text)).child(format!("+{add}")))
                            .child(div().flex_none().text_color(look.c(look.theme.diff.removed_text)).child(format!("−{del}")))
                    })
                    .when_some(t.exit.flatten().filter(|c| *c != 0), |d, c| d.child(div().flex_none().text_color(look.c(ui.error)).child(format!("exit {c}"))))
                    .when(asking, |d| d.child(div().flex_none().text_color(look.c(self.accent)).child("needs you")))
                    .child(div().flex_none().text_color(look.c(color)).child(glyph))
                    .when(expandable, |d| d.child(div().flex_none().text_color(look.c(ui.text_muted)).child(if open { "▾" } else { "▸" })))
                    .on_click(cx.listener(move |t, _, _, cx| {
                        if expandable {
                            t.toggle(tid.to_string(), ix, cx);
                        } else if let Some(l) = &loc {
                            cx.emit(AgentEvent::OpenFile(l.path.clone(), l.line));
                        }
                    })),
            )
            .when_some(body.filter(|_| open), |d, b| d.child(div().pl(look.z(48.)).pt(look.z(4.)).child(b)))
    }

    /// The tool calls of a group (`run` of chat rows).
    fn run_tools(&self, run: &Range<usize>) -> Vec<&ToolCall> {
        self.chat[run.clone()]
            .iter()
            .filter_map(|c| match c {
                ChatItem::Tool(id) => self.tools.get(id),
                _ => None,
            })
            .collect()
    }

    /// A group's fold key: its first call's id.
    fn group_key(&self, run: &Range<usize>) -> String {
        match &self.chat[run.start] {
            ChatItem::Tool(id) => format!("group-{id}"),
            _ => String::new(),
        }
    }

    /// Groups start folded; a click on the summary unfolds them.
    fn group_open(&self, run: &Range<usize>) -> bool {
        self.toggled.contains(&self.group_key(run))
    }

    /// A call that shows even in a folded group.
    fn tool_needs_eye(&self, ix: usize, id: &ToolCallId) -> bool {
        let status = self.tools.get(id).map(|t| t.status);
        matches!(status, Some(ToolStatus::Pending | ToolStatus::InProgress | ToolStatus::Failed))
            || self.permissions.iter().any(|p| p.tool.id == *id)
            || self.find_current().is_some_and(|(row, _)| row == ix)
    }

    fn toggle_group(&mut self, run: Range<usize>, cx: &mut Context<Self>) {
        let key = self.group_key(&run);
        if !self.toggled.remove(&key) {
            self.toggled.insert(key);
        }
        self.list.remeasure_items(run);
        cx.notify();
    }

    /// "Read 4 files, ran 2 commands  ✓ ▸".
    fn group_header(&self, run: Range<usize>, open: bool, look: &Look, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let tools = self.run_tools(&run);
        let failed = tools.iter().filter(|t| t.status == ToolStatus::Failed).count();
        let running = tools.iter().any(|t| matches!(t.status, ToolStatus::Pending | ToolStatus::InProgress));
        let (glyph, color) = if running {
            ("…", ui.text_muted)
        } else if failed > 0 {
            ("✕", ui.error)
        } else {
            ("✓", ui.success)
        };
        let summary = group_summary(&tools);
        let n = tools.len();
        div().px(look.z(16.)).py(look.z(2.)).w_full().child(
            div()
                .id(("tool-group", run.start))
                .flex()
                .items_center()
                .gap(look.z(8.))
                .text_size(look.text(0.))
                .text_color(look.c(ui.text_secondary))
                .cursor_pointer()
                .hover(|d| d.text_color(look.c(ui.text)))
                .child(div().w(look.z(40.)).flex_none().text_color(look.c(ui.text_muted)).child(format!("{n} ×")))
                .child(div().min_w(px(0.)).truncate().child(summary))
                .when(failed > 0, |d| d.child(div().flex_none().text_color(look.c(ui.error)).child(format!("{failed} failed"))))
                .child(div().flex_none().text_color(look.c(color)).child(glyph))
                .child(div().flex_none().text_color(look.c(ui.text_muted)).child(if open { "▾" } else { "▸" }))
                .on_click(cx.listener(move |t, _, _, cx| t.toggle_group(run.clone(), cx))),
        )
    }

    /// Fold or unfold a chat item (tool call `key` = its id, thought
    /// `thought-<ix>`) and re-measure its row.
    fn toggle(&mut self, key: String, ix: usize, cx: &mut Context<Self>) {
        if !self.toggled.remove(&key) {
            self.toggled.insert(key);
        }
        self.list.splice(ix..ix + 1, 1);
        cx.notify();
    }

    /// An error-coloured callout: what went wrong, the full error (bounded,
    /// scrolls), then Retry / Edit & resend when a message can be sent
    /// again, and Copy error.
    fn failure_callout(&self, ix: usize, f: &Failure, look: &Look, hl: Option<&FindHl>, cx: &mut Context<Self>) -> gpui::Div {
        let ui = &look.theme.ui;
        let button = |id: &'static str, label: &'static str| {
            div()
                .id((id, ix))
                .px(look.z(10.))
                .py(look.z(3.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_size(look.text(0.))
                .bg(look.c(ui.button_bg))
                .text_color(look.c(ui.button_text))
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .child(label)
        };
        let copy = [Some(f.title.clone()), f.summary.clone(), f.detail.clone()].into_iter().flatten().collect::<Vec<_>>().join("\n\n");
        let (retry, edit) = (f.resend.clone(), f.resend.clone());
        div()
            .px(look.z(12.))
            .py(look.z(8.))
            .rounded(look.z(6.))
            .border_1()
            .border_color(look.c(ui.error))
            .flex()
            .flex_col()
            .gap(look.z(6.))
            .text_size(look.text(0.))
            .child(div().text_color(look.c(ui.error)).child(marked(hl, f.title.clone())))
            .children(f.summary.clone().map(|t| div().text_color(look.c(ui.text)).child(marked(hl, t))))
            // The raw error, small and scrollable, for when the summary isn't enough.
            .children(f.detail.clone().map(|d| {
                div()
                    .id(("failure-detail", ix))
                    .max_h(look.z(if f.summary.is_some() { 90. } else { 180. }))
                    .overflow_y_scroll()
                    .child(code_block_find(&d, look, hl))
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(look.z(6.))
                    .children(retry.map(|out| button("failure-retry", "Retry").on_click(cx.listener(move |t, _, _, cx| t.retry(out.clone(), cx)))))
                    .children(edit.map(|out| button("failure-edit", "Edit & resend").on_click(cx.listener(move |t, _, window, cx| t.edit_resend(&out, window, cx)))))
                    .child(button("failure-copy", "Copy error").on_click(move |_, _, cx: &mut App| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())))),
            )
    }

    /// What an unfolded tool call shows: its text (code blocks in the code
    /// font), each file's diff, a command's output and exit, or else its raw
    /// input/output. None when there's nothing to show.
    fn tool_body(&self, t: &ToolCall, diffs: &[(&PathBuf, Vec<(char, &str)>)], look: &Look, hl: Option<&FindHl>, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let ui = &look.theme.ui;
        let mut parts: Vec<gpui::AnyElement> = Vec::new();
        for c in &t.content {
            match c {
                ToolContent::Text(s) if !s.trim().is_empty() => parts.push(self.markdown_find(s, format!("md-{}-{}-{}", cx.entity_id(), t.id, parts.len()), look, hl, cx).into_any_element()),
                ToolContent::Terminal(_) => {
                    let out = t.output.as_deref().unwrap_or("").trim_end();
                    let status = match t.exit {
                        None => "running…".to_string(),
                        Some(Some(0)) => "exit 0".into(),
                        Some(Some(c)) => format!("exit {c}"),
                        Some(None) => "ended".into(),
                    };
                    let failed = matches!(t.exit, Some(Some(c)) if c != 0);
                    // Still running with nothing yet: just "running…" (the
                    // Claude adapter sends the output when the command ends).
                    let block = (!out.is_empty() || t.exit.is_some()).then(|| code_block_find(&tail_lines(out, MAX_TOOL_LINES), look, hl).when(out.is_empty(), |d| d.child("(no output)")));
                    parts.push(
                        div()
                            .flex()
                            .flex_col()
                            .gap(look.z(2.))
                            .children(block)
                            .child(div().text_size(look.text(0.)).text_color(look.c(if failed { ui.error } else { ui.text_muted })).child(status))
                            .into_any_element(),
                    );
                }
                _ => {}
            }
        }
        for (path, lines) in diffs {
            let path = (*path).clone();
            let shown = lines.iter().filter(|(k, _)| *k != ' ').count().min(MAX_TOOL_LINES);
            let rows = lines.iter().filter(|(k, _)| *k != ' ').take(MAX_TOOL_LINES).map(|(k, l)| {
                let (bg, fg) = if *k == '+' { (look.theme.diff.added_line, look.theme.diff.added_text) } else { (look.theme.diff.removed_line, look.theme.diff.removed_text) };
                div().px(look.z(6.)).bg(look.c(bg)).child(div().text_color(look.c(fg)).child(format!("{k} {l}")))
            });
            let more = lines.iter().filter(|(k, _)| *k != ' ').count() - shown;
            parts.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(look.z(2.))
                    .child(
                        div()
                            .id(SharedString::from(format!("diff-{}", path.display())))
                            .text_size(look.text(0.))
                            .text_color(look.c(ui.text_muted))
                            .cursor_pointer()
                            .hover(|d| d.text_color(look.c(ui.text)))
                            .child(self.rel(&path))
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(AgentEvent::OpenFile(path.clone(), None)))),
                    )
                    .child(
                        div()
                            .rounded(look.z(4.))
                            .border_1()
                            .border_color(look.c(ui.border))
                            .overflow_hidden()
                            .font_family(look.code_font.clone())
                            .text_size(look.text(0.))
                            .children(rows)
                            .when(more > 0, |d| d.child(div().px(look.z(6.)).text_color(look.c(ui.text_muted)).child(format!("… {more} more changed lines")))),
                    )
                    .into_any_element(),
            );
        }
        if parts.is_empty() {
            // Nothing structured: the raw input, then the raw result.
            for v in [&t.raw_input, &t.raw_output].into_iter().flatten() {
                let text = match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Object(m) if m.is_empty() => continue,
                    v => serde_json::to_string_pretty(v).unwrap_or_default(),
                };
                if !text.trim().is_empty() {
                    parts.push(code_block(&tail_lines(text.trim_end(), MAX_TOOL_LINES), look).into_any_element());
                }
            }
        }
        (!parts.is_empty()).then(|| div().flex().flex_col().gap(look.z(6.)).children(parts).into_any_element())
    }

    /// A tool call of the running turn: remember it and recount the files
    /// the turn changed (the strip above the prompt).
    pub(super) fn track_turn_tool(&mut self, id: ToolCallId) {
        if !matches!(self.status, Status::Working | Status::NeedsYou) {
            return;
        }
        if !self.turn_tools.contains(&id) {
            self.turn_tools.push(id);
        }
        let tools: Vec<&ToolCall> = self.turn_tools.iter().filter_map(|id| self.tools.get(id)).collect();
        self.turn_files = turn_changes(&tools);
    }

    /// "3 files changed · +40 −12 · Review", then a chip per file (opens it).
    pub(super) fn files_strip(&self, look: &Look, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.turn_files.is_empty() {
            return None;
        }
        let ui = &look.theme.ui;
        let (add, del) = change_totals(&self.turn_files);
        let n = self.turn_files.len();
        let counts = |d: gpui::Stateful<gpui::Div>, add: usize, del: usize| {
            d.when(add > 0, |d| d.child(div().flex_none().text_color(look.c(look.theme.diff.added_text)).child(format!("+{add}"))))
                .when(del > 0, |d| d.child(div().flex_none().text_color(look.c(look.theme.diff.removed_text)).child(format!("−{del}"))))
        };
        let chips: Vec<_> = self
            .turn_files
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let path = f.path.clone();
                let name = f.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.rel(&f.path));
                let chip = div()
                    .id(("turn-file", i))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(look.z(4.))
                    .px(look.z(6.))
                    .py(look.z(1.))
                    .rounded(look.z(4.))
                    .cursor_pointer()
                    .bg(look.c(ui.active_bg))
                    .text_color(look.c(ui.text_secondary))
                    .hover(|d| d.text_color(look.c(ui.text)))
                    .child(div().max_w(look.z(220.)).truncate().child(name));
                counts(chip, f.add, f.del).on_click(cx.listener(move |_, _, _, cx| cx.emit(AgentEvent::OpenFile(path.clone(), None))))
            })
            .collect();
        let summary = div()
            .id("turn-files")
            .flex()
            .flex_none()
            .items_center()
            .gap(look.z(6.))
            .text_color(look.c(ui.text_secondary))
            .child(format!("{n} file{} changed", if n == 1 { "" } else { "s" }));
        Some(
            div()
                .flex_none()
                .mx(look.z(12.))
                .mb(look.z(6.))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(look.z(6.))
                .text_size(look.text(0.))
                .child(counts(summary, add, del))
                .child(
                    div()
                        .id("turn-review")
                        .flex_none()
                        .px(look.z(6.))
                        .rounded(look.z(4.))
                        .cursor_pointer()
                        .text_color(look.c(self.accent))
                        .hover(|d| d.bg(look.c(ui.hover_bg)))
                        .child("Review")
                        .on_click(cx.listener(|t, _, _, cx| t.show_changes(cx))),
                )
                .children(chips),
        )
    }

    pub(super) fn plan_strip(&self, look: &Look) -> Option<impl IntoElement> {
        if self.plan.is_empty() {
            return None;
        }
        let ui = &look.theme.ui;
        let done = self.plan.iter().filter(|p| p.status == PlanStatus::Completed).count();
        Some(
            div()
                .flex_none()
                .px(look.z(16.))
                .py(look.z(8.))
                .border_b_1()
                .border_color(look.c(ui.border))
                .flex()
                .flex_col()
                .gap(look.z(3.))
                .text_size(look.text(0.))
                .child(div().text_size(look.text(0.)).font_weight(gpui::FontWeight::MEDIUM).text_color(look.c(ui.text_muted)).child(format!("TO-DO  {done}/{}", self.plan.len())))
                .children(self.plan.iter().map(|p| {
                    let (g, c) = match p.status {
                        PlanStatus::Completed => ("●", ui.text_muted),
                        PlanStatus::InProgress => ("◐", ui.text),
                        PlanStatus::Pending => ("○", ui.text_secondary),
                    };
                    div()
                        .flex()
                        .gap(look.z(8.))
                        .text_color(look.c(c))
                        .when(p.status == PlanStatus::InProgress, |d| d.font_weight(gpui::FontWeight::MEDIUM))
                        .child(div().flex_none().text_color(look.c(self.accent)).child(g))
                        .child(div().min_w(px(0.)).child(p.content.clone()))
                })),
        )
    }

    pub(super) fn permission_bar(&self, look: &Look, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = self.permissions.front()?;
        let waiting = self.permissions.len();
        let ui = &look.theme.ui;
        let what = p.tool.title.clone().unwrap_or_else(|| "use a tool".into());
        let file = p.tool.locations.as_ref().and_then(|l| l.first()).map(|l| self.rel(&l.path));
        // What's being approved: the diff, text or command, as in the chat.
        let tool = self.tools.get(&p.tool.id).cloned().map(|mut t| {
            t.apply(&p.tool);
            t
        });
        let tool = tool.unwrap_or_else(|| p.tool.to_tool_call());
        let diffs = tool_diffs(&tool);
        let detail = self.tool_body(&tool, &diffs, look, None, cx);
        // Every option the agent offers, in its words ("Yes, and auto-accept
        // edits", "No, keep planning"…); the first allow is the primary one.
        let primary = p.options.iter().position(|o| o.kind == PermissionKind::AllowOnce).or_else(|| p.options.iter().position(|o| matches!(o.kind, PermissionKind::AllowAlways)));
        // Cmd+Y / Cmd+Opt+Y / Cmd+Opt+Z answer with the first option of their kind.
        let key_for = |i: usize, o: &PermissionOption| -> Option<String> {
            let action: Box<dyn gpui::Action> = match o.kind {
                PermissionKind::AllowOnce => Box::new(aa::AllowOnce),
                PermissionKind::AllowAlways => Box::new(aa::AllowAlways),
                PermissionKind::RejectOnce => Box::new(aa::Deny),
                PermissionKind::RejectAlways => return None,
            };
            let first = p.options.iter().position(|x| x.kind == o.kind) == Some(i);
            let keys = crate::commands::find(action.as_ref())?.first_keys();
            (first && !keys.is_empty()).then(|| crate::commands::display_keys(&keys))
        };
        let buttons: Vec<_> = p
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let is_primary = Some(i) == primary;
                let reject = matches!(o.kind, PermissionKind::RejectOnce | PermissionKind::RejectAlways);
                let label = if o.name.trim().is_empty() {
                    match o.kind {
                        PermissionKind::AllowOnce => "Allow once",
                        PermissionKind::AllowAlways => "Allow for this task",
                        PermissionKind::RejectOnce => "Deny",
                        PermissionKind::RejectAlways => "Always deny",
                    }
                    .to_string()
                } else {
                    o.name.clone()
                };
                let id = o.id.clone();
                let key = key_for(i, o);
                div()
                    .id(("perm-option", i))
                    .px(look.z(10.))
                    .py(look.z(3.))
                    .rounded(look.z(4.))
                    .cursor_pointer()
                    .text_size(look.text(0.))
                    .bg(look.c(if is_primary { ui.button_primary_bg } else { ui.button_bg }))
                    .text_color(look.c(if is_primary { ui.button_primary_text } else if reject { ui.text_secondary } else { ui.button_text }))
                    .flex()
                    .items_center()
                    .gap(look.z(6.))
                    .child(label)
                    .children(key.map(|k| div().text_size(look.text(-2.)).opacity(0.6).child(k)))
                    .on_click(cx.listener(move |t, _, _, cx| t.answer_option(&id, cx)))
            })
            .collect();
        // More waiting: allow them all at once (each "allow once").
        let allow_all = (waiting > 1).then(|| {
            div()
                .id("perm-allow-all")
                .px(look.z(10.))
                .py(look.z(3.))
                .rounded(look.z(4.))
                .cursor_pointer()
                .text_size(look.text(0.))
                .border_1()
                .border_color(look.c(ui.border_strong))
                .text_color(look.c(ui.text))
                .hover(|d| d.bg(look.c(ui.hover_bg)))
                .child(format!("Allow all {waiting} once"))
                .on_click(cx.listener(|t, _, _, cx| {
                    t.allow_all_pending(cx);
                }))
        });
        Some(
            div()
                .flex_none()
                .mx(look.z(12.))
                .mb(look.z(6.))
                .px(look.z(12.))
                .py(look.z(8.))
                .rounded(look.z(6.))
                .border_1()
                .border_color(look.c(self.accent))
                .bg(look.c(ui.permission_bar_bg))
                .flex()
                .flex_col()
                .gap(look.z(6.))
                // The buttons come right after the title, so a long command or
                // diff can't push them off screen; both of those scroll in
                // their own boxes instead of growing the bar over the chat.
                .child(
                    div()
                        .id("perm-title")
                        .min_w(px(0.))
                        .max_h(look.z(90.))
                        .overflow_y_scroll()
                        .text_color(look.c(ui.text))
                        .child(format!("{} wants to {what}", self.def.label))
                        .when(waiting > 1, |d| d.child(div().text_size(look.text(-1.)).text_color(look.c(ui.text_muted)).child(format!("1 of {waiting} requests waiting")))),
                )
                .child(div().flex().flex_wrap().gap(look.z(6.)).children(buttons).children(allow_all))
                .when(file.is_some() || detail.is_some(), |d| {
                    d.child(
                        div()
                            .id("perm-what")
                            .min_w(px(0.))
                            .max_h(look.z(220.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap(look.z(4.))
                            .children(file.map(|f| div().truncate().text_color(look.c(ui.text_muted)).child(f)))
                            .children(detail),
                    )
                }),
        )
    }
}
