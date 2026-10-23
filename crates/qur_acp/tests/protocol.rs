//! End-to-end protocol tests against the scripted fake agent, plus one
//! (ignored) run against the real claude-agent-acp.

use std::path::Path;
use std::time::{Duration, Instant};

use qur_acp::*;

const FAKE: &str = env!("CARGO_BIN_EXE_qur-fake-agent");

fn fake(env: &[(&str, &str)]) -> (Client, Events) {
    let cmd = AgentCommand {
        program: FAKE.into(),
        args: vec![],
        env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    };
    Client::spawn(&cmd, &std::env::temp_dir()).expect("spawn fake agent")
}

fn wait<T>(mut p: Pending<T>, secs: u64) -> Result<T, AcpError> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        match p.try_recv() {
            Ok(Some(r)) => return r,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => panic!("timed out waiting for a response"),
            Err(_) => panic!("responder dropped"),
        }
    }
}

/// Collect events until `stop` matches (inclusive) or `secs` pass.
fn events_until(rx: &mut Events, secs: u64, mut stop: impl FnMut(&Event) -> bool) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut out = Vec::new();
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok(ev) => {
                let done = stop(&ev);
                out.push(ev);
                if done {
                    return out;
                }
            }
            Err(futures::channel::mpsc::TryRecvError::Closed) => return out,
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("timed out; got {out:#?}");
}

fn texts(evs: &[Event]) -> Vec<String> {
    evs.iter()
        .filter_map(|e| match e {
            Event::Update { update: SessionUpdate::AgentMessage(c), .. } => Some(c.text.clone()),
            _ => None,
        })
        .collect()
}

fn group_gone(pgid: i32) -> bool {
    unsafe { libc::killpg(pgid, 0) != 0 }
}

fn start(client: &Client) -> String {
    let init = wait(client.initialize(), 10).unwrap();
    assert_eq!(init.protocol_version, 1);
    assert_eq!(init.agent_name.as_deref(), Some("fake-agent"));
    assert!(init.load_session);
    wait(client.new_session(Path::new("/tmp")), 10).unwrap()
}

#[test]
fn full_turn_with_permission_and_clean_shutdown() {
    let (client, mut rx) = fake(&[("FAKE_AGENT_CHILD", "1")]);
    let sid = start(&client);
    assert_eq!(sid, "s1");
    let turn = client.prompt(&sid, vec![PromptBlock::Text("do it".into()), PromptBlock::ResourceLink { uri: "file:///w/a.txt".into(), name: "a.txt".into() }]);

    let evs = events_until(&mut rx, 10, |e| matches!(e, Event::Permission { .. }));
    assert_eq!(texts(&evs), ["hello"]);
    assert!(evs.iter().any(|e| matches!(e, Event::Update { update: SessionUpdate::ToolCall(t), .. } if t.id == "t1" && t.kind == ToolKind::Edit)));
    assert!(evs.iter().any(|e| matches!(e, Event::Update { update: SessionUpdate::ToolCallUpdate(u), .. } if u.locations.as_ref().is_some_and(|l| l[0].path == Path::new("/w/a.txt")))));
    assert!(evs.iter().any(|e| matches!(e, Event::Update { update: SessionUpdate::Plan(p), .. } if p.len() == 2 && p[0].status == PlanStatus::InProgress)));
    assert!(!evs.iter().any(|e| matches!(e, Event::Warning(_))), "extension notifications are silent: {evs:?}");
    let Some(Event::Permission { id, session_id, tool_call, options }) = evs.last().cloned() else { unreachable!() };
    assert_eq!(session_id, "s1");
    assert_eq!(tool_call.to_tool_call().title, "Write a.txt");
    assert_eq!(options.iter().map(|o| o.kind).collect::<Vec<_>>(), [PermissionKind::AllowOnce, PermissionKind::AllowAlways, PermissionKind::RejectOnce]);

    client.respond_permission(id.clone(), PermissionOutcome::Selected("allow".into()));
    client.respond_permission(id, PermissionOutcome::Selected("reject".into())); // second answer is ignored
    assert_eq!(wait(turn, 10).unwrap(), StopReason::EndTurn);
    let evs = events_until(&mut rx, 10, |e| matches!(e, Event::Update { update: SessionUpdate::AgentMessage(c), .. } if c.text.starts_with("fs refused")));
    let t = texts(&evs);
    assert!(t.contains(&"chose allow".to_string()), "{t:?}");
    assert!(t.contains(&"fs refused -32601".to_string()), "unsupported requests get -32601: {t:?}");
    assert!(evs.iter().any(|e| matches!(e, Event::Warning(w) if w.contains("fs/read_text_file"))));

    let pgid = client.pid();
    assert!(!group_gone(pgid));
    let t0 = Instant::now();
    client.shutdown(Duration::from_secs(2));
    assert!(group_gone(pgid), "agent and its child are gone");
    assert!(t0.elapsed() < Duration::from_secs(2), "SIGTERM was enough: {:?}", t0.elapsed());
    let evs = events_until(&mut rx, 5, |e| matches!(e, Event::Exited { .. }));
    assert!(matches!(evs.last(), Some(Event::Exited { .. })));
    client.shutdown(Duration::from_secs(1)); // idempotent
    assert!(wait(client.new_session(Path::new("/tmp")), 2).is_err(), "requests after exit fail fast");
}

#[test]
fn cancel_resolves_the_prompt() {
    let (client, mut rx) = fake(&[]);
    let sid = start(&client);
    let turn = client.prompt(&sid, vec![PromptBlock::Text("slow".into())]);
    events_until(&mut rx, 10, |e| matches!(e, Event::Update { update: SessionUpdate::AgentMessage(_), .. }));
    client.cancel(&sid);
    assert_eq!(wait(turn, 10).unwrap(), StopReason::Cancelled);
    client.shutdown(Duration::from_secs(2));
    assert!(group_gone(client.pid()));
}

#[test]
fn cancel_answers_open_permission_requests() {
    let (client, mut rx) = fake(&[]);
    let sid = start(&client);
    let turn = client.prompt(&sid, vec![PromptBlock::Text("go".into())]);
    let evs = events_until(&mut rx, 10, |e| matches!(e, Event::Permission { .. }));
    let Some(Event::Permission { id, .. }) = evs.last().cloned() else { unreachable!() };
    client.cancel(&sid);
    assert_eq!(wait(turn, 10).unwrap(), StopReason::Cancelled);
    let evs = events_until(&mut rx, 10, |e| matches!(e, Event::Update { update: SessionUpdate::AgentMessage(c), .. } if c.text.starts_with("chose")));
    assert_eq!(texts(&evs).last().unwrap(), "chose cancelled");
    client.respond_permission(id, PermissionOutcome::Selected("allow".into())); // no-op after cancel
    client.shutdown(Duration::from_secs(2));
}

#[test]
fn crash_fails_pending_and_reports_stderr() {
    let (client, mut rx) = fake(&[]);
    let sid = start(&client);
    let turn = client.prompt(&sid, vec![PromptBlock::Text("crash".into())]);
    let err = wait(turn, 10).unwrap_err();
    assert!(err.message.contains("exited"), "{err}");
    let evs = events_until(&mut rx, 5, |e| matches!(e, Event::Exited { .. }));
    let Some(Event::Exited { code, stderr_tail }) = evs.last() else { unreachable!() };
    assert_eq!(*code, Some(3));
    assert!(stderr_tail.contains("fake crash"), "{stderr_tail:?}");
    assert!(client.has_exited());
    client.shutdown(Duration::from_millis(200));
}

#[test]
fn non_json_output_is_a_warning() {
    let (client, mut rx) = fake(&[]);
    let sid = start(&client);
    let turn = client.prompt(&sid, vec![PromptBlock::Text("garbage".into())]);
    let evs = events_until(&mut rx, 10, |e| matches!(e, Event::Warning(_)));
    assert!(matches!(evs.last(), Some(Event::Warning(w)) if w.contains("non-JSON")));
    client.cancel(&sid);
    let _ = wait(turn, 10);
    client.shutdown(Duration::from_secs(2));
}

#[test]
fn sigkill_after_grace_when_sigterm_is_ignored() {
    let (client, _rx) = fake(&[("FAKE_AGENT_IGNORE_TERM", "1"), ("FAKE_AGENT_CHILD", "1")]);
    start(&client);
    let pgid = client.pid();
    let t0 = Instant::now();
    client.shutdown(Duration::from_millis(300));
    let took = t0.elapsed();
    assert!(group_gone(pgid), "group survived SIGKILL");
    assert!(took >= Duration::from_millis(300), "waited for the grace period: {took:?}");
    assert!(took < Duration::from_secs(3), "{took:?}");
}

#[test]
fn load_session_replays_history() {
    let (client, mut rx) = fake(&[]);
    start(&client);
    wait(client.load_session("s1", Path::new("/tmp")), 10).unwrap();
    let evs = events_until(&mut rx, 10, |e| matches!(e, Event::Update { update: SessionUpdate::AgentMessage(_), .. }));
    assert!(evs.iter().any(|e| matches!(e, Event::Update { update: SessionUpdate::UserMessage(c), .. } if c.text == "earlier question")));
    assert_eq!(texts(&evs), ["earlier answer"]);
    client.shutdown(Duration::from_secs(2));
}

#[test]
fn api_keys_never_reach_the_agent() {
    let out = std::env::temp_dir().join(format!("qur-acp-env-{}", std::process::id()));
    let cmd = AgentCommand {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), format!("printf '%s|%s|%s' \"$ANTHROPIC_API_KEY\" \"$OPENAI_API_KEY\" \"$KEEP\" > {}", out.display())],
        env: vec![("ANTHROPIC_API_KEY".into(), "sk-x".into()), ("OPENAI_API_KEY".into(), "sk-y".into()), ("KEEP".into(), "yes".into())],
    };
    let (client, mut rx) = Client::spawn(&cmd, &std::env::temp_dir()).unwrap();
    events_until(&mut rx, 5, |e| matches!(e, Event::Exited { .. }));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "||yes");
    let _ = std::fs::remove_file(out);
    client.shutdown(Duration::from_millis(100));
}

/// Real Claude Code adapter, tiny prompt (owner asked to save usage).
/// `cargo test -p qur_acp -- --ignored --nocapture`
#[test]
#[ignore]
fn real_claude_agent_acp() {
    let repo = std::env::temp_dir().join(format!("qur-acp-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    assert!(std::process::Command::new("git").arg("-C").arg(&repo).args(["init", "-q"]).status().unwrap().success());
    let cmd = AgentCommand { program: "claude-agent-acp".into(), args: vec![], env: vec![] };
    let t0 = Instant::now();
    let (client, mut rx) = Client::spawn(&cmd, &repo).expect("claude-agent-acp on PATH");
    let init = wait(client.initialize(), 60).unwrap();
    eprintln!("initialize {:?}: {:?} {:?}", t0.elapsed(), init.agent_name, init.agent_version);
    assert_eq!(init.protocol_version, 1);
    let t1 = Instant::now();
    let sid = wait(client.new_session(&repo), 120).unwrap();
    eprintln!("session/new {:?}", t1.elapsed());
    let t2 = Instant::now();
    let stop = wait(client.prompt(&sid, vec![PromptBlock::Text("Reply with just the word ok.".into())]), 300).unwrap();
    eprintln!("session/prompt {:?} -> {stop:?}", t2.elapsed());
    assert_eq!(stop, StopReason::EndTurn);
    let mut all = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        all.push(ev);
    }
    let reply: String = texts(&all).concat();
    eprintln!("reply: {reply:?}; {} events, warnings: {:?}", all.len(), all.iter().filter(|e| matches!(e, Event::Warning(_))).collect::<Vec<_>>());
    assert!(reply.to_lowercase().contains("ok"), "{reply:?}");
    let pgid = client.pid();
    let t3 = Instant::now();
    client.shutdown(Duration::from_secs(2));
    eprintln!("shutdown {:?}", t3.elapsed());
    assert!(group_gone(pgid), "no orphans");
    let _ = std::fs::remove_dir_all(repo);
}
