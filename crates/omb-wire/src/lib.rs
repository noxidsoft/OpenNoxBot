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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostPolicy {
    /// Only known status commands run; anything else is refused.
    ReadOnly,
    /// Status commands run; anything else waits for the person.
    Ask,
    /// Every command runs.
    Full,
}

/// A server agents may reach over SSH. `target` is a hostname, an address or an alias from the person's ssh config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewHost {
    pub name: String,
    pub target: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub policy: HostPolicy,
}

fn plain(text: &str, max: usize, extra: &str) -> bool {
    !text.is_empty()
        && text.len() <= max
        && !text.starts_with(['-', '.'])
        && text.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

impl NewHost {
    /// Names, targets and users end up on an `ssh` command line, so nothing that could be read as an option gets through.
    pub fn validate(&self) -> Result<(), String> {
        if !plain(&self.name, 32, "_-.") {
            return Err("Use up to 32 letters, digits, dots, dashes or underscores for the name".into());
        }
        if !plain(&self.target, 253, "._:-") {
            return Err("Enter a hostname, an address or an ssh config alias".into());
        }
        if self.user.as_deref().is_some_and(|user| !plain(user, 32, "_-.")) {
            return Err("Enter a valid user name".into());
        }
        if self.port == Some(0) {
            return Err("Enter a port from 1 to 65535".into());
        }
        Ok(())
    }
}

/// The mailbox agents may read and draft in. The password lives in the OS keyring, never here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailSettings {
    pub user: String,
    pub host: String,
    pub port: u16,
}

impl EmailSettings {
    pub fn validate(&self) -> Result<(), String> {
        let plain = |text: &str, max: usize, extra: &str| {
            !text.is_empty() && text.len() <= max && text.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
        };
        if !plain(&self.user, 254, "@._+-") || !self.user.contains('@') {
            return Err("Enter the full mailbox address".into());
        }
        if !plain(&self.host, 253, ".-") || self.host.starts_with(['-', '.']) {
            return Err("Enter the IMAP server name".into());
        }
        if self.port == 0 {
            return Err("Enter a port from 1 to 65535".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailStatus {
    pub settings: Option<EmailSettings>,
    pub password_saved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshHost {
    pub id: i64,
    pub name: String,
    pub target: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub policy: HostPolicy,
}

/// A host with whether its key has been pinned yet; agents cannot connect until it has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStatus {
    pub host: SshHost,
    pub trusted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SshOutcome {
    Ran,
    Declined,
    Refused,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshLogEntry {
    pub id: i64,
    pub at: i64,
    pub bot_id: Option<i64>,
    pub host: String,
    pub command: String,
    pub outcome: SshOutcome,
    pub exit_code: Option<i32>,
    pub millis: Option<i64>,
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
    fn hosts_must_not_be_readable_as_ssh_options() {
        let host = |name: &str, target: &str, user: Option<&str>, port| NewHost { name: name.into(), target: target.into(), user: user.map(Into::into), port, policy: HostPolicy::Ask };
        assert!(host("web-1", "10.0.0.5", Some("deploy"), Some(2222)).validate().is_ok());
        assert!(host("linode", "example.com", None, None).validate().is_ok());
        assert!(host("v6", "fe80::1", None, None).validate().is_ok());
        for bad in [
            host("", "h", None, None),
            host("-x", "h", None, None),
            host("a b", "h", None, None),
            host("a", "-oProxyCommand=sh", None, None),
            host("a", "h;rm", None, None),
            host("a", "h", Some("-l"), None),
            host("a", "h", Some("a b"), None),
            host("a", "h", None, Some(0)),
            host(&"n".repeat(33), "h", None, None),
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        assert_eq!(serde_json::to_string(&HostPolicy::ReadOnly).unwrap(), "\"read-only\"");
    }

    #[test]
    fn runtime_events_are_tagged_by_dotted_type() {
        let event = RuntimeEvent::ContentDelta { text: "hi".into() };
        assert_eq!(serde_json::to_string(&event).unwrap(), r#"{"type":"content.delta","text":"hi"}"#);
        assert_eq!(serde_json::from_str::<RuntimeEvent>(r#"{"type":"runtime.error","message":"x"}"#).unwrap(), RuntimeEvent::RuntimeError { message: "x".into() });
    }
}
