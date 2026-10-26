//! Large files, read from disk a page at a time and never whole: a sparse
//! index of line starts (one entry every `LINES_PER_ENTRY` lines or
//! `BYTES_PER_ENTRY` bytes, whichever comes first), reading the lines a view
//! shows, and searching by streaming through the file. UI-free; the app runs
//! the index and searches off the main thread.

use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

pub use regex::bytes::Regex;

pub const LINES_PER_ENTRY: u64 = 1024;
pub const BYTES_PER_ENTRY: u64 = 256 << 10;
/// Bytes of one line kept for display (and searched); the rest is skipped.
pub const MAX_LINE: usize = 16 << 10;
/// Most bytes one `read_lines` call reads (a file with giant lines).
pub const READ_BUDGET: usize = 8 << 20;
const CHUNK: usize = 1 << 20;

/// Line starts seen so far: (line number, byte offset) pairs in order,
/// starting with (0, 0).
#[derive(Clone, Debug)]
pub struct LineIndex {
    pub entries: Vec<(u64, u64)>,
    /// Lines found so far (newlines + 1 once done, like the editor counts).
    pub lines: u64,
    /// Bytes scanned.
    pub scanned: u64,
    pub done: bool,
    last: (u64, u64),
}

impl Default for LineIndex {
    fn default() -> Self {
        LineIndex { entries: vec![(0, 0)], lines: 1, scanned: 0, done: false, last: (0, 0) }
    }
}

impl LineIndex {
    /// Take the next chunk of the file (in order).
    pub fn feed(&mut self, chunk: &[u8]) {
        let base = self.scanned;
        for i in memchr::memchr_iter(b'\n', chunk) {
            // The line after this newline.
            let (line, start) = (self.lines, base + i as u64 + 1);
            self.lines += 1;
            if line - self.last.0 >= LINES_PER_ENTRY || start - self.last.1 >= BYTES_PER_ENTRY {
                self.last = (line, start);
                self.entries.push(self.last);
            }
        }
        self.scanned += chunk.len() as u64;
    }

    /// The indexed line start at or before `line`.
    pub fn entry_for(&self, line: u64) -> (u64, u64) {
        let i = self.entries.partition_point(|e| e.0 <= line);
        self.entries[i.saturating_sub(1)]
    }

    /// The index entry at or before byte `offset`.
    pub fn entry_before_offset(&self, offset: u64) -> usize {
        self.entries.partition_point(|e| e.1 <= offset).saturating_sub(1)
    }
}

/// Index a whole file, `progress` after each chunk (false stops early).
pub fn index_file(mut r: impl Read, mut progress: impl FnMut(&[u8]) -> bool) -> io::Result<()> {
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = read_full(&mut r, &mut buf)?;
        if n == 0 || !progress(&buf[..n]) {
            return Ok(());
        }
    }
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// One line read for display: its bytes (at most `MAX_LINE`, no newline or
/// CR) and whether it was cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub bytes: Vec<u8>,
    pub cut: bool,
}

/// Walks lines from a known line start, a chunk at a time.
struct Lines<'a, R> {
    r: &'a mut R,
    buf: Vec<u8>,
    pos: usize,
    len: usize,
    eof: bool,
    read: usize,
    /// Byte offset of the next line's start.
    offset: u64,
}

impl<'a, R: Read> Lines<'a, R> {
    fn new(r: &'a mut R, offset: u64) -> Self {
        Lines { r, buf: vec![0u8; CHUNK.min(256 << 10)], pos: 0, len: 0, eof: false, read: 0, offset }
    }

    fn fill(&mut self) -> io::Result<bool> {
        if self.pos < self.len {
            return Ok(true);
        }
        if self.eof {
            return Ok(false);
        }
        self.len = read_full(self.r, &mut self.buf)?;
        self.pos = 0;
        self.read += self.len;
        if self.len < self.buf.len() {
            self.eof = true;
        }
        Ok(self.len > 0)
    }

    /// The next line into `out` (capped at `cap` bytes), None at the end.
    /// `Some(true)` when the line ended with a newline.
    fn next_into(&mut self, out: &mut Vec<u8>, cap: usize, cut: &mut bool) -> io::Result<Option<bool>> {
        out.clear();
        *cut = false;
        let mut any = false;
        loop {
            if !self.fill()? {
                return Ok(any.then_some(false));
            }
            any = true;
            let rest = &self.buf[self.pos..self.len];
            let (take, nl) = match memchr::memchr(b'\n', rest) {
                Some(i) => (i, true),
                None => (rest.len(), false),
            };
            let room = cap.saturating_sub(out.len());
            out.extend_from_slice(&rest[..take.min(room)]);
            *cut |= take > room;
            self.pos += take + usize::from(nl);
            self.offset += (take + usize::from(nl)) as u64;
            if nl {
                if out.last() == Some(&b'\r') {
                    out.pop();
                }
                return Ok(Some(true));
            }
        }
    }
}

/// Read `count` lines from line `first`, seeking to the nearest indexed line
/// start. Stops early at the end of the file or after `READ_BUDGET` bytes.
pub fn read_lines<R: Read + Seek>(r: &mut R, index: &LineIndex, first: u64, count: usize) -> io::Result<Vec<Line>> {
    let (line0, off) = index.entry_for(first);
    r.seek(SeekFrom::Start(off))?;
    let mut it = Lines::new(r, off);
    let mut out = Vec::with_capacity(count);
    let mut buf = Vec::new();
    let mut cut = false;
    let mut line = line0;
    while out.len() < count && it.read <= READ_BUDGET {
        let keep = line >= first;
        let cap = if keep { MAX_LINE } else { 0 };
        match it.next_into(&mut buf, cap, &mut cut)? {
            Some(nl) => {
                if keep {
                    out.push(Line { bytes: std::mem::take(&mut buf), cut });
                }
                if !nl {
                    break;
                }
            }
            None => {
                // The file ends with a newline: the last line is empty.
                if keep && line == index.lines.saturating_sub(1) && index.done {
                    out.push(Line { bytes: Vec::new(), cut: false });
                }
                break;
            }
        }
        line += 1;
    }
    Ok(out)
}

/// Byte offset where line `line` starts (scanning from the nearest entry).
pub fn line_offset<R: Read + Seek>(r: &mut R, index: &LineIndex, line: u64) -> io::Result<u64> {
    let (mut l, off) = index.entry_for(line);
    r.seek(SeekFrom::Start(off))?;
    let mut it = Lines::new(r, off);
    let (mut buf, mut cut) = (Vec::new(), false);
    while l < line {
        if it.next_into(&mut buf, 0, &mut cut)? != Some(true) {
            break;
        }
        l += 1;
    }
    Ok(it.offset)
}

/// A match: its line and the byte range in that line's (capped) bytes.
pub type Hit = (u64, Range<usize>);

/// Call `f` for every match from line `line` (starting at byte `offset`) up
/// to byte `end`; `f` returning false stops. Lines are searched up to
/// `MAX_LINE` bytes. Returns false if `cancel` stopped it.
pub fn scan<R: Read + Seek>(
    r: &mut R,
    re: &regex::bytes::Regex,
    (line, offset): (u64, u64),
    end: u64,
    cancel: &AtomicBool,
    mut f: impl FnMut(Hit) -> bool,
) -> io::Result<bool> {
    r.seek(SeekFrom::Start(offset))?;
    let mut it = Lines::new(r, offset);
    let (mut buf, mut cut) = (Vec::new(), false);
    let mut l = line;
    let mut n = 0u32;
    while it.offset < end {
        n = n.wrapping_add(1);
        if n % 4096 == 0 && cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let Some(nl) = it.next_into(&mut buf, MAX_LINE, &mut cut)? else { break };
        for m in re.find_iter(&buf) {
            if !f((l, m.range())) {
                return Ok(true);
            }
        }
        if !nl {
            break;
        }
        l += 1;
    }
    Ok(true)
}

/// The first match after `from` (a line and its start offset; matches on
/// that line count only after byte `col`), wrapping around to the top.
pub fn find_next<R: Read + Seek>(r: &mut R, re: &regex::bytes::Regex, from: (u64, u64), col: usize, len: u64, cancel: &AtomicBool) -> io::Result<Option<Hit>> {
    let mut hit = None;
    scan(r, re, from, len, cancel, |(l, m)| {
        if l == from.0 && m.start < col {
            return true;
        }
        hit = Some((l, m));
        false
    })?;
    if hit.is_none() && !cancel.load(Ordering::Relaxed) {
        scan(r, re, (0, 0), from.1 + 1, cancel, |(l, m)| {
            if l > from.0 || (l == from.0 && m.start >= col) {
                return false;
            }
            hit = Some((l, m));
            false
        })?;
    }
    Ok(hit)
}

/// The last match before line `line`, byte `col` (wrapping around to the
/// bottom), searching back one index block at a time.
pub fn find_prev<R: Read + Seek>(r: &mut R, re: &regex::bytes::Regex, index: &LineIndex, line: u64, col: usize, len: u64, cancel: &AtomicBool) -> io::Result<Option<Hit>> {
    let before = |l: u64, m: &Range<usize>| l < line || (l == line && m.start < col);
    let start_block = index.entries.partition_point(|e| e.0 <= line).saturating_sub(1);
    let n = index.entries.len();
    // Blocks from the cursor's back to the first, then from the last back to
    // the cursor's again (k == n), where anything left is at or after it.
    for k in 0..=n {
        let b = (start_block + n - k) % n;
        let wrapped = k > start_block;
        let end = index.entries.get(b + 1).map_or(len, |e| e.1);
        let mut last = None;
        let ok = scan(r, re, index.entries[b], end, cancel, |(l, m)| {
            if wrapped || before(l, &m) {
                last = Some((l, m));
            }
            true
        })?;
        if !ok {
            return Ok(None);
        }
        if last.is_some() {
            return Ok(last);
        }
    }
    Ok(None)
}

/// Count every match in the file, `progress(bytes scanned, count)` now and then.
pub fn count<R: Read + Seek>(r: &mut R, re: &regex::bytes::Regex, len: u64, cancel: &AtomicBool, mut progress: impl FnMut(u64, u64)) -> io::Result<Option<u64>> {
    r.seek(SeekFrom::Start(0))?;
    let mut it = Lines::new(r, 0);
    let (mut buf, mut cut) = (Vec::new(), false);
    let mut total = 0u64;
    let mut n = 0u32;
    while it.offset < len {
        n = n.wrapping_add(1);
        if n % 8192 == 0 {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            progress(it.offset, total);
        }
        let Some(nl) = it.next_into(&mut buf, MAX_LINE, &mut cut)? else { break };
        total += re.find_iter(&buf).count() as u64;
        if !nl {
            break;
        }
    }
    Ok(Some(total))
}

/// A find query as a bytes regex: literal unless `regex`, case-insensitive
/// unless `case_sensitive`, whole words with `whole_word`.
pub fn query_regex(query: &str, opts: crate::FindOptions) -> Result<regex::bytes::Regex, String> {
    let mut pat = if opts.regex { query.to_string() } else { regex::escape(query) };
    if opts.whole_word {
        pat = format!(r"\b(?:{pat})\b");
    }
    regex::bytes::RegexBuilder::new(&pat).case_insensitive(!opts.case_sensitive).unicode(false).build().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn index_of(data: &[u8]) -> LineIndex {
        let mut ix = LineIndex::default();
        index_file(Cursor::new(data), |c| {
            ix.feed(c);
            true
        })
        .unwrap();
        ix.done = true;
        ix
    }

    fn numbered(n: usize) -> Vec<u8> {
        (0..n).map(|i| format!("line {i}\n")).collect::<String>().into_bytes()
    }

    #[test]
    fn counts_lines_like_the_editor() {
        assert_eq!(index_of(b"").lines, 1);
        assert_eq!(index_of(b"a").lines, 1);
        assert_eq!(index_of(b"a\n").lines, 2);
        assert_eq!(index_of(b"a\nb\nc").lines, 3);
    }

    #[test]
    fn sparse_entries_every_1024_lines() {
        let data = numbered(5000);
        let ix = index_of(&data);
        assert_eq!(ix.lines, 5001);
        assert_eq!(ix.entries.iter().map(|e| e.0).collect::<Vec<_>>(), vec![0, 1024, 2048, 3072, 4096]);
        // Each entry is that line's real start.
        for &(l, off) in &ix.entries {
            assert!(data[off as usize..].starts_with(format!("line {l}\n").as_bytes()));
        }
        assert_eq!(ix.entry_for(2047), (1024, ix.entries[1].1));
        assert_eq!(ix.entry_for(2048).0, 2048);
    }

    #[test]
    fn entries_every_256k_for_long_lines() {
        let line = format!("{}\n", "x".repeat(100_000));
        let data = line.repeat(10).into_bytes();
        let ix = index_of(&data);
        // A new entry once 256 KB have passed: lines 3, 6, 9.
        assert_eq!(ix.entries.iter().map(|e| e.0).collect::<Vec<_>>(), vec![0, 3, 6, 9]);
    }

    #[test]
    fn chunked_feeding_matches_one_shot() {
        let data = numbered(3000);
        let mut a = LineIndex::default();
        for c in data.chunks(7) {
            a.feed(c);
        }
        let b = index_of(&data);
        assert_eq!(a.entries, b.entries);
        assert_eq!(a.lines, b.lines);
    }

    #[test]
    fn reads_visible_lines() {
        let data = numbered(5000);
        let ix = index_of(&data);
        let mut c = Cursor::new(&data);
        let got = read_lines(&mut c, &ix, 2500, 3).unwrap();
        assert_eq!(got.iter().map(|l| String::from_utf8_lossy(&l.bytes).into_owned()).collect::<Vec<_>>(), ["line 2500", "line 2501", "line 2502"]);
        // The empty last line after the final newline.
        let tail = read_lines(&mut c, &ix, 4999, 5).unwrap();
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[1].bytes, b"");
        assert_eq!(line_offset(&mut c, &ix, 2500).unwrap() as usize, data.windows(10).position(|w| w == b"line 2500\n").unwrap());
    }

    #[test]
    fn crlf_and_long_lines_are_trimmed() {
        let mut data = b"a\r\nb\r\n".to_vec();
        data.extend(std::iter::repeat_n(b'z', MAX_LINE + 10));
        data.extend(b"\nend");
        let ix = index_of(&data);
        let got = read_lines(&mut Cursor::new(&data), &ix, 0, 10).unwrap();
        assert_eq!(got.len(), 4);
        assert_eq!(got[0].bytes, b"a");
        assert_eq!(got[2].bytes.len(), MAX_LINE);
        assert!(got[2].cut);
        assert_eq!(got[3], Line { bytes: b"end".to_vec(), cut: false });
    }

    #[test]
    fn finds_forward_backward_and_wraps() {
        let data = numbered(5000);
        let ix = index_of(&data);
        let len = data.len() as u64;
        let re = query_regex("line 42", crate::FindOptions::default()).unwrap();
        let stop = AtomicBool::new(false);
        let mut c = Cursor::new(&data);
        let off = |l| line_offset(&mut Cursor::new(&data), &ix, l).unwrap();
        // From line 100: next is 420.
        let hit = find_next(&mut c, &re, (100, off(100)), 0, len, &stop).unwrap().unwrap();
        assert_eq!(hit, (420, 0..7));
        // From 4300: wraps to 42.
        assert_eq!(find_next(&mut c, &re, (4300, off(4300)), 0, len, &stop).unwrap().unwrap().0, 42);
        // Backward from 4300: 4299? no, "line 42" starts lines 42, 420-429, 4200-4299.
        assert_eq!(find_prev(&mut c, &re, &ix, 4300, 0, len, &stop).unwrap().unwrap().0, 4299);
        // Backward from 42: wraps to the bottom-most, 4299.
        assert_eq!(find_prev(&mut c, &re, &ix, 42, 0, len, &stop).unwrap().unwrap().0, 4299);
        assert_eq!(find_prev(&mut c, &re, &ix, 43, 0, len, &stop).unwrap().unwrap().0, 42);
        assert_eq!(count(&mut c, &re, len, &stop, |_, _| {}).unwrap(), Some(1 + 10 + 100));
        let none = query_regex("nope", crate::FindOptions::default()).unwrap();
        assert_eq!(find_next(&mut c, &none, (0, 0), 0, len, &stop).unwrap(), None);
        assert_eq!(find_prev(&mut c, &none, &ix, 10, 0, len, &stop).unwrap(), None);
    }

    #[test]
    fn find_options() {
        let opts = crate::FindOptions { case_sensitive: false, whole_word: true, regex: false };
        let re = query_regex("Foo", opts).unwrap();
        assert!(re.is_match(b"a foo b"));
        assert!(!re.is_match(b"foobar"));
        assert!(query_regex("(", crate::FindOptions { regex: true, ..Default::default() }).is_err());
    }
}
