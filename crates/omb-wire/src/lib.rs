use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalMode {
    Ask,
    Edits,
    Auto,
    Full,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineId {
    Claude,
    Codex,
    OpenaiApi,
    Jev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthState {
    SignedIn,
    SignedOut,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineStatus {
    pub id: EngineId,
    pub available: bool,
    pub version: Option<String>,
    pub auth: AuthState,
    pub account: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
    Approval,
    Error,
}

/// The body of a `MessageRole::Tool` message, as JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub input: serde_json::Value,
    pub ok: Option<bool>,
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalState {
    Pending,
    Allowed,
    Denied,
}

/// The body of a `MessageRole::Approval` message, as JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub tool: String,
    pub input: serde_json::Value,
    pub state: ApprovalState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bot {
    pub id: i64,
    pub name: String,
    pub engine: EngineId,
    pub model: Option<String>,
    pub approval: ApprovalMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub bot_id: i64,
    pub role: MessageRole,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BotEvent {
    pub bot_id: i64,
    pub event: RuntimeEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RuntimeEvent {
    #[serde(rename = "turn.started")]
    TurnStarted { session_id: String, model: Option<String> },
    #[serde(rename = "content.delta")]
    ContentDelta { text: String },
    #[serde(rename = "item.started")]
    ItemStarted { id: String, name: String, input: serde_json::Value },
    #[serde(rename = "item.completed")]
    ItemCompleted { id: String, ok: bool, output: String },
    #[serde(rename = "request.opened")]
    RequestOpened { request_id: String, tool: String, input: serde_json::Value },
    #[serde(rename = "request.resolved")]
    RequestResolved { request_id: String, allowed: bool },
    #[serde(rename = "turn.completed")]
    TurnCompleted { is_error: bool, result: Option<String>, cost_usd: Option<f64> },
    #[serde(rename = "runtime.error")]
    RuntimeError { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_ordered_and_serialize_lowercase() {
        assert!(ApprovalMode::Ask < ApprovalMode::Edits);
        assert!(ApprovalMode::Edits < ApprovalMode::Auto);
        assert!(ApprovalMode::Auto < ApprovalMode::Full);
        assert_eq!(serde_json::to_string(&ApprovalMode::Edits).unwrap(), "\"edits\"");
    }

    #[test]
    fn runtime_events_are_tagged_by_dotted_type() {
        let event = RuntimeEvent::ContentDelta { text: "hi".into() };
        assert_eq!(serde_json::to_string(&event).unwrap(), r#"{"type":"content.delta","text":"hi"}"#);
        assert_eq!(serde_json::from_str::<RuntimeEvent>(r#"{"type":"runtime.error","message":"x"}"#).unwrap(), RuntimeEvent::RuntimeError { message: "x".into() });
    }
}
