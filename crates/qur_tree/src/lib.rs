//! File tree model for the Files dropdown, plus the project file list for
//! Quick Open. Folders load lazily (on expand) so huge repos open instantly.
//! Respects .gitignore / .ignore / global git excludes; shows dotfiles except `.git`.
//! (Heatmap state joins this crate in M3.)

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
}

impl FileTree {
    /// Loads only the top level.
    pub fn new(root: &Path) -> Self {
        let mut t = FileTree { root: root.to_path_buf(), nodes: Vec::new(), top: Vec::new(), visible: Vec::new() };
        t.top = t.load_dir(Path::new(""), 0, None);
        t.rebuild();
        t
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
            let fresh = FileTree::new(&self.root);
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
        let mut entries: Vec<(String, bool)> = WalkBuilder::new(&dir)
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
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().is_some_and(|t| t.is_dir())))
            .collect();
        // Folders first, then case-insensitive name (VS Code / Zed order).
        entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        let mut ids = Vec::with_capacity(entries.len());
        for (name, is_dir) in entries {
            let id = self.nodes.len();
            self.nodes.push(Node {
                rel: rel.join(&name),
                name,
                depth,
                parent,
                kind: if is_dir { Kind::Dir { expanded: false, children: None } } else { Kind::File },
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

    fn fixture() -> PathBuf {
        let d = std::env::temp_dir().join(format!("qur-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
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
        let d = fixture();
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
        let d = fixture();
        let mut t = FileTree::new(&d);
        let row = t.reveal(Path::new("src/deep/b.rs")).unwrap();
        assert_eq!(t.node(t.visible()[row]).name, "b.rs");
        assert!(t.reveal(Path::new("target/debug/x")).is_none());
        t.refresh(Path::new(""));
        assert!(names(&t).iter().any(|n| n.trim() == "b.rs"), "expansion survives refresh");
    }

    #[test]
    fn quick_open_list() {
        let d = fixture();
        let files = list_files(&d, 1000);
        let s: Vec<_> = files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert!(s.contains(&"src/deep/b.rs".to_string()));
        assert!(s.contains(&".github/ci.yml".to_string()));
        assert!(!s.iter().any(|p| p.starts_with("target") || p.starts_with(".git/")));
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
