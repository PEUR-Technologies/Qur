//! `Editor::run`: every keymap command.

use std::collections::BTreeSet;

use crate::history::Kind;
use crate::text::{
    self, class, indent_len, line_len, line_string, next_grapheme, prev_grapheme, Class,
};
use crate::{Change, Command, Ctx, Editor, Outcome, Plan, Point, Selection};

const MOVED: Outcome = Outcome { text_changed: false, selections_changed: true, reveal: true };

impl Editor {
    pub(crate) fn run_command(&mut self, cmd: Command, cx: &mut Ctx) -> Outcome {
        use Command::*;
        match cmd {
            Insert(t) => self.insert(&t),
            Newline => self.newline(),
            Tab => self.tab(cx),
            Backtab | Outdent => self.outdent(cx),
            Indent => self.indent(cx),
            Backspace => self.backspace(cx),
            Delete => self.delete_forward(),
            DeleteWordLeft => self.delete_with(|ed, p| ed.word_left(p), true),
            DeleteWordRight => self.delete_with(|ed, p| ed.word_right(p), false),
            DeleteToLineStart => self.delete_with(
                |ed, p| if p.col == 0 { ed.left_of(p) } else { Point { line: p.line, col: 0 } },
                true,
            ),
            DeleteLine => self.delete_lines(),

            Left { select } => self.move_horizontal(select, true),
            Right { select } => self.move_horizontal(select, false),
            Up { select } => self.move_vertical(select, -1),
            Down { select } => self.move_vertical(select, 1),
            PageUp { select } => self.move_vertical(select, -(cx.page_rows.max(1) as isize)),
            PageDown { select } => self.move_vertical(select, cx.page_rows.max(1) as isize),
            WordLeft { select } => self.move_with(select, |ed, s| ed.word_left(s.head)),
            WordRight { select } => self.move_with(select, |ed, s| ed.word_right(s.head)),
            LineStart { select } => self.move_with(select, |ed, s| ed.smart_home(s.head)),
            LineEnd { select } => self.move_with(select, |ed, s| Point {
                line: s.head.line,
                col: line_len(&ed.rope, s.head.line),
            }),
            DocStart { select } => self.move_with(select, |_, _| Point::default()),
            DocEnd { select } => self.move_with(select, |ed, _| ed.end_point()),
            GoToLine(n) => {
                let line = n.saturating_sub(1).min(self.rope.len_lines() - 1);
                self.break_group = true;
                self.set_sels(vec![Selection::caret(Point { line, col: 0 })], 0);
                MOVED
            }

            MoveLineUp => self.move_lines(true),
            MoveLineDown => self.move_lines(false),
            CopyLineUp => self.copy_lines(true),
            CopyLineDown => self.copy_lines(false),
            InsertLineBelow => self.insert_line(true),
            InsertLineAbove => self.insert_line(false),
            ToggleLineComment => self.toggle_line_comment(cx),
            ToggleBlockComment => match cx.block_comment {
                Some((o, c)) => self.toggle_block_comment(o, c),
                None => Outcome::default(),
            },
            TrimTrailingWhitespace => self.trim_trailing_whitespace(),

            SelectAll => {
                self.break_group = true;
                let s = Selection { anchor: Point::default(), head: self.end_point(), goal: None };
                self.set_sels(vec![s], 0);
                MOVED
            }
            SelectLine => self.select_line(),
            SingleCursor => {
                self.break_group = true;
                let p = self.primary();
                let s = if self.sels.len() == 1 { Selection::caret(p.head) } else { p };
                self.set_sels(vec![s], 0);
                MOVED
            }
            AddNextMatch => self.add_next_match(),
            SelectAllMatches => self.select_all_matches(),
            AddCursorAbove => self.add_cursor_vertical(-1),
            AddCursorBelow => self.add_cursor_vertical(1),
            JumpToMatchingBracket => self.jump_to_bracket(cx),

            Copy => {
                cx.clipboard_out = Some(self.copy_text());
                self.last_copy = cx.clipboard_out.clone();
                Outcome::default()
            }
            Cut => {
                let clip = self.copy_text();
                let whole = clip.1;
                cx.clipboard_out = Some(clip.clone());
                self.last_copy = Some(clip);
                if whole { self.delete_lines() } else { self.replace_selections("", Kind::Other) }
            }
            Paste => match cx.clipboard_in {
                Some(clip) => self.paste(clip),
                None => Outcome::default(),
            },

            Undo => self.undo(),
            Redo => self.redo(),
        }
    }

    // --- Positions --------------------------------------------------------

    fn line(&self, l: usize) -> String {
        line_string(&self.rope, l)
    }

    pub(crate) fn left_of(&self, p: Point) -> Point {
        if p.col > 0 {
            Point { line: p.line, col: prev_grapheme(&self.line(p.line), p.col) }
        } else if p.line > 0 {
            Point { line: p.line - 1, col: line_len(&self.rope, p.line - 1) }
        } else {
            p
        }
    }

    fn right_of(&self, p: Point) -> Point {
        if p.col < line_len(&self.rope, p.line) {
            Point { line: p.line, col: next_grapheme(&self.line(p.line), p.col) }
        } else if p.line + 1 < self.rope.len_lines() {
            Point { line: p.line + 1, col: 0 }
        } else {
            p
        }
    }

    pub(crate) fn word_left(&self, p: Point) -> Point {
        if p.col == 0 {
            return self.left_of(p);
        }
        let chars: Vec<char> = self.line(p.line).chars().collect();
        let mut i = p.col.min(chars.len());
        while i > 0 && class(chars[i - 1]) == Class::Space {
            i -= 1;
        }
        if i > 0 {
            let k = class(chars[i - 1]);
            while i > 0 && class(chars[i - 1]) == k {
                i -= 1;
            }
        }
        Point { line: p.line, col: i }
    }

    pub(crate) fn word_right(&self, p: Point) -> Point {
        let chars: Vec<char> = self.line(p.line).chars().collect();
        if p.col >= chars.len() {
            return self.right_of(p);
        }
        let mut i = p.col;
        while i < chars.len() && class(chars[i]) == Class::Space {
            i += 1;
        }
        if i < chars.len() {
            let k = class(chars[i]);
            while i < chars.len() && class(chars[i]) == k {
                i += 1;
            }
        }
        Point { line: p.line, col: i }
    }

    fn smart_home(&self, p: Point) -> Point {
        let fnb = indent_len(&self.line(p.line));
        Point { line: p.line, col: if p.col == fnb { 0 } else { fnb } }
    }

    /// Word (or punctuation run) touching `p`, preferring the one to the right.
    pub(crate) fn word_selection_at(&self, p: Point) -> Option<Selection> {
        let chars: Vec<char> = self.line(p.line).chars().collect();
        let pick = if p.col < chars.len() && class(chars[p.col]) == Class::Word {
            p.col
        } else if p.col > 0 && class(chars[p.col - 1]) == Class::Word {
            p.col - 1
        } else {
            return None;
        };
        let (mut a, mut b) = (pick, pick + 1);
        while a > 0 && class(chars[a - 1]) == Class::Word {
            a -= 1;
        }
        while b < chars.len() && class(chars[b]) == Class::Word {
            b += 1;
        }
        Some(Selection {
            anchor: Point { line: p.line, col: a },
            head: Point { line: p.line, col: b },
            goal: None,
        })
    }

    /// Lines a selection covers (a selection ending at col 0 excludes that line).
    fn sel_lines(s: &Selection) -> (usize, usize) {
        let (a, b) = (s.start(), s.end());
        let last = if b.line > a.line && b.col == 0 { b.line - 1 } else { b.line };
        (a.line, last)
    }

    /// Merged line blocks of all selections; `adjacent` also merges touching blocks.
    fn line_blocks(&self, adjacent: bool) -> Vec<(usize, usize)> {
        let mut v: Vec<(usize, usize)> = self.sels.iter().map(Self::sel_lines).collect();
        v.sort();
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (a, b) in v {
            if let Some(last) = out.last_mut()
                && (a <= last.1 || (adjacent && a == last.1 + 1))
            {
                last.1 = last.1.max(b);
                continue;
            }
            out.push((a, b));
        }
        out
    }

    fn line_set(&self) -> BTreeSet<usize> {
        self.line_blocks(false).into_iter().flat_map(|(a, b)| a..=b).collect()
    }

    fn line_start(&self, l: usize) -> usize {
        self.rope.line_to_char(l)
    }

    fn line_end(&self, l: usize) -> usize {
        self.rope.line_to_char(l) + line_len(&self.rope, l)
    }

    // --- Movement ---------------------------------------------------------

    fn move_with(&mut self, select: bool, f: impl Fn(&Editor, &Selection) -> Point) -> Outcome {
        self.break_group = true;
        let sels = self
            .sels
            .iter()
            .map(|s| {
                let p = f(self, s);
                if select {
                    Selection { anchor: s.anchor, head: p, goal: None }
                } else {
                    Selection::caret(p)
                }
            })
            .collect();
        self.set_sels(sels, self.primary);
        MOVED
    }

    fn move_horizontal(&mut self, select: bool, left: bool) -> Outcome {
        self.move_with(select, |ed, s| {
            if !select && !s.is_empty() {
                // Collapse to the side we're moving towards.
                if left { s.start() } else { s.end() }
            } else if left {
                ed.left_of(s.head)
            } else {
                ed.right_of(s.head)
            }
        })
    }

    /// Head moved `delta` visual rows, keeping the goal column.
    fn vertical(&self, s: &Selection, delta: isize) -> (Point, Option<usize>) {
        let (row, vcol) = self.wrap.point_to_row(&self.rope, s.head);
        let goal = s.goal.unwrap_or(vcol);
        let target = row as isize + delta;
        if target < 0 {
            return (Point::default(), None);
        }
        if target as usize >= self.wrap.row_count() {
            return (self.end_point(), None);
        }
        (self.wrap.row_to_point(&self.rope, target as usize, goal), Some(goal))
    }

    fn move_vertical(&mut self, select: bool, delta: isize) -> Outcome {
        self.break_group = true;
        let sels = self
            .sels
            .iter()
            .map(|s| {
                let (p, goal) = self.vertical(s, delta);
                if select {
                    Selection { anchor: s.anchor, head: p, goal }
                } else {
                    Selection { anchor: p, head: p, goal }
                }
            })
            .collect();
        self.set_sels(sels, self.primary);
        MOVED
    }

    // --- Typing -----------------------------------------------------------

    fn replace_selections(&mut self, text: &str, kind: Kind) -> Outcome {
        let n = text.chars().count() as isize;
        let changes = self
            .sels
            .iter()
            .enumerate()
            .map(|(i, s)| Change::new(self.to_char(s.start()), self.to_char(s.end()), text, n, i))
            .collect();
        self.commit(changes, Plan::Carets, kind)
    }

    fn insert(&mut self, t: &str) -> Outcome {
        let t = text::normalize_newlines(t);
        let kind = if t.chars().count() == 1 && t != "\n" { Kind::Type } else { Kind::Other };
        self.replace_selections(&t, kind)
    }

    fn newline(&mut self) -> Outcome {
        let changes = self
            .sels
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let p = s.start();
                let line = self.line(p.line);
                let ind: String = line.chars().take(indent_len(&line).min(p.col)).collect();
                let text = format!("\n{ind}");
                let n = text.chars().count() as isize;
                Change::new(self.to_char(s.start()), self.to_char(s.end()), text, n, i)
            })
            .collect();
        self.commit(changes, Plan::Carets, Kind::Other)
    }

    fn tab(&mut self, cx: &Ctx) -> Outcome {
        if self.sels.iter().any(|s| s.start().line != s.end().line) {
            return self.indent(cx);
        }
        let unit = cx.indent_unit;
        let changes = self
            .sels
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let text = if unit.starts_with(' ') {
                    let u = unit.len();
                    " ".repeat(u - s.start().col % u)
                } else {
                    unit.to_string()
                };
                let n = text.chars().count() as isize;
                Change::new(self.to_char(s.start()), self.to_char(s.end()), text, n, i)
            })
            .collect();
        self.commit(changes, Plan::Carets, Kind::Other)
    }

    fn indent(&mut self, cx: &Ctx) -> Outcome {
        let changes = self
            .line_set()
            .into_iter()
            .filter(|&l| line_len(&self.rope, l) > 0)
            .map(|l| Change::edit(self.line_start(l), self.line_start(l), cx.indent_unit))
            .collect();
        self.commit(changes, Plan::Map, Kind::Other)
    }

    fn outdent(&mut self, cx: &Ctx) -> Outcome {
        let unit_cols = if cx.indent_unit == "\t" { 4 } else { cx.indent_unit.len().max(1) };
        let changes = self
            .line_set()
            .into_iter()
            .filter_map(|l| {
                let line = self.line(l);
                let n = if line.starts_with('\t') {
                    1
                } else {
                    line.chars().take(unit_cols).take_while(|c| *c == ' ').count()
                };
                (n > 0).then(|| Change::edit(self.line_start(l), self.line_start(l) + n, ""))
            })
            .collect();
        self.commit(changes, Plan::Map, Kind::Other)
    }

    fn backspace(&mut self, cx: &Ctx) -> Outcome {
        let unit = cx.indent_unit;
        let soft_tabs = !unit.is_empty() && unit.chars().all(|c| c == ' ');
        let changes = self
            .sels
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (a, b) = (self.to_char(s.start()), self.to_char(s.end()));
                if a != b {
                    return Change::new(a, b, "", 0, i);
                }
                let p = s.head;
                if p.col == 0 {
                    return if p.line == 0 { Change::new(a, a, "", 0, i) } else { Change::new(a - 1, a, "", 0, i) };
                }
                let line = self.line(p.line);
                let before: String = line.chars().take(p.col).collect();
                let n = if soft_tabs && before.chars().all(|c| c == ' ') {
                    (p.col - 1) % unit.len() + 1
                } else {
                    p.col - prev_grapheme(&line, p.col)
                };
                Change::new(a - n, a, "", 0, i)
            })
            .collect();
        self.commit(changes, Plan::Carets, Kind::Backspace)
    }

    fn delete_forward(&mut self) -> Outcome {
        let changes = self
            .sels
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (a, b) = (self.to_char(s.start()), self.to_char(s.end()));
                if a != b {
                    return Change::new(a, b, "", 0, i);
                }
                let to = self.to_char(self.right_of(s.head));
                Change::new(a, to, "", 0, i)
            })
            .collect();
        self.commit(changes, Plan::Carets, Kind::Other)
    }

    /// Delete each selection, or from the caret to `target(caret)`.
    fn delete_with(&mut self, target: impl Fn(&Editor, Point) -> Point, leftwards: bool) -> Outcome {
        let changes = self
            .sels
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (a, b) = (self.to_char(s.start()), self.to_char(s.end()));
                if a != b {
                    return Change::new(a, b, "", 0, i);
                }
                let t = self.to_char(target(self, s.head));
                if leftwards { Change::new(t, a, "", 0, i) } else { Change::new(a, t, "", 0, i) }
            })
            .collect();
        self.commit(changes, Plan::Carets, Kind::Other)
    }

    fn delete_lines(&mut self) -> Outcome {
        let last = self.rope.len_lines() - 1;
        let head_col = self.primary().head.col;
        let changes = self
            .line_blocks(false)
            .into_iter()
            .map(|(a, b)| {
                if b < last {
                    let keep = head_col.min(line_len(&self.rope, b + 1)) as isize;
                    Change::new(self.line_start(a), self.line_start(b + 1), "", keep, 0)
                } else if a > 0 {
                    let col = head_col.min(line_len(&self.rope, a - 1));
                    let start = self.line_end(a - 1);
                    let caret = (self.line_start(a - 1) + col) as isize - start as isize;
                    Change::new(start, self.line_end(b), "", caret, 0)
                } else {
                    Change::new(0, self.rope.len_chars(), "", 0, 0)
                }
            })
            .collect();
        self.commit(changes, Plan::Carets, Kind::Other)
    }

    // --- Line operations ----------------------------------------------------

    fn move_lines(&mut self, up: bool) -> Outcome {
        let last = self.rope.len_lines() - 1;
        let blocks: Vec<(usize, usize)> = self
            .line_blocks(true)
            .into_iter()
            .filter(|&(a, b)| if up { a > 0 } else { b < last })
            .collect();
        if blocks.is_empty() {
            return Outcome::default();
        }
        let mut changes = Vec::new();
        for &(a, b) in &blocks {
            let body: Vec<String> = (a..=b).map(|l| self.line(l)).collect();
            let body = body.join("\n");
            if up {
                let other = self.line(a - 1);
                changes.push(Change::edit(self.line_start(a - 1), self.line_end(b), format!("{body}\n{other}")));
            } else {
                let other = self.line(b + 1);
                changes.push(Change::edit(self.line_start(a), self.line_end(b + 1), format!("{other}\n{body}")));
            }
        }
        let sels = self
            .sels
            .iter()
            .map(|s| {
                // A selection ending at col 0 of the next line belongs with its block.
                let (a, b) = Self::sel_lines(s);
                let in_block = blocks.iter().any(|&(x, y)| a >= x && b <= y);
                if !in_block {
                    return *s;
                }
                let d = |p: Point| {
                    if up { Point { line: p.line - 1, col: p.col } } else { Point { line: p.line + 1, col: p.col } }
                };
                Selection { anchor: d(s.anchor), head: d(s.head), goal: s.goal }
            })
            .collect();
        self.commit(changes, Plan::Explicit(sels, self.primary), Kind::Other)
    }

    fn copy_lines(&mut self, up: bool) -> Outcome {
        let last = self.rope.len_lines() - 1;
        let blocks = self.line_blocks(false);
        let mut changes = Vec::new();
        for &(a, b) in &blocks {
            let body: Vec<String> = (a..=b).map(|l| self.line(l)).collect();
            let body = body.join("\n");
            if b < last {
                changes.push(Change::edit(self.line_start(b + 1), self.line_start(b + 1), format!("{body}\n")));
            } else {
                changes.push(Change::edit(self.line_end(b), self.line_end(b), format!("\n{body}")));
            }
        }
        // Lines inserted before each block, and the block's own length.
        let mut offset_of = Vec::new();
        let mut cum = 0usize;
        for &(a, b) in &blocks {
            offset_of.push((a, b, cum));
            cum += b - a + 1;
        }
        let sels = self
            .sels
            .iter()
            .map(|s| {
                let (a, _) = Self::sel_lines(s);
                let &(x, y, before) = offset_of.iter().find(|&&(x, y, _)| a >= x && a <= y).unwrap();
                let d = before + if up { 0 } else { y - x + 1 };
                let mv = |p: Point| Point { line: p.line + d, col: p.col };
                Selection { anchor: mv(s.anchor), head: mv(s.head), goal: s.goal }
            })
            .collect();
        self.commit(changes, Plan::Explicit(sels, self.primary), Kind::Other)
    }

    fn insert_line(&mut self, below: bool) -> Outcome {
        let mut seen = BTreeSet::new();
        let mut changes = Vec::new();
        for (i, s) in self.sels.iter().enumerate() {
            let l = s.head.line;
            if !seen.insert(l) {
                continue;
            }
            let line = self.line(l);
            let ind: String = line.chars().take(indent_len(&line)).collect();
            let n = ind.chars().count() as isize;
            if below {
                changes.push(Change::new(self.line_end(l), self.line_end(l), format!("\n{ind}"), n + 1, i));
            } else {
                changes.push(Change::new(self.line_start(l), self.line_start(l), format!("{ind}\n"), n, i));
            }
        }
        self.commit(changes, Plan::Carets, Kind::Other)
    }

    fn toggle_line_comment(&mut self, cx: &Ctx) -> Outcome {
        let Some(prefix) = cx.line_comment else {
            return match cx.block_comment {
                Some((o, c)) => self.block_comment_lines(o, c),
                None => Outcome::default(),
            };
        };
        let lines: Vec<(usize, String)> = self.line_set().into_iter().map(|l| (l, self.line(l))).collect();
        let mut targets: Vec<&(usize, String)> = lines.iter().filter(|(_, s)| !s.trim().is_empty()).collect();
        if targets.is_empty() {
            targets = lines.iter().collect();
        }
        let all_commented = targets.iter().all(|(_, s)| s.trim_start().starts_with(prefix));
        let plen = prefix.chars().count();
        let changes = if all_commented {
            targets
                .iter()
                .map(|(l, s)| {
                    let at = self.line_start(*l) + indent_len(s);
                    let rest: String = s.chars().skip(indent_len(s) + plen).take(1).collect();
                    let n = plen + usize::from(rest == " ");
                    Change::edit(at, at + n, "")
                })
                .collect()
        } else {
            let min = targets.iter().map(|(_, s)| indent_len(s)).min().unwrap_or(0);
            targets
                .iter()
                .map(|(l, s)| {
                    let at = self.line_start(*l) + min.min(s.chars().count());
                    Change::edit(at, at, format!("{prefix} "))
                })
                .collect()
        };
        self.commit(changes, Plan::Map, Kind::Other)
    }

    /// Languages without line comments: wrap each line block in a block comment.
    fn block_comment_lines(&mut self, open: &str, close: &str) -> Outcome {
        let blocks = self.line_blocks(false);
        let sels = blocks
            .iter()
            .map(|&(a, b)| {
                let s = self.line(a);
                Selection {
                    anchor: Point { line: a, col: indent_len(&s) },
                    head: Point { line: b, col: line_len(&self.rope, b) },
                    goal: None,
                }
            })
            .collect();
        let saved = (self.sels.clone(), self.primary);
        self.sels = sels;
        self.primary = 0;
        let out = self.toggle_block_comment(open, close);
        if !out.text_changed {
            self.sels = saved.0;
            self.primary = saved.1;
        }
        out
    }

    fn toggle_block_comment(&mut self, open: &str, close: &str) -> Outcome {
        let (ol, cl) = (open.chars().count(), close.chars().count());
        if self.sels.iter().all(Selection::is_empty) {
            let text = format!("{open}  {close}");
            let caret = (ol + 1) as isize;
            let changes = self
                .sels
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let a = self.to_char(s.head);
                    Change::new(a, a, text.clone(), caret, i)
                })
                .collect();
            return self.commit(changes, Plan::Carets, Kind::Other);
        }
        let mut changes = Vec::new();
        for s in self.sels.iter().filter(|s| !s.is_empty()) {
            let (a, b) = (self.to_char(s.start()), self.to_char(s.end()));
            let t = self.rope.slice(a..b).to_string();
            let lead = t.chars().count() - t.trim_start().chars().count();
            let trail = t.chars().count() - t.trim_end().chars().count();
            let inner = t.trim();
            if inner.starts_with(open) && inner.ends_with(close) && inner.chars().count() >= ol + cl {
                let o_at = a + lead;
                let c_end = b - trail;
                let chars: Vec<char> = inner.chars().collect();
                let o_n = ol + usize::from(chars.get(ol) == Some(&' ') && chars.len() > ol + cl);
                let c_n = cl + usize::from(chars.len() > o_n + cl && chars[chars.len() - cl - 1] == ' ');
                changes.push(Change::edit(o_at, o_at + o_n, ""));
                changes.push(Change::edit(c_end - c_n, c_end, ""));
            } else {
                changes.push(Change::edit(a, a, format!("{open} ")));
                changes.push(Change::edit(b, b, format!(" {close}")));
            }
        }
        self.commit(changes, Plan::Map, Kind::Other)
    }

    fn trim_trailing_whitespace(&mut self) -> Outcome {
        let mut changes = Vec::new();
        let mut buf = String::new();
        for l in 0..self.rope.len_lines() {
            text::line_into(&self.rope, l, &mut buf);
            let kept = buf.trim_end_matches([' ', '\t']).chars().count();
            let total = buf.chars().count();
            if kept < total {
                let s = self.line_start(l);
                changes.push(Change::edit(s + kept, s + total, ""));
            }
        }
        if changes.is_empty() {
            return Outcome::default();
        }
        self.commit(changes, Plan::Map, Kind::Other)
    }

    // --- Selection ----------------------------------------------------------

    fn select_line(&mut self) -> Outcome {
        self.break_group = true;
        let last = self.rope.len_lines() - 1;
        let next_start = |l: usize| -> Point {
            if l < last { Point { line: l + 1, col: 0 } } else { Point { line: last, col: line_len(&self.rope, last) } }
        };
        let sels = self
            .sels
            .iter()
            .map(|s| {
                let (a, b) = Self::sel_lines(s);
                let full = s.start() == Point { line: a, col: 0 } && s.end() == next_start(b) && !s.is_empty();
                let end = if full { next_start((b + 1).min(last)) } else { next_start(b) };
                Selection { anchor: Point { line: a, col: 0 }, head: end, goal: None }
            })
            .collect();
        self.set_sels(sels, self.primary);
        MOVED
    }

    /// All non-overlapping occurrences of `needle` (case-sensitive), as char ranges.
    fn occurrences(&self, needle: &str) -> Vec<(usize, usize)> {
        if needle.is_empty() {
            return Vec::new();
        }
        let hay = self.rope.to_string();
        let n = needle.chars().count();
        let (mut last_b, mut last_c) = (0usize, 0usize);
        hay.match_indices(needle)
            .map(|(b, _)| {
                last_c += hay[last_b..b].chars().count();
                last_b = b;
                (last_c, last_c + n)
            })
            .collect()
    }

    fn add_next_match(&mut self) -> Outcome {
        self.break_group = true;
        if self.sels.iter().all(Selection::is_empty) {
            let sels: Vec<Selection> =
                self.sels.iter().map(|s| self.word_selection_at(s.head).unwrap_or(*s)).collect();
            self.set_sels(sels, self.primary);
            return MOVED;
        }
        let p = self.primary();
        let needle = self.text_of(&p);
        let taken: Vec<(usize, usize)> =
            self.sels.iter().map(|s| (self.to_char(s.start()), self.to_char(s.end()))).collect();
        let from = self.to_char(p.end());
        let occ = self.occurrences(&needle);
        let next = occ
            .iter()
            .filter(|o| o.0 >= from)
            .chain(occ.iter().filter(|o| o.0 < from))
            .find(|o| !taken.iter().any(|t| t.0 < o.1 && o.0 < t.1));
        let Some(&(a, b)) = next else { return Outcome::default() };
        let mut sels = self.sels.clone();
        sels.push(Selection { anchor: self.to_point(a), head: self.to_point(b), goal: None });
        let n = sels.len();
        self.set_sels(sels, n - 1);
        MOVED
    }

    fn select_all_matches(&mut self) -> Outcome {
        self.break_group = true;
        let mut p = self.primary();
        if p.is_empty() {
            match self.word_selection_at(p.head) {
                Some(w) => p = w,
                None => return Outcome::default(),
            }
        }
        let needle = self.text_of(&p);
        let origin = self.to_char(p.start());
        let occ = self.occurrences(&needle);
        if occ.is_empty() {
            return Outcome::default();
        }
        let primary = occ.iter().position(|o| o.0 == origin).unwrap_or(0);
        let sels = occ
            .iter()
            .map(|&(a, b)| Selection { anchor: self.to_point(a), head: self.to_point(b), goal: None })
            .collect();
        self.set_sels(sels, primary);
        MOVED
    }

    fn add_cursor_vertical(&mut self, delta: isize) -> Outcome {
        self.break_group = true;
        let edge = if delta < 0 {
            self.sels.iter().min_by_key(|s| s.head)
        } else {
            self.sels.iter().max_by_key(|s| s.head)
        };
        let Some(edge) = edge.copied() else { return Outcome::default() };
        let (row, _) = self.wrap.point_to_row(&self.rope, edge.head);
        if (delta < 0 && row == 0) || (delta > 0 && row + 1 >= self.wrap.row_count()) {
            return Outcome::default();
        }
        let (p, goal) = self.vertical(&edge, delta);
        let mut sels = self.sels.clone();
        sels.push(Selection { anchor: p, head: p, goal });
        let n = sels.len();
        self.set_sels(sels, n - 1);
        MOVED
    }

    fn jump_to_bracket(&mut self, cx: &Ctx) -> Outcome {
        self.break_group = true;
        let sels = self
            .sels
            .iter()
            .map(|s| {
                let c = self.to_char(s.head);
                let found = [c, c.saturating_sub(1)].into_iter().find_map(|at| {
                    if at >= self.rope.len_chars() {
                        return None;
                    }
                    match cx.matching_bracket {
                        Some(f) => f(self.rope.char_to_byte(at)).map(|r| self.rope.byte_to_char(r.start)),
                        None => self.scan_bracket(at),
                    }
                });
                found.map_or(*s, |m| Selection::caret(self.to_point(m)))
            })
            .collect();
        self.set_sels(sels, self.primary);
        MOVED
    }

    /// Plain-text bracket matching (no syntax awareness) for char at `at`.
    fn scan_bracket(&self, at: usize) -> Option<usize> {
        const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];
        let c = self.rope.char(at);
        if let Some(&(o, cl)) = PAIRS.iter().find(|p| p.0 == c) {
            let mut depth = 0usize;
            for (i, ch) in self.rope.chars_at(at).enumerate() {
                if ch == o {
                    depth += 1;
                } else if ch == cl {
                    depth -= 1;
                    if depth == 0 {
                        return Some(at + i);
                    }
                }
            }
        } else if let Some(&(o, cl)) = PAIRS.iter().find(|p| p.1 == c) {
            let mut depth = 0usize;
            let mut it = self.rope.chars_at(at + 1);
            let mut i = at + 1;
            while let Some(ch) = it.prev() {
                i -= 1;
                if ch == cl {
                    depth += 1;
                } else if ch == o {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
            }
        }
        None
    }

    // --- Clipboard ----------------------------------------------------------

    /// (text, whole_line). With every selection empty, copies whole lines.
    fn copy_text(&self) -> (String, bool) {
        if self.sels.iter().all(Selection::is_empty) {
            let mut lines = BTreeSet::new();
            let mut out = String::new();
            for s in &self.sels {
                if lines.insert(s.head.line) {
                    out.push_str(&self.line(s.head.line));
                    out.push('\n');
                }
            }
            (out, true)
        } else {
            (self.selected_text(), false)
        }
    }

    fn paste(&mut self, clip: &str) -> Outcome {
        let clip = text::normalize_newlines(clip).into_owned();
        let whole = self.last_copy.as_ref().is_some_and(|(t, w)| *w && *t == clip);
        let n = self.sels.len();
        let pieces: Vec<String> = if whole {
            let lines: Vec<&str> = clip.split_inclusive('\n').collect();
            if n > 1 && lines.len() == n {
                lines.iter().map(|s| s.to_string()).collect()
            } else {
                vec![clip.clone(); n]
            }
        } else {
            let parts: Vec<&str> = clip.split('\n').collect();
            if n > 1 && parts.len() == n {
                parts.iter().map(|s| s.to_string()).collect()
            } else {
                vec![clip.clone(); n]
            }
        };
        let changes = self
            .sels
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let piece = &pieces[i];
                let len = piece.chars().count() as isize;
                if whole && s.is_empty() {
                    // Whole-line paste goes above the current line; caret keeps its column.
                    let at = self.line_start(s.head.line);
                    Change::new(at, at, piece.clone(), len + s.head.col as isize, i)
                } else {
                    Change::new(self.to_char(s.start()), self.to_char(s.end()), piece.clone(), len, i)
                }
            })
            .collect();
        self.commit(changes, Plan::Carets, Kind::Other)
    }
}
