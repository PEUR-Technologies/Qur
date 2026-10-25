//! Writes Qur itself made (an editor save, Replace All). The watcher reports them like
//! any other change; the heatmap must not count them as the agent's edits, or saving a
//! file from Qur's editor would glow as agent heat in a project-folder agent's tree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long after Qur writes a file its watcher events are still Qur's. FSEvents plus the
/// debounce deliver in well under a second; a later write is someone else's.
pub const OWN_WINDOW: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct OwnWrites {
    /// Absolute path (as given, and canonical) -> when Qur wrote it.
    at: HashMap<PathBuf, Instant>,
}

impl OwnWrites {
    /// Qur just wrote `abs` (call it right after the write, or just before one that runs
    /// on another thread).
    pub fn note(&mut self, abs: &Path, now: Instant) {
        self.at.retain(|_, t| now < *t + OWN_WINDOW);
        if let Ok(c) = abs.canonicalize() {
            self.at.insert(c, now);
        }
        self.at.insert(abs.to_path_buf(), now);
    }

    /// Was `abs` written by Qur within the last [`OWN_WINDOW`]?
    pub fn is_own(&self, abs: &Path, now: Instant) -> bool {
        let fresh = |p: &Path| self.at.get(p).is_some_and(|t| now < *t + OWN_WINDOW);
        if self.at.is_empty() {
            return false;
        }
        fresh(abs) || abs.canonicalize().is_ok_and(|c| fresh(&c))
    }

    /// Splits a watcher batch into (Qur's own writes, everything else).
    pub fn split(&self, batch: Vec<PathBuf>, now: Instant) -> (Vec<PathBuf>, Vec<PathBuf>) {
        if self.at.is_empty() {
            return (Vec::new(), batch);
        }
        batch.into_iter().partition(|p| self.is_own(p, now))
    }
}

/// The process-wide record (every window's editor and search share it).
pub fn own_writes() -> &'static Mutex<OwnWrites> {
    static OWN: OnceLock<Mutex<OwnWrites>> = OnceLock::new();
    OWN.get_or_init(Default::default)
}
