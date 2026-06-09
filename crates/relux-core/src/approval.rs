use serde::{Deserialize, Serialize};

use crate::namespace::NamespaceId;
use crate::permission::RiskLevel;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApprovalId(pub String);

impl ApprovalId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ApprovalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle of a human approval request.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.9 (Approval) and section 10.3 (Approval Rules).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
}

/// A human approval request raised when Prime (or an agent) proposes a risky
/// action it must not perform silently.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 9.9 (Approval) and section 10.3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: ApprovalId,
    pub requested_by: String,
    /// A human-readable rendering of the proposed action.
    pub action: String,
    pub reason: String,
    pub risk: RiskLevel,
    pub status: ApprovalStatus,
    pub approved_by: Option<String>,
    pub namespace_id: Option<NamespaceId>,
    pub created_at: String,
    pub resolved_at: Option<String>,
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_round_trips() {
        let approval = Approval {
            id: ApprovalId::new("appr_0001"),
            requested_by: "prime".to_string(),
            action: "grant tool:relux-tools-github:access to code-agent".to_string(),
            reason: "Granting a permission widens what an actor can do.".to_string(),
            risk: RiskLevel::High,
            status: ApprovalStatus::Pending,
            approved_by: None,
            namespace_id: Some(NamespaceId::new("workspace")),
            created_at: "2026-06-08T00:00:00Z".to_string(),
            resolved_at: None,
            note: None,
        };
        let json = serde_json::to_string(&approval).expect("serialize");
        let back: Approval = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.status, ApprovalStatus::Pending);
        assert_eq!(back.requested_by, "prime");
    }
}
