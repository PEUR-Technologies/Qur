//! Turn checkpoints: a tree of an agent's worktree at the start and end of
//! each turn, what a turn changed, and Rewind (put the worktree back to a
//! tree).
//!
//! Snapshots go through a throwaway copy of the worktree's index (like
//! `task_diff`), so the real index, HEAD and the stash are never touched;
//! only blobs and trees are written to the object store. They are pinned
//! under `refs/qur/turns/<task>/<n>/start|end` so `git gc` keeps them. Refs
//! that point at trees are skipped by `git log --all`, so they don't show up
//! as history.

use std::path::Path;

use crate::diff::{DIFF_ARGS, index_path, parse_patch};
use crate::{FileDiff, Git, TempFile, check_rev, nul_fields, temp_name};

/// Where a task's turn refs live.
pub const TURNS_REF_PREFIX: &str = "refs/qur/turns";

/// A copy of the checkout's index with every change in the working tree
/// staged (`add -A`: modified, deleted and untracked files, respecting
/// .gitignore). Deleted on drop.
fn staged_copy(worktree: &Path) -> Result<TempFile, String> {
    // In a task's own worktree (a `.git` file), refresh its index's stat
    // cache first, as `task_diff` does: content-neutral, and without it a
    // fresh worktree's "racy" entries make every snapshot re-hash every
    // file. Never in a main checkout (the user's index). Skipped when locked.
    if worktree.join(".git").is_file() {
        let _ = Git::new(worktree).args(["update-index", "-q", "--refresh", "--unmerged"]).run();
    }
    let real = index_path(worktree)?;
    let dir = real.parent().ok_or("no git dir")?;
    let tmp = TempFile(dir.join(temp_name("turn-index")));
    if real.exists() {
        std::fs::copy(&real, &tmp.0).map_err(|e| format!("copy index: {e}"))?;
    }
    Git::new(worktree)
        .env("GIT_INDEX_FILE", &tmp.0)
        .args(["-c", "core.splitIndex=false", "add", "-A", "--ignore-errors"])
        .ok()?;
    Ok(tmp)
}

/// The tree of everything in the worktree right now (tracked and untracked,
/// not ignored), as a tree id. Writes objects only.
pub fn snapshot_tree(worktree: &Path) -> Result<String, String> {
    let idx = staged_copy(worktree)?;
    let tree = Git::new(worktree).env("GIT_INDEX_FILE", &idx.0).args(["write-tree"]).text()?;
    if tree.len() < 40 || !tree.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("write-tree gave {tree:?}"));
    }
    Ok(tree)
}

/// What changed between two trees, in the same shape as `task_diff`.
pub fn tree_diff(worktree: &Path, from: &str, to: &str) -> Result<Vec<FileDiff>, String> {
    check_rev(from)?;
    check_rev(to)?;
    let out = Git::ro(worktree).args(DIFF_ARGS).arg(from).arg(to).arg("--").ok()?;
    let mut files = parse_patch(&out);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Files changed, lines added and removed between two trees (binary files
/// count as changed with 0 lines).
pub fn tree_stats(worktree: &Path, from: &str, to: &str) -> Result<(u32, u32, u32), String> {
    check_rev(from)?;
    check_rev(to)?;
    let out = Git::ro(worktree)
        .args(["diff", "--numstat", "-z", "-M", "--no-ext-diff", "--no-textconv", "--no-relative"])
        .arg(from)
        .arg(to)
        .arg("--")
        .ok()?;
    let (mut files, mut a, mut r) = (0u32, 0u32, 0u32);
    let f = nul_fields(&out);
    let mut i = 0;
    while i < f.len() {
        let mut parts = f[i].splitn(3, |&c| c == b'\t');
        let num = |b: Option<&[u8]>| b.and_then(|b| std::str::from_utf8(b).ok()).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        a = a.saturating_add(num(parts.next()));
        r = r.saturating_add(num(parts.next()));
        let path = parts.next().unwrap_or_default();
        files += 1;
        // A rename's paths follow as two more fields.
        i += if path.is_empty() { 3 } else { 1 };
    }
    Ok((files, a, r))
}

/// `refs/qur/turns/<task>/<n>/<side>`; refuses task ids that aren't one
/// plain ref component.
pub fn turn_ref(task: &str, n: u32, side: &str) -> Result<String, String> {
    let ok = !task.is_empty()
        && !task.starts_with('.')
        && !task.ends_with(".lock")
        && !task.contains("..")
        && task.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if !ok || !matches!(side, "start" | "end") {
        return Err(format!("invalid turn ref for task {task:?}"));
    }
    Ok(format!("{TURNS_REF_PREFIX}/{task}/{n}/{side}"))
}

/// Pin a snapshot so gc can't prune it (`side` is "start" or "end"). Refs
/// are shared by the repo and all its worktrees.
pub fn pin_turn(repo: &Path, task: &str, n: u32, side: &str, tree: &str) -> Result<(), String> {
    check_rev(tree)?;
    let name = turn_ref(task, n, side)?;
    Git::new(repo).args(["update-ref", "--no-deref", "-m", "qur turn"]).arg(&name).arg(tree).ok().map(|_| ())
}

/// Delete every turn ref of a task (Discard, Merge cleanup). Fine when
/// there are none.
pub fn delete_turn_refs(repo: &Path, task: &str) -> Result<(), String> {
    turn_ref(task, 0, "start")?;
    delete_refs_under(repo, &format!("{TURNS_REF_PREFIX}/{task}/"))
}

/// Delete one turn's refs (a turn that changed nothing keeps no checkpoint).
pub fn unpin_turn(repo: &Path, task: &str, n: u32) -> Result<(), String> {
    turn_ref(task, n, "start")?;
    delete_refs_under(repo, &format!("{TURNS_REF_PREFIX}/{task}/{n}/"))
}

/// Paths that differ between two trees (renames as two paths).
pub fn changed_paths_between(worktree: &Path, from: &str, to: &str) -> Result<Vec<std::path::PathBuf>, String> {
    check_rev(from)?;
    check_rev(to)?;
    let out = Git::ro(worktree).args(["diff", "--name-only", "-z", "--no-renames", "--no-relative", "--no-ext-diff"]).arg(from).arg(to).arg("--").ok()?;
    Ok(nul_fields(&out).into_iter().map(crate::path_of).collect())
}

fn delete_refs_under(repo: &Path, prefix: &str) -> Result<(), String> {
    let out = Git::ro(repo).args(["for-each-ref", "--format=%(refname)"]).arg(prefix).text()?;
    let mut cmds = String::new();
    for r in out.lines().filter(|l| l.starts_with(prefix)) {
        cmds.push_str(&format!("delete {r}\n"));
    }
    if cmds.is_empty() {
        return Ok(());
    }
    Git::new(repo).args(["update-ref", "--stdin"]).stdin(cmds.into_bytes()).ok().map(|_| ())
}

/// Put the worktree's files back to `tree` (Rewind): files the tree has are
/// written, files it doesn't have are deleted (ignored files are left
/// alone). Then the worktree's index is reset to its HEAD, so the restored
/// state shows as plain uncommitted changes. HEAD, branches and the stash
/// are untouched.
///
/// Only in a task's own linked worktree: refuses the project folder (and any
/// main checkout), like `qur_worktree::remove`.
pub fn restore_tree(project: &Path, worktree: &Path, tree: &str) -> Result<(), String> {
    check_rev(tree)?;
    let same = std::fs::canonicalize(project).ok().zip(std::fs::canonicalize(worktree).ok()).is_some_and(|(a, b)| a == b) || project == worktree;
    if same {
        return Err("refusing to rewind the project folder".into());
    }
    // A linked worktree has a `.git` file; a `.git` folder is a main checkout.
    if !worktree.join(".git").is_file() {
        return Err(format!("refusing to rewind {}: not a task worktree", worktree.display()));
    }
    Git::ro(worktree).args(["cat-file", "-e"]).arg(format!("{tree}^{{tree}}")).ok().map_err(|_| format!("snapshot {tree} is gone"))?;
    // Read the current state (incl. untracked files) into a scratch index, so
    // the one-tree reset knows what to delete, then switch it to `tree`.
    let idx = staged_copy(worktree)?;
    Git::new(worktree)
        .env("GIT_INDEX_FILE", &idx.0)
        .args(["-c", "core.splitIndex=false", "read-tree", "-u", "--reset"])
        .arg(tree)
        .ok()?;
    // The worktree's own index back to HEAD (a mixed reset; Qur's worktree).
    Git::new(worktree).args(["reset", "-q"]).ok().map(|_| ())
}
