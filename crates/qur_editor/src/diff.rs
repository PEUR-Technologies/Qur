//! Line diff of a buffer against a base text (the code modal's change
//! markers): lines are compared by hash, common ends are trimmed, and the
//! middle goes through Myers' O(ND) diff. Pure and UI-free, so it can run on
//! a background thread with a rope clone.

use std::ops::Range;

use ropey::Rope;

/// Edit distance (after trimming common ends) beyond which the changed
/// middle becomes one hunk instead of a precise diff. Bounds the trace to
/// about `MAX_D²` integers and the time to `(N + M) · MAX_D` steps.
pub const MAX_D: usize = 1500;

/// One changed region: buffer lines `new` replaced base lines `old`
/// (0-based, half-open). An empty `old` is an insertion before base line
/// `old.start`; an empty `new` is a deletion before buffer line `new.start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineHunk {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkKind {
    Added,
    Removed,
    Modified,
}

impl LineHunk {
    pub fn kind(&self) -> HunkKind {
        match (self.old.is_empty(), self.new.is_empty()) {
            (true, _) => HunkKind::Added,
            (_, true) => HunkKind::Removed,
            _ => HunkKind::Modified,
        }
    }
}

/// FNV-1a over a line's bytes; streaming, so a line split across rope
/// chunks hashes the same as the whole string.
fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}
const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

/// Byte ranges of `text`'s lines, each with its `\n` (the last one may lack
/// it). An empty text has no lines; a trailing `\n` doesn't start one.
pub fn line_ranges(text: &str) -> Vec<Range<usize>> {
    let mut v = Vec::new();
    let mut s = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            v.push(s..i + 1);
            s = i + 1;
        }
    }
    if s < text.len() {
        v.push(s..text.len());
    }
    v
}

/// Line hashes of a text split by `line_ranges`.
pub fn text_hashes(text: &str, lines: &[Range<usize>]) -> Vec<u64> {
    lines.iter().map(|r| fnv(FNV0, &text.as_bytes()[r.clone()])).collect()
}

/// Line hashes of a rope, lines as `line_ranges` counts them (no empty
/// line after a trailing `\n`).
pub fn rope_hashes(rope: &Rope) -> Vec<u64> {
    let n = real_lines(rope);
    (0..n).map(|l| rope.line(l).chunks().fold(FNV0, |h, c| fnv(h, c.as_bytes()))).collect()
}

/// Lines in the rope, not counting the empty one after a trailing `\n`.
pub fn real_lines(rope: &Rope) -> usize {
    let n = rope.len_lines();
    if rope.len_chars() == 0 {
        0
    } else if rope.line(n - 1).len_chars() == 0 {
        n - 1
    } else {
        n
    }
}

/// The hunks turning `old` into `new` (line hashes), in order.
pub fn diff_lines(old: &[u64], new: &[u64]) -> Vec<LineHunk> {
    let pre = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let (o, n) = (&old[pre..], &new[pre..]);
    let suf = o.iter().rev().zip(n.iter().rev()).take_while(|(a, b)| a == b).count();
    let (o, n) = (&o[..o.len() - suf], &n[..n.len() - suf]);
    if o.is_empty() && n.is_empty() {
        return Vec::new();
    }
    let whole = || vec![LineHunk { old: pre..pre + o.len(), new: pre..pre + n.len() }];
    if o.is_empty() || n.is_empty() {
        return whole();
    }
    let Some(matches) = myers(o, n, MAX_D) else { return whole() };
    // Gaps between matched pairs are the hunks.
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    for (mi, mj) in matches.into_iter().chain(std::iter::once((o.len(), n.len()))) {
        if mi > i || mj > j {
            out.push(LineHunk { old: pre + i..pre + mi, new: pre + j..pre + mj });
        }
        i = mi + 1;
        j = mj + 1;
    }
    out
}

/// Matched (old, new) index pairs of a shortest edit script, in order, or
/// None when the edit distance exceeds `max_d`.
fn myers(a: &[u64], b: &[u64], max_d: usize) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let dmax = max_d.min(a.len() + b.len()) as isize;
    let off = dmax + 1;
    let mut v = vec![0isize; (2 * dmax + 3) as usize];
    // trace[d] = v for k in -(d+1)..=d+1 as it was before step d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=dmax {
        trace.push(v[(off - d - 1) as usize..=(off + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let down = k == -d || (k != d && v[(off + k - 1) as usize] < v[(off + k + 1) as usize]);
            let mut x = if down { v[(off + k + 1) as usize] } else { v[(off + k - 1) as usize] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(off + k) as usize] = x;
            if x >= n && y >= m {
                return Some(backtrack(&trace, d, n, m));
            }
            k += 2;
        }
    }
    None
}

fn backtrack(trace: &[Vec<isize>], dend: isize, n: isize, m: isize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=dend).rev() {
        if d == 0 {
            while x > 0 && y > 0 {
                x -= 1;
                y -= 1;
                out.push((x as usize, y as usize));
            }
            break;
        }
        let vd = &trace[d as usize];
        let get = |k: isize| vd[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && get(k - 1) < get(k + 1)) { k + 1 } else { k - 1 };
        let prev_x = get(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            out.push((x as usize, y as usize));
        }
        x = prev_x;
        y = prev_y;
    }
    out.reverse();
    out
}
