//! Qur's editor model: text buffer, selections, undo, edit commands, find,
//! soft wrap. No UI and no GPUI here; `qur_app` renders it and feeds input.
//!
//! CONTRACT (M1): the public API below is used by `qur_app`; keep the
//! signatures. Implementation details are free. Positions are char-based
//! [`Point`]s for the UI; [`Edit`] carries byte offsets for tree-sitter.

use std::ops::Range;
use std::path::{Path, PathBuf};

pub use ropey::Rope;

/// (line, column in chars). Columns count Unicode scalar values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub line: usize,
    pub col: usize,
}

/// One cursor: `head` moves, `anchor` stays. Empty when equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Point,
    pub head: Point,
    /// Column the cursor wants on vertical moves (visual col of wrap row).
    pub goal: Option<usize>,
}

impl Selection {
    pub fn caret(p: Point) -> Self {
        Selection { anchor: p, head: p, goal: None }
    }
    pub fn start(&self) -> Point {
        self.anchor.min(self.head)
    }
    pub fn end(&self) -> Point {
        self.anchor.max(self.head)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// A text change in byte terms (mirrors tree-sitter's InputEdit; points are
/// (row, byte col)). Emitted for every change, including undo/redo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: (usize, usize),
    pub old_end_point: (usize, usize),
    pub new_end_point: (usize, usize),
}

/// Every editing command the keymap can trigger (docs/keymap.md, Editing,
/// Find, Selection). The app maps actions to these 1:1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    // Typing
    Insert(String),
    Newline,
    Tab,
    Backtab,
    Backspace,
    Delete,
    DeleteWordLeft,
    DeleteWordRight,
    DeleteToLineStart,
    DeleteLine,
    // Movement (`select` extends the selection)
    Left { select: bool },
    Right { select: bool },
    Up { select: bool },
    Down { select: bool },
    WordLeft { select: bool },
    WordRight { select: bool },
    LineStart { select: bool }, // smart home: first non-blank, then column 0
    LineEnd { select: bool },
    DocStart { select: bool },
    DocEnd { select: bool },
    PageUp { select: bool },
    PageDown { select: bool },
    GoToLine(usize), // 1-based
    // Line operations
    MoveLineUp,
    MoveLineDown,
    CopyLineUp,
    CopyLineDown,
    InsertLineBelow,
    InsertLineAbove,
    Indent,
    Outdent,
    ToggleLineComment,
    ToggleBlockComment,
    TrimTrailingWhitespace,
    // Selection
    SelectAll,
    SelectLine,
    /// Collapse to the primary cursor (Esc).
    SingleCursor,
    AddNextMatch,
    SelectAllMatches,
    AddCursorAbove,
    AddCursorBelow,
    JumpToMatchingBracket,
    /// Grow every selection to the enclosing syntax node (`Ctx::expand`),
    /// else word, line, file. Remembers each step for `ShrinkSelection`.
    ExpandSelection,
    /// Undo the last `ExpandSelection` step (only right after expanding).
    ShrinkSelection,
    // Clipboard: the app reads/writes the system clipboard via `Ctx`.
    Cut,
    Copy,
    Paste,
    // History
    Undo,
    Redo,
}

/// Per-command context supplied by the UI.
pub struct Ctx<'a> {
    /// Visible text rows (for PageUp/PageDown).
    pub page_rows: usize,
    /// Text to paste (system clipboard), for `Paste`.
    pub clipboard_in: Option<&'a str>,
    /// Set by Cut/Copy: (text, whole_line) for the system clipboard.
    pub clipboard_out: Option<(String, bool)>,
    /// Comment syntax and indent for the buffer's language.
    pub line_comment: Option<&'a str>,
    pub block_comment: Option<(&'a str, &'a str)>,
    pub indent_unit: &'a str,
    /// Matching bracket lookup (from qur_syntax), byte offset -> byte range.
    pub matching_bracket: Option<&'a dyn Fn(usize) -> Option<Range<usize>>>,
    /// Syntax node strictly enclosing a byte range (from qur_syntax), for
    /// Expand Selection. `None` falls back to word, line, file.
    pub expand: Option<&'a dyn Fn(Range<usize>) -> Option<Range<usize>>>,
}

/// What a command did, so the UI knows what to refresh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub text_changed: bool,
    pub selections_changed: bool,
    /// Scroll the primary cursor into view.
    pub reveal: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FindOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

mod commands;
pub mod encoding;
mod find;
#[cfg(test)]
mod tests;
mod history;
pub mod large;
mod text;
pub mod vim;
mod wrap;

use std::time::Instant;

use find::FindState;
use encoding::Encoding;
use history::{Applied, History, Kind, Meta};
use text::{byte_point, line_len, line_string};

/// The editor model for one open file.
pub struct Editor {
    rope: Rope,
    path: Option<PathBuf>,
    crlf: bool,
    /// How the file is stored (restored on save), and whether it starts with a BOM.
    encoding: Encoding,
    bom: bool,
    /// Chosen with Reopen with Encoding: reloads keep it instead of detecting.
    forced: Option<(Encoding, bool)>,
    /// Saving wouldn't give back the bytes that were read (bytes that didn't
    /// decode, or a binary file's stray CRs): open it read-only.
    lossy: bool,
    indent: String,
    version: u64,
    sels: Vec<Selection>,
    primary: usize,
    /// Edits not yet taken by `take_edits`.
    pending_edits: Vec<Edit>,
    /// Edits of the command in progress (wrap/find refresh after it).
    cmd_edits: Vec<Edit>,
    history: History,
    saved_state: u64,
    /// Next typing/backspace starts a new undo step.
    break_group: bool,
    wrap: WrapMap,
    find: Option<FindState>,
    /// Last text we put on the clipboard, and whether it was a whole-line copy.
    last_copy: Option<(String, bool)>,
    /// Expand Selection steps (selections, primary) to shrink back to.
    expand_stack: Vec<(Vec<Selection>, usize)>,
    /// Text version and selections the last expand/shrink left: the stack
    /// only applies while both are unchanged.
    expanded: Option<(u64, Vec<Selection>)>,
    now: fn() -> Instant,
    /// Inserted text of every edit, so tests can replay edits exactly.
    #[cfg(test)]
    edit_texts: Vec<String>,
}

/// One planned text change, in char offsets of the text before the command.
#[derive(Clone, Debug)]
struct Change {
    start: usize,
    end: usize,
    text: String,
    /// For `Plan::Carets`: caret position after the command, relative to the
    /// change's start in the new text.
    caret: Option<isize>,
    /// Selection index this change came from (to keep the primary).
    origin: usize,
}

impl Change {
    fn new(start: usize, end: usize, text: impl Into<String>, caret: isize, origin: usize) -> Self {
        Change { start, end, text: text.into(), caret: Some(caret), origin }
    }
    fn edit(start: usize, end: usize, text: impl Into<String>) -> Self {
        Change { start, end, text: text.into(), caret: None, origin: 0 }
    }
    fn is_noop(&self) -> bool {
        self.start == self.end && self.text.is_empty()
    }
}

/// How selections are computed after a text change.
enum Plan {
    /// One caret per change (`Change::caret`).
    Carets,
    /// Existing selections shift through the changes.
    Map,
    /// Final selections, already in post-change coordinates.
    Explicit(Vec<Selection>, usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bias {
    Left,
    Right,
}

impl Editor {
    /// Empty, untitled.
    pub fn new() -> Self {
        Self::from_text("")
    }

    /// Load a file. Detects the encoding (`encoding::detect`; any bytes open,
    /// Windows-1252 being the fallback), line endings (LF/CRLF) and the BOM,
    /// all preserved on save, and the indent unit (from content, else
    /// `default_indent`).
    pub fn open(path: &Path, default_indent: &str) -> std::io::Result<Self> {
        Self::open_with(path, default_indent, None)
    }

    /// `open`, decoding as `forced` (Reopen with Encoding) instead of detecting.
    pub fn open_with(path: &Path, default_indent: &str, forced: Option<(Encoding, bool)>) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        let (encoding, bom) = forced.unwrap_or_else(|| encoding::detect(&bytes));
        let (raw, lossless) = encoding::decode(&bytes, encoding);
        let binary = encoding::is_binary(&bytes);
        drop(bytes);
        let (crlf, exact) = line_endings(&raw);
        let text = text::normalize_newlines(&raw);
        let mut ed = Self::from_text(&text);
        ed.path = Some(path.to_path_buf());
        ed.crlf = crlf;
        ed.encoding = encoding;
        ed.bom = bom && encoding != Encoding::Windows1252;
        ed.forced = forced;
        // Mixed line endings in a text file are evened out on save, as they
        // always were; in a binary file a CR is data.
        ed.lossy = !lossless || (binary && !exact);
        ed.indent = detect_indent(&ed.rope).unwrap_or_else(|| default_indent.to_string());
        Ok(ed)
    }

    /// The file's text as `open` would read it now, in this buffer's encoding
    /// (LF line endings), to compare with the buffer.
    pub fn read_disk_text(&self) -> std::io::Result<String> {
        let path = self.path.as_deref().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "untitled buffer"))?;
        let bytes = std::fs::read(path)?;
        let (raw, _) = encoding::decode(&bytes, self.encoding);
        Ok(text::normalize_newlines(&raw).into_owned())
    }

    /// (encoding, BOM) the buffer saves in.
    pub fn encoding(&self) -> (Encoding, bool) {
        (self.encoding, self.bom)
    }

    /// The encoding Reopen with Encoding chose, if any (reloads keep it).
    pub fn forced_encoding(&self) -> Option<(Encoding, bool)> {
        self.forced
    }

    /// Decode as this on the next reload (`open_with(.., forced_encoding())`);
    /// None detects again.
    pub fn force_encoding(&mut self, forced: Option<(Encoding, bool)>) {
        self.forced = forced;
    }

    /// Saving wouldn't reproduce the bytes read: keep the buffer read-only.
    pub fn is_lossy(&self) -> bool {
        self.lossy
    }

    /// Convert to LF (`crlf` false) or CRLF line endings on save: one undo
    /// step, no text change. False when they already are.
    pub fn set_line_endings(&mut self, crlf: bool) -> bool {
        if self.crlf == crlf {
            return false;
        }
        let before = self.meta();
        self.crlf = crlf;
        self.push_meta(before);
        true
    }

    /// Convert to another encoding (saved in it from now on): one undo step,
    /// no text change. Err names a char the encoding can't store; Ok(false)
    /// when nothing changes.
    pub fn set_encoding(&mut self, encoding: Encoding, bom: bool) -> Result<bool, char> {
        let bom = bom && encoding != Encoding::Windows1252;
        if (self.encoding, self.bom) == (encoding, bom) {
            return Ok(false);
        }
        if let Some(c) = self.rope.chunks().find_map(|chunk| encoding::unencodable(chunk, encoding)) {
            return Err(c);
        }
        let before = self.meta();
        self.encoding = encoding;
        self.bom = bom;
        self.push_meta(before);
        Ok(true)
    }

    fn meta(&self) -> Meta {
        Meta { crlf: self.crlf, encoding: self.encoding, bom: self.bom }
    }

    fn set_meta(&mut self, m: Meta) {
        self.crlf = m.crlf;
        self.encoding = m.encoding;
        self.bom = m.bom;
    }

    fn push_meta(&mut self, before: Meta) {
        let after = self.meta();
        let sels = (self.sels.clone(), self.primary);
        let now = (self.now)();
        self.history.push_meta(before, after, sels, now);
        self.break_group = true;
    }

    pub fn from_text(text: &str) -> Self {
        let text = text::normalize_newlines(text);
        let rope = Rope::from_str(&text);
        let wrap = WrapMap::build(&rope, None, 4);
        Editor {
            indent: detect_indent(&rope).unwrap_or_else(|| "    ".to_string()),
            rope,
            path: None,
            crlf: false,
            encoding: Encoding::Utf8,
            bom: false,
            forced: None,
            lossy: false,
            version: 0,
            sels: vec![Selection::caret(Point::default())],
            primary: 0,
            pending_edits: Vec::new(),
            cmd_edits: Vec::new(),
            history: History::default(),
            saved_state: 0,
            break_group: true,
            wrap,
            find: None,
            last_copy: None,
            expand_stack: Vec::new(),
            expanded: None,
            now: Instant::now,
            #[cfg(test)]
            edit_texts: Vec::new(),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Increments on every text change (incl. undo/redo).
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Indent unit detected from the file ("\t", "  ", "    "...), else the
    /// default given to `open`. Pass it as `Ctx::indent_unit`.
    pub fn indent_unit(&self) -> &str {
        &self.indent
    }

    /// Whether the file uses CRLF line endings (restored on save).
    pub fn is_crlf(&self) -> bool {
        self.crlf
    }

    /// Dirty == differs from the last saved/opened state, tracked through
    /// undo (undo back to the saved point is clean).
    pub fn is_dirty(&self) -> bool {
        self.history.state() != self.saved_state
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "untitled buffer"))?;
        self.write_to(&path)?;
        self.saved_state = self.history.state();
        self.break_group = true;
        Ok(())
    }

    pub fn save_as(&mut self, path: PathBuf) -> std::io::Result<()> {
        self.path = Some(path);
        self.save()
    }

    /// Atomic write (temp file + rename) that keeps permissions and follows symlinks.
    fn write_to(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let dir = target.parent().unwrap_or(Path::new("."));
        let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let tmp = dir.join(format!(".{name}.qur-{}.tmp", std::process::id()));
        let result = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            let mut w = std::io::BufWriter::new(&mut f);
            if self.bom {
                w.write_all(&encoding::encode("", self.encoding, true).unwrap_or_default())?;
            }
            let mut buf = Vec::new();
            for chunk in self.rope.chunks() {
                let chunk = if self.crlf { std::borrow::Cow::Owned(chunk.replace('\n', "\r\n")) } else { std::borrow::Cow::Borrowed(chunk) };
                if self.encoding == Encoding::Utf8 {
                    w.write_all(chunk.as_bytes())?;
                    continue;
                }
                buf.clear();
                encoding::encode_into(&chunk, self.encoding, &mut buf).map_err(|c| {
                    let msg = format!("{} can't store {c:?} (U+{:04X}); convert it to UTF-8 first", encoding::label(self.encoding, self.bom), c as u32);
                    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
                })?;
                w.write_all(&buf)?;
            }
            w.flush()?;
            drop(w);
            f.sync_all()?;
            if let Ok(meta) = std::fs::metadata(&target) {
                std::fs::set_permissions(&tmp, meta.permissions())?;
            }
            std::fs::rename(&tmp, &target)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Selections sorted by position, non-overlapping; index 0 is not
    /// necessarily primary, see `primary()`.
    pub fn selections(&self) -> &[Selection] {
        &self.sels
    }

    pub fn primary(&self) -> Selection {
        self.sels[self.primary]
    }

    /// Index of the primary selection in `selections()`.
    pub fn primary_index(&self) -> usize {
        self.primary
    }

    pub fn set_selections(&mut self, sels: Vec<Selection>, primary: usize) {
        self.break_group = true;
        self.set_sels(sels, primary);
    }

    /// Mouse: place caret (`extend`: shift-click), `add`: Opt-click adds a cursor.
    pub fn click(&mut self, p: Point, extend: bool, add: bool) {
        let p = self.clamp(p);
        self.break_group = true;
        if extend {
            let mut s = self.primary();
            s.head = p;
            s.goal = None;
            let mut sels = self.sels.clone();
            sels[self.primary] = s;
            self.set_sels(sels, self.primary);
        } else if add {
            // Opt-click on an existing caret removes it (if it isn't the last one).
            if let Some(i) = self.sels.iter().position(|s| s.is_empty() && s.head == p) {
                if self.sels.len() > 1 {
                    let mut sels = self.sels.clone();
                    sels.remove(i);
                    let n = sels.len();
                    self.set_sels(sels, n - 1);
                }
                return;
            }
            let mut sels = self.sels.clone();
            sels.push(Selection::caret(p));
            let n = sels.len();
            self.set_sels(sels, n - 1);
        } else {
            self.set_sels(vec![Selection::caret(p)], 0);
        }
    }

    /// Double-click selects a word, triple-click a line.
    pub fn select_word_at(&mut self, p: Point) {
        let p = self.clamp(p);
        self.break_group = true;
        let s = self.word_selection_at(p).unwrap_or(Selection::caret(p));
        self.set_sels(vec![s], 0);
    }

    pub fn select_line_at(&mut self, p: Point) {
        let p = self.clamp(p);
        self.break_group = true;
        let start = Point { line: p.line, col: 0 };
        let end = if p.line + 1 < self.rope.len_lines() {
            Point { line: p.line + 1, col: 0 }
        } else {
            Point { line: p.line, col: line_len(&self.rope, p.line) }
        };
        self.set_sels(vec![Selection { anchor: start, head: end, goal: None }], 0);
    }

    /// Drag to `p` from the current primary anchor.
    pub fn drag_to(&mut self, p: Point) {
        let p = self.clamp(p);
        self.break_group = true;
        let mut sels = self.sels.clone();
        sels[self.primary].head = p;
        sels[self.primary].goal = None;
        self.set_sels(sels, self.primary);
    }

    /// The editor owns its wrap map and keeps it current on every edit;
    /// Up/Down/PageUp/PageDown move by visual rows.
    pub fn wrap(&self) -> &WrapMap {
        &self.wrap
    }

    /// `None` disables soft wrap. Called on resize, zoom and Opt+Z.
    pub fn set_wrap_width(&mut self, width: Option<usize>) {
        if self.wrap.width() != width {
            self.wrap.set_width_impl(&self.rope, width);
        }
    }

    pub fn run(&mut self, cmd: Command, cx: &mut Ctx) -> Outcome {
        self.run_command(cmd, cx)
    }

    /// Edits since the last call, in order (for qur_syntax incremental parsing).
    pub fn take_edits(&mut self) -> Vec<Edit> {
        std::mem::take(&mut self.pending_edits)
    }

    /// Clamp a point to valid text.
    pub fn clamp(&self, p: Point) -> Point {
        let line = p.line.min(self.rope.len_lines() - 1);
        Point { line, col: p.col.min(line_len(&self.rope, line)) }
    }

    pub fn point_to_byte(&self, p: Point) -> usize {
        self.rope.char_to_byte(self.to_char(p))
    }

    pub fn byte_to_point(&self, byte: usize) -> Point {
        let c = self.rope.byte_to_char(byte.min(self.rope.len_bytes()));
        self.to_point(c)
    }

    /// Text of the selections (joined with newlines), for "Use selection for find".
    pub fn selected_text(&self) -> String {
        self.sels
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| self.text_of(s))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // --- Find / replace ---------------------------------------------------

    /// Set the query; recomputes matches. Invalid regex -> no matches, Err message.
    pub fn set_find(&mut self, query: &str, opts: FindOptions) -> Result<usize, String> {
        if query.is_empty() {
            self.find = None;
            return Ok(0);
        }
        match FindState::new(query, opts, self.rope.len_lines()) {
            Ok(f) => {
                let n = f.total(&self.rope);
                self.find = Some(f);
                Ok(n)
            }
            Err(e) => {
                self.find = None;
                Err(e)
            }
        }
    }

    pub fn clear_find(&mut self) {
        self.find = None;
    }

    /// Matches intersecting a line range (for highlighting visible rows), as
    /// char ranges per line: (line, start_col..end_col). Multi-line matches
    /// are split per line.
    pub fn find_matches_in_lines(&self, lines: Range<usize>) -> Vec<(usize, Range<usize>)> {
        let Some(f) = &self.find else { return Vec::new() };
        let mut buf = String::new();
        let end = lines.end.min(self.rope.len_lines());
        let mut out = Vec::new();
        for l in lines.start.min(end)..end {
            for (s, e) in f.line(&self.rope, l, &mut buf) {
                out.push((l, s as usize..e as usize));
            }
        }
        out
    }

    /// (current 1-based index, total). Index 0 when the primary selection
    /// isn't exactly a match.
    pub fn find_status(&self) -> (usize, usize) {
        let Some(f) = &self.find else { return (0, 0) };
        let total = f.total(&self.rope);
        let p = self.primary();
        if p.is_empty() || p.start().line != p.end().line {
            return (0, total);
        }
        let mut buf = String::new();
        let line = p.start().line;
        let here = f.line(&self.rope, line, &mut buf);
        let Some(i) = here.iter().position(|&(s, e)| s as usize == p.start().col && e as usize == p.end().col)
        else {
            return (0, total);
        };
        let before: usize = (0..line).map(|l| f.line(&self.rope, l, &mut buf).len()).sum();
        (before + i + 1, total)
    }

    /// Select the next/previous match from the primary cursor (wraps).
    pub fn find_next(&mut self) -> Outcome {
        let from = self.primary().end();
        let found = self.search_match(from, true);
        self.select_match(found)
    }

    pub fn find_prev(&mut self) -> Outcome {
        let from = self.primary().start();
        let found = self.search_match(from, false);
        self.select_match(found)
    }

    /// Replace the current match (regex: `$1` groups) and move to the next.
    pub fn replace_one(&mut self, replacement: &str) -> Outcome {
        let p = self.primary();
        if let Some(text) = self.replacement_for_selection(&p, replacement) {
            let (a, b) = (self.to_char(p.start()), self.to_char(p.end()));
            let n = text.chars().count() as isize;
            self.commit(vec![Change::new(a, b, text, n, self.primary)], Plan::Carets, Kind::Other);
        }
        let mut out = self.find_next();
        out.text_changed = true;
        out
    }

    /// Replace all matches as one undo step; returns the count.
    pub fn replace_all(&mut self, replacement: &str) -> usize {
        let Some(f) = &self.find else { return 0 };
        let mut changes = Vec::new();
        let mut buf = String::new();
        for l in 0..self.rope.len_lines() {
            let ms = f.line(&self.rope, l, &mut buf);
            if ms.is_empty() {
                continue;
            }
            let line = line_string(&self.rope, l);
            let base = self.rope.line_to_char(l);
            for (s, e) in ms {
                let text = expand_replacement(f, &line, s as usize, replacement);
                changes.push(Change::edit(base + s as usize, base + e as usize, text));
            }
        }
        let n = changes.len();
        if n > 0 {
            self.commit(changes, Plan::Map, Kind::Other);
        }
        n
    }

    fn replacement_for_selection(&self, p: &Selection, replacement: &str) -> Option<String> {
        let f = self.find.as_ref()?;
        if p.is_empty() || p.start().line != p.end().line {
            return None;
        }
        let mut buf = String::new();
        let (l, s, e) = (p.start().line, p.start().col, p.end().col);
        let ms = f.line(&self.rope, l, &mut buf);
        ms.iter().find(|&&(ms, me)| ms as usize == s && me as usize == e)?;
        Some(expand_replacement(f, &line_string(&self.rope, l), s, replacement))
    }

    /// First match at/after (`forward`) or before `from`, wrapping around.
    fn search_match(&self, from: Point, forward: bool) -> Option<(usize, usize, usize)> {
        let f = self.find.as_ref()?;
        let n = self.rope.len_lines();
        let mut buf = String::new();
        let mut ml = |l: usize| f.line(&self.rope, l, &mut buf);
        if forward {
            if let Some(&(s, e)) = ml(from.line).iter().find(|(s, _)| *s as usize >= from.col) {
                return Some((from.line, s as usize, e as usize));
            }
            for k in 1..=n {
                let l = (from.line + k) % n;
                if let Some(&(s, e)) = ml(l).first() {
                    return Some((l, s as usize, e as usize));
                }
            }
        } else {
            if let Some(&(s, e)) = ml(from.line).iter().rev().find(|(s, _)| (*s as usize) < from.col) {
                return Some((from.line, s as usize, e as usize));
            }
            for k in 1..=n {
                let l = (from.line + n - k) % n;
                if let Some(&(s, e)) = ml(l).last() {
                    return Some((l, s as usize, e as usize));
                }
            }
        }
        None
    }

    fn select_match(&mut self, m: Option<(usize, usize, usize)>) -> Outcome {
        let Some((l, s, e)) = m else { return Outcome::default() };
        self.break_group = true;
        let sel = Selection {
            anchor: Point { line: l, col: s },
            head: Point { line: l, col: e },
            goal: None,
        };
        self.set_sels(vec![sel], 0);
        Outcome { text_changed: false, selections_changed: true, reveal: true }
    }

    // --- Internals ----------------------------------------------------------

    fn to_char(&self, p: Point) -> usize {
        let p = self.clamp(p);
        self.rope.line_to_char(p.line) + p.col
    }

    fn to_point(&self, c: usize) -> Point {
        let c = c.min(self.rope.len_chars());
        let line = self.rope.char_to_line(c);
        Point { line, col: c - self.rope.line_to_char(line) }
    }

    fn text_of(&self, s: &Selection) -> String {
        let (a, b) = (self.to_char(s.start()), self.to_char(s.end()));
        self.rope.slice(a..b).to_string()
    }

    fn end_point(&self) -> Point {
        let l = self.rope.len_lines() - 1;
        Point { line: l, col: line_len(&self.rope, l) }
    }

    /// Clamp, sort and merge selections; keeps track of the primary.
    fn set_sels(&mut self, sels: Vec<Selection>, primary: usize) {
        let mut v: Vec<(Selection, bool)> = sels
            .into_iter()
            .enumerate()
            .map(|(i, mut s)| {
                s.anchor = self.clamp(s.anchor);
                s.head = self.clamp(s.head);
                (s, i == primary)
            })
            .collect();
        if v.is_empty() {
            v.push((Selection::caret(Point::default()), true));
        }
        v.sort_by_key(|(s, _)| (s.start(), s.end()));
        let mut out: Vec<(Selection, bool)> = Vec::with_capacity(v.len());
        for (s, p) in v {
            if let Some((last, lp)) = out.last_mut() {
                let overlap = s.start() < last.end()
                    || s.start() == last.start()
                    || (s.start() == last.end() && (s.is_empty() || last.is_empty()));
                if overlap {
                    let start = last.start().min(s.start());
                    let end = last.end().max(s.end());
                    let reversed = if *lp || !p { last.head < last.anchor } else { s.head < s.anchor };
                    let goal = if p { s.goal } else { last.goal };
                    *last = if reversed {
                        Selection { anchor: end, head: start, goal }
                    } else {
                        Selection { anchor: start, head: end, goal }
                    };
                    *lp |= p;
                    continue;
                }
            }
            out.push((s, p));
        }
        self.primary = out.iter().position(|(_, p)| *p).unwrap_or(out.len() - 1);
        self.sels = out.into_iter().map(|(s, _)| s).collect();
    }

    /// Low-level replace that records undo data and the tree-sitter edit.
    fn apply_raw(&mut self, start: usize, end: usize, text: &str) -> Applied {
        let rope = &mut self.rope;
        let start_byte = rope.char_to_byte(start);
        let old_end_byte = rope.char_to_byte(end);
        let start_point = byte_point(rope, start_byte);
        let old_end_point = byte_point(rope, old_end_byte);
        let removed = rope.slice(start..end).to_string();
        if end > start {
            rope.remove(start..end);
        }
        if !text.is_empty() {
            rope.insert(start, text);
        }
        self.cmd_edits.push(Edit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte + text.len(),
            start_point,
            old_end_point,
            new_end_point: text::advance_point(start_point, text),
        });
        #[cfg(test)]
        self.edit_texts.push(text.to_string());
        Applied { start, removed, inserted: text.to_string() }
    }

    /// Refresh wrap, find and version after a batch of `apply_raw` calls.
    fn after_text_change(&mut self) {
        let edits = std::mem::take(&mut self.cmd_edits);
        self.wrap.apply_edits_impl(&self.rope, &edits);
        if let Some(f) = &mut self.find {
            f.apply_edits(&edits);
        }
        self.pending_edits.extend(edits);
        self.version += 1;
    }

    /// Apply changes as one undo step and compute the new selections.
    fn commit(&mut self, mut changes: Vec<Change>, plan: Plan, kind: Kind) -> Outcome {
        changes.sort_by_key(|c| (c.start, c.end));
        // Merge overlapping changes (e.g. two cursors deleting the same line).
        let mut merged: Vec<Change> = Vec::with_capacity(changes.len());
        for c in changes {
            if let Some(last) = merged.last_mut() {
                let overlap = c.start < last.end || (c.start == last.start && c.end == last.end && !c.is_noop());
                if overlap {
                    last.end = last.end.max(c.end);
                    if c.text != last.text {
                        last.text.push_str(&c.text);
                    }
                    if c.origin == self.primary {
                        last.origin = c.origin;
                    }
                    continue;
                }
            }
            merged.push(c);
        }
        let changes = merged;

        // New selections as char offsets in the post-change text.
        let char_sels: Vec<(usize, usize, Option<usize>)>;
        let mut primary = 0;
        let mut explicit = None;
        match plan {
            Plan::Carets => {
                let mut delta: isize = 0;
                let mut v = Vec::with_capacity(changes.len());
                for c in &changes {
                    if let Some(k) = c.caret {
                        let pos = (c.start as isize + delta + k).max(0) as usize;
                        if c.origin == self.primary {
                            primary = v.len();
                        }
                        v.push((pos, pos, None));
                    }
                    delta += c.text.chars().count() as isize - (c.end - c.start) as isize;
                }
                char_sels = v;
            }
            Plan::Map => {
                primary = self.primary;
                char_sels = self
                    .sels
                    .iter()
                    .map(|s| {
                        let (a, h) = (self.to_char(s.anchor), self.to_char(s.head));
                        if a == h {
                            let p = map_pos(&changes, a, Bias::Right);
                            (p, p, s.goal)
                        } else if a < h {
                            (map_pos(&changes, a, Bias::Left), map_pos(&changes, h, Bias::Right), None)
                        } else {
                            (map_pos(&changes, a, Bias::Right), map_pos(&changes, h, Bias::Left), None)
                        }
                    })
                    .collect();
            }
            Plan::Explicit(s, p) => {
                explicit = Some((s, p));
                char_sels = Vec::new();
            }
        }

        let effective = changes.iter().any(|c| !c.is_noop());
        let before = (self.sels.clone(), self.primary);
        let mut applied = Vec::new();
        if effective {
            for c in changes.iter().rev() {
                if !c.is_noop() {
                    applied.push(self.apply_raw(c.start, c.end, &c.text));
                }
            }
            self.after_text_change();
        }
        match explicit {
            Some((s, p)) => self.set_sels(s, p),
            None => {
                let sels = char_sels
                    .iter()
                    .map(|&(a, h, goal)| Selection { anchor: self.to_point(a), head: self.to_point(h), goal })
                    .collect();
                self.set_sels(sels, primary);
            }
        }
        if !effective {
            self.break_group = true;
            return Outcome { text_changed: false, selections_changed: true, reveal: true };
        }
        let after = (self.sels.clone(), self.primary);
        let now = (self.now)();
        self.history.push(applied, before, after, kind, now, self.break_group);
        self.break_group = false;
        Outcome { text_changed: true, selections_changed: true, reveal: true }
    }

    fn undo(&mut self) -> Outcome {
        let Some(t) = self.history.undo.pop() else { return Outcome::default() };
        for a in t.changes.iter().rev() {
            let end = a.start + a.inserted.chars().count();
            self.apply_raw(a.start, end, &a.removed);
        }
        self.after_text_change();
        if let Some((before, _)) = t.meta {
            self.set_meta(before);
        }
        self.set_sels(t.before.0.clone(), t.before.1);
        self.history.redo.push(t);
        self.break_group = true;
        Outcome { text_changed: true, selections_changed: true, reveal: true }
    }

    fn redo(&mut self) -> Outcome {
        let Some(t) = self.history.redo.pop() else { return Outcome::default() };
        for a in &t.changes {
            let end = a.start + a.removed.chars().count();
            self.apply_raw(a.start, end, &a.inserted);
        }
        self.after_text_change();
        if let Some((_, after)) = t.meta {
            self.set_meta(after);
        }
        self.set_sels(t.after.0.clone(), t.after.1);
        self.history.undo.push(t);
        self.break_group = true;
        Outcome { text_changed: true, selections_changed: true, reveal: true }
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

/// Where a char offset lands after `changes` (ascending, non-overlapping).
fn map_pos(changes: &[Change], pos: usize, bias: Bias) -> usize {
    let mut delta: isize = 0;
    for c in changes {
        if c.is_noop() {
            continue;
        }
        let len = c.text.chars().count() as isize;
        if pos < c.start {
            break;
        }
        if pos > c.end {
            delta += len - (c.end - c.start) as isize;
            continue;
        }
        // c.start <= pos <= c.end
        let base = c.start as isize + delta;
        if c.start == c.end {
            return (base + if bias == Bias::Right { len } else { 0 }) as usize;
        }
        if pos == c.start && bias == Bias::Left {
            return base as usize;
        }
        if pos == c.end || bias == Bias::Right {
            return (base + len) as usize;
        }
        return base as usize;
    }
    (pos as isize + delta) as usize
}

/// Replacement text for the match starting at char `start` of `line`.
fn expand_replacement(f: &FindState, line: &str, start: usize, replacement: &str) -> String {
    if !f.opts.regex {
        return replacement.to_string();
    }
    let b = text::char_to_byte(line, start);
    match f.regex().captures_at(line, b) {
        Some(caps) if caps.get(0).is_some_and(|m| m.start() == b) => {
            let mut dst = String::new();
            caps.expand(replacement, &mut dst);
            dst
        }
        _ => replacement.to_string(),
    }
}

/// (CRLF?, saving back reproduces the line endings exactly): CRLF when at
/// least half the line breaks are CRLF; exact when there's no lone CR and
/// every break is the winning kind.
fn line_endings(raw: &str) -> (bool, bool) {
    let crlf_count = raw.matches("\r\n").count();
    let lf_count = raw.matches('\n').count();
    let cr_count = raw.matches('\r').count();
    let crlf = crlf_count > 0 && crlf_count * 2 >= lf_count;
    let exact = cr_count == crlf_count && if crlf { crlf_count == lf_count } else { crlf_count == 0 };
    (crlf, exact)
}

/// Most common indent step among the first 1000 lines; tabs win if more
/// lines start with a tab than with spaces.
fn detect_indent(rope: &Rope) -> Option<String> {
    let mut tabs = 0usize;
    let mut spaces = 0usize;
    let mut steps = [0usize; 9];
    let mut prev = 0usize;
    let mut buf = String::new();
    for l in 0..rope.len_lines().min(1000) {
        text::line_into(rope, l, &mut buf);
        if buf.trim().is_empty() {
            continue;
        }
        if buf.starts_with('\t') {
            tabs += 1;
            continue;
        }
        let n = buf.chars().take_while(|c| *c == ' ').count();
        if n > 0 {
            spaces += 1;
        }
        if n > prev && n - prev <= 8 {
            steps[n - prev] += 1;
        }
        prev = n;
    }
    if tabs == 0 && spaces == 0 {
        return None;
    }
    if tabs > spaces {
        return Some("\t".to_string());
    }
    let (best, count) = steps.iter().enumerate().skip(1).max_by_key(|&(i, c)| (*c, usize::MAX - i))?;
    (*count > 0).then(|| " ".repeat(best))
}

// --- Soft wrap ---------------------------------------------------------------

/// Maps buffer lines to visual rows for a wrap width in columns (monospace).
/// Wraps at word boundaries when possible, else hard at the width. Wide (CJK)
/// chars count 2 columns, tabs expand to `tab_width`.
pub struct WrapMap {
    width: Option<usize>,
    tab_width: usize,
    /// Per buffer line; `None` only transiently while applying edits.
    breaks: Vec<Option<LineWrap>>,
    /// First visual row of each line; one extra entry = total rows.
    row_start: Vec<usize>,
}

struct LineWrap {
    /// Line length in chars (excluding '\n').
    len: u32,
    /// Char offsets where rows 2.. start.
    breaks: Vec<u32>,
}

/// One visual row: buffer `line`, the char range of that line it shows, and
/// whether it is the first row of the line (gets a line number).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualRow {
    pub line: usize,
    pub chars: Range<usize>,
    pub first: bool,
}

impl WrapMap {
    /// `width` = None disables wrapping (one row per line).
    pub fn new(rope: &Rope, width: Option<usize>, tab_width: usize) -> Self {
        Self::build(rope, width, tab_width)
    }

    /// Update after edits (only rewrap changed lines).
    pub fn apply_edits(&mut self, rope: &Rope, edits: &[Edit]) {
        self.apply_edits_impl(rope, edits)
    }

    /// Rewrap everything for a new width.
    pub fn set_width(&mut self, rope: &Rope, width: Option<usize>) {
        self.set_width_impl(rope, width)
    }

    pub fn row_count(&self) -> usize {
        self.row_count_impl()
    }

    /// Rows `range` (clamped), for rendering the viewport.
    pub fn rows(&self, range: Range<usize>) -> Vec<VisualRow> {
        self.rows_impl(range)
    }

    /// Visual (row, visual col) of a buffer point.
    pub fn point_to_row(&self, rope: &Rope, p: Point) -> (usize, usize) {
        self.point_to_row_impl(rope, p)
    }

    /// Buffer point at a visual (row, visual col), clamped to the row.
    pub fn row_to_point(&self, rope: &Rope, row: usize, col: usize) -> Point {
        self.row_to_point_impl(rope, row, col)
    }
}
