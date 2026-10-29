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
}
