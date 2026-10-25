//! Recursive file watching for one worktree (FSEvents via `notify`), debounced per path.
//!
//! One `Watcher` = one FSEvents stream + one debounce thread. Batches of absolute paths
//! (under the root as it was passed in, even if it is a symlink) arrive on an unbounded
//! `futures` channel, so the UI can await them on its own executor. Dropping the
//! `Watcher` stops the stream and the thread; the channel then ends.
//!
//! Filtering: everything under a `.git/` folder is dropped except the root's `.git/HEAD`
//! and `.git/index`; Qur's own save temp files are dropped; paths ignored the way the
//! Files tree ignores them (see [`Ignores`]) are dropped. If FSEvents asks for a rescan
//! (dropped events), the root itself is emitted: treat it as "rescan everything".

use std::collections::HashMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{EventKind, RecursiveMode, Watcher as _};

pub type Changes = UnboundedReceiver<Vec<PathBuf>>;

enum Msg {
    Event(notify::Event),
    Stop,
}

pub struct Watcher {
    inner: Option<notify::RecommendedWatcher>,
    tx: mpsc::Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

impl Watcher {
    /// Recursive watch of `root`. Emits batches of absolute paths that changed
    /// (created/modified/removed/renamed), debounced per path: a path is emitted once it
    /// has been quiet for `debounce`.
    pub fn new(root: &Path, debounce: Duration) -> io::Result<(Watcher, Changes)> {
        let canon = root.canonicalize()?;
        if !canon.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotADirectory, "watch root is not a directory"));
        }
        let (tx, rx) = mpsc::channel::<Msg>();
        let (out_tx, out_rx) = unbounded();

        let ev_tx = tx.clone();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let _ = ev_tx.send(Msg::Event(ev));
            }
        })
        .map_err(to_io)?;
        inner.watch(&canon, RecursiveMode::Recursive).map_err(to_io)?;

        let mut state = Debouncer {
            given: root.to_path_buf(),
            filter: Filter::new(&canon),
            canon,
            debounce,
            pending: HashMap::new(),
            out: out_tx,
        };
        let thread = std::thread::Builder::new()
            .name("qur-watch".into())
            .spawn(move || state.run(rx))?;

        Ok((Watcher { inner: Some(inner), tx, thread: Some(thread) }, out_rx))
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        drop(self.inner.take()); // stops the FSEvents stream first
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn to_io(e: notify::Error) -> io::Error {
    match e.kind {
        notify::ErrorKind::Io(e) => e,
        other => io::Error::other(format!("{other:?}")),
    }
}

/// Qur's own temporary files: the editor's atomic save (`.<name>.qur-<pid>.tmp`) and
/// Replace All (`<stem>.qur-replace.tmp`). They live for a few milliseconds and are
/// never anyone's edit.
pub fn is_qur_temp(name: &str) -> bool {
    name.ends_with(".qur-replace.tmp")
        || (name.starts_with('.')
            && name.strip_suffix(".tmp").and_then(|s| s.rsplit_once(".qur-")).is_some_and(|(_, pid)| {
                !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())
            }))
}

/// Ignore rules for one watched root, matching what the Files tree hides (`qur_tree`
/// walks with `.ignore`, `.gitignore`, `.git/info/exclude` and the global excludes file):
/// per-folder `.ignore` and `.gitignore` files at every depth, deeper folders first, then
/// `info/exclude`, then the global file. A path inside an ignored folder is ignored.
///
/// Matchers are cached per folder and loaded on first use (two `stat`s per folder, once),
/// so filtering an event never walks the file system. A change to a folder's `.gitignore`
/// or `.ignore` drops that folder's entry; a change to `info/exclude` reloads it.
pub struct Ignores {
    root: PathBuf,
    /// Folder (relative; `""` = root) -> its `.ignore` and `.gitignore` matchers, in
    /// precedence order. Empty when the folder has neither.
    dirs: HashMap<PathBuf, Vec<Gitignore>>,
    exclude: Gitignore,
    global: Gitignore,
}

impl Ignores {
    /// `root` should be canonical (FSEvents reports canonical paths).
    pub fn new(root: &Path) -> Self {
        let global = {
            let (g, _) = GitignoreBuilder::new(root).build_global();
            g
        };
        Ignores { root: root.to_path_buf(), dirs: HashMap::new(), exclude: load_exclude(root), global }
    }

    /// Forget cached rules after `rel` (relative to the root) changed. Returns true when
    /// `rel` is an ignore file, i.e. what is hidden may have changed.
    pub fn invalidate(&mut self, rel: &Path) -> bool {
        let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if rel == Path::new(".git/info/exclude") {
            self.exclude = load_exclude(&self.root);
            return true;
        }
        if name == ".gitignore" || name == ".ignore" {
            let dir = rel.parent().unwrap_or(Path::new(""));
            self.dirs.remove(dir);
            return true;
        }
        false
    }

    /// Is `rel` (relative to the root) ignored? `is_dir` describes `rel` itself; every
    /// ancestor is checked as a folder first.
    pub fn is_ignored(&mut self, rel: &Path, is_dir: bool) -> bool {
        let comps: Vec<Component> = rel.components().collect();
        let mut prefix = PathBuf::new();
        for (i, c) in comps.iter().enumerate() {
            prefix.push(c);
            let last = i + 1 == comps.len();
            if self.matched(&prefix, if last { is_dir } else { true }) {
                return true;
            }
        }
        false
    }

    /// The decision for `rel` alone (not its ancestors): the deepest folder with a rule
    /// that matches wins (ignore or `!re-include`), then info/exclude, then global.
    fn matched(&mut self, rel: &Path, is_dir: bool) -> bool {
        let abs = self.root.join(rel);
        let mut dir = rel.parent();
        while let Some(d) = dir {
            if !self.dirs.contains_key(d) {
                let m = load_dir(&self.root, d);
                self.dirs.insert(d.to_path_buf(), m);
            }
            for m in &self.dirs[d] {
                match m.matched(&abs, is_dir) {
                    Match::Ignore(_) => return true,
                    Match::Whitelist(_) => return false,
                    Match::None => {}
                }
            }
            dir = d.parent();
        }
        for m in [&self.exclude, &self.global] {
            match m.matched(&abs, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
    }

    /// Folders with cached rules (for tests and diagnostics).
    pub fn cached(&self) -> usize {
        self.dirs.len()
    }
}

fn load_dir(root: &Path, rel_dir: &Path) -> Vec<Gitignore> {
    let dir = root.join(rel_dir);
    let mut out = Vec::new();
    for name in [".ignore", ".gitignore"] {
        let f = dir.join(name);
        if f.is_file() {
            let mut b = GitignoreBuilder::new(&dir);
            let _ = b.add(f);
            if let Ok(g) = b.build()
                && !g.is_empty()
            {
                out.push(g);
            }
        }
    }
    out
}

fn load_exclude(root: &Path) -> Gitignore {
    let f = root.join(".git/info/exclude");
    if !f.is_file() {
        return Gitignore::empty();
    }
    let mut b = GitignoreBuilder::new(root);
    let _ = b.add(f);
    b.build().unwrap_or_else(|_| Gitignore::empty())
}

/// What the debouncer keeps from a raw event path.
struct Filter {
    canon: PathBuf,
    ignores: Ignores,
}

impl Filter {
    fn new(canon: &Path) -> Self {
        Filter { canon: canon.to_path_buf(), ignores: Ignores::new(canon) }
    }

    /// Filters one canonical event path; returns it if it should be reported.
    fn accept(&mut self, p: PathBuf) -> Option<PathBuf> {
        let rel = p.strip_prefix(&self.canon).ok()?;
        let mut comps = rel.components();
        match comps.next() {
            None => return None,
            Some(Component::Normal(first)) if first == ".git" => {
                let rest = comps.as_path();
                if rest == Path::new("info/exclude") {
                    self.ignores.invalidate(rel);
                    return None;
                }
                return (rest == Path::new("HEAD") || rest == Path::new("index")).then_some(p);
            }
            _ => {}
        }
        // Nested repositories' (and submodules') git folders.
        if rel.components().any(|c| c.as_os_str() == ".git") {
            return None;
        }
        if rel.file_name().and_then(|n| n.to_str()).is_some_and(is_qur_temp) {
            return None;
        }
        let rel = rel.to_path_buf();
        // An ignore file itself is reported (the tree may need to show or hide paths).
        self.ignores.invalidate(&rel);
        if self.ignores.is_ignored(&rel, p.is_dir()) {
            return None;
        }
        Some(p)
    }
}

struct Debouncer {
    given: PathBuf,
    canon: PathBuf,
    debounce: Duration,
    filter: Filter,
    /// canonical path -> last time it was seen
    pending: HashMap<PathBuf, Instant>,
    out: UnboundedSender<Vec<PathBuf>>,
}

impl Debouncer {
    fn run(&mut self, rx: mpsc::Receiver<Msg>) {
        loop {
            let msg = match self.pending.values().min() {
                Some(&oldest) => {
                    let wait = (oldest + self.debounce).saturating_duration_since(Instant::now());
                    match rx.recv_timeout(wait) {
                        Ok(m) => Some(m),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                },
            };
            match msg {
                Some(Msg::Stop) => return,
                Some(Msg::Event(ev)) => self.on_event(ev),
                None => {}
            }
            if !self.flush(Instant::now()) {
                return; // receiver dropped
            }
        }
    }

    fn on_event(&mut self, ev: notify::Event) {
        let now = Instant::now();
        if ev.need_rescan() {
            self.pending.insert(self.canon.clone(), now);
        }
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        for p in ev.paths {
            if let Some(p) = self.filter.accept(p) {
                self.pending.insert(p, now);
            }
        }
    }

    /// Emits every path quiet for `debounce`. Returns false when nobody listens anymore.
    fn flush(&mut self, now: Instant) -> bool {
        let deb = self.debounce;
        let mut due: Vec<PathBuf> = Vec::new();
        self.pending.retain(|p, t| {
            if now.duration_since(*t) >= deb {
                due.push(p.clone());
                false
            } else {
                true
            }
        });
        if due.is_empty() {
            return !self.out.is_closed();
        }
        due.sort();
        let batch = due
            .into_iter()
            .map(|p| match p.strip_prefix(&self.canon) {
                Ok(rel) if rel.as_os_str().is_empty() => self.given.clone(),
                Ok(rel) => self.given.join(rel),
                Err(_) => p,
            })
            .collect();
        self.out.unbounded_send(batch).is_ok()
    }
}

#[cfg(test)]
mod tests;
