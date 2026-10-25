//! Second-opinion review (research shortlist #1): what a reviewer is asked,
//! what it may do, and how its findings go back to the task's agent.
//!
//! The reviewer is a second ACP session in the task's worktree, possibly a
//! different agent (Codex reviewing a Claude task, or the reverse). It's
//! read-only because Qur denies every permission request it makes and puts
//! it in the agent's plan / read-only mode, not because of an OS sandbox.
//! Everything here is pure (no UI, no processes) so it can be tested.

use qur_acp::{ConfigOption, PermissionKind, PermissionOption};
use qur_agents::{AgentDef, Mode};
use qur_git::{FileDiff, FileStatus, LineKind};

/// Bytes of the task's chat sent to the reviewer.
pub const THREAD_CAP: usize = 24 * 1024;
/// Bytes of one chat message within that.
pub const ENTRY_CAP: usize = 3 * 1024;
/// Bytes of the task's diff sent to the reviewer.
pub const DIFF_CAP: usize = 96 * 1024;
/// Bytes of the plan (Review Agent's Plan…).
pub const PLAN_CAP: usize = 16 * 1024;
/// Bytes of the user's instruction line.
pub const INSTRUCTION_CAP: usize = 2 * 1024;
/// Bytes of findings handed back to the agent.
pub const FINDINGS_CAP: usize = 32 * 1024;
/// The whole review prompt never exceeds this (the caps above, plus the fixed text).
pub const PROMPT_CAP: usize = THREAD_CAP + DIFF_CAP + PLAN_CAP + INSTRUCTION_CAP + 8 * 1024;

/// What's reviewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    /// The task's changes so far (its diff against the base commit).
    Work,
    /// The agent's plan (its To-do), before or while it works.
    Plan,
}

impl Subject {
    pub fn as_str(self) -> &'static str {
        match self {
            Subject::Work => "work",
            Subject::Plan => "plan",
        }
    }
    pub fn parse(s: &str) -> Subject {
        if s == "plan" { Subject::Plan } else { Subject::Work }
    }
}

// -- the task's thread ---------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Agent,
    /// A tool call: its title (and file).
    Tool,
}

impl Role {
    fn tag(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Agent => "agent",
            Role::Tool => "tool",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub role: Role,
    pub text: String,
}

impl Entry {
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Entry { role, text: text.into() }
    }
}

/// `s` cut to at most `max` bytes on a char boundary, marked when cut.
pub fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    const MARK: &str = " […cut]";
    let mut end = max.saturating_sub(MARK.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{MARK}", &s[..end])
}

fn render_entry(e: &Entry) -> String {
    let tag = e.role.tag();
    // Tool calls are one line; messages keep their text, clipped.
    let text = clip(e.text.trim(), if e.role == Role::Tool { 200 } else { ENTRY_CAP });
    format!("<{tag}>\n{text}\n</{tag}>\n")
}

/// The task's chat as tagged text, at most `cap` bytes. The first entry
/// (normally the task itself) is always kept; then the newest entries that
/// fit, with a marker for what was left out in between.
pub fn serialize_thread(entries: &[Entry], cap: usize) -> String {
    let entries: Vec<&Entry> = entries.iter().filter(|e| !e.text.trim().is_empty()).collect();
    let Some((first, rest)) = entries.split_first() else { return String::new() };
    let head = clip(&render_entry(first), cap);
    let marker = |n: usize| format!("<omitted entries=\"{n}\"/>\n");
    // Room for the head and a marker, whatever follows.
    let mut budget = cap.saturating_sub(head.len() + marker(rest.len()).len());
    let mut tail: Vec<String> = Vec::new();
    for e in rest.iter().rev() {
        let r = render_entry(e);
        if r.len() > budget {
            break;
        }
        budget -= r.len();
        tail.push(r);
    }
    let omitted = rest.len() - tail.len();
    let mut out = head;
    if omitted > 0 {
        out.push_str(&marker(omitted));
    }
    out.extend(tail.into_iter().rev());
    out
}

// -- the task's diff -----------------------------------------------------------

/// The task's changes as a unified diff, at most `cap` bytes: whole files
/// while they fit, then a list of the files left out.
pub fn diff_text(files: &[FileDiff], cap: usize) -> String {
    let mut out = String::new();
    let mut left_out: Vec<String> = Vec::new();
    for f in files {
        let path = f.path.display().to_string();
        let old = f.old_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| path.clone());
        let mut one = format!("diff --git a/{old} b/{path}\n");
        match f.status {
            FileStatus::Binary => one.push_str("Binary file changed\n"),
            _ => {
                let from = if f.status == FileStatus::Added { "/dev/null".to_string() } else { format!("a/{old}") };
                let to = if f.status == FileStatus::Deleted { "/dev/null".to_string() } else { format!("b/{path}") };
                one.push_str(&format!("--- {from}\n+++ {to}\n"));
                for h in &f.hunks {
                    one.push_str(&h.header);
                    one.push('\n');
                    for l in &h.lines {
                        one.push(match l.kind {
                            LineKind::Added => '+',
                            LineKind::Removed => '-',
                            LineKind::Context => ' ',
                        });
                        one.push_str(&l.text);
                        one.push('\n');
                    }
                }
            }
        }
        // Keep room for the list of what's left out.
        if left_out.is_empty() && out.len() + one.len() + 512 <= cap {
            out.push_str(&one);
        } else {
            left_out.push(format!("{path} (+{} −{})", f.added, f.removed));
        }
    }
    if !left_out.is_empty() {
        let mut note = format!("[diff cut to fit: {} more file(s), read them in the worktree: ", left_out.len());
        for (i, f) in left_out.iter().enumerate() {
            if out.len() + note.len() + f.len() + 8 > cap {
                note.push('…');
                break;
            }
            if i > 0 {
                note.push_str(", ");
            }
            note.push_str(f);
        }
        note.push_str("]\n");
        out.push_str(&note);
    }
    clip(&out, cap)
}

// -- the prompts -----------------------------------------------------------------

const READ_ONLY: &str = "You are an independent reviewer giving a second opinion on another coding agent's task, in that task's working directory.
This session is read-only: do not modify files, create commits or branches, or run commands that change anything. Qur denies every edit and command that asks for permission.
Read files in the working directory when you need more context than the diff.";

const FINDING_FORMAT: &str = "For each finding: Location, Issue, Criticality: N/10, Suggestion. Always write the score with the /10. Bullets only, no prose.";

const DIMENSIONS: &str = "Only raise findings you would block a pull request on: real bugs, real risks, or a clearly better approach. No nitpicks, no speculation.

Score each finding by severity, likelihood, user impact and engineering risk: 10/10 blocks a release, 7-9/10 is high risk, 4-6/10 matters but needn't block, 1-3/10 is minor.

Look at:
1. Correctness: logic errors, wrong assumptions, broken edge cases, regressions.
2. Security: injection, unsafe input handling, secrets in code, trust boundaries.
3. Better approaches: clearly simpler or safer reuse of what the code already has.
4. Tests: tests that wouldn't catch a regression, missing edge cases, brittle assertions.
5. Robustness: unexpected input, concurrency, partial failure, swallowed errors.";

const SENSITIVITY: &str = "Don't comment on style, formatting or naming unless it causes a bug.
Check the surrounding code for existing patterns before suggesting new ones.
If you have no blocking findings, say so in one line.
End with a short section \"Things that might be intentional (confirm)\" with up to 3 items.";

/// What the reviewer is sent: the task's thread, its diff (and plan), the
/// findings format, and the user's instruction. Never longer than `PROMPT_CAP`.
pub fn review_prompt(subject: Subject, thread: &str, base: &str, diff: &str, plan: Option<&str>, instruction: &str) -> String {
    let mut p = format!("{READ_ONLY}\n\n");
    let thread = clip(thread, THREAD_CAP);
    if !thread.is_empty() {
        p.push_str(&format!("<task_thread>\n{thread}</task_thread>\n\n"));
    }
    let diff = clip(diff, DIFF_CAP);
    let short_base = &base[..base.len().min(12)];
    if diff.trim().is_empty() {
        p.push_str(&format!("<task_diff base=\"{short_base}\">\n(no changes yet)\n</task_diff>\n\n"));
    } else {
        p.push_str(&format!("<task_diff base=\"{short_base}\">\n{diff}</task_diff>\n\n"));
    }
    match subject {
        Subject::Plan => {
            let plan = clip(plan.unwrap_or("").trim(), PLAN_CAP);
            p.push_str(&format!("<plan_to_review>\n{plan}\n</plan_to_review>\n\n"));
            p.push_str("Review this plan. Check its assumptions against the current code and the diff so far. Put gaps and blockers first.\n");
        }
        Subject::Work => {
            p.push_str("Review the work so far: the diff above is everything the task changed against its base commit. Put bugs, regressions and risky complexity first.\n");
        }
    }
    p.push_str(&format!("{FINDING_FORMAT}\n\n{DIMENSIONS}\n\n{SENSITIVITY}\n"));
    let instruction = clip(instruction.trim(), INSTRUCTION_CAP);
    if !instruction.is_empty() {
        p.push_str(&format!("\n<additional_instructions>\n{instruction}\n</additional_instructions>\n"));
    }
    // The parts are capped already; this only guards the fixed text.
    clip(&p, PROMPT_CAP)
}

/// What goes into the task agent's prompt (editable before it's sent): the
/// reviewer's findings and how to answer each one.
pub fn handoff_prompt(subject: Subject, reviewer: &str, findings: &str) -> String {
    let what = match subject {
        Subject::Work => "your recent work",
        Subject::Plan => "your plan",
    };
    let findings = clip(findings.trim(), FINDINGS_CAP);
    format!(
        "<review_feedback reviewer=\"{reviewer}\">\n{findings}\n</review_feedback>\n\n\
A second reviewer ({reviewer}) shared the feedback above on {what}. For each finding, answer in this format:\n\n\
### Finding N: <short summary>\n\
- Criticality: N/10 (keep the reviewer's score, or give one)\n\
- Decision: Agree | Disagree\n\
- Why: <brief reasoning>\n\
- Fix: <the change you would make, or N/A if you disagree>\n\n\
Then list the fixes you agree with under \"### Proposed Changes\", and ask: \"Would you like me to proceed with the agreed-upon changes?\" Don't change any files before I answer."
    )
}

// -- read-only enforcement ---------------------------------------------------------

/// How Qur answers a reviewer's permission request: always no. A reviewer
/// that asks is about to edit, run a command or otherwise act (reads in the
/// worktree don't ask), so it's denied without showing the permission bar.
/// The agent's own reject option when it has one, else the request is cancelled (None).
pub fn deny_option(options: &[PermissionOption]) -> Option<String> {
    [PermissionKind::RejectOnce, PermissionKind::RejectAlways]
        .iter()
        .find_map(|k| options.iter().find(|o| o.kind == *k))
        .map(|o| o.id.clone())
}

/// The read-only mode to start the reviewer in: `plan` (Claude), else a
/// read-only mode (Codex), else none (Qur's deny still applies).
pub fn read_only_mode(settings: &[ConfigOption]) -> Option<(String, String)> {
    let mode = settings.iter().find(|o| o.is_mode())?;
    let norm = |s: &str| s.to_lowercase().replace(['-', '_', ' '], "");
    let pick = mode
        .values
        .iter()
        .find(|v| v.value == "plan")
        .or_else(|| mode.values.iter().find(|v| norm(&v.value).contains("readonly") || norm(&v.name).contains("readonly")))
        .or_else(|| mode.values.iter().find(|v| norm(&v.name) == "plan" || norm(&v.name).starts_with("planmode")))?;
    Some((mode.id.clone(), pick.value.clone()))
}

/// The session's model setting, if the agent offers one.
pub fn model_option(settings: &[ConfigOption]) -> Option<&ConfigOption> {
    settings.iter().find(|o| o.category.as_deref() == Some("model")).or_else(|| settings.iter().find(|o| o.id == "model"))
}

// -- choosing the reviewer --------------------------------------------------------

/// Which account an agent uses ("claude", "codex", or its id).
pub fn vendor(def: &AgentDef) -> String {
    let c = def.command.to_lowercase();
    if c.contains("codex") {
        "codex".into()
    } else if c.contains("claude") {
        "claude".into()
    } else {
        def.id.clone()
    }
}

/// One entry in the reviewer picker.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// Index into the roster.
    pub agent: usize,
    /// (value, display name) of the model; None = the agent's default.
    pub model: Option<(String, String)>,
}

impl Choice {
    /// "Codex", "Claude Code · Sonnet 4.5".
    pub fn label(&self, roster: &[AgentDef]) -> String {
        let a = roster.get(self.agent).map(|a| a.label.as_str()).unwrap_or("agent");
        match &self.model {
            Some((_, name)) => format!("{a} · {name}"),
            None => a.to_string(),
        }
    }

}

/// A reviewer's short name: its model's first word ("opus"), or its vendor ("codex").
pub fn short_name(def: &AgentDef, model_name: Option<&str>) -> String {
    match model_name.and_then(|n| n.split_whitespace().next()) {
        Some(w) => w.to_lowercase(),
        None => vendor(def),
    }
}

/// Every reviewer on offer: each installed ACP agent, and for the task's
/// own agent each of its models (other models first; the same model last,
/// as "fresh eyes"). Other agents start on their default model.
pub fn choices(roster: &[AgentDef], task_agent: &str, task_models: Option<&ConfigOption>, installed: impl Fn(&AgentDef) -> bool) -> Vec<Choice> {
    let mut out = Vec::new();
    for (i, a) in roster.iter().enumerate() {
        if a.mode != Mode::Acp || !installed(a) {
            continue;
        }
        match task_models.filter(|_| a.id == task_agent) {
            Some(m) if !m.values.is_empty() => {
                let others = m.values.iter().filter(|v| v.value != m.current);
                let same = m.values.iter().filter(|v| v.value == m.current);
                out.extend(others.chain(same).map(|v| Choice { agent: i, model: Some((v.value.clone(), v.name.clone())) }));
            }
            _ => out.push(Choice { agent: i, model: None }),
        }
    }
    out
}

/// The default: another vendor's agent that's signed in; else another
/// model of the task's own agent; else the first choice.
pub fn default_choice(choices: &[Choice], roster: &[AgentDef], task_agent: &str, signed_in: impl Fn(&AgentDef) -> bool) -> usize {
    let task_vendor = roster.iter().find(|a| a.id == task_agent).map(vendor);
    let def = |c: &Choice| roster.get(c.agent);
    choices
        .iter()
        .position(|c| def(c).is_some_and(|a| Some(vendor(a)) != task_vendor && signed_in(a)))
        .or_else(|| choices.iter().position(|c| def(c).is_some_and(|a| a.id == task_agent) && c.model.is_some()))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qur_acp::ConfigValue;
    use qur_git::{DiffLine, Hunk};
    use std::path::PathBuf;

    fn agent(id: &str, command: &str, mode: Mode) -> AgentDef {
        AgentDef { id: id.into(), label: id.to_uppercase(), mode, command: command.into(), args: vec![], accent: None, prompt: None }
    }

    fn select(id: &str, category: &str, current: &str, values: &[(&str, &str)]) -> ConfigOption {
        ConfigOption {
            id: id.into(),
            name: id.into(),
            category: Some(category.into()),
            current: current.into(),
            values: values.iter().map(|(v, n)| ConfigValue { value: v.to_string(), name: n.to_string(), description: None }).collect(),
            legacy_mode: false,
        }
    }

    #[test]
    fn thread_keeps_the_task_and_the_newest_under_the_cap() {
        let mut e = vec![Entry::new(Role::User, "Fix the nav bar")];
        for i in 0..200 {
            e.push(Entry::new(Role::Tool, format!("Read src/file{i}.rs")));
            e.push(Entry::new(Role::Agent, format!("step {i}: {}", "x".repeat(300))));
        }
        e.push(Entry::new(Role::Agent, "All done: the nav works."));
        let t = serialize_thread(&e, 4096);
        assert!(t.len() <= 4096, "{} bytes", t.len());
        assert!(t.starts_with("<user>\nFix the nav bar\n</user>\n"), "the task stays first");
        assert!(t.ends_with("<agent>\nAll done: the nav works.\n</agent>\n"), "the newest is kept");
        assert!(t.contains("<omitted entries=\""), "says what's left out");
        // Everything fits: nothing omitted, order kept.
        let small = serialize_thread(&e[..3], 4096);
        assert!(!small.contains("omitted") && small.find("<tool>").unwrap() < small.find("<agent>").unwrap());
        assert_eq!(serialize_thread(&[], 100), "");
    }

    #[test]
    fn thread_clips_long_messages_and_tiny_caps() {
        let long = "é".repeat(10_000);
        let t = serialize_thread(&[Entry::new(Role::User, long.clone()), Entry::new(Role::Agent, long)], 100_000);
        assert!(t.len() < 2 * ENTRY_CAP + 200 && t.contains("[…cut]"), "each message is clipped ({} bytes)", t.len());
        let t = serialize_thread(&[Entry::new(Role::User, "a".repeat(500))], 64);
        assert!(t.len() <= 64, "even the first entry obeys the cap");
    }

    fn file(path: &str, status: FileStatus, n: usize) -> FileDiff {
        let lines = (0..n)
            .map(|i| DiffLine { kind: if i % 2 == 0 { LineKind::Added } else { LineKind::Removed }, old_no: None, new_no: None, text: format!("line {i}"), emphasis: vec![] })
            .collect();
        FileDiff {
            path: PathBuf::from(path),
            old_path: None,
            status,
            added: n as u32 / 2,
            removed: n as u32 / 2,
            hunks: vec![Hunk { old_start: 1, old_lines: 1, new_start: 1, new_lines: 1, header: "@@ -1,1 +1,1 @@".into(), lines }],
        }
    }

    #[test]
    fn diff_is_unified_and_capped() {
        let d = diff_text(&[file("a.rs", FileStatus::Modified, 2), file("new.rs", FileStatus::Added, 2)], 10_000);
        assert!(d.contains("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,1 +1,1 @@\n+line 0\n-line 1\n"), "{d}");
        assert!(d.contains("--- /dev/null\n+++ b/new.rs"), "{d}");
        let big: Vec<FileDiff> = (0..50).map(|i| file(&format!("f{i}.rs"), FileStatus::Modified, 400)).collect();
        let d = diff_text(&big, 20_000);
        assert!(d.len() <= 20_000, "{} bytes", d.len());
        assert!(d.contains("diff --git a/f0.rs") && d.contains("[diff cut to fit:") && d.contains("(+200 −200)"), "lists what's left out");
        assert!(!d.contains("diff --git a/f49.rs"));
    }

    #[test]
    fn prompt_has_the_parts_and_obeys_the_cap() {
        let p = review_prompt(Subject::Work, "<user>\nFix nav\n</user>\n", "0123456789abcdef", "diff --git a/x b/x\n", None, "focus on the tests");
        for part in ["read-only", "<task_thread>", "Fix nav", "<task_diff base=\"0123456789ab\">", "Criticality: N/10", "Things that might be intentional", "<additional_instructions>\nfocus on the tests"] {
            assert!(p.contains(part), "missing {part}");
        }
        assert!(!p.contains("<plan_to_review>"));
        let plan = review_prompt(Subject::Plan, "", "abc", "", Some("- [ ] add tests"), "");
        assert!(plan.contains("<plan_to_review>\n- [ ] add tests") && plan.contains("(no changes yet)") && !plan.contains("<task_thread>") && !plan.contains("additional_instructions"));
        let huge = "z".repeat(1_000_000);
        let p = review_prompt(Subject::Plan, &huge, "abc", &huge, Some(&huge), &huge);
        assert!(p.len() <= PROMPT_CAP, "{} > {PROMPT_CAP}", p.len());
        assert!(p.ends_with("</additional_instructions>\n"), "each part is capped on its own; the whole is never cut");
    }

    #[test]
    fn handoff_asks_per_finding_then_asks_before_proceeding() {
        let h = handoff_prompt(Subject::Work, "codex", "- Location: a.rs:3\n- Issue: off by one\n- Criticality: 7/10\n");
        assert!(h.starts_with("<review_feedback reviewer=\"codex\">\n- Location: a.rs:3"));
        for part in ["your recent work", "Decision: Agree | Disagree", "- Why:", "- Fix:", "### Proposed Changes", "Would you like me to proceed", "Don't change any files"] {
            assert!(h.contains(part), "missing {part}");
        }
        assert!(handoff_prompt(Subject::Plan, "opus", "x").contains("your plan"));
        assert!(handoff_prompt(Subject::Work, "x", &"f".repeat(100_000)).len() < FINDINGS_CAP + 2048);
    }

    #[test]
    fn every_permission_request_is_denied() {
        let opt = |id: &str, kind| PermissionOption { id: id.into(), name: id.into(), kind };
        let all = [opt("allow", PermissionKind::AllowOnce), opt("always", PermissionKind::AllowAlways), opt("reject-always", PermissionKind::RejectAlways), opt("reject", PermissionKind::RejectOnce)];
        assert_eq!(deny_option(&all).as_deref(), Some("reject"), "reject once first");
        assert_eq!(deny_option(&all[..3]).as_deref(), Some("reject-always"));
        assert_eq!(deny_option(&all[..2]), None, "no reject option: cancel, never allow");
        assert_eq!(deny_option(&[]), None);
    }

    #[test]
    fn read_only_mode_prefers_plan_then_read_only() {
        let claude = select("mode", "mode", "default", &[("default", "Default"), ("acceptEdits", "Accept Edits"), ("plan", "Plan Mode"), ("bypassPermissions", "Bypass")]);
        assert_eq!(read_only_mode(&[claude]), Some(("mode".into(), "plan".into())));
        let codex = select("mode", "mode", "auto", &[("read-only", "Read Only"), ("auto", "Default"), ("full-access", "Full Access")]);
        assert_eq!(read_only_mode(&[codex]), Some(("mode".into(), "read-only".into())));
        let none = select("mode", "mode", "a", &[("a", "Ask"), ("b", "Code")]);
        assert_eq!(read_only_mode(&[none]), None);
        assert_eq!(read_only_mode(&[select("model", "model", "x", &[("plan", "Plan")])]), None, "only the mode setting");
    }

    #[test]
    fn reviewer_defaults_to_the_other_vendor_when_signed_in() {
        let roster = vec![
            agent("claude", "claude-agent-acp", Mode::Acp),
            agent("codex", "codex-acp", Mode::Acp),
            agent("claude-term", "claude", Mode::Terminal),
            agent("gone", "not-installed-acp", Mode::Acp),
        ];
        let models = select("model", "model", "opus", &[("opus", "Opus 5.5"), ("sonnet", "Sonnet 5")]);
        let installed = |a: &AgentDef| a.id != "gone";
        let c = choices(&roster, "claude", Some(&models), installed);
        let labels: Vec<String> = c.iter().map(|c| c.label(&roster)).collect();
        assert_eq!(labels, ["CLAUDE · Sonnet 5", "CLAUDE · Opus 5.5", "CODEX"], "ACP, installed, other models first");
        assert_eq!(short_name(&roster[0], Some("Sonnet 5")), "sonnet");
        assert_eq!(short_name(&roster[1], None), "codex");
        // Codex signed in: it reviews the Claude task.
        assert_eq!(default_choice(&c, &roster, "claude", |_| true), 2);
        // Not signed in: another Claude model.
        assert_eq!(default_choice(&c, &roster, "claude", |a| vendor(a) == "claude"), 0);
        // A Codex task (its models unknown to Qur): Claude reviews it.
        let c = choices(&roster, "codex", None, installed);
        assert_eq!(c.len(), 2);
        assert_eq!(default_choice(&c, &roster, "codex", |_| true), 0);
        assert_eq!(roster[c[0].agent].id, "claude");
    }
}
