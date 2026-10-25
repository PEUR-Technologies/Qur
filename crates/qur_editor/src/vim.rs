//! Vim mode (M6, optional; `[keys] vim = true`): normal, insert and visual
//! basics on top of the editor model. The UI turns keystrokes into `Key`s and
//! calls `Vim::key`; `None` means "not mine" (insert-mode typing goes through
//! the normal editor path).
//!
//! Normal: h j k l w b e 0 ^ $ gg G (counts), x X D C J r p P u Ctrl-R . i a I
//! A o O v V, operators d c y > < with motions or doubled (dd, cc, yy, >>, <<).
//! Visual / visual line: the motions, then d x y c > < or Esc.

use crate::{Command, Ctx, Editor, Outcome, Point, Selection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl Mode {
    /// For the status line.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "VISUAL LINE",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Esc,
    /// Ctrl+R (redo).
    Redo,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// The motion's target char is not included (w, h, l, 0, ^, b).
    Exclusive,
    /// Included (e, $).
    Inclusive,
    /// Whole lines (j, k, gg, G, and doubled operators).
    Linewise,
}

#[derive(Clone, Debug, Default)]
struct Change {
    keys: Vec<Key>,
    /// Text typed in insert mode as part of the change (c, o, i…).
    typed: String,
}

#[derive(Default)]
pub struct Vim {
    pub mode: Option<Mode>,
    count: Option<usize>,
    op: Option<char>,
    op_count: Option<usize>,
    pending: Option<char>,
    /// The unnamed register: text and whether it's whole lines.
    register: (String, bool),
    /// Keys of the change in progress, and the last finished one (for `.`).
    recording: Option<Change>,
    last_change: Option<Change>,
    insert_start: Option<usize>,
    replaying: bool,
}

impl Vim {
    pub fn new() -> Vim {
        Vim { mode: Some(Mode::Normal), ..Default::default() }
    }

    pub fn mode(&self) -> Mode {
        self.mode.unwrap_or(Mode::Normal)
    }

    /// Show a block cursor (normal and visual).
    pub fn block_cursor(&self) -> bool {
        self.mode() != Mode::Insert
    }

    /// Pending keys for the status line ("2d", "g").
    pub fn pending_text(&self) -> String {
        let mut s = String::new();
        if let Some(n) = self.op_count {
            s.push_str(&n.to_string());
        }
        if let Some(o) = self.op {
            s.push(o);
        }
        if let Some(n) = self.count {
            s.push_str(&n.to_string());
        }
        if let Some(p) = self.pending {
            s.push(p);
        }
        s
    }

    pub fn key(&mut self, e: &mut Editor, key: Key, cx: &mut Ctx) -> Option<Outcome> {
        if self.mode() == Mode::Insert {
            if key != Key::Esc {
                return None;
            }
            self.leave_insert(e, cx);
            return Some(changed());
        }
        if let Some(rec) = self.recording.as_mut() {
            rec.keys.push(key.clone());
        }
        let out = match self.mode() {
            Mode::Visual | Mode::VisualLine => self.visual_key(e, key, cx),
            _ => self.normal_key(e, key, cx),
        };
        Some(out)
    }

    fn leave_insert(&mut self, e: &mut Editor, cx: &mut Ctx) {
        let head = cur(e);
        if let (Some(start), Some(rec)) = (self.insert_start.take(), self.recording.as_mut())
            && head >= start
        {
            rec.typed = e.rope().slice(start..head).to_string();
        }
        self.finish_change();
        self.mode = Some(Mode::Normal);
        // Like Vim, the cursor steps back onto the last typed char.
        let p = e.primary().head;
        if p.col > 0 {
            set_cursor(e, Point { line: p.line, col: p.col - 1 });
        }
        let _ = cx;
    }

    fn start_change(&mut self, key: Key) {
        if !self.replaying {
            self.recording = Some(Change { keys: vec![key], typed: String::new() });
        }
    }

    fn finish_change(&mut self) {
        if let Some(c) = self.recording.take() {
            self.last_change = Some(c);
        }
    }

    fn enter_insert(&mut self, e: &Editor) {
        self.mode = Some(Mode::Insert);
        self.insert_start = Some(cur(e));
    }

    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1)
    }

    fn reset(&mut self) {
        self.count = None;
        self.op = None;
        self.op_count = None;
        self.pending = None;
    }

    fn normal_key(&mut self, e: &mut Editor, key: Key, cx: &mut Ctx) -> Outcome {
        let c = match key {
            Key::Esc => {
                self.reset();
                self.recording = None;
                return moved();
            }
            Key::Redo => {
                self.reset();
                return e.run(Command::Redo, cx);
            }
            Key::Char(c) => c,
        };
        // Two-key commands: r<char>, g<g>.
        if let Some(p) = self.pending.take() {
            return match (p, c) {
                ('r', ch) => {
                    let n = self.take_count();
                    let out = self.replace_chars(e, ch, n, cx);
                    self.finish_change();
                    out
                }
                ('g', 'g') => {
                    let line = self.count.take().map(|n| n.saturating_sub(1)).unwrap_or(0);
                    self.motion_to(e, line_first_nonblank(e, line), Kind::Linewise, cx)
                }
                _ => {
                    self.reset();
                    moved()
                }
            };
        }
        if c.is_ascii_digit() && (c != '0' || self.count.is_some()) {
            let d = c.to_digit(10).unwrap_or(0) as usize;
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(d).min(99_999));
            return Outcome::default();
        }
        if let Some((target, kind)) = self.motion(e, c) {
            return self.motion_to(e, target, kind, cx);
        }
        // Operators: d c y > <. Doubled = whole lines.
        if matches!(c, 'd' | 'c' | 'y' | '>' | '<') {
            match self.op {
                Some(o) if o == c => {
                    let n = self.op_count.take().unwrap_or(1) * self.take_count();
                    self.op = None;
                    let line = e.primary().head.line;
                    let last = (line + n - 1).min(e.line_count() - 1);
                    return self.apply_lines(e, c, line, last, cx);
                }
                Some(_) => {
                    self.reset();
                    self.recording = None;
                    return moved();
                }
                None => {
                    if c != 'y' {
                        self.start_change(Key::Char(c));
                    }
                    self.op = Some(c);
                    self.op_count = self.count.take();
                    return Outcome::default();
                }
            }
        }
        if self.op.is_some() {
            // Not a motion: cancel the operator.
            self.reset();
            self.recording = None;
            return moved();
        }
        let n = self.take_count();
        let p = e.primary().head;
        match c {
            'g' | 'r' => {
                if c == 'r' {
                    self.start_change(Key::Char('r'));
                }
                self.pending = Some(c);
                self.count = (n > 1).then_some(n);
                Outcome::default()
            }
            'i' | 'a' | 'I' | 'A' | 'o' | 'O' => {
                self.start_change(Key::Char(c));
                match c {
                    'a' => {
                        if line_len(e, p.line) > 0 {
                            set_cursor(e, Point { line: p.line, col: p.col + 1 });
                        }
                    }
                    'I' => set_cursor_char(e, line_first_nonblank(e, p.line)),
                    'A' => set_cursor(e, Point { line: p.line, col: line_len(e, p.line) }),
                    'o' => {
                        e.run(Command::InsertLineBelow, cx);
                    }
                    'O' => {
                        e.run(Command::InsertLineAbove, cx);
                    }
                    _ => {}
                }
                self.enter_insert(e);
                changed()
            }
            'x' | 'X' => {
                self.start_change(Key::Char(c));
                let len = line_len(e, p.line);
                let (from, to) = if c == 'x' { (p.col, (p.col + n).min(len)) } else { (p.col.saturating_sub(n), p.col) };
                let out = if from < to {
                    let ls = line_start(e, p.line);
                    self.register = (e.rope().slice(ls + from..ls + to).to_string(), false);
                    replace(e, ls + from, ls + to, "", cx)
                } else {
                    moved()
                };
                self.finish_change();
                clamp_normal(e);
                out
            }
            'D' | 'C' => {
                self.start_change(Key::Char(c));
                let ls = line_start(e, p.line);
                let le = ls + line_len(e, p.line);
                let at = ls + p.col.min(line_len(e, p.line));
                self.register = (e.rope().slice(at..le).to_string(), false);
                let out = replace(e, at, le, "", cx);
                if c == 'C' {
                    self.enter_insert(e);
                } else {
                    self.finish_change();
                    clamp_normal(e);
                }
                out
            }
            'J' => {
                self.start_change(Key::Char('J'));
                let mut out = Outcome::default();
                for _ in 0..n.max(1) {
                    let line = e.primary().head.line;
                    if line + 1 >= e.line_count() {
                        break;
                    }
                    let end = line_start(e, line) + line_len(e, line);
                    let next = line_start(e, line + 1);
                    let rest = line_first_nonblank(e, line + 1);
                    let next_empty = line_len(e, line + 1) == 0;
                    let sep = if next_empty || e.rope().char(rest) == ')' { "" } else { " " };
                    let trimmed = trim_trailing(e, line);
                    out = replace(e, trimmed, rest.max(next), sep, cx);
                    let _ = end;
                    set_cursor_char(e, trimmed);
                }
                self.finish_change();
                out
            }
            'p' | 'P' => {
                self.start_change(Key::Char(c));
                let (text, lines) = self.register.clone();
                let out = self.paste(e, &text, lines, c == 'p', n, cx);
                self.finish_change();
                out
            }
            'u' => e.run(Command::Undo, cx),
            '.' => self.repeat(e, n, cx),
            'v' => {
                self.mode = Some(Mode::Visual);
                let p = e.primary().head;
                e.set_selections(vec![Selection::caret(p)], 0);
                moved()
            }
            'V' => {
                self.mode = Some(Mode::VisualLine);
                let p = e.primary().head;
                e.set_selections(vec![Selection::caret(p)], 0);
                moved()
            }
            _ => Outcome::default(),
        }
    }

    fn visual_key(&mut self, e: &mut Editor, key: Key, cx: &mut Ctx) -> Outcome {
        let c = match key {
            Key::Esc => {
                self.mode = Some(Mode::Normal);
                let h = e.primary().head;
                e.set_selections(vec![Selection::caret(h)], 0);
                clamp_normal(e);
                return moved();
            }
            Key::Redo => return Outcome::default(),
            Key::Char(c) => c,
        };
        if let Some(p) = self.pending.take() {
            if (p, c) == ('g', 'g') {
                let line = self.count.take().map(|n| n.saturating_sub(1)).unwrap_or(0);
                return self.extend_to(e, line_first_nonblank(e, line));
            }
            return Outcome::default();
        }
        if c.is_ascii_digit() && (c != '0' || self.count.is_some()) {
            let d = c.to_digit(10).unwrap_or(0) as usize;
            self.count = Some(self.count.unwrap_or(0) * 10 + d);
            return Outcome::default();
        }
        if c == 'g' {
            self.pending = Some('g');
            return Outcome::default();
        }
        if let Some((target, _)) = self.motion(e, c) {
            return self.extend_to(e, target);
        }
        let sel = e.primary();
        let (a, b) = (sel.anchor, sel.head);
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let linewise = self.mode() == Mode::VisualLine;
        self.mode = Some(Mode::Normal);
        match c {
            'v' | 'V' => {
                let want = if c == 'v' { Mode::Visual } else { Mode::VisualLine };
                self.mode = Some(if (c == 'v' && !linewise) || (c == 'V' && linewise) { Mode::Normal } else { want });
                if self.mode() == Mode::Normal {
                    e.set_selections(vec![Selection::caret(b)], 0);
                }
                moved()
            }
            'd' | 'x' | 'y' | 'c' | '>' | '<' => {
                let op = if c == 'x' { 'd' } else { c };
                if linewise || matches!(op, '>' | '<') {
                    if op != 'y' {
                        self.recording = None;
                    }
                    self.apply_lines(e, op, lo.line, hi.line, cx)
                } else {
                    let s = to_char(e, lo);
                    let t = (to_char(e, hi) + 1).min(e.rope().len_chars());
                    self.apply_range(e, op, s, t, false, cx)
                }
            }
            _ => {
                self.mode = Some(if linewise { Mode::VisualLine } else { Mode::Visual });
                Outcome::default()
            }
        }
    }

    /// Where motion `c` lands from the cursor, or None if `c` isn't a motion.
    fn motion(&mut self, e: &Editor, c: char) -> Option<(usize, Kind)> {
        let n = if matches!(c, 'h' | 'j' | 'k' | 'l' | 'w' | 'b' | 'e' | '$' | 'G') { self.count.take().unwrap_or(1) } else { 1 };
        let p = e.primary().head;
        let here = to_char(e, p);
        let last_line = e.line_count() - 1;
        Some(match c {
            'h' => (line_start(e, p.line) + p.col.saturating_sub(n), Kind::Exclusive),
            'l' => (line_start(e, p.line) + (p.col + n).min(line_len(e, p.line)), Kind::Exclusive),
            'j' | 'k' => {
                let line = if c == 'j' { (p.line + n).min(last_line) } else { p.line.saturating_sub(n) };
                (line_start(e, line) + p.col.min(line_len(e, line).saturating_sub(1)), Kind::Linewise)
            }
            '0' => (line_start(e, p.line), Kind::Exclusive),
            '^' => (line_first_nonblank(e, p.line), Kind::Exclusive),
            '$' => {
                let line = (p.line + n - 1).min(last_line);
                (line_start(e, line) + line_len(e, line).saturating_sub(1).max(0), Kind::Inclusive)
            }
            'w' => ((0..n).fold(here, |at, _| word_forward(e, at)), Kind::Exclusive),
            'b' => ((0..n).fold(here, |at, _| word_back(e, at)), Kind::Exclusive),
            'e' => ((0..n).fold(here, |at, _| word_end(e, at)), Kind::Inclusive),
            'G' => {
                let line = if self.count.is_some() || n > 1 { (n - 1).min(last_line) } else { last_line };
                (line_first_nonblank(e, line), Kind::Linewise)
            }
            _ => return None,
        })
    }

    fn motion_to(&mut self, e: &mut Editor, target: usize, kind: Kind, cx: &mut Ctx) -> Outcome {
        let Some(op) = self.op.take() else {
            self.count = None;
            set_cursor_char(e, target);
            clamp_normal(e);
            return moved();
        };
        self.op_count = None;
        let here = cur(e);
        if kind == Kind::Linewise {
            let (a, b) = (e.byte_to_point(e.rope().char_to_byte(here)).line, e.byte_to_point(e.rope().char_to_byte(target)).line);
            return self.apply_lines(e, op, a.min(b), a.max(b), cx);
        }
        let (mut s, mut t) = (here.min(target), here.max(target));
        if kind == Kind::Inclusive {
            t = (t + 1).min(e.rope().len_chars());
        }
        // `cw` changes to the end of the word, like `ce` (Vim's special case).
        if op == 'c' && t > s && e.rope().char(t - 1).is_whitespace() && !e.rope().char(s).is_whitespace() {
            while t > s && e.rope().char(t - 1).is_whitespace() {
                t -= 1;
            }
        }
        // An exclusive motion that crosses to the next line stops at the line end.
        let line_end = line_start(e, e.primary().head.line) + line_len(e, e.primary().head.line);
        if kind == Kind::Exclusive && t > line_end && s <= line_end && !matches!(op, '>' | '<') {
            t = line_end;
        }
        if s > t {
            std::mem::swap(&mut s, &mut t);
        }
        self.apply_range(e, op, s, t, false, cx)
    }

    fn extend_to(&mut self, e: &mut Editor, target: usize) -> Outcome {
        let sel = e.primary();
        let head = e.byte_to_point(e.rope().char_to_byte(target));
        e.set_selections(vec![Selection { anchor: sel.anchor, head, goal: None }], 0);
        moved()
    }

    /// Operator over chars [s, t).
    fn apply_range(&mut self, e: &mut Editor, op: char, s: usize, t: usize, lines: bool, cx: &mut Ctx) -> Outcome {
        let text = e.rope().slice(s..t).to_string();
        match op {
            'y' => {
                self.register = (text, lines);
                set_cursor_char(e, s);
                clamp_normal(e);
                moved()
            }
            'd' | 'c' => {
                self.register = (text, lines);
                let out = replace(e, s, t, "", cx);
                set_cursor_char(e, s);
                if op == 'c' {
                    self.enter_insert(e);
                } else {
                    self.finish_change();
                    clamp_normal(e);
                }
                out
            }
            _ => moved(),
        }
    }

    /// Operator over whole lines `a..=b` (dd, cc, yy, >>, <<, visual line).
    fn apply_lines(&mut self, e: &mut Editor, op: char, a: usize, b: usize, cx: &mut Ctx) -> Outcome {
        let total = e.rope().len_chars();
        let s = line_start(e, a);
        let t = if b + 1 < e.line_count() { line_start(e, b + 1) } else { total };
        let mut text = e.rope().slice(s..t).to_string();
        let had_newline = text.ends_with('\n');
        if !had_newline {
            text.push('\n');
        }
        match op {
            'y' => {
                self.register = (text, true);
                set_cursor_char(e, line_first_nonblank(e, a));
                moved()
            }
            'd' => {
                self.register = (text, true);
                // The file's last line has no newline of its own: take the
                // one before it instead.
                let (s2, t2) = if !had_newline && a > 0 { (s - 1, t) } else { (s, t) };
                let out = replace(e, s2, t2, "", cx);
                let line = a.min(e.line_count() - 1);
                set_cursor_char(e, line_first_nonblank(e, line));
                self.finish_change();
                out
            }
            'c' => {
                self.register = (text, true);
                let indent: String = e.rope().slice(s..line_first_nonblank(e, a)).to_string();
                let end = if t == total { t } else { t - 1 };
                let out = replace(e, s, end, &indent, cx);
                set_cursor_char(e, s + indent.chars().count());
                self.enter_insert(e);
                out
            }
            '>' | '<' => {
                let first = e.byte_to_point(e.rope().char_to_byte(s));
                let last = Point { line: b, col: line_len(e, b) };
                e.set_selections(vec![Selection { anchor: first, head: last, goal: None }], 0);
                let out = e.run(if op == '>' { Command::Indent } else { Command::Outdent }, cx);
                set_cursor_char(e, line_first_nonblank(e, a));
                self.finish_change();
                out
            }
            _ => moved(),
        }
    }

    fn replace_chars(&mut self, e: &mut Editor, ch: char, n: usize, cx: &mut Ctx) -> Outcome {
        let p = e.primary().head;
        if p.col + n > line_len(e, p.line) {
            return moved();
        }
        let at = to_char(e, p);
        let out = replace(e, at, at + n, &ch.to_string().repeat(n), cx);
        set_cursor_char(e, at + n - 1);
        out
    }

    fn paste(&mut self, e: &mut Editor, text: &str, lines: bool, after: bool, n: usize, cx: &mut Ctx) -> Outcome {
        if text.is_empty() {
            return moved();
        }
        let text = text.repeat(n.max(1));
        let p = e.primary().head;
        if lines {
            let total = e.rope().len_chars();
            let at = if after { if p.line + 1 < e.line_count() { line_start(e, p.line + 1) } else { total } } else { line_start(e, p.line) };
            let ins = if after && at == total && !e.rope().to_string().ends_with('\n') { format!("\n{}", text.trim_end_matches('\n')) } else { text.clone() };
            let out = replace(e, at, at, &ins, cx);
            let line = if after { p.line + 1 } else { p.line };
            set_cursor_char(e, line_first_nonblank(e, line.min(e.line_count() - 1)));
            out
        } else {
            let at = to_char(e, p) + if after && line_len(e, p.line) > 0 { 1 } else { 0 };
            let out = replace(e, at, at, &text, cx);
            set_cursor_char(e, at + text.chars().count() - 1);
            out
        }
    }

    /// `.`: replay the last change (its keys, then the text typed with it).
    fn repeat(&mut self, e: &mut Editor, n: usize, cx: &mut Ctx) -> Outcome {
        let Some(change) = self.last_change.clone() else { return Outcome::default() };
        self.replaying = true;
        let mut out = Outcome::default();
        for _ in 0..n.max(1) {
            for k in &change.keys {
                if let Some(o) = self.key(e, k.clone(), cx) {
                    out = o;
                }
            }
            if self.mode() == Mode::Insert {
                if !change.typed.is_empty() {
                    e.run(Command::Insert(change.typed.clone()), cx);
                }
                self.key(e, Key::Esc, cx);
            }
        }
        self.replaying = false;
        out
    }
}

fn moved() -> Outcome {
    Outcome { text_changed: false, selections_changed: true, reveal: true }
}

fn changed() -> Outcome {
    Outcome { text_changed: true, selections_changed: true, reveal: true }
}

fn line_start(e: &Editor, line: usize) -> usize {
    e.rope().line_to_char(line.min(e.line_count() - 1))
}

fn line_len(e: &Editor, line: usize) -> usize {
    crate::text::line_len(e.rope(), line.min(e.line_count() - 1))
}

fn to_char(e: &Editor, p: Point) -> usize {
    line_start(e, p.line) + p.col.min(line_len(e, p.line))
}

fn cur(e: &Editor) -> usize {
    to_char(e, e.primary().head)
}

fn set_cursor(e: &mut Editor, p: Point) {
    let p = e.clamp(p);
    e.set_selections(vec![Selection::caret(p)], 0);
}

fn set_cursor_char(e: &mut Editor, c: usize) {
    let c = c.min(e.rope().len_chars());
    let p = e.byte_to_point(e.rope().char_to_byte(c));
    set_cursor(e, p);
}

/// Normal mode sits on a char: never past the last one.
fn clamp_normal(e: &mut Editor) {
    let p = e.primary().head;
    let len = line_len(e, p.line);
    if len > 0 && p.col >= len {
        set_cursor(e, Point { line: p.line, col: len - 1 });
    }
}

fn line_first_nonblank(e: &Editor, line: usize) -> usize {
    let s = line_start(e, line);
    let len = line_len(e, line);
    (0..len).find(|&i| !matches!(e.rope().char(s + i), ' ' | '\t')).map(|i| s + i).unwrap_or(s + len.saturating_sub(0).min(len))
}

fn trim_trailing(e: &Editor, line: usize) -> usize {
    let s = line_start(e, line);
    let mut end = s + line_len(e, line);
    while end > s && matches!(e.rope().char(end - 1), ' ' | '\t') {
        end -= 1;
    }
    end
}

/// Replace chars [s, t) with `text` (one undo step).
fn replace(e: &mut Editor, s: usize, t: usize, text: &str, cx: &mut Ctx) -> Outcome {
    let a = e.byte_to_point(e.rope().char_to_byte(s));
    let b = e.byte_to_point(e.rope().char_to_byte(t));
    e.set_selections(vec![Selection { anchor: a, head: b, goal: None }], 0);
    let mut out = e.run(Command::Insert(text.to_string()), cx);
    out.reveal = true;
    out
}

#[derive(PartialEq, Clone, Copy)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

fn word_forward(e: &Editor, at: usize) -> usize {
    let r = e.rope();
    let n = r.len_chars();
    if at >= n {
        return n;
    }
    let mut i = at;
    let start = class(r.char(i));
    if start != Class::Space {
        while i < n && class(r.char(i)) == start {
            i += 1;
        }
    }
    while i < n && class(r.char(i)) == Class::Space {
        // An empty line is a word of its own.
        if r.char(i) == '\n' && i + 1 < n && r.char(i + 1) == '\n' && i != at {
            return i + 1;
        }
        i += 1;
    }
    i.min(n.saturating_sub(1).max(0))
}

fn word_back(e: &Editor, at: usize) -> usize {
    let r = e.rope();
    if at == 0 {
        return 0;
    }
    let mut i = at - 1;
    while i > 0 && class(r.char(i)) == Class::Space {
        i -= 1;
    }
    let c = class(r.char(i));
    while i > 0 && class(r.char(i - 1)) == c {
        i -= 1;
    }
    i
}

fn word_end(e: &Editor, at: usize) -> usize {
    let r = e.rope();
    let n = r.len_chars();
    if n == 0 {
        return 0;
    }
    let mut i = (at + 1).min(n - 1);
    while i < n - 1 && class(r.char(i)) == Class::Space {
        i += 1;
    }
    let c = class(r.char(i));
    while i + 1 < n && class(r.char(i + 1)) == c {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx<'a>() -> Ctx<'a> {
        Ctx { page_rows: 10, clipboard_in: None, clipboard_out: None, line_comment: Some("//"), block_comment: None, indent_unit: "    ", matching_bracket: None, expand: None }
    }

    /// Feed keys; `<esc>` and `<C-r>` are special.
    fn keys(v: &mut Vim, e: &mut Editor, s: &str) {
        let mut rest = s;
        while !rest.is_empty() {
            let (k, len) = if let Some(r) = rest.strip_prefix("<esc>") {
                let _ = r;
                (Key::Esc, 5)
            } else if rest.starts_with("<C-r>") {
                (Key::Redo, 5)
            } else {
                let c = rest.chars().next().unwrap();
                (Key::Char(c), c.len_utf8())
            };
            if v.key(e, k.clone(), &mut cx()).is_none()
                && let Key::Char(c) = k
            {
                e.run(Command::Insert(c.to_string()), &mut cx());
            }
            rest = &rest[len..];
        }
    }

    fn setup(text: &str) -> (Vim, Editor) {
        (Vim::new(), Editor::from_text(text))
    }

    fn at(e: &Editor) -> (usize, usize) {
        let p = e.primary().head;
        (p.line, p.col)
    }

    fn text(e: &Editor) -> String {
        e.rope().to_string()
    }

    #[test]
    fn motions() {
        let (mut v, mut e) = setup("fn main() {\n    let x = 1;\n}\n");
        keys(&mut v, &mut e, "w");
        assert_eq!(at(&e), (0, 3));
        keys(&mut v, &mut e, "e");
        assert_eq!(at(&e), (0, 6));
        keys(&mut v, &mut e, "b");
        assert_eq!(at(&e), (0, 3));
        keys(&mut v, &mut e, "$");
        assert_eq!(at(&e), (0, 10));
        keys(&mut v, &mut e, "0j^");
        assert_eq!(at(&e), (1, 4));
        keys(&mut v, &mut e, "G");
        assert_eq!(at(&e), (3, 0));
        keys(&mut v, &mut e, "gg");
        assert_eq!(at(&e), (0, 0));
        keys(&mut v, &mut e, "2G");
        assert_eq!(at(&e), (1, 4));
        keys(&mut v, &mut e, "3l");
        assert_eq!(at(&e), (1, 7));
        keys(&mut v, &mut e, "kh");
        assert_eq!(at(&e), (0, 6));
    }

    #[test]
    fn insert_and_back() {
        let (mut v, mut e) = setup("ab\n");
        keys(&mut v, &mut e, "ix<esc>");
        assert_eq!((text(&e).as_str(), v.mode()), ("xab\n", Mode::Normal));
        assert_eq!(at(&e), (0, 0));
        keys(&mut v, &mut e, "Ay<esc>");
        assert_eq!(text(&e), "xaby\n");
        keys(&mut v, &mut e, "onew<esc>");
        assert_eq!(text(&e), "xaby\nnew\n");
        keys(&mut v, &mut e, "Otop<esc>");
        assert_eq!(text(&e), "xaby\ntop\nnew\n");
        keys(&mut v, &mut e, "Iz<esc>");
        assert_eq!(text(&e), "xaby\nztop\nnew\n");
    }

    #[test]
    fn delete_change_yank_paste() {
        let (mut v, mut e) = setup("one two three\nfour\nfive\n");
        keys(&mut v, &mut e, "dw");
        assert_eq!(text(&e), "two three\nfour\nfive\n");
        keys(&mut v, &mut e, "cwTWO<esc>");
        assert_eq!(text(&e), "TWO three\nfour\nfive\n");
        // Like Vim, Esc leaves the cursor on the last typed char.
        keys(&mut v, &mut e, "0x");
        assert_eq!(text(&e), "WO three\nfour\nfive\n");
        keys(&mut v, &mut e, "dd");
        assert_eq!(text(&e), "four\nfive\n");
        keys(&mut v, &mut e, "p");
        assert_eq!(text(&e), "four\nWO three\nfive\n");
        assert_eq!(at(&e), (1, 0));
        keys(&mut v, &mut e, "yyjP");
        assert_eq!(text(&e), "four\nWO three\nWO three\nfive\n");
        keys(&mut v, &mut e, "ggD");
        assert_eq!(text(&e), "\nWO three\nWO three\nfive\n");
        keys(&mut v, &mut e, "jwd$");
        assert_eq!(text(&e), "\nWO \nWO three\nfive\n");
        keys(&mut v, &mut e, "jccnew<esc>");
        assert_eq!(text(&e), "\nWO \nnew\nfive\n");
        keys(&mut v, &mut e, "2dd");
        assert_eq!(text(&e), "\nWO \n");
    }

    #[test]
    fn undo_redo_and_dot() {
        let (mut v, mut e) = setup("a b c d\n");
        keys(&mut v, &mut e, "dw");
        assert_eq!(text(&e), "b c d\n");
        keys(&mut v, &mut e, ".");
        assert_eq!(text(&e), "c d\n");
        keys(&mut v, &mut e, "u");
        assert_eq!(text(&e), "b c d\n");
        keys(&mut v, &mut e, "<C-r>");
        assert_eq!(text(&e), "c d\n");
        let (mut v, mut e) = setup("x x x\n");
        keys(&mut v, &mut e, "cwy<esc>w.");
        assert_eq!(text(&e), "y y x\n");
        let (mut v, mut e) = setup("a\nb\n");
        keys(&mut v, &mut e, "Aend<esc>j.");
        assert_eq!(text(&e), "aend\nbend\n");
    }

    #[test]
    fn visual_modes() {
        let (mut v, mut e) = setup("hello world\nsecond\nthird\n");
        keys(&mut v, &mut e, "vey");
        assert_eq!((v.mode(), v.register.0.as_str()), (Mode::Normal, "hello"));
        keys(&mut v, &mut e, "wvld");
        assert_eq!(text(&e), "hello rld\nsecond\nthird\n");
        keys(&mut v, &mut e, "jVjd");
        assert_eq!(text(&e), "hello rld\n");
        let (mut v, mut e) = setup("a\nb\n");
        keys(&mut v, &mut e, "Vj>");
        assert_eq!(text(&e), "    a\n    b\n");
        keys(&mut v, &mut e, "<<");
        assert_eq!(text(&e), "a\n    b\n");
        keys(&mut v, &mut e, "vc!<esc>");
        assert_eq!(text(&e), "!\n    b\n");
    }

    #[test]
    fn dd_at_end_of_file() {
        let (mut v, mut e) = setup("a\nb");
        keys(&mut v, &mut e, "jdd");
        assert_eq!(text(&e), "a");
        let (mut v, mut e) = setup("a\nb\n");
        keys(&mut v, &mut e, "jdd");
        assert_eq!(text(&e), "a\n");
    }

    #[test]
    fn replace_join_counts() {
        let (mut v, mut e) = setup("abc\n  def\nghi\n");
        keys(&mut v, &mut e, "2rx");
        assert_eq!(text(&e), "xxc\n  def\nghi\n");
        keys(&mut v, &mut e, "J");
        assert_eq!(text(&e), "xxc def\nghi\n");
        keys(&mut v, &mut e, "3x");
        assert_eq!(text(&e), "xxcf\nghi\n");
        keys(&mut v, &mut e, "<esc>d<esc>");
        assert_eq!(v.pending_text(), "");
    }
}
