use serde::{Deserialize, Serialize};

use crate::namespace::NamespaceId;

/// The outcome of an audited action.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §9.10 (Audit Event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditResult {
    Success,
    Denied,
    Failed,
    Approved,
    Rejected,
}

/// An immutable record of an important system action.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` §9.10 (Audit Event).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub ts: String,
    pub actor_type: String,
    pub actor_id: String,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub namespace_id: Option<NamespaceId>,
    pub result: AuditResult,
    pub metadata: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_event_serializes_cleanly() {
        let event = AuditEvent {
            id: "audit_001".to_string(),
            ts: "2026-06-08T00:00:00Z".to_string(),
            actor_type: "agent".to_string(),
            actor_id: "code-agent".to_string(),
            action: "tool:relux-tools-github:create_pr".to_string(),
            target_type: Some("task".to_string()),
            target_id: Some("task_001".to_string()),
            namespace_id: Some(NamespaceId::new("engineering")),
            result: AuditResult::Success,
            metadata: serde_json::json!({ "pr": 42 }),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let back: AuditEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.result, AuditResult::Success);
        assert_eq!(back.actor_id, "code-agent");
    }
}
