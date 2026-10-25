//! Optional client file system (`fs/read_text_file`, `fs/write_text_file`).
//!
//! Off unless the app calls `Client::serve_fs` before `initialize` (the
//! agent's `client_fs = true` in agents.toml). Paths must be absolute and
//! inside the session's cwd, symlinks included; everything else is refused.
//! Requests run in order on one worker thread, so a slow `ClientFs` (the app
//! asking its UI thread for an unsaved buffer) never stalls the reader.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// Where agent reads and writes go. The defaults are plain disk IO; the app
/// overrides `read` to serve unsaved editor buffers and `write` to refresh
/// open editors afterwards.
pub trait ClientFs: Send + Sync + 'static {
    fn read(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
    fn write(&self, path: &Path, content: &str) -> std::io::Result<()> {
        write_atomic(path, content)
    }
}

/// Plain disk IO.
pub struct DiskFs;
impl ClientFs for DiskFs {}

/// Write via a temp file in the same folder and a rename, creating parent
/// folders. An existing file keeps its permissions.
pub fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no parent folder"))?;
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().ok_or_else(|| std::io::Error::other("no file name"))?.to_string_lossy();
    let tmp = dir.join(format!(".{name}.qur-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let res = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        if let Ok(meta) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

pub(crate) type Roots = Arc<Mutex<HashMap<String, PathBuf>>>;

pub(crate) struct Job {
    pub id: Value,
    pub method: String,
    pub params: Value,
}

/// JSON-RPC error codes: invalid params (bad or outside path), resource not
/// found (ACP), internal error.
const INVALID: i64 = -32602;
const NOT_FOUND: i64 = -32002;
const INTERNAL: i64 = -32603;

/// Run one request: `Ok(result)` or `Err((code, message))`.
pub(crate) fn handle(fs: &dyn ClientFs, roots: &Roots, job: &Job) -> Result<Value, (i64, String)> {
    let p = &job.params;
    let sid = p.get("sessionId").and_then(Value::as_str).unwrap_or("");
    let root = roots.lock().unwrap().get(sid).cloned().ok_or((INVALID, format!("unknown session {sid:?}")))?;
    let raw = p.get("path").and_then(Value::as_str).ok_or((INVALID, "missing path".to_string()))?;
    let path = resolve_inside(&root, Path::new(raw)).map_err(|e| (INVALID, e))?;
    let io = |e: std::io::Error| {
        let code = if e.kind() == std::io::ErrorKind::NotFound { NOT_FOUND } else { INTERNAL };
        (code, format!("{}: {e}", path.display()))
    };
    match job.method.as_str() {
        "fs/read_text_file" => {
            let text = fs.read(&path).map_err(io)?;
            let line = p.get("line").and_then(Value::as_u64).map(|n| n as usize);
            let limit = p.get("limit").and_then(Value::as_u64).map(|n| n as usize);
            Ok(json!({ "content": slice_lines(&text, line, limit) }))
        }
        "fs/write_text_file" => {
            let content = p.get("content").and_then(Value::as_str).ok_or((INVALID, "missing content".to_string()))?;
            fs.write(&path, content).map_err(io)?;
            Ok(Value::Null)
        }
        m => Err((-32601, format!("method not supported by Qur: {m}"))),
    }
}

/// Lines `line..line+limit` (1-based), keeping line endings.
pub fn slice_lines(text: &str, line: Option<usize>, limit: Option<usize>) -> String {
    if line.is_none() && limit.is_none() {
        return text.to_string();
    }
    let skip = line.unwrap_or(1).saturating_sub(1);
    text.split_inclusive('\n').skip(skip).take(limit.unwrap_or(usize::MAX)).collect()
}

/// `path` with `.` and `..` folded away, if it is absolute and (following
/// symlinks of whatever part exists) inside `root`.
pub fn resolve_inside(root: &Path, path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("path must be absolute: {}", path.display()));
    }
    let mut norm = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            c => norm.push(c),
        }
    }
    let outside = || format!("path is outside the task's folder: {}", path.display());
    let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    // The deepest part that exists decides where symlinks lead; the rest is
    // plain names (no `..` left).
    let mut existing = norm.as_path();
    let mut rest = Vec::new();
    while existing.symlink_metadata().is_err() {
        rest.push(existing.file_name().ok_or_else(outside)?);
        existing = existing.parent().ok_or_else(outside)?;
    }
    let mut real = existing.canonicalize().map_err(|e| format!("{}: {e}", existing.display()))?;
    real.extend(rest.iter().rev());
    if real.starts_with(&root) && real != root { Ok(norm) } else { Err(outside()) }
}
