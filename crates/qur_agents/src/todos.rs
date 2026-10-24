//! A terminal Claude's to-do list, read from its session transcript
//! (`~/.claude/projects/<cwd slug>/<session>.jsonl`). Claude keeps it with
//! `TodoWrite` (the whole list each time) or `TaskCreate` / `TaskUpdate` (one
//! task at a time); the ACP adapter turns the same calls into its plan.
//! Read incrementally: only lines appended since the last look are parsed.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    pub content: String,
    pub status: TodoStatus,
}

fn status(s: &str) -> Option<TodoStatus> {
    match s {
        "pending" => Some(TodoStatus::Pending),
        "in_progress" => Some(TodoStatus::InProgress),
        "completed" => Some(TodoStatus::Completed),
        _ => None,
    }
}

/// Where Claude keeps transcripts for sessions run in `cwd`.
pub fn claude_project_dir(cwd: &Path) -> PathBuf {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".claude"));
    let slug: String = cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    base.join("projects").join(slug)
}

/// Follows the newest transcript in one folder.
#[derive(Default)]
pub struct TodoTracker {
    file: Option<PathBuf>,
    offset: u64,
    /// Bytes after the last newline, waiting for the rest of the line.
    partial: Vec<u8>,
    /// TaskCreate order → id ("1", "2", …), as Claude numbers them.
    tasks: Vec<(String, Todo)>,
    /// TaskCreate tool_use id → its task's index, to pick up the id from the result.
    created: HashMap<String, usize>,
    pub todos: Vec<Todo>,
}

impl TodoTracker {
    /// Read what's new in `dir`'s newest transcript. True when the list changed.
    pub fn refresh(&mut self, dir: &Path) -> bool {
        let newest = std::fs::read_dir(dir).ok().and_then(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
                .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
                .max()
                .map(|(_, p)| p)
        });
        let Some(path) = newest else { return false };
        if self.file.as_ref() != Some(&path) {
            // A new session (or /clear): start its list from scratch.
            let had = !self.todos.is_empty();
            *self = TodoTracker { file: Some(path.clone()), ..Default::default() };
            return self.read(&path) || had;
        }
        self.read(&path)
    }

    fn read(&mut self, path: &Path) -> bool {
        let Ok(mut f) = std::fs::File::open(path) else { return false };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            // Rewritten: read it again.
            self.offset = 0;
            self.partial.clear();
        }
        if len == self.offset || f.seek(SeekFrom::Start(self.offset)).is_err() {
            return false;
        }
        let mut buf = std::mem::take(&mut self.partial);
        if f.read_to_end(&mut buf).is_err() {
            return false;
        }
        self.offset = len;
        let end = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        self.partial = buf.split_off(end);
        let before = self.todos.clone();
        for line in buf.split(|&b| b == b'\n') {
            if let Ok(v) = serde_json::from_slice::<Value>(line) {
                self.line(&v);
            }
        }
        self.todos != before
    }

    fn line(&mut self, v: &Value) {
        let Some(content) = v.pointer("/message/content").and_then(Value::as_array) else { return };
        for c in content {
            match c.get("type").and_then(Value::as_str) {
                Some("tool_use") => self.tool_use(c),
                Some("tool_result") => self.tool_result(c),
                _ => {}
            }
        }
    }

    fn tool_use(&mut self, c: &Value) {
        let input = c.get("input").unwrap_or(&Value::Null);
        let s = |k: &str| input.get(k).and_then(Value::as_str);
        match c.get("name").and_then(Value::as_str) {
            Some("TodoWrite") => {
                let Some(list) = input.get("todos").and_then(Value::as_array) else { return };
                self.todos = list
                    .iter()
                    .filter_map(|t| {
                        let content = t.get("content").and_then(Value::as_str)?.to_string();
                        Some(Todo { content, status: t.get("status").and_then(Value::as_str).and_then(status)? })
                    })
                    .collect();
            }
            Some("TaskCreate") => {
                let Some(subject) = s("subject").or(s("description")) else { return };
                let id = (self.tasks.len() + 1).to_string();
                self.tasks.push((id, Todo { content: subject.to_string(), status: TodoStatus::Pending }));
                if let Some(use_id) = c.get("id").and_then(Value::as_str) {
                    self.created.insert(use_id.to_string(), self.tasks.len() - 1);
                }
                self.sync_tasks();
            }
            Some("TaskUpdate") => {
                let Some(id) = input.get("taskId").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())) else { return };
                let Some(ix) = self.tasks.iter().position(|(i, _)| *i == id) else { return };
                if s("status") == Some("deleted") {
                    self.tasks.remove(ix);
                } else {
                    let t = &mut self.tasks[ix].1;
                    if let Some(st) = s("status").and_then(status) {
                        t.status = st;
                    }
                    if let Some(subject) = s("subject") {
                        t.content = subject.to_string();
                    }
                }
                self.sync_tasks();
            }
            _ => {}
        }
    }

    /// "Task #7 created successfully: …": use Claude's id, not our count.
    fn tool_result(&mut self, c: &Value) {
        let Some(ix) = c.get("tool_use_id").and_then(Value::as_str).and_then(|u| self.created.remove(u)) else { return };
        let text = match c.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(a)) => a.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect(),
            _ => return,
        };
        if let Some(rest) = text.split("Task #").nth(1) {
            let id: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if !id.is_empty()
                && let Some(t) = self.tasks.get_mut(ix)
            {
                t.0 = id;
            }
        }
    }

    fn sync_tasks(&mut self) {
        self.todos = self.tasks.iter().map(|(_, t)| t.clone()).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn append(path: &Path, s: &str) {
        std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(s.as_bytes()).unwrap();
    }

    #[test]
    fn slug_matches_claude() {
        let d = claude_project_dir(Path::new("/Users/y/Library/Application Support/Qur/worktrees/ab/fix-it"));
        assert!(d.ends_with("projects/-Users-y-Library-Application-Support-Qur-worktrees-ab-fix-it"));
    }

    #[test]
    fn todowrite_and_tasks() {
        let dir = std::env::temp_dir().join(format!("qur-todos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s.jsonl");
        let mut t = TodoTracker::default();
        assert!(!t.refresh(&dir));

        append(&f, r#"{"message":{"content":[{"type":"tool_use","id":"a","name":"TodoWrite","input":{"todos":[{"content":"Read","status":"completed"},{"content":"Fix","status":"in_progress"}]}}]}}"#);
        append(&f, "\n");
        assert!(t.refresh(&dir));
        assert_eq!(t.todos.len(), 2);
        assert_eq!(t.todos[1], Todo { content: "Fix".into(), status: TodoStatus::InProgress });

        // Half a line: wait for the rest.
        let line = r#"{"message":{"content":[{"type":"tool_use","id":"b","name":"TaskCreate","input":{"subject":"Ship"}}]}}"#;
        append(&f, &line[..20]);
        assert!(!t.refresh(&dir));
        append(&f, &line[20..]);
        append(&f, "\n");
        append(&f, r#"{"message":{"content":[{"type":"tool_result","tool_use_id":"b","content":"Task #4 created successfully: Ship"}]}}"#);
        append(&f, "\n");
        append(&f, r#"{"message":{"content":[{"type":"tool_use","id":"c","name":"TaskUpdate","input":{"taskId":"4","status":"completed"}}]}}"#);
        append(&f, "\n");
        assert!(t.refresh(&dir));
        assert_eq!(t.todos, vec![Todo { content: "Ship".into(), status: TodoStatus::Completed }]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
