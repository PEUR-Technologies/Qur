//! Small helpers over the rope and single-line strings.

use ropey::Rope;
use unicode_segmentation::GraphemeCursor;
use unicode_width::UnicodeWidthChar;

/// Chars in line `l`, excluding the trailing '\n'.
pub fn line_len(rope: &Rope, l: usize) -> usize {
    let line = rope.line(l);
    let n = line.len_chars();
    if n > 0 && line.char(n - 1) == '\n' { n - 1 } else { n }
}

/// Line `l` without its trailing '\n'.
pub fn line_string(rope: &Rope, l: usize) -> String {
    let mut s = String::new();
    for chunk in rope.line(l).chunks() {
        s.push_str(chunk);
    }
    if s.ends_with('\n') {
        s.pop();
    }
    s
}

/// Fills `buf` with line `l` (without '\n'); avoids an allocation per line.
pub fn line_into(rope: &Rope, l: usize, buf: &mut String) {
    buf.clear();
    for chunk in rope.line(l).chunks() {
        buf.push_str(chunk);
    }
    if buf.ends_with('\n') {
        buf.pop();
    }
}

/// (row, byte column) of a byte offset, as tree-sitter wants.
pub fn byte_point(rope: &Rope, byte: usize) -> (usize, usize) {
    let line = rope.byte_to_line(byte);
    (line, byte - rope.line_to_byte(line))
}

/// Point reached after inserting `text` at `start` (row, byte col).
pub fn advance_point(start: (usize, usize), text: &str) -> (usize, usize) {
    match text.rfind('\n') {
        None => (start.0, start.1 + text.len()),
        Some(i) => (start.0 + text.matches('\n').count(), text.len() - i - 1),
    }
}

pub fn char_to_byte(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(b, _)| b).unwrap_or(s.len())
}

pub fn byte_to_char(s: &str, byte: usize) -> usize {
    s[..byte].chars().count()
}

/// Previous grapheme boundary (in chars) before `col` within `line`.
pub fn prev_grapheme(line: &str, col: usize) -> usize {
    let b = char_to_byte(line, col);
    let mut c = GraphemeCursor::new(b, line.len(), true);
    match c.prev_boundary(line, 0) {
        Ok(Some(p)) => byte_to_char(line, p),
        _ => col.saturating_sub(1),
    }
}

/// Next grapheme boundary (in chars) after `col` within `line`.
pub fn next_grapheme(line: &str, col: usize) -> usize {
    let b = char_to_byte(line, col);
    let mut c = GraphemeCursor::new(b, line.len(), true);
    match c.next_boundary(line, 0) {
        Ok(Some(p)) => byte_to_char(line, p),
        _ => col + 1,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Word,
    Space,
    Punct,
}

pub fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Punct
    }
}

pub fn is_word(c: char) -> bool {
    class(c) == Class::Word
}

/// Leading whitespace of a line, in chars.
pub fn indent_len(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// Display width of `c` at visual column `col` (tabs expand to the next stop).
pub fn char_width(c: char, col: usize, tab_width: usize) -> usize {
    if c == '\t' {
        let t = tab_width.max(1);
        t - col % t
    } else {
        c.width().unwrap_or(1)
    }
}

/// Normalize line endings to '\n'.
pub fn normalize_newlines(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains('\r') {
        std::borrow::Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}
