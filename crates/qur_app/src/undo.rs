//! Undo/redo for the text fields (TextInput, PromptInput). Whole-state
//! snapshots (the fields hold a few KB at most), taken before each edit.
//! Consecutive typing or erasing at the caret joins one step until a pause
//! or a caret jump, like the editor's `history.rs`.

use std::time::{Duration, Instant};

/// Typing within this window of the last keystroke joins the same step.
pub const GROUP_WINDOW: Duration = Duration::from_secs(1);
const MAX_STEPS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One character typed at the caret.
    Type,
    /// One character or word erased at the caret.
    Erase,
    /// Paste, newline, mention, cut, replace-all: always its own step.
    Other,
}

pub struct History<S> {
    undo: Vec<S>,
    redo: Vec<S>,
    /// The open group: its kind, the caret it expects next, and when it was last extended.
    group: Option<(Kind, usize, Instant)>,
}

impl<S> Default for History<S> {
    fn default() -> Self {
        History { undo: Vec::new(), redo: Vec::new(), group: None }
    }
}

impl<S> History<S> {
    /// Call before an edit that moves the caret `from` → `to`. `before` is
    /// the state to return to; it's dropped when the edit joins the open group.
    pub fn record(&mut self, before: S, kind: Kind, from: usize, to: usize, now: Instant) {
        self.redo.clear();
        let joins = kind != Kind::Other
            && matches!(self.group, Some((k, at, t)) if k == kind && at == from && now.saturating_duration_since(t) < GROUP_WINDOW);
        if !joins {
            self.undo.push(before);
            if self.undo.len() > MAX_STEPS {
                self.undo.remove(0);
            }
        }
        self.group = (kind != Kind::Other).then_some((kind, to, now));
    }

    /// The next edit starts a new step (a caret move).
    pub fn seal(&mut self) {
        self.group = None;
    }

    /// The state to restore, given the current one (kept for redo).
    pub fn undo(&mut self, current: S) -> Option<S> {
        let s = self.undo.pop()?;
        self.redo.push(current);
        self.group = None;
        Some(s)
    }

    pub fn redo(&mut self, current: S) -> Option<S> {
        let s = self.redo.pop()?;
        self.undo.push(current);
        self.group = None;
        Some(s)
    }
}

/// The step inserting `new` makes: one typed character groups (unless it's
/// a newline or replaces a selection); anything else is its own step.
pub fn insert_kind(new: &str, replaces_all: bool) -> Kind {
    let mut cs = new.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) if !replaces_all && c != '\n' => Kind::Type,
        _ => Kind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field: text + caret, editing through History like the real ones.
    struct Field {
        text: String,
        caret: usize,
        h: History<(String, usize)>,
        now: Instant,
    }

    impl Field {
        fn new() -> Field {
            Field { text: String::new(), caret: 0, h: History::default(), now: Instant::now() }
        }
        fn insert(&mut self, s: &str) {
            let to = self.caret + s.len();
            self.h.record((self.text.clone(), self.caret), insert_kind(s, false), self.caret, to, self.now);
            self.text.insert_str(self.caret, s);
            self.caret = to;
        }
        fn type_str(&mut self, s: &str) {
            for c in s.chars() {
                self.insert(&c.to_string());
                self.now += Duration::from_millis(100);
            }
        }
        fn backspace(&mut self) {
            let p = self.text[..self.caret].char_indices().last().map(|(i, _)| i).unwrap_or(0);
            self.h.record((self.text.clone(), self.caret), Kind::Erase, self.caret, p, self.now);
            self.text.replace_range(p..self.caret, "");
            self.caret = p;
        }
        fn undo(&mut self) -> bool {
            let cur = (self.text.clone(), self.caret);
            self.h.undo(cur).map(|(t, c)| (self.text, self.caret) = (t, c)).is_some()
        }
        fn redo(&mut self) -> bool {
            let cur = (self.text.clone(), self.caret);
            self.h.redo(cur).map(|(t, c)| (self.text, self.caret) = (t, c)).is_some()
        }
    }

    #[test]
    fn typing_coalesces_into_one_step() {
        let mut f = Field::new();
        f.type_str("hello world");
        assert_eq!(f.h.undo.len(), 1);
        assert!(f.undo());
        assert_eq!((f.text.as_str(), f.caret), ("", 0));
        // Redo brings it all back.
        assert!(f.redo());
        assert_eq!(f.text, "hello world");
    }

    #[test]
    fn newline_is_its_own_step() {
        let mut f = Field::new();
        f.type_str("ab\ncd");
        assert!(f.undo());
        assert_eq!(f.text, "ab\n");
        assert!(f.undo());
        assert_eq!(f.text, "ab");
    }

    #[test]
    fn pause_ends_a_step() {
        let mut f = Field::new();
        f.type_str("ab");
        f.now += GROUP_WINDOW * 2;
        f.type_str("cd");
        assert!(f.undo());
        assert_eq!(f.text, "ab");
    }

    #[test]
    fn caret_jump_ends_a_step() {
        let mut f = Field::new();
        f.type_str("ab");
        f.caret = 0;
        f.type_str("x");
        assert_eq!(f.text, "xab");
        assert!(f.undo());
        assert_eq!((f.text.as_str(), f.caret), ("ab", 0));
        // A move that lands where the group expects still ends it once sealed.
        f.caret = 2;
        f.type_str("c");
        f.h.seal();
        f.type_str("d");
        assert!(f.undo());
        assert_eq!(f.text, "abc");
    }

    #[test]
    fn paste_is_one_step() {
        let mut f = Field::new();
        f.type_str("go");
        f.insert("long text");
        f.type_str("yz");
        assert!(f.undo());
        assert_eq!(f.text, "golong text");
        assert!(f.undo());
        assert_eq!(f.text, "go");
    }

    #[test]
    fn erasing_groups_but_not_with_typing() {
        let mut f = Field::new();
        f.type_str("abc");
        f.backspace();
        f.backspace();
        assert_eq!(f.text, "a");
        assert!(f.undo());
        assert_eq!(f.text, "abc");
        assert!(f.undo());
        assert_eq!(f.text, "");
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut f = Field::new();
        f.type_str("one");
        f.undo();
        f.type_str("x");
        assert!(!f.redo());
        assert!(f.undo());
        assert_eq!(f.text, "");
    }

    #[test]
    fn replacing_a_selection_is_its_own_step() {
        assert_eq!(insert_kind("a", false), Kind::Type);
        assert_eq!(insert_kind("a", true), Kind::Other);
        assert_eq!(insert_kind("\n", false), Kind::Other);
        assert_eq!(insert_kind("ab", false), Kind::Other);
    }

    #[test]
    fn capped() {
        let mut h = History::default();
        let t = Instant::now();
        for i in 0..MAX_STEPS + 50 {
            h.record(i, Kind::Other, 0, 0, t);
        }
        let mut n = 0;
        while h.undo(0).is_some() {
            n += 1;
        }
        assert_eq!(n, MAX_STEPS);
    }
}
