//! File tree model for the Files dropdown, plus the project file list for
//! Quick Open. Folders load lazily (on expand) so huge repos open instantly.
//! Respects .gitignore / .ignore / global git excludes; shows dotfiles except `.git`.
//! A task's tree also shows the files its agent deleted, as ghost rows ([`FileTree::set_ghosts`]).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

pub type NodeId = usize;

#[derive(Debug)]
pub struct Node {
    pub name: String,
    /// Path relative to the tree root.
    pub rel: PathBuf,
    pub depth: usize,
    pub parent: Option<NodeId>,
    pub kind: Kind,
    /// Not on disk: a deleted file (or a folder that only holds deleted files), shown so
    /// the deletion is visible.
    pub ghost: bool,
}

#[derive(Debug)]
pub enum Kind {
    File,
    Dir { expanded: bool, children: Option<Vec<NodeId>> },
}

impl Node {
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, Kind::Dir { .. })
    }
    pub fn is_expanded(&self) -> bool {
        matches!(self.kind, Kind::Dir { expanded: true, .. })
    }
}

pub struct FileTree {
    root: PathBuf,
    nodes: Vec<Node>,
    top: Vec<NodeId>,
    /// Cached visible rows; rebuilt after expand/collapse.
    visible: Vec<NodeId>,
    /// Deleted files (relative) that still get a row while they're missing on disk.
    ghosts: BTreeSet<PathBuf>,
}

impl FileTree {
    /// Loads only the top level.
    pub fn new(root: &Path) -> Self {
        Self::with_ghosts(root, BTreeSet::new())
    }

    fn with_ghosts(root: &Path, ghosts: BTreeSet<PathBuf>) -> Self {
        let mut t = FileTree { root: root.to_path_buf(), nodes: Vec::new(), top: Vec::new(), visible: Vec::new(), ghosts };
        t.top = t.load_dir(Path::new(""), 0, None);
        t.rebuild();
        t
    }

    /// Deleted files (relative to the root) to keep showing as rows, with ghost folders
    /// for any of their folders that are gone too. A ghost that exists on disk again is
    /// an ordinary row. Returns false (and does nothing) when the set is unchanged;
    /// otherwise the tree reloads, keeping what was expanded.
    pub fn set_ghosts(&mut self, rels: impl IntoIterator<Item = PathBuf>) -> bool {
        let ghosts: BTreeSet<PathBuf> = rels.into_iter().filter(|r| !r.as_os_str().is_empty()).collect();
        if ghosts == self.ghosts {
            return false;
        }
        self.ghosts = ghosts;
        self.refresh(Path::new(""));
        true
    }

    pub fn ghosts(&self) -> &BTreeSet<PathBuf> {
        &self.ghosts
    }

    /// `rel` has a row that is on disk (not a ghost).
    pub fn on_disk(&self, rel: &Path) -> bool {
        self.find(rel).is_some_and(|id| !self.nodes[id].ghost)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn abs_path(&self, id: NodeId) -> PathBuf {
        self.root.join(&self.nodes[id].rel)
    }

    /// Visible rows in display order.
    pub fn visible(&self) -> &[NodeId] {
        &self.visible
    }

    pub fn toggle(&mut self, id: NodeId) {
        let expanded = self.nodes[id].is_expanded();
        self.set_expanded(id, !expanded);
    }

    pub fn set_expanded(&mut self, id: NodeId, on: bool) {
        let needs_load = matches!(self.nodes[id].kind, Kind::Dir { children: None, .. });
        if on && needs_load {
            let rel = self.nodes[id].rel.clone();
            let depth = self.nodes[id].depth + 1;
            let kids = self.load_dir(&rel, depth, Some(id));
            if let Kind::Dir { children, .. } = &mut self.nodes[id].kind {
                *children = Some(kids);
            }
        }
        if let Kind::Dir { expanded, .. } = &mut self.nodes[id].kind {
            *expanded = on;
        }
        self.rebuild();
    }

    pub fn collapse_all(&mut self) {
        for n in &mut self.nodes {
            if let Kind::Dir { expanded, .. } = &mut n.kind {
                *expanded = false;
            }
        }
        self.rebuild();
    }

    /// Expand the folders leading to `rel` (relative to the root) and return
    /// its row, if the path exists in the tree (not ignored).
    pub fn reveal(&mut self, rel: &Path) -> Option<usize> {
        let mut level = self.top.clone();
        let mut found = None;
        let comps: Vec<_> = rel.components().collect();
        for (i, c) in comps.iter().enumerate() {
            let name = c.as_os_str().to_string_lossy();
            let id = *level.iter().find(|&&id| self.nodes[id].name == name)?;
            found = Some(id);
            if i + 1 < comps.len() {
                self.set_expanded(id, true);
                level = match &self.nodes[id].kind {
                    Kind::Dir { children: Some(c), .. } => c.clone(),
                    _ => return None,
                };
            }
        }
        let id = found?;
        self.visible.iter().position(|&v| v == id)
    }

    /// Reload a folder's children (after file-system changes), keeping the
    /// expansion state of surviving subfolders.
    pub fn refresh(&mut self, rel_dir: &Path) {
        if rel_dir.as_os_str().is_empty() {
            let fresh = FileTree::with_ghosts(&self.root, std::mem::take(&mut self.ghosts));
            let expanded = self.expanded_paths();
            *self = fresh;
            for p in expanded {
                self.reveal(&p);
                if let Some(id) = self.find(&p) {
                    self.set_expanded(id, true);
                }
            }
            return;
        }
        // Simple and correct: rebuild the whole tree, restoring expansion.
        self.refresh(Path::new(""));
    }

    fn expanded_paths(&self) -> Vec<PathBuf> {
        self.nodes.iter().filter(|n| n.is_expanded()).map(|n| n.rel.clone()).collect()
    }

    pub fn find(&self, rel: &Path) -> Option<NodeId> {
        self.nodes.iter().position(|n| n.rel == rel)
    }

    fn load_dir(&mut self, rel: &Path, depth: usize, parent: Option<NodeId>) -> Vec<NodeId> {
        let dir = self.root.join(rel);
        let mut entries: Vec<(String, bool, bool)> = WalkBuilder::new(&dir)
            .max_depth(Some(1))
            .hidden(false)
            .parents(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false)
            .filter_entry(|e| e.file_name() != ".git")
            .build()
            .filter_map(Result::ok)
            .filter(|e| e.depth() == 1)
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().is_some_and(|t| t.is_dir()), false))
            .collect();
        // Ghosts: the next path component of every deleted file under `rel` that isn't
        // on disk (a folder when more components follow).
        for g in self.ghosts.range(rel.to_path_buf()..) {
            let Ok(rest) = g.strip_prefix(rel) else { break };
            let mut comps = rest.components();
            let Some(first) = comps.next() else { continue };
            let name = first.as_os_str().to_string_lossy().into_owned();
            let is_dir = comps.next().is_some();
            if !entries.iter().any(|e| e.0 == name) && !dir.join(&name).exists() {
                entries.push((name, is_dir, true));
            }
        }
        // Folders first, then case-insensitive name (VS Code / Zed order).
        entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        let mut ids = Vec::with_capacity(entries.len());
        for (name, is_dir, ghost) in entries {
            let id = self.nodes.len();
            self.nodes.push(Node {
                rel: rel.join(&name),
                name,
                depth,
                parent,
                kind: if is_dir { Kind::Dir { expanded: false, children: None } } else { Kind::File },
                ghost,
            });
            ids.push(id);
        }
        ids
    }

    fn rebuild(&mut self) {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.top.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            out.push(id);
            if let Kind::Dir { expanded: true, children: Some(c) } = &self.nodes[id].kind {
                stack.extend(c.iter().rev().copied());
            }
        }
        self.visible = out;
    }
}

/// All files under `root` (relative paths), gitignore-aware, for Quick Open.
/// Parallel walk; call off the UI thread. Stops after `limit` files.
pub fn list_files(root: &Path, limit: usize) -> Vec<PathBuf> {
    use std::sync::{Arc, Mutex};
    let out = Arc::new(Mutex::new(Vec::new()));
    let root_buf = root.to_path_buf();
    WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build_parallel()
        .run(|| {
            let out = out.clone();
            let root = root_buf.clone();
            Box::new(move |e| {
                use ignore::WalkState;
                let Ok(e) = e else { return WalkState::Continue };
                if e.file_type().is_some_and(|t| t.is_file()) {
                    let mut v = out.lock().unwrap();
                    if v.len() >= limit {
                        return WalkState::Quit;
                    }
                    if let Ok(rel) = e.path().strip_prefix(&root) {
                        v.push(rel.to_path_buf());
                    }
                }
                WalkState::Continue
            })
        });
    let mut v = Arc::try_unwrap(out).map(|m| m.into_inner().unwrap()).unwrap_or_default();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Its own folder per test: tests run in parallel, and a shared one was
    /// deleted under another test (quick_open_list failed in CI).
    fn fixture(name: &str) -> PathBuf {
        let d = tmp(name);
        for p in ["src/a.rs", "src/deep/b.rs", "target/debug/x", "README.md", ".github/ci.yml", "Zeta/z.txt", ".git/HEAD"] {
            let f = d.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, "x").unwrap();
        }
        std::fs::write(d.join(".gitignore"), "target/\n").unwrap();
        d
    }

    fn names(t: &FileTree) -> Vec<String> {
        t.visible().iter().map(|&i| format!("{}{}", "  ".repeat(t.node(i).depth), t.node(i).name)).collect()
    }

    #[test]
    fn lazy_sorted_ignored() {
        let d = fixture("lazy_sorted_ignored");
        let mut t = FileTree::new(&d);
        assert_eq!(names(&t), [".github", "src", "Zeta", ".gitignore", "README.md"]);
        let src = t.find(Path::new("src")).unwrap();
        t.toggle(src);
        assert_eq!(names(&t), [".github", "src", "  deep", "  a.rs", "Zeta", ".gitignore", "README.md"]);
        t.toggle(src);
        assert_eq!(t.visible().len(), 5);
    }

    #[test]
    fn reveal_expands_ancestors() {
        let d = fixture("reveal_expands_ancestors");
        let mut t = FileTree::new(&d);
        let row = t.reveal(Path::new("src/deep/b.rs")).unwrap();
        assert_eq!(t.node(t.visible()[row]).name, "b.rs");
        assert!(t.reveal(Path::new("target/debug/x")).is_none());
        t.refresh(Path::new(""));
        assert!(names(&t).iter().any(|n| n.trim() == "b.rs"), "expansion survives refresh");
    }

    #[test]
    fn quick_open_list() {
        let d = fixture("quick_open_list");
        let files = list_files(&d, 1000);
        let s: Vec<_> = files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert!(s.contains(&"src/deep/b.rs".to_string()));
        assert!(s.contains(&".github/ci.yml".to_string()));
        assert!(!s.iter().any(|p| p.starts_with("target") || p.starts_with(".git/")));
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qur-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(d: &Path, rels: &[&str]) {
        for p in rels {
            let f = d.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, "x").unwrap();
        }
    }

    fn expand_all(t: &mut FileTree) {
        loop {
            let closed: Vec<NodeId> = t.visible().iter().copied().filter(|&i| t.node(i).is_dir() && !t.node(i).is_expanded()).collect();
            if closed.is_empty() {
                break;
            }
            for id in closed {
                t.set_expanded(id, true);
            }
        }
    }

    #[test]
    fn nested_ignore_files_and_git_exclude() {
        let d = tmp("nested");
        put(&d, &["web/dist/app.js", "web/src/app.ts", "web/src/api.gen.ts", "web/keep.log", "web/x.log", "docs/draft.md", "docs/final.md", "a.local", ".git/HEAD"]);
        std::fs::write(d.join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(d.join("web/.gitignore"), "dist/\n!keep.log\n").unwrap();
        std::fs::write(d.join("web/src/.gitignore"), "*.gen.ts\n").unwrap();
        std::fs::write(d.join("docs/.ignore"), "draft.md\n").unwrap();
        std::fs::create_dir_all(d.join(".git/info")).unwrap();
        std::fs::write(d.join(".git/info/exclude"), "*.local\n").unwrap();
        let mut t = FileTree::new(&d);
        expand_all(&mut t);
        let shown: Vec<String> = names(&t).into_iter().map(|n| n.trim().to_string()).collect();
        for want in ["web", "src", "app.ts", "keep.log", "final.md", ".gitignore"] {
            assert!(shown.iter().any(|n| n == want), "{want} missing: {shown:?}");
        }
        for hidden in ["dist", "api.gen.ts", "x.log", "draft.md", "a.local", ".git"] {
            assert!(!shown.iter().any(|n| n == hidden), "{hidden} shown: {shown:?}");
        }
        let files: Vec<String> = list_files(&d, 100).iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert!(files.contains(&"web/src/app.ts".into()) && !files.contains(&"web/src/api.gen.ts".into()), "{files:?}");
    }

    #[test]
    fn refresh_picks_up_new_and_removed_files_keeping_expansion() {
        let d = tmp("refresh");
        put(&d, &["src/a.rs", "src/b.rs", "lib/c.rs"]);
        let mut t = FileTree::new(&d);
        t.toggle(t.find(Path::new("src")).unwrap());
        put(&d, &["src/new.rs", "top.txt"]);
        std::fs::remove_file(d.join("src/a.rs")).unwrap();
        std::fs::remove_dir_all(d.join("lib")).unwrap();
        t.refresh(Path::new("src"));
        assert_eq!(names(&t), ["src", "  b.rs", "  new.rs", "top.txt"]);
        t.collapse_all();
        assert_eq!(names(&t), ["src", "top.txt"]);
        assert!(t.reveal(Path::new("src/nope.rs")).is_none());
        assert_eq!(list_files(&d, 2).len(), 2, "the limit stops the walk");
    }

    #[test]
    fn ghost_rows_for_deleted_files() {
        let d = tmp("ghosts");
        put(&d, &["src/a.rs", "src/b.rs", "README.md"]);
        let mut t = FileTree::new(&d);
        t.toggle(t.find(Path::new("src")).unwrap());
        // The agent deleted src/a.rs, a whole folder (old/x/y.rs) and a top-level file.
        std::fs::remove_file(d.join("src/a.rs")).unwrap();
        let ghosts = [PathBuf::from("src/a.rs"), PathBuf::from("old/x/y.rs"), PathBuf::from("gone.txt")];
        assert!(t.set_ghosts(ghosts.clone()));
        assert!(!t.set_ghosts(ghosts.clone()), "unchanged set: no reload");
        assert_eq!(names(&t), ["old", "src", "  a.rs", "  b.rs", "gone.txt", "README.md"]);
        let ghost = |t: &FileTree, rel: &str| t.node(t.find(Path::new(rel)).unwrap()).ghost;
        assert!(ghost(&t, "src/a.rs") && ghost(&t, "old") && ghost(&t, "gone.txt"));
        assert!(!ghost(&t, "src") && !ghost(&t, "src/b.rs"));
        assert!(!t.on_disk(Path::new("src/a.rs")) && t.on_disk(Path::new("src/b.rs")));
        // Ghost folders expand to their deleted contents; reveal finds them.
        let row = t.reveal(Path::new("old/x/y.rs")).unwrap();
        assert_eq!(t.node(t.visible()[row]).name, "y.rs");
        assert!(t.node(t.visible()[row]).ghost && t.node(t.find(Path::new("old/x")).unwrap()).is_dir());
        // Ghosts and their expansion survive a refresh.
        t.refresh(Path::new(""));
        assert!(names(&t).iter().any(|n| n.trim() == "y.rs"));
        // Re-created: an ordinary row again, even while the heatmap still lists it.
        put(&d, &["src/a.rs"]);
        t.refresh(Path::new(""));
        assert!(!ghost(&t, "src/a.rs"));
        assert_eq!(names(&t).iter().filter(|n| n.trim() == "a.rs").count(), 1);
        // Merged or discarded: the ghosts go.
        assert!(t.set_ghosts(Vec::new()));
        assert_eq!(names(&t), ["src", "  a.rs", "  b.rs", "README.md"]);
    }

    #[test]
    fn big_repo_is_fast() {
        let big = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/fixtures/big-repo");
        if !big.exists() {
            return;
        }
        let t0 = std::time::Instant::now();
        let t = FileTree::new(&big);
        let open = t0.elapsed();
        let t1 = std::time::Instant::now();
        let n = list_files(&big, 100_000).len();
        eprintln!("big-repo: top level {:?} ({} rows), list_files {:?} ({n} files)", open, t.visible().len(), t1.elapsed());
        assert_eq!(n, 5000);
    }
}
