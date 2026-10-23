//! Recursive file watching for one worktree (FSEvents via `notify`), debounced per path.
//!
//! One `Watcher` = one FSEvents stream + one debounce thread. Batches of absolute paths
//! (under the root as it was passed in, even if it is a symlink) arrive on an unbounded
//! `futures` channel, so the UI can await them on its own executor. Dropping the
//! `Watcher` stops the stream and the thread; the channel then ends.
//!
//! Filtering: everything under `.git/` is dropped except `.git/HEAD` and `.git/index`;
//! paths matched by the root `.gitignore` (and `.git/info/exclude`) are dropped. The
//! matcher is reloaded when `.gitignore` changes. If FSEvents asks for a rescan
//! (dropped events), the root itself is emitted: treat it as "rescan everything".

use std::collections::HashMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
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
            ignore: load_gitignore(&canon),
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

fn load_gitignore(root: &Path) -> Gitignore {
    let mut b = GitignoreBuilder::new(root);
    for f in [root.join(".gitignore"), root.join(".git/info/exclude")] {
        if f.is_file() {
            let _ = b.add(f);
        }
    }
    b.build().unwrap_or_else(|_| Gitignore::empty())
}

struct Debouncer {
    given: PathBuf,
    canon: PathBuf,
    debounce: Duration,
    ignore: Gitignore,
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
            if let Some(p) = self.accept(p) {
                self.pending.insert(p, now);
            }
        }
    }

    /// Filters one canonical event path; returns it if it should be reported.
    fn accept(&mut self, p: PathBuf) -> Option<PathBuf> {
        let rel = p.strip_prefix(&self.canon).ok()?;
        let mut comps = rel.components();
        match comps.next() {
            None => return None,
            Some(Component::Normal(first)) if first == ".git" => {
                let rest = comps.as_path();
                return (rest == Path::new("HEAD") || rest == Path::new("index")).then_some(p);
            }
            Some(Component::Normal(first)) if first == ".gitignore" && comps.next().is_none() => {
                self.ignore = load_gitignore(&self.canon);
                return Some(p);
            }
            _ => {}
        }
        if self.ignore.matched_path_or_any_parents(rel, p.is_dir()).is_ignore() {
            return None;
        }
        Some(p)
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
