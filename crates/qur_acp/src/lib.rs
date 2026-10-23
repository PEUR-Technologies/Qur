//! Qur's ACP v1 client: newline-delimited JSON-RPC 2.0 over an agent's stdio.
//!
//! Hand-rolled (M0 decision): serde_json, std threads and futures channels,
//! no async runtime. Each client owns three threads: a reader (routes
//! responses to their `Pending` and turns notifications/requests into
//! `Event`s), a stderr collector, and a waiter that reaps the process and
//! reports `Event::Exited` exactly once.
//!
//! The agent runs in its own process group; `shutdown` signals the whole
//! group, so nothing it spawned outlives it.

mod types;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use futures::channel::{mpsc, oneshot};
use serde_json::{Value, json};

pub use types::*;

pub const PROTOCOL_VERSION: u64 = 1;
/// Never handed to an agent: CLIs must use their own logins (handoff section 10).
const STRIPPED_ENV: [&str; 2] = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"];
const STDERR_TAIL: usize = 4096;

pub type Events = mpsc::UnboundedReceiver<Event>;
pub type Pending<T> = oneshot::Receiver<Result<T, AcpError>>;

type Responder = Box<dyn FnOnce(Result<Value, AcpError>) + Send>;

struct Inner {
    pid: i32,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<u64, Responder>>,
    next_id: AtomicU64,
    events: mpsc::UnboundedSender<Event>,
    /// Permission requests not answered yet, by session (answered `Cancelled` on cancel).
    open_permissions: Mutex<HashMap<String, Vec<RequestId>>>,
    answered: Mutex<HashSet<String>>,
    /// Set once the waiter has reaped the child; `exit_cv` wakes `shutdown`.
    exited: Mutex<bool>,
    exit_cv: Condvar,
    closed: AtomicBool,
    log: Option<Mutex<std::fs::File>>,
    start: Instant,
}

/// A running agent. Cheap to clone; all clones talk to the same process.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Inner {
    fn log(&self, dir: &str, msg: &Value) {
        if let Some(f) = &self.log {
            let line = json!({ "t_ms": self.start.elapsed().as_millis() as u64, "dir": dir, "msg": msg });
            let _ = writeln!(f.lock().unwrap(), "{line}");
        }
    }

    fn send(&self, msg: &Value) -> Result<(), AcpError> {
        self.log("out", msg);
        let mut line = serde_json::to_string(msg).map_err(|e| AcpError::new(e.to_string()))?;
        line.push('\n');
        let mut guard = self.stdin.lock().unwrap();
        let stdin = guard.as_mut().ok_or_else(|| AcpError::new("agent stdin is closed"))?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|e| AcpError::new(format!("write to agent failed: {e}")))
    }

    fn request(&self, method: &str, params: Value, respond: Responder) {
        if self.closed.load(Ordering::SeqCst) {
            return respond(Err(AcpError::new("agent exited")));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.pending.lock().unwrap().insert(id, respond);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(e) = self.send(&msg)
            && let Some(r) = self.pending.lock().unwrap().remove(&id)
        {
            r(Err(e));
        }
    }

    fn notify(&self, method: &str, params: Value) {
        let _ = self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn respond(&self, id: &Value, result: Value) {
        let _ = self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn respond_error(&self, id: &Value, code: i64, message: &str) {
        let _ = self.send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }));
    }

    fn emit(&self, ev: Event) {
        let _ = self.events.unbounded_send(ev);
    }

    /// Resolve every outstanding request with an error (agent gone).
    fn fail_all(&self, why: &str) {
        self.closed.store(true, Ordering::SeqCst);
        let pending: Vec<_> = self.pending.lock().unwrap().drain().collect();
        for (_, r) in pending {
            r(Err(AcpError::new(why)));
        }
    }

    fn handle_line(&self, line: &str) {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                self.log("in-nonjson", &Value::String(line.to_string()));
                return self.emit(Event::Warning(format!("non-JSON line from agent ({e}): {}", truncate(line, 200))));
            }
        };
        self.log("in", &msg);
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").filter(|v| !v.is_null());
        match (method, id) {
            // Response to one of our requests.
            (None, Some(id)) => {
                let Some(n) = id.as_u64() else {
                    return self.emit(Event::Warning(format!("response with unknown id {id}")));
                };
                let Some(r) = self.pending.lock().unwrap().remove(&n) else {
                    return self.emit(Event::Warning(format!("response for unknown request {n}")));
                };
                match msg.get("error") {
                    Some(e) => r(Err(AcpError::from_rpc(e))),
                    None => r(Ok(msg.get("result").cloned().unwrap_or(Value::Null))),
                }
            }
            // Request from the agent.
            (Some(method), Some(id)) => {
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                if method == "session/request_permission" {
                    let session_id = params.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
                    let rid = RequestId(id.clone());
                    self.open_permissions.lock().unwrap().entry(session_id.clone()).or_default().push(rid.clone());
                    self.emit(Event::Permission {
                        id: rid,
                        session_id,
                        tool_call: params.get("toolCall").map(ToolCallUpdate::parse).unwrap_or_default(),
                        options: params
                            .get("options")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(PermissionOption::parse).collect())
                            .unwrap_or_default(),
                    });
                } else {
                    self.respond_error(id, -32601, &format!("method not supported by Qur: {method}"));
                    if !method.starts_with('_') {
                        self.emit(Event::Warning(format!("agent called unsupported method {method}")));
                    }
                }
            }
            // Notification from the agent.
            (Some(method), None) => {
                if method == "session/update" {
                    let params = msg.get("params").cloned().unwrap_or(Value::Null);
                    let session_id = params.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
                    let update = params.get("update").map(SessionUpdate::parse).unwrap_or(SessionUpdate::Other {
                        kind: String::new(),
                        raw: Value::Null,
                    });
                    self.emit(Event::Update { session_id, update });
                } else if !method.starts_with('_') {
                    self.emit(Event::Warning(format!("unknown notification {method}")));
                }
            }
            (None, None) => self.emit(Event::Warning(format!("message without method or id: {}", truncate(line, 200)))),
        }
    }
}

fn truncate(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Is any process left in process group `pgid`?
fn group_alive(pgid: i32) -> bool {
    unsafe { libc::killpg(pgid, 0) == 0 }
}

fn typed<T: Send + 'static>(f: impl FnOnce(Value) -> Result<T, AcpError> + Send + 'static) -> (Pending<T>, Responder) {
    let (tx, rx) = oneshot::channel();
    let r: Responder = Box::new(move |res: Result<Value, AcpError>| {
        let _ = tx.send(res.and_then(f));
    });
    (rx, r)
}

impl Client {
    /// Spawn `cmd` in its own process group with `cwd`. API keys are always
    /// stripped from its environment.
    pub fn spawn(cmd: &AgentCommand, cwd: &Path) -> std::io::Result<(Client, Events)> {
        let mut c = Command::new(&cmd.program);
        c.args(&cmd.args).current_dir(cwd);
        for (k, v) in &cmd.env {
            if !STRIPPED_ENV.contains(&k.as_str()) {
                c.env(k, v);
            }
        }
        for k in STRIPPED_ENV {
            c.env_remove(k);
        }
        let mut child = c
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let pid = child.id() as i32;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let log = std::env::var_os("QUR_ACP_LOG").and_then(|dir| {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).ok()?;
            std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("{pid}.jsonl"))).ok().map(Mutex::new)
        });
        let (tx, rx) = mpsc::unbounded();
        let inner = Arc::new(Inner {
            pid,
            stdin: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            events: tx,
            open_permissions: Mutex::new(HashMap::new()),
            answered: Mutex::new(HashSet::new()),
            exited: Mutex::new(false),
            exit_cv: Condvar::new(),
            closed: AtomicBool::new(false),
            log,
            start: Instant::now(),
        });

        // Reader: stdout lines -> responses / events. Signals the waiter at EOF.
        let (eof_tx, eof_rx) = std_mpsc::channel::<()>();
        {
            let inner = inner.clone();
            thread::Builder::new().name(format!("acp-read-{pid}")).spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match reader.read_until(b'\n', &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let line = String::from_utf8_lossy(&buf);
                            let line = line.trim();
                            if !line.is_empty() {
                                inner.handle_line(line);
                            }
                        }
                    }
                }
                let _ = eof_tx.send(());
            })?;
        }

        // Stderr: keep the last few KB for error reports.
        let tail = Arc::new(Mutex::new(VecDeque::<u8>::with_capacity(STDERR_TAIL)));
        {
            let tail = tail.clone();
            thread::Builder::new().name(format!("acp-err-{pid}")).spawn(move || {
                let mut stderr = stderr;
                let mut chunk = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    let mut t = tail.lock().unwrap();
                    t.extend(&chunk[..n]);
                    while t.len() > STDERR_TAIL {
                        t.pop_front();
                    }
                }
            })?;
        }

        // Waiter: reap the child, let the reader drain, fail what's left, report once.
        {
            let inner = inner.clone();
            thread::Builder::new().name(format!("acp-wait-{pid}")).spawn(move || {
                let mut child: Child = child;
                let code = child.wait().ok().and_then(|s| s.code());
                // Responses already in the pipe still resolve their requests.
                let _ = eof_rx.recv_timeout(Duration::from_millis(500));
                thread::sleep(Duration::from_millis(20));
                inner.fail_all("agent exited");
                let stderr_tail = String::from_utf8_lossy(&tail.lock().unwrap().iter().copied().collect::<Vec<_>>()).into_owned();
                inner.stdin.lock().unwrap().take();
                inner.emit(Event::Exited { code, stderr_tail });
                *inner.exited.lock().unwrap() = true;
                inner.exit_cv.notify_all();
            })?;
        }

        Ok((Client { inner }, rx))
    }

    pub fn pid(&self) -> i32 {
        self.inner.pid
    }

    /// True once the agent process has exited.
    pub fn has_exited(&self) -> bool {
        *self.inner.exited.lock().unwrap()
    }

    pub fn initialize(&self) -> Pending<InitializeResult> {
        let (rx, r) = typed(|v| Ok(InitializeResult::parse(v)));
        self.inner.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                // No fs, no terminal: agents use their own tools in the worktree.
                // `terminal_output` (Zed's extension, which claude-agent-acp
                // reads) gets a command's output and exit code on its tool call.
                "clientCapabilities": { "_meta": { "terminal_output": true } },
                "clientInfo": { "name": "qur", "title": "Qur", "version": env!("CARGO_PKG_VERSION") }
            }),
            r,
        );
        rx
    }

    /// A new session: its id and the settings it offers (mode, model…).
    pub fn new_session(&self, cwd: &Path) -> Pending<(String, Vec<ConfigOption>)> {
        let (rx, r) = typed(|v| {
            let id = v
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| AcpError::new("session/new returned no sessionId"))?;
            Ok((id, ConfigOption::parse_session(&v)))
        });
        self.inner.request("session/new", json!({ "cwd": cwd, "mcpServers": [] }), r);
        rx
    }

    /// Resume a session; the agent replays its history as `Update` events.
    /// Resolves with the session's settings.
    pub fn load_session(&self, session_id: &str, cwd: &Path) -> Pending<Vec<ConfigOption>> {
        let (rx, r) = typed(|v| Ok(ConfigOption::parse_session(&v)));
        self.inner.request("session/load", json!({ "sessionId": session_id, "cwd": cwd, "mcpServers": [] }), r);
        rx
    }

    /// Change a session setting. Resolves with the agent's full new set of
    /// settings, or None for a mode set the old way (`session/set_mode`
    /// answers with nothing; the agent sends `current_mode_update`).
    pub fn set_option(&self, session_id: &str, option: &ConfigOption, value: &str) -> Pending<Option<Vec<ConfigOption>>> {
        if option.legacy_mode {
            let (rx, r) = typed(|_| Ok(None));
            self.inner.request("session/set_mode", json!({ "sessionId": session_id, "modeId": value }), r);
            return rx;
        }
        let (rx, r) = typed(|v| Ok(Some(ConfigOption::parse_list(&v))));
        self.inner.request("session/set_config_option", json!({ "sessionId": session_id, "configId": option.id, "value": value }), r);
        rx
    }

    pub fn prompt(&self, session_id: &str, blocks: Vec<PromptBlock>) -> Pending<StopReason> {
        let (rx, r) = typed(|v| Ok(StopReason::parse(&v)));
        let prompt: Vec<Value> = blocks.iter().map(PromptBlock::to_json).collect();
        self.inner.request("session/prompt", json!({ "sessionId": session_id, "prompt": prompt }), r);
        rx
    }

    /// Cancel the running turn. Open permission requests for the session are
    /// answered `Cancelled`, as the spec requires. The pending `prompt`
    /// still resolves with the agent's reply (normally `Cancelled`).
    pub fn cancel(&self, session_id: &str) {
        self.inner.notify("session/cancel", json!({ "sessionId": session_id }));
        let open = self.inner.open_permissions.lock().unwrap().remove(session_id).unwrap_or_default();
        for id in open {
            self.respond_permission(id, PermissionOutcome::Cancelled);
        }
    }

    pub fn respond_permission(&self, request: RequestId, outcome: PermissionOutcome) {
        let key = request.0.to_string();
        if !self.inner.answered.lock().unwrap().insert(key) {
            return; // already answered (e.g. by cancel)
        }
        for list in self.inner.open_permissions.lock().unwrap().values_mut() {
            list.retain(|r| *r != request);
        }
        self.inner.respond(&request.0, outcome.to_json());
    }

    /// Close stdin, SIGTERM the process group, SIGKILL after `grace`. Blocks
    /// until the agent is reaped and its group is empty (or 2 s past the
    /// SIGKILL). Idempotent.
    pub fn shutdown(&self, grace: Duration) {
        let pgid = self.inner.pid;
        self.inner.stdin.lock().unwrap().take();
        if group_alive(pgid) {
            unsafe { libc::killpg(pgid, libc::SIGTERM) };
        }
        let deadline = Instant::now() + grace;
        if self.wait_gone(deadline) {
            return;
        }
        unsafe { libc::killpg(pgid, libc::SIGKILL) };
        self.wait_gone(Instant::now() + Duration::from_secs(2));
    }

    /// Wait until the child is reaped and nothing is left in its group.
    fn wait_gone(&self, deadline: Instant) -> bool {
        let mut exited = self.inner.exited.lock().unwrap();
        loop {
            if *exited && !group_alive(self.inner.pid) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let wait = (deadline - now).min(Duration::from_millis(20));
            exited = self.inner.exit_cv.wait_timeout(exited, wait).unwrap().0;
        }
    }
}

#[cfg(test)]
mod tests;
