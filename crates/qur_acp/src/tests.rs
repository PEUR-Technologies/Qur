use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;

/// Real messages from claude-agent-acp 0.94.0 (M0 spike logs).
fn fixture() -> Vec<Value> {
    include_str!("../tests/fixtures/claude-0.94.0.jsonl").lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn updates() -> Vec<SessionUpdate> {
    fixture()
        .iter()
        .filter(|m| m["method"] == "session/update")
        .map(|m| SessionUpdate::parse(&m["params"]["update"]))
        .collect()
}

#[test]
fn parses_real_claude_updates() {
    let ups = updates();
    assert!(ups.iter().any(|u| matches!(u, SessionUpdate::AgentMessage(c) if !c.text.is_empty())));
    assert!(ups.iter().any(|u| matches!(u, SessionUpdate::AvailableCommands(c) if !c.is_empty() && !c[0].name.is_empty())));
    let tc = ups.iter().find_map(|u| match u {
        SessionUpdate::ToolCall(t) => Some(t.clone()),
        _ => None,
    });
    let tc = tc.expect("a tool_call");
    assert!(!tc.id.is_empty());
    assert_ne!(tc.kind, ToolKind::Other);
    // Claude sends locations later, in an update (M0 finding).
    assert!(ups.iter().any(|u| matches!(u, SessionUpdate::ToolCallUpdate(t) if t.locations.as_ref().is_some_and(|l| !l.is_empty()))));
    assert!(ups.iter().any(|u| matches!(u, SessionUpdate::ToolCallUpdate(t) if t.status == Some(ToolStatus::Completed))));
    assert!(ups.iter().any(|u| matches!(u, SessionUpdate::ToolCallUpdate(t) if t.content.is_some())));
    // Context usage (shown above the prompt).
    assert!(ups.iter().any(|u| matches!(u, SessionUpdate::Usage { used: 24577, size: 1_000_000, cost: None })));
}

#[test]
fn command_output_and_exit_from_meta() {
    use serde_json::json;
    // What claude-agent-acp sends a client that advertises `terminal_output`.
    let start = SessionUpdate::parse(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "ls", "kind": "execute", "status": "pending",
        "content": [{ "type": "terminal", "terminalId": "t1" }], "rawInput": { "command": "ls" },
        "_meta": { "terminal_info": { "terminal_id": "t1" } }
    }));
    let SessionUpdate::ToolCall(mut t) = start else { panic!("tool_call") };
    assert_eq!(t.content, [ToolContent::Terminal("t1".into())]);
    let out = ToolCallUpdate::parse(&json!({ "toolCallId": "t1", "_meta": { "terminal_output": { "terminal_id": "t1", "data": "a\nb\n" } } }));
    t.apply(&out);
    assert_eq!(t.output.as_deref(), Some("a\nb\n"));
    assert_eq!(t.exit, None, "still running");
    let done = ToolCallUpdate::parse(&json!({ "toolCallId": "t1", "status": "completed", "_meta": { "terminal_exit": { "terminal_id": "t1", "exit_code": 2, "signal": null } } }));
    t.apply(&done);
    assert_eq!(t.exit, Some(Some(2)));
    let unknown = ToolCallUpdate::parse(&json!({ "toolCallId": "t1", "_meta": { "terminal_exit": { "terminal_id": "t1", "exit_code": null } } }));
    t.apply(&unknown);
    assert_eq!(t.exit, Some(None), "ended, code unknown");
}

#[test]
fn parses_real_permission_request() {
    let m = fixture().into_iter().find(|m| m["method"] == "session/request_permission").unwrap();
    let tc = ToolCallUpdate::parse(&m["params"]["toolCall"]);
    assert!(!tc.id.is_empty());
    let diff = tc.content.as_ref().unwrap().iter().find_map(|c| match c {
        ToolContent::Diff { path, new_text, .. } => Some((path.clone(), new_text.clone())),
        _ => None,
    });
    let (path, new_text) = diff.expect("permission carries a diff");
    assert!(path.is_absolute());
    assert!(!new_text.is_empty());
    let opts: Vec<_> = m["params"]["options"].as_array().unwrap().iter().filter_map(PermissionOption::parse).collect();
    assert!(opts.iter().any(|o| o.kind == PermissionKind::AllowOnce));
    assert!(opts.iter().any(|o| matches!(o.kind, PermissionKind::RejectOnce | PermissionKind::RejectAlways)));
}

#[test]
fn parses_schema_shaped_variants() {
    let plan = SessionUpdate::parse(&json!({
        "sessionUpdate": "plan",
        "entries": [
            { "content": "read files", "priority": "high", "status": "completed" },
            { "content": "edit", "priority": "medium", "status": "in_progress" },
            { "content": "test", "priority": "low", "status": "pending", "_meta": { "x": 1 } },
        ]
    }));
    let SessionUpdate::Plan(entries) = plan else { panic!("plan") };
    assert_eq!(entries.iter().map(|e| e.status).collect::<Vec<_>>(), [PlanStatus::Completed, PlanStatus::InProgress, PlanStatus::Pending]);
    assert_eq!(entries[0].priority, "high");

    let thought = SessionUpdate::parse(&json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "hmm" } }));
    assert_eq!(thought, SessionUpdate::AgentThought(ContentChunk::text("hmm")));
    let user = SessionUpdate::parse(&json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "image", "data": "..", "mimeType": "image/png" } }));
    assert_eq!(user, SessionUpdate::UserMessage(ContentChunk::text("[image]")));
    let mode = SessionUpdate::parse(&json!({ "sessionUpdate": "current_mode_update", "currentModeId": "plan" }));
    assert_eq!(mode, SessionUpdate::CurrentMode("plan".into()));
    let sub = SessionUpdate::parse(&json!({ "sessionUpdate": "subagent_update", "sessionId": "c1", "description": "Find tests", "state": { "state": "running" } }));
    assert_eq!(sub, SessionUpdate::Subagent { id: "c1".into(), title: Some("Find tests".into()), state: Some("running".into()) });
    let unknown = SessionUpdate::parse(&json!({ "sessionUpdate": "brand_new_thing", "x": 1 }));
    assert!(matches!(unknown, SessionUpdate::Other { kind, .. } if kind == "brand_new_thing"));

    let tc = SessionUpdate::parse(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Run tests", "kind": "execute",
        "status": "in_progress", "locations": [{ "path": "/w/a.ts", "line": 3 }, { "path": "/w/b.ts" }],
        "content": [{ "type": "content", "content": { "type": "text", "text": "ok" } },
                    { "type": "diff", "path": "/w/a.ts", "oldText": null, "newText": "x" },
                    { "type": "terminal", "terminalId": "term-1" }],
        "rawInput": { "command": "npm test" }
    }));
    let SessionUpdate::ToolCall(tc) = tc else { panic!("tool call") };
    assert_eq!((tc.kind, tc.status), (ToolKind::Execute, ToolStatus::InProgress));
    assert_eq!(tc.locations, vec![
        Location { path: PathBuf::from("/w/a.ts"), line: Some(3) },
        Location { path: PathBuf::from("/w/b.ts"), line: None },
    ]);
    assert_eq!(tc.content[1], ToolContent::Diff { path: "/w/a.ts".into(), old_text: None, new_text: "x".into() });
    assert_eq!(tc.content[2], ToolContent::Terminal("term-1".into()));
    assert_eq!(tc.raw_input.as_ref().unwrap()["command"], "npm test");

    // Unknown enum strings fall back instead of failing.
    let odd = SessionUpdate::parse(&json!({ "sessionUpdate": "tool_call", "toolCallId": "t2", "kind": "teleport", "status": "??" }));
    let SessionUpdate::ToolCall(odd) = odd else { panic!() };
    assert_eq!((odd.kind, odd.status), (ToolKind::Other, ToolStatus::Pending));
}

#[test]
fn tool_call_apply_merges_updates() {
    let mut tc = ToolCall { id: "t".into(), title: "Preparing file…".into(), kind: ToolKind::Edit, ..Default::default() };
    let u1 = ToolCallUpdate::parse(&json!({ "toolCallId": "t", "title": "Write a.ts", "locations": [{ "path": "/w/a.ts" }] }));
    tc.apply(&u1);
    assert_eq!(tc.title, "Write a.ts");
    assert_eq!(tc.kind, ToolKind::Edit, "absent kind keeps the old one");
    assert_eq!(tc.status, ToolStatus::Pending);
    assert_eq!(tc.locations.len(), 1);
    let u2 = ToolCallUpdate::parse(&json!({ "toolCallId": "t", "status": "completed", "kind": null }));
    assert_eq!(u2.kind, None, "null means absent");
    tc.apply(&u2);
    assert_eq!(tc.status, ToolStatus::Completed);
    assert_eq!(tc.locations.len(), 1, "absent locations keep the old ones");
    let u3 = ToolCallUpdate::parse(&json!({ "toolCallId": "t", "locations": [] }));
    tc.apply(&u3);
    assert!(tc.locations.is_empty(), "present (even empty) replaces");
    assert_eq!(u1.to_tool_call().title, "Write a.ts");
}

#[test]
fn json_shapes_we_send() {
    assert_eq!(PromptBlock::Text("hi".into()).to_json(), json!({ "type": "text", "text": "hi" }));
    assert_eq!(
        PromptBlock::Image { mime: "image/png".into(), data_base64: "AAAA".into() }.to_json(),
        json!({ "type": "image", "mimeType": "image/png", "data": "AAAA" })
    );
    assert_eq!(
        PromptBlock::ResourceLink { uri: "file:///w/a.ts".into(), name: "a.ts".into() }.to_json(),
        json!({ "type": "resource_link", "uri": "file:///w/a.ts", "name": "a.ts" })
    );
    assert_eq!(PermissionOutcome::Selected("allow".into()).to_json(), json!({ "outcome": { "outcome": "selected", "optionId": "allow" } }));
    assert_eq!(PermissionOutcome::Cancelled.to_json(), json!({ "outcome": { "outcome": "cancelled" } }));
    assert_eq!(StopReason::parse(&json!({ "stopReason": "end_turn" })), StopReason::EndTurn);
    assert_eq!(StopReason::parse(&json!({ "stopReason": "later" })), StopReason::Other("later".into()));
    let init = InitializeResult::parse(json!({
        "protocolVersion": 1, "agentInfo": { "name": "x", "version": "1.2" },
        "agentCapabilities": { "loadSession": true }, "authMethods": [{ "id": "chat-gpt", "name": "ChatGPT" }]
    }));
    assert_eq!((init.protocol_version, init.load_session), (1, true));
    assert_eq!(init.agent_version.as_deref(), Some("1.2"));
    assert_eq!(init.auth_methods.iter().map(|m| (m.id.as_str(), m.name.as_str(), m.terminal.is_some())).collect::<Vec<_>>(), [("chat-gpt", "ChatGPT", false)]);
    let e = AcpError::from_rpc(&json!({ "code": -32603, "message": "Internal error", "data": "boom" }));
    assert_eq!(e.to_string(), "Internal error: \"boom\" (-32603)");
}

#[test]
fn parses_claude_auth_methods() {
    // What claude-agent-acp 0.94 answers a client with `auth.terminal` and
    // `_meta["terminal-auth"]` (acp-agent.js `initialize`).
    let init = InitializeResult::parse(json!({
        "protocolVersion": 1,
        "authMethods": [
            { "description": "Use Claude subscription ", "name": "Claude Subscription", "id": "claude-ai-login", "type": "terminal",
              "args": ["--cli", "auth", "login", "--claudeai"],
              "_meta": { "terminal-auth": { "command": "/n/node", "args": ["/a/index.js", "--cli", "auth", "login", "--claudeai"], "label": "Claude Login" } } },
            { "description": "Use Anthropic Console (API usage billing)", "name": "Anthropic Console", "id": "console-login", "type": "terminal",
              "args": ["--cli", "auth", "login", "--console"], "env": [{ "name": "X", "value": "1" }] },
            { "id": "gateway", "name": "Custom model gateway", "description": "", "_meta": { "gateway": { "protocol": "anthropic" } } },
            { "name": "no id" }
        ]
    }));
    let m = &init.auth_methods;
    assert_eq!(m.len(), 3, "a method without an id is dropped");
    assert_eq!((m[0].id.as_str(), m[0].name.as_str(), m[0].description.as_deref()), ("claude-ai-login", "Claude Subscription", Some("Use Claude subscription")));
    let agent = AgentCommand { program: "/bin/claude-agent-acp".into(), args: vec!["--x".into()], env: vec![] };
    // `_meta` names the exact program; it wins over `type: "terminal"`.
    let t = m[0].terminal.as_ref().unwrap();
    assert_eq!(t.label.as_deref(), Some("Claude Login"));
    assert_eq!(t.command_line(&agent), ("/n/node".to_string(), vec!["/a/index.js".into(), "--cli".into(), "auth".into(), "login".into(), "--claudeai".into()]));
    // The spec's form: the agent's own command plus `args`.
    let t = m[1].terminal.as_ref().unwrap();
    assert_eq!(t.command_line(&agent), ("/bin/claude-agent-acp".to_string(), vec!["--x".into(), "--cli".into(), "auth".into(), "login".into(), "--console".into()]));
    assert_eq!(t.env, [("X".to_string(), "1".to_string())]);
    // Not a terminal login: sign in with `authenticate`.
    assert_eq!((m[2].terminal.as_ref(), m[2].description.as_ref()), (None, None));
}

#[test]
fn detects_auth_required() {
    let e = AcpError::from_rpc(&json!({ "code": -32000, "message": "Authentication required" }));
    assert!(e.is_auth_required());
    assert!(!AcpError::from_rpc(&json!({ "code": -32603, "message": "Internal error" })).is_auth_required());
    assert!(!AcpError::new("agent exited").is_auth_required());
}

#[test]
fn parses_session_title() {
    let t = SessionUpdate::parse(&json!({ "sessionUpdate": "session_info_update", "title": " Fix the login bug ", "updatedAt": "2026-01-01T00:00:00.000Z" }));
    assert_eq!(t, SessionUpdate::SessionTitle("Fix the login bug".into()));
    // Only `_meta` (AIR extensions), or a null title: nothing to show.
    let meta = SessionUpdate::parse(&json!({ "sessionUpdate": "session_info_update", "_meta": { "x": 1 } }));
    assert!(matches!(meta, SessionUpdate::Other { kind, .. } if kind == "session_info_update"));
    let null = SessionUpdate::parse(&json!({ "sessionUpdate": "session_info_update", "title": null }));
    assert!(matches!(null, SessionUpdate::Other { .. }));
}

#[test]
fn parses_image_and_link_blocks() {
    let chunk = |content: Value| match SessionUpdate::parse(&json!({ "sessionUpdate": "agent_message_chunk", "content": content })) {
        SessionUpdate::AgentMessage(c) => c,
        other => panic!("{other:?}"),
    };
    // "hi!" in base64; the MIME type is kept.
    let c = chunk(json!({ "type": "image", "mimeType": "image/jpeg", "data": "aGkh" }));
    assert_eq!(c, ContentChunk { text: String::new(), media: Some(Media::Image { mime: "image/jpeg".into(), data: b"hi!".to_vec() }) });
    // Claude's shape for an image given by URL: empty data, a uri.
    let c = chunk(json!({ "type": "image", "mimeType": "", "data": "", "uri": "https://x.dev/cat.png" }));
    assert_eq!(c.media, Some(Media::Link { uri: "https://x.dev/cat.png".into(), name: "cat.png".into() }));
    let c = chunk(json!({ "type": "resource_link", "uri": "file:///w/src/a.ts", "name": "a.ts" }));
    assert_eq!(c.media, Some(Media::Link { uri: "file:///w/src/a.ts".into(), name: "a.ts".into() }));
    let c = chunk(json!({ "type": "resource", "resource": { "uri": "file:///w/b.rs", "text": "fn main() {}" } }));
    assert_eq!(c.media, Some(Media::Link { uri: "file:///w/b.rs".into(), name: "b.rs".into() }));
    // Neither usable data nor a uri: a placeholder, as before.
    assert_eq!(chunk(json!({ "type": "image", "data": "%%%" })), ContentChunk::text("[image]"));
    assert_eq!(decode_base64("aGVsbG8gd29ybGQ="), Some(b"hello world".to_vec()));
    assert_eq!(decode_base64("aGVs\nbG8"), Some(b"hello".to_vec()));
    assert_eq!(decode_base64("a*b"), None);
}

#[test]
fn friendly_stop_reasons() {
    assert_eq!(StopReason::EndTurn.describe("Claude"), None);
    assert!(StopReason::Refusal.describe("Claude").unwrap().starts_with("Claude declined"));
    assert!(StopReason::MaxTokens.describe("Claude").unwrap().contains("length limit"));
    assert_eq!(StopReason::Other("tool_limit".into()).describe("Claude").unwrap(), "Claude ended the turn early (tool limit).");
}

#[test]
fn ids_are_typed() {
    let sub = SessionUpdate::parse(&json!({ "sessionUpdate": "subagent_update", "sessionId": "child-1" }));
    let SessionUpdate::Subagent { id, .. } = sub else { panic!("subagent") };
    assert_eq!(id, SessionId::from("child-1"));
    let u = ToolCallUpdate::parse(&json!({ "toolCallId": "toolu_1" }));
    assert_eq!(u.id, ToolCallId::from("toolu_1"));
    let by_id = std::collections::HashMap::from([(u.id.clone(), 1)]);
    assert_eq!(by_id.get("toolu_1"), Some(&1), "looked up by &str");
}
