use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Valid permission prefixes from the Relux permission model.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 7.5 (Permission And Approval Layer)
/// and `docs/Relux spec.md` section 12.1 (Permission Philosophy).
///
/// Format: `<prefix>:<resource>:<action>`
pub const VALID_PREFIXES: &[&str] = &[
    "tool:",
    "adapter:",
    "provider:",
    "exec:",
    "plugin:",
    "agent:",
    "task:",
    "approval:",
    "audit:",
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PermissionError {
    #[error("permission string is empty")]
    Empty,
    #[error("permission '{0}' does not start with a canonical prefix (tool:, adapter:, provider:, exec:, plugin:, agent:, task:, approval:, audit:)")]
    InvalidPrefix(String),
}

/// A capability string that controls what an actor may do.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 7.5 and section 12.1.
/// Format: `<category>:<resource>:<action>` e.g. `tool:relux-tools-github:create_pr`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Permission(String);

impl Permission {
    /// Construct a `Permission`, validating the canonical prefix.
    pub fn new(s: impl Into<String>) -> Result<Self, PermissionError> {
        let s = s.into();
        if s.is_empty() {
            return Err(PermissionError::Empty);
        }
        if !VALID_PREFIXES.iter().any(|p| s.starts_with(p)) {
            return Err(PermissionError::InvalidPrefix(s));
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Exact-match comparison.
    pub fn matches_exact(&self, other: &Permission) -> bool {
        self.0 == other.0
    }
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// How dangerous a tool call or action is considered to be.
///
/// Spec ref: `docs/Relux spec.md` section 12.4 (Risk Levels).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// Whether an action requires a human approval gate before execution.
///
/// Spec ref: `docs/Relux spec.md` section 12.5 (Approval Rules).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRequirement {
    Never,
    Required,
    RequiredWhenRisk(RiskLevel),
}

/// A single callable tool exposed by a ToolSet plugin.
///
/// Spec ref: `docs/RELUX_MASTER_PLAN.md` section 8.2 (ToolSet Plugins) and
/// `docs/Relux spec.md` section 10.2 (ToolSet Plugin).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub risk: RiskLevel,
    pub permission: Permission,
    pub approval: ApprovalRequirement,
    pub timeout_secs: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_permissions_are_accepted() {
        for prefix in VALID_PREFIXES {
            let s = format!("{}resource:action", prefix);
            assert!(Permission::new(&s).is_ok(), "expected ok for {s}");
        }
    }

    #[test]
    fn empty_permission_is_rejected() {
        assert_eq!(Permission::new(""), Err(PermissionError::Empty));
    }

    #[test]
    fn invalid_prefix_is_rejected() {
        let err = Permission::new("fs:some:action").unwrap_err();
        assert!(matches!(err, PermissionError::InvalidPrefix(_)));
    }

    #[test]
    fn exact_match_works() {
        let a = Permission::new("tool:relux-tools-github:create_pr").unwrap();
        let b = Permission::new("tool:relux-tools-github:create_pr").unwrap();
        let c = Permission::new("tool:relux-tools-github:merge_pr").unwrap();
        assert!(a.matches_exact(&b));
        assert!(!a.matches_exact(&c));
    }
}
