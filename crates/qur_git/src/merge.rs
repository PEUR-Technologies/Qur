//! Committing a task's work and merging its branch into the main checkout.
//!
//! The clean path is the M0 recipe (docs/spikes/acp-worktree.md): compute the
//! merge in memory with `merge-tree --write-tree`, make the commit with
//! `commit-tree`, move the checkout with a two-way `read-tree -m -u` (which
//! carries the user's staged and unstaged changes), then `update-ref` guarded
//! by the old HEAD. The user's index and stash are left as they were.

use std::path::{Path, PathBuf};

use crate::{Git, check_rev, ident_args, join_paths, nul_fields, path_of, rel, rev_parse};

#[derive(Clone, Debug, PartialEq)]
pub enum MergeOutcome {
    Merged { commit: String },
    UpToDate,
    Conflicts { files: Vec<PathBuf> },
    Refused { reason: String },
}

/// Stage everything in the worktree (respecting .gitignore) and commit it as
/// `message`. Author from the user's git config, else "Qur <qur@localhost>".
/// Hooks are skipped: this is Qur's bookkeeping commit on the task branch.
/// Ok(None) when there is nothing to commit.
pub fn commit_all(worktree: &Path, message: &str) -> Result<Option<String>, String> {
    Git::new(worktree).args(["add", "-A"]).ok()?;
    let head_exists = Git::ro(worktree).args(["rev-parse", "--verify", "--quiet", "HEAD"]).run()?.code == 0;
    if head_exists {
        let d = Git::ro(worktree).args(["diff", "--cached", "--quiet", "--ignore-submodules=none"]).run()?;
        if d.code == 0 {
            return Ok(None);
        }
    } else if Git::ro(worktree).args(["ls-files", "-z"]).ok()?.is_empty() {
        return Ok(None);
    }
    let msg = if message.trim().is_empty() { "qur: task" } else { message };
    Git::new(worktree)
        .args(ident_args(worktree))
        .args(["commit", "-q", "--no-verify", "--cleanup=strip", "-m", msg])
        .ok()?;
    Ok(Some(Git::ro(worktree).args(["rev-parse", "HEAD"]).text()?))
}

/// `merge-tree --write-tree` of HEAD and `theirs`: Ok(tree) when clean,
/// Err(Ok(files)) on conflicts, Err(Err(msg)) when git failed.
fn merge_tree(root: &Path, ours: &str, theirs: &str) -> Result<String, Result<Vec<PathBuf>, String>> {
    let out = Git::ro(root)
        .args(["merge-tree", "--write-tree", "-z", "--name-only", "--no-messages", ours, theirs])
        .run()
        .map_err(Err)?;
    let f = nul_fields(&out.stdout);
    match out.code {
        0 => f.first().map(|t| String::from_utf8_lossy(t).trim().to_string()).ok_or(Err("merge-tree printed nothing".into())),
        1 => {
            let mut files: Vec<PathBuf> = Vec::new();
            for p in f.iter().skip(1).take_while(|p| !p.is_empty()) {
                let p = path_of(p);
                if !files.contains(&p) {
                    files.push(p);
                }
            }
            Err(Ok(files))
        }
        _ => Err(Err(out.error())),
    }
}

/// Files that would conflict merging `branch` into the main checkout's HEAD;
/// empty = clean. Touches nothing.
pub fn precheck(root: &Path, branch: &str) -> Result<Vec<PathBuf>, String> {
    let head = rev_parse(root, "HEAD").map_err(|_| "The checkout has no commits yet".to_string())?;
    let theirs = rev_parse(root, branch)?;
    match merge_tree(root, &head, &theirs) {
        Ok(_) => Ok(Vec::new()),
        Err(Ok(files)) => Ok(files),
        Err(Err(e)) => Err(e),
    }
}

/// Every path with uncommitted state in the checkout: staged, unstaged,
/// untracked, conflicted (and the old side of renames).
fn dirty_paths(root: &Path, include_untracked: bool) -> Result<Vec<PathBuf>, String> {
    let s = crate::status(root)?;
    let mut v: Vec<PathBuf> = Vec::new();
    for e in s.staged.iter().chain(&s.unstaged) {
        v.push(e.path.clone());
        if let Some(o) = &e.orig {
            v.push(o.clone());
        }
    }
    v.extend(s.conflicted);
    if include_untracked {
        v.extend(s.untracked);
    }
    v.sort();
    v.dedup();
    Ok(v)
}

/// True when `a` and `b` are the same path or one contains the other.
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Merge `branch` into the main checkout's current branch with a merge commit
/// (the M0 recipe). Never touches the user's stash or unrelated index entries.
pub fn merge_clean(root: &Path, branch: &str, message: &str) -> MergeOutcome {
    match merge_inner(root, branch, message) {
        Ok(o) => o,
        Err(reason) => MergeOutcome::Refused { reason },
    }
}

fn merge_inner(root: &Path, branch: &str, message: &str) -> Result<MergeOutcome, String> {
    if is_merging(root) {
        return Err("A merge is already in progress in the checkout; finish or abort it first".into());
    }
    let old = rev_parse(root, "HEAD").map_err(|_| "The checkout has no commits yet".to_string())?;
    let theirs = rev_parse(root, branch)?;
    if Git::ro(root).args(["merge-base", "--is-ancestor", &theirs, &old]).run()?.code == 0 {
        return Ok(MergeOutcome::UpToDate);
    }
    let tree = match merge_tree(root, &old, &theirs) {
        Ok(t) => t,
        Err(Ok(files)) => return Ok(MergeOutcome::Conflicts { files }),
        Err(Err(e)) => return Err(e),
    };
    // Refuse up front when a dirty path would be overwritten (read-tree would
    // refuse too; this names every file at once).
    let changed = crate::diff::changed_paths(root, &old, &tree)?;
    let dirty = dirty_paths(root, true)?;
    let hit: Vec<PathBuf> = dirty.into_iter().filter(|d| changed.iter().any(|c| overlaps(c, d))).collect();
    if !hit.is_empty() {
        return Err(format!(
            "These files have uncommitted changes that the merge would overwrite: {}. Commit or stash them first.",
            join_paths(&hit)
        ));
    }
    let msg = if message.trim().is_empty() { format!("Merge {branch}") } else { message.to_string() };
    let merge = Git::new(root)
        .args(ident_args(root))
        .args(["commit-tree", &tree, "-p", &old, "-p", &theirs, "-m", &msg])
        .text()?;
    // Fresh stat info so unchanged-but-touched files don't read as dirty.
    let _ = Git::new(root).args(["update-index", "-q", "--refresh"]).run();
    if rev_parse(root, "HEAD").ok().as_deref() != Some(old.as_str()) {
        return Err("HEAD moved while merging; nothing was changed. Try again.".into());
    }
    let rt = Git::new(root).args(["read-tree", "-m", "-u", &old, &merge]).run()?;
    if rt.code != 0 {
        return Err(format!("Git refused to update the checkout; nothing was changed.\n{}", rt.error()));
    }
    let reflog = format!("merge {branch}: Merge made by Qur");
    let up = Git::new(root).args(["update-ref", "-m", &reflog, "HEAD", &merge, &old]).run()?;
    if up.code != 0 {
        // HEAD moved under us: put the files and index back the same way.
        let _ = Git::new(root).args(["read-tree", "-m", "-u", &merge, &old]).run();
        return Err(format!("HEAD moved while merging; nothing was merged. Try again.\n{}", up.error()));
    }
    Ok(MergeOutcome::Merged { commit: merge })
}

/// The conflict path: requires a clean checkout (no staged, unstaged or
/// conflicted files; untracked ones are fine unless the merge would overwrite
/// them, which git refuses). Runs `git merge --no-ff --no-commit <branch>` and
/// returns the conflicted files, which now hold conflict markers.
pub fn start_conflict_merge(root: &Path, branch: &str) -> Result<Vec<PathBuf>, String> {
    check_rev(branch)?;
    if is_merging(root) {
        return Err("A merge is already in progress; finish or abort it first".into());
    }
    let dirty = dirty_paths(root, false)?;
    if !dirty.is_empty() {
        return Err(format!("Commit or stash these changes before merging: {}", join_paths(&dirty)));
    }
    let theirs = rev_parse(root, branch)?;
    let head = rev_parse(root, "HEAD")?;
    if Git::ro(root).args(["merge-base", "--is-ancestor", &theirs, &head]).run()?.code == 0 {
        return Err("Already up to date: nothing to merge".into());
    }
    let out = Git::new(root)
        .args(ident_args(root))
        .args(["merge", "--no-ff", "--no-commit", "--no-edit", "--no-progress", branch])
        .run()?;
    if !is_merging(root) {
        return Err(out.error());
    }
    Ok(conflicted_files(root))
}

pub fn is_merging(root: &Path) -> bool {
    Git::ro(root).args(["rev-parse", "--verify", "--quiet", "MERGE_HEAD"]).run().is_ok_and(|o| o.code == 0)
}

/// Unmerged paths in the index.
pub fn conflicted_files(root: &Path) -> Vec<PathBuf> {
    let Ok(out) = Git::ro(root).args(["diff", "--name-only", "-z", "--diff-filter=U", "--no-relative"]).ok() else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = nul_fields(&out).into_iter().map(path_of).collect();
    v.dedup();
    v
}

/// After the user resolved every file: verify no markers remain (Err listing
/// the files that still have them), `git add` them, and commit the merge.
pub fn finish_merge(root: &Path, conflicted: &[PathBuf], message: &str) -> Result<String, String> {
    if !is_merging(root) {
        return Err("No merge in progress".into());
    }
    let ps: Vec<PathBuf> = conflicted.iter().map(|p| rel(root, p)).collect::<Result<_, _>>()?;
    let marked: Vec<PathBuf> = ps
        .iter()
        .filter(|p| {
            std::fs::read(root.join(p)).is_ok_and(|b| !crate::find_conflicts(&String::from_utf8_lossy(&b)).is_empty())
        })
        .cloned()
        .collect();
    if !marked.is_empty() {
        return Err(format!("These files still have conflict markers: {}", join_paths(&marked)));
    }
    if !ps.is_empty() {
        Git::new(root).args(["add", "-A", "--"]).args(&ps).ok()?;
    }
    let left = conflicted_files(root);
    if !left.is_empty() {
        return Err(format!("These files are still unresolved: {}", join_paths(&left)));
    }
    let msg = if message.trim().is_empty() { "Merge" } else { message };
    Git::new(root)
        .args(ident_args(root))
        .args(["commit", "-q", "--no-verify", "--cleanup=strip", "-m", msg])
        .ok()?;
    Git::ro(root).args(["rev-parse", "HEAD"]).text()
}

pub fn abort_merge(root: &Path) -> Result<(), String> {
    if !is_merging(root) {
        return Err("No merge in progress".into());
    }
    Git::new(root).args(["merge", "--abort"]).ok().map(drop)
}
