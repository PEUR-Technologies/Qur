//! Saved chats, and long chats kept small: the chat is written to disk, and
//! only its most recent part stays in memory. Older rows wait behind "Show
//! earlier messages" at the top of the chat.
//!
//! ## On disk
//!
//! `<qur home>/projects/<project-id>.chats/<task-id>/` (`qur_agents::chat_dir`;
//! a reviewer's is `<task-id>~review`), deleted with the task:
//! - `earlier.jsonl`: the oldest rows, one JSON object per line, oldest
//!   first. Only ever appended to (and cut back after a crash, below).
//! - `recent.jsonl`: a header line `{"t":"meta","v":1,"earlier":N}`, then
//!   the rest of the chat, one row per line, oldest first. Rewritten whole
//!   (atomically) when a turn starts or ends, rows move to `earlier.jsonl`,
//!   or Qur quits. `N` is how many lines of `earlier.jsonl` come before
//!   it; extra lines there (a crash between the two writes) are dropped.
//! - `images/<id>.<ext>`: images in the chat (`<id>` is GPUI's hash of the
//!   bytes), referenced by rows as `"file": "images/…"`.
//!
//! The whole chat is `earlier.jsonl`'s first `N` lines, then `recent.jsonl`
//! past its header. Each row has a type `t` and the fields below; optional
//! ones are left out when empty:
//! - `{"t":"user"|"agent"|"thought"|"notice"|"output", "text"}`: your
//!   message, the agent's reply (Markdown), its thinking, a notice, command
//!   output.
//! - `{"t":"image", "file", "mime", "w", "h"}`: an image in a message.
//! - `{"t":"link", "uri", "name"}`: a file or web link in a message.
//! - `{"t":"files", "files": [{"image", "mime", "w"?, "h"?, "path"?} | {"file"}]}`:
//!   what you attached to the message above (`image` is a saved image as
//!   above; `path` and `file` are absolute paths of your files).
//! - `{"t":"failure", "title", "summary"?, "detail"?, "resend"?: {"text",
//!   "mentions"?, "context"?, "labels"?, "files"?}}`: a failed turn.
//! - `{"t":"turn", "n"}`: the end of turn `n` (its numbers are in the task's
//!   `turns` in `<project-id>.tasks.toml`).
//! - `{"t":"tool", "id", "title", "kind", "status", "locations"?: [{"path",
//!   "line"?}], "content"?: [{"text"} | {"diff", "old"?, "new"} | {"terminal"}],
//!   "output"?, "exit"?, "raw_input"?, "raw_output"?}`: a tool call. `kind`
//!   and `status` are ACP's words (`read`, `edit`, `execute`…; `completed`,
//!   `failed`…); `diff` is the file's path; `exit` is the exit code, or null
//!   when it ended without one. The raw input and output are kept only when
//!   the call has nothing else to show.

use std::io::Write as _;

use super::*;

/// Past this many rows (or bytes, roughly) in memory, the oldest move to
/// disk until `KEEP_ROWS` / `KEEP_BYTES` are left: spilled in batches, not
/// one row per row added. A real turn is about 10 rows and 50–100 KB (file
/// reads carry the file), so this keeps the last 20–30 turns at hand.
const SPILL_ROWS: usize = 300;
const KEEP_ROWS: usize = 200;
const SPILL_BYTES: usize = 4 << 20;
const KEEP_BYTES: usize = 2 << 20;
/// While you're scrolled up reading (or after Show earlier messages), rows
/// only move out past these.
const HARD_ROWS: usize = 4 * SPILL_ROWS;
const HARD_BYTES: usize = 4 * SPILL_BYTES;
/// The newest rows always stay.
const MIN_ROWS: usize = 40;
/// Rows one click on "Show earlier messages" brings back.
const LOAD_ROWS: usize = 100;

/// The task's chat on disk.
#[derive(Default)]
pub(super) struct History {
    /// The chat's folder; None: nothing is saved (yet).
    dir: Option<PathBuf>,
    /// Lines of `earlier.jsonl` (the chat's first rows).
    on_disk: usize,
    /// How many of those aren't in memory (the oldest): "Show earlier
    /// messages" is the chat's first row while there are any.
    pub(super) hidden: usize,
    /// Tool calls whose rows are hidden: late updates for them are dropped.
    tools: HashSet<ToolCallId>,
    /// The chat came back from disk: session/load's replay of the same
    /// conversation isn't shown again.
    pub(super) restored: bool,
    /// Dropping session/load's replay now.
    pub(super) replaying: bool,
}

/// Saved chats' folders under `dir` that no task in `ids` owns (left by a
/// task removed while Qur wasn't running, or by a crash).
pub fn stale_chats(dir: &Path, ids: &[String]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let owner = name.strip_suffix("~review").unwrap_or(&name);
            !ids.iter().any(|id| id == owner)
        })
        .collect()
}

impl AgentTask {
    /// The chat's first rows in memory: 1 when "Show earlier messages" is.
    pub(super) fn marker(&self) -> usize {
        usize::from(self.history.hidden > 0)
    }

    /// Where the chat is saved; it comes back from there when it was saved
    /// before (a restart): the recent part in memory, the rest on disk.
    pub fn restore_chat(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let (n, rows) = saved_rows(&dir);
        self.history = History { dir: Some(dir.clone()), on_disk: n, hidden: n, ..Default::default() };
        self.chat.clear();
        self.tools.clear();
        if n > 0 {
            self.chat.push(ChatItem::Earlier);
        }
        for line in &rows {
            let (item, tool) = row_from_line(line, &dir, true);
            if let Some(t) = tool {
                self.tools.insert(t.id.clone(), t);
            }
            self.chat.push(item);
        }
        self.list.reset(self.chat.len());
        self.history.restored = self.chat.len() > self.marker() || n > 0;
        if self.history.restored {
            println!("CHAT {} restored: {} rows in memory, {n} earlier", self.record.id, self.chat.len() - self.marker());
        }
        self.spill_check();
        self.chat_changed();
        cx.notify();
    }

    /// The chat's folder, once it's known.
    pub(super) fn history_dir(&self) -> Option<&Path> {
        self.history.dir.as_deref()
    }

    /// Discard, Merge or Close: the saved chat goes (the reviewer's too),
    /// and nothing more is written.
    pub fn delete_chat(&mut self, cx: &mut Context<Self>) {
        if let Some(dir) = self.history.dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
        if let Some(r) = &self.reviewer {
            r.update(cx, |r, cx| r.delete_chat(cx));
        }
    }

    /// An empty chat, on disk too (session/load is about to replay it).
    pub(super) fn clear_chat(&mut self) {
        self.chat.clear();
        self.tools.clear();
        self.toggled.clear();
        self.list.reset(0);
        if let Some(dir) = &self.history.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
        self.history = History { dir: self.history.dir.take(), ..Default::default() };
        self.chat_changed();
    }

    /// The session's history replayed by session/load isn't shown when the
    /// chat came back from disk (it's the same conversation).
    pub(super) fn begin_replay(&mut self) {
        self.history.replaying = self.history.restored && self.record.session_id.is_some();
    }

    /// The rows past `earlier.jsonl`, written to `recent.jsonl`.
    pub(super) fn save_chat(&self) {
        let Some(dir) = &self.history.dir else { return };
        let m = self.marker();
        // Rows brought back by "Show earlier messages" are on disk already.
        let skip = self.history.on_disk.saturating_sub(self.history.hidden);
        let rows = self.chat.iter().skip(m + skip);
        if self.chat.len() <= m && self.history.on_disk == 0 && !dir.exists() {
            return;
        }
        let header = serde_json::json!({ "t": "meta", "v": 1, "earlier": self.history.on_disk }).to_string();
        let lines: Vec<String> = std::iter::once(header).chain(rows.map(|c| row_json(c, &self.tools, dir).to_string())).collect();
        if let Err(e) = write_lines(&dir.join("recent.jsonl"), &lines, false) {
            eprintln!("CHAT {} save: {e}", self.record.id);
        }
    }

    /// A tool call that's still going (or waiting for you): its row stays.
    fn tool_live(&self, id: &ToolCallId) -> bool {
        let running = matches!(self.status, Status::Working | Status::NeedsYou | Status::Starting);
        (running && self.tools.get(id).is_some_and(|t| matches!(t.status, ToolStatus::Pending | ToolStatus::InProgress))) || self.permissions.iter().any(|p| p.tool.id == *id)
    }

    /// Too much in memory: move the oldest rows to disk.
    pub(super) fn spill_check(&mut self) {
        if self.history.dir.is_none() {
            return;
        }
        let m = self.marker();
        let n = self.chat.len() - m;
        if n <= MIN_ROWS {
            return;
        }
        let (rows, bytes) = if self.list.is_following_tail() { (SPILL_ROWS, SPILL_BYTES) } else { (HARD_ROWS, HARD_BYTES) };
        let sizes: Vec<usize> = self.chat[m..].iter().map(|c| row_bytes(c, &self.tools)).collect();
        if n <= rows && sizes.iter().sum::<usize>() <= bytes {
            return;
        }
        let k = spill_cut(&self.chat[m..], &sizes, |id| self.tool_live(id));
        if k > 0 {
            self.spill(k);
        }
    }

    /// Move the `k` oldest rows in memory to disk.
    fn spill(&mut self, k: usize) {
        let Some(dir) = self.history.dir.clone() else { return };
        let m = self.marker();
        // Rows brought back earlier are on disk already; the rest are appended.
        let new_from = self.history.on_disk.saturating_sub(self.history.hidden).min(k);
        let lines: Vec<String> = self.chat[m + new_from..m + k].iter().map(|c| row_json(c, &self.tools, &dir).to_string()).collect();
        if let Err(e) = write_lines(&dir.join("earlier.jsonl"), &lines, true) {
            eprintln!("CHAT {} spill: {e}", self.record.id);
            return;
        }
        let gone: Vec<ChatItem> = self.chat.drain(m..m + k).collect();
        for c in &gone {
            if let ChatItem::Tool(id) = c {
                self.history.tools.insert(id.clone());
                self.toggled.remove(id.as_str());
                self.toggled.remove(&format!("group-{id}"));
                // The running turn's calls stay until it ends (the files strip).
                if !self.turn_tools.contains(id) {
                    self.tools.remove(id);
                }
            }
        }
        self.history.on_disk += lines.len();
        self.history.hidden += k;
        if m == 0 {
            self.chat.insert(0, ChatItem::Earlier);
        }
        self.list.splice(0..m + k, 1);
        self.rows_replaced(0..m + k, 1);
        self.save_chat();
        println!("CHAT {} spilled {k} rows: {} in memory, {} earlier", self.record.id, self.chat.len() - 1, self.history.hidden);
    }

    /// The running turn's calls whose rows were hidden leave memory once
    /// the files strip is done with them.
    pub(super) fn drop_hidden_tools(&mut self) {
        if !self.history.tools.is_empty() {
            let hidden = &self.history.tools;
            self.tools.retain(|id, _| !hidden.contains(id));
        }
    }

    /// An update for a call whose row is on disk: dropped.
    pub(super) fn tool_hidden(&self, id: &ToolCallId) -> bool {
        self.history.tools.contains(id)
    }

    /// "Show earlier messages": the next older rows back from disk, above
    /// what's shown, which stays where it is.
    pub(super) fn show_earlier(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.history.dir.clone() else { return };
        let hidden = self.history.hidden;
        let lines = read_lines(&dir.join("earlier.jsonl"));
        let mut from = hidden.saturating_sub(LOAD_ROWS);
        if lines.len() >= hidden {
            // Not halfway through a group of tool calls.
            let is_tool = |i: usize| serde_json::from_str::<serde_json::Value>(&lines[i]).is_ok_and(|v| v["t"] == "tool");
            while from > 0 && is_tool(from - 1) && is_tool(from) {
                from -= 1;
            }
        }
        let rows: Vec<(ChatItem, Option<ToolCall>)> = if lines.len() >= hidden {
            lines[from..hidden].iter().map(|l| row_from_line(l, &dir, true)).collect()
        } else {
            // The file went missing or was cut short.
            from = 0;
            vec![(ChatItem::Notice("Earlier messages couldn't be read.".into()), None)]
        };
        let k = rows.len();
        let mut items = Vec::with_capacity(k);
        for (item, tool) in rows {
            if let Some(t) = tool {
                self.history.tools.remove(&t.id);
                self.tools.insert(t.id.clone(), t);
            }
            items.push(item);
        }
        self.history.hidden = from;
        let keep = usize::from(from > 0);
        self.chat.splice(0..1, (keep == 1).then_some(ChatItem::Earlier).into_iter().chain(items)).for_each(drop);
        self.list.splice(0..1, keep + k);
        self.rows_replaced(0..1, keep + k);
        // The row that was first stays in view; the new ones are above it.
        self.list.scroll_to(gpui::ListOffset { item_ix: keep + k, offset_in_item: px(0.) });
        self.chat_changed();
        println!("CHAT {} showed {k} earlier rows: {} in memory, {from} earlier", self.record.id, self.chat.len() - keep);
        cx.notify();
    }

    /// Rows `range` were replaced by `count` others: what refers to rows by
    /// number (folded thoughts, Find's matches) moves with them.
    fn rows_replaced(&mut self, range: Range<usize>, count: usize) {
        let moved = |row: usize| moved_row(&range, count, row);
        self.toggled = std::mem::take(&mut self.toggled)
            .into_iter()
            .filter_map(|k| match k.strip_prefix("thought-").and_then(|n| n.parse::<usize>().ok()) {
                Some(row) => moved(row).map(|r| format!("thought-{r}")),
                None => Some(k),
            })
            .collect();
        if let Some(f) = &mut self.find {
            let current = f.current.and_then(|c| f.matches.get(c).copied()).and_then(|(r, n)| Some((moved(r)?, n)));
            f.matches = f.matches.iter().filter_map(|&(r, n)| Some((moved(r)?, n))).collect();
            f.current = current.and_then(|c| f.matches.iter().position(|m| *m == c));
            f.dirty = true;
        }
    }

    /// The whole chat, the part on disk too (Copy Chat as Markdown, a
    /// reviewer's thread): rows and their tool calls. Images aren't read.
    pub(super) fn full_chat(&self) -> (Vec<ChatItem>, HashMap<ToolCallId, ToolCall>) {
        let mut tools = self.tools.clone();
        let mut chat = Vec::new();
        if let Some(dir) = self.history.dir.as_ref().filter(|_| self.history.hidden > 0) {
            for line in read_lines(&dir.join("earlier.jsonl")).iter().take(self.history.hidden) {
                let (item, tool) = row_from_line(line, dir, false);
                if let Some(t) = tool {
                    tools.insert(t.id.clone(), t);
                }
                chat.push(item);
            }
        }
        chat.extend(self.chat[self.marker()..].iter().cloned());
        (chat, tools)
    }

    /// Dev/measuring: QUR_DEBUG_CHAT=<turns> fills the chat with that many
    /// synthetic turns when the agent starts (no agent usage).
    pub(super) fn debug_fill(&mut self, cx: &mut Context<Self>) {
        let Some(turns) = std::env::var("QUR_DEBUG_CHAT").ok().and_then(|v| v.parse::<usize>().ok()) else { return };
        for n in 1..=turns {
            for (item, tool) in synthetic_turn(n) {
                if let Some(t) = tool {
                    self.tools.insert(t.id.clone(), t);
                }
                self.push(item);
            }
        }
        self.save_chat();
        println!("CHAT {} filled: {} rows in memory, {} tool calls, {} earlier", self.record.id, self.chat.len() - self.marker(), self.tools.len(), self.history.hidden);
        cx.notify();
    }
}

/// How many of the oldest rows (`rows`, sized `sizes`) to move to disk so
/// that at most `KEEP_ROWS` rows and about `KEEP_BYTES` stay, never the
/// newest `MIN_ROWS`, a live tool call or anything after it, and not
/// splitting a group of tool calls (unless that would move nothing).
fn spill_cut(rows: &[ChatItem], sizes: &[usize], live: impl Fn(&ToolCallId) -> bool) -> usize {
    let n = rows.len();
    let max = n.saturating_sub(MIN_ROWS);
    let mut rest: usize = sizes.iter().sum();
    let mut k = 0;
    while k < max && (n - k > KEEP_ROWS || rest > KEEP_BYTES) {
        rest -= sizes[k];
        k += 1;
    }
    if let Some(first) = rows[..k].iter().position(|c| matches!(c, ChatItem::Tool(id) if live(id))) {
        k = first;
    }
    let is_tool = |i: usize| matches!(rows[i], ChatItem::Tool(_));
    let mut cut = k;
    while cut > 0 && cut < n && is_tool(cut - 1) && is_tool(cut) {
        cut -= 1;
    }
    if cut == 0 { k } else { cut }
}

/// Roughly what a row holds in memory, in bytes.
fn row_bytes(c: &ChatItem, tools: &HashMap<ToolCallId, ToolCall>) -> usize {
    64 + match c {
        ChatItem::User(t) | ChatItem::Agent(t) | ChatItem::Thought(t) | ChatItem::Notice(t) | ChatItem::Output(t) => t.len(),
        ChatItem::Image(img, _) => img.bytes().len(),
        ChatItem::Link { uri, name } => uri.len() + name.len(),
        ChatItem::Files(files) => files
            .iter()
            .map(|f| match f {
                Sent::Image(img, ..) => img.bytes().len(),
                Sent::File(p) => p.as_os_str().len(),
            })
            .sum(),
        ChatItem::Failure(f) => f.title.len() + f.summary.as_ref().map_or(0, String::len) + f.detail.as_ref().map_or(0, String::len),
        ChatItem::Tool(id) => tools.get(id).map_or(0, tool_bytes),
        ChatItem::Turn(_) | ChatItem::Earlier => 0,
    }
}

fn tool_bytes(t: &ToolCall) -> usize {
    let content: usize = t
        .content
        .iter()
        .map(|c| match c {
            ToolContent::Text(s) | ToolContent::Terminal(s) => s.len(),
            ToolContent::Diff { path, old_text, new_text } => path.as_os_str().len() + old_text.as_ref().map_or(0, String::len) + new_text.len(),
        })
        .sum();
    let raw = [&t.raw_input, &t.raw_output].into_iter().flatten().map(value_bytes).sum::<usize>();
    t.title.len() + 64 * t.locations.len() + content + t.output.as_ref().map_or(0, String::len) + raw
}

fn value_bytes(v: &serde_json::Value) -> usize {
    use serde_json::Value as V;
    match v {
        V::String(s) => s.len() + 24,
        V::Array(a) => 24 + a.iter().map(value_bytes).sum::<usize>(),
        V::Object(o) => 48 + o.iter().map(|(k, v)| k.len() + 24 + value_bytes(v)).sum::<usize>(),
        _ => 16,
    }
}

/// Whether a tool call shows its raw input and output: only when it has
/// nothing else (`tool_body`).
pub(super) fn raw_shown(t: &ToolCall) -> bool {
    !t.content.iter().any(|c| match c {
        ToolContent::Text(s) => !s.trim().is_empty(),
        ToolContent::Diff { .. } | ToolContent::Terminal(_) => true,
    })
}

// -- rows as JSON ----------------------------------------------------------------

fn kind_name(k: ToolKind) -> &'static str {
    match k {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "switch_mode",
        ToolKind::Other => "other",
    }
}

fn kind_from(s: &str) -> ToolKind {
    [ToolKind::Read, ToolKind::Edit, ToolKind::Delete, ToolKind::Move, ToolKind::Search, ToolKind::Execute, ToolKind::Think, ToolKind::Fetch, ToolKind::SwitchMode]
        .into_iter()
        .find(|k| kind_name(*k) == s)
        .unwrap_or(ToolKind::Other)
}

fn status_name(s: ToolStatus) -> &'static str {
    match s {
        ToolStatus::Pending => "pending",
        ToolStatus::InProgress => "in_progress",
        ToolStatus::Completed => "completed",
        ToolStatus::Failed => "failed",
    }
}

fn status_from(s: &str) -> ToolStatus {
    [ToolStatus::InProgress, ToolStatus::Completed, ToolStatus::Failed].into_iter().find(|k| status_name(*k) == s).unwrap_or(ToolStatus::Pending)
}

/// An image saved in the chat's folder (once per image): its path there.
fn save_image(img: &gpui::Image, dir: &Path) -> String {
    let rel = format!("images/{:016x}.{}", img.id(), img.format().extension());
    let path = dir.join(&rel);
    if !path.exists() {
        let _ = std::fs::create_dir_all(dir.join("images"));
        if let Err(e) = std::fs::write(&path, img.bytes()) {
            eprintln!("CHAT image {}: {e}", path.display());
        }
    }
    rel
}

/// A saved image back (`read`: its bytes too; otherwise an empty stand-in
/// that only says what it was, for Markdown and review threads).
fn load_image(v: &serde_json::Value, key: &str, dir: &Path, read: bool) -> Option<Arc<gpui::Image>> {
    let format = gpui::ImageFormat::from_mime_type(v["mime"].as_str()?)?;
    let bytes = if read { std::fs::read(dir.join(v[key].as_str()?)).ok()? } else { Vec::new() };
    Some(Arc::new(gpui::Image::from_bytes(format, bytes)))
}

fn opt_str(o: &mut serde_json::Map<String, serde_json::Value>, key: &str, v: Option<&str>) {
    if let Some(v) = v {
        o.insert(key.into(), v.into());
    }
}

fn paths_json(paths: &[PathBuf]) -> serde_json::Value {
    paths.iter().map(|p| serde_json::Value::from(p.to_string_lossy())).collect()
}

fn strings_from(v: &serde_json::Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn paths_from(v: &serde_json::Value) -> Vec<PathBuf> {
    strings_from(v).into_iter().map(PathBuf::from).collect()
}

/// A chat row as saved (see the module's notes); images go to `dir`.
fn row_json(c: &ChatItem, tools: &HashMap<ToolCallId, ToolCall>, dir: &Path) -> serde_json::Value {
    use serde_json::{Map, Value, json};
    match c {
        ChatItem::User(t) => json!({ "t": "user", "text": t }),
        ChatItem::Agent(t) => json!({ "t": "agent", "text": t }),
        ChatItem::Thought(t) => json!({ "t": "thought", "text": t }),
        ChatItem::Notice(t) => json!({ "t": "notice", "text": t }),
        ChatItem::Output(t) => json!({ "t": "output", "text": t }),
        // Never saved (it's `hidden`), but harmless.
        ChatItem::Earlier => json!({ "t": "notice", "text": "" }),
        ChatItem::Image(img, (w, h)) => json!({ "t": "image", "file": save_image(img, dir), "mime": img.format().mime_type(), "w": w, "h": h }),
        ChatItem::Link { uri, name } => json!({ "t": "link", "uri": uri, "name": name }),
        ChatItem::Files(files) => {
            let files: Vec<Value> = files
                .iter()
                .map(|f| match f {
                    Sent::Image(img, size, path) => {
                        let mut o = Map::new();
                        o.insert("image".into(), save_image(img, dir).into());
                        o.insert("mime".into(), img.format().mime_type().into());
                        if let Some((w, h)) = size {
                            o.insert("w".into(), (*w).into());
                            o.insert("h".into(), (*h).into());
                        }
                        opt_str(&mut o, "path", path.as_ref().map(|p| p.to_string_lossy()).as_deref());
                        Value::Object(o)
                    }
                    Sent::File(p) => json!({ "file": p.to_string_lossy() }),
                })
                .collect();
            json!({ "t": "files", "files": files })
        }
        ChatItem::Failure(f) => {
            let mut o = Map::new();
            o.insert("t".into(), "failure".into());
            o.insert("title".into(), f.title.clone().into());
            opt_str(&mut o, "summary", f.summary.as_deref());
            opt_str(&mut o, "detail", f.detail.as_deref());
            if let Some(r) = &f.resend {
                let mut s = Map::new();
                s.insert("text".into(), r.text.clone().into());
                for (k, v) in [("mentions", paths_json(&r.mentions)), ("context", r.context.clone().into()), ("labels", r.labels.clone().into()), ("files", paths_json(&r.files))] {
                    if v.as_array().is_some_and(|a| !a.is_empty()) {
                        s.insert(k.into(), v);
                    }
                }
                o.insert("resend".into(), Value::Object(s));
            }
            Value::Object(o)
        }
        ChatItem::Turn(n) => json!({ "t": "turn", "n": n }),
        ChatItem::Tool(id) => {
            let Some(t) = tools.get(id) else { return json!({ "t": "tool", "id": id.as_str(), "title": "", "kind": "other", "status": "completed" }) };
            let mut o = Map::new();
            o.insert("t".into(), "tool".into());
            o.insert("id".into(), id.as_str().into());
            o.insert("title".into(), t.title.clone().into());
            o.insert("kind".into(), kind_name(t.kind).into());
            o.insert("status".into(), status_name(t.status).into());
            if !t.locations.is_empty() {
                let locs: Vec<Value> = t
                    .locations
                    .iter()
                    .map(|l| match l.line {
                        Some(n) => json!({ "path": l.path.to_string_lossy(), "line": n }),
                        None => json!({ "path": l.path.to_string_lossy() }),
                    })
                    .collect();
                o.insert("locations".into(), locs.into());
            }
            if !t.content.is_empty() {
                let content: Vec<Value> = t
                    .content
                    .iter()
                    .map(|c| match c {
                        ToolContent::Text(s) => json!({ "text": s }),
                        ToolContent::Diff { path, old_text: Some(old), new_text } => json!({ "diff": path.to_string_lossy(), "old": old, "new": new_text }),
                        ToolContent::Diff { path, old_text: None, new_text } => json!({ "diff": path.to_string_lossy(), "new": new_text }),
                        ToolContent::Terminal(id) => json!({ "terminal": id }),
                    })
                    .collect();
                o.insert("content".into(), content.into());
            }
            opt_str(&mut o, "output", t.output.as_deref());
            if let Some(exit) = t.exit {
                o.insert("exit".into(), exit.into());
            }
            if raw_shown(t) {
                for (k, v) in [("raw_input", &t.raw_input), ("raw_output", &t.raw_output)] {
                    if let Some(v) = v {
                        o.insert(k.into(), v.clone());
                    }
                }
            }
            Value::Object(o)
        }
    }
}

/// A saved row back (and its tool call). A line that can't be read shows
/// as a notice, so rows keep their places.
fn row_from_line(line: &str, dir: &Path, read_images: bool) -> (ChatItem, Option<ToolCall>) {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| row_from_json(&v, dir, read_images))
        .unwrap_or_else(|| (ChatItem::Notice("[a message that couldn't be read]".into()), None))
}

fn row_from_json(v: &serde_json::Value, dir: &Path, read_images: bool) -> Option<(ChatItem, Option<ToolCall>)> {
    let text = || v["text"].as_str().unwrap_or("").to_string();
    let s = |key: &str| v[key].as_str().map(str::to_string);
    let item = match v["t"].as_str()? {
        "user" => ChatItem::User(text()),
        "agent" => ChatItem::Agent(text()),
        "thought" => ChatItem::Thought(text()),
        "notice" => ChatItem::Notice(text()),
        "output" => ChatItem::Output(text()),
        "image" => {
            let size = (v["w"].as_u64()? as u32, v["h"].as_u64()? as u32);
            match load_image(v, "file", dir, read_images) {
                Some(img) => ChatItem::Image(img, size),
                None => ChatItem::Notice(format!("[image {}×{} no longer saved]", size.0, size.1)),
            }
        }
        "link" => ChatItem::Link { uri: s("uri")?, name: s("name").unwrap_or_default() },
        "files" => {
            let files: Vec<Sent> = v["files"]
                .as_array()?
                .iter()
                .filter_map(|f| {
                    let path = f["path"].as_str().map(PathBuf::from);
                    if f.get("image").is_some() {
                        let size = f["w"].as_u64().zip(f["h"].as_u64()).map(|(w, h)| (w as u32, h as u32));
                        return match load_image(f, "image", dir, read_images) {
                            Some(img) => Some(Sent::Image(img, size, path)),
                            None => path.map(Sent::File),
                        };
                    }
                    f["file"].as_str().map(|p| Sent::File(p.into()))
                })
                .collect();
            ChatItem::Files(files)
        }
        "failure" => {
            let r = &v["resend"];
            let resend = r["text"].as_str().map(|t| Outgoing {
                text: t.to_string(),
                mentions: paths_from(&r["mentions"]),
                context: strings_from(&r["context"]),
                labels: strings_from(&r["labels"]),
                images: Vec::new(),
                files: paths_from(&r["files"]),
            });
            ChatItem::Failure(Failure { title: s("title").unwrap_or_default(), summary: s("summary"), detail: s("detail"), resend })
        }
        "turn" => ChatItem::Turn(v["n"].as_u64()? as u32),
        "tool" => {
            let id: ToolCallId = s("id")?.into();
            let locations = v["locations"]
                .as_array()
                .map(|a| a.iter().filter_map(|l| Some(qur_acp::Location { path: l["path"].as_str()?.into(), line: l["line"].as_u64().map(|n| n as u32) })).collect())
                .unwrap_or_default();
            let content = v["content"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|c| {
                            if let Some(t) = c["text"].as_str() {
                                return Some(ToolContent::Text(t.into()));
                            }
                            if let Some(p) = c["diff"].as_str() {
                                return Some(ToolContent::Diff { path: p.into(), old_text: c["old"].as_str().map(str::to_string), new_text: c["new"].as_str().unwrap_or("").into() });
                            }
                            c["terminal"].as_str().map(|t| ToolContent::Terminal(t.into()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let exit = v.get("exit").map(|e| e.as_i64());
            let t = ToolCall {
                id: id.clone(),
                title: s("title").unwrap_or_default(),
                kind: kind_from(v["kind"].as_str().unwrap_or("")),
                status: status_from(v["status"].as_str().unwrap_or("")),
                locations,
                content,
                raw_input: v.get("raw_input").cloned(),
                raw_output: v.get("raw_output").cloned(),
                output: s("output"),
                exit,
            };
            return Some((ChatItem::Tool(id), Some(t)));
        }
        _ => return None,
    };
    Some((item, None))
}

/// Where row `row` went when rows `range` were replaced by `count` others
/// (None: it was one of them).
fn moved_row(range: &Range<usize>, count: usize, row: usize) -> Option<usize> {
    match row {
        r if r < range.start => Some(r),
        r if r < range.end => None,
        r => Some(r - range.len() + count),
    }
}

// -- files -------------------------------------------------------------------------

/// A saved chat: how many rows `earlier.jsonl` has, and the lines of the
/// rest (`recent.jsonl` past its header). Lines in `earlier.jsonl` past the
/// header's count (Qur stopped between the two writes) are cut off: they're
/// in `recent.jsonl` too.
fn saved_rows(dir: &Path) -> (usize, Vec<String>) {
    let earlier = read_lines(&dir.join("earlier.jsonl"));
    let mut recent = read_lines(&dir.join("recent.jsonl"));
    let header = recent.first().and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter(|v| v["t"] == "meta");
    if header.is_some() {
        recent.remove(0);
    }
    let n = header.and_then(|h| h["earlier"].as_u64()).map_or(earlier.len(), |n| (n as usize).min(earlier.len()));
    if n < earlier.len()
        && let Err(e) = write_lines(&dir.join("earlier.jsonl"), &earlier[..n], false)
    {
        eprintln!("CHAT {}: {e}", dir.display());
    }
    (n, recent)
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path).map(|s| s.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()).unwrap_or_default()
}

/// Lines to a file: appended, or the whole file replaced (atomically).
fn write_lines(path: &Path, lines: &[String], append: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut body = String::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
    for l in lines {
        body.push_str(l);
        body.push('\n');
    }
    if append {
        return std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(body.as_bytes());
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, body)?;
    std::fs::rename(tmp, path)
}

/// One synthetic turn shaped like a real Claude session's (measuring
/// memory, tests): a message, a thought, a reply, three file reads (their
/// text twice, as Claude sends it: content and raw output), an edit with
/// its diff, a command with output, and a closing line. About 90 KB.
pub(super) fn synthetic_turn(n: usize) -> Vec<(ChatItem, Option<ToolCall>)> {
    let file: String = (1..=300).map(|i| format!("{i}\tlet value_{i} = compute(input, {i}); // turn {n}\n")).collect();
    let read = |k: usize| {
        let id: ToolCallId = format!("toolu_{n}_read{k}").into();
        let t = ToolCall {
            id: id.clone(),
            title: format!("Read src/module_{k}.rs"),
            kind: ToolKind::Read,
            status: ToolStatus::Completed,
            locations: vec![qur_acp::Location { path: format!("/w/src/module_{k}.rs").into(), line: Some(1) }],
            content: vec![ToolContent::Text(format!("```\n{file}```"))],
            raw_input: Some(serde_json::json!({ "file_path": format!("/w/src/module_{k}.rs") })),
            raw_output: Some(serde_json::Value::String(file.clone())),
            ..Default::default()
        };
        (ChatItem::Tool(id), Some(t))
    };
    let old: String = (0..40).map(|i| format!("    let a{i} = old_value({i});\n")).collect();
    let new: String = (0..50).map(|i| format!("    let a{i} = new_value({i}, turn_{n});\n")).collect();
    let edit_id: ToolCallId = format!("toolu_{n}_edit").into();
    let edit = ToolCall {
        id: edit_id.clone(),
        title: "Edit src/module_1.rs".into(),
        kind: ToolKind::Edit,
        status: ToolStatus::Completed,
        locations: vec![qur_acp::Location { path: "/w/src/module_1.rs".into(), line: None }],
        content: vec![ToolContent::Diff { path: "/w/src/module_1.rs".into(), old_text: Some(old.clone()), new_text: new.clone() }],
        raw_input: Some(serde_json::json!({ "file_path": "/w/src/module_1.rs", "old_string": old, "new_string": new })),
        ..Default::default()
    };
    let out: String = (0..80).map(|i| format!("   Compiling crate_{i} v0.1.{n} (/w/crates/crate_{i})\n")).collect();
    let run_id: ToolCallId = format!("toolu_{n}_run").into();
    let run = ToolCall {
        id: run_id.clone(),
        title: "cargo build --release".into(),
        kind: ToolKind::Execute,
        status: ToolStatus::Completed,
        content: vec![ToolContent::Terminal(format!("term_{n}"))],
        raw_output: Some(serde_json::Value::String(out.clone())),
        output: Some(out),
        exit: Some(Some(0)),
        ..Default::default()
    };
    let reply = format!(
        "Turn {n}: I looked at the modules and the build. The problem is in `module_1.rs`, where each value is computed from the old input.\n\n\
         Here's the change:\n\n```rust\nfn compute(input: &Input, k: usize) -> Value {{\n    input.values[k].scale(2.0)\n}}\n```\n\n\
         - Reads the three modules\n- Edits the computation\n- Builds in release mode\n\n{}",
        "It keeps the public API the same and only changes how the values are derived. ".repeat(8)
    );
    vec![
        (ChatItem::User(format!("Turn {n}: please fix the computation in module 1 so the values come from the new input, and build it. {}", "Keep the API. ".repeat(10))), None),
        (ChatItem::Thought(format!("The user wants turn {n} fixed. {}", "I should read the modules first, then edit, then build. ".repeat(14))), None),
        (ChatItem::Agent(reply), None),
        read(1),
        read(2),
        read(3),
        (ChatItem::Tool(edit_id), Some(edit)),
        (ChatItem::Tool(run_id), Some(run)),
        (ChatItem::Agent(format!("Done with turn {n}: the build passes. {}", "The values now come from the new input. ".repeat(12))), None),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qur-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A chat's rows (and tool calls) as saved, for comparing.
    fn saved(rows: &[(ChatItem, Option<ToolCall>)], dir: &Path) -> Vec<serde_json::Value> {
        let tools: HashMap<ToolCallId, ToolCall> = rows.iter().filter_map(|(_, t)| t.clone()).map(|t| (t.id.clone(), t)).collect();
        rows.iter().map(|(c, _)| row_json(c, &tools, dir)).collect()
    }

    #[test]
    fn rows_round_trip() {
        let dir = scratch("round");
        let png = {
            let mut bytes = Vec::new();
            image::RgbaImage::new(3, 2).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
            Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes))
        };
        let mut rows = synthetic_turn(7);
        let mut failed = ToolCall { id: "t-raw".into(), title: "mcp thing".into(), kind: ToolKind::Other, status: ToolStatus::Failed, exit: Some(None), ..Default::default() };
        failed.raw_input = Some(serde_json::json!({ "q": 1 }));
        rows.extend([
            (ChatItem::Tool(failed.id.clone()), Some(failed)),
            (ChatItem::Notice("Stopped.".into()), None),
            (ChatItem::Output("$ make\nok".into()), None),
            (ChatItem::Image(png.clone(), (3, 2)), None),
            (ChatItem::Link { uri: "file:///w/a.rs#L3".into(), name: "a.rs".into() }, None),
            (ChatItem::Files(vec![Sent::Image(png.clone(), Some((3, 2)), Some("/w/shot.png".into())), Sent::Image(png, None, None), Sent::File("/w/notes.pdf".into())]), None),
            (
                ChatItem::Failure(Failure {
                    title: "Claude couldn't finish.".into(),
                    summary: Some("Rate limited".into()),
                    detail: None,
                    resend: Some(Outgoing { files: vec!["/w/x.txt".into()], ..Outgoing::plain("Fix it".into(), vec!["/w/a.rs".into()]) }),
                }),
                None,
            ),
            (ChatItem::Turn(7), None),
        ]);
        let before = saved(&rows, &dir);
        let back: Vec<(ChatItem, Option<ToolCall>)> = before.iter().map(|v| row_from_line(&v.to_string(), &dir, true)).collect();
        assert_eq!(saved(&back, &dir), before, "every row reads back as it was saved");
        // The tool calls come back whole (the read's raw output, shown
        // nowhere, isn't kept; a call with nothing else keeps its raw input).
        let read = back.iter().find_map(|(_, t)| t.as_ref().filter(|t| t.kind == ToolKind::Read)).unwrap();
        assert!(read.content.len() == 1 && read.raw_output.is_none() && read.locations[0].line == Some(1));
        let raw = back.iter().find_map(|(_, t)| t.as_ref().filter(|t| t.id.as_str() == "t-raw")).unwrap();
        assert_eq!((raw.raw_input.clone(), raw.exit, raw.status), (Some(serde_json::json!({ "q": 1 })), Some(None), ToolStatus::Failed));
        assert!(matches!(&back[back.len() - 5].0, ChatItem::Image(img, (3, 2)) if !img.bytes().is_empty()));
        // One image file however often it's in the chat.
        assert_eq!(std::fs::read_dir(dir.join("images")).unwrap().count(), 1);
        // Without reading images (Copy Chat as Markdown) they're stand-ins.
        assert!(matches!(row_from_line(&before[before.len() - 5].to_string(), &dir, false).0, ChatItem::Image(img, (3, 2)) if img.bytes().is_empty()));
        // A missing image file: a notice, or the file it came from.
        std::fs::remove_dir_all(dir.join("images")).unwrap();
        assert!(matches!(row_from_line(&before[before.len() - 5].to_string(), &dir, true).0, ChatItem::Notice(_)));
        match row_from_line(&before[before.len() - 3].to_string(), &dir, true).0 {
            ChatItem::Files(f) => assert!(matches!(f.as_slice(), [Sent::File(p), Sent::File(q)] if p.ends_with("shot.png") && q.ends_with("notes.pdf"))),
            _ => panic!("files row"),
        }
        // A broken line keeps its place.
        assert!(matches!(row_from_line("{not json", &dir, true).0, ChatItem::Notice(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sizes(rows: &[ChatItem]) -> Vec<usize> {
        rows.iter().map(|_| 1000).collect()
    }

    #[test]
    fn spill_cut_keeps_the_newest() {
        let msg = |i: usize| ChatItem::Agent(format!("m{i}"));
        let rows: Vec<ChatItem> = (0..350).map(msg).collect();
        assert_eq!(spill_cut(&rows, &sizes(&rows), |_| false), 150, "down to KEEP_ROWS");
        // Big rows: down to KEEP_BYTES, but never the newest MIN_ROWS.
        let big = vec![300_000; 50];
        assert_eq!(spill_cut(&rows[..50], &big, |_| false), 10);
        // A live tool call and what follows it stay.
        let mut rows = rows;
        rows[100] = ChatItem::Tool("live".into());
        assert_eq!(spill_cut(&rows, &sizes(&rows), |id| id.as_str() == "live"), 100);
        // A group of tool calls isn't split...
        for (i, r) in rows.iter_mut().enumerate().skip(140).take(20) {
            *r = ChatItem::Tool(format!("t{i}").into());
        }
        assert_eq!(spill_cut(&rows, &sizes(&rows), |_| false), 140);
        // ...unless it's all there is to move.
        let tools: Vec<ChatItem> = (0..350).map(|i| ChatItem::Tool(format!("t{i}").into())).collect();
        assert_eq!(spill_cut(&tools, &sizes(&tools), |_| false), 150);
    }

    #[test]
    fn saved_chat_after_a_crash() {
        let dir = scratch("crash");
        let line = |s: &str| serde_json::json!({ "t": "user", "text": s }).to_string();
        write_lines(&dir.join("earlier.jsonl"), &[line("a"), line("b")], true).unwrap();
        write_lines(&dir.join("earlier.jsonl"), &[line("c")], true).unwrap();
        let header = serde_json::json!({ "t": "meta", "v": 1, "earlier": 2 }).to_string();
        write_lines(&dir.join("recent.jsonl"), &[header, line("c"), line("d")], false).unwrap();
        // Qur stopped after moving "c" to earlier.jsonl, before rewriting
        // recent.jsonl: "c" is in both. recent.jsonl wins.
        let (n, rows) = saved_rows(&dir);
        assert_eq!((n, rows), (2, vec![line("c"), line("d")]));
        assert_eq!(read_lines(&dir.join("earlier.jsonl")), [line("a"), line("b")], "earlier.jsonl cut back");
        // No recent.jsonl: everything is earlier.
        std::fs::remove_file(dir.join("recent.jsonl")).unwrap();
        assert_eq!(saved_rows(&dir), (2, Vec::new()));
        // Nothing saved.
        assert_eq!(saved_rows(&scratch("none")), (0, Vec::new()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rows_move_with_a_replaced_range() {
        // Rows 0..5 became one ("Show earlier messages").
        assert_eq!(moved_row(&(0..5), 1, 3), None);
        assert_eq!(moved_row(&(0..5), 1, 5), Some(1));
        assert_eq!(moved_row(&(0..5), 1, 9), Some(5));
        // The marker became itself and 100 older rows.
        assert_eq!(moved_row(&(0..1), 101, 1), Some(101));
        assert_eq!(moved_row(&(0..1), 101, 0), None);
    }

    #[test]
    fn stale_chat_folders() {
        let dir = scratch("stale");
        for d in ["keep", "keep~review", "gone", "gone~review"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        let mut stale: Vec<String> = stale_chats(&dir, &["keep".into()]).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        stale.sort();
        assert_eq!(stale, ["gone", "gone~review"]);
        assert!(stale_chats(&dir.join("missing"), &[]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
