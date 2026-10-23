//! Blocking git helpers (call off the UI thread). Uses the git CLI, like the rest of Qur.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::DiffStat;

/// Pathspec chunk size, to stay far below ARG_MAX.
const CHUNK: usize = 500;
/// Bytes sniffed for a NUL to call an untracked file binary (git uses the same amount).
const SNIFF: usize = 8000;

fn git(worktree: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(worktree)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_OPTIONAL_LOCKS", "0") // don't take index.lock just to refresh stat info
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    c
}

fn run(mut c: Command) -> Vec<u8> {
    match c.output() {
        Ok(o) if o.status.success() => o.stdout,
        _ => Vec::new(),
    }
}

fn path_of(b: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(b))
}

/// Tracked differences between `base` and the working tree (`--raw --numstat -z`).
fn tracked(worktree: &Path, base: &str, rels: Option<&[PathBuf]>, out: &mut BTreeMap<PathBuf, DiffStat>) {
    let mut c = git(worktree);
    c.args(["diff", "--no-renames", "--no-ext-diff", "--no-textconv", "--raw", "--numstat", "-z", base, "--"]);
    if let Some(r) = rels {
        c.args(r);
    }
    let buf = run(c);
    let mut it = buf.split(|&b| b == 0).peekable();
    while let Some(rec) = it.next() {
        if rec.is_empty() {
            continue;
        }
        if rec[0] == b':' {
            // ":100644 100644 <sha> <sha> M" then the path.
            let status = rec.split(|&b| b == b' ').nth(4).and_then(|s| s.first()).copied();
            let Some(p) = it.next() else { break };
            let e = out.entry(path_of(p)).or_default();
            match status {
                Some(b'A') => e.created = true,
                Some(b'D') => e.deleted = true,
                _ => {}
            }
        } else {
            // "<added>\t<removed>\t<path>" ("-\t-\t" for binary).
            let mut f = rec.splitn(3, |&b| b == b'\t');
            let (Some(a), Some(r), Some(p)) = (f.next(), f.next(), f.next()) else { continue };
            let e = out.entry(path_of(p)).or_default();
            if a == b"-" {
                e.binary = true;
            } else {
                e.added = num(a);
                e.removed = num(r);
            }
        }
    }
}

fn num(b: &[u8]) -> u32 {
    std::str::from_utf8(b).ok().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Untracked, not ignored files: every line counts as added.
fn untracked(worktree: &Path, rels: Option<&[PathBuf]>, out: &mut BTreeMap<PathBuf, DiffStat>) {
    let mut c = git(worktree);
    c.args(["ls-files", "-z", "--others", "--exclude-standard", "--"]);
    if let Some(r) = rels {
        c.args(r);
    }
    for p in run(c).split(|&b| b == 0).filter(|p| !p.is_empty()) {
        let rel = path_of(p);
        let mut st = DiffStat { created: true, ..Default::default() };
        if let Ok(data) = std::fs::read(worktree.join(&rel)) {
            if data[..data.len().min(SNIFF)].contains(&0) {
                st.binary = true;
            } else {
                let nl = data.iter().filter(|&&b| b == b'\n').count();
                let partial = !data.is_empty() && data.last() != Some(&b'\n');
                st.added = (nl + partial as usize) as u32;
            }
        }
        out.insert(rel, st);
    }
}

/// Diff stats for `rels` (relative to `worktree`) against `base`. Every input path gets an
/// entry (`None` = same as base); a directory path also yields entries for the changed
/// files inside it (FSEvents reports `mv dir/` as the directory only).
pub fn diff_stats(worktree: &Path, base: &str, rels: &[PathBuf]) -> Vec<(PathBuf, Option<DiffStat>)> {
    let mut found = BTreeMap::new();
    for chunk in rels.chunks(CHUNK) {
        tracked(worktree, base, Some(chunk), &mut found);
        untracked(worktree, Some(chunk), &mut found);
    }
    let mut out: Vec<(PathBuf, Option<DiffStat>)> =
        rels.iter().filter(|r| !found.contains_key(*r)).map(|r| (r.clone(), None)).collect();
    out.extend(found.into_iter().map(|(p, s)| (p, Some(s))));
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// Every file differing from `base`, untracked ones included (ignored files excluded).
pub fn full_scan(worktree: &Path, base: &str) -> Vec<(PathBuf, DiffStat)> {
    let mut found = BTreeMap::new();
    tracked(worktree, base, None, &mut found);
    untracked(worktree, None, &mut found);
    found.into_iter().collect()
}
