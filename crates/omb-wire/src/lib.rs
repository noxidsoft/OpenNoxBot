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
