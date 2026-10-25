//! Diffs: a task's whole change set vs its base, one file for the Git panel,
//! and line totals. Parses `git diff --patch-with-raw -z` (paths come from the
//! NUL-separated raw part, so odd names never need unquoting).

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::{Git, TempFile, check_rev, nul_fields, path_of, rel, temp_name};

#[derive(Clone, Debug, PartialEq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Binary,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// The line without its prefix and line ending (a trailing `\r` is dropped too).
    pub text: String,
    /// Byte ranges of `text` that changed vs. the paired line (word-level, for
    /// paired -/+ runs); empty = whole line. A zero-length range marks where
    /// the other side inserted text when nothing on this side changed.
    pub emphasis: Vec<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// The full `@@ -a,b +c,d @@ section` line.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileDiff {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub status: FileStatus,
    pub added: u32,
    pub removed: u32,
    pub hunks: Vec<Hunk>,
}

/// Lines kept per file; the rest are counted, not stored, and a flag line
/// (Context, no line numbers) says how many were left out.
pub const MAX_LINES_PER_FILE: usize = 5000;
/// Lines longer than this (in tokens) get no word emphasis.
const MAX_EMPHASIS_TOKENS: usize = 256;

pub(crate) const DIFF_ARGS: &[&str] = &[
    "diff",
    "--patch-with-raw",
    "-z",
    "-M",
    "-U3",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--no-relative",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--submodule=short",
];

/// Everything the task changed vs its base commit: committed + uncommitted +
/// untracked (respecting .gitignore), 3 lines of context, sorted by path.
///
/// Works on a throwaway copy of the worktree's index where untracked files are
/// marked intent-to-add, then diffs the working tree against `base`; the real
/// index and the object store are never written.
pub fn task_diff(worktree: &Path, base: &str) -> Result<Vec<FileDiff>, String> {
    check_rev(base)?;
    let idx = task_index(worktree)?;
    let out = Git::ro(worktree)
        .env("GIT_INDEX_FILE", &idx.0)
        .args(DIFF_ARGS)
        .arg(base)
        .arg("--")
        .ok()?;
    let mut files = parse_patch(&out);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Lines added/removed for a task branch vs its base incl. uncommitted and
/// untracked work (for the sidebar "+n −m"). Binary files count 0.
pub fn diff_totals(worktree: &Path, base: &str) -> Result<(u32, u32), String> {
    check_rev(base)?;
    let idx = task_index(worktree)?;
    let out = Git::ro(worktree)
        .env("GIT_INDEX_FILE", &idx.0)
        .args(["diff", "--numstat", "-z", "-M", "--no-ext-diff", "--no-textconv", "--no-relative"])
        .arg(base)
        .arg("--")
        .ok()?;
    let (mut a, mut r) = (0u32, 0u32);
    // `added\tremoved\tpath\0`, or `added\tremoved\t\0old\0new\0` for renames.
    let f = nul_fields(&out);
    let mut i = 0;
    while i < f.len() {
        let rec = f[i];
        let mut parts = rec.splitn(3, |&c| c == b'\t');
        let na = parts.next().unwrap_or_default();
        let nr = parts.next().unwrap_or_default();
        let path = parts.next().unwrap_or_default();
        a = a.saturating_add(num(na));
        r = r.saturating_add(num(nr));
        i += if path.is_empty() { 3 } else { 1 };
    }
    Ok((a, r))
}

fn num(b: &[u8]) -> u32 {
    std::str::from_utf8(b).ok().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// A copy of the worktree's index with every untracked, non-ignored file
/// added as intent-to-add. Lives next to the real index (so split-index
/// setups still resolve) and is deleted on drop.
fn task_index(worktree: &Path) -> Result<TempFile, String> {
    // The untracked scan reads the real index (read-only) while we copy it.
    let wt = worktree.to_path_buf();
    let scan = std::thread::spawn(move || {
        Git::ro(&wt).args(["ls-files", "-z", "--others", "--exclude-standard", "--no-empty-directory"]).ok()
    });
    // Refresh the task worktree's stat cache (content-neutral; it's Qur's
    // worktree, never the user's checkout). A freshly created worktree's
    // index is "racy", which would make every later step re-hash every file.
    // Skipped silently when the index is locked.
    let _ = Git::new(worktree).args(["update-index", "-q", "--refresh", "--unmerged"]).run();
    let real = index_path(worktree)?;
    let dir = real.parent().ok_or("no git dir")?;
    let tmp = TempFile(dir.join(temp_name("diff-index")));
    if real.exists() {
        std::fs::copy(&real, &tmp.0).map_err(|e| format!("copy index: {e}"))?;
    }
    let untracked = scan.join().map_err(|_| "untracked scan panicked".to_string())??;
    if !untracked.is_empty() {
        Git::new(worktree)
            .env("GIT_INDEX_FILE", &tmp.0)
            .args(["-c", "core.splitIndex=false", "add", "-N", "--pathspec-from-file=-", "--pathspec-file-nul"])
            .stdin(untracked)
            .ok()?;
    }
    Ok(tmp)
}

/// The checkout's index file: `.git/index`, or `<gitdir>/index` for a linked
/// worktree (`.git` file). Falls back to asking git.
pub(crate) fn index_path(worktree: &Path) -> Result<PathBuf, String> {
    let dot = worktree.join(".git");
    if dot.is_dir() {
        return Ok(dot.join("index"));
    }
    if let Ok(s) = std::fs::read_to_string(&dot)
        && let Some(g) = s.lines().next().and_then(|l| l.strip_prefix("gitdir: "))
    {
        let g = Path::new(g.trim());
        let g = if g.is_absolute() { g.to_path_buf() } else { worktree.join(g) };
        if g.is_dir() {
            return Ok(g.join("index"));
        }
    }
    let p = Git::ro(worktree).args(["rev-parse", "--git-path", "index"]).text()?;
    Ok(worktree.join(p))
}

/// One file for the Git panel: index vs HEAD (`staged`) or working tree vs
/// index. Untracked files come back as Added. `None` when there is no change.
pub fn file_diff(root: &Path, path: &Path, staged: bool) -> Result<Option<FileDiff>, String> {
    let p = rel(root, path)?;
    if staged {
        // A staged rename needs both sides in the pathspec to be seen as one.
        let raw = Git::ro(root).args(["diff", "--cached", "--raw", "-z", "-M", "--no-relative"]).ok()?;
        let mut spec = vec![p.clone()];
        for e in parse_raw(&raw).0 {
            if e.path == p
                && let Some(o) = e.old_path
            {
                spec.push(o);
            }
        }
        let out = Git::ro(root).args(DIFF_ARGS).arg("--cached").arg("--").args(&spec).ok()?;
        return Ok(parse_patch(&out).into_iter().find(|f| f.path == p));
    }
    let out = Git::ro(root).args(DIFF_ARGS).arg("--").arg(&p).ok()?;
    if let Some(f) = parse_patch(&out).into_iter().find(|f| f.path == p) {
        return Ok(Some(f));
    }
    let others = Git::ro(root).args(["ls-files", "-z", "--others", "--exclude-standard", "--"]).arg(&p).ok()?;
    if !nul_fields(&others).iter().any(|o| path_of(o) == p) {
        return Ok(None);
    }
    // Untracked: diff it against a scratch index holding just an intent-to-add entry.
    let tmp = TempFile(std::env::temp_dir().join(temp_name("file-index")));
    Git::new(root)
        .env("GIT_INDEX_FILE", &tmp.0)
        .args(["-c", "core.splitIndex=false", "add", "-N", "--"])
        .arg(&p)
        .ok()?;
    let out = Git::ro(root).env("GIT_INDEX_FILE", &tmp.0).args(DIFF_ARGS).arg("--").arg(&p).ok()?;
    Ok(parse_patch(&out).into_iter().find(|f| f.path == p))
}

struct RawEntry {
    status: u8,
    path: PathBuf,
    old_path: Option<PathBuf>,
}

/// Parse the `--raw -z` records at the start of `b`; returns them and the
/// offset where the patch text starts.
fn parse_raw(b: &[u8]) -> (Vec<RawEntry>, usize) {
    let mut v = Vec::new();
    let mut p = 0;
    let field = |p: &mut usize| -> &[u8] {
        let s = *p;
        let e = b[s..].iter().position(|&c| c == 0).map_or(b.len(), |i| s + i);
        *p = (e + 1).min(b.len());
        &b[s..e]
    };
    while p < b.len() && b[p] == b':' {
        let meta = field(&mut p);
        let status = meta.rsplit(|&c| c == b' ').next().and_then(|s| s.first().copied()).unwrap_or(b'M');
        let first = path_of(field(&mut p));
        if matches!(status, b'R' | b'C') {
            let second = path_of(field(&mut p));
            v.push(RawEntry { status, path: second, old_path: Some(first) });
        } else {
            v.push(RawEntry { status, path: first, old_path: None });
        }
    }
    if p < b.len() && b[p] == 0 {
        p += 1;
    }
    (v, p)
}

/// Parse `git diff --patch-with-raw -z` output. Patch blocks follow the raw
/// records one for one (unmerged entries have no block).
pub(crate) fn parse_patch(b: &[u8]) -> Vec<FileDiff> {
    let (entries, start) = parse_raw(b);
    let text = &b[start..];
    let mut lines: Vec<&[u8]> = text.split(|&c| c == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut li = 0;
    let mut out = Vec::new();
    for e in entries {
        if e.status == b'U' {
            continue;
        }
        let mut f = FileDiff {
            status: match e.status {
                b'A' | b'C' => FileStatus::Added,
                b'D' => FileStatus::Deleted,
                b'R' => FileStatus::Renamed,
                _ => FileStatus::Modified,
            },
            path: e.path,
            old_path: e.old_path,
            added: 0,
            removed: 0,
            hunks: Vec::new(),
        };
        // Find this entry's block.
        while li < lines.len() && !lines[li].starts_with(b"diff ") {
            li += 1;
        }
        if li < lines.len() {
            li += 1;
            parse_block(&lines, &mut li, &mut f);
        }
        out.push(f);
    }
    out
}

/// Parse one file block's extended header and hunks, starting after `diff --git`.
fn parse_block(lines: &[&[u8]], li: &mut usize, f: &mut FileDiff) {
    // Extended header.
    while *li < lines.len() && !lines[*li].starts_with(b"@@ ") && !lines[*li].starts_with(b"diff ") {
        let l = lines[*li];
        if l.starts_with(b"Binary files ") || l.starts_with(b"GIT binary patch") {
            f.status = FileStatus::Binary;
        }
        *li += 1;
    }
    let mut kept = 0usize;
    let mut dropped = 0usize;
    while *li < lines.len() && lines[*li].starts_with(b"@@ ") {
        let header = String::from_utf8_lossy(lines[*li]).into_owned();
        *li += 1;
        let Some((os, ol, ns, nl)) = parse_hunk_header(&header) else { break };
        let mut h = Hunk { old_start: os, old_lines: ol, new_start: ns, new_lines: nl, header, lines: Vec::new() };
        let (mut o_left, mut n_left) = (ol, nl);
        let (mut o_no, mut n_no) = (os, ns);
        while *li < lines.len() && (o_left > 0 || n_left > 0 || lines[*li].starts_with(b"\\")) {
            let l = lines[*li];
            let (kind, body) = match l.first() {
                Some(b' ') => (LineKind::Context, &l[1..]),
                Some(b'-') => (LineKind::Removed, &l[1..]),
                Some(b'+') => (LineKind::Added, &l[1..]),
                Some(b'\\') => {
                    // "\ No newline at end of file"
                    *li += 1;
                    continue;
                }
                // An empty context line with its space stripped (some tools do this).
                None if o_left > 0 && n_left > 0 => (LineKind::Context, l),
                _ => break,
            };
            *li += 1;
            let (old_no, new_no) = match kind {
                LineKind::Context => {
                    o_left = o_left.saturating_sub(1);
                    n_left = n_left.saturating_sub(1);
                    (Some(o_no), Some(n_no))
                }
                LineKind::Removed => {
                    o_left = o_left.saturating_sub(1);
                    f.removed += 1;
                    (Some(o_no), None)
                }
                LineKind::Added => {
                    n_left = n_left.saturating_sub(1);
                    f.added += 1;
                    (None, Some(n_no))
                }
            };
            if old_no.is_some() {
                o_no += 1;
            }
            if new_no.is_some() {
                n_no += 1;
            }
            if kept >= MAX_LINES_PER_FILE {
                dropped += 1;
                continue;
            }
            kept += 1;
            let body = body.strip_suffix(b"\r").unwrap_or(body);
            h.lines.push(DiffLine {
                kind,
                old_no,
                new_no,
                text: String::from_utf8_lossy(body).into_owned(),
                emphasis: Vec::new(),
            });
        }
        if !h.lines.is_empty() {
            emphasize(&mut h.lines);
            f.hunks.push(h);
        }
    }
    if f.status == FileStatus::Binary {
        f.hunks.clear();
        f.added = 0;
        f.removed = 0;
    }
    if dropped > 0
        && let Some(h) = f.hunks.last_mut()
    {
        h.lines.push(DiffLine {
            kind: LineKind::Context,
            old_no: None,
            new_no: None,
            text: format!("… {dropped} more diff lines not shown (file too large)"),
            emphasis: Vec::new(),
        });
    }
}

/// `@@ -a[,b] +c[,d] @@ ...` -> (a, b, c, d); counts default to 1.
fn parse_hunk_header(h: &str) -> Option<(u32, u32, u32, u32)> {
    let rest = h.strip_prefix("@@ -")?;
    let end = rest.find(" @@")?;
    let (old, new) = rest[..end].split_once(" +")?;
    let pair = |s: &str| -> Option<(u32, u32)> {
        match s.split_once(',') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (a, b) = pair(old)?;
    let (c, d) = pair(new)?;
    Some((a, b, c, d))
}

/// Word-level emphasis for runs of removed lines followed by added lines,
/// paired i-th with i-th.
fn emphasize(lines: &mut [DiffLine]) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != LineKind::Removed {
            i += 1;
            continue;
        }
        let rs = i;
        while i < lines.len() && lines[i].kind == LineKind::Removed {
            i += 1;
        }
        let as_ = i;
        while i < lines.len() && lines[i].kind == LineKind::Added {
            i += 1;
        }
        let pairs = (as_ - rs).min(i - as_);
        for k in 0..pairs {
            let (a, b) = word_diff(&lines[rs + k].text, &lines[as_ + k].text);
            lines[rs + k].emphasis = a;
            lines[as_ + k].emphasis = b;
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// Words, whitespace runs, and single punctuation characters, as byte ranges.
fn tokens(s: &str) -> Vec<Range<usize>> {
    let mut v: Vec<Range<usize>> = Vec::new();
    let mut prev: Option<Class> = None;
    for (i, c) in s.char_indices() {
        let k = class(c);
        let end = i + c.len_utf8();
        match (prev, v.last_mut()) {
            (Some(p), Some(last)) if p == k && k != Class::Other => last.end = end,
            _ => v.push(i..end),
        }
        prev = Some(k);
    }
    v
}

/// Changed byte ranges of `a` and `b` (LCS over tokens). Both empty when the
/// lines share nothing but whitespace (whole-line change) or are too long.
pub(crate) fn word_diff(a: &str, b: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let ta = tokens(a);
    let tb = tokens(b);
    if ta.len() > MAX_EMPHASIS_TOKENS || tb.len() > MAX_EMPHASIS_TOKENS || a == b {
        return (Vec::new(), Vec::new());
    }
    let (n, m) = (ta.len(), tb.len());
    let eq = |i: usize, j: usize| a[ta[i].clone()] == b[tb[j].clone()];
    // dp[i][j] = LCS of ta[i..], tb[j..]
    let mut dp = vec![0u16; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[at(i, j)] = if eq(i, j) { dp[at(i + 1, j + 1)] + 1 } else { dp[at(i + 1, j)].max(dp[at(i, j + 1)]) };
        }
    }
    let mut ka = vec![false; n];
    let mut kb = vec![false; m];
    let (mut i, mut j) = (0, 0);
    let mut shared_words = 0;
    while i < n && j < m {
        if eq(i, j) {
            ka[i] = true;
            kb[j] = true;
            if !a[ta[i].clone()].trim().is_empty() {
                shared_words += 1;
            }
            i += 1;
            j += 1;
        } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    if shared_words == 0 {
        return (Vec::new(), Vec::new());
    }
    let ra = ranges(&ta, &ka);
    let rb = ranges(&tb, &kb);
    // One side may only have insertions on the other: all its tokens matched,
    // so the other side's first change sits before token #f here.
    let point = |toks: &[Range<usize>], text: &str, other_kept: &[bool]| {
        let f = other_kept.iter().position(|k| !k).unwrap_or(other_kept.len());
        let at = toks.get(f).map_or(text.len(), |t| t.start);
        std::iter::once(at..at).collect::<Vec<_>>()
    };
    match (ra.is_empty(), rb.is_empty()) {
        (true, false) => (point(&ta, a, &kb), rb),
        (false, true) => (ra, point(&tb, b, &ka)),
        _ => (ra, rb),
    }
}

/// Merge unmatched tokens into contiguous byte ranges.
fn ranges(toks: &[Range<usize>], kept: &[bool]) -> Vec<Range<usize>> {
    let mut v: Vec<Range<usize>> = Vec::new();
    for (t, k) in toks.iter().zip(kept) {
        if *k {
            continue;
        }
        match v.last_mut() {
            Some(last) if last.end == t.start => last.end = t.end,
            _ => v.push(t.clone()),
        }
    }
    v
}

/// Paths with any difference, for overlap checks.
pub(crate) fn changed_paths(root: &Path, from: &str, to: &str) -> Result<HashSet<PathBuf>, String> {
    let out = Git::ro(root)
        .args(["diff", "--name-only", "-z", "--no-renames", "--no-relative", "--no-ext-diff", from, to, "--"])
        .ok()?;
    Ok(nul_fields(&out).into_iter().map(path_of).collect())
}
