//! The git layer for M4: review an agent task's changes, merge its branch into
//! the main checkout, and the Git panel (status, stage, commit).
//!
//! Uses the git CLI only (no libgit2/gix) and everything blocks, so call it off
//! the UI thread. Every command runs with literal pathspecs, no prompts, no
//! editor, no pager, and `-z`/porcelain output, so odd file names work.
//! Read-only commands also set `GIT_OPTIONAL_LOCKS=0`.
//!
//! Hard rules (handoff section 10): never force-push, never rewrite history,
//! never touch the main checkout except on an explicit Merge or a user
//! edit/stage/commit. `root` is always the top level of a checkout.

mod conflict;
mod diff;
mod merge;
mod status;
mod turns;

#[cfg(test)]
mod tests;

pub use conflict::{Conflict, Resolution, find_conflicts, resolve};
pub use diff::{DiffLine, FileDiff, FileStatus, Hunk, LineKind, MAX_LINES_PER_FILE, diff_totals, file_diff, task_diff};
pub use merge::{
    MergeOutcome, abort_merge, commit_all, conflicted_files, finish_merge, is_merging, merge_clean, precheck,
    start_conflict_merge,
};
pub use status::{Entry, Status, commit, discard_unstaged, stage, status, unstage};
pub use turns::{TURNS_REF_PREFIX, changed_paths_between, delete_turn_refs, pin_turn, unpin_turn, restore_tree, snapshot_tree, tree_diff, tree_stats, turn_ref};

use std::ffi::OsStr;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Result of one git invocation.
pub(crate) struct Out {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Out {
    pub fn ok(self) -> Result<Vec<u8>, String> {
        if self.code == 0 { Ok(self.stdout) } else { Err(self.error()) }
    }

    /// A readable message for a failed command (stderr, else stdout).
    pub fn error(&self) -> String {
        let s = self.stderr.trim();
        if !s.is_empty() {
            return s.to_string();
        }
        let o = String::from_utf8_lossy(&self.stdout).trim().to_string();
        if o.is_empty() { format!("git exited with status {}", self.code) } else { o }
    }
}

/// Builder for one git command in `dir`.
pub(crate) struct Git {
    cmd: Command,
    stdin: Option<Vec<u8>>,
}

impl Git {
    /// A command that may write (index, refs, files).
    pub fn new(dir: &Path) -> Git {
        let mut cmd = Command::new("git");
        cmd.current_dir(dir)
            .env("GIT_LITERAL_PATHSPECS", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .env("GIT_PAGER", "cat")
            .env("GIT_ASKPASS", "true")
            .env("SSH_ASKPASS", "true")
            .env("LC_ALL", "C")
            // Inherited repo overrides would point git somewhere else entirely.
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_PREFIX")
            .args(["-c", "core.quotepath=false", "-c", "color.ui=false", "-c", "core.fsmonitor=false"]);
        Git { cmd, stdin: None }
    }

    /// A read-only command: never takes optional locks (no index refresh writes).
    pub fn ro(dir: &Path) -> Git {
        let mut g = Git::new(dir);
        g.cmd.env("GIT_OPTIONAL_LOCKS", "0");
        g
    }

    pub fn arg(mut self, a: impl AsRef<OsStr>) -> Git {
        self.cmd.arg(a);
        self
    }

    pub fn args<I, S>(mut self, a: I) -> Git
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.cmd.args(a);
        self
    }

    pub fn env(mut self, k: &str, v: impl AsRef<OsStr>) -> Git {
        self.cmd.env(k, v);
        self
    }

    pub fn stdin(mut self, data: Vec<u8>) -> Git {
        self.stdin = Some(data);
        self
    }

    pub fn run(mut self) -> Result<Out, String> {
        let out = if let Some(data) = self.stdin.take() {
            self.cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = self.cmd.spawn().map_err(|e| format!("git: {e}"))?;
            let mut sin = child.stdin.take().ok_or("git: no stdin")?;
            // Write on a thread so a large stdin can't deadlock against a full stdout pipe.
            let writer = std::thread::spawn(move || {
                let _ = sin.write_all(&data);
            });
            let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
            let _ = writer.join();
            out
        } else {
            self.cmd.stdin(Stdio::null()).output().map_err(|e| format!("git: {e}"))?
        };
        Ok(Out {
            code: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// Run and require success; returns stdout.
    pub fn ok(self) -> Result<Vec<u8>, String> {
        self.run()?.ok()
    }

    /// Run and require success; returns trimmed stdout as text.
    pub fn text(self) -> Result<String, String> {
        Ok(String::from_utf8_lossy(&self.ok()?).trim().to_string())
    }
}

pub(crate) fn path_of(b: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(b))
}

/// Split NUL-terminated output into fields (no trailing empty field).
pub(crate) fn nul_fields(b: &[u8]) -> Vec<&[u8]> {
    let mut v: Vec<&[u8]> = b.split(|&c| c == 0).collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

/// `p` relative to `root` (accepts absolute paths under `root`). Refuses
/// paths that escape the checkout.
pub(crate) fn rel(root: &Path, p: &Path) -> Result<PathBuf, String> {
    let r = if p.is_absolute() {
        p.strip_prefix(root)
            .map(Path::to_path_buf)
            .map_err(|_| format!("{} is outside {}", p.display(), root.display()))?
    } else {
        p.to_path_buf()
    };
    if r.as_os_str().is_empty() || r.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return Err(format!("invalid path in checkout: {}", p.display()));
    }
    Ok(r)
}

pub(crate) fn rels(root: &Path, ps: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    ps.iter().map(|p| rel(root, p)).collect()
}

/// A revision argument that can't be mistaken for an option.
pub(crate) fn check_rev(r: &str) -> Result<(), String> {
    if r.is_empty() || r.starts_with('-') || r.contains('\0') {
        Err(format!("invalid revision: {r:?}"))
    } else {
        Ok(())
    }
}

/// Full commit id of `rev`, or Err.
pub(crate) fn rev_parse(root: &Path, rev: &str) -> Result<String, String> {
    check_rev(rev)?;
    Git::ro(root).args(["rev-parse", "--verify", "--quiet", "--end-of-options"]).arg(format!("{rev}^{{commit}}")).text().and_then(
        |s| if s.is_empty() { Err(format!("unknown revision: {rev}")) } else { Ok(s) },
    )
}

/// `-c user.name=Qur -c user.email=qur@localhost` for whichever is unset.
pub(crate) fn ident_args(root: &Path) -> Vec<String> {
    let mut v = Vec::new();
    let set = |key: &str, env: &str| {
        std::env::var_os(env).is_some_and(|s| !s.is_empty())
            || Git::ro(root).args(["config", "--get", key]).text().is_ok_and(|s| !s.is_empty())
    };
    if !set("user.name", "GIT_AUTHOR_NAME") {
        v.extend(["-c".to_string(), "user.name=Qur".to_string()]);
    }
    if !set("user.email", "GIT_AUTHOR_EMAIL") {
        v.extend(["-c".to_string(), "user.email=qur@localhost".to_string()]);
    }
    v
}

/// A unique file name for temporary indexes.
pub(crate) fn temp_name(tag: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("qur-{tag}-{}-{n}-{t}", std::process::id())
}

/// Deletes a temporary index (and its lock) on drop.
pub(crate) struct TempFile(pub PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut lock = self.0.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(lock);
    }
}

pub(crate) fn join_paths(ps: &[PathBuf]) -> String {
    ps.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
}
