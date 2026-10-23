//! M0 ACP spike: a minimal hand-rolled ACP v1 client (JSON-RPC 2.0 over stdio).
//!
//! Spawns an adapter in its own process group with `cwd` = a worktree, runs
//! initialize -> session/new -> session/prompt, logs every message (both
//! directions, timestamped) to a JSONL file, auto-answers permission requests
//! with "allow once", then shuts the adapter down and checks for orphans.
//!
//! usage: acp_spike <adapter-cmd> <cwd> <out.jsonl> <prompt> [--cancel-after-first-tool-call]

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const PROTOCOL_VERSION: u64 = 1;

struct Log {
    file: std::fs::File,
    start: Instant,
}

impl Log {
    fn write(&mut self, dir: &str, msg: &Value) {
        let line = json!({ "t_ms": self.start.elapsed().as_millis() as u64, "dir": dir, "msg": msg });
        writeln!(self.file, "{line}").unwrap();
    }
}

enum Incoming {
    Response { id: u64, result: Result<Value, Value> },
    Request { id: Value, method: String, params: Value },
    Notification { method: String, params: Value },
    Eof,
}

struct Conn {
    stdin: std::process::ChildStdin,
    next_id: u64,
    log: Arc<Mutex<Log>>,
}

impl Conn {
    fn send(&mut self, msg: Value) {
        self.log.lock().unwrap().write("out", &msg);
        let mut s = serde_json::to_string(&msg).unwrap();
        s.push('\n');
        self.stdin.write_all(s.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn respond(&mut self, id: Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn respond_error(&mut self, id: Value, code: i64, message: &str) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }));
    }
}

fn spawn_reader(stdout: std::process::ChildStdout, log: Arc<Mutex<Log>>) -> mpsc::Receiver<Incoming> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                log.lock().unwrap().write("in-nonjson", &Value::String(line));
                continue;
            };
            log.lock().unwrap().write("in", &msg);
            let method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
            let inc = match (method, msg.get("id")) {
                (Some(method), Some(id)) => Incoming::Request {
                    id: id.clone(),
                    method,
                    params: msg.get("params").cloned().unwrap_or(Value::Null),
                },
                (Some(method), None) => Incoming::Notification {
                    method,
                    params: msg.get("params").cloned().unwrap_or(Value::Null),
                },
                (None, Some(id)) => Incoming::Response {
                    id: id.as_u64().unwrap_or(u64::MAX),
                    result: match msg.get("error") {
                        Some(e) => Err(e.clone()),
                        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                    },
                },
                (None, None) => continue,
            };
            if tx.send(inc).is_err() {
                break;
            }
        }
        let _ = tx.send(Incoming::Eof);
    });
    rx
}

struct Session {
    conn: Conn,
    rx: mpsc::Receiver<Incoming>,
    cancel_after_first_tool_call: bool,
    cancel_sent: bool,
    session_id: Option<String>,
    stats: std::collections::BTreeMap<String, usize>,
}

impl Session {
    /// Pump messages until the response to `want` arrives.
    fn wait_for(&mut self, want: u64, timeout: Duration) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(Incoming::Response { id, result }) if id == want => {
                    return result.map_err(|e| e.to_string());
                }
                Ok(Incoming::Response { .. }) => {}
                Ok(Incoming::Request { id, method, params }) => self.on_request(id, &method, &params),
                Ok(Incoming::Notification { method, params }) => self.on_notification(&method, &params),
                Ok(Incoming::Eof) => return Err("adapter closed stdout".into()),
                Err(_) => return Err(format!("timeout waiting for response {want}")),
            }
        }
    }

    fn on_request(&mut self, id: Value, method: &str, params: &Value) {
        *self.stats.entry(format!("request:{method}")).or_default() += 1;
        match method {
            "session/request_permission" => {
                // Spike: pick the first "allow_once" option, else the first option.
                let opts = params["options"].as_array().cloned().unwrap_or_default();
                let pick = opts
                    .iter()
                    .find(|o| o["kind"] == "allow_once")
                    .or_else(|| opts.first())
                    .and_then(|o| o["optionId"].as_str())
                    .unwrap_or("")
                    .to_owned();
                eprintln!("  permission: {} -> {pick}", params["toolCall"]["title"]);
                self.conn.respond(id, json!({ "outcome": { "outcome": "selected", "optionId": pick } }));
            }
            _ => self.conn.respond_error(id, -32601, "method not supported by qur spike"),
        }
    }

    fn on_notification(&mut self, method: &str, params: &Value) {
        if method != "session/update" {
            *self.stats.entry(format!("notification:{method}")).or_default() += 1;
            return;
        }
        let u = &params["update"];
        let kind = u["sessionUpdate"].as_str().unwrap_or("?");
        let key = match kind {
            "tool_call" | "tool_call_update" => format!(
                "update:{kind}:{}",
                u["kind"].as_str().unwrap_or(if kind == "tool_call" { "?" } else { "-" })
            ),
            _ => format!("update:{kind}"),
        };
        *self.stats.entry(key).or_default() += 1;
        if kind == "tool_call" {
            eprintln!(
                "  tool_call [{}] {} locations={}",
                u["kind"].as_str().unwrap_or("?"),
                u["title"],
                u["locations"]
            );
            if self.cancel_after_first_tool_call && !self.cancel_sent {
                self.cancel_sent = true;
                eprintln!("  -> session/cancel");
                let sid = self.session_id.clone().unwrap();
                self.conn.notify("session/cancel", json!({ "sessionId": sid }));
            }
        }
    }
}

/// Every pid whose process-group id is `pgid`.
fn group_members(pgid: i32) -> Vec<i32> {
    let out = Command::new("pgrep").arg("-g").arg(pgid.to_string()).output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect()
}

/// Descendants of `pid` (any process group), via `ps`.
fn descendants(pid: i32) -> Vec<(i32, String)> {
    let out = Command::new("ps").args(["-axo", "pid=,ppid=,command="]).output().unwrap();
    let rows: Vec<(i32, i32, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let p = it.next()?.parse().ok()?;
            let pp = it.next()?.parse().ok()?;
            Some((p, pp, it.collect::<Vec<_>>().join(" ")))
        })
        .collect();
    let mut found = vec![];
    let mut frontier = vec![pid];
    while let Some(parent) = frontier.pop() {
        for (p, pp, cmd) in &rows {
            if *pp == parent {
                found.push((*p, cmd.chars().take(80).collect()));
                frontier.push(*p);
            }
        }
    }
    found
}

/// Qur's shutdown sequence: close stdin, SIGTERM the group, SIGKILL after 2 s.
fn shutdown(child: &mut Child, conn: Conn) -> Duration {
    let t = Instant::now();
    let pgid = child.id() as i32;
    drop(conn); // closes stdin
    unsafe { libc::killpg(pgid, libc::SIGTERM) };
    while t.elapsed() < Duration::from_secs(2) {
        if child.try_wait().unwrap().is_some() && group_members(pgid).is_empty() {
            return t.elapsed();
        }
        thread::sleep(Duration::from_millis(20));
    }
    eprintln!("  SIGKILL after 2 s");
    unsafe { libc::killpg(pgid, libc::SIGKILL) };
    let _ = child.wait();
    t.elapsed()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: acp_spike <adapter-cmd> <cwd> <out.jsonl> <prompt> [--cancel-after-first-tool-call]");
        std::process::exit(2);
    }
    let (cmd, cwd, out, prompt) = (&args[1], &args[2], &args[3], &args[4]);
    let cancel_after_first_tool_call = args.iter().any(|a| a == "--cancel-after-first-tool-call");

    let log = Arc::new(Mutex::new(Log { file: std::fs::File::create(out).unwrap(), start: Instant::now() }));
    let stderr_file = std::fs::File::create(format!("{out}.stderr")).unwrap();

    // Hard constraint: never pass API keys to agents; they must use the CLI login.
    let mut child = Command::new(cmd)
        .current_dir(cwd)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr_file)
        .process_group(0) // own group, so the whole tree can be signalled
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {cmd}: {e}"));
    let t0 = Instant::now();

    let rx = spawn_reader(child.stdout.take().unwrap(), log.clone());
    let conn = Conn { stdin: child.stdin.take().unwrap(), next_id: 1, log };
    let mut s = Session {
        conn,
        rx,
        cancel_after_first_tool_call,
        cancel_sent: false,
        session_id: None,
        stats: Default::default(),
    };

    let id = s.conn.request(
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            // Minimal: no fs, no terminal. The agent uses its own tools in the worktree.
            "clientCapabilities": {},
            "clientInfo": { "name": "qur", "title": "Qur", "version": "0.0.0-spike" }
        }),
    );
    let init = s.wait_for(id, Duration::from_secs(30)).expect("initialize");
    eprintln!("initialize ok in {} ms: {}", t0.elapsed().as_millis(), init["agentInfo"]);

    let id = s.conn.request("session/new", json!({ "cwd": cwd, "mcpServers": [] }));
    let new = match s.wait_for(id, Duration::from_secs(60)) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("session/new failed: {e}");
            eprintln!("shutdown: {} ms", shutdown(&mut child, s.conn).as_millis());
            std::process::exit(1);
        }
    };
    let sid = new["sessionId"].as_str().expect("sessionId").to_owned();
    s.session_id = Some(sid.clone());
    eprintln!("session/new ok in {} ms: {sid}", t0.elapsed().as_millis());

    let t_prompt = Instant::now();
    let id = s.conn.request(
        "session/prompt",
        json!({ "sessionId": sid, "prompt": [{ "type": "text", "text": prompt }] }),
    );
    let res = s.wait_for(id, Duration::from_secs(600));
    eprintln!(
        "session/prompt -> {:?} in {} ms",
        res.as_ref().map(|r| r["stopReason"].clone()),
        t_prompt.elapsed().as_millis()
    );

    eprintln!("stats:");
    for (k, v) in &s.stats {
        eprintln!("  {k}: {v}");
    }

    let pgid = child.id() as i32;
    let tree_before = descendants(pgid);
    eprintln!("process tree before shutdown: {} descendants", tree_before.len());
    let took = shutdown(&mut child, s.conn);
    thread::sleep(Duration::from_millis(300));
    let left: Vec<_> = tree_before
        .iter()
        .filter(|(p, _)| unsafe { libc::kill(*p, 0) } == 0)
        .collect();
    let group_left = group_members(pgid);
    eprintln!("shutdown: {} ms; survivors of tree: {:?}; group members left: {:?}", took.as_millis(), left, group_left);
    if !left.is_empty() || !group_left.is_empty() {
        eprintln!("ORPHANS LEFT");
        std::process::exit(3);
    }
    if res.is_err() {
        std::process::exit(1);
    }
}
