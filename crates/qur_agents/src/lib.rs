//! The agent roster (`agents.toml`) and the per-project task store.
//! Runtime state (processes, chat, plan) lives in the app; this crate is the
//! data that survives restarts.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Login-shell PATH
// ---------------------------------------------------------------------------

/// The user's login-shell PATH. Apps started from the Dock get a minimal
/// PATH, so agent CLIs installed via nvm/homebrew (and `node` for the ACP
/// adapters) would not be found. Read once; blocks ~50-200 ms the first time,
/// so warm it off the UI thread with `warm_login_path()`.
pub fn login_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let out = std::process::Command::new(&shell).args(["-l", "-c", "printf %s \"$PATH\""]).output();
        match out {
            Ok(o) if o.status.success() && !o.stdout.is_empty() => String::from_utf8_lossy(&o.stdout).into_owned(),
            _ => std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into()),
        }
    })
}

pub fn warm_login_path() {
    std::thread::spawn(|| {
        login_path();
    });
}

/// `command -v` against the login PATH.
pub fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let p = PathBuf::from(program);
        return p.is_file().then_some(p);
    }
    login_path().split(':').map(|d| Path::new(d).join(program)).find(|p| p.is_file())
}

// ---------------------------------------------------------------------------
// Roster: agents.toml
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Structured: ACP over stdio.
    Acp,
    /// Raw PTY fallback (also New Agent Terminal).
    Terminal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentDef {
    pub id: String,
    pub label: String,
    pub mode: Mode,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Optional "#RRGGBB" override of the theme's accent slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Roster {
    #[serde(default, rename = "agent")]
    pub agents: Vec<AgentDef>,
}

pub fn roster_path() -> PathBuf {
    qur_config::qur_home().join("agents.toml")
}

const ROSTER_HEADER: &str = "# Agents Qur can start. Written on first launch; edit freely.\n# mode = \"acp\" (structured) or \"terminal\" (PTY). Qur never sets API keys:\n# agents use their own CLI logins.\n\n";

impl Roster {
    /// The built-in roster: Claude Code over ACP (decision 6: no Codex for now).
    pub fn defaults() -> Roster {
        Roster {
            agents: vec![AgentDef {
                id: "claude".into(),
                label: "Claude Code".into(),
                mode: Mode::Acp,
                command: "claude-agent-acp".into(),
                args: vec![],
                accent: None,
            }],
        }
    }

    /// Load `agents.toml`; write the defaults there when it doesn't exist.
    /// A broken file gives the defaults plus an error message for the UI.
    pub fn load() -> (Roster, Option<String>) {
        Self::load_from(&roster_path())
    }

    pub fn load_from(path: &Path) -> (Roster, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(s) => match toml_edit::de::from_str::<Roster>(&s) {
                Ok(r) if !r.agents.is_empty() => (r, None),
                Ok(_) => (Roster::defaults(), Some("agents.toml lists no agents; using Claude Code".into())),
                Err(e) => (Roster::defaults(), Some(format!("agents.toml: {e}"))),
            },
            Err(_) => {
                let r = Roster::defaults();
                if let Ok(body) = toml_edit::ser::to_string_pretty(&r) {
                    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
                    let _ = std::fs::write(path, format!("{ROSTER_HEADER}{body}"));
                }
                (r, None)
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&AgentDef> {
        self.agents.iter().find(|a| a.id == id)
    }
}

// ---------------------------------------------------------------------------
// Tasks: <qur home>/projects/<project-id>.tasks.toml
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskRecord {
    /// Unique within the project; equals the worktree slug.
    pub id: String,
    pub agent: String,
    pub title: String,
    pub branch: String,
    pub worktree: PathBuf,
    /// Commit the task started from.
    pub base: String,
    /// ACP session, for session/load on resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Unix seconds.
    pub created: u64,
    /// "acp" or "terminal" (New Agent Terminal).
    pub mode: Mode,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskStore {
    #[serde(default, rename = "task")]
    pub tasks: Vec<TaskRecord>,
}

pub fn tasks_path(project_id: &str) -> PathBuf {
    qur_config::qur_home().join("projects").join(format!("{project_id}.tasks.toml"))
}

pub fn worktrees_dir(project_id: &str) -> PathBuf {
    qur_config::qur_home().join("worktrees").join(project_id)
}

impl TaskStore {
    /// Tasks whose worktree still exists (others were removed outside Qur).
    pub fn load(project_id: &str) -> TaskStore {
        let mut s: TaskStore = std::fs::read_to_string(tasks_path(project_id))
            .ok()
            .and_then(|s| toml_edit::de::from_str(&s).ok())
            .unwrap_or_default();
        s.tasks.retain(|t| t.worktree.exists());
        s
    }

    pub fn save(&self, project_id: &str) -> std::io::Result<()> {
        let path = tasks_path(project_id);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let body = toml_edit::ser::to_string_pretty(self).map_err(|e| std::io::Error::other(e.to_string()))?;
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        std::fs::write(&tmp, format!("# Managed by Qur.\n{body}"))?;
        std::fs::rename(tmp, path)
    }

    pub fn upsert(&mut self, t: TaskRecord) {
        match self.tasks.iter_mut().find(|x| x.id == t.id) {
            Some(x) => *x = t,
            None => self.tasks.push(t),
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.tasks.retain(|t| t.id != id);
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_defaults_written_and_parsed() {
        let d = std::env::temp_dir().join(format!("qur-agents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("agents.toml");
        let (r, err) = Roster::load_from(&p);
        assert!(err.is_none());
        assert_eq!(r.agents[0].id, "claude");
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("[[agent]]") && text.contains("claude-agent-acp"), "{text}");
        assert_eq!(Roster::load_from(&p).0, r);
        std::fs::write(&p, "[[agent]]\nid = \"aider\"\nlabel = \"Aider\"\nmode = \"terminal\"\ncommand = \"aider\"\naccent = \"#5B9BFF\"\n").unwrap();
        let (r, err) = Roster::load_from(&p);
        assert!(err.is_none());
        assert_eq!((r.agents[0].mode, r.agents[0].accent.as_deref()), (Mode::Terminal, Some("#5B9BFF")));
        std::fs::write(&p, "[[agent]\n").unwrap();
        assert!(Roster::load_from(&p).1.is_some());
    }

    #[test]
    fn task_store_round_trip() {
        let d = std::env::temp_dir().join(format!("qur-tasks-{}", std::process::id()));
        let wt = d.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        let mut s = TaskStore::default();
        s.upsert(TaskRecord {
            id: "fix-nav".into(),
            agent: "claude".into(),
            title: "Fix nav".into(),
            branch: "qur/claude/fix-nav".into(),
            worktree: wt.clone(),
            base: "abc".into(),
            session_id: Some("s1".into()),
            created: 1,
            mode: Mode::Acp,
        });
        let body = toml_edit::ser::to_string_pretty(&s).unwrap();
        let back: TaskStore = toml_edit::de::from_str(&body).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn which_finds_sh() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-command-qur").is_none());
    }
}
