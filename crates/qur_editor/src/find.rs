//! Find state with a per-line match cache. Matches never span lines (regex
//! multi-line mode is off), so an edit only invalidates the lines it touched,
//! and the viewport only computes the lines it shows.

use std::cell::{Cell, RefCell};

use regex::{Regex, RegexBuilder};
use ropey::Rope;

use crate::text::{is_word, line_into};
use crate::wrap::splice_lines;
use crate::{Edit, FindOptions};

/// Char-column ranges of the matches on one line.
type LineMatches = Vec<(u32, u32)>;

pub(crate) struct FindState {
    pub(crate) opts: FindOptions,
    re: Regex,
    /// Per line: char-column ranges of matches; `None` = not computed yet.
    lines: RefCell<Vec<Option<LineMatches>>>,
    total: Cell<Option<usize>>,
}

impl FindState {
    pub(crate) fn new(query: &str, opts: FindOptions, line_count: usize) -> Result<FindState, String> {
        let pattern = if opts.regex { query.to_string() } else { regex::escape(query) };
        let re = RegexBuilder::new(&pattern)
            .case_insensitive(!opts.case_sensitive)
            .multi_line(false)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(FindState {
            opts,
            re,
            lines: RefCell::new(vec![None; line_count]),
            total: Cell::new(None),
        })
    }

    pub(crate) fn regex(&self) -> &Regex {
        &self.re
    }

    pub(crate) fn apply_edits(&mut self, edits: &[Edit]) {
        let lines = self.lines.get_mut();
        for e in edits {
            splice_lines(lines, e);
        }
        self.total.set(None);
    }

    fn compute(&self, rope: &Rope, l: usize, buf: &mut String) -> Vec<(u32, u32)> {
        line_into(rope, l, buf);
        let mut out = Vec::new();
        let (mut last_b, mut last_c) = (0usize, 0usize);
        for m in self.re.find_iter(buf) {
            if m.start() == m.end() {
                continue;
            }
            if self.opts.whole_word {
                let before = buf[..m.start()].chars().next_back();
                let after = buf[m.end()..].chars().next();
                if before.is_some_and(is_word) || after.is_some_and(is_word) {
                    continue;
                }
            }
            let s = last_c + buf[last_b..m.start()].chars().count();
            let e = s + buf[m.start()..m.end()].chars().count();
            (last_b, last_c) = (m.end(), e);
            out.push((s as u32, e as u32));
        }
        out
    }

    /// Matches on line `l` (computed and cached on demand).
    pub(crate) fn line(&self, rope: &Rope, l: usize, buf: &mut String) -> Vec<(u32, u32)> {
        let mut lines = self.lines.borrow_mut();
        if l >= lines.len() {
            return Vec::new();
        }
        if lines[l].is_none() {
            lines[l] = Some(self.compute(rope, l, buf));
        }
        lines[l].clone().unwrap_or_default()
    }

    pub(crate) fn total(&self, rope: &Rope) -> usize {
        if let Some(t) = self.total.get() {
            return t;
        }
        let mut buf = String::new();
        let n = self.lines.borrow().len();
        let mut t = 0;
        for l in 0..n {
            t += self.line(rope, l, &mut buf).len();
        }
        self.total.set(Some(t));
        t
    }
}
