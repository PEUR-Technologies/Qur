//! Qur's settings files, all under `~/Library/Application Support/Qur/`
//! (`QUR_HOME` overrides it, for tests and spikes):
//!
//! - `config.toml`: the user's settings. Hand-editable; Qur only rewrites the
//!   keys it changes and keeps comments and formatting (toml_edit).
//! - `projects/<project-id>.toml`: per-project overrides (decision 13).
//! - `projects/<project-id>.state.toml`: machine-managed (open files).
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
    /// Zen mode: hide the top bar and traffic lights until the mouse
    /// reaches the top edge of the window.
    pub zen: bool,
    /// Full screen's traffic lights: "auto" (grey in dark themes, colored in
    /// light ones), "color" or "mono".
    pub traffic_lights: String,
    /// Turns every accent this many degrees around the color wheel (-180..180).
    pub accent_hue: f32,
    /// Makes every accent lighter (toward 1) or darker (toward -1).
    pub accent_brightness: f32,
    /// Window gaps (Hyprland's look): every region floats as its own tile
    /// with `gap` pixels of desktop around it.
    pub gaps: bool,
    /// The space between tiles and around the window's edge, in points
    /// before zoom (`MIN_GAP..=MAX_GAP`).
    pub gap: f32,
    /// Round the tiles' corners (only while gaps are on).
    pub rounded: bool,
    /// The tiles' corner radius in points before zoom (`0..=MAX_RADIUS`).
    pub radius: f32,
    /// A hairline border around each tile.
    pub tile_border: bool,
    /// The sidebar: "show" (always), "auto" (hidden until the mouse reaches
    /// its edge of the window) or "hidden" (until `Cmd+B`).
    pub sidebar: String,
    /// Which side of the window the sidebar is on: "left" or "right".
    pub sidebar_side: String,
    /// The sidebar's sections, top to bottom (`SIDEBAR_SECTIONS`, any order).
    pub sidebar_sections: Vec<String>,
}

/// The sidebar's sections in their default order (`[ui] sidebar_sections`).
pub const SIDEBAR_SECTIONS: [&str; 4] = ["agents", "terminals", "git", "files"];

/// Window gaps' slider range (`[ui] gap`).
pub const MIN_GAP: f32 = 2.0;
pub const MAX_GAP: f32 = 24.0;
pub const DEFAULT_GAP: f32 = 8.0;
/// The tiles' corner radius range (`[ui] radius`).
pub const MAX_RADIUS: f32 = 16.0;
pub const DEFAULT_RADIUS: f32 = 8.0;

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
        UiConfig {
            font_size: 13.0,
            zoom: 1.0,
            accent: 0,
            keep_awake: true,
            zen: true,
            traffic_lights: "auto".into(),
            accent_hue: 0.0,
            accent_brightness: 0.0,
            gaps: false,
            gap: DEFAULT_GAP,
            rounded: true,
            radius: DEFAULT_RADIUS,
            tile_border: false,
            sidebar: "show".into(),
            sidebar_side: "left".into(),
            sidebar_sections: SIDEBAR_SECTIONS.map(String::from).to_vec(),
        }
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
        self.ui.gap = if self.ui.gap.is_finite() { self.ui.gap.clamp(MIN_GAP, MAX_GAP) } else { DEFAULT_GAP };
        self.ui.radius = if self.ui.radius.is_finite() { self.ui.radius.clamp(0.0, MAX_RADIUS) } else { DEFAULT_RADIUS };
        self.editor.font_size = self.editor.font_size.clamp(6.0, 72.0);
        self.editor.line_height = self.editor.line_height.clamp(1.0, 3.0);
        self.editor.tab_width = self.editor.tab_width.clamp(1, 16);
        self.window.opacity = self.window.opacity.clamp(0.0, 1.0);
        if !matches!(self.ui.sidebar.as_str(), "show" | "auto" | "hidden") {
            self.ui.sidebar = "show".into();
        }
        if self.ui.sidebar_side != "right" {
            self.ui.sidebar_side = "left".into();
        }
        // Known sections once each, in the order given; any left out go last.
        let mut sections: Vec<String> = Vec::new();
        for s in self.ui.sidebar_sections.iter().chain(SIDEBAR_SECTIONS.map(String::from).iter()) {
            if SIDEBAR_SECTIONS.contains(&s.as_str()) && !sections.contains(s) {
                sections.push(s.clone());
            }
        }
        self.ui.sidebar_sections = sections;
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
    List(Vec<String>),
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
                Value::List(l) => toml_edit::value(l.into_iter().collect::<toml_edit::Array>()),
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
    /// The newest crash report (`crash-*.txt` in the logs folder) the user
    /// has seen the "quit unexpectedly" notice for.
    pub crash_seen: String,
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
        if !self.crash_seen.is_empty() {
            doc["crash_seen"] = toml_edit::value(&self.crash_seen);
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

// ---------------------------------------------------------------------------
// projects/<project-id>.state.toml (machine-managed)
// ---------------------------------------------------------------------------

/// Files restored with the project: more than this only slows opening.
pub const MAX_OPEN_FILES: usize = 20;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectState {
    /// Open files, most recent first.
    #[serde(rename = "file")]
    pub files: Vec<OpenFile>,
    /// The code view was showing the first file.
    pub code_open: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct OpenFile {
    pub path: PathBuf,
    /// Cursor, 0-based.
    pub line: usize,
    pub col: usize,
}

pub fn project_state_path(id: &str) -> PathBuf {
    qur_home().join("projects").join(format!("{id}.state.toml"))
}

impl ProjectState {
    pub fn load(id: &str) -> ProjectState {
        std::fs::read_to_string(project_state_path(id))
            .ok()
            .and_then(|s| toml_edit::de::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, id: &str) -> std::io::Result<()> {
        write_atomic(&project_state_path(id), self.to_toml().as_bytes())
    }

    fn to_toml(&self) -> String {
        let mut doc = toml_edit::DocumentMut::new();
        if self.code_open {
            doc["code_open"] = toml_edit::value(true);
        }
        let mut files = toml_edit::ArrayOfTables::new();
        for f in self.files.iter().take(MAX_OPEN_FILES) {
            let mut t = toml_edit::Table::new();
            t["path"] = toml_edit::value(f.path.to_string_lossy().as_ref());
            t["line"] = toml_edit::value(f.line as i64);
            t["col"] = toml_edit::value(f.col as i64);
            files.push(t);
        }
        doc["file"] = toml_edit::Item::ArrayOfTables(files);
        format!("# Managed by Qur.\n{doc}")
    }
}

// ---------------------------------------------------------------------------
// keymap.toml
// ---------------------------------------------------------------------------

pub fn keymap_path() -> PathBuf {
    qur_home().join("keymap.toml")
}

/// One rebinding from keymap.toml: a command, named by its action
/// (`qur::GoToSymbol`) or palette title (`Go to Symbol in File…`), and its
/// new keys in GPUI syntax (`cmd-shift-o`, chords `cmd-k cmd-t`). Empty
/// `keys` unbinds it. The app resolves the name and checks the keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyOverride {
    pub command: String,
    pub keys: Vec<String>,
    /// 1-based line in keymap.toml, for messages.
    pub line: usize,
}

/// Reads keymap.toml's `[bindings]`. A missing file gives nothing; an
/// unreadable one gives one error; each entry of the wrong shape gives an
/// error and is skipped (never a crash).
pub fn load_keymap() -> (Vec<KeyOverride>, Vec<String>) {
    load_keymap_from(&keymap_path())
}

pub fn load_keymap_from(path: &Path) -> (Vec<KeyOverride>, Vec<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), Vec::new()),
        Err(e) => return (Vec::new(), vec![format!("keymap.toml: {e}")]),
    };
    let doc: toml_edit::DocumentMut = match text.parse() {
        Ok(d) => d,
        Err(e) => return (Vec::new(), vec![format!("keymap.toml: {}", e.to_string().trim())]),
    };
    // The line a key is on (spans aren't kept by DocumentMut): the first
    // line starting with the key, bare or quoted.
    let line_of = |key: &str| {
        text.lines()
            .position(|l| {
                let l = l.trim_start();
                let rest = l.strip_prefix(&format!("\"{key}\"")).or_else(|| l.strip_prefix(&format!("'{key}'"))).or_else(|| l.strip_prefix(key));
                rest.is_some_and(|r| r.trim_start().starts_with('='))
            })
            .map_or(0, |i| i + 1)
    };
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (key, _) in doc.iter() {
        if key != "bindings" {
            errors.push(format!("keymap.toml: unknown key `{key}` (bindings go under [bindings])"));
        }
    }
    let Some(item) = doc.get("bindings") else { return (out, errors) };
    let Some(table) = item.as_table_like() else {
        errors.push("keymap.toml: [bindings] must be a table".into());
        return (out, errors);
    };
    for (command, item) in table.iter() {
        let line = line_of(command);
        let keys = match item.as_value() {
            Some(toml_edit::Value::String(s)) => Some(vec![s.value().trim().to_string()]),
            Some(toml_edit::Value::Array(a)) => {
                a.iter().map(|v| v.as_str().map(|s| s.trim().to_string())).collect::<Option<Vec<_>>>()
            }
            _ => None,
        };
        match keys {
            Some(keys) => {
                let keys = keys.into_iter().filter(|k| !k.is_empty()).collect();
                out.push(KeyOverride { command: command.trim().to_string(), keys, line });
            }
            None => errors.push(format!(
                "keymap.toml line {line}: `{command}`: expected keys as a string or a list of strings"
            )),
        }
    }
    (out, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_entries_and_bad_ones() {
        let d = tmp("keymap");
        let p = d.join("keymap.toml");
        assert_eq!(load_keymap_from(&p), (Vec::new(), Vec::new()), "missing file: no overrides");
        std::fs::write(
            &p,
            "# mine\n[bindings]\n\"qur::GoToSymbol\" = \"cmd-t\"\n\"Delete Line\" = [\"cmd-shift-k\", \" ctrl-k \"]\n\"Toggle Sidebar\" = \"\"\nbad = 3\nalso_bad = [1]\n[other]\nx = 1\n",
        )
        .unwrap();
        let (ov, errs) = load_keymap_from(&p);
        let got: Vec<(&str, Vec<&str>, usize)> =
            ov.iter().map(|o| (o.command.as_str(), o.keys.iter().map(String::as_str).collect(), o.line)).collect();
        assert_eq!(
            got,
            vec![
                ("qur::GoToSymbol", vec!["cmd-t"], 3),
                ("Delete Line", vec!["cmd-shift-k", "ctrl-k"], 4),
                ("Toggle Sidebar", vec![], 5),
            ]
        );
        assert_eq!(errs.len(), 3, "{errs:?}");
        assert!(errs[0].contains("`other`"), "{errs:?}");
        assert!(errs[1].contains("line 6") && errs[1].contains("`bad`"), "{errs:?}");
        assert!(errs[2].contains("`also_bad`"), "{errs:?}");
        std::fs::write(&p, "[bindings\n").unwrap();
        let (ov, errs) = load_keymap_from(&p);
        assert!(ov.is_empty());
        assert!(errs[0].starts_with("keymap.toml"), "{errs:?}");
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qur-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sidebar_settings_sanitize_and_round_trip() {
        let d = tmp("sidebar");
        let p = d.join("config.toml");
        std::fs::write(&p, "[ui]\nsidebar = \"bogus\"\nsidebar_side = \"up\"\nsidebar_sections = [\"files\", \"nope\", \"files\", \"git\"]\n").unwrap();
        let (c, err) = Config::load_from(&p);
        assert!(err.is_none(), "{err:?}");
        assert_eq!((c.ui.sidebar.as_str(), c.ui.sidebar_side.as_str()), ("show", "left"));
        assert_eq!(c.ui.sidebar_sections, ["files", "git", "agents", "terminals"], "unknown and repeated dropped, missing appended");
        write_key(&p, "ui.sidebar_sections", Some(Value::List(vec!["git".into(), "agents".into()]))).unwrap();
        write_key(&p, "ui.sidebar", Some("auto".into())).unwrap();
        write_key(&p, "ui.sidebar_side", Some("right".into())).unwrap();
        let (c, _) = Config::load_from(&p);
        assert_eq!((c.ui.sidebar.as_str(), c.ui.sidebar_side.as_str()), ("auto", "right"));
        assert_eq!(c.ui.sidebar_sections, ["git", "agents", "terminals", "files"]);
    }

    #[test]
    fn gaps_round_trip_and_clamp() {
        let d = tmp("gaps");
        let p = d.join("config.toml");
        let (c, _) = Config::load_from(&p);
        assert!(!c.ui.gaps && c.ui.rounded && !c.ui.tile_border);
        assert_eq!((c.ui.gap, c.ui.radius), (DEFAULT_GAP, DEFAULT_RADIUS));
        write_key(&p, "ui.gaps", Some(true.into())).unwrap();
        write_key(&p, "ui.gap", Some(12.0f32.into())).unwrap();
        write_key(&p, "ui.rounded", Some(false.into())).unwrap();
        write_key(&p, "ui.radius", Some(4.0f32.into())).unwrap();
        write_key(&p, "ui.tile_border", Some(true.into())).unwrap();
        let (c, err) = Config::load_from(&p);
        assert!(err.is_none(), "{err:?}");
        assert!(c.ui.gaps && !c.ui.rounded && c.ui.tile_border);
        assert_eq!((c.ui.gap, c.ui.radius), (12.0, 4.0));
        std::fs::write(&p, "[ui]\ngaps = true\ngap = 500\nradius = -3\n").unwrap();
        let (c, _) = Config::load_from(&p);
        assert_eq!((c.ui.gap, c.ui.radius), (MAX_GAP, 0.0));
        std::fs::write(&p, "[ui]\ngap = 0\n").unwrap();
        assert_eq!(Config::load_from(&p).0.ui.gap, MIN_GAP);
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

    #[test]
    fn project_state_round_trip() {
        let s = ProjectState {
            files: vec![
                OpenFile { path: "/p/src/main.rs".into(), line: 41, col: 3 },
                OpenFile { path: "/p/README.md".into(), line: 0, col: 0 },
            ],
            code_open: true,
        };
        let back: ProjectState = toml_edit::de::from_str(&s.to_toml()).unwrap();
        assert_eq!(back, s);
        let empty: ProjectState = toml_edit::de::from_str(&ProjectState::default().to_toml()).unwrap();
        assert_eq!(empty, ProjectState::default());
    }
}
