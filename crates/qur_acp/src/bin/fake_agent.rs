//! A scripted ACP v1 agent for qur_acp's protocol tests (not shipped).
//!
//! Prompt text picks the script:
//! - default: message, tool call + update, plan, permission request (reports
//!   the chosen option), an unsupported `fs/read_text_file` call, end_turn
//! - contains "slow": one chunk, then waits for session/cancel -> cancelled
//! - contains "crash": prints to stderr and exits 3
//! - contains "garbage": writes a non-JSON line first
//! - contains "picture": a session title, an image and a file link, max_tokens
//! - contains "parallel": three finished tool calls in a row (two reads and
//!   a command), then two edits whose permission requests are both sent
//!   before either is answered; reports both choices
//!
//! Env: FAKE_AGENT_CHILD=1 spawns a `sleep 300` child (same process group);
//! FAKE_AGENT_IGNORE_TERM=1 ignores SIGTERM (tests the SIGKILL path);
//! FAKE_AGENT_AUTH=1 refuses session/new and prompts with -32000 until
//! `authenticate` with method "token".

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn send(v: Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{v}").unwrap();
    out.flush().unwrap();
}

fn update(sid: &str, u: Value) {
    send(json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": sid, "update": u } }));
}

fn chunk(sid: &str, text: &str) {
    update(sid, json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } }));
}

/// Read stdin until a message satisfying `want` arrives; other messages are
/// handled by `other` (returns true to stop waiting, e.g. on cancel).
fn wait_for(lines: &mut impl Iterator<Item = String>, mut want: impl FnMut(&Value) -> bool) -> Option<Value> {
    for line in lines.by_ref() {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if want(&v) {
            return Some(v);
        }
    }
    None
}

fn main() {
    if std::env::var("FAKE_AGENT_IGNORE_TERM").as_deref() == Ok("1") {
        unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
    }
    let _child = (std::env::var("FAKE_AGENT_CHILD").as_deref() == Ok("1"))
        .then(|| std::process::Command::new("sleep").arg("300").spawn().unwrap());
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines().map_while(Result::ok);
    let mut next_out_id = 1000u64;
    let auth = std::env::var("FAKE_AGENT_AUTH").as_deref() == Ok("1");
    let mut signed_in = false;
    while let Some(line) = lines.next() {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let p = msg.get("params").cloned().unwrap_or(Value::Null);
        let reply = |result: Value| send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        match method {
            "initialize" => reply(json!({
                "protocolVersion": 1,
                "agentInfo": { "name": "fake-agent", "version": "0.0.1" },
                "agentCapabilities": { "loadSession": true, "_meta": { "ignored": true } },
                // Shaped like claude-agent-acp's, plus one `authenticate` method.
                "authMethods": if auth { json!([
                    { "id": "token", "name": "Token", "description": "Sign in with a token" },
                    { "id": "claude-ai-login", "name": "Claude Subscription", "description": "Use Claude subscription ",
                      "type": "terminal", "args": ["--cli", "auth", "login", "--claudeai"],
                      "_meta": { "terminal-auth": { "command": "/usr/bin/node", "args": ["/x/index.js", "--cli", "auth", "login", "--claudeai"], "label": "Claude Login" } } }
                ]) } else { json!([]) }
            })),
            "authenticate" => {
                signed_in = p["methodId"] == "token";
                if signed_in {
                    reply(json!({}))
                } else {
                    send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": "Method not implemented." } }))
                }
            }
            "session/new" | "session/prompt" if auth && !signed_in => {
                send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": "Authentication required" } }))
            }
            "session/new" => {
                send(json!({ "jsonrpc": "2.0", "method": "_fake/extension", "params": {} }));
                // Modes the old way, the model as a config option (in a group).
                reply(json!({
                    "sessionId": "s1",
                    "modes": {
                        "currentModeId": "default",
                        "availableModes": [
                            { "id": "default", "name": "Default" },
                            { "id": "acceptEdits", "name": "Accept Edits" },
                            { "id": "plan", "name": "Plan Mode", "description": "Read only" }
                        ]
                    },
                    "configOptions": [
                        { "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "sonnet",
                          "options": [{ "group": "all", "name": "All", "options": [
                              { "value": "sonnet", "name": "Sonnet" }, { "value": "opus", "name": "Opus" }
                          ] }] },
                        { "id": "fast", "name": "Fast", "type": "boolean", "currentValue": false }
                    ]
                }))
            }
            "session/set_mode" => {
                let sid = p["sessionId"].as_str().unwrap_or("s1").to_string();
                update(&sid, json!({ "sessionUpdate": "current_mode_update", "currentModeId": p["modeId"] }));
                reply(json!({}))
            }
            "session/set_config_option" => reply(json!({
                "configOptions": [
                    { "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": p["value"],
                      "options": [{ "value": "sonnet", "name": "Sonnet" }, { "value": "opus", "name": "Opus" }] }
                ]
            })),
            "session/load" => {
                let sid = p["sessionId"].as_str().unwrap_or("s1").to_string();
                update(&sid, json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": "earlier question" } }));
                chunk(&sid, "earlier answer");
                reply(Value::Null)
            }
            "session/prompt" => {
                let sid = p["sessionId"].as_str().unwrap_or("s1").to_string();
                let text: String = p["prompt"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect();
                if text.contains("crash") {
                    eprintln!("fake crash: something went wrong");
                    std::process::exit(3);
                }
                if text.contains("garbage") {
                    let mut out = std::io::stdout().lock();
                    writeln!(out, "this is not json").unwrap();
                    out.flush().unwrap();
                }
                if text.contains("picture") {
                    // A title for the session, then an image and a file link.
                    update(&sid, json!({ "sessionUpdate": "session_info_update", "title": "Draw a picture", "updatedAt": "2026-01-01T00:00:00Z" }));
                    update(&sid, json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "image", "mimeType": "image/png", "data": "iVBORw0KGgo=" } }));
                    update(&sid, json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "resource_link", "uri": "file:///w/a.txt", "name": "a.txt" } }));
                    reply(json!({ "stopReason": "max_tokens" }));
                    continue;
                }
                if text.contains("parallel") {
                    chunk(&sid, "Looking first.");
                    for (tid, title, kind) in [("r1", "Read a.txt", "read"), ("r2", "Read b.txt", "read"), ("x1", "grep -n needle a.txt", "execute")] {
                        update(&sid, json!({ "sessionUpdate": "tool_call", "toolCallId": tid, "title": title, "kind": kind, "status": "completed",
                                             "content": [{ "type": "content", "content": { "type": "text", "text": format!("{title}: found the needle") } }] }));
                    }
                    let mut asked = Vec::new();
                    for (tid, file) in [("w1", "a.txt"), ("w2", "b.txt")] {
                        update(&sid, json!({ "sessionUpdate": "tool_call", "toolCallId": tid, "title": format!("Write {file}"), "kind": "edit", "status": "pending" }));
                        let perm = format!("perm-{tid}");
                        send(json!({ "jsonrpc": "2.0", "id": perm, "method": "session/request_permission", "params": {
                            "sessionId": sid,
                            "toolCall": { "toolCallId": tid, "title": format!("Write {file}"), "kind": "edit" },
                            "options": [
                                { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                                { "optionId": "always", "name": "Always allow", "kind": "allow_always" },
                                { "optionId": "reject", "name": "Reject", "kind": "reject_once" } ] } }));
                        asked.push((tid, perm));
                    }
                    let mut chosen = Vec::new();
                    for _ in 0..asked.len() {
                        let r = wait_for(&mut lines, |v| asked.iter().any(|(_, p)| v["id"] == p.as_str()) || v["method"] == "session/cancel");
                        let Some(r) = r.filter(|r| r["method"] != "session/cancel") else { break };
                        let (tid, _) = asked.iter().find(|(_, p)| r["id"] == p.as_str()).unwrap();
                        let option = r["result"]["outcome"]["optionId"].as_str().unwrap_or("cancelled").to_string();
                        let status = if option == "allow" || option == "always" { "completed" } else { "failed" };
                        update(&sid, json!({ "sessionUpdate": "tool_call_update", "toolCallId": tid, "status": status }));
                        chosen.push(format!("{tid}={option}"));
                    }
                    chunk(&sid, &format!("chose {}", chosen.join(" ")));
                    reply(json!({ "stopReason": if chosen.len() == asked.len() { "end_turn" } else { "cancelled" } }));
                    continue;
                }
                if text.contains("slow") {
                    chunk(&sid, "working…");
                    wait_for(&mut lines, |v| v["method"] == "session/cancel");
                    reply(json!({ "stopReason": "cancelled" }));
                    continue;
                }
                chunk(&sid, "hello");
                update(&sid, json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Preparing file…", "kind": "edit", "status": "pending", "locations": [] }));
                update(&sid, json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "title": "Write a.txt", "locations": [{ "path": "/w/a.txt" }] }));
                update(&sid, json!({ "sessionUpdate": "plan", "entries": [
                    { "content": "write a.txt", "priority": "high", "status": "in_progress" },
                    { "content": "report", "priority": "low", "status": "pending" } ] }));
                // Permission round trip (string id, to test non-numeric ids).
                send(json!({ "jsonrpc": "2.0", "id": "perm-1", "method": "session/request_permission", "params": {
                    "sessionId": sid,
                    "toolCall": { "toolCallId": "t1", "title": "Write a.txt", "kind": "edit",
                                  "content": [{ "type": "diff", "path": "/w/a.txt", "oldText": null, "newText": "hi\n" }] },
                    "options": [
                        { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                        { "optionId": "always", "name": "Always allow", "kind": "allow_always" },
                        { "optionId": "reject", "name": "Reject", "kind": "reject_once" } ] } }));
                let mut cancelled = false;
                let resp = wait_for(&mut lines, |v| {
                    if v["method"] == "session/cancel" {
                        cancelled = true;
                    }
                    v["id"] == "perm-1"
                });
                let chosen = resp
                    .as_ref()
                    .and_then(|r| r["result"]["outcome"]["optionId"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "cancelled".into());
                chunk(&sid, &format!("chose {chosen}"));
                if cancelled || chosen == "cancelled" {
                    reply(json!({ "stopReason": "cancelled" }));
                    continue;
                }
                update(&sid, json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed" }));
                // An unsupported client method must be refused, not hang.
                next_out_id += 1;
                let fs_id = next_out_id;
                send(json!({ "jsonrpc": "2.0", "id": fs_id, "method": "fs/read_text_file", "params": { "sessionId": sid, "path": "/w/a.txt" } }));
                let r = wait_for(&mut lines, |v| v["id"] == fs_id);
                let code = r.and_then(|r| r["error"]["code"].as_i64()).unwrap_or(0);
                chunk(&sid, &format!("fs refused {code}"));
                reply(json!({ "stopReason": "end_turn" }));
            }
            "session/cancel" => {}
            _ if id.is_some() => send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "nope" } })),
            _ => {}
        }
    }
}
