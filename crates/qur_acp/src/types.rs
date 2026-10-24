//! ACP v1 data types Qur reads, parsed leniently from JSON: unknown fields
//! are ignored, unknown enum strings map to `Other`/defaults, nothing panics.

use std::path::PathBuf;

use serde_json::{Value, json};

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

/// The id of a request the agent sent us (a number or a string).
#[derive(Clone, Debug, PartialEq)]
pub struct RequestId(pub Value);

/// String ids that mustn't be mixed up: a session (the agent's own, or a
/// subagent's child session) and a tool call.
macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }
        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl From<String> for $name {
            fn from(s: String) -> Self {
                $name(s)
            }
        }
        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                $name(s.to_string())
            }
        }
        impl PartialEq<str> for $name {
            fn eq(&self, o: &str) -> bool {
                self.0 == o
            }
        }
        impl PartialEq<&str> for $name {
            fn eq(&self, o: &&str) -> bool {
                self.0 == *o
            }
        }
    };
}

string_id!(
    /// An ACP session id: the agent's own, or a subagent's child session.
    SessionId
);
string_id!(
    /// A tool call's id, unique within its session.
    ToolCallId
);

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
    pub auth_methods: Vec<AuthMethod>,
    pub raw: Value,
}

/// A way to sign in the agent offers (`authMethods` in `initialize`).
#[derive(Clone, Debug, PartialEq)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Signing in means running a command in a terminal (`type: "terminal"`,
    /// or Zed's `_meta["terminal-auth"]`), not calling `authenticate`.
    pub terminal: Option<TerminalLogin>,
}

/// A login command to run in a terminal.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalLogin {
    /// The program (`_meta["terminal-auth"].command`); None means the
    /// agent's own command with `args` added (`type: "terminal"`).
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub label: Option<String>,
}

impl TerminalLogin {
    /// Program and arguments to run, given how the agent itself was started.
    pub fn command_line(&self, agent: &AgentCommand) -> (String, Vec<String>) {
        match &self.command {
            Some(c) => (c.clone(), self.args.clone()),
            None => (agent.program.clone(), agent.args.iter().chain(&self.args).cloned().collect()),
        }
    }
}

impl AuthMethod {
    pub(crate) fn parse(v: &Value) -> Option<Self> {
        let strings = |v: Option<&Value>| -> Vec<String> {
            v.and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
        };
        // `env` is an object in `_meta["terminal-auth"]`, a list of
        // `{name, value}` in the spec's terminal method.
        let env = |v: Option<&Value>| -> Vec<(String, String)> {
            match v {
                Some(Value::Object(m)) => m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect(),
                Some(Value::Array(a)) => a.iter().filter_map(|e| Some((s(e, "name")?, s(e, "value")?))).collect(),
                _ => Vec::new(),
            }
        };
        // The `_meta` form names the exact program; prefer it.
        let terminal = v
            .pointer("/_meta/terminal-auth")
            .and_then(|m| Some(TerminalLogin { command: Some(s(m, "command")?), args: strings(m.get("args")), env: env(m.get("env")), label: s(m, "label") }))
            .or_else(|| {
                (v.get("type").and_then(Value::as_str) == Some("terminal"))
                    .then(|| TerminalLogin { command: None, args: strings(v.get("args")), env: env(v.get("env")), label: None })
            });
        Some(AuthMethod {
            id: s(v, "id")?,
            name: s(v, "name").unwrap_or_default(),
            description: s(v, "description").map(|d| d.trim().to_string()).filter(|d| !d.is_empty()),
            terminal,
        })
    }
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
                .map(|a| a.iter().filter_map(AuthMethod::parse).collect())
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

    /// What to tell the user when a turn ends this way (`agent`: its name).
    /// None for a normal end.
    pub fn describe(&self, agent: &str) -> Option<String> {
        Some(match self {
            StopReason::EndTurn => return None,
            StopReason::Cancelled => "Stopped.".into(),
            StopReason::MaxTokens => "The reply hit its length limit before it was done. Retry, or say \"continue\" to go on.".into(),
            StopReason::MaxTurnRequests => "This turn took too many steps and was stopped. Retry, or send a message to carry on.".into(),
            StopReason::Refusal => format!("{agent} declined this request. Rephrase it and send again."),
            StopReason::Other(o) if o.is_empty() => format!("{agent} ended the turn without saying why."),
            StopReason::Other(o) => format!("{agent} ended the turn early ({}).", o.replace('_', " ")),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpError {
    pub code: Option<i64>,
    pub message: String,
}

/// ACP's JSON-RPC error code for "authentication required".
pub const AUTH_REQUIRED: i64 = -32000;
/// The error every outstanding request gets when the agent process ends.
pub(crate) const AGENT_EXITED: &str = "agent exited";

impl AcpError {
    /// The agent needs the user to sign in first (`session/new` or
    /// `session/prompt` refused with -32000).
    pub fn is_auth_required(&self) -> bool {
        self.code == Some(AUTH_REQUIRED)
    }

    /// The request failed because the agent process ended (`Event::Exited`
    /// follows with its exit code and stderr).
    pub fn is_agent_exited(&self) -> bool {
        self.code.is_none() && self.message == AGENT_EXITED
    }

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

    /// The sentence worth showing. Agents wrap the real reason in layers
    /// (codex-acp: "Internal error" with `data.message` holding the API's JSON
    /// as a string, whose `error.message` says what's wrong), so this digs to
    /// the innermost `message`. The full text stays in `Display` for Copy.
    pub fn summary(&self) -> String {
        let raw = self.message.split_once(": ").map(|(_, d)| d).unwrap_or("");
        serde_json::from_str::<Value>(raw).ok().and_then(|v| innermost_message(&v)).unwrap_or_else(|| self.message.clone())
    }
}

/// The deepest `message` in `v`, looking inside JSON-in-a-string too.
fn innermost_message(v: &Value) -> Option<String> {
    match v {
        Value::String(t) => serde_json::from_str::<Value>(t).ok().and_then(|inner| innermost_message(&inner)).or_else(|| Some(t.clone())).filter(|t| !t.trim().is_empty()),
        Value::Object(o) => o
            .get("error")
            .and_then(innermost_message)
            .or_else(|| o.get("message").and_then(innermost_message))
            .or_else(|| o.get("detail").and_then(innermost_message)),
        _ => None,
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
    Update { session_id: SessionId, update: SessionUpdate },
    Permission { id: RequestId, session_id: SessionId, tool_call: ToolCallUpdate, options: Vec<PermissionOption> },
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
    /// `usage_update`: context tokens used of the window, and the session's
    /// cost so far (USD) when the agent knows it.
    Usage { used: u64, size: u64, cost: Option<f64> },
    /// A subagent the agent started (Claude's Agent/Task tool): announced, renamed
    /// or changed state. Its own updates arrive under `id` as the session.
    Subagent { id: SessionId, title: Option<String>, state: Option<String> },
    /// `session_info_update` with a title (the agent named the session).
    /// Updates carrying only `_meta`, or a null title, come as `Other`.
    SessionTitle(String),
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
            "usage_update" => SessionUpdate::Usage {
                used: u.get("used").and_then(Value::as_u64).unwrap_or(0),
                size: u.get("size").and_then(Value::as_u64).unwrap_or(0),
                cost: u.pointer("/cost/amount").and_then(Value::as_f64),
            },
            "subagent_update" => SessionUpdate::Subagent {
                id: s(u, "sessionId").unwrap_or_default().into(),
                title: s(u, "title").or_else(|| s(u, "description")),
                state: u.pointer("/state/state").and_then(Value::as_str).map(str::to_string),
            },
            "session_info_update" if s(u, "title").is_some_and(|t| !t.trim().is_empty()) => {
                SessionUpdate::SessionTitle(s(u, "title").unwrap_or_default().trim().to_string())
            }
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

/// A message block that isn't text.
#[derive(Clone, Debug, PartialEq)]
pub enum Media {
    /// Decoded image bytes and their MIME type.
    Image { mime: String, data: Vec<u8> },
    /// A `resource_link`, an embedded `resource`, or an image given only by
    /// URL: a file (`file://…`) or a web address.
    Link { uri: String, name: String },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentChunk {
    /// The text; empty when the block is `media`.
    pub text: String,
    pub media: Option<Media>,
}

impl ContentChunk {
    pub fn text(text: impl Into<String>) -> Self {
        ContentChunk { text: text.into(), media: None }
    }

    fn parse(u: &Value) -> Self {
        let Some(b) = u.get("content") else { return ContentChunk::default() };
        match media(b) {
            Some(m) => ContentChunk { text: String::new(), media: Some(m) },
            None => ContentChunk::text(block_text(b)),
        }
    }
}

/// The last path segment of a URI, for a link's name.
fn uri_name(uri: &str) -> String {
    let t = uri.trim_end_matches('/');
    t.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(t).to_string()
}

/// An image, resource or link block as `Media`. None for text, audio, an
/// image that neither decodes nor has a URI, or anything unknown (those
/// stay text).
fn media(b: &Value) -> Option<Media> {
    match b.get("type").and_then(Value::as_str)? {
        "image" => match s(b, "data").and_then(|d| decode_base64(&d)).filter(|d| !d.is_empty()) {
            Some(data) => Some(Media::Image { mime: s(b, "mimeType").filter(|m| !m.is_empty()).unwrap_or_else(|| "image/png".into()), data }),
            // Claude sends `data: ""` and a `uri` for an image given by URL.
            None => s(b, "uri").filter(|u| !u.is_empty()).map(|uri| Media::Link { name: uri_name(&uri), uri }),
        },
        "resource_link" => {
            let uri = s(b, "uri")?;
            Some(Media::Link { name: s(b, "title").or_else(|| s(b, "name")).unwrap_or_else(|| uri_name(&uri)), uri })
        }
        "resource" => {
            let uri = b.get("resource").and_then(|r| s(r, "uri"))?;
            Some(Media::Link { name: uri_name(&uri), uri })
        }
        _ => None,
    }
}

/// Standard or URL-safe base64; padding and whitespace optional. None on
/// any other character.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
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
    pub id: ToolCallId,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolStatus,
    pub locations: Vec<Location>,
    pub content: Vec<ToolContent>,
    pub raw_input: Option<Value>,
    /// The tool's raw result, when no other field carries it.
    pub raw_output: Option<Value>,
    /// A command's output (`_meta.terminal_output`, sent because Qur
    /// advertises `terminal_output`), and its exit once it ended: the code,
    /// or None when the agent doesn't know it.
    pub output: Option<String>,
    pub exit: Option<Option<i64>>,
}

impl ToolCall {
    fn parse(u: &Value) -> Self {
        ToolCall {
            id: s(u, "toolCallId").unwrap_or_default().into(),
            title: s(u, "title").unwrap_or_default(),
            kind: ToolKind::parse(u.get("kind").and_then(Value::as_str).unwrap_or("")),
            status: ToolStatus::parse(u.get("status").and_then(Value::as_str).unwrap_or("")),
            locations: u.get("locations").map(parse_locations).unwrap_or_default(),
            content: u.get("content").map(parse_content).unwrap_or_default(),
            raw_input: u.get("rawInput").filter(|v| !v.is_null()).cloned(),
            ..Default::default()
        }
        .with_output(&ToolCallUpdate::parse(u))
    }

    fn with_output(mut self, u: &ToolCallUpdate) -> Self {
        self.apply_output(u);
        self
    }

    fn apply_output(&mut self, u: &ToolCallUpdate) {
        if let Some(r) = &u.raw_output {
            self.raw_output = Some(r.clone());
        }
        if let Some(o) = &u.output {
            self.output = Some(o.clone());
        }
        if let Some(d) = &u.output_delta {
            self.output.get_or_insert_with(String::new).push_str(d);
        }
        if let Some(e) = u.exit {
            self.exit = Some(e);
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
        self.apply_output(u);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCallUpdate {
    pub id: ToolCallId,
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolStatus>,
    pub locations: Option<Vec<Location>>,
    pub content: Option<Vec<ToolContent>>,
    pub raw_input: Option<Value>,
    pub raw_output: Option<Value>,
    /// `_meta.terminal_output.data`: the command's whole output so far.
    pub output: Option<String>,
    /// `_meta.terminal_output_delta.data`: more output.
    pub output_delta: Option<String>,
    /// `_meta.terminal_exit`: the command ended (exit code if known).
    pub exit: Option<Option<i64>>,
}

impl ToolCallUpdate {
    pub(crate) fn parse(u: &Value) -> Self {
        let present = |k: &str| u.get(k).filter(|v| !v.is_null());
        ToolCallUpdate {
            id: s(u, "toolCallId").unwrap_or_default().into(),
            title: s(u, "title"),
            kind: present("kind").and_then(Value::as_str).map(ToolKind::parse),
            status: present("status").and_then(Value::as_str).map(ToolStatus::parse),
            locations: present("locations").map(parse_locations),
            content: present("content").map(parse_content),
            raw_input: present("rawInput").cloned(),
            raw_output: present("rawOutput").cloned(),
            output: u.pointer("/_meta/terminal_output/data").and_then(Value::as_str).map(str::to_owned),
            output_delta: u.pointer("/_meta/terminal_output_delta/data").and_then(Value::as_str).map(str::to_owned),
            exit: u.pointer("/_meta/terminal_exit").filter(|v| v.is_object()).map(|e| e.get("exit_code").and_then(Value::as_i64)),
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
