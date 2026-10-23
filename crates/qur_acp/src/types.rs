//! ACP v1 data types Qur reads, parsed leniently from JSON: unknown fields
//! are ignored, unknown enum strings map to `Other`/defaults, nothing panics.

use std::path::PathBuf;

use serde_json::{Value, json};

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

#[derive(Clone, Debug, PartialEq)]
pub struct RequestId(pub Value);

#[derive(Clone, Debug, PartialEq)]
pub struct AgentCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PromptBlock {
    Text(String),
    /// An `@file` mention; `uri` is usually `file:///abs/path`.
    ResourceLink { uri: String, name: String },
    /// An image (base64), e.g. a screenshot of an inspected element. Only
    /// send when the agent's `promptCapabilities.image` is true.
    Image { mime: String, data_base64: String },
}

impl PromptBlock {
    pub(crate) fn to_json(&self) -> Value {
        match self {
            PromptBlock::Text(t) => json!({ "type": "text", "text": t }),
            PromptBlock::ResourceLink { uri, name } => json!({ "type": "resource_link", "uri": uri, "name": name }),
            PromptBlock::Image { mime, data_base64 } => json!({ "type": "image", "mimeType": mime, "data": data_base64 }),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InitializeResult {
    pub protocol_version: u64,
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
    pub load_session: bool,
    pub auth_methods: Vec<String>,
    pub raw: Value,
}

impl InitializeResult {
    pub(crate) fn parse(v: Value) -> Self {
        let info = v.get("agentInfo").cloned().unwrap_or(Value::Null);
        InitializeResult {
            protocol_version: v.get("protocolVersion").and_then(Value::as_u64).unwrap_or(0),
            agent_name: s(&info, "name"),
            agent_version: s(&info, "version"),
            load_session: v.pointer("/agentCapabilities/loadSession").and_then(Value::as_bool).unwrap_or(false),
            auth_methods: v
                .get("authMethods")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|m| s(m, "id")).collect())
                .unwrap_or_default(),
            raw: v,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    Other(String),
}

impl StopReason {
    pub(crate) fn parse(v: &Value) -> Self {
        match v.get("stopReason").and_then(Value::as_str).unwrap_or("") {
            "end_turn" => StopReason::EndTurn,
            "max_tokens" => StopReason::MaxTokens,
            "max_turn_requests" => StopReason::MaxTurnRequests,
            "refusal" => StopReason::Refusal,
            "cancelled" => StopReason::Cancelled,
            o => StopReason::Other(o.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpError {
    pub code: Option<i64>,
    pub message: String,
}

impl AcpError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        AcpError { code: None, message: message.into() }
    }
    pub(crate) fn from_rpc(e: &Value) -> Self {
        let mut message = s(e, "message").unwrap_or_else(|| e.to_string());
        if let Some(d) = e.get("data").filter(|d| !d.is_null()) {
            message = format!("{message}: {d}");
        }
        AcpError { code: e.get("code").and_then(Value::as_i64), message }
    }
}

impl std::fmt::Display for AcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(c) => write!(f, "{} ({c})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for AcpError {}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Update { session_id: String, update: SessionUpdate },
    Permission { id: RequestId, session_id: String, tool_call: ToolCallUpdate, options: Vec<PermissionOption> },
    /// The agent process exited (exit code, last ~4 KB of stderr).
    Exited { code: Option<i32>, stderr_tail: String },
    /// A malformed message, or a request Qur doesn't handle (answered with -32601).
    Warning(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionUpdate {
    AgentMessage(ContentChunk),
    AgentThought(ContentChunk),
    UserMessage(ContentChunk),
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCallUpdate),
    Plan(Vec<PlanEntry>),
    AvailableCommands(Vec<AvailableCommand>),
    CurrentMode(String),
    /// `config_option_update`: the agent's whole set of session settings.
    ConfigOptions(Vec<ConfigOption>),
    Other { kind: String, raw: Value },
}

/// One session setting the agent offers (mode, model, thinking…): a
/// `select` from `configOptions`, or the older `modes` made to look like one.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    /// `mode`, `model`, `thought_level`, … (None or unknown is fine).
    pub category: Option<String>,
    pub current: String,
    pub values: Vec<ConfigValue>,
    /// Came from `modes`: set with `session/set_mode`, not `set_config_option`.
    pub legacy_mode: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigValue {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

impl ConfigOption {
    pub fn is_mode(&self) -> bool {
        self.category.as_deref() == Some("mode")
    }

    /// The current value's display name.
    pub fn current_name(&self) -> &str {
        self.values.iter().find(|v| v.value == self.current).map_or(self.current.as_str(), |v| v.name.as_str())
    }

    /// The value after the current one, wrapping (Shift+Tab cycles modes).
    pub fn next_value(&self) -> Option<&str> {
        let i = self.values.iter().position(|v| v.value == self.current).map_or(0, |i| i + 1);
        self.values.get(i % self.values.len().max(1)).map(|v| v.value.as_str())
    }

    fn parse(o: &Value) -> Option<Self> {
        // Only selects: boolean options need a client capability we don't send.
        if o.get("type").and_then(Value::as_str) != Some("select") {
            return None;
        }
        let value = |v: &Value| Some(ConfigValue { value: s(v, "value")?, name: s(v, "name")?, description: s(v, "description") });
        let mut values = Vec::new();
        for item in o.get("options").and_then(Value::as_array).into_iter().flatten() {
            match item.get("options").and_then(Value::as_array) {
                // A group: its values, flattened.
                Some(group) => values.extend(group.iter().filter_map(value)),
                None => values.extend(value(item)),
            }
        }
        Some(ConfigOption {
            id: s(o, "id")?,
            name: s(o, "name").unwrap_or_default(),
            category: s(o, "category"),
            current: o.get("currentValue").and_then(Value::as_str)?.to_string(),
            values,
            legacy_mode: false,
        })
    }

    /// `configOptions` from a `session/new`, `session/load` or
    /// `set_config_option` result, or a `config_option_update`.
    pub fn parse_list(v: &Value) -> Vec<Self> {
        v.get("configOptions").and_then(Value::as_array).map(|a| a.iter().filter_map(Self::parse).collect()).unwrap_or_default()
    }

    /// The settings in a `session/new` or `session/load` result: its
    /// `configOptions`, plus `modes` when no option is the mode (the spec
    /// says to prefer config options and fall back to modes).
    pub fn parse_session(v: &Value) -> Vec<Self> {
        let mut list = Self::parse_list(v);
        if !list.iter().any(Self::is_mode)
            && let Some(m) = v.get("modes")
            && let Some(current) = s(m, "currentModeId")
        {
            let values: Vec<ConfigValue> = m
                .get("availableModes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|x| Some(ConfigValue { value: s(x, "id")?, name: s(x, "name")?, description: s(x, "description") }))
                .collect();
            if !values.is_empty() {
                list.insert(0, ConfigOption { id: "mode".into(), name: "Mode".into(), category: Some("mode".into()), current, values, legacy_mode: true });
            }
        }
        list
    }
}

impl SessionUpdate {
    pub fn parse(u: &Value) -> Self {
        let kind = u.get("sessionUpdate").and_then(Value::as_str).unwrap_or("");
        match kind {
            "agent_message_chunk" => SessionUpdate::AgentMessage(ContentChunk::parse(u)),
            "agent_thought_chunk" => SessionUpdate::AgentThought(ContentChunk::parse(u)),
            "user_message_chunk" => SessionUpdate::UserMessage(ContentChunk::parse(u)),
            "tool_call" => SessionUpdate::ToolCall(ToolCall::parse(u)),
            "tool_call_update" => SessionUpdate::ToolCallUpdate(ToolCallUpdate::parse(u)),
            "plan" => SessionUpdate::Plan(
                u.get("entries").and_then(Value::as_array).map(|a| a.iter().map(PlanEntry::parse).collect()).unwrap_or_default(),
            ),
            "available_commands_update" => SessionUpdate::AvailableCommands(
                u.get("availableCommands")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(AvailableCommand::parse).collect())
                    .unwrap_or_default(),
            ),
            "current_mode_update" => SessionUpdate::CurrentMode(s(u, "currentModeId").unwrap_or_default()),
            "config_option_update" => SessionUpdate::ConfigOptions(ConfigOption::parse_list(u)),
            k => SessionUpdate::Other { kind: k.to_string(), raw: u.clone() },
        }
    }
}

/// Text of one content block. Non-text blocks become short placeholders.
fn block_text(b: &Value) -> String {
    match b.get("type").and_then(Value::as_str).unwrap_or("") {
        "text" => s(b, "text").unwrap_or_default(),
        "image" => "[image]".into(),
        "audio" => "[audio]".into(),
        "resource_link" => format!("[{}]", s(b, "name").or_else(|| s(b, "uri")).unwrap_or_default()),
        "resource" => {
            let r = b.get("resource").cloned().unwrap_or(Value::Null);
            s(&r, "text").unwrap_or_else(|| format!("[{}]", s(&r, "uri").unwrap_or_else(|| "resource".into())))
        }
        other => format!("[{other}]"),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentChunk {
    pub text: String,
}

impl ContentChunk {
    fn parse(u: &Value) -> Self {
        ContentChunk { text: u.get("content").map(block_text).unwrap_or_default() }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    #[default]
    Other,
}

impl ToolKind {
    fn parse(v: &str) -> Self {
        match v {
            "read" => ToolKind::Read,
            "edit" => ToolKind::Edit,
            "delete" => ToolKind::Delete,
            "move" => ToolKind::Move,
            "search" => ToolKind::Search,
            "execute" => ToolKind::Execute,
            "think" => ToolKind::Think,
            "fetch" => ToolKind::Fetch,
            "switch_mode" => ToolKind::SwitchMode,
            _ => ToolKind::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl ToolStatus {
    fn parse(v: &str) -> Self {
        match v {
            "in_progress" => ToolStatus::InProgress,
            "completed" => ToolStatus::Completed,
            "failed" => ToolStatus::Failed,
            _ => ToolStatus::Pending,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub line: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolContent {
    Text(String),
    Diff { path: PathBuf, old_text: Option<String>, new_text: String },
    /// A terminal embedded by id (Qur doesn't offer the terminal capability,
    /// so agents shouldn't send these).
    Terminal(String),
}

fn parse_locations(v: &Value) -> Vec<Location> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    Some(Location {
                        path: PathBuf::from(s(l, "path")?),
                        line: l.get("line").and_then(Value::as_u64).map(|n| n as u32),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_content(v: &Value) -> Vec<ToolContent> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| match c.get("type").and_then(Value::as_str)? {
                    "content" => Some(ToolContent::Text(c.get("content").map(block_text).unwrap_or_default())),
                    "diff" => Some(ToolContent::Diff {
                        path: PathBuf::from(s(c, "path")?),
                        old_text: s(c, "oldText"),
                        new_text: s(c, "newText").unwrap_or_default(),
                    }),
                    "terminal" => Some(ToolContent::Terminal(s(c, "terminalId").unwrap_or_default())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolStatus,
    pub locations: Vec<Location>,
    pub content: Vec<ToolContent>,
    pub raw_input: Option<Value>,
}

impl ToolCall {
    fn parse(u: &Value) -> Self {
        ToolCall {
            id: s(u, "toolCallId").unwrap_or_default(),
            title: s(u, "title").unwrap_or_default(),
            kind: ToolKind::parse(u.get("kind").and_then(Value::as_str).unwrap_or("")),
            status: ToolStatus::parse(u.get("status").and_then(Value::as_str).unwrap_or("")),
            locations: u.get("locations").map(parse_locations).unwrap_or_default(),
            content: u.get("content").map(parse_content).unwrap_or_default(),
            raw_input: u.get("rawInput").filter(|v| !v.is_null()).cloned(),
        }
    }

    /// Merge an update in. Fields the update carries replace ours
    /// (`content` and `locations` are replaced as a whole, per the spec).
    pub fn apply(&mut self, u: &ToolCallUpdate) {
        if let Some(t) = &u.title {
            self.title = t.clone();
        }
        if let Some(k) = u.kind {
            self.kind = k;
        }
        if let Some(st) = u.status {
            self.status = st;
        }
        if let Some(l) = &u.locations {
            self.locations = l.clone();
        }
        if let Some(c) = &u.content {
            self.content = c.clone();
        }
        if let Some(r) = &u.raw_input {
            self.raw_input = Some(r.clone());
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCallUpdate {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolStatus>,
    pub locations: Option<Vec<Location>>,
    pub content: Option<Vec<ToolContent>>,
    pub raw_input: Option<Value>,
}

impl ToolCallUpdate {
    pub(crate) fn parse(u: &Value) -> Self {
        let present = |k: &str| u.get(k).filter(|v| !v.is_null());
        ToolCallUpdate {
            id: s(u, "toolCallId").unwrap_or_default(),
            title: s(u, "title"),
            kind: present("kind").and_then(Value::as_str).map(ToolKind::parse),
            status: present("status").and_then(Value::as_str).map(ToolStatus::parse),
            locations: present("locations").map(parse_locations),
            content: present("content").map(parse_content),
            raw_input: present("rawInput").cloned(),
        }
    }

    /// A full tool call built from an update alone (permission requests).
    pub fn to_tool_call(&self) -> ToolCall {
        let mut t = ToolCall { id: self.id.clone(), ..Default::default() };
        t.apply(self);
        t
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlanStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanEntry {
    pub content: String,
    pub priority: String,
    pub status: PlanStatus,
}

impl PlanEntry {
    fn parse(v: &Value) -> Self {
        PlanEntry {
            content: s(v, "content").unwrap_or_default(),
            priority: s(v, "priority").unwrap_or_else(|| "medium".into()),
            status: match v.get("status").and_then(Value::as_str).unwrap_or("") {
                "in_progress" => PlanStatus::InProgress,
                "completed" => PlanStatus::Completed,
                _ => PlanStatus::Pending,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableCommand {
    pub name: String,
    pub description: String,
}

impl AvailableCommand {
    fn parse(v: &Value) -> Self {
        AvailableCommand { name: s(v, "name").unwrap_or_default(), description: s(v, "description").unwrap_or_default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    pub kind: PermissionKind,
}

impl PermissionOption {
    pub(crate) fn parse(v: &Value) -> Option<Self> {
        Some(PermissionOption {
            id: s(v, "optionId")?,
            name: s(v, "name").unwrap_or_default(),
            kind: match v.get("kind").and_then(Value::as_str).unwrap_or("") {
                "allow_always" => PermissionKind::AllowAlways,
                "reject_once" => PermissionKind::RejectOnce,
                "reject_always" => PermissionKind::RejectAlways,
                _ => PermissionKind::AllowOnce,
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// The chosen option's id.
    Selected(String),
    Cancelled,
}

impl PermissionOutcome {
    pub(crate) fn to_json(&self) -> Value {
        match self {
            PermissionOutcome::Selected(id) => json!({ "outcome": { "outcome": "selected", "optionId": id } }),
            PermissionOutcome::Cancelled => json!({ "outcome": { "outcome": "cancelled" } }),
        }
    }
}
