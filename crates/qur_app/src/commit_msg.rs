//! A commit message from what an agent did, built from what Qur already has
//! (the task's title, its turn records, your prompts and the agent's last
//! message per turn). Pure text work: no network, no agent.

use std::path::PathBuf;

/// Git's conventional subject width.
pub const SUBJECT_MAX: usize = 72;
/// Turn lines in the body before "and N more turns".
const MAX_TURN_LINES: usize = 8;
/// Files listed in the body when there are no turn lines to describe the work.
const MAX_FILE_LINES: usize = 6;

/// One turn that changed files.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnNote {
    pub n: u32,
    pub files: u32,
    pub added: u32,
    pub removed: u32,
    /// What you asked (the turn's message), when the chat still has it.
    pub prompt: Option<String>,
    /// The agent's last message in that turn.
    pub reply: Option<String>,
}

/// Everything the message is made from.
#[derive(Clone, Debug, Default)]
pub struct Source {
    /// The task's title (the agent's session title or your rename).
    pub title: String,
    /// Turns that changed files, oldest first (rewound ones left out).
    pub turns: Vec<TurnNote>,
    /// Every changed file with lines added and removed.
    pub files: Vec<(PathBuf, u32, u32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub subject: String,
    pub body: String,
}

impl Message {
    /// Subject, a blank line and the body (just the subject without one).
    pub fn full(&self) -> String {
        if self.body.is_empty() { self.subject.clone() } else { format!("{}\n\n{}", self.subject, self.body) }
    }

    /// A merge commit's message: "Merge <branch>: <subject>" kept within
    /// the subject width, and the same body.
    pub fn merge(&self, branch: &str) -> String {
        let head = format!("Merge {branch}: ");
        let room = SUBJECT_MAX.saturating_sub(head.chars().count()).max(20);
        let m = Message { subject: format!("{head}{}", fit(&self.subject, room)), body: self.body.clone() };
        m.full()
    }
}

/// The message for `src`.
pub fn generate(src: &Source) -> Message {
    let notes: Vec<(String, &TurnNote)> = src.turns.iter().filter_map(|t| turn_summary(t).map(|s| (s, t))).collect();
    let title = clean_line(&src.title);
    let subject = if !title.is_empty() && !placeholder(&title) {
        imperative(&title).0
    } else if let Some((s, _)) = notes.first() {
        s.clone()
    } else {
        match src.files.as_slice() {
            [] => "Update files".into(),
            [(p, ..)] => format!("Update {}", p.display()),
            f => format!("Update {} files", f.len()),
        }
    };
    let subject = fit(&subject, SUBJECT_MAX);

    let mut body: Vec<String> = Vec::new();
    // A single turn that just repeats the subject says nothing new.
    let repeats = notes.len() == 1 && notes[0].0.eq_ignore_ascii_case(&subject);
    if !repeats {
        for (s, t) in notes.iter().take(if notes.len() > MAX_TURN_LINES { MAX_TURN_LINES - 1 } else { MAX_TURN_LINES }) {
            let stats = format!(" ({}, +{} -{})", plural(t.files, "file"), t.added, t.removed);
            let room = SUBJECT_MAX.saturating_sub(2 + stats.chars().count()).max(20);
            body.push(format!("- {}{stats}", fit(s, room)));
        }
        if notes.len() > MAX_TURN_LINES {
            body.push(format!("- and {} more turns", notes.len() - (MAX_TURN_LINES - 1)));
        }
    }
    if !src.files.is_empty() {
        if body.is_empty() && src.files.len() > 1 {
            for (p, a, r) in src.files.iter().take(MAX_FILE_LINES) {
                body.push(format!("- {} (+{a} -{r})", p.display()));
            }
            if src.files.len() > MAX_FILE_LINES {
                body.push(format!("- and {} more", src.files.len() - MAX_FILE_LINES));
            }
        }
        let (a, r) = src.files.iter().fold((0, 0), |(a, r), (_, x, y)| (a + x, r + y));
        if !body.is_empty() {
            body.push(String::new());
        }
        body.push(format!("{} changed, +{a} -{r}", plural(src.files.len() as u32, "file")));
    }
    Message { subject, body: body.join("\n") }
}

fn plural(n: u32, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// A turn's line: the agent's own account when it starts with a verb
/// ("Fixed the parser…" → "Fix the parser"), else what you asked, else
/// whatever the agent said first.
fn turn_summary(t: &TurnNote) -> Option<String> {
    let reply = t.reply.as_deref().map(first_sentence).filter(|s| !s.is_empty()).map(|s| imperative(&s));
    let prompt = t.prompt.as_deref().map(clean_line).filter(|s| !s.is_empty()).map(|s| imperative(&s).0);
    match (reply, prompt) {
        (Some((r, true)), _) => Some(r),
        (_, Some(p)) => Some(p),
        (Some((r, false)), None) => Some(r),
        (None, None) => None,
    }
    .filter(|s| !s.is_empty())
}

/// Default task names that say nothing about the work.
fn placeholder(title: &str) -> bool {
    let t = title.to_lowercase();
    let numbered = |prefix: &str| t.strip_prefix(prefix).is_some_and(|n| n.trim().chars().all(|c| c.is_ascii_digit()));
    numbered("agent terminal") || numbered("task") || numbered("new task") || numbered("claude") || numbered("codex")
}

/// The first non-empty line, without Markdown marks, leading glyphs (a
/// terminal title's spinner) or extra spaces.
pub fn clean_line(s: &str) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("```")).unwrap_or("");
    let line = line.replace("**", "").replace("__", "").replace('`', "");
    let line = line.trim_start_matches(|c: char| !c.is_alphanumeric());
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first sentence of a message's first line.
fn first_sentence(s: &str) -> String {
    let line = clean_line(s);
    let mut end = line.len();
    for (i, c) in line.char_indices() {
        if matches!(c, '.' | '!' | '?' | ':' | ';') {
            let next = line[i + c.len_utf8()..].chars().next();
            if next.is_none_or(char::is_whitespace) {
                end = i;
                break;
            }
        }
    }
    line[..end].trim().to_string()
}

/// Verb forms → base, for turning "Fixing…", "Fixed…", "Adds…" into "Fix…".
const VERBS: &[&str] = &[
    "add|adding|added|adds",
    "fix|fixing|fixed|fixes",
    "update|updating|updated|updates",
    "remove|removing|removed|removes",
    "delete|deleting|deleted|deletes",
    "refactor|refactoring|refactored|refactors",
    "implement|implementing|implemented|implements",
    "create|creating|created|creates",
    "rename|renaming|renamed|renames",
    "move|moving|moved|moves",
    "rewrite|rewriting|rewrote|rewrites|rewritten",
    "write|writing|wrote|writes|written",
    "make|making|made|makes",
    "improve|improving|improved|improves",
    "clean|cleaning|cleaned|cleans",
    "change|changing|changed|changes",
    "replace|replacing|replaced|replaces",
    "support|supporting|supported|supports",
    "handle|handling|handled|handles",
    "use|using|used|uses",
    "bump|bumping|bumped|bumps",
    "simplify|simplifying|simplified|simplifies",
    "extract|extracting|extracted|extracts",
    "document|documenting|documented|documents",
    "test|testing|tested|tests",
    "build|building|built|builds",
    "run|running|ran|runs",
    "set|setting|sets",
    "debug|debugging|debugged|debugs",
    "convert|converting|converted|converts",
    "split|splitting|splits",
    "allow|allowing|allowed|allows",
    "enable|enabling|enabled|enables",
    "disable|disabling|disabled|disables",
    "show|showing|showed|shows|shown",
    "hide|hiding|hid|hides|hidden",
    "speed|speeding|sped|speeds",
    "upgrade|upgrading|upgraded|upgrades",
    "migrate|migrating|migrated|migrates",
    "restore|restoring|restored|restores",
    "revert|reverting|reverted|reverts",
    "tidy|tidying|tidied|tidies",
    "correct|correcting|corrected|corrects",
    "adjust|adjusting|adjusted|adjusts",
    "introduce|introducing|introduced|introduces",
    "drop|dropping|dropped|drops",
    "prevent|preventing|prevented|prevents",
    "avoid|avoiding|avoided|avoids",
    "optimize|optimizing|optimized|optimizes",
    "format|formatting|formatted|formats",
    "rework|reworking|reworked|reworks",
    "expose|exposing|exposed|exposes",
    "wire|wiring|wired|wires",
];

/// The base form when `word` is a known verb form.
fn verb_base(word: &str) -> Option<&'static str> {
    let w = word.to_lowercase();
    VERBS.iter().find_map(|forms| {
        let mut it = forms.split('|');
        let base = it.next()?;
        (base == w || it.any(|f| f == w)).then_some(base)
    })
}

/// Strip `prefix` (case-insensitive, whole words) from the start of `s`.
fn strip_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    (head.eq_ignore_ascii_case(prefix) && s[prefix.len()..].starts_with(' ')).then(|| s[prefix.len()..].trim_start())
}

/// An imperative line where possible: requests lose "please / can you",
/// reports lose "I've", and a leading verb form becomes its base ("Fixed
/// the bug" → "Fix the bug"). Also: true when it starts with a known verb.
pub fn imperative(s: &str) -> (String, bool) {
    const POLITE: &[&str] = &[
        "please", "pls", "can you", "could you", "would you", "will you", "can we", "could we", "let's", "lets", "i want you to",
        "i'd like you to", "i would like you to", "i need you to", "help me", "go ahead and", "ok", "okay", "ok,", "okay,", "now",
        "sure,", "sure!", "sure.", "done.", "done!", "great,", "great!", "great.", "all done.", "all done!",
    ];
    const PRONOUN: &[&str] = &["i've", "i have", "i've now", "i have now", "i", "we've", "we have", "we", "i'll", "i will", "i just", "i've just"];
    let mut t = s.trim();
    loop {
        let before = t;
        for p in POLITE {
            if let Some(rest) = strip_ci(t, p) {
                t = rest;
            }
        }
        if before == t {
            break;
        }
    }
    // "I've fixed…": only when a verb follows ("I think…" stays).
    let mut longest: Option<&str> = None;
    for p in PRONOUN {
        if let Some(rest) = strip_ci(t, p)
            && rest.split_whitespace().next().and_then(verb_base).is_some()
            && longest.is_none_or(|l| rest.len() < l.len())
        {
            longest = Some(rest);
        }
    }
    if let Some(rest) = longest {
        t = rest;
    }
    let t = t.trim_end_matches(|c: char| matches!(c, '.' | '!' | '?' | ',' | ':' | ';' | '…') || c.is_whitespace());
    let mut t = t.to_string();
    for tail in [" please", " thanks", " thank you", " pls"] {
        if t.len() > tail.len() && t.get(t.len() - tail.len()..).is_some_and(|e| e.eq_ignore_ascii_case(tail)) {
            t.truncate(t.len() - tail.len());
        }
    }
    let t = t.trim_end_matches(|c: char| matches!(c, '.' | '!' | '?' | ',') || c.is_whitespace());
    let (first, rest) = t.split_once(' ').unwrap_or((t, ""));
    let (first, verb) = match verb_base(first) {
        Some(b) => (b.to_string(), true),
        None => (first.to_string(), false),
    };
    let joined = if rest.is_empty() { first } else { format!("{first} {rest}") };
    (capitalize(&joined), verb)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// At most `max` characters, cut at a word boundary with "…" when long.
pub fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    let at = cut.rfind(' ').filter(|&i| i >= cut.len() / 2).unwrap_or(cut.len());
    let head = cut[..at].trim_end_matches(|c: char| matches!(c, ',' | ';' | ':' | '.' | '-') || c.is_whitespace());
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(n: u32, files: u32, added: u32, removed: u32, prompt: Option<&str>, reply: Option<&str>) -> TurnNote {
        TurnNote { n, files, added, removed, prompt: prompt.map(Into::into), reply: reply.map(Into::into) }
    }

    fn files(v: &[(&str, u32, u32)]) -> Vec<(PathBuf, u32, u32)> {
        v.iter().map(|(p, a, r)| (PathBuf::from(p), *a, *r)).collect()
    }

    #[test]
    fn imperative_forms() {
        assert_eq!(imperative("can you fix the login bug please?").0, "Fix the login bug");
        assert_eq!(imperative("Fixing the login bug").0, "Fix the login bug");
        assert_eq!(imperative("I've added a dark mode toggle.").0, "Add a dark mode toggle");
        assert_eq!(imperative("Done. Updated the README").0, "Update the README");
        assert_eq!(imperative("I think it's fine").0, "I think it's fine");
        assert!(imperative("Refactored parser").1);
        assert!(!imperative("The parser was slow").1);
        assert_eq!(imperative("dark mode").0, "Dark mode");
    }

    #[test]
    fn clean_and_fit() {
        assert_eq!(clean_line("\n\n## **Fixed** the `parse()` call\nmore"), "Fixed the parse() call");
        assert_eq!(clean_line("✳ Add tests"), "Add tests");
        let long = "Add a very long feature description that goes well beyond the seventy two character limit of git";
        let f = fit(long, SUBJECT_MAX);
        assert!(f.chars().count() <= SUBJECT_MAX, "{f}");
        assert!(f.ends_with('…') && !f.ends_with(" …"), "{f}");
        assert_eq!(fit("short", 72), "short");
    }

    #[test]
    fn subject_from_title_and_body_per_turn() {
        let src = Source {
            title: "add dark mode to the settings screen".into(),
            turns: vec![
                turn(1, 3, 40, 2, Some("add dark mode to the settings screen"), Some("I've added a **Dark mode** toggle. It follows the system.")),
                turn(2, 1, 3, 1, Some("the test fails, fix it"), Some("The test expected the old label: updated it.")),
            ],
            files: files(&[("src/settings.rs", 30, 2), ("src/theme.rs", 10, 0), ("tests/settings.rs", 3, 1)]),
        };
        let m = generate(&src);
        assert_eq!(m.subject, "Add dark mode to the settings screen");
        assert_eq!(
            m.body,
            "- Add a Dark mode toggle (3 files, +40 -2)\n- The test fails, fix it (1 file, +3 -1)\n\n3 files changed, +43 -3"
        );
        assert_eq!(m.full(), format!("{}\n\n{}", m.subject, m.body));
    }

    #[test]
    fn placeholder_title_uses_first_turn_then_files() {
        let src = Source { title: "agent terminal 1".into(), turns: vec![turn(1, 1, 2, 0, Some("please bump the version"), None)], files: files(&[("Cargo.toml", 2, 0)]) };
        let m = generate(&src);
        assert_eq!(m.subject, "Bump the version");
        // One turn repeating the subject: just the count.
        assert_eq!(m.body, "1 file changed, +2 -0");

        let src = Source { title: "agent terminal 2".into(), turns: vec![], files: files(&[("a.rs", 1, 1), ("b.rs", 2, 0)]) };
        let m = generate(&src);
        assert_eq!(m.subject, "Update 2 files");
        assert_eq!(m.body, "- a.rs (+1 -1)\n- b.rs (+2 -0)\n\n2 files changed, +3 -1");

        let src = Source { title: String::new(), turns: vec![], files: files(&[("a.rs", 1, 1)]) };
        assert_eq!(generate(&src), Message { subject: "Update a.rs".into(), body: "1 file changed, +1 -1".into() });
    }

    #[test]
    fn turns_without_chat_and_many_turns() {
        // After a restart the chat is gone: turn records alone give no lines.
        let src = Source { title: "Speed up search".into(), turns: vec![turn(1, 2, 5, 5, None, None)], files: files(&[("a.rs", 3, 3), ("b.rs", 2, 2)]) };
        assert_eq!(generate(&src).body, "- a.rs (+3 -3)\n- b.rs (+2 -2)\n\n2 files changed, +5 -5");

        let turns: Vec<TurnNote> = (1..=10).map(|n| turn(n, 1, 1, 0, Some(&format!("step {n}")), None)).collect();
        let m = generate(&Source { title: "Do things".into(), turns, files: Vec::new() });
        let lines: Vec<&str> = m.body.lines().collect();
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[0], "- Step 1 (1 file, +1 -0)");
        assert_eq!(lines[7], "- and 3 more turns");
        assert!(m.body.lines().all(|l| l.chars().count() <= SUBJECT_MAX));
    }

    #[test]
    fn merge_subject_fits() {
        let m = Message { subject: "Add a very long feature description that goes beyond the limit".into(), body: "x".into() };
        let full = m.merge("qur/add-a-very-long-feature");
        let subject = full.lines().next().unwrap();
        assert!(subject.starts_with("Merge qur/add-a-very-long-feature: Add a very"), "{subject}");
        assert!(subject.chars().count() <= SUBJECT_MAX, "{subject}");
        assert!(full.ends_with("\n\nx"));
        assert_eq!(Message { subject: "Fix".into(), body: String::new() }.merge("qur/fix"), "Merge qur/fix: Fix");
    }
}
