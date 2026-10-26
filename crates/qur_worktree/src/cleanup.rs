//! Clean Up Worktrees…: what Qur's task worktrees, `qur/*` branches and turn
//! checkpoints take, which of them no task owns any more, and removing those.
//!
//! Only things Qur made are listed: linked worktrees inside Qur's worktrees
//! folder (`<qur home>/worktrees/<project>/<slug>`), `refs/heads/qur/*` and
//! `refs/qur/turns/<task>/*`. The project folder, the user's own worktrees and
//! branches are never touched. Everything here blocks: call it off the UI thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::git;

const TURNS: &str = "refs/qur/turns/";
/// A worktree made this recently may belong to a task that is still starting
/// (its record isn't saved yet): never preselected.
const RECENT_SECS: u64 = 10 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A task worktree (folder and git's record of it), with its branch.
    Worktree,
    /// Git still lists the worktree but its folder is gone (`git worktree prune`).
    Missing,
    /// A `qur/*` branch that no worktree has checked out.
    Branch,
    /// Turn checkpoints (`refs/qur/turns/<task>/`) of a task that's gone.
    TurnRefs,
    /// A folder in Qur's worktrees folder that no repo knows as a worktree
    /// (its repo moved or was deleted, or git forgot it).
    Stray,
}

/// A task that still exists, as far as cleanup cares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnedTask {
    pub id: String,
    pub title: String,
    pub branch: String,
    pub worktree: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    /// The main checkout it belongs to (None for a stray folder).
    pub repo: Option<PathBuf>,
    pub path: Option<PathBuf>,
    /// Branch name without `refs/heads/` (Worktree, Missing, Branch).
    pub branch: Option<String>,
    /// The commit checked: the worktree's HEAD or the branch tip.
    pub sha: Option<String>,
    /// Turn refs: the task id.
    pub task: Option<String>,
    /// The title of the task that still owns it.
    pub owner: Option<String>,
    /// Disk use (allocated blocks), filled by `measure`.
    pub bytes: u64,
    /// Commits no other (non-`qur/`) local or remote branch has; None when git couldn't tell.
    pub unmerged: Option<u32>,
    /// Uncommitted or untracked changes (also when git couldn't tell).
    pub dirty: bool,
    /// `git worktree lock`ed: never removed here.
    pub locked: bool,
    /// Created in the last few minutes.
    pub recent: bool,
    /// Turn refs: (name, id) pairs.
    pub refs: Vec<(String, String)>,
}

impl Item {
    fn new(kind: Kind, repo: Option<&Path>) -> Item {
        Item {
            kind,
            repo: repo.map(Path::to_path_buf),
            path: None,
            branch: None,
            sha: None,
            task: None,
            owner: None,
            bytes: 0,
            unmerged: Some(0),
            dirty: false,
            locked: false,
            recent: false,
            refs: Vec::new(),
        }
    }

    /// What removing it would lose, for the user; empty when nothing.
    pub fn at_risk(&self) -> Vec<String> {
        let mut out = Vec::new();
        match self.unmerged {
            None => out.push("can't tell whether its commits are merged".to_string()),
            Some(0) => {}
            Some(1) => out.push("1 unmerged commit".to_string()),
            Some(n) => out.push(format!("{n} unmerged commits")),
        }
        if self.dirty {
            out.push("uncommitted changes".into());
        }
        if self.kind == Kind::Stray {
            out.push("not a git worktree any more, so its files can't be checked".into());
        }
        out
    }

    /// Removable from Clean Up at all (owned tasks are Discarded from their agent).
    pub fn removable(&self) -> bool {
        self.owner.is_none() && !self.locked
    }

    /// No task, nothing unmerged or uncommitted, not brand new: preselected.
    pub fn orphan(&self) -> bool {
        self.removable() && !self.recent && self.at_risk().is_empty()
    }

    /// The row's name: the branch, the folder, or the task of turn refs.
    pub fn label(&self) -> String {
        let folder = || self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match self.kind {
            Kind::Worktree => self.branch.clone().unwrap_or_else(|| format!("{} (detached)", folder())),
            Kind::Missing => format!("{} (folder gone)", self.branch.clone().unwrap_or_else(folder)),
            Kind::Branch => format!("{} (no worktree)", self.branch.clone().unwrap_or_default()),
            Kind::TurnRefs => {
                let n = self.refs.len();
                format!("turn checkpoints of “{}” ({n} ref{})", self.task.clone().unwrap_or_default(), if n == 1 { "" } else { "s" })
            }
            Kind::Stray => format!("{} (unknown folder)", folder()),
        }
    }

    /// Same thing (to find it again after a fresh look).
    fn same(&self, other: &Item) -> bool {
        let p = |i: &Item| i.path.as_deref().map(canon);
        self.kind == other.kind && self.repo.as_deref().map(canon) == other.repo.as_deref().map(canon) && p(self) == p(other) && self.branch == other.branch && self.task == other.task
    }
}

/// The path with symlinks resolved; for a missing path, its parent's (so
/// `/var/…` and `/private/var/…` compare equal even after a folder is gone).
pub fn canon(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) => canon(parent).join(name),
        _ => p.to_path_buf(),
    }
}

/// The main checkout a linked worktree belongs to, read from its `.git`
/// file (`gitdir: <repo>/.git/worktrees/<name>`, whose `commondir` leads
/// back to `<repo>/.git`). None for anything else, or when that repo is gone.
pub fn main_checkout(worktree: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(worktree.join(".git")).ok()?;
    let gitdir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_absolute() { gitdir } else { worktree.join(gitdir) };
    let common = match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(c) => gitdir.join(c.trim()),
        Err(_) => gitdir.parent()?.parent()?.to_path_buf(),
    };
    let common = std::fs::canonicalize(common).ok()?;
    if common.file_name()? != ".git" {
        return None; // a bare repo: no checkout to work from
    }
    let repo = common.parent()?.to_path_buf();
    repo.join(".git").is_dir().then_some(repo)
}

/// True when `path` sits where Qur makes worktrees: exactly
/// `<qur_dir>/<project>/<slug>`.
pub fn in_qur_dir(path: &Path, qur_dir: &Path) -> bool {
    let (p, q) = (canon(path), canon(qur_dir));
    p.parent().and_then(Path::parent) == Some(q.as_path())
}

struct Listed {
    path: PathBuf,
    head: Option<String>,
    branch: Option<String>,
    locked: bool,
    prunable: bool,
}

fn worktree_list(repo: &Path) -> Result<Vec<Listed>, String> {
    let out = git(repo, &["worktree", "list", "--porcelain"])?;
    let mut list = Vec::new();
    for block in out.split("\n\n") {
        let mut w = Listed { path: PathBuf::new(), head: None, branch: None, locked: false, prunable: false };
        for line in block.lines() {
            let (key, val) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "worktree" => w.path = PathBuf::from(val),
                "HEAD" => w.head = Some(val.to_string()),
                "branch" => w.branch = Some(val.strip_prefix("refs/heads/").unwrap_or(val).to_string()),
                "locked" => w.locked = true,
                "prunable" => w.prunable = true,
                _ => {}
            }
        }
        if !w.path.as_os_str().is_empty() {
            list.push(w);
        }
    }
    Ok(list)
}

fn refs_under(repo: &Path, prefix: &str) -> Result<Vec<(String, String)>, String> {
    let out = git(repo, &["for-each-ref", "--format=%(refname) %(objectname)", prefix])?;
    Ok(out.lines().filter_map(|l| l.split_once(' ')).map(|(n, id)| (n.to_string(), id.to_string())).collect())
}

/// Commits reachable from `rev` that no other branch has: local branches and
/// remote-tracking ones, leaving out Qur's own `qur/*` branches.
fn unmerged(repo: &Path, rev: &str) -> Option<u32> {
    git(repo, &["rev-list", "--count", rev, "--not", "--exclude=qur/*", "--branches", "--exclude=*/qur/*", "--remotes"]).ok()?.parse().ok()
}

/// Uncommitted or untracked changes; an error counts as dirty.
fn dirty(path: &Path) -> bool {
    git(path, &["status", "--porcelain"]).map(|s| !s.is_empty()).unwrap_or(true)
}

fn recent(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.created().or_else(|_| m.modified()))
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() < RECENT_SECS)
}

/// Everything Qur made in `repo`: its worktrees in `qur_dir`, `qur/*`
/// branches without a worktree, and turn refs, each with whether a task in
/// `owned` still has it, unmerged commits and uncommitted changes. Sizes
/// are left at 0 (`measure`).
pub fn survey(repo: &Path, qur_dir: &Path, owned: &[OwnedTask]) -> Result<Vec<Item>, String> {
    let owned_paths: Vec<(PathBuf, &OwnedTask)> = owned.iter().map(|t| (canon(&t.worktree), t)).collect();
    let by_branch = |b: &str| owned.iter().find(|t| t.branch == b);
    let list = worktree_list(repo)?;
    let mut items = Vec::new();
    let checked_out: HashSet<&str> = list.iter().filter_map(|w| w.branch.as_deref()).collect();
    for w in &list {
        if !in_qur_dir(&w.path, qur_dir) {
            continue; // the main checkout, or the user's own worktrees
        }
        let gone = w.prunable || !w.path.exists();
        let mut it = Item::new(if gone { Kind::Missing } else { Kind::Worktree }, Some(repo));
        let cp = canon(&w.path);
        it.owner = owned_paths
            .iter()
            .find(|(p, _)| *p == cp)
            .map(|(_, t)| t.title.clone())
            .or_else(|| w.branch.as_deref().and_then(by_branch).map(|t| t.title.clone()));
        it.path = Some(w.path.clone());
        it.branch = w.branch.clone();
        it.sha = w.head.clone();
        it.locked = w.locked;
        it.unmerged = w.head.as_deref().and_then(|h| unmerged(repo, h));
        if !gone {
            it.dirty = dirty(&w.path);
            it.recent = recent(&w.path);
        }
        items.push(it);
    }
    for (name, id) in refs_under(repo, "refs/heads/qur/")? {
        let b = name.trim_start_matches("refs/heads/");
        if checked_out.contains(b) {
            continue;
        }
        let mut it = Item::new(Kind::Branch, Some(repo));
        it.owner = by_branch(b).map(|t| t.title.clone());
        it.unmerged = unmerged(repo, &id);
        it.branch = Some(b.to_string());
        it.sha = Some(id);
        items.push(it);
    }
    let mut turns: Vec<Item> = Vec::new();
    for (name, id) in refs_under(repo, TURNS)? {
        let Some(task) = name[TURNS.len()..].split('/').next().filter(|t| !t.is_empty()) else { continue };
        match turns.iter_mut().find(|t| t.task.as_deref() == Some(task)) {
            Some(t) => t.refs.push((name, id)),
            None => {
                let mut it = Item::new(Kind::TurnRefs, Some(repo));
                it.owner = owned.iter().find(|t| t.id == task).map(|t| t.title.clone());
                it.task = Some(task.to_string());
                it.refs.push((name, id));
                turns.push(it);
            }
        }
    }
    items.extend(turns);
    Ok(items)
}

/// Folders in `qur_dir/<project>/` that aren't among `known` worktree paths.
pub fn strays(qur_dir: &Path, known: &[PathBuf]) -> Vec<Item> {
    let known: HashSet<PathBuf> = known.iter().map(|p| canon(p)).collect();
    let mut out = Vec::new();
    for project in subdirs(qur_dir) {
        for path in subdirs(&project) {
            if !known.contains(&canon(&path)) {
                let mut it = Item::new(Kind::Stray, None);
                it.unmerged = None;
                it.recent = recent(&path);
                it.path = Some(path);
                out.push(it);
            }
        }
    }
    out
}

/// Every repo with worktrees in `qur_dir` (from the worktrees' `.git` files).
pub fn repos_in(qur_dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for project in subdirs(qur_dir) {
        for path in subdirs(&project) {
            if let Some(r) = main_checkout(&path)
                && !out.contains(&r)
            {
                out.push(r);
            }
        }
    }
    out
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Disk use of a folder like `du`: allocated blocks, symlinks not followed,
/// hard-linked files counted once. (APFS clones from `.worktreeinclude`
/// share their blocks, so this can count more than the disk actually lost.)
pub fn disk_use(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if m.nlink() > 1 && !m.is_dir() && !seen.insert((m.dev(), m.ino())) {
            continue;
        }
        total += m.blocks() * 512;
        if m.is_dir()
            && let Ok(rd) = std::fs::read_dir(&p)
        {
            stack.extend(rd.filter_map(Result::ok).map(|e| e.path()));
        }
    }
    total
}

/// Fill in `bytes` for every item with a folder, a few folders at a time.
pub fn measure(items: &mut [Item]) {
    let n = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4);
    let chunk = items.len().div_ceil(n).max(1);
    std::thread::scope(|s| {
        for part in items.chunks_mut(chunk) {
            s.spawn(move || {
                for it in part {
                    if let Some(p) = it.path.as_ref().filter(|p| p.exists()) {
                        it.bytes = disk_use(p);
                    }
                }
            });
        }
    });
}

/// Delete a ref only if it still points where it did when checked.
fn delete_refs(repo: &Path, refs: &[(String, String)]) -> Result<(), String> {
    if refs.is_empty() {
        return Ok(());
    }
    let cmds: String = refs.iter().map(|(n, id)| format!("delete {n} {id}\n")).collect();
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["update-ref", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(cmds.as_bytes()).map_err(|e| format!("git: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// Git's record of a worktree whose folder is gone: the admin folder under
/// `<repo>/.git/worktrees/` whose `gitdir` names `<path>/.git`. Removing
/// just that one is `git worktree prune` limited to this worktree.
fn forget_missing(repo: &Path, path: &Path) -> Result<(), String> {
    let want = canon(&path.join(".git"));
    let admin = repo.join(".git").join("worktrees");
    for d in subdirs(&admin) {
        let Ok(g) = std::fs::read_to_string(d.join("gitdir")) else { continue };
        if canon(Path::new(g.trim())) == want {
            return std::fs::remove_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()));
        }
    }
    Err(format!("git has no record of {}", path.display()))
}

/// Remove one item. It is looked at again first: still owned by a task in
/// `owned`, or (unless `allow_loss`) holding unmerged commits, uncommitted
/// changes or unknown files, and it's refused. Branches and refs are deleted
/// only if they still point at the commit just checked. Never touches the
/// project folder or anything outside `<qur_dir>/<project>/<slug>`.
pub fn remove_item(item: &Item, qur_dir: &Path, owned: &[OwnedTask], allow_loss: bool) -> Result<(), String> {
    if let Some(p) = &item.path {
        if !in_qur_dir(p, qur_dir) {
            return Err(format!("refusing to remove {}: not in Qur's worktrees folder", p.display()));
        }
        if let Some(r) = &item.repo
            && canon(r) == canon(p)
        {
            return Err("refusing to remove the project folder".into());
        }
    }
    // Look again: the state may have changed since the list was shown.
    let fresh = match &item.repo {
        Some(repo) => survey(repo, qur_dir, owned)?.into_iter().find(|i| i.same(item)).ok_or_else(|| format!("{} is already gone", item.label()))?,
        None => {
            let path = item.path.as_ref().ok_or("nothing to remove")?;
            if let Some(repo) = main_checkout(path)
                && worktree_list(&repo)?.iter().any(|w| canon(&w.path) == canon(path))
            {
                return Err(format!("{} is a worktree again; look again", path.display()));
            }
            let mut it = item.clone();
            it.unmerged = None;
            it
        }
    };
    if let Some(t) = &fresh.owner {
        return Err(format!("{} belongs to the task “{t}”; discard it from its agent", fresh.label()));
    }
    if fresh.locked {
        return Err(format!("{} is locked (git worktree unlock)", fresh.label()));
    }
    let risk = fresh.at_risk();
    if !allow_loss && !risk.is_empty() {
        return Err(format!("{}: {}", fresh.label(), risk.join(", ")));
    }
    let branch_ref = || fresh.branch.as_ref().zip(fresh.sha.as_ref()).map(|(b, id)| vec![(format!("refs/heads/{b}"), id.clone())]).unwrap_or_default();
    match (fresh.kind, &fresh.repo, &fresh.path) {
        (Kind::Worktree, Some(repo), Some(path)) => {
            let p = path.to_str().ok_or("non-UTF-8 path")?;
            let mut args = vec!["worktree", "remove"];
            if allow_loss {
                args.push("--force");
            }
            args.push(p);
            git(repo, &args)?;
            delete_refs(repo, &branch_ref())
        }
        (Kind::Missing, Some(repo), Some(path)) => {
            forget_missing(repo, path)?;
            delete_refs(repo, &branch_ref())
        }
        (Kind::Branch, Some(repo), _) => delete_refs(repo, &branch_ref()),
        (Kind::TurnRefs, Some(repo), _) => delete_refs(repo, &fresh.refs),
        (Kind::Stray, None, Some(path)) => std::fs::remove_dir_all(path).map_err(|e| format!("{}: {e}", path.display())),
        _ => Err(format!("can't remove {}", fresh.label())),
    }
}

/// Remove empty `<qur_dir>/<project>/` folders left after a cleanup.
pub fn prune_empty(qur_dir: &Path) {
    for project in subdirs(qur_dir) {
        let _ = std::fs::remove_dir(project); // fails (keeps it) unless empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{create, remove};

    fn repo(name: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("qur-clean-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let r = base.join("repo");
        std::fs::create_dir_all(&r).unwrap();
        git(&r, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(r.join("a.txt"), "a\n").unwrap();
        std::fs::write(r.join(".gitignore"), "node_modules/\n").unwrap();
        commit_all(&r, "init");
        (r, base.join("worktrees"))
    }

    fn commit_all(r: &Path, msg: &str) {
        git(r, &["add", "-A"]).unwrap();
        git(r, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", msg]).unwrap();
    }

    fn find<'a>(items: &'a [Item], label: &str) -> &'a Item {
        items.iter().find(|i| i.label() == label).unwrap_or_else(|| panic!("no {label} in {:?}", items.iter().map(Item::label).collect::<Vec<_>>()))
    }

    fn old(items: &mut [Item]) {
        for i in items {
            i.recent = false; // as if made long ago
        }
    }

    #[test]
    fn orphans_owned_unmerged_and_dirty() {
        let (r, q) = repo("kinds");
        let dir = q.join("proj");
        let owned_wt = create(&r, &dir, "claude", "owned").unwrap();
        let orphan = create(&r, &dir, "claude", "orphan").unwrap();
        let work = create(&r, &dir, "claude", "work").unwrap();
        let wip = create(&r, &dir, "claude", "wip").unwrap();
        // Ignored files (node_modules) don't make a worktree dirty, and count on disk.
        std::fs::create_dir_all(orphan.path.join("node_modules")).unwrap();
        std::fs::write(orphan.path.join("node_modules/big.js"), vec![b'x'; 64 * 1024]).unwrap();
        std::fs::write(work.path.join("w.txt"), "w\n").unwrap();
        commit_all(&work.path, "agent work");
        std::fs::write(wip.path.join("a.txt"), "edited\n").unwrap();
        // A branch the user made isn't Qur's; a worktree of theirs elsewhere isn't either.
        git(&r, &["branch", "mine"]).unwrap();
        let theirs = r.with_file_name("their-wt");
        git(&r, &["worktree", "add", "-q", "-b", "theirs", theirs.to_str().unwrap()]).unwrap();
        let owned = vec![OwnedTask { id: "owned".into(), title: "Owned task".into(), branch: owned_wt.branch.clone(), worktree: owned_wt.path.clone() }];
        let mut items = survey(&r, &q, &owned).unwrap();
        assert_eq!(items.len(), 4, "{:?}", items.iter().map(Item::label).collect::<Vec<_>>());
        assert!(items.iter().all(|i| i.recent), "just made");
        assert!(!items.iter().any(|i| i.orphan()), "nothing brand new is preselected");
        old(&mut items);
        measure(&mut items);
        let o = find(&items, "qur/claude/orphan");
        assert!(o.orphan() && o.at_risk().is_empty() && !o.dirty && o.unmerged == Some(0));
        assert!(o.bytes >= 64 * 1024, "{}", o.bytes);
        let w = find(&items, "qur/claude/owned");
        assert_eq!(w.owner.as_deref(), Some("Owned task"));
        assert!(!w.removable() && !w.orphan());
        let k = find(&items, "qur/claude/work");
        assert_eq!((k.unmerged, k.dirty, k.orphan()), (Some(1), false, false));
        assert_eq!(k.at_risk(), ["1 unmerged commit"]);
        let d = find(&items, "qur/claude/wip");
        assert_eq!((d.unmerged, d.dirty, d.orphan()), (Some(0), true, false));
        assert_eq!(d.at_risk(), ["uncommitted changes"]);
        // Merged into a branch of the user's: no longer unmerged.
        git(&r, &["merge", "-q", "--no-edit", &work.branch]).unwrap();
        let k = survey(&r, &q, &owned).unwrap().into_iter().find(|i| i.label() == "qur/claude/work").unwrap();
        assert_eq!(k.unmerged, Some(0));
    }

    #[test]
    fn removes_only_whats_safe_unless_allowed() {
        let (r, q) = repo("remove");
        let dir = q.join("proj");
        let orphan = create(&r, &dir, "claude", "orphan").unwrap();
        let work = create(&r, &dir, "claude", "work").unwrap();
        let wip = create(&r, &dir, "claude", "wip").unwrap();
        let owned_wt = create(&r, &dir, "claude", "owned").unwrap();
        std::fs::write(work.path.join("w.txt"), "w\n").unwrap();
        commit_all(&work.path, "agent work");
        std::fs::write(wip.path.join("new.txt"), "untracked\n").unwrap();
        let owned = vec![OwnedTask { id: "owned".into(), title: "Owned".into(), branch: owned_wt.branch.clone(), worktree: owned_wt.path.clone() }];
        let items = survey(&r, &q, &owned).unwrap();
        let get = |l: &str| find(&items, l).clone();

        remove_item(&get("qur/claude/orphan"), &q, &owned, false).unwrap();
        assert!(!orphan.path.exists());
        assert!(git(&r, &["rev-parse", "--verify", "--quiet", &orphan.branch]).is_err(), "branch gone too");

        let e = remove_item(&get("qur/claude/work"), &q, &owned, false).unwrap_err();
        assert!(e.contains("1 unmerged commit"), "{e}");
        let e = remove_item(&get("qur/claude/wip"), &q, &owned, false).unwrap_err();
        assert!(e.contains("uncommitted changes"), "{e}");
        assert!(work.path.join("w.txt").exists() && wip.path.join("new.txt").exists(), "refused means untouched");
        // Owned: refused even when allowed to lose work.
        let e = remove_item(&get("qur/claude/owned"), &q, &owned, true).unwrap_err();
        assert!(e.contains("belongs to the task"), "{e}");
        assert!(owned_wt.path.exists());
        // A shown-clean worktree that got dirty since is refused on the second look.
        let clean = create(&r, &dir, "claude", "clean").unwrap();
        let shown = survey(&r, &q, &owned).unwrap().into_iter().find(|i| i.label() == "qur/claude/clean").unwrap();
        assert!(shown.at_risk().is_empty());
        std::fs::write(clean.path.join("late.txt"), "x").unwrap();
        assert!(remove_item(&shown, &q, &owned, false).unwrap_err().contains("uncommitted"));
        // With the separate confirmation: removed, unmerged branch included.
        remove_item(&get("qur/claude/work"), &q, &owned, true).unwrap();
        remove_item(&get("qur/claude/wip"), &q, &owned, true).unwrap();
        assert!(!work.path.exists() && !wip.path.exists());
        assert!(git(&r, &["rev-parse", "--verify", "--quiet", &work.branch]).is_err());
        // Refs are deleted only at the commit checked: a branch that moved isn't.
        git(&r, &["branch", "qur/claude/moving"]).unwrap();
        let old_tip = git(&r, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(r.join("m.txt"), "m\n").unwrap();
        commit_all(&r, "moved on");
        git(&r, &["branch", "-f", "qur/claude/moving", "HEAD"]).unwrap();
        assert!(delete_refs(&r, &[("refs/heads/qur/claude/moving".into(), old_tip)]).is_err());
        assert!(git(&r, &["rev-parse", "--verify", "--quiet", "qur/claude/moving"]).is_ok());
        assert!(r.join(".git").exists() && r.join("a.txt").exists(), "the project is never touched");
    }

    #[test]
    fn branches_refs_missing_and_strays() {
        let (r, q) = repo("refs");
        let dir = q.join("proj");
        // A qur/* branch with no worktree, merged, and one with unmerged work.
        git(&r, &["branch", "qur/claude/left"]).unwrap();
        let w = create(&r, &dir, "claude", "unm").unwrap();
        std::fs::write(w.path.join("u.txt"), "u\n").unwrap();
        commit_all(&w.path, "u");
        git(&r, &["worktree", "remove", w.path.to_str().unwrap()]).unwrap();
        // A worktree whose folder was deleted by hand.
        let gone = create(&r, &dir, "claude", "gone").unwrap();
        std::fs::remove_dir_all(&gone.path).unwrap();
        // Turn refs: one task alive, one gone.
        let head = git(&r, &["rev-parse", "HEAD^{tree}"]).unwrap();
        for t in ["alive", "dead"] {
            for side in ["start", "end"] {
                git(&r, &["update-ref", &format!("refs/qur/turns/{t}/1/{side}"), &head]).unwrap();
            }
        }
        // The live task's worktree (owned).
        let alive = create(&r, &dir, "claude", "alive").unwrap();
        // A folder no repo knows.
        std::fs::create_dir_all(q.join("other/leftover/src")).unwrap();
        std::fs::write(q.join("other/leftover/src/x.rs"), "fn main() {}\n").unwrap();
        let owned = vec![OwnedTask { id: "alive".into(), title: "Alive".into(), branch: "qur/claude/alive".into(), worktree: dir.join("alive") }];
        let items = survey(&r, &q, &owned).unwrap();
        let left = find(&items, "qur/claude/left (no worktree)");
        assert!(left.orphan(), "{left:?}");
        let unm = find(&items, "qur/claude/unm (no worktree)");
        assert_eq!(unm.unmerged, Some(1));
        let g = find(&items, "qur/claude/gone (folder gone)");
        assert_eq!(g.kind, Kind::Missing);
        assert!(g.orphan(), "{g:?}");
        let dead = find(&items, "turn checkpoints of “dead” (2 refs)");
        assert!(dead.orphan());
        assert!(!find(&items, "turn checkpoints of “alive” (2 refs)").removable());
        let known: Vec<PathBuf> = items.iter().filter_map(|i| i.path.clone()).collect();
        let strays = strays(&q, &known);
        assert_eq!(strays.len(), 1);
        assert_eq!(strays[0].label(), "leftover (unknown folder)");
        assert!(!strays[0].orphan() && strays[0].at_risk().iter().any(|r| r.contains("can't be checked")));
        assert_eq!(repos_in(&q), vec![canon(&r)], "repos found from their worktrees");

        for it in [left, g, dead] {
            remove_item(it, &q, &owned, false).unwrap();
        }
        assert!(git(&r, &["rev-parse", "--verify", "--quiet", "qur/claude/left"]).is_err());
        assert!(git(&r, &["rev-parse", "--verify", "--quiet", &gone.branch]).is_err());
        assert_eq!(git(&r, &["worktree", "list"]).unwrap().lines().count(), 2, "git forgot the missing worktree");
        assert!(alive.path.exists());
        let refs = git(&r, &["for-each-ref", "--format=%(refname)", "refs/qur/"]).unwrap();
        assert_eq!(refs.lines().collect::<Vec<_>>(), ["refs/qur/turns/alive/1/end", "refs/qur/turns/alive/1/start"]);
        assert!(remove_item(unm, &q, &owned, false).unwrap_err().contains("unmerged"));
        remove_item(unm, &q, &owned, true).unwrap();
        assert!(remove_item(&strays[0], &q, &owned, false).unwrap_err().contains("can't be checked"));
        remove_item(&strays[0], &q, &owned, true).unwrap();
        prune_empty(&q);
        assert!(!q.join("other").exists());
        assert!(r.join(".git").exists() && r.join("a.txt").exists());
    }

    #[test]
    fn never_outside_the_qur_folder_or_the_project() {
        let (r, q) = repo("guard");
        for p in [r.clone(), q.clone(), q.join("proj"), r.with_file_name("elsewhere").join("x")] {
            let mut it = Item::new(Kind::Stray, None);
            it.path = Some(p.clone());
            let e = remove_item(&it, &q, &[], true).unwrap_err();
            assert!(e.contains("not in Qur's worktrees folder"), "{p:?}: {e}");
        }
        // The project folder itself, even if it were (by a symlink) in Qur's folder.
        std::fs::create_dir_all(q.join("proj")).unwrap();
        let link = q.join("proj/repo-link");
        std::os::unix::fs::symlink(&r, &link).unwrap();
        let mut it = Item::new(Kind::Worktree, Some(&r));
        it.path = Some(link.clone());
        assert!(remove_item(&it, &q, &[], true).is_err());
        assert!(r.join(".git").exists() && r.join("a.txt").exists());
        // `remove` keeps its own guarantee.
        assert!(remove(&r, &r, "main", true).is_err());
    }

    #[test]
    fn disk_use_counts_blocks_once() {
        let (r, _) = repo("du");
        let d = r.join("sizes");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a"), vec![1u8; 100_000]).unwrap();
        std::fs::hard_link(d.join("a"), d.join("b")).unwrap();
        std::os::unix::fs::symlink("/usr", d.join("link")).unwrap();
        let n = disk_use(&d);
        assert!((100_000..200_000).contains(&n), "{n}");
    }
}
