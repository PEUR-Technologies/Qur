//! Git panel on the main checkout: status, stage, unstage, discard, commit.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::{Git, ident_args, join_paths, nul_fields, path_of, rels};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    /// The old path of a rename.
    pub orig: Option<PathBuf>,
    /// `M` modified (also type change), `A` added (also copied), `D` deleted,
    /// `R` renamed, `U` unmerged, `?` untracked.
    pub code: char,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// None = detached HEAD.
    pub branch: Option<String>,
    /// HEAD commit; None before the first commit.
    pub head: Option<String>,
    /// Commits ahead of / behind the upstream (0 without one).
    pub ahead: u32,
    pub behind: u32,
    pub staged: Vec<Entry>,
    pub unstaged: Vec<Entry>,
    pub untracked: Vec<PathBuf>,
    pub conflicted: Vec<PathBuf>,
    pub merging: bool,
}

fn code(c: u8) -> char {
    match c {
        b'M' | b'T' => 'M',
        b'A' | b'C' => 'A',
        b'D' => 'D',
        b'R' => 'R',
        b'U' => 'U',
        _ => 'M',
    }
}

/// `git status --porcelain=v2 --branch -z`, untracked files listed one by one.
pub fn status(root: &Path) -> Result<Status, String> {
    let out = Git::ro(root)
        .args(["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all", "--ignore-submodules=dirty"])
        .ok()?;
    let mut s = parse_status(&out);
    s.merging = crate::is_merging(root);
    Ok(s)
}

pub(crate) fn parse_status(out: &[u8]) -> Status {
    let mut s = Status::default();
    let f = nul_fields(out);
    let mut i = 0;
    while i < f.len() {
        let rec = f[i];
        i += 1;
        let text = String::from_utf8_lossy(rec);
        if let Some(h) = text.strip_prefix("# ") {
            if let Some(v) = h.strip_prefix("branch.oid ") {
                s.head = (v != "(initial)").then(|| v.to_string());
            } else if let Some(v) = h.strip_prefix("branch.head ") {
                s.branch = (v != "(detached)").then(|| v.to_string());
            } else if let Some(v) = h.strip_prefix("branch.ab ") {
                for part in v.split_whitespace() {
                    if let Some(n) = part.strip_prefix('+') {
                        s.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix('-') {
                        s.behind = n.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        // Path is after a fixed number of space-separated fields.
        let after = |n: usize| -> &[u8] {
            let mut k = 0;
            let mut seen = 0;
            while k < rec.len() && seen < n {
                if rec[k] == b' ' {
                    seen += 1;
                }
                k += 1;
            }
            &rec[k..]
        };
        match rec.first() {
            Some(b'1') | Some(b'2') => {
                let xy = rec.get(2..4).unwrap_or(b"..");
                let renamed = rec[0] == b'2';
                let path = path_of(after(if renamed { 9 } else { 8 }));
                let orig = if renamed {
                    let o = f.get(i).map(|o| path_of(o));
                    i += 1;
                    o
                } else {
                    None
                };
                if xy[0] != b'.' {
                    let c = code(xy[0]);
                    s.staged.push(Entry { path: path.clone(), orig: if c == 'R' { orig.clone() } else { None }, code: c });
                }
                if xy[1] != b'.' {
                    let c = code(xy[1]);
                    s.unstaged.push(Entry { path, orig: if c == 'R' { orig } else { None }, code: c });
                }
            }
            Some(b'u') => s.conflicted.push(path_of(after(10))),
            Some(b'?') => s.untracked.push(path_of(after(1))),
            _ => {}
        }
    }
    s
}

/// Stage paths (new, modified and deleted alike).
pub fn stage(root: &Path, paths: &[PathBuf]) -> Result<(), String> {
    let ps = rels(root, paths)?;
    if ps.is_empty() {
        return Ok(());
    }
    Git::new(root).args(["add", "-A", "--"]).args(&ps).ok().map(drop)
}

/// Unstage paths (`git restore --staged`; before the first commit, drop
/// them from the index instead). Unstaging a rename's new path also unstages
/// its old path.
pub fn unstage(root: &Path, paths: &[PathBuf]) -> Result<(), String> {
    let mut ps = rels(root, paths)?;
    if ps.is_empty() {
        return Ok(());
    }
    let st = status(root)?;
    for e in &st.staged {
        if let Some(o) = &e.orig
            && ps.contains(&e.path)
            && !ps.contains(o)
        {
            ps.push(o.clone());
        }
    }
    if st.head.is_none() {
        return Git::new(root).args(["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--"]).args(&ps).ok().map(drop);
    }
    Git::new(root).args(["restore", "--staged", "--"]).args(&ps).ok().map(drop)
}

/// Throw away unstaged changes: `git restore` tracked paths from the index,
/// delete untracked files. Destructive; the caller confirms first.
pub fn discard_unstaged(root: &Path, paths: &[PathBuf]) -> Result<(), String> {
    let ps = rels(root, paths)?;
    if ps.is_empty() {
        return Ok(());
    }
    let tracked: HashSet<PathBuf> =
        nul_fields(&Git::ro(root).args(["ls-files", "-z", "--"]).args(&ps).ok()?).into_iter().map(path_of).collect();
    let (restore, delete): (Vec<PathBuf>, Vec<PathBuf>) =
        ps.into_iter().partition(|p| tracked.contains(p) || tracked.iter().any(|t| t.starts_with(p)));
    if !restore.is_empty() {
        Git::new(root).args(["restore", "--worktree", "--"]).args(&restore).ok()?;
    }
    let mut failed = Vec::new();
    for p in delete {
        let abs = root.join(&p);
        match std::fs::symlink_metadata(&abs) {
            Ok(m) if m.is_dir() => failed.push(p), // never delete whole folders here
            Ok(_) => {
                if std::fs::remove_file(&abs).is_err() {
                    failed.push(p);
                }
            }
            Err(_) => {} // already gone
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(format!("Could not discard: {}", join_paths(&failed))) }
}

/// Commit what's staged (only). Runs the user's hooks. Returns the new commit.
pub fn commit(root: &Path, message: &str) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("Write a commit message first".into());
    }
    let staged = Git::ro(root).args(["diff", "--cached", "--quiet", "--ignore-submodules=none"]).run()?;
    if staged.code == 0 {
        return Err("Nothing is staged".into());
    }
    Git::new(root).args(ident_args(root)).args(["commit", "-q", "--cleanup=strip", "-m", message]).ok()?;
    Git::ro(root).args(["rev-parse", "HEAD"]).text()
}
