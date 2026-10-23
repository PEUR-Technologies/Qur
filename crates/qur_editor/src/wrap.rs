//! Soft wrap: buffer lines -> visual rows (monospace columns).

use std::ops::Range;

use ropey::Rope;

use crate::text::{char_width, line_into, line_len};
use crate::{Edit, LineWrap, Point, VisualRow, WrapMap};

impl WrapMap {
    pub(crate) fn build(rope: &Rope, width: Option<usize>, tab_width: usize) -> WrapMap {
        let mut m = WrapMap {
            width,
            tab_width: tab_width.max(1),
            breaks: Vec::new(),
            row_start: Vec::new(),
        };
        m.rewrap_all(rope);
        m
    }

    fn rewrap_all(&mut self, rope: &Rope) {
        let n = rope.len_lines();
        self.breaks.clear();
        self.breaks.reserve(n);
        let mut buf = String::new();
        let mut chars = Vec::new();
        for l in 0..n {
            let b = self.wrap_line(rope, l, &mut buf, &mut chars);
            self.breaks.push(Some(b));
        }
        self.rebuild_prefix();
    }

    /// Char offsets where rows 2.. of line `l` start (empty: one row).
    fn wrap_line(&self, rope: &Rope, l: usize, buf: &mut String, chars: &mut Vec<char>) -> LineWrap {
        let n = line_len(rope, l);
        let lw = |breaks| LineWrap { len: n as u32, breaks };
        let Some(width) = self.width else { return lw(Vec::new()) };
        let width = width.max(1);
        // Fast path: a line can't wrap if even its widest possible chars fit.
        if n * 2 <= width && !rope.line(l).chars().any(|c| c == '\t') {
            return lw(Vec::new());
        }
        line_into(rope, l, buf);
        chars.clear();
        chars.extend(buf.chars());
        let tab = self.tab_width;
        let mut out = Vec::new();
        let mut row_start = 0usize;
        let mut col = 0usize;
        let mut last_break: Option<usize> = None;
        let mut i = 0usize;
        while i < chars.len() {
            let c = chars[i];
            let w = char_width(c, col, tab);
            // Whitespace may hang past the edge; anything else wraps.
            if !c.is_whitespace() && col + w > width && i > row_start {
                let brk = match last_break {
                    Some(b) if b > row_start && b <= i => b,
                    _ => i,
                };
                out.push(brk as u32);
                row_start = brk;
                last_break = None;
                col = 0;
                for &c2 in &chars[brk..i] {
                    col += char_width(c2, col, tab);
                }
                continue; // re-measure chars[i] on the new row
            }
            col += w;
            if c.is_whitespace() {
                last_break = Some(i + 1);
            }
            i += 1;
        }
        lw(out)
    }

    fn rebuild_prefix(&mut self) {
        self.row_start.clear();
        self.row_start.reserve(self.breaks.len() + 1);
        let mut acc = 0usize;
        for b in &self.breaks {
            self.row_start.push(acc);
            acc += 1 + b.as_ref().map_or(0, |w| w.breaks.len());
        }
        self.row_start.push(acc);
    }

    fn line_breaks(&self, l: usize) -> &[u32] {
        self.breaks.get(l).and_then(|b| b.as_ref()).map_or(&[], |w| &w.breaks)
    }

    fn line_chars(&self, l: usize) -> usize {
        self.breaks.get(l).and_then(|b| b.as_ref()).map_or(0, |w| w.len as usize)
    }

    /// Char range of segment `seg` of line `l`.
    fn segment(&self, l: usize, seg: usize) -> Range<usize> {
        let b = self.line_breaks(l);
        let start = if seg == 0 { 0 } else { b[seg - 1] as usize };
        let end = if seg < b.len() { b[seg] as usize } else { self.line_chars(l) };
        start..end
    }

    /// Visual width of chars `range` of line `l`.
    fn width_of(&self, rope: &Rope, l: usize, range: Range<usize>) -> usize {
        let mut col = 0;
        for c in rope.line(l).chars().skip(range.start).take(range.len()) {
            col += char_width(c, col, self.tab_width);
        }
        col
    }

    /// Line containing visual `row` (clamped).
    fn line_of_row(&self, row: usize) -> usize {
        let n = self.breaks.len();
        let idx = self.row_start[..n].partition_point(|&s| s <= row);
        idx.saturating_sub(1).min(n.saturating_sub(1))
    }

    pub(crate) fn width(&self) -> Option<usize> {
        self.width
    }
}

impl WrapMap {
    pub(crate) fn apply_edits_impl(&mut self, rope: &Rope, edits: &[Edit]) {
        if edits.is_empty() {
            return;
        }
        for e in edits {
            splice_lines(&mut self.breaks, e);
        }
        debug_assert_eq!(self.breaks.len(), rope.len_lines());
        let mut buf = String::new();
        let mut chars = Vec::new();
        for l in 0..self.breaks.len() {
            if self.breaks[l].is_none() {
                let b = self.wrap_line(rope, l, &mut buf, &mut chars);
                self.breaks[l] = Some(b);
            }
        }
        self.rebuild_prefix();
    }

    pub(crate) fn set_width_impl(&mut self, rope: &Rope, width: Option<usize>) {
        self.width = width;
        self.rewrap_all(rope);
    }

    pub(crate) fn row_count_impl(&self) -> usize {
        *self.row_start.last().unwrap_or(&0)
    }

    pub(crate) fn rows_impl(&self, range: Range<usize>) -> Vec<VisualRow> {
        let total = self.row_count_impl();
        let end = range.end.min(total);
        if range.start >= end {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(end - range.start);
        let mut l = self.line_of_row(range.start);
        let mut seg = range.start - self.row_start[l];
        let mut row = range.start;
        while row < end {
            let nb = self.line_breaks(l).len();
            out.push(VisualRow { line: l, chars: self.segment(l, seg), first: seg == 0 });
            row += 1;
            if seg < nb {
                seg += 1;
            } else {
                l += 1;
                seg = 0;
            }
        }
        out
    }

    pub(crate) fn point_to_row_impl(&self, rope: &Rope, p: Point) -> (usize, usize) {
        let l = p.line.min(self.breaks.len().saturating_sub(1));
        let b = self.line_breaks(l);
        let seg = b.partition_point(|&x| (x as usize) <= p.col);
        let start = if seg == 0 { 0 } else { b[seg - 1] as usize };
        let row = self.row_start[l] + seg;
        (row, self.width_of(rope, l, start..p.col.max(start)))
    }

    pub(crate) fn row_to_point_impl(&self, rope: &Rope, row: usize, vcol: usize) -> Point {
        let total = self.row_count_impl();
        if total == 0 {
            return Point::default();
        }
        let row = row.min(total - 1);
        let l = self.line_of_row(row);
        let seg = row - self.row_start[l];
        let r = self.segment(l, seg);
        let is_last = seg == self.line_breaks(l).len();
        let mut acc = 0usize;
        let mut col = r.start;
        for c in rope.line(l).chars().skip(r.start).take(r.len()) {
            let w = char_width(c, acc, self.tab_width);
            if acc + w > vcol {
                // Snap to the nearer edge of the char.
                if vcol - acc > w / 2 && w > 1 {
                    col += 1;
                }
                return Point { line: l, col };
            }
            acc += w;
            col += 1;
        }
        // Past the end of the row: a wrapped row ends before its break char.
        let col = if !is_last && r.end > r.start { r.end - 1 } else { r.end };
        Point { line: l, col }
    }
}

/// Replace the per-line entries an edit touched with `None` (stale).
pub(crate) fn splice_lines<T>(lines: &mut Vec<Option<T>>, e: &Edit) {
    let start = e.start_point.0;
    let old_end = (e.old_end_point.0 + 1).min(lines.len());
    let new_count = e.new_end_point.0 - start + 1;
    let start = start.min(lines.len());
    lines.splice(start..old_end.max(start), (0..new_count).map(|_| None));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, width: usize) -> Vec<String> {
        let rope = Rope::from_str(text);
        let m = WrapMap::build(&rope, Some(width), 4);
        m.rows_impl(0..usize::MAX)
            .into_iter()
            .map(|r| {
                let s: String = rope.line(r.line).chars().filter(|c| *c != '\n').collect();
                s.chars().skip(r.chars.start).take(r.chars.len()).collect()
            })
            .collect()
    }

    #[test]
    fn wraps_at_word_boundaries() {
        assert_eq!(rows("hello brave new world", 10), vec!["hello ", "brave new ", "world"]);
        // Trailing whitespace may hang past the edge.
        assert_eq!(rows("hello brave new world", 11), vec!["hello brave ", "new world"]);
    }

    #[test]
    fn hard_breaks_long_words() {
        assert_eq!(rows("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn no_wrap_when_disabled_or_fits() {
        let rope = Rope::from_str("short\nlines\n");
        let m = WrapMap::build(&rope, None, 4);
        assert_eq!(m.row_count_impl(), 3);
        let m = WrapMap::build(&rope, Some(80), 4);
        assert_eq!(m.row_count_impl(), 3);
    }

    #[test]
    fn wide_chars_count_two_columns() {
        // 5 CJK chars = 10 columns; width 6 -> 3 + 2 chars.
        assert_eq!(rows("日本語漢字", 6), vec!["日本語", "漢字"]);
    }

    #[test]
    fn tabs_expand_to_stops() {
        let rope = Rope::from_str("\tab");
        let m = WrapMap::build(&rope, Some(80), 4);
        assert_eq!(m.point_to_row_impl(&rope, Point { line: 0, col: 1 }), (0, 4));
        assert_eq!(m.point_to_row_impl(&rope, Point { line: 0, col: 2 }), (0, 5));
    }

    #[test]
    fn point_row_round_trip() {
        let rope = Rope::from_str("hello brave new world\nnext");
        let m = WrapMap::build(&rope, Some(10), 4);
        assert_eq!(m.row_count_impl(), 4);
        // "world" starts at col 16 on row 2.
        assert_eq!(m.point_to_row_impl(&rope, Point { line: 0, col: 16 }), (2, 0));
        assert_eq!(m.point_to_row_impl(&rope, Point { line: 0, col: 18 }), (2, 2));
        assert_eq!(m.row_to_point_impl(&rope, 2, 2), Point { line: 0, col: 18 });
        assert_eq!(m.row_to_point_impl(&rope, 3, 99), Point { line: 1, col: 4 });
        // Past the end of a wrapped row clamps before the break.
        assert_eq!(m.row_to_point_impl(&rope, 0, 99), Point { line: 0, col: 5 });
        let rows = m.rows_impl(0..10);
        assert_eq!(rows.iter().filter(|r| r.first).count(), 2);
    }
}
