//! Qur's settings files, all under `~/Library/Application Support/Qur/`
//! (`QUR_HOME` overrides it, for tests and spikes):
//!
//! - `config.toml`: the user's settings. Hand-editable; Qur only rewrites the
//!   keys it changes and keeps comments and formatting (toml_edit).
//! - `projects/<project-id>.toml`: per-project overrides (decision 13).
//! - `state.toml`: machine-managed state (recent projects). Not for editing.
//!
//! Nothing is ever written into the user's repo.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

pub const THEME_AUTO: &str = "Auto";
pub const MIN_ZOOM: f32 = 0.6;
pub const MAX_ZOOM: f32 = 2.0;

pub fn qur_home() -> PathBuf {
    if let Some(p) = std::env::var_os("QUR_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library/Application Support/Qur")
}

pub fn config_path() -> PathBuf {
    qur_home().join("config.toml")
}

// ---------------------------------------------------------------------------
// config.toml
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Theme name, or "Auto" (follow macOS: Qur Light / Qur Dark).
    pub theme: String,
    /// Agent stage layout: "A" watch + side notes, "B" one view at a time,
    /// "C" watch + drawer (decision 25).
    pub peek_layout: String,
    pub window: WindowConfig,
    pub editor: EditorConfig,
    pub ui: UiConfig,
    pub panels: PanelsConfig,
    pub keys: KeysConfig,
    pub alerts: AlertsConfig,
    pub layout: LayoutConfig,
}

/// The agent stage's arrangement (dock.rs), written by Qur as you drag tabs
/// and dividers. Empty = the `peek_layout` preset.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct LayoutConfig {
    pub acp: Option<String>,
    pub terminal: Option<String>,
}

/// When an agent finishes, errors or needs you while you're elsewhere
/// (handoff section 11, M6).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct AlertsConfig {
    /// macOS notification (needs the bundled app; dev builds skip it).
    pub notify: bool,
    /// Dock badge with the number of agents that need you.
    pub badge: bool,
    /// A short, soft chime.
    pub sound: bool,
}

impl Default for AlertsConfig {
    fn default() -> Self {
        AlertsConfig { notify: true, badge: true, sound: true }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct WindowConfig {
    /// 1.0 = opaque. Clamped to the theme crate's minimum (decision 8).
    pub opacity: f32,
    pub blur: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct EditorConfig {
    /// Empty = system monospace (SF Mono, else Menlo).
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub soft_wrap: bool,
    pub tab_width: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiConfig {
    pub font_size: f32,
    pub zoom: f32,
    /// Which of the theme's six accents comes first (agents take them in
    /// order from here): 0 = the theme's own first (Qur's orange).
    pub accent: usize,
    /// Show the Keep Awake switch at the top of the sidebar.
    pub keep_awake: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct PanelsConfig {
    pub terminal: bool,
    pub git: bool,
    pub search: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct KeysConfig {
    pub vim: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: THEME_AUTO.into(),
            peek_layout: "A".into(),
            window: WindowConfig::default(),
            editor: EditorConfig::default(),
            ui: UiConfig::default(),
            panels: PanelsConfig::default(),
            keys: KeysConfig::default(),
            alerts: AlertsConfig::default(),
            layout: LayoutConfig::default(),
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        WindowConfig { opacity: 1.0, blur: true }
    }
}

impl Default for EditorConfig {
    fn default() -> Self {
        EditorConfig { font_family: String::new(), font_size: 13.0, line_height: 1.5, soft_wrap: true, tab_width: 4 }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig { font_size: 13.0, zoom: 1.0, accent: 0, keep_awake: true }
    }
}

impl Default for PanelsConfig {
    fn default() -> Self {
        PanelsConfig { terminal: true, git: true, search: true }
    }
}

impl Config {
    /// Loads `config.toml`; a missing file gives defaults, a broken one gives
    /// defaults plus an error message for the UI (never a crash).
    pub fn load() -> (Config, Option<String>) {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> (Config, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(s) => match toml_edit::de::from_str::<Config>(&s) {
                Ok(c) => (c.sanitized(), None),
                Err(e) => (Config::default(), Some(format!("config.toml: {e}"))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config::default(), None),
            Err(e) => (Config::default(), Some(format!("config.toml: {e}"))),
        }
    }

    fn sanitized(mut self) -> Config {
        self.ui.zoom = self.ui.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.ui.accent %= 6;
        self.editor.font_size = self.editor.font_size.clamp(6.0, 72.0);
        self.editor.line_height = self.editor.line_height.clamp(1.0, 3.0);
        self.editor.tab_width = self.editor.tab_width.clamp(1, 16);
        self.window.opacity = self.window.opacity.clamp(0.0, 1.0);
        if self.theme.trim().is_empty() {
            self.theme = THEME_AUTO.into();
        }
        if !matches!(self.peek_layout.as_str(), "A" | "B" | "C") {
            self.peek_layout = "A".into();
        }
        self
    }
}

/// A value Qur writes back into a TOML file.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Float(f64),
    Bool(bool),
    Int(i64),
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.into())
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Self {
        // Round so 0.85f32 is written as 0.85, not 0.8500000238418579.
        Value::Float((v as f64 * 1000.0).round() / 1000.0)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

/// Set (`Some`) or remove (`None`) `key` (dotted: "window.opacity") in a
/// TOML file, keeping everything else byte-for-byte. Creates the file and
/// parent folders when missing. Writes atomically.
pub fn write_key(path: &Path, key: &str, value: Option<Value>) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    let parts: Vec<&str> = key.split('.').collect();
    let (last, tables) = parts.split_last().expect("non-empty key");
    let mut table = doc.as_table_mut();
    for t in tables {
        if !table.contains_key(t) {
            if value.is_none() {
                return Ok(());
            }
            table.insert(t, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        table = table[*t]
            .as_table_mut()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("`{t}` is not a table")))?;
    }
    match value {
        Some(v) => {
            let v = match v {
                Value::Str(s) => toml_edit::value(s),
                Value::Float(f) => toml_edit::value(f),
                Value::Bool(b) => toml_edit::value(b),
                Value::Int(i) => toml_edit::value(i),
            };
            // Keep any comment/decor attached to an existing value.
            match table.get_mut(last).and_then(|i| i.as_value_mut()) {
                Some(old) => {
                    let decor = old.decor().clone();
                    *old = v.into_value().expect("value");
                    *old.decor_mut() = decor;
                }
                None => {
                    table.insert(last, v);
                }
            }
        }
        None => {
            table.remove(last);
        }
    }
    write_atomic(path, doc.to_string().as_bytes())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

/// Stable id for a project folder. For a git repo: the first 12 hex chars of
/// its root commit (same across clones, moves and renames; also used for the
/// worktrees folder). Otherwise: `path-` + a hash of the absolute path.
pub fn project_id(root: &Path) -> String {
    let out = Command::new("git").arg("-C").arg(root).args(["rev-list", "--max-parents=0", "HEAD"]).output();
    if let Ok(out) = out
        && out.status.success()
    {
        // Several roots (merged histories): take the oldest-listed last line
        // deterministically by sorting.
        let mut roots: Vec<&str> = std::str::from_utf8(&out.stdout).unwrap_or("").lines().collect();
        roots.sort_unstable();
        if let Some(first) = roots.first().filter(|r| r.len() >= 12) {
            return first[..12].to_string();
        }
    }
    let abs = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    format!("path-{:016x}", fnv1a(abs.to_string_lossy().as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Per-project overrides (`projects/<id>.toml`). `None` = use the global value.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectSettings {
    pub theme: Option<String>,
    pub window: ProjectWindow,
    pub worktree: ProjectWorktree,
    pub checks: ProjectChecks,
    pub run: ProjectRun,
}

/// Run Project: the dev server for a task's worktree (decision 29).
/// `$PORT` in `url` is the free port Qur picks per task; the command gets it
/// as the `PORT` environment variable (and `$PORT` works in it too).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectRun {
    /// e.g. "pnpm dev --port $PORT".
    pub command: Option<String>,
    /// e.g. "http://localhost:$PORT". Only localhost URLs are opened.
    pub url: Option<String>,
}

/// Turn-end checks: run after an agent turn that changed files (OpenADE /
/// PandaCode research, decision 29).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectChecks {
    /// e.g. "cargo check" or "pnpm test --run". None = no checks.
    pub command: Option<String>,
    /// Seconds before the check is stopped and counted as failed.
    pub timeout: u64,
    /// Send a failure to the agent right away instead of prefilling the prompt.
    pub auto_send: bool,
    /// Also run after an Agent Terminal goes quiet having changed files (it
    /// has no real turns, so this is a guess; off by default).
    pub terminal: bool,
}

impl Default for ProjectChecks {
    fn default() -> Self {
        ProjectChecks { command: None, timeout: 300, auto_send: false, terminal: false }
    }
}

/// New task worktrees (decision 22).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectWorktree {
    /// Run in each new worktree before the agent starts, e.g. "pnpm install".
    pub setup: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectWindow {
    pub opacity: Option<f32>,
    pub blur: Option<bool>,
}

pub fn project_settings_path(id: &str) -> PathBuf {
    qur_home().join("projects").join(format!("{id}.toml"))
}

impl ProjectSettings {
    pub fn load(id: &str) -> ProjectSettings {
        std::fs::read_to_string(project_settings_path(id))
            .ok()
            .and_then(|s| toml_edit::de::from_str(&s).ok())
            .unwrap_or_default()
    }
}

/// Global settings with a project's overrides applied.
#[derive(Clone, Debug, PartialEq)]
pub struct Effective {
    pub theme: String,
    pub opacity: f32,
    pub blur: bool,
    /// Which of the above come from the project file (for the picker's scope toggle).
    pub theme_is_project: bool,
    pub window_is_project: bool,
}

pub fn effective(global: &Config, project: &ProjectSettings) -> Effective {
    Effective {
        theme: project.theme.clone().unwrap_or_else(|| global.theme.clone()),
        opacity: project.window.opacity.unwrap_or(global.window.opacity),
        blur: project.window.blur.unwrap_or(global.window.blur),
        theme_is_project: project.theme.is_some(),
        window_is_project: project.window.opacity.is_some() || project.window.blur.is_some(),
    }
}

// ---------------------------------------------------------------------------
// state.toml (machine-managed)
// ---------------------------------------------------------------------------

const MAX_RECENT: usize = 12;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct State {
    /// Most recent first.
    pub recent_projects: Vec<PathBuf>,
    /// The first-run picker was shown (or this home predates it).
    pub setup_done: bool,
}

pub fn state_path() -> PathBuf {
    qur_home().join("state.toml")
}

impl State {
    pub fn load() -> State {
        std::fs::read_to_string(state_path())
            .ok()
            .and_then(|s| toml_edit::de::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let mut doc = toml_edit::DocumentMut::new();
        let mut arr = toml_edit::Array::new();
        for p in &self.recent_projects {
            arr.push(p.to_string_lossy().as_ref());
        }
        doc["recent_projects"] = toml_edit::value(arr);
        if self.setup_done {
            doc["setup_done"] = toml_edit::value(true);
        }
        let text = format!("# Managed by Qur. Edit config.toml instead.\n{doc}");
        write_atomic(&state_path(), text.as_bytes())
    }

    pub fn touch_project(&mut self, root: &Path) {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        self.recent_projects.retain(|p| p != &root);
        self.recent_projects.insert(0, root);
        self.recent_projects.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qur-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn defaults_when_missing() {
        let (c, err) = Config::load_from(&tmp("missing").join("config.toml"));
        assert_eq!(c, Config::default());
        assert!(err.is_none());
        assert_eq!(c.theme, "Auto");
        assert!(c.editor.soft_wrap);
    }

    #[test]
    fn partial_file_and_errors() {
        let d = tmp("partial");
        let p = d.join("config.toml");
        std::fs::write(&p, "theme = \"Nord\"\n[window]\nopacity = 0.85\n").unwrap();
        let (c, err) = Config::load_from(&p);
        assert!(err.is_none());
        assert_eq!(c.theme, "Nord");
        assert_eq!(c.window.opacity, 0.85);
        assert!(c.window.blur);
        std::fs::write(&p, "theme = [").unwrap();
        let (c, err) = Config::load_from(&p);
        assert_eq!(c, Config::default());
        assert!(err.unwrap().contains("config.toml"));
    }

    #[test]
    fn write_key_keeps_comments() {
        let d = tmp("write");
        let p = d.join("config.toml");
        std::fs::write(&p, "# my settings\ntheme = \"Nord\" # cold\n\n[editor]\nfont_size = 14 # big\n").unwrap();
        write_key(&p, "theme", Some("Gruvbox Dark".into())).unwrap();
        write_key(&p, "window.opacity", Some(0.85f32.into())).unwrap();
        write_key(&p, "editor.soft_wrap", Some(false.into())).unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.starts_with("# my settings\n"), "{s}");
        assert!(s.contains("theme = \"Gruvbox Dark\" # cold"), "{s}");
        assert!(s.contains("font_size = 14 # big"), "{s}");
        assert!(s.contains("opacity = 0.85"), "{s}");
        let (c, _) = Config::load_from(&p);
        assert_eq!(c.theme, "Gruvbox Dark");
        assert!(!c.editor.soft_wrap);
        write_key(&p, "theme", None).unwrap();
        assert_eq!(Config::load_from(&p).0.theme, "Auto");
        write_key(&p, "nope.key", None).unwrap();
    }

    #[test]
    fn project_overrides() {
        let global = Config::default();
        let mut proj = ProjectSettings::default();
        let e = effective(&global, &proj);
        assert_eq!((e.theme.as_str(), e.opacity, e.theme_is_project), ("Auto", 1.0, false));
        proj.theme = Some("Tokyo Night".into());
        proj.window.opacity = Some(0.9);
        let e = effective(&global, &proj);
        assert_eq!((e.theme.as_str(), e.opacity, e.blur), ("Tokyo Night", 0.9, true));
        assert!(e.theme_is_project && e.window_is_project);
    }

    #[test]
    fn project_checks_parse() {
        let p: ProjectSettings = toml_edit::de::from_str("[checks]\ncommand = \"cargo check\"\n").unwrap();
        assert_eq!(p.checks.command.as_deref(), Some("cargo check"));
        assert_eq!((p.checks.timeout, p.checks.auto_send), (300, false));
        let p: ProjectSettings = toml_edit::de::from_str("[checks]\ncommand = \"make\"\ntimeout = 20\nauto_send = true\n").unwrap();
        assert_eq!((p.checks.timeout, p.checks.auto_send), (20, true));
        assert_eq!(ProjectSettings::default().checks.command, None);
        let p: ProjectSettings = toml_edit::de::from_str("[run]\ncommand = \"pnpm dev --port $PORT\"\nurl = \"http://localhost:$PORT\"\n").unwrap();
        assert_eq!(p.run.url.as_deref(), Some("http://localhost:$PORT"));
    }

    #[test]
    fn project_id_git_and_plain() {
        let d = tmp("pid");
        let plain = project_id(&d);
        assert!(plain.starts_with("path-"));
        assert_eq!(plain, project_id(&d));
        let git = |args: &[&str]| {
            assert!(Command::new("git").arg("-C").arg(&d).args(args).status().unwrap().success());
        };
        git(&["init", "-q"]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "root"]);
        let id = project_id(&d);
        assert_eq!(id.len(), 12);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn recent_projects_order() {
        let mut s = State::default();
        let a = tmp("ra");
        let b = tmp("rb");
        s.touch_project(&a);
        s.touch_project(&b);
        s.touch_project(&a);
        assert_eq!(s.recent_projects.len(), 2);
        assert_eq!(s.recent_projects[0], std::fs::canonicalize(&a).unwrap());
    }
}
